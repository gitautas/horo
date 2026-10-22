//! Events pushed from Rust to the frontend, and the tasks that produce them.
//!
//! Names are namespaced with `matrix://` so they can't collide with Tauri's own
//! event traffic.

use std::{sync::Arc, time::Duration};

use tauri::{AppHandle, Emitter};
use tokio::task::JoinHandle;

use crate::{
    dto::{SyncState, SyncStatus, TypingUpdate, TypingUser},
    matrix::MatrixCore,
};

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

/// First wait before restarting a sync that stopped on an error, doubled on
/// each failure in a row up to [`SYNC_RETRY_MAX`].
const SYNC_RETRY_MIN: Duration = Duration::from_secs(2);
const SYNC_RETRY_MAX: Duration = Duration::from_secs(60);

/// Mirror the sync service's state so the UI can show a connection indicator,
/// and restart it when it stops on an error.
///
/// Offline mode only covers the network going away. Anything else (a 5xx from
/// the proxy, a sliding sync session the server dropped) leaves the service
/// stopped until someone calls `start()` again, and until then nothing new
/// arrives: no messages, no notifications, no incoming calls.
pub fn spawn_sync_status_task(app: AppHandle, core: Arc<MatrixCore>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut states = core.sync_service.state();
        let mut retry = SYNC_RETRY_MIN;

        loop {
            let state = states.next().await;
            let Some(state) = state else { break };

            use matrix_sdk_ui::sync_service::State;
            let (sync_state, message) = match &state {
                State::Idle => (SyncState::Idle, None),
                State::Running => (SyncState::Running, None),
                State::Terminated => (SyncState::Terminated, None),
                State::Error { .. } => (SyncState::Error, Some("sync stopped — retrying".to_owned())),
                State::Offline => (SyncState::Offline, Some("offline".to_owned())),
            };

            emit(
                &app,
                EV_SYNC_STATUS,
                SyncStatus { state: sync_state, message },
            );

            match state {
                State::Running => retry = SYNC_RETRY_MIN,
                State::Error(error) => {
                    tracing::warn!("sync stopped: {error}; restarting in {retry:?}");
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(SYNC_RETRY_MAX);
                    // Signing out stops the service on purpose; don't fight it.
                    if core.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    core.sync_service.start().await;
                }
                _ => {}
            }
        }
    })
}

/// Watch our own cross-signing state, and surface verification requests from
/// our other devices — the "new device wants in~" banner in the design.
pub fn spawn_verification_task(app: AppHandle, core: Arc<MatrixCore>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let encryption = core.client.encryption();

        let app_for_state = app.clone();
        let state_task = tokio::spawn(async move {
            let mut states = encryption.verification_state();
            while let Some(state) = states.next().await {
                emit(
                    &app_for_state,
                    EV_VERIFICATION_STATE,
                    serde_json::json!({ "state": format!("{state:?}").to_lowercase() }),
                );
            }
        });

        crate::verification::watch_requests(app, core).await;
        state_task.abort();
    })
}

/// Typing notifications for one room. Started when a timeline opens and dropped
/// when it closes, so we're not subscribed to rooms nobody is looking at.
pub fn spawn_typing_task(
    app: AppHandle,
    core: Arc<MatrixCore>,
    room_id: matrix_sdk::ruma::OwnedRoomId,
) -> Option<JoinHandle<()>> {
    let room = core.client.get_room(&room_id)?;
    let (_drop_guard, mut stream) = room.subscribe_to_typing_notifications();

    Some(tokio::spawn(async move {
        // Keep the subscription alive for as long as the task runs.
        let _drop_guard = _drop_guard;

        while let Ok(user_ids) = stream.recv().await {
            let own = core.client.user_id().map(|u| u.to_owned());

            let mut users = Vec::new();
            for user_id in user_ids {
                // The server echoes our own typing notice back; the UI never
                // wants to be told that it is typing.
                if own.as_deref() == Some(&user_id) {
                    continue;
                }
                let display_name = room
                    .get_member_no_sync(&user_id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|m| m.display_name().map(str::to_owned));
                users.push(TypingUser { user_id: user_id.to_string(), display_name });
            }

            emit(
                &app,
                EV_TYPING,
                TypingUpdate { room_id: room_id.to_string(), users },
            );
        }
    }))
}
