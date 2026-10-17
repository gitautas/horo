//! The wire format between the Rust core and the React frontend.
//!
//! Everything here is `camelCase` on the JS side and deliberately flat: the
//! frontend should never need to know a `matrix-sdk` type to render a message.
//!
//! Each type derives `TS`, which is how `src/lib/bindings` gets written — see
//! "Wire types" in ARCHITECTURE.md before adding one.

use matrix_sdk::ruma::{
    MilliSecondsSinceUnixEpoch, OwnedUserId,
    events::room::{MediaSource, message::MessageType},
};
use matrix_sdk_ui::timeline::{
    EventSendState, EventTimelineItem, MsgLikeKind, ReactionStatus, TimelineDetails, TimelineItem,
    TimelineItemContent, TimelineItemKind, VirtualTimelineItem,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ---------------------------------------------------------------------------
// auth / session
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct HomeserverInfo {
    pub server_name: String,
    pub homeserver_url: String,
    pub supports_password: bool,
    pub supports_sso: bool,
    pub sso_providers: Vec<SsoProvider>,
    /// MatrixRTC focus advertised in the server's `.well-known`, if any.
    pub livekit_service_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SsoProvider {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub user_id: String,
    pub device_id: String,
    pub homeserver: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    /// True when secrets had to be written to a file because the OS keychain
    /// was unavailable. The UI warns about this rather than hiding it.
    pub insecure_storage: bool,
}

/// One answer to a homeserver's sign-up questions, sent with the `session` the
/// previous [`RegisterStep::NeedsAuth`] handed out.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum RegisterAuth {
    /// `m.login.dummy` — a stage that asks nothing, there to make a flow non-empty.
    Dummy,
    /// `m.login.registration_token` — an invite code from the server's admin.
    #[serde(rename_all = "camelCase")]
    RegistrationToken { token: String },
    /// `m.login.terms` — the policies were shown and agreed to.
    Terms,
    /// Any stage finished in the browser on the server's fallback page.
    Fallback,
}

/// How the frontend can get through one stage of a sign-up flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum RegisterStageKind {
    Dummy,
    RegistrationToken,
    Terms,
    /// Done on the homeserver's own web page (captcha, mostly), then acknowledged.
    Fallback,
    /// Email and phone verification: these need an identity-server round trip
    /// and have no fallback page, so a flow containing one can't be finished.
    Unsupported,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RegisterStage {
    pub kind: RegisterStageKind,
    /// The raw `m.login.*` type — what `completed` lists.
    pub auth_type: String,
    /// Where to send the user for a [`RegisterStageKind::Fallback`] stage.
    pub fallback_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TermsPolicy {
    pub name: String,
    pub url: String,
}

/// What the server still wants before it creates the account.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RegisterChallenge {
    pub session: Option<String>,
    /// Each inner list is one acceptable sequence of stages.
    pub flows: Vec<Vec<RegisterStage>>,
    /// Stage types already passed in this session.
    pub completed: Vec<String>,
    /// The documents behind an `m.login.terms` stage, if any flow has one.
    pub policies: Vec<TermsPolicy>,
    /// Why the last answer was rejected (a wrong token, say).
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum RegisterStep {
    /// The account exists and is signed in.
    #[serde(rename_all = "camelCase")]
    Done { session: SessionInfo },
    #[serde(rename_all = "camelCase")]
    NeedsAuth { challenge: RegisterChallenge },
}

// ---------------------------------------------------------------------------
// rooms
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RoomSummary {
    pub id: String,
    pub name: String,
    pub topic: Option<String>,
    pub canonical_alias: Option<String>,
    pub avatar_url: Option<String>,
    pub is_direct: bool,
    /// The other person, when a DM has exactly one of them — what a presence
    /// dot in the sidebar hangs off. `None` for group rooms, and for "direct"
    /// rooms with any number of members other than one, which the flag allows.
    pub dm_user_id: Option<String>,
    pub is_encrypted: bool,
    pub is_space: bool,
    /// A room that isn't a conversation — a pack room, or anything else with a
    /// room type we don't draw. Spaces have their own flag and their own rail.
    pub is_utility: bool,
    pub is_muted: bool,
    pub membership: RoomMembership,
    pub notification_count: u64,
    pub highlight_count: u64,
    pub has_unread: bool,
    pub is_marked_unread: bool,
    pub member_count: u64,
    pub latest: Option<LatestEvent>,
    /// Server-side recency stamp, used to order the list.
    pub recency: u64,
    /// Space IDs this room is a child of.
    pub parent_spaces: Vec<String>,
    /// True while a MatrixRTC session is live in this room.
    pub has_active_call: bool,
    /// An MSC3417 video room — a call you join, not a text room.
    pub is_video_room: bool,
}

/// Where the account stands with a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum RoomMembership {
    Joined,
    Invited,
    Left,
    Knocked,
    Banned,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct LatestEvent {
    pub sender: String,
    pub sender_name: Option<String>,
    pub body: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SpaceSummary {
    pub id: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub children: Vec<String>,
    pub notification_count: u64,
    pub highlight_count: u64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "RoomMember")]
#[serde(rename_all = "camelCase")]
pub struct RoomMemberDto {
    pub user_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub power_level: i64,
    /// `join` | `invite` | `leave` | `ban` | `knock` | `unknown` — the event's
    /// own spelling, not `RoomMembership`'s.
    pub membership: String,
    pub is_ignored: bool,
    pub verification: Verification,
}

/// Whether we've verified someone's identity via cross-signing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum Verification {
    Verified,
    Unverified,
    /// No identity to judge — they have never set up cross-signing, or it
    /// couldn't be fetched.
    Unknown,
}

// ---------------------------------------------------------------------------
// timeline
// ---------------------------------------------------------------------------

/// One row of the timeline. `kind` discriminates; only the matching payload is
/// populated.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "TimelineItem")]
#[serde(rename_all = "camelCase")]
pub struct TimelineItemDto {
    /// Stable identity for React keys and for diff application.
    pub id: String,
    pub kind: TimelineRowKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub event: Option<Box<EventItemDto>>,
}

/// Which kind of row a `TimelineItemDto` is. Not the SDK's `TimelineItemKind`,
/// which carries the payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum TimelineRowKind {
    Event,
    DateDivider,
    ReadMarker,
    TimelineStart,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "EventItem")]
