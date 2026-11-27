//! Installing a new version from inside the app: the release's
//! `latest.json` names a signed archive per platform, which
//! `cargo-packager-updater` downloads, checks against [`PUBKEY`] and puts
//! in place of this install.
//!
//! Only installs that can replace themselves do: a packaged `.app`, an
//! NSIS install, an AppImage. A `.deb`, a dev build or a binary run from
//! `target/` gets the release page instead.
//!
//! [`PUBKEY`]: horo_core::update::PUBKEY

use std::path::PathBuf;

use cargo_packager_updater::{Config, UpdaterBuilder, WindowsConfig, WindowsUpdateInstallMode};

/// The bundle identifier of the released app (and of the Tauri app it
/// replaces); dev bundles have their own, so they never update in place.
#[cfg(target_os = "macos")]
const RELEASE_IDENTIFIER: &str = "lt.uwu.uwum";

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
        // An AppImage runtime says where the image is; a .deb doesn't.
        std::env::var_os("APPIMAGE").is_some()
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
    let updater = UpdaterBuilder::new(current, config).timeout(std::time::Duration::from_secs(300)).build().map_err(|e| e.to_string())?;
    let update = updater.check().map_err(|e| e.to_string())?.ok_or("there's no newer version to install")?;
    // What the banner offered is what gets installed; a release published
    // in between waits for the next look.
    if update.version.trim_start_matches('v') != version {
        return Err(format!("horo {} was published meanwhile — look again", update.version));
    }
    update.download_and_install().map_err(|e| e.to_string())
}

/// Start the newly installed version and quit this one.
pub fn relaunch() -> ! {
    #[cfg(target_os = "macos")]
    if let Some(app) = bundle() {
        // `-n`: a new instance even though this one is still running.
        let _ = std::process::Command::new("open").arg("-n").arg(app).spawn();
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    if let Some(image) = std::env::var_os("APPIMAGE") {
        let _ = std::process::Command::new(image).spawn();
    }
    std::process::exit(0)
}
