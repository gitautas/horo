mod commands;
mod dto;
mod error;
mod events;
mod matrix;
mod photos;
mod rtc;
mod update;
mod verification;
mod webrtc;

use matrix::core::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    install_crypto_provider();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                // The SDK is extremely chatty at debug level; keep our own
                // module at debug and everything else at info.
                "info,horo=debug,matrix_sdk=info,matrix_sdk_crypto=info".into()
            }),
        )
        .init();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init());

    // Not compiled on mobile at all (see Cargo.toml), so the registration has
    // to be gated too — `update_mode` reports `manual` there and the frontend
    // never reaches for these commands.
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init());

    // The mirror image: haptics only exist on a phone (see Cargo.toml).
    #[cfg(mobile)]
    let builder = builder.plugin(tauri_plugin_haptics::init());

    builder
        .manage(AppState::default())
        .register_asynchronous_uri_scheme_protocol("horo", handle_media_request)
        .invoke_handler(tauri::generate_handler![
            // auth
            commands::discover_homeserver,
            commands::login_password,
            commands::login_sso,
            commands::register_account,
            commands::request_register_email,
            commands::restore_session,
            commands::logout,
            commands::current_session,
            // rooms
            commands::get_rooms,
            commands::get_spaces,
            commands::get_members,
            commands::create_room,
            commands::join_room,
            commands::leave_room,
            commands::update_room,
            commands::get_room_permissions,
            commands::invite_user,
            commands::set_typing,
            commands::set_room_muted,
            commands::set_room_marked_unread,
            // image packs
            commands::get_image_packs,
            commands::get_all_image_packs,
            commands::edit_image_pack,
            commands::set_pack_everywhere,
            commands::get_pack_rooms,
            commands::create_personal_pack,
            // timeline
            commands::open_timeline,
            commands::close_timeline,
            commands::send_message,
            commands::send_sticker,
            commands::paginate_back,
            commands::toggle_reaction,
            commands::edit_message,
            commands::redact_event,
            commands::mark_room_read,
            commands::send_read_receipt,
            commands::send_attachment,
            commands::send_attachment_bytes,
            commands::get_pinned_events,
            commands::get_event_body,
            // presence
            commands::watch_presence,
            commands::get_presence,
            commands::set_own_presence,
            // media
            commands::get_profile,
            commands::set_profile,
            commands::get_user_context,
            commands::open_dm,
            commands::upload_media,
            commands::get_media_bytes,
            commands::save_media,
            // verification
            commands::request_verification,
            commands::accept_verification,
            commands::confirm_verification,
            commands::mismatch_verification,
            commands::cancel_verification,
            commands::get_own_devices,
            commands::verify_device,
            commands::get_recovery_status,
            commands::enable_recovery,
            commands::recover_with_key,
            // calls
            commands::join_call,
            commands::leave_call,
            commands::refresh_call_membership,
            commands::get_call_participants,
            commands::get_active_calls,
            // photos
            photos::photos_recent,
            photos::photos_export,
            // updates
            update::update_mode,
            update::update_available,
            update::latest_release,
            // webrtc
            webrtc::webrtc_diagnosis,
        ])
        .setup(|app| {
            // Read only by the platform-gated calls below; on iOS/Android
            // neither applies and the binding would otherwise look unused.
            let _ = &app;
            #[cfg(desktop)]
            install_settings_menu_item(app.handle())?;
            #[cfg(target_os = "linux")]
            webrtc::enable(app)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building horo")
        .run(|app, event| {
            // Give the SDK a chance to flush and release the SQLite store,
            // otherwise the next launch can find a locked database.
            if let tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit = event {
                let state = app.state::<AppState>();
                tauri::async_runtime::block_on(async {
                    if let Some(core) = state.take().await {
                        core.shutdown().await;
                    }
                });
            }
        });
}

/// Settle rustls on one crypto backend before anything can open a connection.
///
/// `rustls` normally works this out from its own crate features, but ours has
/// both `aws-lc-rs` and `ring` switched on by different dependencies, and it
/// treats that as ambiguous rather than picking one: the lookup returns `None`
/// and the caller panics. That panic lands on a tokio worker, so the symptom is
/// not a crash but a `Promise` that never settles — a spinner that spins for
/// ever, with the reason only in the log.
///
/// Naming the provider here removes the ambiguity for the whole process. It has
/// to happen before the first handshake, which in practice means before Tauri
/// starts anything. `install_default` fails only if a provider is already
/// installed, and that outcome is just as good as ours.
fn install_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// Put "settings…" in the app menu, with the shortcut every mac app uses.
///
/// This has to be a menu item rather than a `keydown` handler in the WebView.
/// On macOS a command-key combination is offered to the menu bar first, and one
/// no menu item claims is swallowed by the responder chain — it never reaches
/// the web content, so a JS listener for `cmd+,` simply never fires. Making it
/// a real menu item is also the honest thing: the shortcut is discoverable, and
/// it sits where a mac user looks for it.
///
/// The frontend keeps its own `ctrl+,` handler for Windows and Linux, where the
/// menu bar isn't in the way.
///
/// Desktop only: `tauri::menu` is `#[cfg(desktop)]`, and a phone has neither a
/// menu bar nor a shortcut to claim.
#[cfg(desktop)]
fn install_settings_menu_item(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};

    let settings = MenuItemBuilder::with_id("settings", "Settings…")
        .accelerator("CmdOrCtrl+,")
        .build(app)?;

    // Spelled out rather than inserted into `Menu::default()`: the default
    // menu's shape is not ours to depend on, and an insert that misses fails
    // silently, which is exactly how this was first written and why it didn't
    // work.
    //
    // Edit has to be here even though nothing in it is ours — on macOS, cut,
    // copy, paste and select-all only function because these menu items claim
    // their shortcuts.
    let app_menu = SubmenuBuilder::new(app, "horo")
        .about(None)
        .separator()
        .item(&settings)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let edit_menu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    // Zoom claims its shortcuts here on macOS for the same reason settings
    // does. Elsewhere the page's own `ctrl+=`/`ctrl+-` handler does it, and a
    // menu accelerator as well could step the zoom twice for one press.
    let view_menu = SubmenuBuilder::new(app, "View");
    #[cfg(target_os = "macos")]
    let view_menu = view_menu
        .item(&MenuItemBuilder::with_id("zoom-in", "Zoom In").accelerator("Cmd+=").build(app)?)
        .item(&MenuItemBuilder::with_id("zoom-out", "Zoom Out").accelerator("Cmd+-").build(app)?)
        .item(&MenuItemBuilder::with_id("zoom-reset", "Actual Size").accelerator("Cmd+0").build(app)?)
        .separator();
    let view_menu = view_menu.fullscreen().build()?;

    let window_menu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .separator()
        .close_window()
        .build()?;

    let menu = MenuBuilder::new(app)
        .items(&[&app_menu, &edit_menu, &view_menu, &window_menu])
        .build()?;
    app.set_menu(menu)?;

    app.on_menu_event(|app, event| {
        match event.id().as_ref() {
            "settings" => events::emit(app, events::EV_OPEN_SETTINGS, ()),
            "zoom-in" => events::emit(app, events::EV_ZOOM, "in"),
            "zoom-out" => events::emit(app, events::EV_ZOOM, "out"),
            "zoom-reset" => events::emit(app, events::EV_ZOOM, "reset"),
            _ => {}
        }
    });

    Ok(())
}