#[serde(rename_all = "camelCase")]
pub struct EventItemDto {
    pub event_id: Option<String>,
    pub transaction_id: Option<String>,
    pub sender: String,
    pub sender_name: Option<String>,
    pub sender_avatar: Option<String>,
    pub timestamp: u64,
    pub is_own: bool,
    pub is_editable: bool,
    pub can_reply: bool,
    pub is_highlighted: bool,
    pub is_edited: bool,
    /// True when the body is nothing but emoji — the design renders those big.
    pub is_emoji_only: bool,
    pub send_state: Option<SendStateDto>,
    pub shield: Option<ShieldDto>,
    pub content: ContentDto,
    pub reactions: Vec<ReactionDto>,
    pub reply: Option<ReplyDto>,
    pub thread_root: Option<String>,
    pub thread_summary: Option<ThreadSummaryDto>,
    pub read_receipts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "SendState")]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum SendStateDto {
    #[serde(rename_all = "camelCase")]
    NotSentYet { progress: Option<f32> },
    #[serde(rename_all = "camelCase")]
    Failed { error: String, is_recoverable: bool },
    #[serde(rename_all = "camelCase")]
    Sent { event_id: String },
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "Shield")]
#[serde(rename_all = "camelCase")]
pub struct ShieldDto {
    pub colour: ShieldColour,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum ShieldColour {
    Red,
    Grey,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "Reaction")]
#[serde(rename_all = "camelCase")]
pub struct ReactionDto {
    pub key: String,
    pub count: usize,
    /// True when the local user is one of the senders.
    pub mine: bool,
    /// True while our own reaction is still in flight.
    pub pending: bool,
    pub senders: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "ReplyInfo")]
