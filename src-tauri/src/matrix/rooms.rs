//! Room summaries and the stream that keeps the sidebar current.

use std::sync::Arc;

use futures_util::{StreamExt, pin_mut};
use matrix_sdk::{
    Room, RoomMemberships, RoomState,
    ruma::{
        RoomId,
        events::room::{member::MembershipState, power_levels::UserPowerLevel},
    },
};
use matrix_sdk_ui::room_list_service::filters;
use tauri::AppHandle;
use tokio::task::JoinHandle;
use ts_rs::TS;

use crate::{
    dto::{
        Diff, LatestEvent, NotifyMode, RoomMemberDto, RoomMembership, RoomSummary, RoomsUpdate, SpaceSummary,
        Verification,
    },
    error::{Error, Result},
    events::EV_ROOMS,
    matrix::MatrixCore,
};

/// How many rooms the sliding-sync window holds at once. Large enough that a
/// normal account is fully loaded on first paint, small enough to stay cheap.
const ROOM_PAGE_SIZE: usize = 300;

/// Spell out the membership rather than `Debug`-formatting it: the frontend
/// branches on these strings, so they're an API, not a debugging aid.
fn membership_state_str(state: &MembershipState) -> &'static str {
    match state {
        MembershipState::Join => "join",
        MembershipState::Invite => "invite",
        MembershipState::Leave => "leave",
        MembershipState::Ban => "ban",
        MembershipState::Knock => "knock",
        _ => "unknown",
    }
}

/// The room types that mean "a call you join", not "a room you read".
///
/// There is no agreed value. MSC3417 proposed `org.matrix.msc3417.call`,
/// Element shipped `m.video_room` and, before that, `io.element.video`, and a
/// room made by any of them is a room the user expects to see. Ruma only models
/// the MSC3417 spelling, and only behind a feature we don't enable, so the
/// comparison is on the raw string.
const CALL_ROOM_TYPES: &[&str] = &["m.video_room", "io.element.video", "org.matrix.msc3417.call"];

/// Whether a room is a call rather than a conversation.
pub(crate) fn is_call_room_type(room_type: Option<&str>) -> bool {
    room_type.is_some_and(|t| CALL_ROOM_TYPES.contains(&t))
}

/// Whether a room exists to hold data rather than to be visited.
///
/// This used to be "any room type we don't recognise", which is what MSC1772
/// suggests — and it made voice rooms vanish from the sidebar while every other
/// client still listed them under their space. A room the user joined
/// disappearing is a much worse failure than an odd room appearing, so the test
/// is now an explicit list of the types we know are storage.
///
/// Spaces aren't listed: they're drawn in their own rail and carry `is_space`.
fn is_utility_room_type(room_type: Option<&str>) -> bool {
    room_type == Some(crate::matrix::packs::PACK_ROOM_TYPE)
}

fn room_type_str(room: &Room) -> Option<String> {
    room.room_type().map(|t| t.as_str().to_owned())
}

pub fn membership(state: RoomState) -> RoomMembership {
    match state {
        RoomState::Joined => RoomMembership::Joined,
        RoomState::Left => RoomMembership::Left,
        RoomState::Invited => RoomMembership::Invited,
        RoomState::Knocked => RoomMembership::Knocked,
        RoomState::Banned => RoomMembership::Banned,
    }
}

/// Pull a preview line out of a room's latest event.
///
/// We read the raw JSON rather than matching every event type: the sidebar only
/// needs one line of text, and a preview is never worth failing a room summary
/// over.
async fn latest_event(room: &Room) -> Option<LatestEvent> {
    use matrix_sdk_base::latest_event::LatestEventValue;

    let value = room.latest_event();
    let timestamp = value.timestamp().map(|t| t.0.into()).unwrap_or(0);

    let (sender, body) = match &value {
        LatestEventValue::Remote(event) => {
            let json: serde_json::Value = event.raw().deserialize_as_unchecked().ok()?;
            let sender = json.get("sender")?.as_str()?.to_owned();
            let body = preview_from_content(&json)?;
            (sender, body)
        }
        LatestEventValue::LocalIsSending(local)
        | LatestEventValue::LocalCannotBeSent(local)
        | LatestEventValue::LocalHasBeenSent { value: local, .. } => {
            let content = local.content.deserialize().ok()?;
            let json = serde_json::to_value(&content).ok()?;
            let body = preview_from_content(&serde_json::json!({ "content": json }))
                .unwrap_or_else(|| "…".to_owned());
            (room.own_user_id().to_string(), body)
        }
        LatestEventValue::RemoteInvite { inviter, .. } => (
            inviter.as_ref().map(|u| u.to_string()).unwrap_or_default(),
            "invited you".to_owned(),
        ),
        LatestEventValue::None => return None,
    };

    let sender_name = match matrix_sdk::ruma::UserId::parse(&sender) {
        Ok(user_id) => room
            .get_member_no_sync(&user_id)
            .await
            .ok()
            .flatten()
            .and_then(|m| m.display_name().map(str::to_owned)),
        Err(_) => None,
    };

    Some(LatestEvent { sender, sender_name, body, timestamp })
}

