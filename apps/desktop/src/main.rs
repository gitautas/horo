//! horo on the desktop.

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,horo_ui=info,horo_core=info,horo_rtc=info".into()),
        )
        .init();
    // Before any TLS: matrix-sdk links ring too, and with two providers
    // rustls won't pick one on its own.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    // The installed app keeps its session secrets in the keychain, where the
    // Tauri app it replaces left them. A build pointed at other data (dev
    // builds are) keeps them in that directory's 0600 file instead, so it
    // never reads or overwrites the installed app's entry (same service,
    // same user id).
    if std::env::var_os("HORO_DATA").is_some() {
        horo_core::matrix::session::use_file_storage();
    }

    let backend = horo_ui::Backend::new(horo_ui::Backend::default_data_dir()).expect("a tokio runtime");
    gpui_platform::application().with_assets(horo_ui::assets()).run(|cx| {
        horo_ui::init(backend, cx);
        if let Err(e) = horo_ui::open_window(cx) {
            eprintln!("horo: couldn't open a window: {e}");
            cx.quit();
        }
    });
}