#[serde(rename_all = "camelCase")]
pub struct ReplyDto {
    pub event_id: String,
    pub sender: Option<String>,
    pub sender_name: Option<String>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "ThreadSummary")]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummaryDto {
    pub num_replies: u32,
    pub latest_sender: Option<String>,
    pub latest_body: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "MediaInfo")]
#[serde(rename_all = "camelCase")]
pub struct MediaInfoDto {
    pub mxc: Option<String>,
    pub mimetype: Option<String>,
    pub size: Option<u64>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub duration_ms: Option<u64>,
    pub blurhash: Option<String>,
    pub thumbnail_mxc: Option<String>,
    /// Voice-message amplitude samples, when present.
    pub waveform: Option<Vec<u16>>,
    pub is_voice: bool,
}

/// Message content, flattened into what the UI actually needs to draw.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, rename = "Content")]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ContentDto {
    #[serde(rename_all = "camelCase")]
    Text { body: String, formatted: Option<String> },
    #[serde(rename_all = "camelCase")]
    Emote { body: String, formatted: Option<String> },
    #[serde(rename_all = "camelCase")]
    Notice { body: String, formatted: Option<String> },
    #[serde(rename_all = "camelCase")]
    Image { body: String, media: MediaInfoDto },
    #[serde(rename_all = "camelCase")]
    Video { body: String, media: MediaInfoDto },
    #[serde(rename_all = "camelCase")]
    Audio { body: String, media: MediaInfoDto },
    #[serde(rename_all = "camelCase")]
    File { body: String, media: MediaInfoDto },
    #[serde(rename_all = "camelCase")]
    Location { body: String, geo_uri: String },
    #[serde(rename_all = "camelCase")]
    Sticker { body: String, media: MediaInfoDto },
    #[serde(rename_all = "camelCase")]
    Poll { question: String, answers: Vec<String>, ended: bool },
    Redacted,
    #[serde(rename_all = "camelCase")]
    UnableToDecrypt { reason: String },
    #[serde(rename_all = "camelCase")]
    Membership { change: String, user_id: String, display_name: Option<String> },
    #[serde(rename_all = "camelCase")]
    ProfileChange { summary: String },
    #[serde(rename_all = "camelCase")]
    State { event_type: String, state_key: String, summary: String },
    CallInvite,
    #[serde(rename_all = "camelCase")]
    RtcNotification { intent: Option<String> },
    #[serde(rename_all = "camelCase")]
    Unsupported { event_type: String },
}

// ---------------------------------------------------------------------------
// diffs
// ---------------------------------------------------------------------------

/// A batch of room-list changes, numbered so the frontend can tell whether a
/// snapshot it holds already includes them.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RoomsUpdate {
    pub seq: u64,
    pub diffs: Vec<Diff<RoomSummary>>,
}

/// The room list as it stands, and the last batch folded into it.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RoomsSnapshot {
    pub seq: u64,
    pub rooms: Vec<RoomSummary>,
}

/// A `VectorDiff` rendered for JS. Mirrors `eyeball_im::VectorDiff` so the
/// frontend can apply updates in place instead of re-rendering the world.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", tag = "op")]
pub enum Diff<T> {
    #[serde(rename_all = "camelCase")]
    Append { values: Vec<T> },
    Clear,
    #[serde(rename_all = "camelCase")]
    PushFront { value: T },
    #[serde(rename_all = "camelCase")]
    PushBack { value: T },
    PopFront,
    PopBack,
    #[serde(rename_all = "camelCase")]
    Insert { index: usize, value: T },
    #[serde(rename_all = "camelCase")]
    Set { index: usize, value: T },
    #[serde(rename_all = "camelCase")]
    Remove { index: usize },
    #[serde(rename_all = "camelCase")]
    Truncate { length: usize },
    #[serde(rename_all = "camelCase")]
    Reset { values: Vec<T> },
}

