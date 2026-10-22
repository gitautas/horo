//! MatrixRTC voice calls over a LiveKit SFU.
//!
//! This follows the MSC3401/MSC4143 shape that Element Call uses, so horo can
//! join a call started from Element X and vice versa:
//!
//! 1. Advertise membership by writing an `org.matrix.msc3401.call.member` state
//!    event, keyed by `_<user_id>_<device_id>`, naming the LiveKit focus we'd
//!    like to use. Other clients read this to know who's in the call.
//! 2. Get a short-lived OpenID token from our homeserver — this is how the SFU
//!    verifies we are who we say we are without ever seeing our access token.
//! 3. Trade that token at the LiveKit JWT service for a room-scoped JWT.
//! 4. Hand the JWT and SFU URL to the frontend, which does the actual WebRTC
//!    with `livekit-client` in the WebView.
//! 5. If we were first in, send an `m.rtc.notification` so everyone else's
//!    phone rings. A membership on its own is silent: Element only rings for
//!    the notification, and so does the homeserver's push rule for calls.
//!
//! Media never passes through Rust: the WebView already has a full WebRTC stack
//! and the platform echo cancellation that goes with it.

use std::time::Duration;

use matrix_sdk::ruma::{
    MilliSecondsSinceUnixEpoch, OwnedEventId, OwnedRoomId, RoomId,
    api::client::account::request_openid_token,
    events::{
        Mentions,
        call::member::{
            ActiveFocus, ActiveLivekitFocus, Application, CallApplicationContent,
            CallMemberEventContent, CallMemberStateKey, CallScope, Focus, LivekitFocus,
        },
        relation::Reference,
        rtc::notification::{CallIntent, NotificationType, RtcNotificationEventContent},
    },
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    error::{Error, Result},
    matrix::MatrixCore,
};

/// How long a membership advertisement stays valid before it must be refreshed.
/// Element Call uses four hours; we refresh well before that so a long call
/// never blinks out of the participant list.
const MEMBERSHIP_TTL: Duration = Duration::from_secs(4 * 60 * 60);

/// How long the other end rings for. Matches `RING_TIMEOUT_MS` in `call.ts`,
/// so a horo on the other end and an Element one give up at the same moment.
const RING_LIFETIME: Duration = Duration::from_secs(45);

