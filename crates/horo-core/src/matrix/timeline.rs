//! Open timelines, stream their diffs, and act on individual events.
//!
//! A timeline is opened when the user selects a room and closed when they leave
//! it. Threads get their own timeline keyed by `<room_id>|<thread_root>`, so a
//! thread panel and the main timeline can be open side by side.

use std::sync::Arc;

use futures_util::{StreamExt, pin_mut};
use matrix_sdk::{
    room::edit::EditedContent,
    ruma::{
        EventId, OwnedEventId, RoomId,
        events::{
            room::message::{
                MessageType, RoomMessageEventContent, RoomMessageEventContentWithoutRelation,
                TextMessageEventContent,
            },
        },
    },
};
use matrix_sdk_ui::timeline::{
    RoomExt, TimelineEventItemId, TimelineFocus, TimelineItem, TimelineReadReceiptTracking,
};

use crate::{
    dto::{
        Diff, EmoteRef, SendOptions, StickerOptions, TimelineItemDto, TimelineUpdate,
        convert_timeline_item,
    },
    error::{Error, Result},
    events::{Event, Host, emit, spawn_typing_task},
    matrix::{MatrixCore, core::OpenTimeline},
};

/// How many events a single "load older messages" step fetches.
pub const PAGE_SIZE: u16 = 40;

/// Key under which a timeline is stored. Threads are namespaced under their room.
pub fn timeline_key(room_id: &RoomId, thread_root: Option<&str>) -> String {
    match thread_root {
        Some(root) => format!("{room_id}|{root}"),
        None => room_id.to_string(),
    }
}

/// Open (or return the already-open) timeline for a room or thread.
///
/// Returns the current items so the UI can paint immediately; subsequent
/// changes arrive as `matrix://timeline` diffs.
pub async fn open(
    host: &Host,
    core: &Arc<MatrixCore>,
    room_id: &RoomId,
    thread_root: Option<String>,
) -> Result<Vec<TimelineItemDto>> {
    let key = timeline_key(room_id, thread_root.as_deref());
    let own_user = core.own_user_id()?;

    // Re-opening a timeline we already have is a no-op apart from re-reading the
    // current items: the stream task is still running and still emitting.
    if let Some(open) = core.timelines.lock().await.get(&key) {
        let items = open.timeline.items().await;
        return Ok(items.iter().map(|i| convert_timeline_item(i, &own_user)).collect());
    }

    // Sliding sync only carries ONE event per room in the list — that's the
    // sidebar preview, not a timeline. Subscribing tells the server this is the
    // room we're actually looking at, which raises the limit and pulls the
    // required state. Without this a freshly-restored session opens every room
    // showing a single message.
    core.room_list.subscribe_to_rooms(&[room_id]).await;

    let room = core.room(&room_id.to_owned())?;

    let focus = match &thread_root {
        Some(root) => TimelineFocus::Thread { root_event_id: EventId::parse(root)? },
        // Threaded replies are hidden from the live timeline because we surface
        // them through the thread summary chip instead, as the design does.
        None => TimelineFocus::Live { hide_threaded_events: true },
    };

    let timeline = room
        .timeline_builder()
        .with_focus(focus)
        .track_read_marker_and_receipts(TimelineReadReceiptTracking::MessageLikeEvents)
        .build()
        .await?;
    let timeline = Arc::new(timeline);

    // Member profiles power avatars and display names on every message; without
    // this the first paint shows raw user IDs.
    timeline.fetch_members().await;

    let (initial, stream) = timeline.subscribe().await;

    // Claim the slot *before* spawning the task that emits.
    //
    // Two opens for the same key can be in flight at once: nothing waits for
    // one to finish, so flicking A → B → A starts a second while the first is
    // still building. The check at the top of this function was true when it
    // ran and is several awaits out of date by now, so both callers reach here
    // believing the timeline is theirs to create.
    //
    // The old order was spawn-then-insert. Both spawned, both streamed the same
    // room under the same key, and the UI applied every message twice — until
    // the second `insert` dropped the first `OpenTimeline` and aborted it. That
    // is why the duplicates were occasional, varied in number, and vanished on
    // restart: they were whatever landed inside the overlap, left behind in a
    // list nothing ever deduplicated.
    //
    // Spawning only after winning the slot means the loser never creates an
    // emitter at all. Subscribing first costs nothing — the stream buffers, so
    // no change between `subscribe` and `spawn` is missed.
    let lost_to = {
        let mut open = core.timelines.lock().await;
        match open.get(&key) {
            Some(existing) => Some(existing.timeline.clone()),
            None => {
                let task = {
                    let host = host.clone();
                    let core = core.clone();
                    let key = key.clone();
                    let own_user = own_user.clone();

                    tokio::spawn(async move {
                        pin_mut!(stream);
                        while let Some(diffs) = stream.next().await {
                            if core.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                                break;
                            }
                            let converted: Vec<_> =
                                diffs.into_iter().map(|d| convert_diff(d, &own_user)).collect();

                            emit(
                                &host,
                                Event::Timeline(TimelineUpdate { room_id: key.clone(), diffs: converted }),
                            );
                        }
                    })
                };

                open.insert(key.clone(), OpenTimeline { timeline, task });
                None
            }
        }
    };

    // Lost the race. The timeline built above is dropped with nothing watching
    // it; the winner's is the one emitting, so report *its* items — ours could
    // already disagree with what the UI is about to be sent diffs against.
    if let Some(existing) = lost_to {
        let items = existing.items().await;
        return Ok(items.iter().map(|i| convert_timeline_item(i, &own_user)).collect());
    }

    let items: Vec<_> = initial.iter().map(|i| convert_timeline_item(i, &own_user)).collect();

    // Typing notices only matter for the room the user is actually looking at.
    if thread_root.is_none()
        && let Some(handle) = spawn_typing_task(host.clone(), core.clone(), room_id.to_owned())
    {
        core.tasks.lock().await.push(handle);
    }

    Ok(items)
}

