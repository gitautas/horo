//! Who's online, and when they were last seen.
//!
//! Presence is an EDU, and EDUs only arrive over legacy `/sync` — sliding sync
//! has no presence extension in this SDK, and `SyncService` is the only sync
//! we run. So this polls `GET /_matrix/client/v3/presence/{user}/status`
//! instead, for the people the UI is actually drawing, and pushes what changed.
//!
//! The frontend declares who it cares about (`watch`), which is the set of
//! people on screen: the open room's member list, DM partners in the sidebar,
//! whoever's profile card is open. Polling every member of every joined room
//! would be hundreds of requests a minute for pixels nobody is looking at.
//!
//! Presence is also *optional*, and plenty of homeservers have it switched off
//! — Synapse ships with `presence.enabled: false` in a lot of deployments. A
//! server that says nothing gets no indicator at all rather than a wall of
//! grey "offline" dots, which would be a confident lie.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use futures_util::{StreamExt, stream};
use matrix_sdk::ruma::{
    OwnedUserId, UserId,
    api::client::presence::{get_presence, set_presence},
    presence::PresenceState,
};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{Mutex, Notify},
    task::JoinHandle,
};
use ts_rs::TS;

use crate::{
    error::Result,
    events::{self, Event, Host},
    matrix::MatrixCore,
};

/// How often the watched set is re-read from the server.
///
/// Presence isn't worth a tighter loop than this: the server only learns about
/// someone going idle on their own client's timer, so sub-minute precision is
/// imaginary anyway.
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// How often our own presence is re-asserted while the app is open.
///
/// Servers expire presence after a few minutes of silence, so "online" has to
/// be renewed or we'd quietly go offline while the user sits reading.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);

/// How long to wait before trying again once the server has told us presence
/// isn't a thing here. Long, because the answer almost never changes — but not
/// never, because it changes when an admin flips a config flag.
const UNSUPPORTED_RETRY: Duration = Duration::from_secs(15 * 60);

/// At most this many presence requests in flight at once.
const CONCURRENCY: usize = 8;

/// One person's presence, as the UI draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, rename = "Presence")]
#[serde(rename_all = "camelCase")]
pub struct PresenceDto {
    pub user_id: String,
    pub presence: Availability,
    /// The free-text status the *presence* system carries. Distinct from the
    /// MSC4133 profile status in `profile.rs`, which is the one people set by
    /// hand and the one the profile card shows.
    pub status_msg: Option<String>,
    /// When they were last active, as a unix timestamp in milliseconds.
    ///
    /// The server answers with an *age*, which stops being true the moment it
    /// arrives, so it's turned into an absolute instant here — a "last seen"
    /// label has to keep counting between polls.
    pub last_active: Option<u64>,
    /// The server's own "they're actively doing something right now" flag.
    pub currently_active: bool,
}

/// The spec's three presence states, and a fourth for anything else a server
/// sends — which the UI draws as nothing rather than guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum Availability {
    Online,
    Unavailable,
    Offline,
    Unknown,
}

impl From<&PresenceState> for Availability {
    fn from(state: &PresenceState) -> Self {
        match state {
            PresenceState::Online => Self::Online,
            PresenceState::Unavailable => Self::Unavailable,
            PresenceState::Offline => Self::Offline,
            _ => Self::Unknown,
        }
    }
}

/// A batch of changes, plus whether presence works here at all.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PresenceUpdate {
    pub users: Vec<PresenceDto>,
    /// False once the server has refused every request in a round: the UI hides
    /// its indicators rather than claiming everyone is offline.
    pub supported: bool,
}

/// What the frontend can ask us to publish about the user.
#[derive(Debug, Clone, Copy, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum OwnPresence {
    Online,
    Unavailable,
    Offline,
}

impl From<OwnPresence> for PresenceState {
    fn from(value: OwnPresence) -> Self {
        match value {
            OwnPresence::Online => PresenceState::Online,
            OwnPresence::Unavailable => PresenceState::Unavailable,
            OwnPresence::Offline => PresenceState::Offline,
        }
    }
}

