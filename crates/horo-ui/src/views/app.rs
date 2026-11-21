//! The window's root: login or the shell, by where sign-in stands.

use gpui::{AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window, div, px};

use crate::{
    store::{Phase, Store},
    theme::Palette,
    views::{login::Login, shell::Shell},
};

pub struct AppView {
    store: Entity<Store>,
    login: Option<Entity<Login>>,
    shell: Option<Entity<Shell>>,
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> AppView {
        let store = cx.new(Store::new);
        cx.observe_in(&store, window, |this, _, window, cx| this.sync_views(window, cx)).detach();
        // "Don't notify for the room I'm reading" needs to know whether
        // anyone is reading.
        cx.observe_window_activation(window, |this, window, cx| {
            let active = window.is_window_active();
            this.store.update(cx, |s, _| s.set_window_active(active));
        })
        .detach();
        AppView { store, login: None, shell: None }
    }

    /// Build the view the phase needs, drop the one it doesn't.
    fn sync_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let signed_in = matches!(self.store.read(cx).phase, Phase::SignedIn(_));
        if signed_in {
            self.login = None;
            if self.shell.is_none() {
                let store = self.store.clone();
                self.shell = Some(cx.new(|cx| Shell::new(store, window, cx)));
            }
        } else if !matches!(self.store.read(cx).phase, Phase::Restoring) {
            self.shell = None;
            if self.login.is_none() {
                let store = self.store.clone();
                self.login = Some(cx.new(|cx| Login::new(store, window, cx)));
            }
        }
        cx.notify();
    }
}

impl Render for AppView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        // Any touch of the window counts as being here, for presence.
        let root = div()
            .size_full()
            .bg(palette.app)
            .text_color(palette.text)
            .font_family(palette.body_font.clone())
            .on_mouse_move(cx.listener(|this, _: &gpui::MouseMoveEvent, _, cx| this.store.update(cx, |s, cx| s.activity(cx))))
            .capture_any_mouse_down(cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| this.store.update(cx, |s, cx| s.activity(cx))))
            .capture_key_down(cx.listener(|this, _: &gpui::KeyDownEvent, _, cx| this.store.update(cx, |s, cx| s.activity(cx))))
            .on_scroll(cx.listener(|this, _: &gpui::ScrollEvent, _, cx| this.store.update(cx, |s, cx| s.activity(cx))));
        if let Some(shell) = &self.shell {
            root.child(shell.clone())
        } else if let Some(login) = &self.login {
            root.child(login.clone())
        } else {
            root.flex().items_center().justify_center().child(
                div().font_family(palette.heading_font.clone()).text_size(px(40.)).text_color(palette.text_tertiary).child("horo"),
            )
        }
    }
}