fn preview_from_content(json: &serde_json::Value) -> Option<String> {
    let content = json.get("content")?;
    if let Some(body) = content.get("body").and_then(|b| b.as_str()) {
        return Some(body.chars().take(140).collect());
    }
    match json.get("type").and_then(|t| t.as_str()) {
        Some("m.room.encrypted") => Some("encrypted message".to_owned()),
        Some("m.sticker") => Some("sticker".to_owned()),
        Some("m.room.member") => Some("membership change".to_owned()),
        Some(other) => Some(other.to_owned()),
        None => None,
    }
}

pub async fn summarise(room: &Room) -> Result<RoomSummary> {
    let counts = room.unread_notification_counts();
    let notification_count = counts.notification_count;
    let highlight_count = counts.highlight_count;
    let is_marked_unread = room.is_marked_unread();

    let name = match room.cached_display_name() {
        Some(name) => name.to_string(),
        None => room
            .display_name()
            .await
            .map(|n| n.to_string())
            .unwrap_or_else(|_| room.room_id().to_string()),
    };

    // `is_direct` hits the store, and a store hiccup shouldn't sink the whole
    // summary — a room that's wrongly grouped is better than a missing room.
    let is_direct = room.is_direct().await.unwrap_or(false);

    let notify_mode = room.cached_user_defined_notification_mode().map(NotifyMode::from);
    let muted = notify_mode == Some(NotifyMode::Mute);

    let parent_spaces = collect_parent_spaces(room).await;
    let room_type = room_type_str(room);
    let room_type = room_type.as_deref();

    Ok(RoomSummary {
        id: room.room_id().to_string(),
        name,
        topic: room.topic(),
        canonical_alias: room.canonical_alias().map(|a| a.to_string()),
        avatar_url: room.avatar_url().map(|u| u.to_string()),
        is_direct,
        dm_user_id: is_direct
            .then(|| crate::matrix::presence::dm_partner(room))
            .flatten()
            .map(|u| u.to_string()),
        is_encrypted: room.encryption_state().is_encrypted(),
        is_space: room.is_space(),
        is_utility: is_utility_room_type(room_type),
        is_muted: muted,
        notify_mode,
        membership: membership(room.state()),
        notification_count,
        highlight_count,
        has_unread: notification_count > 0 || highlight_count > 0 || is_marked_unread,
        is_marked_unread,
        member_count: room.joined_members_count(),
        latest: latest_event(room).await,
        recency: room.recency_stamp().map(|s| s.into()).unwrap_or(0),
        parent_spaces,
        has_active_call: room.has_active_room_call(),
        call_participants: call_participants(room),
        is_video_room: room.is_call() || is_call_room_type(room_type),
    })
}

/// Everyone in the room's call, in the order they joined. The SDK lists one
/// entry per membership, so someone on their phone and laptop appears twice.
fn call_participants(room: &Room) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for user in room.active_room_call_participants() {
        let user = user.to_string();
        if !out.contains(&user) {
            out.push(user);
        }
    }
    out
}

async fn collect_parent_spaces(room: &Room) -> Vec<String> {
    use matrix_sdk::room::ParentSpace;

    let Ok(stream) = room.parent_spaces().await else {
        return Vec::new();
    };
    pin_mut!(stream);

    let mut out = Vec::new();
    while let Some(Ok(parent)) = stream.next().await {
        let id = match parent {
            ParentSpace::Reciprocal(space) | ParentSpace::WithPowerlevel(space) => {
                space.room_id().to_owned()
            }
            ParentSpace::Illegitimate(space) => space.room_id().to_owned(),
            ParentSpace::Unverifiable(room_id) => room_id,
        };
        out.push(id.to_string());
    }
    out
}

/// Stream room-list changes to the frontend as diffs.
///
/// The sliding-sync list yields `VectorDiff`s; we translate each one and pass it
/// through so the UI mutates its list in place. A full `Reset` arrives first,
/// and again whenever the filter changes.
pub fn spawn_room_list_task(app: AppHandle, core: Arc<MatrixCore>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let all_rooms = match core.room_list.all_rooms().await {
            Ok(rooms) => rooms,
            Err(e) => {
                tracing::error!("couldn't open the room list: {e}");
                return;
            }
        };

        let (stream, controller) = all_rooms.entries_with_dynamic_adapters(ROOM_PAGE_SIZE);
        // Nothing is emitted until a filter is set. "Everything we haven't left"
        // is the list the sidebar wants; joined/invited splitting happens in the UI.
        controller.set_filter(Box::new(filters::new_filter_non_left()));

        pin_mut!(stream);

        while let Some(diffs) = stream.next().await {
            if core.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }

            let mut converted = Vec::with_capacity(diffs.len());
            for diff in diffs {
                converted.push(convert_room_diff(diff).await);
            }

            // Fold into the mirror and take a sequence number in one locked
            // step, so the snapshot a `get_rooms` returns always states exactly
            // which batches it already contains.
            let update = {
                let mut mirror = core.rooms.lock().await;
                for diff in &converted {
                    apply_diff(&mut mirror.rooms, diff);
                }
                mirror.seq += 1;
                RoomsUpdate { seq: mirror.seq, diffs: converted }
            };

            crate::events::emit(&app, EV_ROOMS, update);
        }
    })
}

