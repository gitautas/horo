//! Writing to a room: a growing text field under the timeline. Enter sends,
//! Shift+Enter starts a new line. (Replies, edits, mentions, emoji and pack
//! autocomplete, attachments and stickers follow.)

use gpui::{AppContext, Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled, Window, div, px};
use gpui_component::{
    h_flex,
    input::{InputEvent, Textarea, TextareaState},
};
use horo_core::{dto::SendOptions, matrix::timeline};
use matrix_sdk::ruma::OwnedRoomId;

use crate::{backend::Backend, theme::Palette};

pub struct Composer {
    room_id: OwnedRoomId,
    input: Entity<TextareaState>,
    error: Option<SharedString>,
}

impl Composer {
    pub fn new(room_id: OwnedRoomId, room_name: &str, window: &mut Window, cx: &mut Context<Self>) -> Composer {
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
        Composer { room_id, input, error: None }
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.input.read(cx).value().trim().to_owned();
        if body.is_empty() {
            return;
        }
        self.input.update(cx, |state, cx| state.set_value("", window, cx));
        self.error = None;
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, room_id) = (backend.state.clone(), self.room_id.clone());
            let options = SendOptions { body: body.clone(), markdown: false, reply_to: None, thread_root: None, msgtype: None, emotes: Default::default() };
            async move { timeline::send(&*state.core().await?, &room_id, options).await }
        });
        // The message shows in the timeline as soon as the SDK queues it;
        // only a failure needs telling here, with the text given back.
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
        div()
            .flex_none()
            .px_4()
            .pb_4()
            .pt_1()
            .children(self.error.clone().map(|e| div().pb_1().text_xs().text_color(palette.danger).child(e)))
            .child(
                h_flex()
                    .rounded(palette.radius_lg)
                    .bg(palette.raised)
                    .border_1()
                    .border_color(palette.border_subtle)
                    .px_2()
                    .py(px(2.))
                    .child(Textarea::new(&self.input).appearance(false).flex_1()),
            )
    }
}
