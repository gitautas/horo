//! Link libvpx (VP8) for the `media` feature, found through pkg-config.
//!
//! For now that's whatever the system has (Homebrew's on a Mac, which
//! `src/video_ffi.rs` was generated from); a release build will carry its
//! own copy.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE_MEDIA").is_none() {
        return;
    }
    if let Err(e) = pkg_config::Config::new().atleast_version("1.13").probe("vpx") {
        panic!("libvpx not found ({e}); install it (e.g. `brew install libvpx`)");
    }
}