/// Push a fresh summary of one room, as if the room list had reported it.
///
/// The sidebar's stream only emits a diff when the SDK marks a room's info
/// notable, and notification settings aren't room state — they're global push
/// rules — so nothing about muting reaches the UI on its own.
///
/// Folding into the mirror under the same lock the stream uses keeps `seq`
/// meaning what it claims to: batch N is the Nth change to the list.
pub async fn emit_room_refresh(app: &AppHandle, core: &MatrixCore, room_id: &RoomId) {
    let Ok(room) = core.room(&room_id.to_owned()) else { return };
    let value = summarise_or_placeholder(&room).await;

    let update = {
        let mut mirror = core.rooms.lock().await;
        // A room the UI doesn't list has nothing to update: emitting a `Set`
        // for an index we made up would shuffle somebody else's row.
        let Some(index) = mirror.rooms.iter().position(|r| r.id == value.id) else {
            return;
        };
        mirror.rooms[index] = value.clone();
        mirror.seq += 1;
        RoomsUpdate { seq: mirror.seq, diffs: vec![Diff::Set { index, value }] }
    };

    crate::events::emit(app, EV_ROOMS, update);
}

/// Replay a diff onto the mirror. Index-bearing operations are bounds-checked:
/// a malformed diff should leave the mirror stale, never panic the sync task.
fn apply_diff(list: &mut Vec<RoomSummary>, diff: &Diff<RoomSummary>) {
    match diff {
        Diff::Append { values } => list.extend(values.iter().cloned()),
        Diff::Clear => list.clear(),
        Diff::PushFront { value } => list.insert(0, value.clone()),
        Diff::PushBack { value } => list.push(value.clone()),
        Diff::PopFront => {
            if !list.is_empty() {
                list.remove(0);
            }
        }
        Diff::PopBack => {
            list.pop();
        }
        Diff::Insert { index, value } if *index <= list.len() => {
            list.insert(*index, value.clone())
        }
        Diff::Set { index, value } if *index < list.len() => list[*index] = value.clone(),
        Diff::Remove { index } if *index < list.len() => {
            list.remove(*index);
        }
        Diff::Truncate { length } => list.truncate(*length),
        Diff::Reset { values } => *list = values.clone(),
        other => tracing::warn!("out-of-range room diff ignored: {other:?}"),
    }
}

async fn summarise_or_placeholder(room: &Room) -> RoomSummary {
    match summarise(room).await {
        Ok(summary) => summary,
        Err(e) => {
            tracing::warn!("couldn't summarise {}: {e}", room.room_id());
            RoomSummary {
                id: room.room_id().to_string(),
                name: room.room_id().to_string(),
                topic: None,
                canonical_alias: None,
                avatar_url: None,
                is_direct: false,
                dm_user_id: None,
                is_encrypted: false,
                is_space: false,
                is_utility: false,
                is_muted: false,
                notify_mode: None,
                membership: membership(room.state()),
                notification_count: 0,
                highlight_count: 0,
                has_unread: false,
                is_marked_unread: false,
                member_count: 0,
                latest: None,
                recency: 0,
                parent_spaces: Vec::new(),
                has_active_call: false,
                call_participants: Vec::new(),
                is_video_room: false,
            }
        }
    }
}

async fn summarise_all(rooms: Vec<impl std::ops::Deref<Target = Room>>) -> Vec<RoomSummary> {
    let mut out = Vec::with_capacity(rooms.len());
    for room in rooms {
        out.push(summarise_or_placeholder(&room).await);
    }
    out
}

async fn convert_room_diff(
    diff: matrix_sdk_ui::eyeball_im::VectorDiff<matrix_sdk_ui::room_list_service::RoomListItem>,
) -> Diff<RoomSummary> {
    use matrix_sdk_ui::eyeball_im::VectorDiff;

    match diff {
        VectorDiff::Append { values } => {
            Diff::Append { values: summarise_all(values.into_iter().collect()).await }
        }
        VectorDiff::Clear => Diff::Clear,
        VectorDiff::PushFront { value } => {
            Diff::PushFront { value: summarise_or_placeholder(&value).await }
        }
        VectorDiff::PushBack { value } => {
            Diff::PushBack { value: summarise_or_placeholder(&value).await }
        }
        VectorDiff::PopFront => Diff::PopFront,
        VectorDiff::PopBack => Diff::PopBack,
        VectorDiff::Insert { index, value } => {
            Diff::Insert { index, value: summarise_or_placeholder(&value).await }
        }
        VectorDiff::Set { index, value } => {
            Diff::Set { index, value: summarise_or_placeholder(&value).await }
        }
        VectorDiff::Remove { index } => Diff::Remove { index },
        VectorDiff::Truncate { length } => Diff::Truncate { length },
        VectorDiff::Reset { values } => {
            Diff::Reset { values: summarise_all(values.into_iter().collect()).await }
        }
    }
}