// ---------------------------------------------------------------------------
// events pushed to the frontend
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TimelineUpdate {
    /// Room ID, or `<roomId>|<threadRoot>` for a thread — `timeline::timeline_key`.
    pub room_id: String,
    pub diffs: Vec<Diff<TimelineItemDto>>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TypingUpdate {
    pub room_id: String,
    pub users: Vec<TypingUser>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TypingUser {
    pub user_id: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub state: SyncState,
    pub message: Option<String>,
}

/// `matrix_sdk_ui::sync_service::State`, without the payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum SyncState {
    Idle,
    Running,
    Terminated,
    Error,
    Offline,
}

// ---------------------------------------------------------------------------
// composer input
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SendOptions {
    pub body: String,
    /// Rendered as Markdown when true; the design's composer sends plain text
    /// unless the user opts in.
    #[serde(default)]
    #[ts(as = "Option<_>", optional)]
    pub markdown: bool,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub reply_to: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub thread_root: Option<String>,
    /// `m.text` (default), `m.emote`, `m.notice`
    #[serde(default)]
    #[ts(optional = nullable)]
    pub msgtype: Option<String>,
    /// Custom emotes the body might mention, as `:shortcode:`.
    ///
    /// The composer sends the whole set it knows about rather than deciding
    /// what's in the text; matching is done once, against the rendered HTML, so
    /// Markdown and emotes can both survive in one message.
    #[serde(default)]
    #[ts(as = "Option<_>", optional)]
    pub emotes: Vec<EmoteRef>,
}

/// One custom emote the composer can substitute into an outgoing message.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct EmoteRef {
    /// Without the surrounding colons.
    pub shortcode: String,
    pub url: String,
}

/// One image from a pack, on its way out as an `m.sticker`.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct StickerOptions {
    /// Alt text — the shortcode, unless the pack gave the image a body.
    pub body: String,
    pub url: String,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub width: Option<u32>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub height: Option<u32>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub size: Option<u32>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub mimetype: Option<String>,
}

// ---------------------------------------------------------------------------
// conversions
// ---------------------------------------------------------------------------

fn ts(t: MilliSecondsSinceUnixEpoch) -> u64 {
    t.0.into()
}

/// Both plain and encrypted attachments carry an `mxc://` URI; the difference
/// only matters when we go to fetch the bytes, which the media layer handles.
pub fn media_source_mxc(source: &MediaSource) -> String {
    // Everything the UI can ask for later goes through here, so this is the one
    // place that reliably sees the keys for encrypted attachments.
    crate::matrix::media::remember_source(source);

    match source {
        MediaSource::Plain(uri) => uri.to_string(),
        MediaSource::Encrypted(file) => file.url.to_string(),
    }
}

/// Stickers carry their own source enum, whose encrypted variant is behind a
/// compat feature we don't enable — so only the plain form is reachable here.
fn sticker_mxc(source: &matrix_sdk::ruma::events::sticker::StickerMediaSource) -> Option<String> {
    match source {
        matrix_sdk::ruma::events::sticker::StickerMediaSource::Plain(uri) => {
            Some(uri.to_string())
        }
        _ => None,
    }
}

fn profile_name(details: &TimelineDetails<matrix_sdk_ui::timeline::Profile>) -> Option<String> {
    match details {
        TimelineDetails::Ready(p) => p.display_name.clone(),
        _ => None,
    }
}

fn profile_avatar(details: &TimelineDetails<matrix_sdk_ui::timeline::Profile>) -> Option<String> {
    match details {
        TimelineDetails::Ready(p) => p.avatar_url.as_ref().map(|u| u.to_string()),
        _ => None,
    }
}

