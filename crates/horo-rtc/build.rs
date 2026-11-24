//! libvpx (VP8) for the `media` feature, built from `third_party/libvpx`
//! (a submodule pinned to v1.17.0, the version `src/video_ffi.rs` was
//! generated from) and linked statically, so the app carries its own and
//! runs on the macOS versions it supports, not only the build machine's.
//!
//! Only what calls need is built: VP8, real-time, no tools or tests. The
//! result is cached in Cargo's build directory; it's rebuilt when the
//! submodule moves or the target changes.
//!
//! On Windows (MSVC) libvpx's configure writes Visual Studio projects
//! instead of makefiles, as vcpkg's port uses it: configure and `make
//! dist` under bash (Git for Windows' or MSYS2's, with `make` and `nasm`
//! on the PATH), then MSBuild builds `vpxmd.lib` (the C runtime Rust links
//! by default).
//!
//! `HORO_SYSTEM_VPX=1` links the system's libvpx instead (through
//! pkg-config), for a quick local build without compiling libvpx.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=HORO_SYSTEM_VPX");
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
    if env::var_os("CARGO_FEATURE_MEDIA").is_none() {
        return;
    }
    if env::var_os("HORO_SYSTEM_VPX").is_some_and(|v| v != "0") {
        if let Err(e) = pkg_config::Config::new().atleast_version("1.13").probe("vpx") {
            panic!("HORO_SYSTEM_VPX is set but libvpx wasn't found ({e})");
        }
        return;
    }

    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/libvpx");
    if !source.join("configure").exists() {
        panic!("third_party/libvpx is empty: run `git submodule update --init --depth 1 third_party/libvpx` (or set HORO_SYSTEM_VPX=1)");
    }
    // Rebuild when the submodule moves to another version.
    println!("cargo:rerun-if-changed={}", source.join("CHANGELOG").display());

    let target = env::var("TARGET").unwrap();
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("libvpx");
    if target.ends_with("-windows-msvc") {
        let lib_dir = out.join(if target.starts_with("aarch64") { "ARM64" } else { "x64" }).join("Release");
        if !lib_dir.join("vpxmd.lib").exists() {
            build_msvc(&source, &out, &target);
        }
        println!("cargo:rustc-link-search=native={}", lib_dir.display());
        println!("cargo:rustc-link-lib=static=vpxmd");
        return;
    }
    if !out.join("libvpx.a").exists() {
        build(&source, &out, &target);
    }
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=vpx");
}

/// The configure, generate and MSBuild steps for Windows.
fn build_msvc(source: &Path, out: &Path, target: &str) {
    let (vpx_target, platform) = match target {
        "x86_64-pc-windows-msvc" => ("x86_64-win64-vs17", "x64"),
        "aarch64-pc-windows-msvc" => ("arm64-win64-vs17", "ARM64"),
        other => panic!("building libvpx for {other} isn't set up yet; set HORO_SYSTEM_VPX=1 to use an installed one"),
    };
    let _ = std::fs::remove_dir_all(out);
    std::fs::create_dir_all(out).unwrap();

    // bash wants `/c/…` rather than `C:\…`.
    let unix = |path: &Path| {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().replace('\\', "/");
        let path = path.trim_start_matches("//?/");
        match path.split_once(":/") {
            Some((drive, rest)) if drive.len() == 1 => format!("/{}/{rest}", drive.to_lowercase()),
            _ => path.to_owned(),
        }
    };
    let configure = format!(
        "{} --target={vpx_target} --as=nasm --enable-static --disable-shared --enable-vp8 --disable-vp9 --enable-realtime-only \
         --disable-examples --disable-tools --disable-docs --disable-unit-tests --disable-webm-io --disable-libyuv",
        unix(&source.join("configure")),
    );
    run(Command::new("bash").current_dir(out).args(["--noprofile", "--norc", "-c", &configure]), "configuring libvpx");
    // Writes vpx.vcxproj and the headers it needs.
    run(Command::new("bash").current_dir(out).args(["--noprofile", "--norc", "-c", "make dist"]), "generating libvpx's projects");

    let msbuild = cc::windows_registry::find_tool(target, "msbuild.exe").unwrap_or_else(|| panic!("MSBuild not found: install Visual Studio's C++ build tools"));
    run(
        msbuild
            .to_command()
            .current_dir(out)
            .arg("vpx.vcxproj")
            .args(["/m", "/nologo", "/p:Configuration=Release", &format!("/p:Platform={platform}")])
            // The project puts its output under $(SolutionDir), which only a
            // solution build sets: it's this directory.
            .arg(format!("/p:SolutionDir={}\\", out.display())),
        "building libvpx",
    );
}

/// libvpx's own name for a Rust target.
fn vpx_target(target: &str) -> &'static str {
    match target {
        "aarch64-apple-darwin" => "arm64-darwin20-gcc",
        "x86_64-apple-darwin" => "x86_64-darwin20-gcc",
        "aarch64-apple-ios" => "arm64-darwin-gcc",
        "aarch64-unknown-linux-gnu" => "arm64-linux-gcc",
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gcc",
        "aarch64-linux-android" => "arm64-android-gcc",
        other => panic!("building libvpx for {other} isn't set up yet; set HORO_SYSTEM_VPX=1 to use an installed one"),
    }
}

fn build(source: &Path, out: &Path, target: &str) {
    let _ = std::fs::remove_dir_all(out);
    std::fs::create_dir_all(out).unwrap();

    let mut cflags = vec!["-O2".to_owned()];
    if target.ends_with("apple-darwin") {
        // What the app supports, not what this Mac runs.
        let min = env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| "11.0".into());
        cflags.push(format!("-mmacosx-version-min={min}"));
    }

    let mut configure = Command::new(source.join("configure"));
    configure
        .current_dir(out)
        .arg(format!("--target={}", vpx_target(target)))
        .args([
            "--enable-static",
            "--disable-shared",
            "--enable-pic",
            "--enable-vp8",
            "--disable-vp9",
            "--enable-realtime-only",
            "--disable-examples",
            "--disable-tools",
            "--disable-docs",
            "--disable-unit-tests",
            "--disable-webm-io",
            "--disable-libyuv",
            "--disable-install-bins",
            "--disable-install-srcs",
        ])
        .arg(format!("--extra-cflags={}", cflags.join(" ")));
    if target.starts_with("x86_64") {
        configure.arg("--as=nasm");
    }
    run(&mut configure, "configuring libvpx");

    let jobs = env::var("NUM_JOBS").unwrap_or_else(|_| "4".into());
    run(Command::new("make").current_dir(out).arg(format!("-j{jobs}")).arg("libvpx.a"), "building libvpx");
}

fn run(command: &mut Command, what: &str) {
    let output = command.output().unwrap_or_else(|e| panic!("{what}: {e}"));
    if !output.status.success() {
        panic!("{what} failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
}
