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
//!
//! Media never passes through Rust: the WebView already has a full WebRTC stack
//! and the platform echo cancellation that goes with it.

use std::time::Duration;

use matrix_sdk::ruma::{
    OwnedRoomId, RoomId,
    api::client::account::request_openid_token,
    events::call::member::{
        ActiveFocus, ActiveLivekitFocus, Application, CallApplicationContent,
        CallMemberEventContent, CallMemberStateKey, CallScope, Focus, LivekitFocus,
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

/// Which LiveKit SFU to use for this room.
///
/// Preference order: the focus already chosen by whoever is in the call (so we
/// join *their* SFU rather than starting a second, disconnected one), then our
/// own homeserver's advertised focus.
async fn resolve_focus(core: &MatrixCore, room_id: &RoomId) -> Result<String> {
    if let Some(url) = focus_from_existing_members(core, room_id).await {
        return Ok(url);
    }

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
pub async fn join(core: &MatrixCore, room_id: &RoomId) -> Result<CallCredentials> {
    let room = core.room(&room_id.to_owned())?;
    let service_url = resolve_focus(core, room_id).await?;
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

    room.send_state_event_for_key(&state_key(core)?, content).await?;

    // Get the SFU token only after the membership is published: if the token
    // request fails we still want other clients to see us trying to join, and
    // the caller retries rather than silently sitting outside the call.
    let credentials = match fetch_credentials(core, &service_url, &alias).await {
        Ok(credentials) => credentials,
        Err(e) => {
            // Don't leave a phantom participant behind on failure.
            let _ = leave(core, room_id).await;
            return Err(e);
        }
    };

    Ok(credentials)
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
    join(core, room_id).await.map(|_| ())
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
