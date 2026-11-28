//! Installing a new version from inside the app: the release's
//! `latest.json` names a signed archive per platform, which
//! `cargo-packager-updater` downloads, checks against [`PUBKEY`] and puts
//! in place of this install.
//!
//! Only installs that can replace themselves do: a packaged `.app`, an
//! NSIS install, a writable AppImage, or a `.deb` install (via `pkexec`).
//! Dev builds, Flatpak/AUR installs, or read-only AppImages get the release
//! page instead.
//!
//! [`PUBKEY`]: horo_core::update::PUBKEY

use std::path::{Path, PathBuf};

use cargo_packager_updater::{Config, UpdaterBuilder, WindowsConfig, WindowsUpdateInstallMode};

/// The bundle identifier of the released app (and of the Tauri app it
/// replaces); dev bundles have their own, so they never update in place.
#[cfg(target_os = "macos")]
const RELEASE_IDENTIFIER: &str = "lt.uwu.uwum";

/// How this Linux build was installed (detected once and cached, since
/// [`installable`] is queried during UI rendering).
#[cfg(not(any(target_os = "macos", windows)))]
#[derive(Debug, Clone)]
enum LinuxInstall {
    /// Running from a writable AppImage at its canonical path (symlinks resolved).
    AppImage(PathBuf),
    /// Installed via dpkg/apt (`.deb`) with `pkexec` available for elevation.
    Deb { exe: PathBuf },
    /// Dev build, read-only AppImage, Flatpak, AUR, etc.
    External,
}

#[cfg(not(any(target_os = "macos", windows)))]
fn linux_install() -> &'static LinuxInstall {
    static INSTALL: std::sync::OnceLock<LinuxInstall> = std::sync::OnceLock::new();
    INSTALL.get_or_init(detect_linux_install)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn detect_linux_install() -> LinuxInstall {
    // 1. AppImage: resolve symlinks and verify the parent directory is writable.
    if let Some(raw) = std::env::var_os("APPIMAGE") {
        let path = PathBuf::from(raw);
        let canonical = std::fs::canonicalize(&path).unwrap_or(path);
        if canonical.is_file() && canonical.parent().is_some_and(dir_is_writable) {
            return LinuxInstall::AppImage(canonical);
        }
        return LinuxInstall::External;
    }

    // 2. Debian/Ubuntu `.deb`: executable owned by dpkg, with `pkexec` available.
    if let Ok(exe) = std::env::current_exe()
        && let Ok(exe) = std::fs::canonicalize(&exe).or(Ok::<PathBuf, std::io::Error>(exe))
        && has_binary("pkexec")
        && (has_binary("apt-get") || has_binary("dpkg"))
        && dpkg_owns(&exe)
    {
        return LinuxInstall::Deb { exe };
    }

    LinuxInstall::External
}

#[cfg(not(any(target_os = "macos", windows)))]
fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".horo-update-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn has_binary(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn dpkg_owns(exe: &Path) -> bool {
    std::process::Command::new("dpkg")
        .arg("-S")
        .arg(exe)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Whether this copy of horo can install an update over itself.
pub fn installable() -> bool {
    #[cfg(target_os = "macos")]
    {
        // …/horo.app/Contents/MacOS/horo, with the release identifier.
        bundle().is_some_and(|app| {
            std::fs::read_to_string(app.join("Contents/Info.plist"))
                .is_ok_and(|plist| plist.contains(&format!("<string>{RELEASE_IDENTIFIER}</string>")))
        })
    }
    #[cfg(windows)]
    {
        // The NSIS installer leaves its uninstaller beside the binary.
        std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join("uninstall.exe").is_file())).unwrap_or(false)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        !matches!(linux_install(), LinuxInstall::External)
    }
}

/// The `.app` this binary runs from.
#[cfg(target_os = "macos")]
fn bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let app = exe.parent()?.parent()?.parent()?;
    (app.extension()? == "app").then(|| app.to_path_buf())
}