pub async fn close(core: &MatrixCore, room_id: &RoomId, thread_root: Option<&str>) -> Result<()> {
    let key = timeline_key(room_id, thread_root);
    // Dropping `OpenTimeline` aborts its stream task.
    core.timelines.lock().await.remove(&key);
    Ok(())
}

/// Close a room's timeline and every thread timeline inside it.
///
/// Thread timelines are keyed `<room>|<root>`, so the room's own key is the
/// prefix of all of them. Used when the room itself is going away and there's
/// nothing left for those streams to watch.
pub async fn close_all(core: &MatrixCore, room_id: &RoomId) {
    let prefix = format!("{room_id}|");
    let own = room_id.as_str();
    core.timelines
        .lock()
        .await
        .retain(|key, _| key != own && !key.starts_with(&prefix));
}


async fn get(
    core: &MatrixCore,
    room_id: &RoomId,
    thread_root: Option<&str>,
) -> Result<Arc<matrix_sdk_ui::Timeline>> {
    let key = timeline_key(room_id, thread_root);
    core.timelines
        .lock()
        .await
        .get(&key)
        .map(|open| open.timeline.clone())
        .ok_or_else(|| Error::NoTimeline(key))
}

fn convert_diff(
    diff: matrix_sdk_ui::eyeball_im::VectorDiff<Arc<TimelineItem>>,
    own_user: &matrix_sdk::ruma::OwnedUserId,
) -> Diff<TimelineItemDto> {
    use matrix_sdk_ui::eyeball_im::VectorDiff;

    let convert = |item: Arc<TimelineItem>| convert_timeline_item(&item, own_user);

    match diff {
        VectorDiff::Append { values } => {
            Diff::Append { values: values.into_iter().map(convert).collect() }
        }
        VectorDiff::Clear => Diff::Clear,
        VectorDiff::PushFront { value } => Diff::PushFront { value: convert(value) },
        VectorDiff::PushBack { value } => Diff::PushBack { value: convert(value) },
        VectorDiff::PopFront => Diff::PopFront,
        VectorDiff::PopBack => Diff::PopBack,
        VectorDiff::Insert { index, value } => Diff::Insert { index, value: convert(value) },
        VectorDiff::Set { index, value } => Diff::Set { index, value: convert(value) },
        VectorDiff::Remove { index } => Diff::Remove { index },
        VectorDiff::Truncate { length } => Diff::Truncate { length },
        VectorDiff::Reset { values } => {
            Diff::Reset { values: values.into_iter().map(convert).collect() }
        }
    }
}

