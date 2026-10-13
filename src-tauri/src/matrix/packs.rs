//! Custom emotes and stickers — image packs.
//!
//! An image pack is a set of named images. The name is a shortcode (`blobcat`),
//! written `:blobcat:` where a person types it, and each image says whether it's
//! usable as an inline emote, as a sticker, or both.
//!
//! A pack lives in a room, as an `m.room.image_pack` state event; a room may
//! hold any number of them, one per state key. `m.image_pack.rooms` in account
//! data is the list of packs you've chosen to carry with you everywhere rather
//! than using only in the room they came from. Between them that's the whole
//! model — there is no such thing as a pack that isn't in a room, which is why
//! [`create_personal_pack`] makes a room nobody else is in.
//!
//! Both events are also read and written under the names MSC2545 used before
//! Matrix 1.19 stabilised them (`im.ponies.room_emotes`, `im.ponies.emote_rooms`),
//! because that's what FluffyChat and Cinny still speak. The one thing with no
//! stable equivalent is `im.ponies.user_emotes`, the MSC's single personal pack
//! in account data: it's read so that a pack made in another client shows up
//! here, and written when it's edited, but nothing new is put in it.
//!
//! Everything is parsed leniently. These events are written by other clients
//! and edited by hand, so a pack with one malformed image should lose that
//! image, not the pack — and a malformed pack should lose the pack, not the
//! user's whole emote set. Fields we don't recognise are carried through an
//! edit rather than dropped.

use std::collections::BTreeMap;

use matrix_sdk::{
    deserialized_responses::RawAnySyncOrStrippedState,
    ruma::{RoomId, events::StateEventType},
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    error::{Error, Result},
    matrix::MatrixCore,
};

/// The personal pack. Only MSC2545 has one — the spec dropped it, on the
/// grounds that a pack of your own is a pack in a room only you are in.
pub const USER_EMOTES: &str = "im.ponies.user_emotes";

/// A room's packs, and the list of the ones you carry everywhere.
///
/// Each exists twice: the name Matrix 1.19 settled on, and the name MSC2545
/// used while it was a proposal. FluffyChat and Cinny are still on the second,
/// so both are read, and both are written — a pack edited here has to stay
/// visible to the people you share rooms with. The legacy half comes out once
/// the clients we care about have moved.
pub const ROOM_PACK: &str = "m.room.image_pack";
pub const ROOM_PACK_LEGACY: &str = "im.ponies.room_emotes";
pub const PACK_ROOMS: &str = "m.image_pack.rooms";
pub const PACK_ROOMS_LEGACY: &str = "im.ponies.emote_rooms";

/// The room type given to a room that exists only to hold a pack.
///
/// Room types are how Matrix says "this isn't a conversation" — `m.space` is
/// one. There's no standard type for a pack room, so this is ours; clients that
/// don't know it should leave the room out of their room list, which is exactly
/// what we want them to do.
///
/// It keeps the pre-rename `uwum` name: it is stored on the server, and changing
/// it would hide every pack room created before the rename.
pub const PACK_ROOM_TYPE: &str = "gg.veil.uwum.image_pack";

// ---------------------------------------------------------------------------
// the wire format
// ---------------------------------------------------------------------------

/// A pack event's content, as it appears on the wire.
///
/// Every one of these carries an `extra` catch-all, because editing a pack is
/// read-modify-write: without it, changing one shortcode in a pack made by
/// another client would quietly delete every field that client knew about and
/// we didn't. The spec asks for exactly this on `m.image_pack.rooms`, and it's
/// the right thing to do for all of them.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ImagePackContent {
    #[serde(default)]
    pub images: BTreeMap<String, PackImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<PackInfo>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PackInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PackImage {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<ImageInfo>,
    /// Per-image usage, which MSC2545 has and the spec doesn't.
    ///
    /// Still written: our own reader and every `im.ponies.*` client honour it,
    /// and a client reading the spec's shape treats it as an unknown field and
    /// falls back to the pack's usage — which [`derive_pack_usage`] keeps in
    /// step, so nothing lands in the wrong picker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ImageInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mimetype: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Room ID → state key → an object the spec reserves for future use.
///
/// That innermost object is opaque and must survive being edited, so it's held
/// as raw JSON and never rebuilt.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EmoteRoomsContent {
    #[serde(default)]
    pub rooms: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// what the frontend sees
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "ImagePack")]
#[serde(rename_all = "camelCase")]
pub struct ImagePackDto {
    /// Stable across reloads, and what the frontend uses to address a pack:
    /// `user` for the personal one, `<room_id>|<state_key>` for a room's.
    pub id: String,
    pub source: PackSource,
    pub room_id: Option<String>,
    pub state_key: Option<String>,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub attribution: Option<String>,
    pub images: Vec<PackImageDto>,
    /// Whether this pack is carried outside the room it belongs to — the
    /// `m.image_pack.rooms` list. Always true for the MSC's personal pack,
    /// which is account data and so has no room to be outside of.
    ///
    /// Not a filter: everything returned here is usable where it was asked for,
    /// including a room's own packs in that room. This says whether it follows
    /// you elsewhere, which is what the settings toggle writes.
    pub everywhere: bool,
    /// Whether this account may write the pack back.
    pub can_edit: bool,
}

