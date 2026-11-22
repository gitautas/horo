//! Inviting someone to a space, as SpaceDialogs.tsx's InviteToSpace:
//! type a name or a full `@someone:server`, pick from what the user
//! directory suggests (people already in or invited are marked so, not
//! offered), and invite. Whether you may is asked first, so someone without
//! the power finds out before typing. Opened from a space's menu in the
//! rail.

use std::{collections::HashMap, time::Duration};

use gpui::{
    AppContext as _, Context, Entity, EventEmitter, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Task, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    h_flex,
    input::{Input, InputEvent, InputState, MoveDown, MoveUp},
    v_flex,
};
use horo_core::matrix::{
    directory::{self, UserSuggestion},
    rooms,
};
use matrix_sdk::ruma::OwnedRoomId;

use crate::{
    backend::Backend,
    theme::{Palette, corner, space, text},
    views::{
        avatar::avatar,
        explore::Closed,
        modal::{accent_button, label, modal, well},
    },
};

const SEARCH_DELAY: Duration = Duration::from_millis(300);

/// `@name:server`, loosely: the server has the final say.
fn plausible(user_id: &str) -> bool {
    let Some(rest) = user_id.strip_prefix('@') else { return false };
    match rest.split_once(':') {
        Some((name, server)) => !name.is_empty() && !server.is_empty() && !user_id.contains(char::is_whitespace),
        None => false,
    }
}

pub struct InviteToSpace {
    space_id: String,
    name: String,
    focus: FocusHandle,
    who: Entity<InputState>,
    suggestions: Vec<UserSuggestion>,
    highlight: usize,
    /// Who's in or invited already, by user id: `join` or `invite`.
    members: HashMap<String, String>,
    /// Unknown counts as allowed: the server decides.
    can_invite: bool,
    /// A suggestion just picked, so its own text doesn't search again.
    picked: Option<String>,
    search: Option<Task<()>>,
    /// A value to put in the field on the next render (picking needs the
    /// window, which an input event doesn't hand over).
    pending_value: Option<String>,
    busy: bool,
    error: Option<SharedString>,
    done: Option<SharedString>,
}

impl EventEmitter<Closed> for InviteToSpace {}

impl InviteToSpace {
    pub fn new(space_id: String, name: String, window: &mut Window, cx: &mut Context<Self>) -> InviteToSpace {
        let who = cx.new(|cx| InputState::new(window, cx).placeholder("a name, or @someone:server.tld"));
        cx.subscribe_in(&who, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::Change => this.typed(cx),
            InputEvent::PressEnter { .. } => this.enter(cx),
            _ => {}
        })
        .detach();
        who.update(cx, |s, cx| s.focus(window, cx));

        if let Ok(room) = OwnedRoomId::try_from(space_id.as_str()) {
            let backend = Backend::global(cx);
            let task = backend.run({
                let state = backend.state.clone();
                async move {
                    let core = state.core().await?;
                    let can = rooms::permissions(&core, &room).await.map(|p| p.can_invite).unwrap_or(true);
                    let members = rooms::members(&core, &room).await.unwrap_or_default();
                    Ok::<_, horo_core::Error>((can, members))
                }
            });
            cx.spawn(async move |this, cx| {
                if let Ok(Ok((can, members))) = task.await {
                    let _ = this.update(cx, |this, cx| {
                        this.can_invite = can;
                        this.members = members.into_iter().map(|m| (m.user_id, m.membership)).collect();
                        cx.notify();
                    });
                }
            })
            .detach();
        }