// ---------------------------------------------------------------------------
// one-shot queries
// ---------------------------------------------------------------------------

pub async fn members(core: &MatrixCore, room_id: &RoomId) -> Result<Vec<RoomMemberDto>> {
    let room = core
        .client
        .get_room(room_id)
        .ok_or_else(|| Error::UnknownRoom(room_id.to_string()))?;

    // The member list is lazily loaded; make sure we have it before reading.
    if !room.are_members_synced() {
        room.sync_members().await?;
    }

    let members = room.members(RoomMemberships::ACTIVE).await?;
    let encryption = core.client.encryption();

    let mut out = Vec::with_capacity(members.len());
    for member in members {
        // Cross-signing state is per-user and needs a store read, so a failure
        // degrades to "unknown" rather than dropping the member.
        let verification = match encryption.get_user_identity(member.user_id()).await {
            Ok(Some(identity)) if identity.is_verified() => Verification::Verified,
            Ok(Some(_)) => Verification::Unverified,
            _ => Verification::Unknown,
        };

        out.push(RoomMemberDto {
            user_id: member.user_id().to_string(),
            display_name: member.display_name().map(str::to_owned),
            avatar_url: member.avatar_url().map(|u| u.to_string()),
            // Room creators have "infinite" power from room version 12; clamp
            // so the value stays sortable alongside ordinary integer levels.
            power_level: match member.power_level() {
                UserPowerLevel::Infinite => i64::MAX,
                UserPowerLevel::Int(v) => v.into(),
                // `UserPowerLevel` is non-exhaustive; treat anything new as an
                // ordinary member rather than guessing at its authority.
                _ => 0,
            },
            membership: membership_state_str(member.membership()).to_owned(),
            is_ignored: member.is_ignored(),
            verification,
        });
    }

    // Power level descending, then name — the ordering the design's member list uses.
    out.sort_by(|a, b| {
        b.power_level
            .cmp(&a.power_level)
            .then_with(|| a.display_name.cmp(&b.display_name))
    });

    Ok(out)
}

/// Spaces the user is in, with the rooms each one contains.
pub async fn spaces(core: &MatrixCore) -> Result<Vec<SpaceSummary>> {
    let mut out = Vec::new();

    for room in core.client.joined_rooms() {
        if !room.is_space() {
            continue;
        }

        let children = space_children(&room).await;
        let counts = room.unread_notification_counts();

        out.push(SpaceSummary {
            id: room.room_id().to_string(),
            name: room
                .cached_display_name()
                .map(|n| n.to_string())
                .unwrap_or_else(|| room.room_id().to_string()),
            avatar_url: room.avatar_url().map(|u| u.to_string()),
            children,
            notification_count: counts.notification_count,
            highlight_count: counts.highlight_count,
        });
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

async fn space_children(space: &Room) -> Vec<String> {
    use matrix_sdk::ruma::events::space::child::SpaceChildEventContent;

    let Ok(events) = space.get_state_events_static::<SpaceChildEventContent>().await else {
        return Vec::new();
    };

    let mut children: Vec<ChildOrder> = events
        .into_iter()
        .filter_map(|raw| {
            let event = raw.deserialize().ok()?;
            let state_key = event.state_key().to_owned();

            // Spec says a child whose content is empty has been removed from the
            // space. We only check that the event still has content rather than
            // insisting on a non-empty `via`: not every server round-trips that
            // field, and dropping a real child leaves the space looking empty —
            // a much worse failure than keeping a stale one.
            let original = event.as_sync()?.as_original()?;
            Some(ChildOrder {
                order: original.content.order.as_ref().map(|o| o.as_str().to_owned()),
                ts: original.origin_server_ts.0.into(),
                id: state_key.to_string(),
            })
        })
        .collect();

    // The sidebar lists a space's rooms in this order, so it has to be the one
    // every client agrees on rather than whatever order the store kept them in.
    children.sort_by(ChildOrder::cmp);
    children.into_iter().map(|c| c.id).collect()
}

/// Where a room sits in its space, by the spec's rule: an explicit `order`
/// first, compared as strings; then when it was added; then the room ID, so
/// two clients never disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChildOrder {
    order: Option<String>,
    ts: u64,
    id: String,
}

impl ChildOrder {
    fn cmp(a: &Self, b: &Self) -> std::cmp::Ordering {
        // Rooms with an order come before rooms without one.
        let by_order = match (&a.order, &b.order) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        by_order.then(a.ts.cmp(&b.ts)).then_with(|| a.id.cmp(&b.id))
    }
}

// ---------------------------------------------------------------------------
// room actions
// ---------------------------------------------------------------------------

/// What the create-room dialog collects.
#[derive(Debug, Clone, serde::Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NewRoom {
    pub name: String,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub topic: Option<String>,
    /// Anyone can find and join it, and it gets listed in the directory.
    #[serde(default)]
    #[ts(as = "Option<_>", optional)]
    pub is_public: bool,
    /// Alias localpart for a public room — `movies`, not `#movies:server`.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub alias: Option<String>,
    #[serde(default)]
    #[ts(as = "Option<_>", optional)]
    pub encrypted: bool,
    #[serde(default)]
    #[ts(as = "Option<_>", optional)]
    pub invite: Vec<String>,
    /// The space to file it under, if one was open when the user hit create.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub parent_space: Option<String>,
    /// A voice room: a call you join by opening it, with chat alongside.
    #[serde(default)]
    #[ts(as = "Option<_>", optional)]
    pub voice: bool,
}

