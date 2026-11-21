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
    WeakEntity, Window, div, px,
};
use gpui_component::{
    h_flex,
    input::{InputEvent, MoveDown, MoveUp, Position, Textarea, TextareaState},
};
use horo_core::{dto::SendOptions, matrix::timeline};
use matrix_sdk::ruma::OwnedRoomId;

use crate::{
    backend::Backend,
    theme::Palette,
    views::timeline::{Timeline, TimelineAction},
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
}

impl Composer {
    pub fn new(room_id: OwnedRoomId, room_name: &str, timeline: WeakEntity<Timeline>, window: &mut Window, cx: &mut Context<Self>) -> Composer {
        let placeholder = format!("message {room_name}");
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
        Composer { room_id, timeline, input, mode: Mode::Write, error: None }
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
            async move {
                let core = state.core().await?;
                match mode {
                    Mode::Edit { event_id } => timeline::edit(&core, &room_id, None, &event_id, &body, true).await,
                    Mode::Write | Mode::Reply { .. } => {
                        let reply_to = match mode {
                            Mode::Reply { event_id, .. } => Some(event_id),
                            _ => None,
                        };
                        let options = SendOptions { body, markdown: true, reply_to, thread_root: None, msgtype: None, emotes: Default::default() };
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
            Mode::Reply { who, preview, .. } => Some((palette.accent_secondary, format!("replying to {who}"), preview.clone())),
            Mode::Edit { .. } => Some((palette.accent, "editing your message".to_owned(), "escape to cancel".to_owned())),
        };
        div()
            .flex_none()
            .px_4()
            .pb_4()
            .pt_1()
            .children(self.error.clone().map(|e| div().pb_1().text_xs().text_color(palette.danger).child(e)))
            .children(bar.map(|(colour, title, detail)| {
                h_flex()
                    .mb_1()
                    .px_3()
                    .py_1()
                    .gap_2()
                    .border_l_2()
                    .border_color(colour)
                    .text_xs()
                    .child(div().flex_none().text_color(colour).child(title))
                    .child(div().flex_1().min_w_0().truncate().text_color(palette.text_tertiary).child(detail))
                    .child(
                        div()
                            .id("composer-cancel")
                            .flex_none()
                            .px_1()
                            .cursor_pointer()
                            .text_color(palette.text_secondary)
                            .hover(|d| d.text_color(palette.text))
                            .child("✕")
                            .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))),
                    )
            }))
            .child(
                h_flex()
                    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| this.on_key(event, window, cx)))
                    .capture_action(cx.listener(|this, _: &MoveUp, window, cx| this.recall(true, window, cx)))
                    .capture_action(cx.listener(|this, _: &MoveDown, window, cx| this.recall(false, window, cx)))
                    .rounded(palette.radius_lg)
                    .bg(palette.raised)
                    .border_1()
                    .border_color(match self.mode {
                        Mode::Edit { .. } => palette.accent,
                        _ => palette.border_subtle,
                    })
                    .px_2()
                    .py(px(2.))
                    .child(Textarea::new(&self.input).appearance(false).flex_1()),
            )
    }
}