        InviteToSpace {
            space_id,
            name,
            focus: cx.focus_handle(),
            who,
            suggestions: Vec::new(),
            highlight: 0,
            members: HashMap::new(),
            can_invite: true,
            picked: None,
            search: None,
            pending_value: None,
            busy: false,
            error: None,
            done: None,
        }
    }

    fn term(&self, cx: &Context<Self>) -> String {
        self.who.read(cx).value().trim().to_owned()
    }

    /// Ask the user directory, after typing pauses; a directory that won't
    /// answer just means no suggestions.
    fn typed(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.done = None;
        let term = self.term(cx);
        if term.chars().count() < 2 || self.picked.as_deref() == Some(term.as_str()) {
            self.suggestions.clear();
            self.search = None;
            cx.notify();
            return;
        }
        self.search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DELAY).await;
            let Ok(task) = this.update(cx, |_, cx| {
                let backend = Backend::global(cx);
                let state = backend.state.clone();
                let term = term.clone();
                backend.run(async move { directory::search_users(&*state.core().await?, &term).await })
            }) else {
                return;
            };
            let found = match task.await {
                Ok(Ok(found)) => found,
                _ => Vec::new(),
            };
            let _ = this.update(cx, |this, cx| {
                // Start on the first who can be invited, so Enter picks them.
                this.highlight = found.iter().position(|s| !this.members.contains_key(&s.user_id)).unwrap_or(0);
                this.suggestions = found;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn pick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chosen) = self.suggestions.get(index).cloned() else { return };
        if self.members.contains_key(&chosen.user_id) {
            return;
        }
        self.picked = Some(chosen.user_id.clone());
        self.suggestions.clear();
        self.who.update(cx, |s, cx| s.set_value(chosen.user_id, window, cx));
        cx.notify();
    }

    /// Enter picks the highlighted suggestion first; Enter again invites.
    fn enter(&mut self, cx: &mut Context<Self>) {
        let term = self.term(cx);
        let chosen = self.suggestions.get(self.highlight).map(|s| s.user_id.clone());
        match chosen {
            Some(user) if user != term && !self.members.contains_key(&user) => {
                self.picked = Some(user.clone());
                self.suggestions.clear();
                // `set_value` needs a window; the next render puts it in.
                self.pending_value = Some(user);
                cx.notify();
            }
            _ => self.invite(cx),
        }
    }

    fn invite(&mut self, cx: &mut Context<Self>) {
        let user = self.term(cx);
        if !plausible(&user) || self.busy || !self.can_invite {
            return;
        }
        let Ok(room) = OwnedRoomId::try_from(self.space_id.as_str()) else { return };
        self.busy = true;
        self.error = None;
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, user) = (backend.state.clone(), user.clone());
            async move { rooms::invite(&*state.core().await?, &room, &user).await }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match error {
                    None => {
                        this.members.insert(user.clone(), "invite".into());
                        this.done = Some(format!("invited {user} to {}", this.name).into());
                    }
                    Some(e) => this.error = Some(e.into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn move_highlight(&mut self, up: bool, cx: &mut Context<Self>) {
        let n = self.suggestions.len();
        if n == 0 {
            return;
        }
        self.highlight = if up { (self.highlight + n - 1) % n } else { (self.highlight + 1) % n };
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for InviteToSpace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(value) = self.pending_value.take() {
            self.who.update(cx, |s, cx| s.set_value(value, window, cx));
        }
        let palette = cx.global::<Palette>().clone();
        let term = self.term(cx);

        let mut list = v_flex().mt(px(6.)).p(space(4.)).gap(space(2.)).rounded(corner(14.)).bg(palette.inset).border_1().border_color(palette.border_subtle);
        for (i, s) in self.suggestions.clone().into_iter().enumerate() {
            let name = s.display_name.clone().unwrap_or_else(|| s.user_id.clone());
            let taken = match self.members.get(&s.user_id).map(String::as_str) {
                Some("join") => Some("already in"),
                Some("invite") => Some("invited"),
                _ => None,
            };
            let face = avatar(&s.user_id, &name, s.avatar_url.as_deref(), px(28.), cx);
            list = list.child(
                h_flex()
                    .id(("suggestion", i))
                    .gap(space(10.))
                    .px(space(8.))
                    .py(space(6.))
                    .rounded(corner(10.))
                    .when(taken.is_none(), |d| d.cursor_pointer())
                    .when(i == self.highlight, |d| d.bg(Hsla { a: 0.07, ..gpui::white() }))
                    .when(taken.is_some(), |d| d.opacity(0.55))
                    .child(face)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().font_weight(FontWeight::BOLD).text_size(text(13.)).child(name))
                            .child(div().truncate().font_family(palette.mono_font.clone()).text_size(text(10.5)).text_color(palette.text_tertiary).child(s.user_id.clone())),
                    )
                    .children(taken.map(|t| div().text_size(text(11.)).text_color(palette.text_tertiary).child(t)))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick(i, window, cx))),
            );
        }
        let suggestions = (!self.suggestions.is_empty()).then_some(list);

        let (line, colour) = if let Some(e) = &self.error {
            (e.to_string(), palette.danger)
        } else if let Some(d) = &self.done {
            (d.to_string(), palette.online)
        } else if !self.can_invite {
            (format!("you can't invite people to {} — that takes more power than you have there. ask one of its admins.", self.name), palette.danger)
        } else {
            ("they get an invite to the space. joining it puts them in every room its members can get into.".to_owned(), palette.text_tertiary)
        };
        let ready = plausible(&term) && !self.busy && self.can_invite;

        let body = v_flex()
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_highlight(true, cx)))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_highlight(false, cx)))
            .child(label(&palette, "WHO"))
            .child(well(&palette).child(div().flex_1().child(Input::new(&self.who).appearance(false))))
            .children(suggestions)
            .child(div().mt(px(8.)).text_size(text(12.)).line_height(gpui::relative(1.45)).text_color(colour).child(line))
            .child(
                h_flex().justify_end().mt(px(20.)).child(
                    accent_button("invite-go", if self.busy { "inviting…" } else { "invite" }, ready, false, &palette)
                        .on_click(cx.listener(|this, _, _, cx| this.invite(cx))),
                ),
            )
            .into_any_element();
        let this = cx.weak_entity();
        modal(
            "invite-to-space",
            format!("invite to {}", self.name),
            440.,
            &self.focus,
            &palette,
            move |_, cx| {
                let _ = this.update(cx, |_, cx| cx.emit(Closed));
            },
            body,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_full_user_ids_are_invitable() {
        assert!(plausible("@cat:example.org"));
        assert!(!plausible("cat"));
        assert!(!plausible("@cat"));
        assert!(!plausible("@:example.org"));
        assert!(!plausible("@c at:example.org"));
    }
}