/// The room type a new voice room gets. Of the spellings in
/// [`CALL_ROOM_TYPES`] it's the one Element Call's rooms use and the one Ruma
/// knows as `RoomType::Call`, so Element and the SDK both see a call room.
const VOICE_ROOM_TYPE: &str = "org.matrix.msc3417.call";

/// Power levels for a room we create, so that everyone in it can join a call.
///
/// Joining is writing a call membership state event, and state defaults to
/// moderators only, which leaves a call nobody else can join. The override
/// replaces the default `events` map wholesale, so it restates Synapse's
/// defaults rather than losing them — the same thing Element does.
fn power_levels_for_calls() -> serde_json::Value {
    serde_json::json!({
        "events": {
            "m.room.name": 50,
            "m.room.power_levels": 100,
            "m.room.history_visibility": 100,
            "m.room.canonical_alias": 50,
            "m.room.avatar": 50,
            "m.room.tombstone": 100,
            "m.room.server_acl": 100,
            "m.room.encryption": 100,
            "org.matrix.msc3401.call.member": 0,
        },
    })
}

#[derive(Debug, Clone, serde::Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NewRoomResult {
    pub room_id: String,
    /// The room exists, but it couldn't be filed under the space. Not an error:
    /// the room is real and usable, it's just loose.
    pub space_warning: Option<String>,
}

/// Create a room, and file it under a space if one was open.
pub async fn create(core: &MatrixCore, new: NewRoom) -> Result<NewRoomResult> {
    use matrix_sdk::ruma::{
        api::client::room::{Visibility, create_room},
        events::{InitialStateEvent, room::encryption::RoomEncryptionEventContent},
        serde::Raw,
    };

    let name = new.name.trim();
    if name.is_empty() {
        return Err(Error::Other("give it a name first~".into()));
    }

    let mut request = create_room::v3::Request::new();
    request.name = Some(name.to_owned());
    request.topic = new.topic.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());

    request.preset = Some(if new.is_public {
        create_room::v3::RoomPreset::PublicChat
    } else {
        create_room::v3::RoomPreset::PrivateChat
    });
    // Directory listing follows the join rule: a room nobody may join has no
    // business being advertised.
    request.visibility = if new.is_public { Visibility::Public } else { Visibility::Private };

    // An alias is a nickname for the room, not a permission: an invite-only
    // room can have one, and having one doesn't let anybody in. Who can join is
    // the join rule, set by the preset above.
    //
    // The user types `movies`; tolerate them typing `#movies:server` too.
    request.room_alias_name = new
        .alias
        .map(|a| a.trim().trim_start_matches('#').split(':').next().unwrap_or("").to_owned())
        .filter(|a| !a.is_empty());

    for user in &new.invite {
        request.invite.push(matrix_sdk::ruma::UserId::parse(user)?);
    }

    // Encryption can only ever be turned on here. There is no supported way to
    // encrypt a room after the fact, and no way at all to undo it.
    if new.encrypted {
        request.initial_state = vec![
            InitialStateEvent::with_empty_state_key(
                RoomEncryptionEventContent::with_recommended_defaults(),
            )
            .to_raw_any(),
        ];
    }

    // Every room, not just voice rooms: a text room has a call button too.
    request.power_level_content_override = Some(
        Raw::new(&power_levels_for_calls())
            .map(Raw::cast_unchecked)
            .map_err(|e| Error::Other(e.to_string()))?,
    );

    if new.voice {
        request.creation_content = Some(
            Raw::new(&serde_json::json!({ "type": VOICE_ROOM_TYPE }))
                .map(Raw::cast_unchecked)
                .map_err(|e| Error::Other(e.to_string()))?,
        );
    }

    let room = core.client.create_room(request).await?;
    let room_id = room.room_id().to_owned();

    let space_warning = match new.parent_space {
        Some(space_id) => add_to_space(core, &space_id, &room_id).await.err().map(|e| {
            format!("made the room, but couldn't add it to that space — {e}")
        }),
        None => None,
    };

    Ok(NewRoomResult { room_id: room_id.to_string(), space_warning })
}

