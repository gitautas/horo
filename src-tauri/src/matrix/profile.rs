//! Extended profiles (MSC4133).
//!
//! The homeserver stores arbitrary key/value pairs alongside `displayname` and
//! `avatar_url`, readable by anyone who can see the user. Continuwuity supports
//! this **without advertising `uk.tcpip.msc4133`** in `unstable_features`, so
//! don't infer support from the capability list — try the endpoint.
//!
//! Ruma doesn't model the endpoint yet, so these are plain HTTP calls carrying
//! the session's access token.
//!
//! The bio and status keys are Commet's, deliberately: it's the other client
//! that implements this, and matching its keys means a bio written in one shows
//! up in the other. Note the shapes differ — bio is an object, status is a bare
//! string. There's no observed key for a cover image anywhere, so that one is
//! ours and only horo will read it.

use std::time::Duration;

use matrix_sdk::ruma::UserId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ts_rs::TS;

use crate::{
    dto::Verification,
    error::{Error, Result},
    matrix::MatrixCore,
};

const BIO_KEY: &str = "chat.commet.profile_bio";
const STATUS_KEY: &str = "chat.commet.profile_status";
const COVER_KEY: &str = "gg.uwu.cover_url";

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "Profile")]
#[serde(rename_all = "camelCase")]
pub struct ProfileDto {
    pub user_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    /// Commet's `chat.commet.profile_bio`, unwrapped from its `body` object.
    pub bio: Option<String>,
    /// Commet's `chat.commet.profile_status`.
    pub status: Option<String>,
    /// `gg.uwu.cover_url` — ours; no other client reads it.
    pub cover_url: Option<String>,
}

/// A partial update. `None` leaves a field alone; `Some("")` clears it.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ProfileUpdate {
    #[serde(default)]
    #[ts(optional = nullable)]
    pub bio: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub status: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub cover_url: Option<String>,
}

fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(Error::Http)
}

fn profile_url(core: &MatrixCore, user_id: &str, key: Option<&str>) -> Result<url::Url> {
    let path = match key {
        Some(key) => format!("/_matrix/client/v3/profile/{user_id}/{key}"),
        None => format!("/_matrix/client/v3/profile/{user_id}"),
    };
    core.client.homeserver().join(&path).map_err(Error::from)
}

/// Read a profile. `None` means our own.
///
/// Unknown or malformed fields are skipped rather than failing the read — a
/// profile is decoration, and one odd key shouldn't blank the rest.
pub async fn get_profile(core: &MatrixCore, user_id: Option<String>) -> Result<ProfileDto> {
    let user_id = match user_id {
        Some(id) => id,
        None => core.own_user_id()?.to_string(),
    };

    let response = http()?
        .get(profile_url(core, &user_id, None)?)
        .send()
        .await?
        .error_for_status()
        .map_err(|e| Error::Other(format!("couldn't read that profile: {e}")))?;

    let body: Value = response.json().await?;

    let text = |key: &str| {
        body.get(key).and_then(|v| v.as_str()).map(str::to_owned).filter(|s| !s.is_empty())
    };

    Ok(ProfileDto {
        user_id,
        display_name: text("displayname"),
        avatar_url: text("avatar_url"),
        // Commet writes the bio as `{ "body": "…" }`; tolerate a bare string
        // too, since nothing stops another client from writing one.
        bio: body
            .get(BIO_KEY)
            .and_then(|v| v.get("body").and_then(|b| b.as_str()).or_else(|| v.as_str()))
            .map(str::to_owned)
            .filter(|s| !s.is_empty()),
        status: text(STATUS_KEY),
        cover_url: text(COVER_KEY),
    })
}

/// Write the fields that were supplied, leaving the rest untouched.
pub async fn set_profile(core: &MatrixCore, update: ProfileUpdate) -> Result<()> {
    if let Some(bio) = update.bio {
        // The object shape is Commet's; keep writing it even when clearing, so
        // the two clients agree on what an empty bio looks like.
        set_field(core, BIO_KEY, json!({ "body": bio }), bio.is_empty()).await?;
    }
    if let Some(status) = update.status {
        let empty = status.is_empty();
        set_field(core, STATUS_KEY, Value::String(status), empty).await?;
    }
    if let Some(cover) = update.cover_url {
        let empty = cover.is_empty();
        set_field(core, COVER_KEY, Value::String(cover), empty).await?;
    }
    Ok(())
}

