//! Writing to a room: a growing text field under the timeline. Enter sends,
//! Shift+Enter starts a new line, Markdown is on (ruma only adds a formatted
//! body when the text has markup in it).
//!
//! It can also be replying to a message or editing one of yours, said in a
//! bar above the field; Escape goes back to writing. On an empty field the
//! up arrow edits your last message and the down arrow replies to the last
//! thing someone else said, as in the WebView build. (Mentions, emoji and
//! pack autocomplete, attachments and stickers follow.)

use gpui::{
    AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    StyledImage, WeakEntity, Window, div, img, prelude::FluentBuilder,
};
use gpui_component::{
    h_flex, v_flex,
    input::{IndentInline, InputEvent, MoveDown, MoveUp, Position, Textarea, TextareaState},
};
use horo_core::{
    dto::{MentionRef, SendOptions, StickerOptions},
    matrix::timeline,
};
use matrix_sdk::ruma::OwnedRoomId;

use crate::theme::zpx;
use crate::{
    backend::Backend,
    media::Media,
    mentions::{self, Person},
    packs::{self, Picked},
    store::Store,
    theme::{Palette, corner, space, text},
    views::{
        avatar::avatar,
        emoji_picker::{EmojiPicker, PickerEvent},
        timeline::{Timeline, TimelineAction},
        voice::icon,
    },
};

/// What Enter will do with the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Write,
    Reply { event_id: String, who: String, preview: String },
    Edit { event_id: String },
}

pub struct Composer {
    room_id: OwnedRoomId,
    /// Writing into this thread, or the room.
    thread_root: Option<String>,
    timeline: WeakEntity<Timeline>,
    input: Entity<TextareaState>,
    mode: Mode,
    error: Option<SharedString>,
    /// Said under the box when it isn't: the one case worth a line.
    encrypted: bool,
    store: Entity<Store>,
    /// The emoji picker, open over the box, and where it was opened.
    picker: Option<(gpui::Point<gpui::Pixels>, Entity<EmojiPicker>)>,
    /// Custom emotes matching the `:shortcode` being typed.
    completing: Option<Completing>,
    /// A voice message being recorded, and its recent levels for the meter.
    recording: Option<Recording>,
    levels: std::collections::VecDeque<f32>,
    /// When we last told the room we're typing; `None` when we aren't.
    typing_sent: Option<std::time::Instant>,
    /// The room's members, for `@` suggestions: fetched the first time
    /// they're wanted.
    people: Option<Vec<Person>>,
    people_loading: bool,
    /// People picked from the suggestions for the message being written.
    mentions: Vec<MentionRef>,
    /// Files pasted in, waiting to go with the message.
    staged: Vec<Staged>,
}

/// A file waiting in the box, and what to show for it.
struct Staged {
    file: Outgoing,
    /// The picture itself, when it is one.
    preview: Option<gpui::ImageSource>,
}

impl Staged {
    fn new(file: Outgoing) -> Staged {
        let preview = match &file {
            Outgoing::Bytes { bytes, mime, .. } => {
                image_format(mime).map(|format| gpui::ImageSource::Image(std::sync::Arc::new(gpui::Image::from_bytes(format, bytes.clone()))))
            }
            Outgoing::Path(path) => packs::mime_from_path(&path.to_string_lossy())
                .is_some_and(|m| m.starts_with("image/"))
                .then(|| gpui::ImageSource::from(path.clone())),
        };
        Staged { file, preview }
    }
}

/// The picture format a clipboard type names.
fn image_format(mime: &str) -> Option<gpui::ImageFormat> {
    use gpui::ImageFormat::*;
    Some(match mime {
        "image/png" => Png,
        "image/jpeg" => Jpeg,
        "image/webp" => Webp,
        "image/gif" => Gif,
        "image/bmp" => Bmp,
        "image/tiff" => Tiff,
        _ => return None,
    })
}

/// How long a typing notice stands before it's refreshed (the server
/// forgets one after about 30 s).
const TYPING_REFRESH: std::time::Duration = std::time::Duration::from_secs(4);

/// How many recent levels the recording meter shows.
const METER_BARS: usize = 36;

/// How long a recording runs before it can be sent: the send button sits
/// where the mic button was, so a second click meant for the mic (or a
/// double click) mustn't send a fragment.
const SEND_AFTER: std::time::Duration = std::time::Duration::from_millis(600);

enum Recording {
    /// The microphone is opening (off the UI thread: it can take a moment).
    Starting,
    Live { recorder: horo_rtc::voice::Recorder, since: std::time::Instant },
}

impl Recording {
    /// Whether it's been recording long enough to send.
    fn sendable(&self) -> bool {
        matches!(self, Recording::Live { since, .. } if since.elapsed() >= SEND_AFTER)
    }
}

/// The shortcode menu over the box: what matches, where the typed `:code`
/// starts (a byte offset), and which match Enter or Tab would put in.
struct Completing {
    start: usize,
    matches: Vec<Suggestion>,
    active: usize,
}

