//! Signing in with a password. (SSO, QR login and registration follow.)

use gpui::{App, AppContext, Context, Entity, FontWeight, IntoElement, ParentElement, Render, Styled, Window, div, px};
use gpui_component::{
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    v_flex,
};

use crate::{
    store::{Phase, Store},
    theme::Palette,
};

pub struct Login {
    store: Entity<Store>,
    server: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
}

impl Login {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Login {
        let server = cx.new(|cx| InputState::new(window, cx).placeholder("homeserver, e.g. matrix.org"));
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("username"));
        let password = cx.new(|cx| InputState::new(window, cx).placeholder("password").masked(true));
        // Enter in any field signs in.
        for input in [&server, &username, &password] {
            cx.subscribe_in(input, window, |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.submit(cx);
                }
            })
            .detach();
        }
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        Login { store, server, username, password }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let value = |input: &Entity<InputState>, cx: &App| input.read(cx).value().trim().to_owned();
        let (server, username, password) = (value(&self.server, cx), value(&self.username, cx), self.password.read(cx).value().to_string());
        if server.is_empty() || username.is_empty() || password.is_empty() {
            return;
        }
        self.store.update(cx, |store, cx| store.sign_in(server, username, password, cx));
    }
}

impl Render for Login {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        let store = self.store.read(cx);
        let busy = matches!(store.phase, Phase::SigningIn);
        let error = store.error.clone();

        let label = |text: &'static str| div().text_sm().text_color(palette.text_secondary).child(text);
        div().size_full().flex().items_center().justify_center().bg(palette.app).child(
            v_flex()
                .w(px(360.))
                .gap_3()
                .p_6()
                .rounded(palette.radius_lg)
                .bg(palette.card)
                .border_1()
                .border_color(palette.border_subtle)
                .child(
                    div()
                        .font_family(palette.heading_font.clone())
                        .text_size(px(40.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(palette.text)
                        .child("horo"),
                )
                .child(div().text_sm().text_color(palette.text_tertiary).child("sign in with your Matrix account"))
                .child(label("homeserver"))
                .child(Input::new(&self.server))
                .child(label("username"))
                .child(Input::new(&self.username))
                .child(label("password"))
                .child(Input::new(&self.password).mask_toggle())
                .children(error.map(|e| div().text_sm().text_color(palette.danger).child(e)))
                .child(
                    Button::new("sign-in")
                        .primary()
                        .label(if busy { "signing in…" } else { "sign in" })
                        .loading(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                ),
        )
    }
}
