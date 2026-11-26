//! A room's settings, as RoomSettings.tsx: rename it and set its topic
//! (only the fields this account may change), and leave it, which sits at
//! the foot behind its own confirmation, since leaving a private room
//! can't be undone from this side. Opened from the pencil in the room
//! info panel.

use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FocusHandle, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    v_flex,
};
use horo_core::{
    dto::{RoomMembership, RoomSummary},
    matrix::rooms::{self, RoomPermissions},
};
use matrix_sdk::ruma::OwnedRoomId;

use crate::theme::zpx;
use crate::{
    backend::Backend,
    store::Store,
    theme::{Palette, space, text},
    views::{
        explore::Closed,
        modal::{accent_button, modal, well},
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Confirm {
    Leave,
    Delete,
}

pub struct RoomSettings {
    store: Entity<Store>,
    pub room_id: String,
    focus: FocusHandle,
    /// `None` until read; the fields show only for what may be changed.
    permissions: Option<RoomPermissions>,
    name: Entity<InputState>,
    topic: Entity<TextareaState>,
    saving: bool,
    /// "Leave…" or "Delete…" was clicked once; its confirmation is showing.
    asked: Option<Confirm>,
    acting: bool,
    error: Option<SharedString>,
}

impl EventEmitter<Closed> for RoomSettings {}

impl RoomSettings {
    pub fn new(store: Entity<Store>, room_id: String, window: &mut Window, cx: &mut Context<Self>) -> RoomSettings {
        let room = store.read(cx).rooms.iter().find(|r| r.id == room_id).cloned();
        let (name_now, topic_now) = room.map(|r| (r.name, r.topic.unwrap_or_default())).unwrap_or_default();
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("room name").default_value(name_now));
        let topic = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx);
            state.set_auto_grow(2, 5, cx);
            state.set_placeholder("what's it for?", window, cx);
            state.set_value(topic_now, window, cx);
            state
        });
        cx.subscribe(&name, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        cx.subscribe(&topic, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        cx.observe(&store, |_, _, cx| cx.notify()).detach();

        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let settings = RoomSettings {
            store,
            room_id,
            focus,
            permissions: None,
            name,
            topic,
            saving: false,
            asked: None,
            acting: false,
            error: None,
        };
        settings.load_permissions(cx);
        settings
    }

    fn load_permissions(&self, cx: &mut Context<Self>) {
        let Ok(room) = OwnedRoomId::try_from(self.room_id.as_str()) else { return };
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { rooms::permissions(&*state.core().await?, &room).await }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(permissions)) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.permissions = Some(permissions);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn room(&self, cx: &Context<Self>) -> Option<RoomSummary> {
        self.store.read(cx).rooms.iter().find(|r| r.id == self.room_id).cloned()
    }

    /// What would change: a new name and topic, where allowed and different.
    fn changes(&self, cx: &Context<Self>) -> (Option<String>, Option<String>) {
        let (Some(permissions), Some(room)) = (&self.permissions, self.room(cx)) else { return (None, None) };
        let name = self.name.read(cx).value().trim().to_owned();
        let topic = self.topic.read(cx).value().trim().to_owned();
        (
            (permissions.can_rename && !name.is_empty() && name != room.name).then_some(name),
            (permissions.can_set_topic && topic != room.topic.unwrap_or_default()).then_some(topic),
        )
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let (name, topic) = self.changes(cx);
        if (name.is_none() && topic.is_none()) || self.saving {
            return;
        }
        let Ok(room) = OwnedRoomId::try_from(self.room_id.as_str()) else { return };
        self.saving = true;
        self.error = None;
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { rooms::update(&*state.core().await?, &room, name, topic).await }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                this.error = error.map(|e| format!("couldn't save: {e}").into());
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Open at the "leave?" question (from a room's right-click menu).
    pub fn ask_to_leave(&mut self, cx: &mut Context<Self>) {
        self.asked = Some(Confirm::Leave);
        cx.notify();
    }

    /// Open at the "delete?" question (from a room's right-click menu).
    pub fn ask_to_delete(&mut self, cx: &mut Context<Self>) {
        self.asked = Some(Confirm::Delete);
        cx.notify();
    }

    fn leave(&mut self, cx: &mut Context<Self>) {
        if self.acting {
            return;
        }
        let Ok(room) = OwnedRoomId::try_from(self.room_id.as_str()) else { return };
        self.acting = true;
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { rooms::leave(&*state.core().await?, &room).await }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            let _ = this.update(cx, |this, cx| match error {
                // Close first: the room is about to vanish from under it.
                None => {
                    cx.emit(Closed);
                    let room_id = this.room_id.clone();
                    this.store.update(cx, |s, cx| s.close_room(&room_id, cx));
                }
                Some(e) => {
                    this.acting = false;
                    this.asked = None;
                    this.error = Some(format!("couldn't leave: {e}").into());
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        if self.acting {
            return;
        }
        let Ok(room) = OwnedRoomId::try_from(self.room_id.as_str()) else { return };
        self.acting = true;
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { rooms::delete(&*state.core().await?, &room).await }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            let _ = this.update(cx, |this, cx| match error {
                None => {
                    cx.emit(Closed);
                    let room_id = this.room_id.clone();
                    this.store.update(cx, |s, cx| {
                        s.close_room(&room_id, cx);
                        // It just came out of its spaces, which only re-read on a slow poll.
                        s.refresh_spaces_now(cx);
                    });
                }
                Some(e) => {
                    this.acting = false;
                    this.asked = None;
                    this.error = Some(format!("couldn't delete: {e}").into());
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn editor(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(permissions) = self.permissions.clone() else {
            return div().text_size(text(12.5)).text_color(palette.text_tertiary).child("checking what you can change…").into_any_element();
        };
        if !permissions.can_rename && !permissions.can_set_topic {
            return div()
                .text_size(text(12.5))
                .line_height(gpui::relative(1.5))
                .text_color(palette.text_tertiary)
                .child("you can't rename this one — that needs more power than you have here.")
                .into_any_element();
        }
        let (name, topic) = self.changes(cx);
        let dirty = name.is_some() || topic.is_some();
        v_flex()
            .gap(zpx(8.))
            .when(permissions.can_rename, |d| d.child(well(palette).child(div().flex_1().child(Input::new(&self.name).appearance(false)))))
            .when(permissions.can_set_topic, |d| d.child(well(palette).child(div().flex_1().child(Textarea::new(&self.topic).appearance(false)))))
            .when(dirty, |d| {
                d.child(
                    h_flex().justify_end().child(
                        accent_button("room-save", if self.saving { "saving…" } else { "save" }, !self.saving, true, palette)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
                )
            })
            .into_any_element()
    }

    fn leave_part(&mut self, room: &RoomSummary, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        // An invite not taken up is declined, and a DM is a person rather
        // than a place: the same call underneath, a different sentence.
        let invited = room.membership == RoomMembership::Invited;
        let verb = if invited {
            "decline invite"
        } else if room.is_direct {
            "leave this chat"
        } else {
            "leave room"
        };
        let can_delete = !invited && !room.is_direct && self.permissions.as_ref().is_some_and(|p| p.can_delete);
        let body = match self.asked {
            Some(Confirm::Leave) => {
                let question = if invited {
                    format!("decline the invite to {}?", room.name)
                } else {
                    format!("leave {}? it drops off your list, and you'd need an invite to get back into a private room.", room.name)
                };
                v_flex()
                    .child(div().mb(zpx(10.)).text_size(text(12.5)).line_height(gpui::relative(1.55)).text_color(palette.text_secondary).child(question))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap(space(8.))
                            .child(Button::new("leave-stay").ghost().small().label("stay").on_click(cx.listener(|this, _, _, cx| {
                                this.asked = None;
                                cx.notify();
                            })))
                            .child(
                                Button::new("leave-go")
                                    .danger()
                                    .small()
                                    .label(if self.acting { "leaving…" } else { verb })
                                    .disabled(self.acting)
                                    .on_click(cx.listener(|this, _, _, cx| this.leave(cx))),
                            ),
                    )
                    .into_any_element()
            }
            Some(Confirm::Delete) => {
                let question = format!(
                    "delete {}? everyone is removed, it comes out of its spaces, and nobody can get back in. this can't be undone. the messages stay on the servers that already have them — matrix has no way to erase those.",
                    room.name
                );
                v_flex()
                    .child(div().mb(zpx(10.)).text_size(text(12.5)).line_height(gpui::relative(1.55)).text_color(palette.text_secondary).child(question))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap(space(8.))
                            .child(Button::new("delete-keep").ghost().small().label("keep it").on_click(cx.listener(|this, _, _, cx| {
                                this.asked = None;
                                cx.notify();
                            })))
                            .child(
                                Button::new("delete-go")
                                    .danger()
                                    .small()
                                    .label(if self.acting { "deleting…" } else { "delete room" })
                                    .disabled(self.acting)
                                    .on_click(cx.listener(|this, _, _, cx| this.delete(cx))),
                            ),
                    )
                    .into_any_element()
            }
            None => h_flex()
                .justify_between()
                .child(
                    div()
                        .id("leave-ask")
                        .cursor_pointer()
                        .text_size(text(12.5))
                        .text_color(palette.danger)
                        .child(format!("{verb}…"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.asked = Some(Confirm::Leave);
                            cx.notify();
                        })),
                )
                .when(can_delete, |row| {
                    row.child(
                        div()
                            .id("delete-ask")
                            .cursor_pointer()
                            .text_size(text(12.5))
                            .text_color(palette.danger)
                            .child("delete room…")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.asked = Some(Confirm::Delete);
                                cx.notify();
                            })),
                    )
                })
                .into_any_element(),
        };
        div().mt(zpx(20.)).pt(space(14.)).border_t_1().border_color(palette.border_subtle).child(body).into_any_element()
    }
}

impl Render for RoomSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        let Some(room) = self.room(cx) else { return div().into_any_element() };
        let editor = self.editor(&palette, cx);
        let leave = self.leave_part(&room, &palette, cx);
        let body = v_flex()
            .child(editor)
            .children(self.error.clone().map(|e| div().mt(zpx(10.)).text_size(text(12.5)).text_color(palette.danger).child(e)))
            .child(leave)
            .into_any_element();
        let this = cx.weak_entity();
        modal(
            "room-settings",
            if room.is_direct { "this chat".into() } else { "room settings".into() },
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