/// Record a room as a child of a space, from both ends.
///
/// Membership of a space is written twice and neither half is reliable alone —
/// the trap in ARCHITECTURE.md. The space's `m.space.child` is the half other
/// clients read, but it's a state event in the *space*, which is frequently
/// someone else's; the room's own `m.space.parent` is one we can always write,
/// because we just created the room and hold every power in it.
///
/// Writing both is also what makes a new room appear under its space *now*: the
/// room summary carries `parent_spaces`, which arrives with the room's own
/// diff, while the space's children list is only re-read on a slow poll.
///
/// Best-effort by design. A room that exists but isn't filed is a far better
/// outcome than a failed creation.
async fn add_to_space(core: &MatrixCore, space_id: &str, child: &RoomId) -> Result<()> {
    use matrix_sdk::ruma::events::space::{
        child::SpaceChildEventContent, parent::SpaceParentEventContent,
    };

    let space_id = RoomId::parse(space_id)?;
    let space = core
        .client
        .get_room(&space_id)
        .ok_or_else(|| Error::UnknownRoom(space_id.to_string()))?;

    // `via` is where someone should try to join; the server that just created
    // the room is the only one that certainly has it.
    let server = core.own_user_id()?.server_name().to_owned();

    // The child's side first: it always succeeds, so the room is where the user
    // expects it even when the space rejects us.
    if let Some(room) = core.client.get_room(child) {
        let mut parent = SpaceParentEventContent::new(vec![server.clone()]);
        parent.canonical = true;
        if let Err(e) = room.send_state_event_for_key(&space_id, parent).await {
            tracing::warn!("couldn't record the space as this room's parent: {e}");
        }
    }

    space.send_state_event_for_key(child, SpaceChildEventContent::new(vec![server])).await?;

    Ok(())
}

/// Rename a room and/or change its topic. `None` leaves a field alone.
pub async fn update(
    core: &MatrixCore,
    room_id: &RoomId,
    name: Option<String>,
    topic: Option<String>,
) -> Result<()> {
    let room = core.room(&room_id.to_owned())?;

    if let Some(name) = name {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Err(Error::Other("a room needs a name~".into()));
        }
        room.set_name(name).await?;
    }

    // An empty topic is a real value: it's how you clear one.
    if let Some(topic) = topic {
        room.set_room_topic(topic.trim()).await?;
    }

    Ok(())
}

/// What this account is allowed to change about a room.
///
/// Read once when the room panel opens rather than carried on every summary:
/// it needs the power-level state event, and the answer only matters for the
/// room you're looking at.
#[derive(Debug, Clone, serde::Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RoomPermissions {
    pub can_rename: bool,
    pub can_set_topic: bool,
    pub can_invite: bool,
}

pub async fn permissions(core: &MatrixCore, room_id: &RoomId) -> Result<RoomPermissions> {
    use matrix_sdk::ruma::events::StateEventType;

    let room = core.room(&room_id.to_owned())?;
    let user_id = core.own_user_id()?;

    // `_or_default` rather than a hard error: a room whose power levels we
    // can't read yet should let the user try and let the server be the
    // authority, not refuse on a guess.
    let levels = room.power_levels_or_default().await;

    Ok(RoomPermissions {
        can_rename: levels.user_can_send_state(&user_id, StateEventType::RoomName),
        can_set_topic: levels.user_can_send_state(&user_id, StateEventType::RoomTopic),
        can_invite: levels.user_can_invite(&user_id),
    })
}

/// How long a join may take before we give up on hearing back.
///
/// Joining a room on another homeserver is the slowest call in the API: ours
/// has to fetch the room's whole state over federation before it answers, and
/// for a big room on matrix.org that's routinely minutes. The SDK's default of
/// 30 seconds turned those joins into an "error sending request" while the
/// server carried on and joined anyway.
const JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Join by alias or ID.
///
/// `via` matters for a room ID on another homeserver: ours has never heard of
/// it and needs to be told whom to ask. An alias carries its own answer, which
/// the server resolves during the join.
pub async fn join(core: &MatrixCore, alias_or_id: &str, via: &[String]) -> Result<String> {
    use matrix_sdk::{config::RequestConfig, ruma::api::client::membership::join_room_by_id_or_alias};

    let target = matrix_sdk::ruma::RoomOrAliasId::parse(alias_or_id.trim())?;
    let servers = super::directory::parse_via(via);

    // The slow part, sent by hand so it can have a long timeout of its own
    // rather than raising it for every request. No retries: the server keeps
    // working on a join we stopped waiting for, and a retry just queues up
    // behind it.
    let mut request = join_room_by_id_or_alias::v3::Request::new(target);
    request.via = servers;
    let response = core
        .client
        .send(request)
        .with_request_config(RequestConfig::new().timeout(JOIN_TIMEOUT).disable_retry())
        .await?;

    // Then the SDK's own join, which is instant now that we're in, and does the
    // local bookkeeping the raw request skips — marking the room joined so it
    // can be opened before the next sync says so.
    let room = core.client.join_room_by_id_or_alias((&*response.room_id).into(), &[]).await?;
    Ok(room.room_id().to_string())
}

