//! What the core tells whoever is driving it, and the tasks that produce it.
//!
//! The core never talks to a UI directly. It pushes typed [`Event`]s into a
//! [`Host`], and the host decides what they become: the Tauri shell turns them
//! into `matrix://…` events for the WebView, a native UI can feed them straight
//! into its own state.

use std::{sync::Arc, time::Duration};

use tokio::task::JoinHandle;

use crate::{
    dto::{RoomsUpdate, SyncState, SyncStatus, TimelineUpdate, TypingUpdate, TypingUser},
    matrix::{MatrixCore, presence::PresenceUpdate},
    verification::{SasStateDto, VerificationRequestDto},
};

/// Everything the core reports on its own initiative, as opposed to in answer
/// to a call.
#[derive(Debug, Clone)]
pub enum Event {
    /// A batch of diffs to the room list, numbered so a snapshot can say which
    /// batches it already contains.
    Rooms(RoomsUpdate),
    /// Diffs to one open timeline.
    Timeline(TimelineUpdate),
    /// Who is typing in a room with an open timeline.
    Typing(TypingUpdate),
    /// Presence changes for watched users, or the server's support for it.
    Presence(PresenceUpdate),
    /// The sync service's state, for the connection indicator.
    SyncStatus(SyncStatus),
    /// Our own cross-signing state, lowercased from the SDK's
    /// `VerificationState` (`unknown`, `verified`, `unverified`).
    VerificationState(String),
    /// A verification request we have just learned about.
    VerificationRequest(VerificationRequestDto),
    /// A known verification request moved on.
    VerificationRequestUpdate(VerificationRequestDto),
    /// An emoji (SAS) verification moved on.
    SasUpdate(SasStateDto),
}

/// What the core needs from the app around it.
///
/// Implementations must be cheap to call from any tokio task: `emit` runs on
/// sync and timeline tasks, and anything slow there holds up the stream.
pub trait EventSink: Send + Sync + 'static {
    /// Deliver an event. Failure (the window is gone, say) is the host's to
    /// swallow: it must never take down the task that produced the event.
    fn emit(&self, event: Event);

    /// Open a URL in the system browser — SSO sign-in sends people there.
    fn open_url(&self, url: &str) -> Result<(), String>;
}

/// A shared handle to the app's [`EventSink`].
pub type Host = Arc<dyn EventSink>;

pub fn emit(host: &Host, event: Event) {
    host.emit(event);
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
pub fn spawn_sync_status_task(host: Host, core: Arc<MatrixCore>) -> JoinHandle<()> {
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

            emit(&host, Event::SyncStatus(SyncStatus { state: sync_state, message }));

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
pub fn spawn_verification_task(host: Host, core: Arc<MatrixCore>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let encryption = core.client.encryption();

        let host_for_state = host.clone();
        let state_task = tokio::spawn(async move {
            let mut states = encryption.verification_state();
            while let Some(state) = states.next().await {
                emit(
                    &host_for_state,
                    Event::VerificationState(format!("{state:?}").to_lowercase()),
                );
            }
        });

        crate::verification::watch_requests(host, core).await;
        state_task.abort();
    })
}

/// Typing notifications for one room. Started when a timeline opens and dropped
/// when it closes, so we're not subscribed to rooms nobody is looking at.
pub fn spawn_typing_task(
    host: Host,
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

            emit(&host, Event::Typing(TypingUpdate { room_id: room_id.to_string(), users }));
        }
    }))
}