/// Download `version` and put it in place of this install. Blocking: run it
/// off the UI thread. On Windows the installer takes over and this process
/// exits (the installer starts horo again); elsewhere it returns, and
/// [`relaunch`] starts the new version.
pub fn install(version: &str) -> Result<(), String> {
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION")).map_err(|e| e.to_string())?;
    let config = Config {
        endpoints: vec![horo_core::update::MANIFEST.parse().map_err(|e: url::ParseError| e.to_string())?],
        pubkey: horo_core::update::PUBKEY.to_owned(),
        windows: Some(WindowsConfig { installer_args: None, install_mode: Some(WindowsUpdateInstallMode::Passive) }),
    };

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        return install_linux(current, config, version);
    }

    #[cfg(any(target_os = "macos", windows))]
    {
        let updater = UpdaterBuilder::new(current, config).timeout(std::time::Duration::from_secs(300)).build().map_err(|e| e.to_string())?;
        let update = updater.check().map_err(|e| e.to_string())?.ok_or("there's no newer version to install")?;
        // What the banner offered is what gets installed; a release published
        // in between waits for the next look.
        if update.version.trim_start_matches('v') != version {
            return Err(format!("horo {} was published meanwhile — look again", update.version));
        }
        update.download_and_install().map_err(|e| e.to_string())
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn install_linux(current: semver::Version, config: Config, version: &str) -> Result<(), String> {
    match linux_install() {
        LinuxInstall::AppImage(path) => {
            let updater = UpdaterBuilder::new(current, config).timeout(std::time::Duration::from_secs(300)).build().map_err(|e| e.to_string())?;
            let update = updater.check().map_err(|e| e.to_string())?.ok_or("there's no newer version to install")?;
            if update.version.trim_start_matches('v') != version {
                return Err(format!("horo {} was published meanwhile — look again", update.version));
            }
            // Download + verify minisign signature, then atomically replace the
            // canonical AppImage in its own directory (same mount point).
            let bytes = update.download().map_err(|e| e.to_string())?;
            atomic_replace_executable(path, &bytes)
        }
        LinuxInstall::Deb { .. } => {
            let target = match std::env::consts::ARCH {
                "x86_64" => "linux-x86_64-deb",
                "aarch64" => "linux-aarch64-deb",
                other => return Err(format!("unsupported architecture for .deb updates: {other}")),
            };
            let updater = UpdaterBuilder::new(current, config)
                .target(target)
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .map_err(|e| e.to_string())?;
            let update = updater
                .check()
                .map_err(|e| format!("this release has no signed .deb update yet — download it from GitHub ({e})"))?
                .ok_or("there's no newer version to install")?;
            if update.version.trim_start_matches('v') != version {
                return Err(format!("horo {} was published meanwhile — look again", update.version));
            }
            let bytes = update.download().map_err(|e| e.to_string())?;
            install_deb_bytes(version, &bytes)
        }
        LinuxInstall::External => Err("this Linux build must be updated where you installed it".to_owned()),
    }
}

/// Atomically replace `target` with `bytes` using a sibling temp file on the
/// same filesystem, preserving executable permissions.
#[cfg(unix)]
fn atomic_replace_executable(target: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let parent = target.parent().ok_or_else(|| format!("{} has no parent directory", target.display()))?;
    let existing_mode = std::fs::metadata(target).map(|m| m.permissions().mode()).unwrap_or(0o755);
    let mode = existing_mode | 0o700;

    let tmp_path = parent.join(format!(".horo-update-{}.tmp", std::process::id()));
    let write_res = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&tmp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(mode))?;
        std::fs::rename(&tmp_path, target)?;
        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("couldn't replace {}: {err}", target.display()));
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn install_deb_bytes(version: &str, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let deb_path = std::env::temp_dir().join(format!("horo-update-{version}-{}.deb", std::process::id()));
    let write_res = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&deb_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        // World-readable so apt's unprivileged `_apt` user can read the local .deb.
        std::fs::set_permissions(&deb_path, std::fs::Permissions::from_mode(0o644))?;
        Ok(())
    })();
    if let Err(err) = write_res {
        let _ = std::fs::remove_file(&deb_path);
        return Err(format!("couldn't write temporary .deb: {err}"));
    }

    let output = if has_binary("apt-get") {
        std::process::Command::new("pkexec")
            .args(["apt-get", "install", "-y", "--allow-downgrades"])
            .arg(&deb_path)
            .output()
    } else {
        std::process::Command::new("pkexec").args(["dpkg", "-i"]).arg(&deb_path).output()
    };

    let _ = std::fs::remove_file(&deb_path);

    let output = output.map_err(|e| format!("couldn't launch pkexec: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if stderr.is_empty() {
            Err(format!("package installation failed ({})", output.status))
        } else {
            Err(stderr)
        }
    }
}