// ---------------------------------------------------------------------------
// actions
// ---------------------------------------------------------------------------

fn build_message(options: &SendOptions) -> RoomMessageEventContent {
    let content = build_plain_message(options);
    with_emotes(content, &options.emotes)
}

fn build_plain_message(options: &SendOptions) -> RoomMessageEventContent {
    let body = &options.body;

    match options.msgtype.as_deref() {
        Some("m.emote") => {
            if options.markdown {
                RoomMessageEventContent::emote_markdown(body)
            } else {
                RoomMessageEventContent::emote_plain(body)
            }
        }
        Some("m.notice") => {
            if options.markdown {
                RoomMessageEventContent::notice_markdown(body)
            } else {
                RoomMessageEventContent::notice_plain(body)
            }
        }
        _ => {
            if options.markdown {
                RoomMessageEventContent::text_markdown(body)
            } else {
                RoomMessageEventContent::text_plain(body)
            }
        }
    }
}

/// Swap `:shortcode:` for `<img data-mx-emoticon>` in the formatted body.
///
/// This runs *after* the message is built, on the HTML rather than on what the
/// user typed, for two reasons: Markdown has already escaped everything, so an
/// emote can't be spelled out of a `<` someone typed; and a message keeps both
/// its formatting and its emotes instead of having to choose.
///
/// A message with no emotes in it comes back untouched — no formatted body is
/// invented for plain text that didn't need one.
fn with_emotes(
    mut content: RoomMessageEventContent,
    emotes: &[EmoteRef],
) -> RoomMessageEventContent {
    use matrix_sdk::ruma::events::room::message::FormattedBody;

    if emotes.is_empty() {
        return content;
    }

    // Only the text-ish message types have a formatted body to put emotes in.
    let (body, formatted) = match &content.msgtype {
        MessageType::Text(m) => (m.body.clone(), m.formatted.clone()),
        MessageType::Emote(m) => (m.body.clone(), m.formatted.clone()),
        MessageType::Notice(m) => (m.body.clone(), m.formatted.clone()),
        _ => return content,
    };

    // Without a formatted body there's nothing but the plain text, which has to
    // be escaped before it can carry markup.
    let source = match &formatted {
        Some(f) => f.body.clone(),
        None => escape_html(&body),
    };

    let Some(html) = substitute_emotes(&source, emotes) else {
        return content;
    };

    let formatted = FormattedBody::html(html);
    match &mut content.msgtype {
        MessageType::Text(m) => m.formatted = Some(formatted),
        MessageType::Emote(m) => m.formatted = Some(formatted),
        MessageType::Notice(m) => m.formatted = Some(formatted),
        _ => {}
    }

    content
}