fn convert_message(msg: &matrix_sdk_ui::timeline::Message) -> ContentDto {
    let body = msg.body().to_owned();
    match msg.msgtype() {
        MessageType::Text(c) => ContentDto::Text {
            body: c.body.clone(),
            formatted: c.formatted.as_ref().map(|f| f.body.clone()),
        },
        MessageType::Emote(c) => ContentDto::Emote {
            body: c.body.clone(),
            formatted: c.formatted.as_ref().map(|f| f.body.clone()),
        },
        MessageType::Notice(c) => ContentDto::Notice {
            body: c.body.clone(),
            formatted: c.formatted.as_ref().map(|f| f.body.clone()),
        },
        MessageType::Image(c) => {
            let info = c.info.as_deref();
            ContentDto::Image {
                body: c.body.clone(),
                media: MediaInfoDto {
                    mxc: Some(media_source_mxc(&c.source)),
                    mimetype: info.and_then(|i| i.mimetype.clone()),
                    size: info.and_then(|i| i.size).map(Into::into),
                    width: info.and_then(|i| i.width).map(Into::into),
                    height: info.and_then(|i| i.height).map(Into::into),
                    duration_ms: None,
                    blurhash: info.and_then(|i| i.blurhash.clone()),
                    thumbnail_mxc: info
                        .and_then(|i| i.thumbnail_source.as_ref())
                        .map(media_source_mxc),
                    waveform: None,
                    is_voice: false,
                },
            }
        }
        MessageType::Video(c) => {
            let info = c.info.as_deref();
            ContentDto::Video {
                body: c.body.clone(),
                media: MediaInfoDto {
                    mxc: Some(media_source_mxc(&c.source)),
                    mimetype: info.and_then(|i| i.mimetype.clone()),
                    size: info.and_then(|i| i.size).map(Into::into),
                    width: info.and_then(|i| i.width).map(Into::into),
                    height: info.and_then(|i| i.height).map(Into::into),
                    duration_ms: info
                        .and_then(|i| i.duration)
                        .map(|d| d.as_millis().min(u64::MAX as u128) as u64),
                    blurhash: info.and_then(|i| i.blurhash.clone()),
                    thumbnail_mxc: info
                        .and_then(|i| i.thumbnail_source.as_ref())
                        .map(media_source_mxc),
                    waveform: None,
                    is_voice: false,
                },
            }
        }
        MessageType::Audio(c) => {
            let info = c.info.as_deref();
            ContentDto::Audio {
                body: c.body.clone(),
                media: MediaInfoDto {
                    mxc: Some(media_source_mxc(&c.source)),
                    mimetype: info.and_then(|i| i.mimetype.clone()),
                    size: info.and_then(|i| i.size).map(Into::into),
                    width: None,
                    height: None,
                    duration_ms: info
                        .and_then(|i| i.duration)
                        .map(|d| d.as_millis().min(u64::MAX as u128) as u64),
                    blurhash: None,
                    thumbnail_mxc: None,
                    // `Amplitude`'s accessor sits behind an unstable ruma
                    // feature, so round-trip through JSON — the wire format is
                    // a plain array of numbers either way.
                    waveform: c
                        .audio
                        .as_ref()
                        .and_then(|a| serde_json::to_value(&a.waveform).ok())
                        .and_then(|v| serde_json::from_value::<Vec<u16>>(v).ok())
                        .filter(|w| !w.is_empty()),
                    is_voice: c.voice.is_some(),
                },
            }
        }
        MessageType::File(c) => {
            let info = c.info.as_deref();
            ContentDto::File {
                body: c.body.clone(),
                media: MediaInfoDto {
                    mxc: Some(media_source_mxc(&c.source)),
                    mimetype: info.and_then(|i| i.mimetype.clone()),
                    size: info.and_then(|i| i.size).map(Into::into),
                    width: None,
                    height: None,
                    duration_ms: None,
                    blurhash: None,
                    thumbnail_mxc: info
                        .and_then(|i| i.thumbnail_source.as_ref())
                        .map(media_source_mxc),
                    waveform: None,
                    is_voice: false,
                },
            }
        }
        MessageType::Location(c) => {
            ContentDto::Location { body: c.body.clone(), geo_uri: c.geo_uri.clone() }
        }
        other => ContentDto::Unsupported { event_type: other.msgtype().to_owned() },
    }
    .pipe_empty_body(body)
}