/// Remove colon-separated entries rooted at `prefix` (e.g. an old AppImage's
/// `$APPDIR` mount point) from a path-list env var. Returns `None` if nothing
/// remains.
#[allow(dead_code)]
fn strip_appdir_entries(value: &str, appdir: &str) -> Option<String> {
    let prefix = appdir.trim_end_matches('/');
    if prefix.is_empty() {
        return Some(value.to_owned());
    }
    let kept: Vec<&str> = value
        .split(':')
        .filter(|entry| {
            let trimmed = entry.trim_end_matches('/');
            !entry.is_empty() && trimmed != prefix && !entry.starts_with(&format!("{prefix}/"))
        })
        .collect();
    if kept.is_empty() { None } else { Some(kept.join(":")) }
}

/// Scrub inherited AppImage mount variables so a relaunched AppImage mounts
/// its own new SquashFS cleanly instead of inheriting the old unmounting one.
#[cfg(not(any(target_os = "macos", windows)))]
fn scrub_appimage_env(cmd: &mut std::process::Command) {
    let appdir = std::env::var("APPDIR").ok();
    for var in ["APPDIR", "APPIMAGE", "ARGV0", "OWD", "APPIMAGE_EXTRACT_AND_RUN"] {
        cmd.env_remove(var);
    }
    if let Some(appdir) = appdir {
        for var in ["LD_LIBRARY_PATH", "PATH", "XDG_DATA_DIRS", "GSETTINGS_SCHEMA_DIR", "QT_PLUGIN_PATH"] {
            if let Ok(val) = std::env::var(var) {
                match strip_appdir_entries(&val, &appdir) {
                    Some(cleaned) => {
                        cmd.env(var, cleaned);
                    }
                    None => {
                        cmd.env_remove(var);
                    }
                }
            }
        }
    }
}

