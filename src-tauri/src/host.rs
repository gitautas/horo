//! The Tauri side of [`horo_core::EventSink`]: core events become `matrix://…`
//! events for the WebView, and URLs open through the opener plugin.
//!
//! Event names and payload shapes are the wire contract with the frontend
//! (`src/lib/ipc.ts`), so they must not change with the core's internals.

use std::sync::Arc;

use horo_core::{Event, EventSink, Host};
use tauri::{AppHandle, Emitter};
use tauri_plugin_opener::OpenerExt;

pub const EV_ROOMS: &str = "matrix://rooms";
pub const EV_TIMELINE: &str = "matrix://timeline";
pub const EV_TYPING: &str = "matrix://typing";
pub const EV_PRESENCE: &str = "matrix://presence";
pub const EV_SYNC_STATUS: &str = "matrix://sync-status";
pub const EV_VERIFICATION_STATE: &str = "matrix://verification-state";
pub const EV_VERIFICATION_REQUEST: &str = "matrix://verification-request";
pub const EV_VERIFICATION_UPDATE: &str = "matrix://verification-update";

/// The menu bar asking the UI to open settings. Not a Matrix event — it comes
/// from the window, so it gets its own namespace.
pub const EV_OPEN_SETTINGS: &str = "horo://open-settings";

/// The View menu's zoom items, with `"in"`, `"out"` or `"reset"`.
pub const EV_ZOOM: &str = "horo://zoom";

pub fn emit<T: serde::Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
    // A failed emit means the window is gone; there's nothing useful to do
    // about it and it must not take down the producing task.
    if let Err(e) = app.emit(event, payload) {
        tracing::debug!("emit {event} failed: {e}");
    }
}

struct TauriHost(AppHandle);

impl EventSink for TauriHost {
    fn emit(&self, event: Event) {
        let app = &self.0;
        match event {
            Event::Rooms(update) => emit(app, EV_ROOMS, update),
            Event::Timeline(update) => emit(app, EV_TIMELINE, update),
            Event::Typing(update) => emit(app, EV_TYPING, update),
            Event::Presence(update) => emit(app, EV_PRESENCE, update),
            Event::SyncStatus(status) => emit(app, EV_SYNC_STATUS, status),
            Event::VerificationState(state) => {
                emit(app, EV_VERIFICATION_STATE, serde_json::json!({ "state": state }))
            }
            Event::VerificationRequest(request) => emit(app, EV_VERIFICATION_REQUEST, request),
            Event::VerificationRequestUpdate(request) => {
                emit(app, EV_VERIFICATION_UPDATE, request)
            }
            Event::SasUpdate(sas) => emit(app, EV_VERIFICATION_UPDATE, sas),
        }
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        self.0.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
    }
}

/// The core's view of this app.
pub fn host(app: &AppHandle) -> Host {
    Arc::new(TauriHost(app.clone()))
}