async fn set_field(core: &MatrixCore, key: &str, value: Value, clear: bool) -> Result<()> {
    let user_id = core.own_user_id()?.to_string();
    let token = core
        .client
        .access_token()
        .ok_or_else(|| Error::Other("no access token on this session".into()))?;
    let url = profile_url(core, &user_id, Some(key))?;

    let client = http()?;
    let request = if clear {
        client.delete(url)
    } else {
        // MSC4133 mirrors the displayname endpoint: the body is the field keyed
        // by its own name.
        client.put(url).json(&json!({ key: value }))
    };

    let response = request.bearer_auth(token).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        // A server without MSC4133 answers here rather than at the read, so
        // this is where the useful diagnosis lives.
        return Err(Error::Other(format!(
            "your homeserver rejected the profile update ({status}). it may not \
             support extended profiles. {body}"
        )));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// what we know about a person locally
// ---------------------------------------------------------------------------

/// A room both of you are in, reduced to what a profile card draws.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "SharedRoom")]
#[serde(rename_all = "camelCase")]
pub struct SharedRoomDto {
    pub id: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub is_space: bool,
    pub is_direct: bool,
}

/// The parts of a profile card that come from our own client rather than from
/// the profile endpoint: whether we've verified them, whether there's already a
/// DM, and where the two of you overlap.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "UserContext")]
#[serde(rename_all = "camelCase")]
pub struct UserContextDto {
    pub user_id: String,
    pub is_me: bool,
    pub verification: Verification,
    pub dm_room_id: Option<String>,
    pub shared_rooms: Vec<SharedRoomDto>,
}

pub async fn get_user_context(core: &MatrixCore, user_id: String) -> Result<UserContextDto> {
    let user_id = UserId::parse(&user_id)?;
    let me = core.own_user_id()?;

    let verification = match core.client.encryption().get_user_identity(&user_id).await {
        Ok(Some(identity)) if identity.is_verified() => Verification::Verified,
        Ok(Some(_)) => Verification::Unverified,
        _ => Verification::Unknown,
    };

    let shared = if user_id == me {
        Vec::new()
    } else {
        shared_rooms(core, &user_id).await
    };

    Ok(UserContextDto {
        is_me: user_id == me,
        dm_room_id: core.client.get_dm_room(&user_id).map(|r| r.room_id().to_string()),
        shared_rooms: shared,
        verification,
        user_id: user_id.to_string(),
    })
}

/// Rooms you're both in.
///
/// Two sources, unioned, because neither is complete on its own. The local scan
/// only sees rooms whose member list has been loaded — with lazy loading that's
/// roughly "rooms you've opened this session" — and MSC2666 is unstable and not
/// implemented everywhere. A missing room is a quieter failure than a wrong
/// one, so anything that errors just contributes nothing.
async fn shared_rooms(core: &MatrixCore, user_id: &UserId) -> Vec<SharedRoomDto> {
    let mut ids: Vec<String> = mutual_rooms_msc2666(core, user_id).await;

    for room in core.client.joined_rooms() {
        let id = room.room_id().to_string();
        if ids.contains(&id) {
            continue;
        }
        // `_no_sync` deliberately: one network round trip per room to answer a
        // decoration isn't a trade worth making.
        if matches!(room.get_member_no_sync(user_id).await, Ok(Some(member))
            if member.membership() == &matrix_sdk::ruma::events::room::member::MembershipState::Join)
        {
            ids.push(id);
        }
    }

    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let Ok(room_id) = matrix_sdk::ruma::RoomId::parse(&id) else { continue };
        let Some(room) = core.client.get_room(&room_id) else { continue };

        out.push(SharedRoomDto {
            id,
            name: room
                .cached_display_name()
                .map(|n| n.to_string())
                .unwrap_or_else(|| room.room_id().to_string()),
            avatar_url: room.avatar_url().map(|u| u.to_string()),
            is_space: room.is_space(),
            is_direct: room.is_direct().await.unwrap_or(false),
        });
    }

    // Spaces last: a shared space is weaker evidence of knowing someone than a
    // shared room, and the card shows the first few.
    out.sort_by(|a, b| a.is_space.cmp(&b.is_space).then_with(|| a.name.cmp(&b.name)));
    out
}

/// MSC2666, which the homeserver may or may not implement. Unsupported answers
/// (404, or anything unexpected in the body) mean "no help from here".
async fn mutual_rooms_msc2666(core: &MatrixCore, user_id: &UserId) -> Vec<String> {
    let Some(token) = core.client.access_token() else { return Vec::new() };
    let Ok(url) = core
        .client
        .homeserver()
        .join("/_matrix/client/unstable/uk.half-shot.msc2666/user/mutual_rooms")
    else {
        return Vec::new();
    };
    let Ok(client) = http() else { return Vec::new() };

    let response = client
        .get(url)
        .query(&[("user_id", user_id.as_str())])
        .bearer_auth(token)
        .send()
        .await;

    let Ok(response) = response else { return Vec::new() };
    if !response.status().is_success() {
        return Vec::new();
    }

    let Ok(body) = response.json::<Value>().await else { return Vec::new() };
    body.get("joined")
        .and_then(|v| v.as_array())
        .map(|rooms| {
            rooms.iter().filter_map(|r| r.as_str().map(str::to_owned)).collect()
        })
        .unwrap_or_default()
}

/// The room to open when you press "message": an existing DM if there is one,
/// otherwise a fresh encrypted DM with an invite out to them.
pub async fn open_dm(core: &MatrixCore, user_id: String) -> Result<String> {
    let user_id = UserId::parse(&user_id)?;
    if user_id == core.own_user_id()? {
        return Err(Error::Other("you can't dm yourself~".into()));
    }

    if let Some(room) = core.client.get_dm_room(&user_id) {
        return Ok(room.room_id().to_string());
    }

    let room = core.client.create_dm(&user_id).await?;
    Ok(room.room_id().to_string())
}

/// Upload a local file to the media repository and return its `mxc://` URI.
///
/// Shared by anything that needs to put an image on the server — cover photos
/// now, custom emoji later.
pub async fn upload_media(core: &MatrixCore, path: &str) -> Result<String> {
    let bytes = tokio::fs::read(path).await?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();

    let response = core
        .client
        .media()
        .upload(&mime, bytes, None)
        .await
        .map_err(|e| Error::Other(format!("upload failed: {e}")))?;

    Ok(response.content_uri.to_string())
}