/// The presence half of a session: who we're watching, what we last saw, and
/// what we're publishing about ourselves.
pub struct PresenceHub {
    /// The people the UI is drawing right now. Replaced wholesale by `watch`.
    watched: Mutex<HashSet<OwnedUserId>>,
    /// The last value pushed to the frontend, so only changes are emitted.
    known: Mutex<HashMap<OwnedUserId, PresenceDto>>,
    /// What we tell the server about ourselves, and whether it's been sent yet.
    own: Mutex<OwnState>,
    /// Rung when the watched set or our own state changes, so the poller acts
    /// immediately instead of at the end of its nap.
    wake: Notify,
}

struct OwnState {
    state: PresenceState,
    /// Set when `state` changes, cleared once the server has been told.
    dirty: bool,
}

impl Default for PresenceHub {
    fn default() -> Self {
        Self {
            watched: Mutex::new(HashSet::new()),
            known: Mutex::new(HashMap::new()),
            // Opening the app is being online; nothing else has to happen for
            // that to be true, so it starts dirty and is published on the first
            // pass of the poll task.
            own: Mutex::new(OwnState { state: PresenceState::Online, dirty: true }),
            wake: Notify::new(),
        }
    }
}

impl PresenceHub {
    /// Forget everyone we're no longer watching, so a long session doesn't
    /// accumulate presence for every person it has ever drawn.
    async fn prune(&self, watched: &HashSet<OwnedUserId>) {
        self.known.lock().await.retain(|user_id, _| watched.contains(user_id));
    }
}

/// Replace the set of people whose presence the UI wants.
///
/// Unparseable IDs are skipped rather than failing the call: the set is
/// assembled from whatever is on screen, and one odd member shouldn't cost the
/// rest of the room its dots.
pub async fn watch(core: &MatrixCore, user_ids: Vec<String>) -> Result<()> {
    let next: HashSet<OwnedUserId> =
        user_ids.iter().filter_map(|id| UserId::parse(id).ok()).collect();

    {
        let mut watched = core.presence.watched.lock().await;
        if *watched == next {
            return Ok(());
        }
        *watched = next;
    }

    core.presence.wake.notify_one();
    Ok(())
}

/// Publish our own presence — the frontend's idle timer drives this.
pub async fn set_own(core: &MatrixCore, presence: OwnPresence) -> Result<()> {
    let state: PresenceState = presence.into();

    {
        let mut own = core.presence.own.lock().await;
        if own.state == state {
            return Ok(());
        }
        own.state = state;
        own.dirty = true;
    }

    core.presence.wake.notify_one();
    Ok(())
}

/// Everything we currently know, for a frontend that has just mounted and
/// missed the pushes so far. Mirrors `get_rooms` and exists for the same
/// reason.
pub async fn snapshot(core: &MatrixCore) -> Result<Vec<PresenceDto>> {
    Ok(core.presence.known.lock().await.values().cloned().collect())
}

/// Poll the watched set, push what changed, and keep our own presence alive.
pub fn spawn_presence_task(host: Host, core: Arc<MatrixCore>) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Presence support is discovered rather than advertised — there's no
        // capability flag for it — so we start optimistic and find out.
        let mut supported = true;
        let mut heartbeat = tokio::time::Instant::now() - HEARTBEAT_INTERVAL;

        loop {
            if core.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }

            let due = heartbeat.elapsed() >= HEARTBEAT_INTERVAL;
            let dirty = core.presence.own.lock().await.dirty;
            if supported && (due || dirty) {
                let state = {
                    let mut own = core.presence.own.lock().await;
                    own.dirty = false;
                    own.state.clone()
                };
                if publish_own(&core, state).await {
                    heartbeat = tokio::time::Instant::now();
                }
            }

            let watched: HashSet<OwnedUserId> = core.presence.watched.lock().await.clone();

            if !watched.is_empty() {
                let (fetched, failures) = fetch_all(&core, &watched).await;

                // Every single request refused is the shape of a server with
                // presence switched off. One failure among several is just a
                // user we can't see, and says nothing about the server.
                let now_supported = fetched.len() > failures;
                if now_supported != supported {
                    supported = now_supported;
                    events::emit(
                        &host,
                        Event::Presence(PresenceUpdate { users: Vec::new(), supported }),
                    );
                }

                if supported {
                    core.presence.prune(&watched).await;

                    let mut changed = Vec::new();
                    {
                        let mut known = core.presence.known.lock().await;
                        for entry in fetched {
                            let Ok(user_id) = UserId::parse(&entry.user_id) else { continue };
                            if known.get(&user_id) == Some(&entry) {
                                continue;
                            }
                            known.insert(user_id, entry.clone());
                            changed.push(entry);
                        }
                    }

                    if !changed.is_empty() {
                        events::emit(
                            &host,
                            Event::Presence(PresenceUpdate { users: changed, supported: true }),
                        );
                    }
                }
            }

            let nap = if supported { POLL_INTERVAL } else { UNSUPPORTED_RETRY };

            // Either the clock or the UI changing its mind — whichever comes
            // first. A new room's member list shouldn't wait out the interval
            // to get its dots.
            tokio::select! {
                _ = tokio::time::sleep(nap) => {}
                _ = core.presence.wake.notified() => {}
            }
        }
    })
}