/// One line of the suggestions menu.
enum Suggestion {
    Emote(horo_core::matrix::packs::PackImageDto),
    Person(Person),
}

/// How many matches the menu offers.
const COMPLETIONS: usize = 8;

impl Composer {
    pub fn new(
        // The room, and the thread in it when writing into one.
        (room_id, thread_root): (OwnedRoomId, Option<String>),
        room_name: &str,
        encrypted: bool,
        store: Entity<Store>,
        timeline: WeakEntity<Timeline>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Composer {
        let placeholder = if thread_root.is_some() { "reply in the thread~".to_owned() } else { format!("say something cute in {room_name}~") };
        let input = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx);
            state.set_auto_grow(1, 10, cx);
            state.set_submit_on_enter(true, cx);
            state.set_placeholder(placeholder, window, cx);
            state
        });
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            match event {
                // While the menu is up, Enter picks from it rather than
                // sending `:blobc` as a message.
                InputEvent::PressEnter { shift: false, .. } if this.completing.is_some() => this.complete(None, window, cx),
                InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                InputEvent::Change => {
                    this.update_completion(cx);
                    this.note_typing(cx);
                }
                _ => {}
            }
        })
        .detach();
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        // Leaving the room mustn't leave us "typing" there for everyone
        // until the server times it out.
        cx.on_release(|this: &mut Composer, cx| {
            if this.typing_sent.is_some() {
                send_typing(this.room_id.clone(), false, cx);
            }
        })
        .detach();
        Composer {
            room_id,
            thread_root,
            timeline,
            input,
            mode: Mode::Write,
            error: None,
            encrypted,
            store,
            picker: None,
            completing: None,
            recording: None,
            levels: Default::default(),
            typing_sent: None,
            people: None,
            people_loading: false,
            mentions: Vec::new(),
            staged: Vec::new(),
        }
    }

    /// Tell the room we're typing while there's text, at most every few
    /// seconds; and that we've stopped when the box empties.
    fn note_typing(&mut self, cx: &mut Context<Self>) {
        let empty = self.input.read(cx).value().trim().is_empty();
        if empty {
            self.stop_typing(cx);
        } else if self.typing_sent.is_none_or(|at| at.elapsed() >= TYPING_REFRESH) {
            self.typing_sent = Some(std::time::Instant::now());
            send_typing(self.room_id.clone(), true, cx);
        }
    }

    fn stop_typing(&mut self, cx: &mut Context<Self>) {
        if self.typing_sent.take().is_some() {
            send_typing(self.room_id.clone(), false, cx);
        }
    }

    /// The picker, over the box; a pick goes in at the cursor.
    fn open_picker(&mut self, at: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.take().is_some() {
            cx.notify();
            return;
        }
        let store = self.store.clone();
        let picker = cx.new(|cx| EmojiPicker::new(store, true, window, cx));
        cx.subscribe_in(&picker, window, |this, _, event: &PickerEvent, window, cx| {
            this.picker = None;
            // An emoji goes in as itself, an emote as its shortcode (sending
            // turns it into the picture), and a sticker is sent at once.
            let text = match event {
                PickerEvent::Pick(Picked::Unicode(emoji)) => Some(emoji.clone()),
                PickerEvent::Pick(Picked::Emote(image)) => Some(format!(":{}: ", image.shortcode)),
                PickerEvent::Pick(Picked::Sticker(image)) => {
                    this.send_sticker(image.clone(), cx);
                    None
                }
                PickerEvent::Close => None,
            };
            this.input.update(cx, |state, cx| {
                if let Some(text) = text {
                    state.insert(text, window, cx);
                }
                state.focus(window, cx);
            });
            cx.notify();
        })
        .detach();
        self.picker = Some((at, picker));
        cx.notify();
    }

    fn send_sticker(&mut self, image: horo_core::matrix::packs::PackImageDto, cx: &mut Context<Self>) {
        let sticker = StickerOptions {
            body: image.body.clone(),
            url: image.url.clone(),
            width: image.width,
            height: image.height,
            size: image.size.and_then(|s| u32::try_from(s).ok()),
            mimetype: image.mimetype.clone(),
        };
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, room_id, thread) = (backend.state.clone(), self.room_id.clone(), self.thread_root.clone());
            async move { timeline::send_sticker(&*state.core().await?, &room_id, thread.as_deref(), sticker).await }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => return,
                Ok(Err(e)) => e.to_string(),
                Err(e) => e.to_string(),
            };
            let _ = this.update(cx, |this, cx| {
                this.error = Some(format!("couldn't send the sticker: {error}").into());
                cx.notify();
            });
        })
        .detach();
    }

    /// Put the caret in the box.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |state, cx| state.focus(window, cx));
    }

    /// Reply to, or edit, what the timeline says.
    pub fn take(&mut self, action: &TimelineAction, window: &mut Window, cx: &mut Context<Self>) {
        match action.clone() {
            TimelineAction::Reply { event_id, who, preview } => {
                // Leaving an edit drops its text; a reply keeps what's typed.
                if matches!(self.mode, Mode::Edit { .. }) {
                    self.input.update(cx, |state, cx| state.set_value("", window, cx));
                }
                self.mode = Mode::Reply { event_id, who, preview };
            }
            TimelineAction::Edit { event_id, body } => {
                // The caret goes after the text, where you'd carry on typing
                // (the column is clamped to the last line's length).
                let last_line = body.matches('\n').count() as u32;
                self.input.update(cx, |state, cx| {
                    state.set_value(body, window, cx);
                    state.set_cursor_position(Position::new(last_line, u32::MAX), window, cx);
                });
                self.mode = Mode::Edit { event_id };
            }
        }
        self.error = None;
        self.input.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.mode, Mode::Edit { .. }) {
            self.input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.mode = Mode::Write;
        cx.notify();
    }

    /// Escape leaves a reply or an edit. Seen before the field does.
    /// Whether a shortcode is being typed at the caret, and what it matches.
    fn update_completion(&mut self, cx: &mut Context<Self>) {
        let (text, caret) = {
            let input = self.input.read(cx);
            (input.value().to_string(), input.cursor())
        };
        self.completing = if let Some((query, start)) = mentions::typing_mention(&text, caret) {
            match &self.people {
                Some(people) => {
                    let matches: Vec<Suggestion> = mentions::match_people(people, &query, COMPLETIONS).into_iter().map(Suggestion::Person).collect();
                    (!matches.is_empty()).then_some(Completing { start, matches, active: 0 })
                }
                None => {
                    self.load_people(cx);
                    None
                }
            }
        } else {
            packs::typing_shortcode(&text, caret).and_then(|(query, start)| {
                let matches: Vec<Suggestion> =
                    packs::match_emotes(&packs::emote_lookup(&self.store.read(cx).packs), &query, COMPLETIONS).into_iter().map(Suggestion::Emote).collect();
                (!matches.is_empty()).then_some(Completing { start, matches, active: 0 })
            })
        };
        cx.notify();
    }

    /// Fetch the room's members for suggestions; the menu opens when
    /// they're in, if an `@name` is still being typed.
    fn load_people(&mut self, cx: &mut Context<Self>) {
        if self.people_loading {
            return;
        }
        self.people_loading = true;
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, room_id) = (backend.state.clone(), self.room_id.clone());
            async move { horo_core::matrix::rooms::members(&*state.core().await?, &room_id).await }
        });
        cx.spawn(async move |this, cx| {
            let members = match task.await {
                Ok(Ok(members)) => members,
                _ => Vec::new(),
            };
            let _ = this.update(cx, |composer, cx| {
                let own = composer.store.read(cx).own_user_id().map(str::to_owned);
                let people = members
                    .iter()
                    .filter(|m| m.membership == "join" && !m.is_ignored && own.as_deref() != Some(m.user_id.as_str()))
                    .map(Person::from_member)
                    .collect();
                composer.people = Some(people);
                composer.people_loading = false;
                composer.update_completion(cx);
            });
        })
        .detach();
    }

    /// Put in the chosen match (the highlighted one unless `pick` says),
    /// replacing the `:code` typed so far.
    fn complete(&mut self, pick: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(completing) = self.completing.take() else { return };
        let Some(pick) = completing.matches.get(pick.unwrap_or(completing.active)) else { return };
        let text = match pick {
            Suggestion::Emote(image) => format!(":{}: ", image.shortcode),
            Suggestion::Person(person) => {
                if !self.mentions.iter().any(|m| m.user_id == person.user_id) {
                    self.mentions.push(MentionRef { user_id: person.user_id.clone(), name: person.name.clone() });
                }
                format!("{} ", person.name)
            }
        };
        self.input.update(cx, |state, cx| {
            let caret = state.cursor();
            state.set_selected_range(completing.start..caret, cx);
            state.replace(text, window, cx);
            state.focus(window, cx);
        });
        // The edit above reopens the menu for what's now `:code: `; it isn't.
        self.completing = None;
        cx.notify();
    }

    fn move_completion(&mut self, up: bool, cx: &mut Context<Self>) {
        if let Some(c) = &mut self.completing {
            let n = c.matches.len();
            c.active = if up { (c.active + n - 1) % n } else { (c.active + 1) % n };
            cx.notify();
        }
    }

    /// Start recording a voice message from the chosen microphone.
    /// The strip shows at once; the microphone opens in the background.
    fn start_recording(&mut self, cx: &mut Context<Self>) {
        if self.recording.is_some() {
            return;
        }
        let microphone = self.store.read(cx).prefs.audio_input.clone();
        self.recording = Some(Recording::Starting);
        self.levels.clear();
        self.error = None;
        cx.notify();
        let opening = cx.background_spawn(async move { horo_rtc::voice::Recorder::start(microphone.as_deref()) });
        cx.spawn(async move |this, cx| {
            let opened = opening.await;
            let _ = this.update(cx, |this, cx| {
                // Cancelled while it opened: dropping the recorder stops it.
                if !matches!(this.recording, Some(Recording::Starting)) {
                    return;
                }
                match opened {
                    Ok(recorder) => this.recording = Some(Recording::Live { recorder, since: std::time::Instant::now() }),
                    Err(e) => {
                        this.recording = None;
                        this.error = Some(format!("couldn't start the microphone: {e}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn cancel_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Dropping it stops the microphone; nothing is kept.
        self.recording = None;
        self.input.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    /// Stop, and send what was recorded as a voice message.
    fn send_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.recording.as_ref().is_some_and(Recording::sendable) {
            return;
        }
        let Some(Recording::Live { recorder, .. }) = self.recording.take() else { return };
        self.input.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
        let note = match recorder.finish() {
            Ok(Some(note)) => note,
            Ok(None) => return,
            Err(e) => {
                self.error = Some(e.to_string().into());
                return;
            }
        };
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, room_id, thread) = (backend.state.clone(), self.room_id.clone(), self.thread_root.clone());
            async move { timeline::send_voice(&*state.core().await?, &room_id, thread.as_deref(), note.bytes, note.duration, note.waveform).await }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => return,
                Ok(Err(e)) => e.to_string(),
                Err(e) => e.to_string(),
            };
            let _ = this.update(cx, |this, cx| {
                this.error = Some(format!("couldn't send the voice message: {error}").into());
                cx.notify();
            });
        })
        .detach();
    }

    /// The box while recording: cancel, the time and a live meter, send.
    fn recording_strip(&mut self, palette: &Palette, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (elapsed, level) = match &self.recording {
            Some(Recording::Live { recorder, .. }) => (recorder.elapsed(), recorder.level()),
            _ => Default::default(),
        };
        let starting = matches!(self.recording, Some(Recording::Starting));
        self.levels.push_back(level);
        while self.levels.len() > METER_BARS {
            self.levels.pop_front();
        }
        let secs = elapsed.as_secs();
        let mut meter = h_flex().flex_1().min_w_0().h(zpx(24.)).items_center().justify_end().gap(zpx(2.));
        for _ in self.levels.len()..METER_BARS {
            meter = meter.child(div().w(zpx(3.)).h(zpx(3.)).rounded_full().bg(palette.border));
        }
        for level in &self.levels {
            meter = meter.child(div().w(zpx(3.)).h(zpx(3. + 19. * level)).rounded_full().bg(palette.danger));
        }
        h_flex()
            .flex_1()
            .min_w_0()
            .gap(space(10.))
            .child(
                div()
                    .id("recording-cancel")
                    .size(zpx(38.))
                    .flex_none()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(palette.text_tertiary)
                    .hover(|d| d.bg(palette.inset).text_color(palette.text))
                    .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("throw it away").build(window, cx))
                    .child("✕")
                    .on_click(cx.listener(|this, _, window, cx| this.cancel_recording(window, cx))),
            )
            .child(div().flex_none().size(zpx(9.)).rounded_full().bg(palette.danger))
            .child(div().flex_none().font_family(palette.mono_font.clone()).text_size(text(13.)).child(format!("{}:{:02}", secs / 60, secs % 60)))
            .map(|row| {
                if starting {
                    row.child(div().flex_1().text_size(text(12.5)).text_color(palette.text_tertiary).child("starting the microphone…"))
                } else {
                    row.child(meter)
                }
            })
            .into_any_element()
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording.is_some() && event.keystroke.key == "escape" {
            self.cancel_recording(window, cx);
            cx.stop_propagation();
            return;
        }
        // Escape closes the shortcode menu first, before it cancels a
        // reply or an edit.
        if self.completing.is_some() && event.keystroke.key == "escape" {
            self.completing = None;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if event.keystroke.key == "escape" && self.mode != Mode::Write {
            self.cancel(window, cx);
            cx.stop_propagation();
        }
    }

    /// The arrows on an empty field reach for a message. They're the
    /// field's own actions (moving the caret), caught on their way to it;
    /// anywhere else they go on to move the caret.
    fn recall(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        // The shortcode menu owns the arrows while it's up.
        if self.completing.is_some() {
            self.move_completion(up, cx);
            cx.stop_propagation();
            return;
        }
        let empty = self.input.read(cx).value().is_empty();
        if !empty || matches!(self.mode, Mode::Edit { .. }) {
            return;
        }
        let Some(timeline) = self.timeline.upgrade() else { return };
        let action = if up {
            timeline.read(cx).last_editable().map(|(event_id, body)| TimelineAction::Edit { event_id, body })
        } else {
            timeline.read(cx).last_replyable()
        };
        if let Some(action) = action {
            self.take(&action, window, cx);
            cx.stop_propagation();
        }
    }

    /// Pick files to attach; they wait in the box with the rest.
    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("attach".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let _ = this.update(cx, |composer, cx| composer.stage_paths(paths, cx));
        })
        .detach();
    }

    /// Files dropped on the room or picked to attach: they wait in the
    /// box, to go with the next message.
    pub fn stage_paths(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        self.staged.extend(paths.into_iter().map(|p| Staged::new(Outgoing::Path(p))));
        cx.notify();
    }

    /// A paste: files copied in Finder or a picture go to the room, in
    /// place of being pasted as text. `false` when it's text after all.
    fn paste(&mut self, item: &gpui::ClipboardItem, cx: &mut Context<Self>) -> bool {
        let files = pasted(item, chrono::Local::now());
        if files.is_empty() {
            return false;
        }
        self.staged.extend(files.into_iter().map(Staged::new));
        cx.notify();
        true
    }

    /// Queue each file in turn, saying under the box which ones failed;
    /// done when the last is queued.
    fn upload(&mut self, files: Vec<Outgoing>, cx: &mut Context<Self>) -> gpui::Task<()> {
        let (room_id, thread_root) = (self.room_id.clone(), self.thread_root.clone());
        self.error = None;
        cx.spawn(async move |this, cx| {
            for file in files {
                let name = file.name();
                if let Outgoing::Path(path) = &file
                    && std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_ATTACHMENT_BYTES)
                {
                    let _ = this.update(cx, |composer, cx| {
                        composer.error = Some(format!("{name} is too big to send — 100mb is the limit here").into());
                        cx.notify();
                    });
                    continue;
                }
                let Ok(task) = this.update(cx, |_, cx| {
                    let backend = Backend::global(cx);
                    let (state, room_id, thread) = (backend.state.clone(), room_id.clone(), thread_root.clone());
                    backend.run(async move {
                        let core = state.core().await?;
                        match file {
                            Outgoing::Path(path) => timeline::send_attachment(&core, &room_id, thread.as_deref(), &path.to_string_lossy(), None).await,
                            Outgoing::Bytes { bytes, mime, name } => timeline::send_attachment_data(&core, &room_id, thread.as_deref(), &name, Some(mime), bytes, None).await,
                        }
                    })
                }) else {
                    return;
                };
                let error = match task.await {
                    Ok(Ok(())) => continue,
                    Ok(Err(e)) => e.to_string(),
                    Err(e) => e.to_string(),
                };
                let _ = this.update(cx, |composer, cx| {
                    composer.error = Some(format!("couldn't send {name}: {error}").into());
                    cx.notify();
                });
            }
        })
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.input.read(cx).value().trim().to_owned();
        // Pasted files go first, then the words, as their own message.
        let files: Vec<Outgoing> = std::mem::take(&mut self.staged).into_iter().map(|s| s.file).collect();
        let uploaded = (!files.is_empty()).then(|| self.upload(files, cx));
        if body.is_empty() {
            if let Some(uploaded) = uploaded {
                uploaded.detach();
            }
            cx.notify();
            return;
        }
        let mode = std::mem::replace(&mut self.mode, Mode::Write);
        self.input.update(cx, |state, cx| state.set_value("", window, cx));
        self.stop_typing(cx);
        self.error = None;
        let mentions = std::mem::take(&mut self.mentions);
        let backend = Backend::global(cx);
        let job = {
            let (state, room_id, body, mode) = (backend.state.clone(), self.room_id.clone(), body.clone(), mode.clone());
            let thread_root = self.thread_root.clone();
            // Every emote we know; the backend puts in the ones the text uses.
            let emotes = packs::emote_refs(&self.store.read(cx).packs);
            // The core keeps only the ones still named in the text.
            let mentions = mentions.clone();
            async move {
                let core = state.core().await?;
                match mode {
                    Mode::Edit { event_id } => timeline::edit(&core, &room_id, thread_root.as_deref(), &event_id, &body, true).await,
                    Mode::Write | Mode::Reply { .. } => {
                        let reply_to = match mode {
                            Mode::Reply { event_id, .. } => Some(event_id),
                            _ => None,
                        };
                        let options = SendOptions { body, markdown: true, reply_to, thread_root, msgtype: None, emotes, mentions };
                        timeline::send(&core, &room_id, options).await
                    }
                }
            }
        };
        // The message shows in the timeline as soon as the SDK queues it;
        // only a failure needs telling here, with the text and the mode
        // given back.
        cx.spawn_in(window, async move |this, cx| {
            // After the files are queued, so it lands under them.
            if let Some(uploaded) = uploaded {
                uploaded.await;
            }
            let Ok(task) = this.update(cx, |_, cx| Backend::global(cx).run(job)) else { return };
            let failure = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            if let Some(error) = failure {
                let _ = this.update_in(cx, |composer, window, cx| {
                    composer.error = Some(format!("couldn't send: {error}").into());
                    if composer.input.read(cx).value().is_empty() {
                        composer.input.update(cx, |state, cx| state.set_value(body, window, cx));
                        composer.mode = mode;
                        composer.mentions = mentions;
                    }
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }
}

impl Composer {
    /// The files waiting to go: a picture of each (or its name), each
    /// with a button to take it out again.
    fn staged_strip(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.staged.is_empty() {
            return None;
        }
        let tiles = self.staged.iter().enumerate().map(|(i, staged)| {
            let face: AnyElement = match &staged.preview {
                Some(source) => img(source.clone()).size_full().object_fit(gpui::ObjectFit::Cover).into_any_element(),
                None => v_flex()
                    .size_full()
                    .p(zpx(6.))
                    .gap_1()
                    .items_center()
                    .justify_center()
                    .child(icon("icons/file.svg").size(zpx(20.)).text_color(palette.text_tertiary))
                    .child(div().w_full().truncate().text_center().text_size(text(10.)).text_color(palette.text_secondary).child(staged.file.name()))
                    .into_any_element(),
            };
            div()
                .id(("staged", i))
                .relative()
                .flex_none()
                .size(zpx(STAGED_SIZE))
                .rounded(corner(14.))
                .overflow_hidden()
                .bg(palette.card)
                .border_1()
                .border_color(palette.border_subtle)
                .tooltip({
                    let name = SharedString::from(staged.file.name());
                    move |window, cx| gpui_component::tooltip::Tooltip::new(name.clone()).build(window, cx)
                })
                .child(face)
                .child(
                    div()
                        .id(("unstage", i))
                        .absolute()
                        .top(zpx(4.))
                        .right(zpx(4.))
                        .size(zpx(20.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::Hsla { a: 0.8, ..palette.app })
                        .text_size(zpx(11.))
                        .text_color(palette.text)
                        .cursor_pointer()
                        .hover(|d| d.bg(palette.danger))
                        .child("✕")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if i < this.staged.len() {
                                this.staged.remove(i);
                            }
                            cx.notify();
                        })),
                )
        });
        Some(h_flex().mb(zpx(8.)).gap(space(8.)).flex_wrap().children(tiles).into_any_element())
    }
}

/// How big a waiting file's tile is.
const STAGED_SIZE: f32 = 84.;

/// A typing notice, sent and forgotten: one that fails only means the
/// dots don't show for someone.
fn send_typing(room_id: OwnedRoomId, typing: bool, cx: &mut App) {
    let backend = Backend::global(cx);
    let state = backend.state.clone();
    backend.spawn(async move {
        if let Ok(core) = state.core().await {
            let _ = horo_core::matrix::rooms::set_typing(&core, &room_id, typing).await;
        }
    });
}

/// The most a file may be (the usual server upload limit).
const MAX_ATTACHMENT_BYTES: u64 = 100 * 1024 * 1024;

/// A file on its way to the room.
#[derive(Debug, PartialEq)]
enum Outgoing {
    Path(std::path::PathBuf),
    /// A picture from the clipboard.
    Bytes { bytes: Vec<u8>, mime: &'static str, name: String },
}

impl Outgoing {
    fn name(&self) -> String {
        match self {
            Outgoing::Path(path) => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            Outgoing::Bytes { name, .. } => name.clone(),
        }
    }
}

/// What in a paste should be sent as files, `now` naming pictures.
///
/// Files copied in Finder come with their icon as a picture and their
/// name as text: the files are what's meant. A picture comes with text
/// when it's copied from a browser (its address) — but also when rich
/// text is copied from an editor that offers a rendering of it, so a
/// picture beside any other text is left to paste as that text.
fn pasted(item: &gpui::ClipboardItem, now: chrono::DateTime<chrono::Local>) -> Vec<Outgoing> {
    use gpui::ClipboardEntry;
    let paths: Vec<Outgoing> = item
        .entries()
        .iter()
        .filter_map(|e| match e {
            ClipboardEntry::ExternalPaths(p) => Some(p.paths().iter().cloned().map(Outgoing::Path)),
            _ => None,
        })
        .flatten()
        .collect();
    if !paths.is_empty() {
        return paths;
    }
    let text = item.text().unwrap_or_default();
    let text = text.trim();
    let just_an_address = !text.contains(char::is_whitespace) && (text.starts_with("http://") || text.starts_with("https://"));
    if !text.is_empty() && !just_an_address {
        return Vec::new();
    }
    let stamp = now.format("%Y%m%d-%H%M%S");
    item.entries()
        .iter()
        .filter_map(|e| match e {
            ClipboardEntry::Image(image) => Some(image),
            _ => None,
        })
        .enumerate()
        .map(|(i, image)| {
            let mime = image.format.mime_type();
            let extension = mime.rsplit('/').next().unwrap_or("png").split('+').next().unwrap_or("png");
            let name = if i == 0 { format!("pasted-{stamp}.{extension}") } else { format!("pasted-{stamp}-{i}.{extension}") };
            Outgoing::Bytes { bytes: image.bytes.clone(), mime, name }
        })
        .collect()
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The time and the meter move while recording.
        if self.recording.is_some() {
            window.request_animation_frame();
        }
        let palette = cx.global::<Palette>().clone();
        let bar = match &self.mode {
            Mode::Write => None,
            Mode::Reply { who, preview, .. } => Some((palette.accent_tertiary, format!("replying to {who}"), preview.clone())),
            Mode::Edit { .. } => Some((palette.accent, "editing your message".to_owned(), "escape to cancel".to_owned())),
        };
        let can_send = !self.input.read(cx).value().trim().is_empty() || !self.staged.is_empty();
        let staged = self.staged_strip(&palette, cx);
        let recording = self.recording.is_some();
        let sendable = self.recording.as_ref().is_some_and(Recording::sendable);
        let strip = recording.then(|| self.recording_strip(&palette, cx));
        let menu = self.completing.as_ref().map(|completing| {
            let mut list = v_flex()
                .id("shortcodes")
                .occlude()
                .absolute()
                .left(zpx(10.))
                .bottom(gpui::relative(1.))
                .mb(zpx(6.))
                .min_w(zpx(220.))
                .max_w(zpx(320.))
                .p(space(5.))
                .rounded(corner(16.))
                .bg(palette.raised)
                .border_1()
                .border_color(palette.border)
                .shadow_lg();
            for (i, suggestion) in completing.matches.iter().enumerate() {
                let on = i == completing.active;
                let (picture, label, detail) = match suggestion {
                    Suggestion::Emote(image) => (
                        match Media::image(&image.url, Some((48, 48)), cx) {
                            Some(picture) => img(picture).size(zpx(22.)).object_fit(gpui::ObjectFit::Contain).into_any_element(),
                            None => div().size(zpx(22.)).into_any_element(),
                        },
                        div().font_family(palette.mono_font.clone()).child(format!(":{}:", image.shortcode)),
                        None,
                    ),
                    Suggestion::Person(person) => (
                        avatar(&person.user_id, &person.name, person.avatar.as_deref(), zpx(22.), cx),
                        div().child(person.name.clone()),
                        Some(person.user_id.clone()),
                    ),
                };
                list = list.child(
                    h_flex()
                        .id(("shortcode", i))
                        .gap(space(9.))
                        .px(space(9.))
                        .py(space(5.))
                        .rounded(corner(11.))
                        .cursor_pointer()
                        .when(on, |d| d.bg(gpui::Hsla { a: 0.08, ..gpui::white() }))
                        .hover(|d| d.bg(gpui::Hsla { a: 0.08, ..gpui::white() }))
                        .child(picture)
                        .child(label.truncate().text_size(text(12.)).text_color(if on { palette.text } else { palette.text_secondary }))
                        .children(detail.map(|id| {
                            div().flex_1().truncate().font_family(palette.mono_font.clone()).text_size(text(10.5)).text_color(palette.text_tertiary).child(id)
                        }))
                        // On press, so the field keeps its focus and caret.
                        .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.complete(Some(i), window, cx);
                        })),
                );
            }
            list
        });
        div()
            .flex_none()
            .px(space(22.))
            .pt(space(12.))
            .pb(space(18.))
            .children(self.error.clone().map(|e| div().pb_1().text_size(text(12.)).text_color(palette.danger).child(e)))
            .children(bar.map(|(colour, title, detail)| {
                h_flex()
                    .mb(zpx(8.))
                    .gap(space(10.))
                    .px(space(14.))
                    .py(space(7.))
                    .rounded(corner(14.))
                    .bg(palette.card)
                    .relative()
                    .overflow_hidden()
                    .border_1()
                    .border_color(palette.border_subtle)
                    // The coloured left edge: gpui borders take one colour.
                    .child(div().absolute().left_0().top_0().bottom_0().w(zpx(2.)).bg(colour))
                    .text_size(text(12.5))
                    .child(div().flex_none().text_color(colour).child(title))
                    .child(div().flex_1().min_w_0().truncate().text_color(palette.text_tertiary).child(detail))
                    .child(
                        div()
                            .id("composer-cancel")
                            .flex_none()
                            .px_1()
                            .cursor_pointer()
                            .text_color(palette.text_tertiary)
                            .hover(|d| d.text_color(palette.text))
                            .child("✕")
                            .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))),
                    )
            }))
            .children(staged)
            .child(
                h_flex()
                    .relative()
                    .children(menu)
                    .items_end()
                    .gap(space(10.))
                    .px(space(12.))
                    .py(space(9.))
                    .rounded(corner(24.))
                    .bg(palette.raised)
                    .border_1()
                    .border_color(match self.mode {
                        Mode::Edit { .. } => palette.accent,
                        _ => palette.border,
                    })
                    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| this.on_key(event, window, cx)))
                    // Tab is the field's indent action, caught on its way:
                    // with the shortcode menu up it completes instead.
                    .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                        if this.completing.is_some() {
                            this.complete(None, window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .capture_action(cx.listener(|this, _: &MoveUp, window, cx| this.recall(true, window, cx)))
                    .capture_action(cx.listener(|this, _: &MoveDown, window, cx| this.recall(false, window, cx)))
                    .map(|row| match strip {
                        Some(strip) => row.child(strip),
                        None => row
                            .child(
                            div()
                                .id("composer-attach")
                                .size(zpx(38.))
                                .flex_none()
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("attach a file").build(window, cx))
                                .child(icon("icons/circle-plus.svg").size(zpx(22.)).text_color(palette.text_tertiary))
                                .on_click(cx.listener(|this, _, window, cx| this.attach(window, cx))),
                        )
                        .child(div().flex_1().min_w_0().py(space(4.)).text_size(crate::theme::message_text()).child(Textarea::new(&self.input).appearance(false).on_paste({
                            let this = cx.weak_entity();
                            move |item, _, cx| this.update(cx, |composer, cx| composer.paste(item, cx)).unwrap_or(false)
                        })))
                        .child(
                            div()
                                .id("composer-emoji")
                                .size(zpx(38.))
                                .flex_none()
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("emoji").build(window, cx))
                                .child(icon("icons/smile.svg").size(zpx(20.)).text_color(if self.picker.is_some() { palette.accent } else { palette.text_tertiary }))
                                // On press, so the picker's own "pressed outside"
                                // doesn't close it and this reopen it.
                                .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    // Above the button, clear of the box.
                                    this.open_picker(gpui::point(event.position.x, event.position.y - zpx(28.)), window, cx)
                                })),
                        ),
                    })
                    .child(
                        div()
                            .id("composer-send")
                            .size(zpx(38.))
                            .flex_none()
                            .rounded_full()
                            .bg(palette.accent)
                            .border_2()
                            .border_color(palette.app)
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(recording && !sendable, |d| d.opacity(0.45))
                            .when(!recording || sendable, |d| d.cursor_pointer())
                            .tooltip(move |window, cx| {
                                let words = if recording { "send the voice message" } else if can_send { "send" } else { "record a voice message" };
                                gpui_component::tooltip::Tooltip::new(words).build(window, cx)
                            })
                            // Empty, it records; recording, it sends that.
                            .child(icon(if can_send || recording { "icons/send.svg" } else { "icons/mic.svg" }).size(zpx(17.)).text_color(palette.text_on_accent))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                // While recording it sends, once it's been
                                // going long enough (send_recording checks).
                                if recording {
                                    this.send_recording(window, cx)
                                } else if can_send {
                                    this.send(window, cx)
                                } else {
                                    this.start_recording(cx)
                                }
                            })),
                    )
)
            .children(self.picker.as_ref().map(|(at, picker)| {
                gpui::deferred(
                    gpui::anchored()
                        .position(*at)
                        .anchor(gpui::Anchor::BottomRight)
                        .snap_to_window_with_margin(zpx(10.))
                        .child(picker.clone()),
                )
                .with_priority(1)
            }))
            .when(!self.encrypted, |d| {
                d.child(
                    h_flex()
                        .mt(zpx(8.))
                        .px(space(6.))
                        .gap(space(8.))
                        .font_family(palette.mono_font.clone())
                        .text_size(text(10.5))
                        .text_color(palette.text_tertiary)
                        .child(icon("icons/lock.svg").size(zpx(11.)).text_color(palette.idle))
                        .child("not encrypted · your homeserver can read this"),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use gpui::{ClipboardEntry, ClipboardItem, ClipboardString, ExternalPaths, Image, ImageFormat};

    use super::*;

    fn now() -> chrono::DateTime<chrono::Local> {
        chrono::Local.with_ymd_and_hms(2026, 11, 23, 17, 40, 5).unwrap()
    }

    fn picture() -> ClipboardEntry {
        ClipboardEntry::Image(Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]))
    }

    #[test]
    fn a_screenshot_is_sent_as_a_named_picture() {
        let item = ClipboardItem { entries: vec![picture()] };
        assert_eq!(pasted(&item, now()), vec![Outgoing::Bytes { bytes: vec![1, 2, 3], mime: "image/png", name: "pasted-20261123-174005.png".into() }]);
    }

    #[test]
    fn finder_files_win_over_their_icon_and_text_stays_text() {
        let file = ClipboardEntry::ExternalPaths(ExternalPaths(vec![std::path::PathBuf::from("/tmp/a.jpg")].into()));
        let name = ClipboardEntry::String(ClipboardString::new("a.jpg".into()));
        let item = ClipboardItem { entries: vec![name.clone(), picture(), file] };
        assert_eq!(pasted(&item, now()), vec![Outgoing::Path("/tmp/a.jpg".into())]);
        // Rich text with a rendering of itself: still text.
        let words = ClipboardEntry::String(ClipboardString::new("some words".into()));
        assert!(pasted(&ClipboardItem { entries: vec![words, picture()] }, now()).is_empty());
        // A browser's "copy image": the picture.
        let address = ClipboardEntry::String(ClipboardString::new("https://x.org/cat.png".into()));
        assert_eq!(pasted(&ClipboardItem { entries: vec![address, picture()] }, now()).len(), 1);
        assert!(pasted(&ClipboardItem { entries: vec![name] }, now()).is_empty());
    }
}