/// The name Element Call sends and every client listens for. Ruma sends the
/// stable `m.rtc.notification`, which matrix-js-sdk (Element Web and Desktop)
/// doesn't know yet.
const RTC_NOTIFICATION: &str = "org.matrix.msc4075.rtc.notification";

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CallCredentials {
    /// WebSocket URL of the SFU, e.g. `wss://livekit.example.org`.
    pub livekit_url: String,
    /// Room-scoped JWT for `livekit-client` to connect with.
    pub jwt: String,
    /// The LiveKit room name — the Matrix room ID, so every client agrees.
    pub alias: String,
    pub room_id: String,
    /// We started this call and rang the other end, so the caller should hear
    /// ringback until someone picks up.
    pub ringing: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CallParticipants {
    pub room_id: String,
    pub user_ids: Vec<String>,
    pub is_active: bool,
}

#[derive(Deserialize)]
struct SfuResponse {
    jwt: String,
    url: String,
}

/// The LiveKit SFU our own homeserver advertises, for a call we're starting.
/// Joining someone else's call uses *their* focus instead, so we land on
/// their SFU rather than a second, disconnected one.
async fn own_focus(core: &MatrixCore) -> Result<String> {
    crate::matrix::auth::discover_livekit_focus(core.client.homeserver().as_str())
        .await
        .ok_or_else(|| {
            Error::Other(
                "no livekit server configured — ask your homeserver admin to advertise \
                 one in .well-known"
                    .into(),
            )
        })
}

/// Read the focus out of the oldest live membership, matching the
/// `oldest_membership` focus-selection rule the MSC specifies.
async fn focus_from_existing_members(core: &MatrixCore, room_id: &RoomId) -> Option<String> {
    let room = core.client.get_room(room_id)?;
    let events = room.get_state_events_static::<CallMemberEventContent>().await.ok()?;

    let mut candidates: Vec<(u64, String)> = Vec::new();

    for raw in events {
        let Ok(event) = raw.deserialize() else { continue };
        let Some(original) = event.as_sync().and_then(|sync| sync.as_original()) else {
            continue;
        };
        let origin_ts: u64 = original.origin_server_ts.0.into();

        for membership in original.content.active_memberships(Some(original.origin_server_ts)) {
            for focus in membership.foci_preferred() {
                if let Focus::Livekit(livekit) = focus {
                    candidates.push((origin_ts, livekit.service_url.clone()));
                }
            }
        }
    }

    candidates.sort_by_key(|(ts, _)| *ts);
    candidates.into_iter().next().map(|(_, url)| url)
}

fn state_key(core: &MatrixCore) -> Result<CallMemberStateKey> {
    let user_id = core.own_user_id()?;
    let device_id = core
        .client
        .device_id()
        .ok_or_else(|| Error::Other("no device id on this session".into()))?
        .to_owned();
    // `underscore: true` produces the `_@user:server_DEVICE` form Element Call
    // writes; the leading underscore keeps the key legal for older servers.
    Ok(CallMemberStateKey::new(user_id, Some(device_id.to_string()), true))
}

/// Announce that we're in the call, and get credentials for the SFU.
pub async fn join(core: &MatrixCore, room_id: &RoomId, video: bool) -> Result<CallCredentials> {
    let room = core.room(&room_id.to_owned())?;
    // Read before our own membership goes in: nobody here yet means we are the
    // one starting the call, and the one who has to ring everyone else.
    let existing = focus_from_existing_members(core, room_id).await;
    let starting = existing.is_none();
    let service_url = match existing {
        Some(url) => url,
        None => own_focus(core).await?,
    };
    // Every client uses the Matrix room ID as the LiveKit room name, which is
    // what puts us all in the same SFU room.
    let alias = room_id.to_string();

    let device_id = core
        .client
        .device_id()
        .ok_or_else(|| Error::Other("no device id on this session".into()))?
        .to_owned();

    let content = CallMemberEventContent::new(
        Application::Call(CallApplicationContent::new(String::new(), CallScope::Room)),
        device_id,
        ActiveFocus::Livekit(ActiveLivekitFocus::new()),
        vec![Focus::Livekit(LivekitFocus::new(alias.clone(), service_url.clone()))],
        None,
        Some(MEMBERSHIP_TTL),
    );

    let membership = room.send_state_event_for_key(&state_key(core)?, content).await?;

    // Get the SFU token only after the membership is published: if the token
    // request fails we still want other clients to see us trying to join, and
    // the caller retries rather than silently sitting outside the call.
    let mut credentials = match fetch_credentials(core, &service_url, &alias).await {
        Ok(credentials) => credentials,
        Err(e) => {
            // Don't leave a phantom participant behind on failure.
            let _ = leave(core, room_id).await;
            return Err(e);
        }
    };

    // Only once we can actually take the call: ringing someone into a call we
    // then fail to join would have them answer to an empty room.
    if starting {
        match notify(&room, membership.event_id, video).await {
            Ok(rang) => credentials.ringing = rang,
            // The call itself is fine; they'll see it in the room list, they
            // just won't be rung for it.
            Err(e) => tracing::warn!("couldn't send the call notification: {e}"),
        }
    }

    Ok(credentials)
}

/// Tell the room a call has started. Returns whether that rings phones.
///
/// A DM rings; a group room gets an ordinary notification. That is Element
/// Call's rule and ours (`notify.ts` rings for DMs only), since a group call
/// that rings forty phones is how a room ends up muted.
async fn notify(room: &matrix_sdk::Room, membership: OwnedEventId, video: bool) -> Result<bool> {
    let ring = room.is_direct().await.unwrap_or(false);
    let content = notification(membership, ring, video, MilliSecondsSinceUnixEpoch::now());
    room.send_raw(RTC_NOTIFICATION, serde_json::to_value(&content)?).await?;
    Ok(ring)
}

fn notification(
    membership: OwnedEventId,
    ring: bool,
    video: bool,
    now: MilliSecondsSinceUnixEpoch,
) -> RtcNotificationEventContent {
    let kind = if ring { NotificationType::Ring } else { NotificationType::Notification };
    let mut content = RtcNotificationEventContent::new(now, RING_LIFETIME, kind);
    content.mentions = Some(Mentions::with_room_mention());
    content.relates_to = Some(Reference::new(membership));
    content.call_intent = Some(if video { CallIntent::Video } else { CallIntent::Audio });
    content
}

async fn fetch_credentials(
    core: &MatrixCore,
    service_url: &str,
    alias: &str,
) -> Result<CallCredentials> {
    let user_id = core.own_user_id()?;
    let device_id = core
        .client
        .device_id()
        .ok_or_else(|| Error::Other("no device id on this session".into()))?
        .to_string();

    // The OpenID token is a deliberately narrow credential: the SFU can use it
    // to ask our homeserver "is this really @user:server?" and nothing else.
    let openid = core
        .client
        .send(request_openid_token::v3::Request::new(user_id))
        .await
        .map_err(|e| Error::Other(format!("homeserver refused an openid token: {e}")))?;

    let endpoint = url::Url::parse(service_url)
        .and_then(|base| base.join("/sfu/get"))
        .map_err(|e| Error::Other(format!("bad livekit service url: {e}")))?;

    let body = serde_json::json!({
        "room": alias,
        "openid_token": {
            "access_token": openid.access_token,
            "token_type": openid.token_type.to_string(),
            "matrix_server_name": openid.matrix_server_name.to_string(),
            "expires_in": openid.expires_in.as_secs(),
        },
        "device_id": device_id,
    });

    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?
        .post(endpoint)
        .json(&body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(Error::Other(format!("livekit token service said {status}: {text}")));
    }

    let sfu: SfuResponse = response.json().await?;

    Ok(CallCredentials {
        livekit_url: sfu.url,
        jwt: sfu.jwt,
        alias: alias.to_owned(),
        room_id: alias.to_owned(),
        ringing: false,
    })
}

/// Withdraw our membership. Sending empty content is how the MSC spells
/// "hung up"; redacting would lose the history other clients rely on.
pub async fn leave(core: &MatrixCore, room_id: &RoomId) -> Result<()> {
    let room = core.room(&room_id.to_owned())?;
    let content = CallMemberEventContent::new_empty(None);
    room.send_state_event_for_key(&state_key(core)?, content).await?;
    Ok(())
}

/// Re-publish our membership so it doesn't expire mid-call.
pub async fn refresh(core: &MatrixCore, room_id: &RoomId) -> Result<()> {
    // Checked up front so a call that emptied out while we were in it ends here,
    // rather than `join` quietly starting a fresh one on our own SFU.
    focus_from_existing_members(core, room_id)
        .await
        .ok_or_else(|| Error::Other("call is no longer active".into()))?;
    // Never rings: our own membership is still there, so this isn't a new call.
    join(core, room_id, false).await.map(|_| ())
}

/// Who is currently in the call, for the participant tiles and the room list dot.
pub async fn participants(core: &MatrixCore, room_id: &RoomId) -> Result<CallParticipants> {
    let room = core.room(&room_id.to_owned())?;
    Ok(CallParticipants {
        room_id: room_id.to_string(),
        user_ids: room.active_room_call_participants().iter().map(|u| u.to_string()).collect(),
        is_active: room.has_active_room_call(),
    })
}

/// Every room with a call in progress — drives the call indicator in the sidebar.
pub async fn active_calls(core: &MatrixCore) -> Result<Vec<OwnedRoomId>> {
    Ok(core
        .client
        .joined_rooms()
        .into_iter()
        .filter(|room| room.has_active_room_call())
        .map(|room| room.room_id().to_owned())
        .collect())
}

#[cfg(test)]
mod tests {
    use matrix_sdk::ruma::{owned_event_id, uint};
    use serde_json::json;

    use super::*;

    /// The shape Element Call sends, which is what Element X and Web ring for.
    #[test]
    fn notification_matches_element_call() {
        let content = notification(
            owned_event_id!("$member"),
            true,
            false,
            MilliSecondsSinceUnixEpoch(uint!(1_000)),
        );

        assert_eq!(
            serde_json::to_value(&content).unwrap(),
            json!({
                "sender_ts": 1_000,
                "lifetime": 45_000,
                "m.mentions": { "room": true },
                "m.relates_to": { "rel_type": "m.reference", "event_id": "$member" },
                "notification_type": "ring",
                "m.call.intent": "audio",
            }),
        );
    }
}
