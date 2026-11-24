//! An invite, where the room would be, as InvitePane.tsx: who asked, and
//! whether to go in. Declining asks once more first (they'd have to invite
//! you again). Accepting a space joins its rooms too, the way joining a
//! Discord server does, and opens it in the rail.

use std::time::Duration;

use gpui::{
    AsyncApp, Context, Entity, FontWeight, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div,
    prelude::FluentBuilder,
};
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use horo_core::{dto::RoomMembership, matrix::rooms};
use matrix_sdk::ruma::OwnedRoomId;

use crate::theme::zpx;
use crate::{
    backend::Backend,
    store::Store,
    theme::{Palette, heading, space, text},
    views::{avatar::avatar, modal::accent_button},
};

pub struct InvitePane {
    store: Entity<Store>,
    pub room_id: String,
    busy: bool,
    /// "Decline" was clicked once; the button that does it is showing.
    asked: bool,
    error: Option<SharedString>,
}

impl InvitePane {
    pub fn new(store: Entity<Store>, room_id: String, cx: &mut Context<Self>) -> InvitePane {
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        InvitePane { store, room_id, busy: false, asked: false, error: None }
    }

    fn accept(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(room) = self.store.read(cx).rooms.iter().find(|r| r.id == self.room_id).cloned() else { return };
        let Ok(id) = OwnedRoomId::try_from(room.id.as_str()) else { return };
        self.busy = true;
        self.error = None;
        let is_space = room.is_space;
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, id) = (backend.state.clone(), id.clone());
            async move {
                let core = state.core().await?;
                if is_space {
                    rooms::join_space(&core, &id, &[]).await.map(|r| r.left_out)
                } else {
                    rooms::join(&core, id.as_str(), &[]).await.map(|_| 0)
                }
            }
        });
        let store = self.store.downgrade();
        cx.spawn(async move |this, cx| {
            let outcome = task.await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
            if outcome.is_ok() {
                let id = room.id.clone();
                // Sync says we're in a little after the join returns.
                wait_until(&store, cx, is_space, move |s| {
                    if is_space {
                        s.spaces.iter().any(|sp| sp.id == id)
                    } else {
                        s.rooms.iter().any(|r| r.id == id && r.membership == RoomMembership::Joined)
                    }
                })
                .await;
            }
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match outcome {
                    Ok(left_out) => {
                        if is_space {
                            let id = room.id.clone();
                            this.store.update(cx, |s, cx| {
                                s.close_room(&id, cx);
                                s.open_space(Some(id.clone()), cx);
                            });
                            if left_out > 0 {
                                let which = if left_out == 1 { "one room is".to_owned() } else { format!("{left_out} rooms are") };
                                let message = format!("you're in {}. {which} invite-only — ask someone there to let you in", room.name);
                                this.store.update(cx, |s, cx| s.show_banner(crate::store::Tone::Info, message, cx));
                            }
                        }
                        // A room: the list now says joined, and the shell
                        // swaps this pane for its timeline.
                    }
                    Err(e) => this.error = Some(format!("couldn't join: {e}").into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn decline(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Ok(id) = OwnedRoomId::try_from(self.room_id.as_str()) else { return };
        self.busy = true;
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { rooms::leave(&*state.core().await?, &id).await }
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
                        let id = this.room_id.clone();
                        this.store.update(cx, |s, cx| s.close_room(&id, cx));
                    }
                    Some(e) => {
                        this.asked = false;
                        this.error = Some(format!("couldn't decline: {e}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

/// Wait (five seconds at most) for the store to show what was just joined.
async fn wait_until(store: &WeakEntity<Store>, cx: &mut AsyncApp, spaces: bool, there: impl Fn(&Store) -> bool) {
    for _ in 0..20 {
        let Ok(done) = store.update(cx, |s, cx| {
            if spaces && !there(s) {
                s.refresh_spaces_now(cx);
            }
            there(s)
        }) else {
            return;
        };
        if done {
            return;
        }
        cx.background_executor().timer(Duration::from_millis(250)).await;
    }
}

impl Render for InvitePane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        let Some(room) = self.store.read(cx).rooms.iter().find(|r| r.id == self.room_id).cloned() else { return div().into_any_element() };
        // For an invite, the room's latest event is the invite itself.
        let inviter = room.latest.as_ref().map(|l| {
            l.sender_name.clone().unwrap_or_else(|| l.sender.trim_start_matches('@').split(':').next().unwrap_or(&l.sender).to_owned())
        });
        let kind = if room.is_space {
            "space"
        } else if room.is_direct {
            "chat"
        } else {
            "room"
        };
        let line = match inviter {
            Some(who) => format!("{who} invited you to this {kind}"),
            None => format!("you're invited to this {kind}"),
        };
        let face = avatar(&room.id, &room.name, room.avatar_url.as_deref(), zpx(76.), cx);
        let busy = self.busy;

        let buttons = if self.asked {
            v_flex()
                .items_center()
                .gap(space(10.))
                .child(
                    div()
                        .max_w(zpx(340.))
                        .text_size(text(12.5))
                        .line_height(gpui::relative(1.5))
                        .text_color(palette.text_secondary)
                        .child(format!("decline the invite? you won't be in {}, and they'd have to invite you again.", room.name)),
                )
                .child(
                    h_flex()
                        .gap(space(8.))
                        .child(Button::new("invite-keep").ghost().small().label("keep it").on_click(cx.listener(|this, _, _, cx| {
                            this.asked = false;
                            cx.notify();
                        })))
                        .child(
                            Button::new("invite-decline-go")
                                .danger()
                                .small()
                                .label(if busy { "declining…" } else { "decline" })
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.decline(cx))),
                        ),
                )
        } else {
            v_flex().child(
                h_flex()
                    .mt(zpx(6.))
                    .gap(space(8.))
                    .child(Button::new("invite-decline").ghost().label("decline").disabled(busy).on_click(cx.listener(|this, _, _, cx| {
                        this.asked = true;
                        cx.notify();
                    })))
                    .child(accent_button("invite-join", if busy { "joining…" } else { "join" }, !busy, false, &palette).on_click(cx.listener(|this, _, _, cx| this.accept(cx)))),
            )
        };

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .justify_center()
            .gap(space(14.))
            .p(space(24.))
            .bg(palette.app)
            .child(face)
            .child(
                v_flex()
                    .items_center()
                    .child(div().font_family(palette.heading_font.clone()).font_weight(FontWeight::EXTRA_BOLD).text_size(heading(20.)).child(room.name.clone()))
                    .child(div().mt(zpx(4.)).text_size(text(13.)).text_color(palette.text_secondary).child(line))
                    .when(room.is_space, |d| {
                        d.child(div().mt(zpx(6.)).max_w(zpx(320.)).text_center().text_size(text(12.)).text_color(palette.text_tertiary).child("joining it puts you in its rooms too."))
                    }),
            )
            .child(buttons)
            .children(self.error.clone().map(|e| div().text_size(text(12.5)).text_color(palette.danger).child(e)))
            .into_any_element()
    }
}
