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
    WeakEntity, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    h_flex,
    input::{InputEvent, MoveDown, MoveUp, Position, Textarea, TextareaState},
};
use horo_core::{
    dto::{SendOptions, StickerOptions},
    matrix::timeline,
};
use matrix_sdk::ruma::OwnedRoomId;

use crate::{
    backend::Backend,
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
    timeline: WeakEntity<Timeline>,
    input: Entity<TextareaState>,
    mode: Mode,
    error: Option<SharedString>,
    /// Said under the box when it isn't: the one case worth a line.
    encrypted: bool,
    store: Entity<Store>,
    /// The emoji picker, open over the box, and where it was opened.
    picker: Option<(gpui::Point<gpui::Pixels>, Entity<EmojiPicker>)>,
}

impl Composer {
    pub fn new(
        room_id: OwnedRoomId,
        room_name: &str,
        encrypted: bool,
        store: Entity<Store>,
        timeline: WeakEntity<Timeline>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Composer {
        let placeholder = format!("say something cute in {room_name}~");
        let input = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx);
            state.set_auto_grow(1, 10, cx);
            state.set_submit_on_enter(true, cx);
            state.set_placeholder(placeholder, window, cx);
            state
        });
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = event {
                this.send(window, cx);
            }
        })
        .detach();
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        Composer { room_id, timeline, input, mode: Mode::Write, error: None, encrypted, store, picker: None }
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
            let (state, room_id) = (backend.state.clone(), self.room_id.clone());
            async move { timeline::send_sticker(&*state.core().await?, &room_id, None, sticker).await }
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
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" && self.mode != Mode::Write {
            self.cancel(window, cx);
            cx.stop_propagation();
        }
    }

    /// The arrows on an empty field reach for a message. They're the
    /// field's own actions (moving the caret), caught on their way to it;
    /// anywhere else they go on to move the caret.
    fn recall(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) {
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
        let room_id = self.room_id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            for path in paths {
                let Ok(task) = this.update(cx, |_, cx| {
                    let backend = Backend::global(cx);
                    let (state, room_id, path) = (backend.state.clone(), room_id.clone(), path.to_string_lossy().into_owned());
                    backend.run(async move { timeline::send_attachment(&*state.core().await?, &room_id, None, &path, None).await })
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
            // Every emote we know; the backend puts in the ones the text uses.
            let emotes = packs::emote_refs(&self.store.read(cx).packs);
            async move {
                let core = state.core().await?;
                match mode {
                    Mode::Edit { event_id } => timeline::edit(&core, &room_id, None, &event_id, &body, true).await,
                    Mode::Write | Mode::Reply { .. } => {
                        let reply_to = match mode {
                            Mode::Reply { event_id, .. } => Some(event_id),
                            _ => None,
                        };
                        let options = SendOptions { body, markdown: true, reply_to, thread_root: None, msgtype: None, emotes };
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        let bar = match &self.mode {
            Mode::Write => None,
            Mode::Reply { who, preview, .. } => Some((palette.accent_tertiary, format!("replying to {who}"), preview.clone())),
            Mode::Edit { .. } => Some((palette.accent, "editing your message".to_owned(), "escape to cancel".to_owned())),
        };
        let can_send = !self.input.read(cx).value().trim().is_empty();
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
                    .border_1()
                    .border_color(palette.border_subtle)
                    .border_l_2()
                    .border_color(colour)
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
                    .capture_action(cx.listener(|this, _: &MoveUp, window, cx| this.recall(true, window, cx)))
                    .capture_action(cx.listener(|this, _: &MoveDown, window, cx| this.recall(false, window, cx)))
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
                    )
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
                            .when(!can_send, |d| d.opacity(0.45))
                            .when(can_send, |d| d.cursor_pointer())
                            .child(icon("icons/send.svg").size(px(17.)).text_color(palette.text_on_accent))
                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                    ),
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