/// Small helper so a message whose body we couldn't map still shows *something*
/// rather than an empty bubble.
trait PipeEmptyBody {
    fn pipe_empty_body(self, fallback: String) -> ContentDto;
}

impl PipeEmptyBody for ContentDto {
    fn pipe_empty_body(self, fallback: String) -> ContentDto {
        match self {
            ContentDto::Unsupported { event_type } if !fallback.is_empty() => {
                ContentDto::Text { body: format!("{fallback} ({event_type})"), formatted: None }
            }
            other => other,
        }
    }
}

fn convert_content(content: &TimelineItemContent) -> (ContentDto, ContentExtras<'_>) {
    let mut extras = ContentExtras::default();

    let dto = match content {
        TimelineItemContent::MsgLike(msg_like) => {
            extras.reactions = Some(&msg_like.reactions);
            extras.in_reply_to = msg_like.in_reply_to.as_ref();
            extras.thread_root = msg_like.thread_root.as_ref().map(|e| e.to_string());
            extras.thread_summary = msg_like.thread_summary.as_ref();

            match &msg_like.kind {
                MsgLikeKind::Message(m) => {
                    extras.is_edited = m.is_edited();
                    convert_message(m)
                }
                MsgLikeKind::Sticker(s) => {
                    let c = s.content();
                    ContentDto::Sticker {
                        body: c.body.clone(),
                        media: MediaInfoDto {
                            mxc: sticker_mxc(&c.source),
                            mimetype: c.info.mimetype.clone(),
                            size: c.info.size.map(Into::into),
                            width: c.info.width.map(Into::into),
                            height: c.info.height.map(Into::into),
                            duration_ms: None,
                            blurhash: c.info.blurhash.clone(),
                            thumbnail_mxc: None,
                            waveform: None,
                            is_voice: false,
                        },
                    }
                }
                MsgLikeKind::Poll(p) => {
                    let results = p.results();
                    ContentDto::Poll {
                        question: results.question.clone(),
                        answers: results.answers.iter().map(|a| a.text.clone()).collect(),
                        ended: results.end_time.is_some(),
                    }
                }
                MsgLikeKind::Redacted => ContentDto::Redacted,
                MsgLikeKind::UnableToDecrypt(e) => {
                    ContentDto::UnableToDecrypt { reason: format!("{e:?}") }
                }
                MsgLikeKind::Other(other) => {
                    ContentDto::Unsupported { event_type: other.event_type().to_string() }
                }
                MsgLikeKind::LiveLocation(_) => {
                    ContentDto::Unsupported { event_type: "m.beacon".to_owned() }
                }
            }
        }
        TimelineItemContent::MembershipChange(change) => ContentDto::Membership {
            change: change
                .change()
                .map(|c| format!("{c:?}"))
                .unwrap_or_else(|| "none".to_owned())
                .to_lowercase(),
            user_id: change.user_id().to_string(),
            display_name: change.display_name(),
        },
        TimelineItemContent::ProfileChange(change) => {
            ContentDto::ProfileChange { summary: describe_profile_change(change) }
        }
        TimelineItemContent::OtherState(state) => ContentDto::State {
            event_type: state.content().event_type().to_string(),
            state_key: state.state_key().to_owned(),
            summary: state.content().event_type().to_string(),
        },
        TimelineItemContent::FailedToParseMessageLike { event_type, .. } => {
            ContentDto::Unsupported { event_type: event_type.to_string() }
        }
        TimelineItemContent::FailedToParseState { event_type, .. } => {
            ContentDto::Unsupported { event_type: event_type.to_string() }
        }
        TimelineItemContent::CallInvite => ContentDto::CallInvite,
        TimelineItemContent::RtcNotification { call_intent, .. } => {
            ContentDto::RtcNotification { intent: call_intent.as_ref().map(|i| format!("{i:?}")) }
        }
    };

    (dto, extras)
}

