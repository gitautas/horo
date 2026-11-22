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
    AppContext, Context, Entity, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    StyledImage, WeakEntity, Window, div, img, prelude::FluentBuilder, px,
};
use gpui_component::{
    h_flex, v_flex,
    input::{IndentInline, InputEvent, MoveDown, MoveUp, Position, Textarea, TextareaState},
};
use horo_core::{
    dto::{SendOptions, StickerOptions},
    matrix::timeline,
};
use matrix_sdk::ruma::OwnedRoomId;

use crate::{
    backend::Backend,
    media::Media,
    packs::{self, Picked},
    store::Store,
    theme::{Palette, corner, space, text},
    views::{
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
}

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
    matches: Vec<horo_core::matrix::packs::PackImageDto>,
    active: usize,
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
                InputEvent::Change => this.update_completion(cx),
                _ => {}
            }
        })
        .detach();
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        Composer { room_id, thread_root, timeline, input, mode: Mode::Write, error: None, encrypted, store, picker: None, completing: None, recording: None, levels: Default::default() }
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
        self.completing = packs::typing_shortcode(&text, caret).and_then(|(query, start)| {
            let matches = packs::match_emotes(&packs::emote_lookup(&self.store.read(cx).packs), &query, COMPLETIONS);
            (!matches.is_empty()).then_some(Completing { start, matches, active: 0 })
        });
        cx.notify();
    }

    /// Put in the chosen match (the highlighted one unless `pick` says),
    /// replacing the `:code` typed so far.
    fn complete(&mut self, pick: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(completing) = self.completing.take() else { return };
        let Some(image) = completing.matches.get(pick.unwrap_or(completing.active)) else { return };
        let text = format!(":{}: ", image.shortcode);
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
        let mut meter = h_flex().flex_1().min_w_0().h(px(24.)).items_center().justify_end().gap(px(2.));
        for _ in self.levels.len()..METER_BARS {
            meter = meter.child(div().w(px(3.)).h(px(3.)).rounded_full().bg(palette.border));
        }
        for level in &self.levels {
            meter = meter.child(div().w(px(3.)).h(px(3. + 19. * level)).rounded_full().bg(palette.danger));
        }
        h_flex()
            .flex_1()
            .min_w_0()
            .gap(space(10.))
            .child(
                div()
                    .id("recording-cancel")
                    .size(px(38.))
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
            .child(div().flex_none().size(px(9.)).rounded_full().bg(palette.danger))
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

    /// Pick files and send each as an attachment.
    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("attach".into()) });
        let (room_id, thread_root) = (self.room_id.clone(), self.thread_root.clone());
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            for path in paths {
                let Ok(task) = this.update(cx, |_, cx| {
                    let backend = Backend::global(cx);
                    let (state, room_id, path) = (backend.state.clone(), room_id.clone(), path.to_string_lossy().into_owned());
                    let thread = thread_root.clone();
                    backend.run(async move { timeline::send_attachment(&*state.core().await?, &room_id, thread.as_deref(), &path, None).await })
                }) else {
                    return;
                };
                let error = match task.await {
                    Ok(Ok(())) => continue,
                    Ok(Err(e)) => e.to_string(),
                    Err(e) => e.to_string(),
                };
                let _ = this.update(cx, |composer, cx| {
                    composer.error = Some(format!("couldn't send {}: {error}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).into());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.input.read(cx).value().trim().to_owned();
        if body.is_empty() {
            return;
        }
        let mode = std::mem::replace(&mut self.mode, Mode::Write);
        self.input.update(cx, |state, cx| state.set_value("", window, cx));
        self.error = None;
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, room_id, body, mode) = (backend.state.clone(), self.room_id.clone(), body.clone(), mode.clone());
            let thread_root = self.thread_root.clone();
            // Every emote we know; the backend puts in the ones the text uses.
            let emotes = packs::emote_refs(&self.store.read(cx).packs);
            async move {
                let core = state.core().await?;
                match mode {
                    Mode::Edit { event_id } => timeline::edit(&core, &room_id, thread_root.as_deref(), &event_id, &body, true).await,
                    Mode::Write | Mode::Reply { .. } => {
                        let reply_to = match mode {
                            Mode::Reply { event_id, .. } => Some(event_id),
                            _ => None,
                        };
                        let options = SendOptions { body, markdown: true, reply_to, thread_root, msgtype: None, emotes };
                        timeline::send(&core, &room_id, options).await
                    }
                }
            }
        });
        // The message shows in the timeline as soon as the SDK queues it;
        // only a failure needs telling here, with the text and the mode
        // given back.
        cx.spawn_in(window, async move |this, cx| {
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
                    }
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }
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
        let can_send = !self.input.read(cx).value().trim().is_empty();
        let recording = self.recording.is_some();
        let sendable = self.recording.as_ref().is_some_and(Recording::sendable);
        let strip = recording.then(|| self.recording_strip(&palette, cx));
        let menu = self.completing.as_ref().map(|completing| {
            let mut list = v_flex()
                .id("shortcodes")
                .occlude()
                .absolute()
                .left(px(10.))
                .bottom(gpui::relative(1.))
                .mb(px(6.))
                .min_w(px(220.))
                .max_w(px(320.))
                .p(space(5.))
                .rounded(corner(16.))
                .bg(palette.raised)
                .border_1()
                .border_color(palette.border)
                .shadow_lg();
            for (i, image) in completing.matches.iter().enumerate() {
                let on = i == completing.active;
                let picture = match Media::image(&image.url, Some((48, 48)), cx) {
                    Some(picture) => img(picture).size(px(22.)).object_fit(gpui::ObjectFit::Contain).into_any_element(),
                    None => div().size(px(22.)).into_any_element(),
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
                        .child(
                            div()
                                .truncate()
                                .font_family(palette.mono_font.clone())
                                .text_size(text(12.))
                                .text_color(if on { palette.text } else { palette.text_secondary })
                                .child(format!(":{}:", image.shortcode)),
                        )
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
                    .mb(px(8.))
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
                    .child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(colour))
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
                                .size(px(38.))
                                .flex_none()
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("attach a file").build(window, cx))
                                .child(icon("icons/circle-plus.svg").size(px(22.)).text_color(palette.text_tertiary))
                                .on_click(cx.listener(|this, _, window, cx| this.attach(window, cx))),
                        )
                        .child(div().flex_1().min_w_0().py(space(4.)).text_size(px(14.5)).child(Textarea::new(&self.input).appearance(false)))
                        .child(
                            div()
                                .id("composer-emoji")
                                .size(px(38.))
                                .flex_none()
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("emoji").build(window, cx))
                                .child(icon("icons/smile.svg").size(px(20.)).text_color(if self.picker.is_some() { palette.accent } else { palette.text_tertiary }))
                                // On press, so the picker's own "pressed outside"
                                // doesn't close it and this reopen it.
                                .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    // Above the button, clear of the box.
                                    this.open_picker(gpui::point(event.position.x, event.position.y - px(28.)), window, cx)
                                })),
                        ),
                    })
                    .child(
                        div()
                            .id("composer-send")
                            .size(px(38.))
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
                            .child(icon(if can_send || recording { "icons/send.svg" } else { "icons/mic.svg" }).size(px(17.)).text_color(palette.text_on_accent))
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
                        .snap_to_window_with_margin(px(10.))
                        .child(picker.clone()),
                )
                .with_priority(1)
            }))
            .when(!self.encrypted, |d| {
                d.child(
                    h_flex()
                        .mt(px(8.))
                        .px(space(6.))
                        .gap(space(8.))
                        .font_family(palette.mono_font.clone())
                        .text_size(text(10.5))
                        .text_color(palette.text_tertiary)
                        .child(icon("icons/lock.svg").size(px(11.)).text_color(palette.idle))
                        .child("not encrypted · your homeserver can read this"),
                )
            })
    }
}