/// Replace every `:shortcode:` outside a tag, or `None` if there were none.
///
/// Scanning past anything between `<` and `>` keeps a shortcode that happens to
/// appear inside an attribute — an autolinked URL, most plausibly — from being
/// rewritten into markup in the middle of a tag.
fn substitute_emotes(html: &str, emotes: &[EmoteRef]) -> Option<String> {
    let patterns: Vec<(String, &EmoteRef)> = emotes
        .iter()
        .filter(|emote| !emote.shortcode.is_empty() && emote.url.starts_with("mxc://"))
        .map(|emote| (format!(":{}:", emote.shortcode), emote))
        .collect();

    let mut out = String::with_capacity(html.len());
    let mut replaced = false;
    let mut in_tag = false;
    let mut at = 0;

    while at < html.len() {
        let c = html[at..].chars().next().expect("at is a char boundary");

        if in_tag {
            in_tag = c != '>';
            out.push(c);
            at += c.len_utf8();
            continue;
        }

        if c == '<' {
            in_tag = true;
            out.push(c);
            at += c.len_utf8();
            continue;
        }

        let matched = (c == ':')
            .then(|| patterns.iter().find(|(pattern, _)| html[at..].starts_with(pattern)))
            .flatten();

        match matched {
            Some((pattern, emote)) => {
                let code = &emote.shortcode;
                out.push_str(&format!(
                    "<img data-mx-emoticon height=\"32\" src=\"{}\" alt=\":{code}:\" title=\":{code}:\" />",
                    escape_html(&emote.url),
                ));
                at += pattern.len();
                replaced = true;
            }
            None => {
                out.push(c);
                at += c.len_utf8();
            }
        }
    }

    replaced.then_some(out)
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub async fn send(
    core: &MatrixCore,
    room_id: &RoomId,
    options: SendOptions,
) -> Result<()> {
    let timeline = get(core, room_id, options.thread_root.as_deref()).await?;
    let content = build_message(&options);

    match &options.reply_to {
        Some(event_id) => {
            let event_id = EventId::parse(event_id)?;
            timeline
                .send_reply(
                    RoomMessageEventContentWithoutRelation::new(content.msgtype),
                    event_id,
                )
                .await?;
        }
        None => {
            timeline.send(content.into()).await?;
        }
    }

    Ok(())
}

/// Send one image from a pack as an `m.sticker`.
///
/// Stickers are their own event type rather than an `m.image` message, which is
/// what makes other clients draw them small and without a filename. The URL is
/// the pack's — already on the server — so nothing is uploaded here.
pub async fn send_sticker(
    core: &MatrixCore,
    room_id: &RoomId,
    thread_root: Option<&str>,
    sticker: StickerOptions,
) -> Result<()> {
    use matrix_sdk::ruma::{
        MxcUri,
        events::{AnyMessageLikeEventContent, room::ImageInfo, sticker::StickerEventContent},
    };

    if !sticker.url.starts_with("mxc://") {
        return Err(Error::Other("that sticker isn't on this server".into()));
    }

    let timeline = get(core, room_id, thread_root).await?;

    let mut info = ImageInfo::new();
    info.width = sticker.width.map(Into::into);
    info.height = sticker.height.map(Into::into);
    info.size = sticker.size.map(Into::into);
    info.mimetype = sticker.mimetype;

    let content = StickerEventContent::new(
        sticker.body,
        info,
        <&MxcUri>::from(sticker.url.as_str()).to_owned(),
    );

    timeline.send(AnyMessageLikeEventContent::Sticker(content)).await?;
    Ok(())
}

pub async fn paginate_back(
    core: &MatrixCore,
    room_id: &RoomId,
    thread_root: Option<&str>,
) -> Result<bool> {
    let timeline = get(core, room_id, thread_root).await?;
    // `true` means we've hit the start of the timeline.
    let reached_start = timeline.paginate_backwards(PAGE_SIZE).await?;
    Ok(reached_start)
}

pub async fn toggle_reaction(
    core: &MatrixCore,
    room_id: &RoomId,
    thread_root: Option<&str>,
    event_id: &str,
    key: &str,
) -> Result<bool> {
    let timeline = get(core, room_id, thread_root).await?;
    let item_id = TimelineEventItemId::EventId(EventId::parse(event_id)?);
    Ok(timeline.toggle_reaction(&item_id, key).await?)
}

pub async fn edit(
    core: &MatrixCore,
    room_id: &RoomId,
    thread_root: Option<&str>,
    event_id: &str,
    new_body: &str,
    markdown: bool,
) -> Result<()> {
    let timeline = get(core, room_id, thread_root).await?;
    let item_id = TimelineEventItemId::EventId(EventId::parse(event_id)?);

    let content = if markdown {
        RoomMessageEventContent::text_markdown(new_body)
    } else {
        RoomMessageEventContent::text_plain(new_body)
    };

    timeline
        .edit(
            &item_id,
            EditedContent::RoomMessage(RoomMessageEventContentWithoutRelation::new(
                content.msgtype,
            )),
        )
        .await?;
    Ok(())
}

pub async fn redact(
    core: &MatrixCore,
    room_id: &RoomId,
    thread_root: Option<&str>,
    event_id: &str,
    reason: Option<&str>,
) -> Result<()> {
    let timeline = get(core, room_id, thread_root).await?;
    let item_id = TimelineEventItemId::EventId(EventId::parse(event_id)?);
    timeline.redact(&item_id, reason).await?;
    Ok(())
}

/// Mark everything up to the newest event as read.
pub async fn mark_as_read(core: &MatrixCore, room_id: &RoomId) -> Result<()> {
    use matrix_sdk::ruma::api::client::receipt::create_receipt::v3::ReceiptType;

    let timeline = get(core, room_id, None).await?;
    timeline.mark_as_read(ReceiptType::Read).await?;
    Ok(())
}

/// Send a read receipt for one specific event.
pub async fn send_read_receipt(
    core: &MatrixCore,
    room_id: &RoomId,
    event_id: &str,
) -> Result<()> {
    use matrix_sdk::ruma::api::client::receipt::create_receipt::v3::ReceiptType;

    let timeline = get(core, room_id, None).await?;
    let event_id: OwnedEventId = EventId::parse(event_id)?;
    timeline.send_single_receipt(ReceiptType::Read, event_id).await?;
    Ok(())
}

/// Upload and send a file. `body` becomes the filename shown in the timeline.
pub async fn send_attachment(
    core: &MatrixCore,
    room_id: &RoomId,
    thread_root: Option<&str>,
    path: &str,
    caption: Option<String>,
) -> Result<()> {
    use matrix_sdk_ui::timeline::AttachmentConfig;

    let timeline = get(core, room_id, thread_root).await?;
    let path = std::path::PathBuf::from(path);

    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    let config = AttachmentConfig {
        caption: caption.map(TextMessageEventContent::plain),
        ..Default::default()
    };

    timeline
        .send_attachment(path, mime, config)
        .use_send_queue()
        .await
        .map_err(|e| Error::Other(format!("upload failed: {e}")))?;
    Ok(())
}

/// Send bytes we already hold as a file — a pasted screenshot, a dropped image
/// the WebView handed us as data rather than as a path.
///
/// The filename comes from the WebView and is only ever a *label*: it names the
/// attachment in the timeline and picks the mime type when the caller didn't
/// supply one. It's reduced to a single path component all the same, so a name
/// like `../../x` can't be interpreted as a path by anything downstream.
pub async fn send_attachment_data(
    core: &MatrixCore,
    room_id: &RoomId,
    thread_root: Option<&str>,
    filename: &str,
    mime: Option<&str>,
    bytes: Vec<u8>,
    caption: Option<String>,
) -> Result<()> {
    use matrix_sdk_ui::timeline::{AttachmentConfig, AttachmentSource};

    if bytes.is_empty() {
        return Err(Error::Other("there was nothing in that file".into()));
    }

    let filename = safe_filename(filename);
    // Trust the caller's mime when it parses — the clipboard knows what it's
    // holding better than a guess from an invented filename does.
    let mime = mime
        .and_then(|m| m.parse::<mime::Mime>().ok())
        .unwrap_or_else(|| mime_guess::from_path(&filename).first_or_octet_stream());

    let timeline = get(core, room_id, thread_root).await?;
    let config = AttachmentConfig {
        caption: caption.map(TextMessageEventContent::plain),
        ..Default::default()
    };

    timeline
        .send_attachment(AttachmentSource::Data { bytes, filename }, mime, config)
        .use_send_queue()
        .await
        .map_err(|e| Error::Other(format!("upload failed: {e}")))?;
    Ok(())
}

/// Send a recorded voice message (an Ogg/Opus file): marked as voice
/// (MSC3245) with its length and waveform (MSC3246, values 0 to 1), so
/// clients draw it as a voice message rather than an audio file.
pub async fn send_voice(
    core: &MatrixCore,
    room_id: &RoomId,
    thread_root: Option<&str>,
    bytes: Vec<u8>,
    duration: std::time::Duration,
    waveform: Vec<f32>,
) -> Result<()> {
    use matrix_sdk::attachment::{AttachmentInfo, BaseAudioInfo};
    use matrix_sdk_ui::timeline::{AttachmentConfig, AttachmentSource};

    if bytes.is_empty() {
        return Err(Error::Other("the recording was empty".into()));
    }
    let size = matrix_sdk::ruma::UInt::new(bytes.len() as u64);
    let info = BaseAudioInfo {
        duration: Some(duration),
        size,
        waveform: Some(waveform.into_iter().map(|w| w.clamp(0., 1.)).collect()),
    };
    let config = AttachmentConfig { info: Some(AttachmentInfo::Voice(info)), ..Default::default() };
    let filename = format!("voice-message-{}.ogg", chrono_stamp());
    let mime: mime::Mime = "audio/ogg".parse().map_err(|e| Error::Other(format!("{e}")))?;

    let timeline = get(core, room_id, thread_root).await?;
    timeline
        .send_attachment(AttachmentSource::Data { bytes, filename }, mime, config)
        .use_send_queue()
        .await
        .map_err(|e| Error::Other(format!("upload failed: {e}")))?;
    Ok(())
}

/// Seconds since the epoch, to tell one recording's file name from the next.
fn chrono_stamp() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

/// Reduce a caller-supplied name to something safe to use as a filename.
fn safe_filename(filename: &str) -> String {
    let trimmed = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim_matches(|c: char| c == '.' || c.is_whitespace() || c.is_control());

    if trimmed.is_empty() { "attachment".to_owned() } else { trimmed.to_owned() }
}

#[cfg(test)]
mod emote_tests {
    use super::{EmoteRef, substitute_emotes};

    fn blobcat() -> Vec<EmoteRef> {
        vec![
            EmoteRef { shortcode: "blobcat".into(), url: "mxc://veil.gg/cat".into() },
            EmoteRef { shortcode: "uwu".into(), url: "mxc://veil.gg/uwu".into() },
        ]
    }

    #[test]
    fn replaces_a_shortcode_with_an_emoticon_image() {
        let html = substitute_emotes("hello :blobcat:", &blobcat()).expect("a replacement");
        assert!(html.starts_with("hello <img data-mx-emoticon "));
        assert!(html.contains("src=\"mxc://veil.gg/cat\""));
        // The shortcode stays as alt text, so clients without the pack still
        // read `:blobcat:` rather than nothing.
        assert!(html.contains("alt=\":blobcat:\""));
    }

    #[test]
    fn leaves_text_with_no_emotes_alone() {
        assert!(substitute_emotes("nothing here", &blobcat()).is_none());
        // A shortcode we don't have is somebody else's, or a typo. Either way
        // it stays as written.
        assert!(substitute_emotes("what about :nope:", &blobcat()).is_none());
    }

    #[test]
    fn keeps_the_markdown_it_was_given() {
        let html =
            substitute_emotes("<strong>yes</strong> :uwu:", &blobcat()).expect("a replacement");
        assert!(html.starts_with("<strong>yes</strong> <img "));
    }

    #[test]
    fn never_writes_into_a_tag() {
        // An autolinked URL that happens to contain a shortcode must not have
        // an <img> spliced into the middle of its href.
        let source = "<a href=\"https://x.example/:blobcat:\">link</a> and :blobcat:";
        let html = substitute_emotes(source, &blobcat()).expect("a replacement");

        assert!(html.contains("href=\"https://x.example/:blobcat:\""));
        assert_eq!(html.matches("<img").count(), 1);
    }

    #[test]
    fn replaces_every_occurrence() {
        let html = substitute_emotes(":uwu: :uwu: :blobcat:", &blobcat()).expect("replacements");
        assert_eq!(html.matches("<img").count(), 3);
    }

    #[test]
    fn survives_text_that_isnt_ascii() {
        let html = substitute_emotes("labas 👋 :uwu: ačiū", &blobcat()).expect("a replacement");
        assert!(html.starts_with("labas 👋 <img"));
        assert!(html.ends_with(" ačiū"));
    }

    #[test]
    fn ignores_an_emote_whose_url_isnt_on_a_homeserver() {
        let remote = vec![EmoteRef {
            shortcode: "tracker".into(),
            url: "https://evil.example/pixel.gif".into(),
        }];
        assert!(substitute_emotes("hi :tracker:", &remote).is_none());
    }

    #[test]
    fn a_lone_colon_is_just_a_colon() {
        assert!(substitute_emotes("ratio 3:1, right", &blobcat()).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::safe_filename;

    #[test]
    fn keeps_an_ordinary_name() {
        assert_eq!(safe_filename("screenshot.png"), "screenshot.png");
        assert_eq!(safe_filename("ekrano nuotrauka.png"), "ekrano nuotrauka.png");
    }

    #[test]
    fn reduces_a_path_to_its_last_component() {
        assert_eq!(safe_filename("../../etc/passwd"), "passwd");
        assert_eq!(safe_filename("C:\\windows\\system32\\x.dll"), "x.dll");
    }

    #[test]
    fn never_returns_something_unusable() {
        // A name that is only separators, dots or blank has nothing left after
        // trimming, and an empty filename is not a valid attachment.
        assert_eq!(safe_filename("../.."), "attachment");
        assert_eq!(safe_filename("   "), "attachment");
        assert_eq!(safe_filename(""), "attachment");
    }
}

/// The set of events currently pinned in a room, newest first.
pub async fn pinned_events(core: &MatrixCore, room_id: &RoomId) -> Result<Vec<TimelineItemDto>> {
    let room = core.room(&room_id.to_owned())?;
    let own_user = core.own_user_id()?;

    let timeline = room
        .timeline_builder()
        .with_focus(TimelineFocus::PinnedEvents)
        .build()
        .await?;

    let items = timeline.items().await;
    Ok(items.iter().map(|i| convert_timeline_item(i, &own_user)).collect())
}

/// The plain-text body of an event, for building reply previews in the composer.
pub async fn event_body(
    core: &MatrixCore,
    room_id: &RoomId,
    event_id: &str,
) -> Result<Option<String>> {
    let timeline = get(core, room_id, None).await?;
    let event_id = EventId::parse(event_id)?;

    let Some(item) = timeline.item_by_event_id(&event_id).await else {
        return Ok(None);
    };

    use matrix_sdk_ui::timeline::{MsgLikeKind, TimelineItemContent};
    let body = match item.content() {
        TimelineItemContent::MsgLike(msg) => match &msg.kind {
            MsgLikeKind::Message(m) => Some(match m.msgtype() {
                MessageType::Text(t) => t.body.clone(),
                other => other.body().to_owned(),
            }),
            _ => None,
        },
        _ => None,
    };
    Ok(body)
}