/// Leave a room. Deliberately *not* forget — see PLAN.md.
///
/// Matrix has no delete, and the two halves behave very differently. Leaving
/// tells the server you're out: the room turns `Left`, the sidebar filters it
/// away, and nothing local is deleted. Forgetting additionally drops the room
/// from the local store, which is where all four of the faults recorded in
/// PLAN.md came from — including one that can't be fixed from this side, where
/// a sync already in flight writes the room back and `restore_session` loads it
/// again at startup. So this leaves, and stops.
///
/// The open timeline goes first. It subscribes to the room's event cache, and a
/// stream still pulling on a room the server has stopped sending us is at best
/// pointless — the same subscription is what turned forgetting into a panic.
///
/// One exception is not ours to make: `Room::leave` forgets the room *for us*
/// when it was in the `Invited` state, because that's what declining an invite
/// means to the SDK. An invite has no local history to lose, so it doesn't grow
/// the ghost that PLAN.md describes, but it does mean "we never call forget" is
/// true of this function and not of the invite path underneath it.
///
/// Calling this on a room already left is a no-op, not an error: `leave_impl`
/// skips any room whose state is `Left` or `Banned` and reports success.
pub async fn leave(core: &MatrixCore, room_id: &RoomId) -> Result<()> {
    let room = core.room(&room_id.to_owned())?;
    tracing::debug!("leaving {room_id} (state {:?})", room.state());

    super::timeline::close_all(core, room_id).await;
    room.leave().await?;
    Ok(())
}

pub async fn set_typing(core: &MatrixCore, room_id: &RoomId, typing: bool) -> Result<()> {
    core.room(&room_id.to_owned())?.typing_notice(typing).await?;
    Ok(())
}

pub async fn set_marked_unread(core: &MatrixCore, room_id: &RoomId, unread: bool) -> Result<()> {
    core.room(&room_id.to_owned())?.set_unread_flag(unread).await?;
    Ok(())
}

pub async fn set_muted(core: &MatrixCore, room_id: &RoomId, muted: bool) -> Result<()> {
    use matrix_sdk::notification_settings::{IsEncrypted, IsOneToOne, RoomNotificationMode};

    let settings = core.client.notification_settings().await;
    let room = core.room(&room_id.to_owned())?;

    let result = if muted {
        settings.set_room_notification_mode(room_id, RoomNotificationMode::Mute).await
    } else {
        // Unmuting is `unmute_room`, not "delete the room's push rules".
        //
        // Deleting outright asks the server to remove whatever rules the SDK's
        // *cached* ruleset believes exist, and when that cache and the server
        // disagree the DELETE comes back 404 — at which point the SDK never
        // applies the change to its cache either, so the room stays muted
        // locally and every further click repeats the same doomed request.
        // That's the stream of `Push rule not found` in the log.
        //
        // `unmute_room` asks the question first: already unmuted, nothing to do;
        // muted by a rule, remove it; muted only because that's the default for
        // this kind of room, write an explicit "all messages" rule instead. It
        // also needs to know what kind of room this is to work out that default.
        let encrypted = if room.encryption_state().is_encrypted() {
            IsEncrypted::Yes
        } else {
            IsEncrypted::No
        };
        let one_to_one =
            if room.joined_members_count() == 2 { IsOneToOne::Yes } else { IsOneToOne::No };

        settings.unmute_room(room_id, encrypted, one_to_one).await
    };
    result.map_err(|e| Error::Other(e.to_string()))?;
    refresh_cached_mode(&settings, &room).await;
    Ok(())
}

/// Set how much a room notifies: everything, mentions only, or nothing.
///
/// "Nothing" is `set_muted`, so that turning it back off takes the careful
/// unmute path rather than a bare rule write.
pub async fn set_notify_mode(core: &MatrixCore, room_id: &RoomId, mode: NotifyMode) -> Result<()> {
    if mode == NotifyMode::Mute {
        return set_muted(core, room_id, true).await;
    }

    let settings = core.client.notification_settings().await;
    let room = core.room(&room_id.to_owned())?;
    settings
        .set_room_notification_mode(room_id, mode.into())
        .await
        .map_err(|e| Error::Other(e.to_string()))?;
    refresh_cached_mode(&settings, &room).await;
    Ok(())
}