fn describe_profile_change(change: &matrix_sdk_ui::timeline::MemberProfileChange) -> String {
    let who = change.user_id().as_str();
    let mut parts = Vec::new();
    if let Some(name) = change.displayname_change() {
        match (&name.old, &name.new) {
            (Some(old), Some(new)) => parts.push(format!("{old} is now {new}")),
            (None, Some(new)) => parts.push(format!("{who} is now {new}")),
            (Some(old), None) => parts.push(format!("{old} removed their display name")),
            (None, None) => {}
        }
    }
    if change.avatar_url_change().is_some() {
        parts.push(format!("{who} changed their avatar"));
    }
    if parts.is_empty() { format!("{who} updated their profile") } else { parts.join(" · ") }
}

#[derive(Default)]
struct ContentExtras<'a> {
    reactions: Option<&'a matrix_sdk_ui::timeline::ReactionsByKeyBySender>,
    in_reply_to: Option<&'a matrix_sdk_ui::timeline::InReplyToDetails>,
    thread_root: Option<String>,
    thread_summary: Option<&'a matrix_sdk_ui::timeline::ThreadSummary>,
    is_edited: bool,
}

fn convert_reactions(
    reactions: Option<&matrix_sdk_ui::timeline::ReactionsByKeyBySender>,
    own_user: &OwnedUserId,
) -> Vec<ReactionDto> {
    let Some(reactions) = reactions else { return Vec::new() };
    reactions
        .iter()
        .map(|(key, by_sender)| {
            let own = by_sender.iter().find(|(sender, _)| *sender == own_user);
            ReactionDto {
                key: key.clone(),
                count: by_sender.len(),
                mine: own.is_some(),
                // Anything that isn't fully remote is still in flight locally,
                // which the UI shows as a dimmed pill.
                pending: own.is_some_and(|(_, info)| {
                    !matches!(info.status, ReactionStatus::RemoteToRemote(_))
                }),
                senders: by_sender.keys().map(|u| u.to_string()).collect(),
            }
        })
        .collect()
}

fn embedded_body(event: &matrix_sdk_ui::timeline::EmbeddedEvent) -> Option<String> {
    match &event.content {
        TimelineItemContent::MsgLike(m) => match &m.kind {
            MsgLikeKind::Message(msg) => Some(msg.body().to_owned()),
            MsgLikeKind::Sticker(_) => Some("sticker".to_owned()),
            MsgLikeKind::Redacted => Some("message deleted".to_owned()),
            MsgLikeKind::UnableToDecrypt(_) => Some("encrypted message".to_owned()),
            _ => None,
        },
        _ => None,
    }
}

fn convert_reply(details: Option<&matrix_sdk_ui::timeline::InReplyToDetails>) -> Option<ReplyDto> {
    let details = details?;
    let (sender, sender_name, body) = match &details.event {
        TimelineDetails::Ready(event) => (
            Some(event.sender.to_string()),
            profile_name(&event.sender_profile),
            embedded_body(event),
        ),
        _ => (None, None, None),
    };
    Some(ReplyDto { event_id: details.event_id.to_string(), sender, sender_name, body })
}

fn convert_thread_summary(
    summary: Option<&matrix_sdk_ui::timeline::ThreadSummary>,
) -> Option<ThreadSummaryDto> {
    let summary = summary?;
    let (latest_sender, latest_body) = match &summary.latest_event {
        TimelineDetails::Ready(event) => {
            (Some(event.sender.to_string()), embedded_body(event.as_ref()))
        }
        _ => (None, None),
    };
    Some(ThreadSummaryDto { num_replies: summary.num_replies, latest_sender, latest_body })
}

fn convert_send_state(state: Option<&EventSendState>) -> Option<SendStateDto> {
    match state? {
        EventSendState::NotSentYet { progress } => Some(SendStateDto::NotSentYet {
            progress: progress.as_ref().and_then(|p| {
                (p.progress.total > 0)
                    .then(|| (p.progress.current as f64 / p.progress.total as f64) as f32)
            }),
        }),
        EventSendState::SendingFailed { error, is_recoverable } => Some(SendStateDto::Failed {
            error: error.to_string(),
            is_recoverable: *is_recoverable,
        }),
        EventSendState::Sent { event_id } => {
            Some(SendStateDto::Sent { event_id: event_id.to_string() })
        }
    }
}