/// Serve `horo://media/<encoded-mxc>?w=&h=` from the Matrix media repository.
///
/// This runs for every avatar and inline image, so it must never block the main
/// thread — hence the asynchronous protocol handler and the spawned task.
fn handle_media_request(
    ctx: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    use tauri::http::{Response, StatusCode};

    let app = ctx.app_handle().clone();
    let uri = request.uri().to_string();
    let range_header = request
        .headers()
        .get("range")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    tauri::async_runtime::spawn(async move {
        let respond_error = |status: StatusCode, message: &str| {
            Response::builder()
                .status(status)
                .header("content-type", "text/plain")
                .body(message.as_bytes().to_vec())
                .unwrap_or_else(|_| Response::new(Vec::new()))
        };

        let Some(parsed) = matrix::media::parse_request(&uri) else {
            responder.respond(respond_error(StatusCode::BAD_REQUEST, "bad media request"));
            return;
        };

        let state = app.state::<AppState>();
        let Ok(core) = state.core().await else {
            responder.respond(respond_error(StatusCode::UNAUTHORIZED, "not signed in"));
            return;
        };

        match matrix::media::fetch(
            &core.client,
            &parsed.mxc,
            parsed.size,
            parsed.mime_hint.as_deref(),
        )
        .await
        {
            Ok(media) => {
                let total = media.bytes.len() as u64;

                // `<video>` and `<audio>` won't play from a source that doesn't
                // do byte ranges — WKWebView probes with `Range: bytes=0-1` and
                // abandons the load if it doesn't get a 206 back.
                let range = range_header
                    .as_deref()
                    .and_then(|header| matrix::media::parse_range(header, total));

                let builder = Response::builder()
                    .header("content-type", &media.mime)
                    .header("accept-ranges", "bytes")
                    // Matrix media is immutable per URI, so it can be cached
                    // aggressively; this is what keeps scrolling smooth.
                    .header("cache-control", "private, max-age=31536000, immutable")
                    .header(
                        "content-security-policy",
                        "default-src 'none'; style-src 'unsafe-inline'; sandbox",
                    );

                let response = match range {
                    Some(range) => {
                        let slice =
                            media.bytes[range.start as usize..=range.end as usize].to_vec();
                        builder
                            .status(StatusCode::PARTIAL_CONTENT)
                            .header(
                                "content-range",
                                format!("bytes {}-{}/{total}", range.start, range.end),
                            )
                            .header("content-length", range.len().to_string())
                            .body(slice)
                    }
                    None => builder
                        .status(StatusCode::OK)
                        .header("content-length", total.to_string())
                        .body(media.bytes),
                };

                responder
                    .respond(response.unwrap_or_else(|_| Response::new(Vec::new())));
            }
            Err(e) => {
                tracing::debug!("media fetch failed for {}: {e}", parsed.mxc);
                responder.respond(respond_error(StatusCode::NOT_FOUND, "media unavailable"));
            }
        }
    });
}