/// Ask for everyone at once, bounded. Returns what came back and how many
/// requests the server refused.
async fn fetch_all(
    core: &MatrixCore,
    watched: &HashSet<OwnedUserId>,
) -> (Vec<PresenceDto>, usize) {
    let results: Vec<Option<PresenceDto>> = stream::iter(watched.iter().cloned())
        .map(|user_id| async move { fetch_one(core, user_id).await })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;

    let failures = results.iter().filter(|r| r.is_none()).count();
    (results.into_iter().flatten().collect(), failures)
}

async fn fetch_one(core: &MatrixCore, user_id: OwnedUserId) -> Option<PresenceDto> {
    let response = core
        .client
        .send(get_presence::v3::Request::new(user_id.clone()))
        .await
        .ok()?;

    // `last_active_ago` is an age at the moment the server wrote it; anchor it
    // to now so the label can keep counting on its own.
    let last_active = response.last_active_ago.and_then(|ago| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis() as u64;
        Some(now.saturating_sub(ago.as_millis() as u64))
    });

    Some(PresenceDto {
        user_id: user_id.to_string(),
        presence: (&response.presence).into(),
        status_msg: response.status_msg.filter(|s| !s.is_empty()),
        last_active,
        currently_active: response.currently_active.unwrap_or(false),
    })
}

/// Tell the server what we are. Returns whether it took it.
async fn publish_own(core: &MatrixCore, state: PresenceState) -> bool {
    let Ok(user_id) = core.own_user_id() else { return false };

    match core.client.send(set_presence::v3::Request::new(user_id, state)).await {
        Ok(_) => true,
        Err(e) => {
            // A server with presence disabled answers here too. Nothing the
            // user can do about it, and the poll below will work out that the
            // whole feature is off.
            tracing::debug!("couldn't publish presence: {e}");
            false
        }
    }
}

/// Best-effort "we're gone" on the way out.
///
/// Bounded tightly because it sits on the shutdown path: a homeserver that
/// hangs must not hold the window open, and the server expires us on its own
/// timer regardless.
pub async fn go_offline(core: &MatrixCore) {
    let Ok(user_id) = core.own_user_id() else { return };
    let request = core.client.send(set_presence::v3::Request::new(user_id, PresenceState::Offline));
    let _ = tokio::time::timeout(Duration::from_secs(2), request).await;
}

/// The user this DM is with, if it's unambiguous.
///
/// Used to hang a presence dot off a sidebar row. A "direct" room can have any
/// number of people in it — the flag is account data, not a headcount — so
/// anything other than exactly one target has no one person to represent.
pub fn dm_partner(room: &matrix_sdk::Room) -> Option<OwnedUserId> {
    let targets = room.direct_targets();
    if targets.len() != 1 {
        return None;
    }
    let partner = targets.iter().next()?.as_user_id()?;
    // `m.direct` is account data and nothing stops it naming us — a note-to-self
    // room, or a client that wrote both sides. Our own dot on our own DM says
    // nothing, so it doesn't get one.
    (partner != room.own_user_id()).then(|| partner.to_owned())
}