/// The trust warning to show beside a message, if any.
///
/// Red means the message is actively suspect (unsigned device, unverified
/// identity); grey means we simply can't vouch for it. The SDK gives us a
/// machine-readable code, and we own the wording.
fn convert_shield(item: &EventTimelineItem) -> Option<ShieldDto> {
    use matrix_sdk_ui::timeline::{TimelineEventShieldState as Shield, TimelineEventShieldStateCode as Code};

    let describe = |code: Code| -> String {
        match code {
            Code::AuthenticityNotGuaranteed => {
                "we can't confirm where this message came from".to_owned()
            }
            Code::UnknownDevice => "sent from an unknown device".to_owned(),
            Code::UnsignedDevice => "sent from a device that isn't cross-signed".to_owned(),
            Code::UnverifiedIdentity => "sent by someone you haven't verified".to_owned(),
            Code::SentInClear => "sent unencrypted in an encrypted room".to_owned(),
            Code::VerificationViolation => {
                "sent by someone whose identity changed since you verified them".to_owned()
            }
            Code::MismatchedSender => "the sender doesn't match the encryption key".to_owned(),
        }
    };

    match item.get_shield(false) {
        Shield::Red { code } => Some(ShieldDto { colour: ShieldColour::Red, message: describe(code) }),
        Shield::Grey { code } => Some(ShieldDto { colour: ShieldColour::Grey, message: describe(code) }),
        Shield::None => None,
    }
}

pub fn convert_event_item(item: &EventTimelineItem, own_user: &OwnedUserId) -> EventItemDto {
    let (content, extras) = convert_content(item.content());

    EventItemDto {
        event_id: item.event_id().map(|e| e.to_string()),
        transaction_id: item.transaction_id().map(|t| t.to_string()),
        sender: item.sender().to_string(),
        sender_name: profile_name(item.sender_profile()),
        sender_avatar: profile_avatar(item.sender_profile()),
        timestamp: ts(item.timestamp()),
        is_own: item.is_own(),
        is_editable: item.is_editable(),
        can_reply: item.can_be_replied_to(),
        is_highlighted: item.is_highlighted(),
        is_edited: extras.is_edited,
        is_emoji_only: item.contains_only_emojis(),
        send_state: convert_send_state(item.send_state()),
        shield: convert_shield(item),
        content,
        reactions: convert_reactions(extras.reactions, own_user),
        reply: convert_reply(extras.in_reply_to),
        thread_root: extras.thread_root,
        thread_summary: convert_thread_summary(extras.thread_summary),
        read_receipts: item.read_receipts().keys().map(|u| u.to_string()).collect(),
    }
}

pub fn convert_timeline_item(item: &TimelineItem, own_user: &OwnedUserId) -> TimelineItemDto {
    let id = item.unique_id().0.clone();
    match item.kind() {
        TimelineItemKind::Event(event) => TimelineItemDto {
            id,
            kind: TimelineRowKind::Event,
            timestamp: Some(ts(event.timestamp())),
            event: Some(Box::new(convert_event_item(event, own_user))),
        },
        TimelineItemKind::Virtual(virt) => match virt {
            VirtualTimelineItem::DateDivider(t) => {
                TimelineItemDto { id, kind: TimelineRowKind::DateDivider, timestamp: Some(ts(*t)), event: None }
            }
            VirtualTimelineItem::ReadMarker => {
                TimelineItemDto { id, kind: TimelineRowKind::ReadMarker, timestamp: None, event: None }
            }
            VirtualTimelineItem::TimelineStart => {
                TimelineItemDto { id, kind: TimelineRowKind::TimelineStart, timestamp: None, event: None }
            }
        },
    }
}
