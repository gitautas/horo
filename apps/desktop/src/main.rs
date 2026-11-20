//! horo on the desktop.

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,horo_ui=info,horo_core=info".into()),
        )
        .init();
    // Before any TLS: matrix-sdk links ring too, and with two providers
    // rustls won't pick one on its own.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    // While the Tauri app still exists, keep this one's session secrets out of
    // the keychain entry that app reads (same service, same user id), in the
    // data directory's 0600 file instead.
    horo_core::matrix::session::use_file_storage();

    let backend = horo_ui::Backend::new(horo_ui::Backend::default_data_dir()).expect("a tokio runtime");
    gpui_platform::application().run(|cx| {
        horo_ui::init(backend, cx);
        if let Err(e) = horo_ui::open_window(cx) {
            eprintln!("horo: couldn't open a window: {e}");
            cx.quit();
        }
    });
}