/// Where a pack lives: account data, or a room's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum PackSource {
    User,
    Room,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "PackImage")]
#[serde(rename_all = "camelCase")]
pub struct PackImageDto {
    /// The `:name:` a person types, without the colons.
    pub shortcode: String,
    pub url: String,
    /// Alt text for the image. Falls back to the shortcode.
    pub body: String,
    pub is_emoticon: bool,
    pub is_sticker: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size: Option<u64>,
    pub mimetype: Option<String>,
}

/// Read a pack's usage list, which is absent when a pack is good for both.
fn usage_of(usage: Option<&Vec<String>>) -> (bool, bool) {
    match usage {
        None => (true, true),
        Some(list) if list.is_empty() => (true, true),
        Some(list) => (
            list.iter().any(|u| u == "emoticon"),
            list.iter().any(|u| u == "sticker"),
        ),
    }
}

impl ImagePackContent {
    /// Flatten to the frontend's shape.
    ///
    /// An image with no usage of its own inherits the pack's, which is how the
    /// MSC lets a whole pack be declared stickers-only in one line.
    fn to_images(&self) -> Vec<PackImageDto> {
        let pack_usage = self.pack.as_ref().and_then(|p| p.usage.as_ref());

        self.images
            .iter()
            // A URL that isn't an mxc is either a mistake or an attempt to make
            // the client fetch something remote; neither is worth rendering.
            .filter(|(_, image)| image.url.starts_with("mxc://"))
            .map(|(shortcode, image)| {
                let (is_emoticon, is_sticker) =
                    usage_of(image.usage.as_ref().or(pack_usage));

                PackImageDto {
                    shortcode: shortcode.clone(),
                    url: image.url.clone(),
                    body: image.body.clone().unwrap_or_else(|| shortcode.clone()),
                    is_emoticon,
                    is_sticker,
                    width: image.info.as_ref().and_then(|i| i.w),
                    height: image.info.as_ref().and_then(|i| i.h),
                    size: image.info.as_ref().and_then(|i| i.size),
                    mimetype: image.info.as_ref().and_then(|i| i.mimetype.clone()),
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// reading
// ---------------------------------------------------------------------------

/// The user's own pack, or `None` when they haven't made one.
pub async fn user_pack(core: &MatrixCore) -> Result<Option<ImagePackContent>> {
    let raw = core
        .client
        .account()
        .account_data_raw(USER_EMOTES.into())
        .await?;

    Ok(raw.and_then(|raw| match raw.deserialize_as_unchecked::<ImagePackContent>() {
        Ok(content) => Some(content),
        Err(e) => {
            tracing::warn!("ignoring an unreadable {USER_EMOTES}: {e}");
            None
        }
    }))
}

/// One account data event, or an empty one if it's missing or unreadable.
async fn read_pack_rooms(core: &MatrixCore, event_type: &str) -> Result<EmoteRoomsContent> {
    let raw = core.client.account().account_data_raw(event_type.into()).await?;

    Ok(raw
        .and_then(|raw| raw.deserialize_as_unchecked::<EmoteRoomsContent>().ok())
        .unwrap_or_default())
}

/// The rooms whose packs the user has enabled everywhere.
///
/// The union of the two event names: a pack turned on from a client that only
/// knows one of them is still turned on here. Turning one off writes both, so
/// the two can't drift apart once we've touched them.
pub async fn emote_rooms(core: &MatrixCore) -> Result<EmoteRoomsContent> {
    let mut merged = read_pack_rooms(core, PACK_ROOMS).await?;
    let legacy = read_pack_rooms(core, PACK_ROOMS_LEGACY).await?;

    for (room, keys) in legacy.rooms {
        let entry = merged.rooms.entry(room).or_default();
        for (state_key, opaque) in keys {
            entry.entry(state_key).or_insert(opaque);
        }
    }

    Ok(merged)
}

/// Every pack in one room, with its state key.
///
/// Read under both names and merged by state key. A room whose packs were
/// written by a client that knows both — including this one — holds each pack
/// twice, and they're the same pack; the spec's name wins so that whichever
/// client last wrote it, the newer shape is the one shown.
pub async fn room_packs(
    core: &MatrixCore,
    room_id: &RoomId,
) -> Result<Vec<(String, ImagePackContent)>> {
    let Ok(room) = core.room(&room_id.to_owned()) else {
        // A room we've left or never joined isn't an error here — it just has
        // nothing to contribute.
        return Ok(Vec::new());
    };

    // Insertion order is the reading order, so the stable name is seen first and
    // `or_insert` leaves it in place.
    let mut merged: BTreeMap<String, ImagePackContent> = BTreeMap::new();

    for event_type in [ROOM_PACK, ROOM_PACK_LEGACY] {
        let events = room.get_state_events(StateEventType::from(event_type)).await?;

        for event in events {
            // Read the raw JSON rather than a typed event: ruma has no type for
            // either name, and the state key and content are both right there. A
            // pack whose key we can't read has no stable identity, so it's
            // skipped rather than guessed at.
            let value = match &event {
                RawAnySyncOrStrippedState::Sync(raw) => {
                    raw.deserialize_as_unchecked::<serde_json::Value>()
                }
                RawAnySyncOrStrippedState::Stripped(raw) => {
                    raw.deserialize_as_unchecked::<serde_json::Value>()
                }
            };
            let Ok(value) = value else { continue };
            let Some(state_key) = value.get("state_key").and_then(|k| k.as_str()) else {
                continue;
            };
            let Some(content) = value.get("content") else { continue };

            match serde_json::from_value::<ImagePackContent>(content.clone()) {
                // An empty pack is how a pack gets removed — state events can't
                // be deleted — so it isn't offered as one.
                Ok(pack) if !pack.images.is_empty() => {
                    merged.entry(state_key.to_owned()).or_insert(pack);
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("ignoring an unreadable {event_type} in {room_id}: {e}")
                }
            }
        }
    }

    Ok(merged.into_iter().collect())
}

/// Every pack available for use right now.
///
/// That's the personal pack, the packs belonging to the room being looked at,
/// and the packs from rooms the user has enabled globally. A room's own packs
/// count there whether or not they've been enabled everywhere — that's the
/// point of them.
///
/// Scanning every joined room instead would mean reading the state of hundreds
/// of rooms to open a picker; the `emote_rooms` list exists precisely so we
/// don't have to.
pub async fn available(core: &MatrixCore, room_id: Option<&RoomId>) -> Result<Vec<ImagePackDto>> {
    let enabled = emote_rooms(core).await?;
    let mut out = Vec::new();

    if let Some(pack) = user_pack(core).await? {
        out.push(to_dto(pack, None, None, true, true));
    }

    // The room in front of the user first, then everything they carry around.
    let mut rooms: Vec<String> = room_id.map(|id| id.to_string()).into_iter().collect();
    for id in enabled.rooms.keys() {
        if !rooms.contains(id) {
            rooms.push(id.clone());
        }
    }

    for room in rooms {
        let Ok(parsed) = RoomId::parse(&room) else { continue };
        let can_edit = can_edit_packs(core, &parsed).await;

        for (state_key, pack) in room_packs(core, &parsed).await? {
            let everywhere = enabled
                .rooms
                .get(&room)
                .is_some_and(|keys| keys.contains_key(&state_key));

            out.push(to_dto(pack, Some(room.clone()), Some(state_key), everywhere, can_edit));
        }
    }

    Ok(out)
}

/// Every pack the settings screen should list.
///
/// Unlike [`available`], this walks every joined room, because the point of the
/// screen is to show packs you *haven't* turned on yet. It's user-initiated and
/// reads from the local store, so the cost lands where someone asked for it.
pub async fn all(core: &MatrixCore) -> Result<Vec<ImagePackDto>> {
    let enabled = emote_rooms(core).await?;
    let mut out = Vec::new();

    out.push(match user_pack(core).await? {
        Some(pack) => to_dto(pack, None, None, true, true),
        // The personal pack is offered even when it doesn't exist yet — an
        // empty card to drop images into is the way you make one.
        None => to_dto(ImagePackContent::default(), None, None, true, true),
    });

    for room in core.client.joined_rooms() {
        let room_id = room.room_id().to_string();
        let can_edit = can_edit_packs(core, room.room_id()).await;

        for (state_key, pack) in room_packs(core, room.room_id()).await? {
            let everywhere = enabled
                .rooms
                .get(&room_id)
                .is_some_and(|keys| keys.contains_key(&state_key));

            out.push(to_dto(pack, Some(room_id.clone()), Some(state_key), everywhere, can_edit));
        }
    }

    Ok(out)
}

/// A room a new pack could go in.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "PackRoom")]
#[serde(rename_all = "camelCase")]
pub struct PackRoomDto {
    pub id: String,
    pub name: String,
}

/// Rooms this account may write packs in, by name.
///
/// A pack has to live somewhere: the personal one is always available, and a
/// shared one needs a room you have the power level to send state in.
pub async fn editable_rooms(core: &MatrixCore) -> Result<Vec<PackRoomDto>> {
    let mut out = Vec::new();

    for room in core.client.joined_rooms() {
        if room.is_space() || !can_edit_packs(core, room.room_id()).await {
            continue;
        }

        out.push(PackRoomDto {
            id: room.room_id().to_string(),
            name: room
                .cached_display_name()
                .map(|n| n.to_string())
                .unwrap_or_else(|| room.room_id().to_string()),
        });
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

// ---------------------------------------------------------------------------
// writing
// ---------------------------------------------------------------------------

/// Replace the personal pack.
pub async fn save_user_pack(core: &MatrixCore, content: ImagePackContent) -> Result<()> {
    use matrix_sdk::ruma::serde::Raw;

    let raw = Raw::new(&content).map_err(|e| Error::Other(e.to_string()))?;
    core.client
        .account()
        .set_account_data_raw(USER_EMOTES.into(), raw.cast_unchecked())
        .await?;
    Ok(())
}

/// Replace one of a room's packs, under both names.
///
/// State events can't be deleted, so removing a pack means writing an empty
/// one; readers treat a pack with no images as nothing at all. Both names are
/// written for the same reason: leaving one behind would resurrect a deleted
/// pack for whichever client reads that one.
///
/// The pack's `usage` is recomputed on the way out, so a client that only
/// understands pack-level usage — everything following the spec rather than the
/// MSC — still puts these images in the right picker.
pub async fn save_room_pack(
    core: &MatrixCore,
    room_id: &RoomId,
    state_key: &str,
    mut content: ImagePackContent,
) -> Result<()> {
    let room = core.room(&room_id.to_owned())?;
    derive_pack_usage(&mut content);

    let body = serde_json::to_value(&content).map_err(|e| Error::Other(e.to_string()))?;

    for event_type in [ROOM_PACK, ROOM_PACK_LEGACY] {
        room.send_state_event_raw(event_type, state_key, body.clone()).await?;
    }
    Ok(())
}

/// Set the pack's own usage to the union of what its images are for.
///
/// Per-image usage is an MSC2545 field the spec doesn't have, so a spec-only
/// reader sees only this. The union is the honest summary: a pack of emotes
/// says emoticon, a pack of stickers says sticker, a mixed one says both and
/// its images turn up in both pickers there — which is worse than our own
/// rendering, but better than being missing from one.
fn derive_pack_usage(content: &mut ImagePackContent) {
    let pack_usage = content.pack.as_ref().and_then(|p| p.usage.clone());

    let (mut any_emoticon, mut any_sticker) = (false, false);
    for image in content.images.values() {
        let (emoticon, sticker) = usage_of(image.usage.as_ref().or(pack_usage.as_ref()));
        any_emoticon |= emoticon;
        any_sticker |= sticker;
    }

    // An empty pack keeps whatever it said before; there's nothing to summarise.
    if content.images.is_empty() {
        return;
    }

    content.pack.get_or_insert_with(PackInfo::default).usage =
        usage_list(any_emoticon, any_sticker);
}

/// Carry a room's pack everywhere, or stop — under both names.
pub async fn set_everywhere(
    core: &MatrixCore,
    room_id: &RoomId,
    state_key: &str,
    on: bool,
) -> Result<()> {
    use matrix_sdk::ruma::serde::Raw;

    let room = room_id.to_string();

    for event_type in [PACK_ROOMS, PACK_ROOMS_LEGACY] {
        // Read each event on its own rather than writing the merged view to
        // both: the two can hold different unknown fields, and this is the one
        // place they'd be flattened into each other.
        let mut content = read_pack_rooms(core, event_type).await?;

        if on {
            content
                .rooms
                .entry(room.clone())
                .or_default()
                // The innermost object is reserved for future use and the spec
                // asks that it be preserved, so an entry that already exists is
                // left exactly as it was.
                .entry(state_key.to_owned())
                .or_insert_with(|| serde_json::json!({}));
        } else {
            if let Some(keys) = content.rooms.get_mut(&room) {
                keys.remove(state_key);
            }
            // A room with no packs left shouldn't linger as an empty object.
            content.rooms.retain(|_, keys| !keys.is_empty());
        }

        let raw = Raw::new(&content).map_err(|e| Error::Other(e.to_string()))?;
        core.client
            .account()
            .set_account_data_raw(event_type.into(), raw.cast_unchecked())
            .await?;
    }

    Ok(())
}

/// Which pack an edit is aimed at.
///
/// The personal pack has no room; a room pack is a room plus a state key. The
/// frontend addresses packs by their `id`, and this is that id taken apart.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PackTarget {
    #[serde(default)]
    #[ts(optional = nullable)]
    pub room_id: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub state_key: Option<String>,
}

/// One change to a pack, applied on top of whatever the server currently has.
///
/// Edits are described rather than sent as a whole pack because two clients
/// touching the same pack shouldn't be able to erase each other's images: every
/// one of these reads first, changes one thing, and writes back.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PackEdit {
    /// Add an image, or replace one already under that shortcode.
    #[serde(rename_all = "camelCase")]
    PutImage {
        shortcode: String,
        url: String,
        #[serde(default)]
        #[ts(optional = nullable)]
        body: Option<String>,
        is_emoticon: bool,
        is_sticker: bool,
        #[serde(default)]
        #[ts(optional = nullable)]
        width: Option<u32>,
        #[serde(default)]
        #[ts(optional = nullable)]
        height: Option<u32>,
        #[serde(default)]
        #[ts(optional = nullable)]
        size: Option<u64>,
        #[serde(default)]
        #[ts(optional = nullable)]
        mimetype: Option<String>,
    },
    /// Move an image to a different shortcode, keeping everything else.
    #[serde(rename_all = "camelCase")]
    Rename { from: String, to: String },
    #[serde(rename_all = "camelCase")]
    RemoveImage { shortcode: String },
    /// Rename the pack itself.
    #[serde(rename_all = "camelCase")]
    SetName { name: String },
}

/// A shortcode people can actually type, and that survives a round trip.
///
/// Colons would make the code unparseable in `:name:` form, and whitespace
/// makes it untypeable; both are rejected rather than silently mangled.
fn check_shortcode(shortcode: &str) -> Result<String> {
    /// The spec's grammar: `1*100shortcode_char`, where a char is
    /// `ALPHA / DIGIT / "-" / "_"`.
    const MAX_LENGTH: usize = 100;

    let trimmed = shortcode.trim();

    if trimmed.is_empty() {
        return Err(Error::Other("a shortcode needs a name~".into()));
    }
    if trimmed.chars().count() > MAX_LENGTH {
        return Err(Error::Other(format!(
            "shortcodes can be at most {MAX_LENGTH} characters"
        )));
    }
    if !trimmed.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(Error::Other(
            "shortcodes can only use letters, numbers, - and _".into(),
        ));
    }

    Ok(trimmed.to_owned())
}

/// Make a pack of your own, in a room that exists only to hold it.
///
/// The spec has no personal pack: images live in rooms, and the list of packs
/// you carry everywhere points at them. A pack "of your own" is therefore a
/// pack in a room nobody else is in — so this makes that room, puts the pack in
/// it, and turns it on everywhere, and the UI never mentions the room at all.
///
/// Unencrypted on purpose. Pack images are plain `mxc://` URLs by definition —
/// every client renders them without keys — so encrypting the room would buy
/// nothing and cost the ability to read the pack from a fresh login.
pub async fn create_personal_pack(core: &MatrixCore, name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Other("give the pack a name first~".into()));
    }

    use matrix_sdk::ruma::{
        api::client::room::{Visibility, create_room},
        serde::Raw,
    };

    let state_key = state_key_for(name);
    let content = ImagePackContent {
        pack: Some(PackInfo {
            display_name: Some(name.to_owned()),
            ..PackInfo::default()
        }),
        ..ImagePackContent::default()
    };
    let content = serde_json::to_value(&content).map_err(|e| Error::Other(e.to_string()))?;

    let mut request = create_room::v3::Request::new();
    request.name = Some(name.to_owned());
    request.topic = Some("holds an image pack".to_owned());
    request.preset = Some(create_room::v3::RoomPreset::PrivateChat);
    request.visibility = Visibility::Private;

    // The pack goes in as creation state rather than being sent afterwards, so
    // that it arrives with the room in the first sync. Sent after, it wouldn't
    // be in the store when the settings screen re-reads a moment later, and the
    // pack it just made would appear to have not been made.
    request.initial_state = [ROOM_PACK, ROOM_PACK_LEGACY]
        .into_iter()
        .map(|event_type| {
            Raw::new(&serde_json::json!({
                "type": event_type,
                "state_key": state_key,
                "content": content,
            }))
            .map(Raw::cast_unchecked)
            .map_err(|e| Error::Other(e.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;

    // A room type marks this as something other than a conversation, the way
    // `m.space` does. Clients that don't recognise it — including our own room
    // list — know to leave it out of the sidebar.
    request.creation_content = Some(
        Raw::new(&serde_json::json!({ "type": PACK_ROOM_TYPE }))
            .map(Raw::cast_unchecked)
            .map_err(|e| Error::Other(e.to_string()))?,
    );

    let room = core.client.create_room(request).await?;
    let room_id = room.room_id().to_owned();

    set_everywhere(core, &room_id, &state_key, true).await?;

    // The room list is fed by sync, so the pack isn't readable the instant the
    // server says yes. Waiting for it here is what lets the caller re-read and
    // actually see what it just made.
    core.client.await_room_remote_echo(&room_id).await;

    Ok(room_id.to_string())
}

/// A readable, room-unique state key for a new pack.
///
/// Only ever seen in a state-event dump, so it's a slug of the name rather than
/// a random id — and falls back to one when the name has nothing sluggable in
/// it, since a state key still has to exist.
fn state_key_for(name: &str) -> String {
    let slug: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    let slug = slug.trim_matches('-').to_owned();
    if slug.is_empty() { format!("pack-{}", uuid_ish()) } else { slug }
}

/// Enough randomness to keep two unnameable packs apart.
fn uuid_ish() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_owned())
}

/// Apply one edit to a pack, reading it first and writing it back.
pub async fn edit(core: &MatrixCore, target: PackTarget, edit: PackEdit) -> Result<()> {
    let room_id = target.room_id.as_deref().map(RoomId::parse).transpose()?;
    let state_key = target.state_key.clone();

    let mut content = one(core, room_id.as_deref(), state_key.as_deref()).await?;

    match edit {
        PackEdit::PutImage {
            shortcode,
            url,
            body,
            is_emoticon,
            is_sticker,
            width,
            height,
            size,
            mimetype,
        } => {
            if !url.starts_with("mxc://") {
                return Err(Error::Other("that image isn't on this server".into()));
            }

            let shortcode = check_shortcode(&shortcode)?;

            // Built on top of whatever is already under that shortcode, so
            // changing one thing about an image doesn't drop the fields the
            // client that made it knew about and we don't. A detail the caller
            // left out keeps its old value rather than being cleared.
            let mut image = content.images.remove(&shortcode).unwrap_or_default();
            let mut info = image.info.unwrap_or_default();

            info.w = width.or(info.w);
            info.h = height.or(info.h);
            info.size = size.or(info.size);
            info.mimetype = mimetype.or(info.mimetype);

            image.url = url;
            image.body = body.filter(|b| !b.trim().is_empty()).or(image.body);
            image.info = Some(info);
            image.usage = usage_list(is_emoticon, is_sticker);

            content.images.insert(shortcode, image);
        }

        PackEdit::Rename { from, to } => {
            let to = check_shortcode(&to)?;
            let Some(image) = content.images.remove(&from) else {
                return Err(Error::Other(format!("there's no :{from}: in this pack")));
            };
            content.images.insert(to, image);
        }

        PackEdit::RemoveImage { shortcode } => {
            content.images.remove(&shortcode);
        }

        PackEdit::SetName { name } => {
            let name = name.trim().to_owned();
            let info = content.pack.get_or_insert_with(PackInfo::default);
            // An empty name isn't stored as one: dropping the field lets the
            // reader fall back to its default rather than showing a blank card.
            info.display_name = (!name.is_empty()).then_some(name);
        }
    }

    match (room_id, state_key) {
        (Some(room), Some(key)) => save_room_pack(core, &room, &key, content).await,
        _ => save_user_pack(core, content).await,
    }
}

/// The MSC's `usage` list, or `None` when an image is good for both.
fn usage_list(is_emoticon: bool, is_sticker: bool) -> Option<Vec<String>> {
    match (is_emoticon, is_sticker) {
        (true, true) => None,
        (true, false) => Some(vec!["emoticon".to_owned()]),
        (false, true) => Some(vec!["sticker".to_owned()]),
        // Neither is not a state the UI offers, and an empty list means "both"
        // to every reader — including ours — so treat it as emoticon-only.
        (false, false) => Some(vec!["emoticon".to_owned()]),
    }
}

/// Read one pack back for editing, so a write is always against what's there.
///
/// Reading first matters: two clients editing the same pack would otherwise
/// have the second overwrite whatever the first added.
pub async fn one(
    core: &MatrixCore,
    room_id: Option<&RoomId>,
    state_key: Option<&str>,
) -> Result<ImagePackContent> {
    match (room_id, state_key) {
        (Some(room), Some(key)) => Ok(room_packs(core, room)
            .await?
            .into_iter()
            .find(|(k, _)| k == key)
            .map(|(_, pack)| pack)
            .unwrap_or_default()),
        _ => Ok(user_pack(core).await?.unwrap_or_default()),
    }
}

/// Whether this account may write a room's packs.
///
/// Defaults to "no" rather than erroring: a room whose power levels we can't
/// read yet should show its packs as read-only, not fail the whole picker.
pub async fn can_edit_packs(core: &MatrixCore, room_id: &RoomId) -> bool {
    let Ok(room) = core.room(&room_id.to_owned()) else { return false };
    let Ok(user_id) = core.own_user_id() else { return false };

    // Both names have to be writable: a pack written under one and not the
    // other would look deleted to half the clients that read it.
    let levels = room.power_levels_or_default().await;
    [ROOM_PACK, ROOM_PACK_LEGACY]
        .into_iter()
        .all(|event_type| levels.user_can_send_state(&user_id, StateEventType::from(event_type)))
}

fn to_dto(
    content: ImagePackContent,
    room_id: Option<String>,
    state_key: Option<String>,
    everywhere: bool,
    can_edit: bool,
) -> ImagePackDto {
    let images = content.to_images();
    let info = content.pack.unwrap_or_default();

    let id = match (&room_id, &state_key) {
        (Some(room), Some(key)) => format!("{room}|{key}"),
        _ => "user".to_owned(),
    };

    let display_name = info
        .display_name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| match &room_id {
            Some(_) => "room pack".to_owned(),
            None => "your emotes".to_owned(),
        });

    ImagePackDto {
        id,
        source: if room_id.is_some() { PackSource::Room } else { PackSource::User },
        room_id,
        state_key,
        display_name,
        avatar_url: info.avatar_url.filter(|url| url.starts_with("mxc://")),
        attribution: info.attribution,
        images,
        everywhere,
        can_edit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: serde_json::Value) -> ImagePackContent {
        serde_json::from_value(json).expect("valid pack")
    }

    #[test]
    fn an_image_inherits_the_packs_usage() {
        let pack = parse(serde_json::json!({
            "images": { "blobcat": { "url": "mxc://veil.gg/a" } },
            "pack": { "usage": ["sticker"] }
        }));

        let images = pack.to_images();
        assert_eq!(images.len(), 1);
        assert!(images[0].is_sticker);
        assert!(!images[0].is_emoticon);
    }

    #[test]
    fn an_image_can_override_the_packs_usage() {
        let pack = parse(serde_json::json!({
            "images": {
                "blobcat": { "url": "mxc://veil.gg/a", "usage": ["emoticon"] }
            },
            "pack": { "usage": ["sticker"] }
        }));

        assert!(pack.to_images()[0].is_emoticon);
        assert!(!pack.to_images()[0].is_sticker);
    }

    #[test]
    fn no_usage_anywhere_means_both() {
        let pack = parse(serde_json::json!({
            "images": { "blobcat": { "url": "mxc://veil.gg/a" } }
        }));

        assert!(pack.to_images()[0].is_emoticon);
        assert!(pack.to_images()[0].is_sticker);
    }

    #[test]
    fn a_remote_url_is_dropped_rather_than_fetched() {
        let pack = parse(serde_json::json!({
            "images": {
                "tracker": { "url": "https://evil.example/pixel.gif" },
                "blobcat": { "url": "mxc://veil.gg/a" }
            }
        }));

        let codes: Vec<_> = pack.to_images().into_iter().map(|i| i.shortcode).collect();
        assert_eq!(codes, ["blobcat"]);
    }

    #[test]
    fn the_shortcode_stands_in_for_a_missing_body() {
        let pack = parse(serde_json::json!({
            "images": { "blobcat": { "url": "mxc://veil.gg/a" } }
        }));

        assert_eq!(pack.to_images()[0].body, "blobcat");
    }

    #[test]
    fn unknown_fields_dont_sink_a_pack() {
        // Other clients write things we've never heard of; a pack that carries
        // one is still a pack.
        let pack = parse(serde_json::json!({
            "images": {
                "blobcat": { "url": "mxc://veil.gg/a", "something_new": 42 }
            },
            "pack": { "display_name": "blobs", "future_field": true }
        }));

        assert_eq!(pack.to_images().len(), 1);
        assert_eq!(pack.pack.unwrap().display_name.unwrap(), "blobs");
    }

    #[test]
    fn usage_round_trips_through_the_msc_list() {
        // Both is the absence of a list, which is what every other client
        // writes and what our own reader treats as "usable anywhere".
        assert_eq!(usage_list(true, true), None);
        assert_eq!(usage_of(usage_list(true, true).as_ref()), (true, true));

        assert_eq!(usage_list(true, false), Some(vec!["emoticon".to_owned()]));
        assert_eq!(usage_of(usage_list(true, false).as_ref()), (true, false));

        assert_eq!(usage_list(false, true), Some(vec!["sticker".to_owned()]));
        assert_eq!(usage_of(usage_list(false, true).as_ref()), (false, true));
    }

    #[test]
    fn neither_usage_is_not_stored_as_an_empty_list() {
        // An empty list reads back as "both" everywhere, which would turn an
        // image the user disabled into one that's on in two places.
        let list = usage_list(false, false);
        assert_ne!(list, Some(Vec::new()));
        assert_eq!(usage_of(list.as_ref()), (true, false));
    }

    #[test]
    fn a_shortcode_follows_the_specs_grammar() {
        // ALPHA / DIGIT / "-" / "_", 1 to 100 of them.
        assert_eq!(check_shortcode("  blob-cat_2 ").unwrap(), "blob-cat_2");
        assert!(check_shortcode(&"a".repeat(100)).is_ok());

        for bad in ["", "   ", "blob cat", "blob:cat", ":blobcat:", "blob+cat", "ačiū"] {
            assert!(check_shortcode(bad).is_err(), "{bad:?} should be refused");
        }
        assert!(check_shortcode(&"a".repeat(101)).is_err());
    }

    #[test]
    fn editing_a_pack_keeps_fields_we_dont_understand() {
        // A pack made by a client that knows something we don't must survive
        // being read, changed and written back.
        let pack = parse(serde_json::json!({
            "images": {
                "blobcat": { "url": "mxc://veil.gg/a", "im.ponies.future": "keep me" }
            },
            "pack": { "display_name": "blobs", "attribution": "someone" },
            "org.example.whole_pack": { "nested": true }
        }));

        let round_tripped = serde_json::to_value(&pack).unwrap();
        assert_eq!(round_tripped["org.example.whole_pack"]["nested"], true);
        assert_eq!(round_tripped["images"]["blobcat"]["im.ponies.future"], "keep me");
        assert_eq!(round_tripped["pack"]["attribution"], "someone");
    }

    #[test]
    fn the_opaque_object_in_pack_rooms_survives() {
        // The spec reserves the innermost object for future use and asks that
        // clients preserve what they find there.
        let content: EmoteRoomsContent = serde_json::from_value(serde_json::json!({
            "rooms": { "!r:veil.gg": { "blobs": { "org.example.pinned": 1 } } },
            "org.example.top": "hello"
        }))
        .unwrap();

        let out = serde_json::to_value(&content).unwrap();
        assert_eq!(out["rooms"]["!r:veil.gg"]["blobs"]["org.example.pinned"], 1);
        assert_eq!(out["org.example.top"], "hello");
    }

    #[test]
    fn pack_usage_summarises_its_images() {
        let mut pack = parse(serde_json::json!({
            "images": {
                "a": { "url": "mxc://veil.gg/a", "usage": ["emoticon"] },
                "b": { "url": "mxc://veil.gg/b", "usage": ["emoticon"] }
            }
        }));
        derive_pack_usage(&mut pack);
        assert_eq!(pack.pack.unwrap().usage, Some(vec!["emoticon".to_owned()]));

        // A mixed pack says both, so a reader that only understands pack-level
        // usage surfaces it in both pickers rather than neither.
        let mut pack = parse(serde_json::json!({
            "images": {
                "a": { "url": "mxc://veil.gg/a", "usage": ["emoticon"] },
                "b": { "url": "mxc://veil.gg/b", "usage": ["sticker"] }
            }
        }));
        derive_pack_usage(&mut pack);
        assert_eq!(pack.pack.unwrap().usage, None);
    }

    #[test]
    fn an_empty_pack_keeps_the_usage_it_had() {
        // Removing the last image shouldn't rewrite what the pack is for.
        let mut pack = parse(serde_json::json!({
            "images": {},
            "pack": { "usage": ["sticker"] }
        }));
        derive_pack_usage(&mut pack);
        assert_eq!(pack.pack.unwrap().usage, Some(vec!["sticker".to_owned()]));
    }

    #[test]
    fn a_state_key_is_a_slug_of_the_name() {
        assert_eq!(state_key_for("The Blob Pack"), "the-blob-pack");
        assert_eq!(state_key_for("  spaces  "), "spaces");
        assert!(state_key_for("🐱🐱").starts_with("pack-"));
    }

    #[test]
    fn a_pack_with_no_name_gets_one() {
        let dto = to_dto(ImagePackContent::default(), None, None, true, true);
        assert_eq!(dto.display_name, "your emotes");

        let dto = to_dto(
            ImagePackContent::default(),
            Some("!r:veil.gg".to_owned()),
            Some(String::new()),
            false,
            false,
        );
        assert_eq!(dto.display_name, "room pack");
        assert_eq!(dto.id, "!r:veil.gg|");
    }
}