/// Start the newly installed version and quit this one.
pub fn relaunch() -> ! {
    #[cfg(target_os = "macos")]
    if let Some(app) = bundle() {
        // `-n`: a new instance even though this one is still running.
        let _ = std::process::Command::new("open").arg("-n").arg(app).spawn();
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    match linux_install() {
        LinuxInstall::AppImage(image) => {
            let mut cmd = std::process::Command::new(image);
            scrub_appimage_env(&mut cmd);
            cmd.stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            let _ = cmd.spawn();
        }
        LinuxInstall::Deb { exe } => {
            let mut cmd = std::process::Command::new(exe);
            cmd.stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            let _ = cmd.spawn();
        }
        LinuxInstall::External => {}
    }
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn strip_appdir_entries_cleans_mount_paths() {
        let appdir = "/tmp/.mount_horoABC123";
        assert_eq!(
            strip_appdir_entries("/tmp/.mount_horoABC123/usr/lib:/usr/local/lib:/tmp/.mount_horoABC123/lib", appdir),
            Some("/usr/local/lib".to_owned()),
        );
        assert_eq!(strip_appdir_entries("/tmp/.mount_horoABC123/usr/lib", appdir), None);
        assert_eq!(strip_appdir_entries("/usr/bin:/bin", appdir), Some("/usr/bin:/bin".to_owned()));
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replace_updates_bytes_and_keeps_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("horo-updater-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("horo.AppImage");
        std::fs::write(&target, b"old-appimage").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();

        atomic_replace_executable(&target, b"new-appimage").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new-appimage");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Confirm `cargo-packager-updater` accepts the dual-updater `latest.json`
    /// written by `.github/workflows/release.yml` (with both `darwin-*` for
    /// Tauri installs, `macos-*` for gpui installs, and `linux-*-deb` for
    /// Debian/Ubuntu installs). When `HORO_UPDATER_TEST_DIR` points at a
    /// downloaded CI artifact directory, also downloads and verifies its real
    /// `.app.tar.gz` + `.sig` against `horo_core::update::PUBKEY`.
    #[test]
    fn manifest_and_signature_verify() {
        let test_dir = std::env::var_os("HORO_UPDATER_TEST_DIR").map(PathBuf::from);
        let (archive_bytes, sig) = match &test_dir {
            Some(dir) => (
                std::fs::read(dir.join("horo_0.11.0_aarch64.app.tar.gz")).expect("archive"),
                std::fs::read_to_string(dir.join("horo_0.11.0_aarch64.app.tar.gz.sig")).expect("sig"),
            ),
            None => (Vec::new(), "dummy-sig".to_owned()),
        };

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let manifest = serde_json::json!({
            "version": "0.11.0",
            "notes": "test release",
            "pub_date": "2026-11-27T00:00:00Z",
            "platforms": {
                "macos-aarch64": { "url": format!("http://{addr}/bundle.app.tar.gz"), "signature": sig, "format": "app" },
                "darwin-aarch64": { "url": format!("http://{addr}/bundle.app.tar.gz"), "signature": sig, "format": "app" },
                "macos-x86_64": { "url": format!("http://{addr}/bundle.app.tar.gz"), "signature": sig, "format": "app" },
                "darwin-x86_64": { "url": format!("http://{addr}/bundle.app.tar.gz"), "signature": sig, "format": "app" },
                "linux-x86_64": { "url": format!("http://{addr}/bundle.AppImage"), "signature": sig, "format": "appimage" },
                "linux-aarch64": { "url": format!("http://{addr}/bundle.AppImage"), "signature": sig, "format": "appimage" },
                "linux-x86_64-deb": { "url": format!("http://{addr}/bundle_amd64.deb"), "signature": sig, "format": "appimage" },
                "linux-aarch64-deb": { "url": format!("http://{addr}/bundle_arm64.deb"), "signature": sig, "format": "appimage" },
                "windows-x86_64": { "url": format!("http://{addr}/bundle-setup.exe"), "signature": sig, "format": "nsis" },
            }
        })
        .to_string();

        let requests = if test_dir.is_some() { 2 } else { 1 };
        let server = std::thread::spawn(move || {
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 1024];
                let n = stream.read(&mut buf).unwrap();
                let req = String::from_utf8_lossy(&buf[..n]);
                let (ctype, body): (&str, &[u8]) = if req.starts_with("GET /latest.json") {
                    ("application/json", manifest.as_bytes())
                } else {
                    ("application/octet-stream", &archive_bytes)
                };
                let header = format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(body).unwrap();
            }
        });

        let config = Config {
            endpoints: vec![format!("http://{addr}/latest.json").parse().unwrap()],
            pubkey: horo_core::update::PUBKEY.to_owned(),
            windows: Some(WindowsConfig { installer_args: None, install_mode: Some(WindowsUpdateInstallMode::Passive) }),
        };
        let updater = UpdaterBuilder::new(semver::Version::new(0, 10, 0), config).build().expect("builder");
        let update = updater.check().expect("check").expect("update available");
        assert_eq!(update.version, "0.11.0");
        if test_dir.is_some() {
            let bytes = update.download().expect("download & minisign signature verification");
            assert!(!bytes.is_empty());
        }
        server.join().unwrap();
    }
}