async fn refresh_cached_mode(
    settings: &matrix_sdk::notification_settings::NotificationSettings,
    room: &Room,
) {
    let room_id = room.room_id();
    // `summarise` reads the mode from a per-room cache that the SDK only
    // refills when a sync brings the new `m.push_rules` back, so a summary
    // taken now would still report the old state. The rules read here are the
    // SDK's own, already updated by the call above, so this doesn't wait on the
    // server either.
    //
    // Not every unmute leaves a rule behind — dropping one that only duplicated
    // the default leaves no user-defined mode at all — and the "update" half of
    // the SDK's cache API can't express that, hence the clear.
    match settings.get_user_defined_room_notification_mode(room_id).await {
        Some(mode) => room.update_cached_user_defined_notification_mode(mode),
        None => room.clear_user_defined_notification_mode(),
    }
}

/// Mark a room read without opening it.
///
/// The timeline's own `mark_as_read` needs the room open, which "mark the
/// whole space read" can't ask for. A receipt on the latest event the room
/// list already knows about says the same thing, and clearing the manual
/// unread flag covers rooms marked unread by hand.
pub async fn mark_read(core: &MatrixCore, room_id: &RoomId) -> Result<()> {
    use matrix_sdk::ruma::{
        OwnedEventId,
        api::client::receipt::create_receipt::v3::ReceiptType,
        events::receipt::ReceiptThread,
    };
    use matrix_sdk_base::latest_event::LatestEventValue;

    let room = core.room(&room_id.to_owned())?;

    if let LatestEventValue::Remote(event) = room.latest_event() {
        let json: serde_json::Value = event.raw().deserialize_as_unchecked()?;
        if let Some(event_id) = json
            .get("event_id")
            .and_then(|id| id.as_str())
            .and_then(|id| OwnedEventId::try_from(id).ok())
        {
            room.send_single_receipt(ReceiptType::Read, ReceiptThread::Unthreaded, event_id)
                .await?;
        }
    }

    room.set_unread_flag(false).await?;
    Ok(())
}

pub async fn invite(core: &MatrixCore, room_id: &RoomId, user_id: &str) -> Result<()> {
    let user_id = matrix_sdk::ruma::UserId::parse(user_id)?;
    core.room(&room_id.to_owned())?.invite_user_by_id(&user_id).await?;
    Ok(())
}

#[cfg(test)]
mod room_type_tests {
    use super::*;
    use matrix_sdk::ruma::room::RoomType;

    use crate::matrix::packs::PACK_ROOM_TYPE;

    #[test]
    fn an_ordinary_room_is_neither() {
        assert!(!is_utility_room_type(None));
        assert!(!is_call_room_type(None));
    }

    #[test]
    fn our_pack_rooms_stay_out_of_the_sidebar() {
        assert!(is_utility_room_type(Some(PACK_ROOM_TYPE)));
    }

    #[test]
    fn a_voice_room_belongs_in_the_sidebar() {
        // The bug this guards: every one of these spellings used to be read as
        // "some room type we don't know", which hid the room entirely.
        for room_type in ["m.video_room", "io.element.video", "org.matrix.msc3417.call"] {
            assert!(is_call_room_type(Some(room_type)), "{room_type} should be a call room");
            assert!(!is_utility_room_type(Some(room_type)), "{room_type} should be visible");
        }
    }

    #[test]
    fn space_children_follow_the_spec_order() {
        let child = |order: Option<&str>, ts: u64, id: &str| ChildOrder {
            order: order.map(str::to_owned),
            ts,
            id: id.to_owned(),
        };
        let mut children = vec![
            child(None, 1, "!late-unordered"),
            child(Some("b"), 9, "!b"),
            child(None, 1, "!early-unordered"),
            child(Some("a"), 50, "!a"),
        ];
        children.sort_by(ChildOrder::cmp);
        let ids: Vec<_> = children.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["!a", "!b", "!early-unordered", "!late-unordered"]);
    }

    #[test]
    fn a_voice_room_we_make_is_one_we_recognise() {
        assert!(is_call_room_type(Some(VOICE_ROOM_TYPE)));
        assert_eq!(VOICE_ROOM_TYPE, RoomType::Call.as_str());
    }

    #[test]
    fn everyone_can_join_a_call_in_a_room_we_make() {
        let levels = power_levels_for_calls();
        assert_eq!(levels["events"]["org.matrix.msc3401.call.member"], 0);
        // Restated, not dropped: the override replaces the whole map.
        assert_eq!(levels["events"]["m.room.power_levels"], 100);
    }

    #[test]
    fn a_room_type_we_dont_know_is_still_shown() {
        assert!(!is_utility_room_type(Some("com.example.something")));
        assert!(!is_call_room_type(Some("com.example.something")));
    }

    #[test]
    fn a_space_is_not_utility() {
        // Spaces have their own flag and their own rail; classifying them as
        // storage here would take them out of both.
        assert!(!is_utility_room_type(Some("m.space")));
    }
}
