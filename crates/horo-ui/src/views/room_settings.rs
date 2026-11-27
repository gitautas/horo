//! Channel / room settings modal: Discord-style overview (header card,
//! copyable address, labeled name & topic fields, segmented notification
//! mode selector, unsaved changes bar, and styled danger zone).

use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    v_flex,
};
use horo_core::{
    dto::{NotifyMode, RoomMembership, RoomSummary},
    matrix::rooms::{self, RoomPermissions},
};
use matrix_sdk::ruma::OwnedRoomId;

use crate::theme::zpx;
use crate::{
    backend::Backend,
    store::Store,
    theme::{Palette, corner, heading, label, space, text},
    views::{
        avatar::avatar,
        explore::Closed,
        modal::{accent_button, modal, well},
        voice::icon,
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
    copied_address: bool,
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
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("channel-name").default_value(name_now));
        let topic = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx);
            state.set_auto_grow(2, 5, cx);
            state.set_placeholder("let everyone know how to use this channel~", window, cx);
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
            copied_address: false,
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

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(room) = self.room(cx) else { return };
        self.name.update(cx, |state, cx| state.set_value(room.name.clone(), window, cx));
        self.topic.update(cx, |state, cx| state.set_value(room.topic.unwrap_or_default(), window, cx));
        self.error = None;
        cx.notify();
    }

    fn set_notify_mode(&mut self, mode: NotifyMode, cx: &mut Context<Self>) {
        let Ok(room) = OwnedRoomId::try_from(self.room_id.as_str()) else { return };
        let backend = Backend::global(cx);
        let task = backend.run({
            let (host, state) = (backend.host.clone(), backend.state.clone());
            async move {
                let core = state.core().await?;
                rooms::set_notify_mode(&core, &room, mode).await?;
                rooms::emit_room_refresh(&host, &core, &room).await;
                Ok::<_, horo_core::Error>(())
            }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            if let Some(err) = error {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(format!("couldn't update notifications: {err}").into());
                    cx.notify();
                });
            }
        })
        .detach();
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

    fn header_card(&self, room: &RoomSummary, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let address = room.canonical_alias.clone().unwrap_or_else(|| room.id.clone());
        let copy_addr = address.clone();
        let copied = self.copied_address;
        let face = avatar(&room.id, &room.name, room.avatar_url.as_deref(), zpx(48.), cx);
        let title = if room.is_direct { room.name.clone() } else { format!("# {}", room.name) };
        h_flex()
            .gap(space(14.))
            .p(space(12.))
            .rounded(corner(16.))
            .bg(palette.app)
            .border_1()
            .border_color(palette.border_subtle)
            .child(face)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(zpx(4.))
                    .child(
                        h_flex()
                            .gap(space(6.))
                            .items_center()
                            .child(
                                div()
                                    .truncate()
                                    .font_family(palette.heading_font.clone())
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .text_size(heading(16.))
                                    .text_color(palette.text)
                                    .child(title),
                            )
                            .when(room.is_encrypted, |d| d.child(icon("icons/lock.svg").size(zpx(13.)).text_color(palette.accent).flex_none())),
                    )
                    .child(
                        h_flex()
                            .gap(space(8.))
                            .items_center()
                            .child(
                                div()
                                    .text_size(text(11.))
                                    .text_color(palette.text_tertiary)
                                    .child(format!("{} {}", room.member_count, if room.member_count == 1 { "member" } else { "members" })),
                            )
                            .child(div().text_size(text(11.)).text_color(palette.text_tertiary).child("·"))
                            .child(
                                div()
                                    .id("room-copy-addr")
                                    .truncate()
                                    .cursor_pointer()
                                    .px(space(6.))
                                    .py(zpx(1.))
                                    .rounded(corner(6.))
                                    .bg(palette.card)
                                    .hover(|d| d.bg(palette.raised).text_color(palette.text))
                                    .font_family(palette.mono_font.clone())
                                    .text_size(text(10.5))
                                    .text_color(if copied { palette.accent } else { palette.text_tertiary })
                                    .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("click to copy room address").build(window, cx))
                                    .child(if copied { "copied!".to_owned() } else { address })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_addr.clone()));
                                        this.copied_address = true;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn overview_section(&mut self, room: &RoomSummary, palette: &Palette, _cx: &mut Context<Self>) -> AnyElement {
        let Some(permissions) = self.permissions.clone() else {
            return div()
                .mt(space(14.))
                .text_size(text(12.))
                .text_color(palette.text_tertiary)
                .child("checking permissions…")
                .into_any_element();
        };
        if !permissions.can_rename && !permissions.can_set_topic {
            return v_flex()
                .mt(space(14.))
                .gap(zpx(6.))
                .child(section_label("overview", palette))
                .child(
                    div()
                        .p(space(12.))
                        .rounded(corner(12.))
                        .bg(palette.app)
                        .border_1()
                        .border_color(palette.border_subtle)
                        .text_size(text(12.))
                        .line_height(gpui::relative(1.5))
                        .text_color(palette.text_tertiary)
                        .child("only admins can rename or edit the topic for this channel."),
                )
                .into_any_element();
        }
        v_flex()
            .mt(space(16.))
            .gap(space(12.))
            .when(permissions.can_rename, |d| {
                d.child(
                    v_flex()
                        .gap(zpx(6.))
                        .child(section_label(if room.is_direct { "chat name" } else { "channel name" }, palette))
                        .child(
                            well(palette)
                                .when(!room.is_direct, |w| {
                                    w.child(
                                        div()
                                            .flex_none()
                                            .pr(zpx(4.))
                                            .font_family(palette.mono_font.clone())
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(palette.text_tertiary)
                                            .child("#"),
                                    )
                                })
                                .child(div().flex_1().child(Input::new(&self.name).appearance(false))),
                        ),
                )
            })
            .when(permissions.can_set_topic, |d| {
                d.child(
                    v_flex()
                        .gap(zpx(6.))
                        .child(section_label("channel topic", palette))
                        .child(well(palette).child(div().flex_1().child(Textarea::new(&self.topic).appearance(false)))),
                )
            })
            .into_any_element()
    }

    fn notifications_section(&self, room: &RoomSummary, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let current = room.notify_mode.unwrap_or(if room.is_muted { NotifyMode::Mute } else { NotifyMode::All });
        let options = [
            (NotifyMode::All, "all messages"),
            (NotifyMode::Mentions, "@mentions only"),
            (NotifyMode::Mute, "muted"),
        ];
        v_flex()
            .mt(space(16.))
            .gap(zpx(6.))
            .child(section_label("notifications", palette))
            .child(
                h_flex()
                    .gap(space(6.))
                    .p(zpx(4.))
                    .rounded(corner(14.))
                    .bg(palette.app)
                    .border_1()
                    .border_color(palette.border_subtle)
                    .children(options.into_iter().enumerate().map(|(ix, (mode, label_text))| {
                        let active = current == mode;
                        div()
                            .id(("room-notify", ix))
                            .flex_1()
                            .py(space(7.))
                            .px(space(8.))
                            .rounded(corner(10.))
                            .text_center()
                            .cursor_pointer()
                            .text_size(text(11.5))
                            .font_weight(if active { FontWeight::BOLD } else { FontWeight::MEDIUM })
                            .bg(if active {
                                if mode == NotifyMode::Mute {
                                    Hsla { a: 0.16, ..palette.idle }
                                } else {
                                    Hsla { a: 0.18, ..palette.accent }
                                }
                            } else {
                                gpui::transparent_black()
                            })
                            .text_color(if active {
                                if mode == NotifyMode::Mute { palette.idle } else { palette.accent }
                            } else {
                                palette.text_secondary
                            })
                            .when(!active, |d| d.hover(|d| d.bg(palette.card).text_color(palette.text)))
                            .child(label_text)
                            .on_click(cx.listener(move |this, _, _, cx| this.set_notify_mode(mode, cx)))
                    })),
            )
            .into_any_element()
    }

    fn unsaved_bar(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (name, topic) = self.changes(cx);
        if name.is_none() && topic.is_none() {
            return None;
        }
        Some(
            h_flex()
                .mt(space(14.))
                .px(space(12.))
                .py(space(8.))
                .rounded(corner(14.))
                .bg(palette.app)
                .border_1()
                .border_color(palette.accent)
                .items_center()
                .justify_between()
                .gap(space(10.))
                .child(div().text_size(text(12.)).text_color(palette.text).child("careful — you have unsaved changes!"))
                .child(
                    h_flex()
                        .gap(space(6.))
                        .child(Button::new("room-reset").ghost().small().label("reset").disabled(self.saving).on_click(cx.listener(|this, _, window, cx| {
                            this.reset(window, cx);
                        })))
                        .child(
                            accent_button("room-save", if self.saving { "saving…" } else { "save changes" }, !self.saving, true, palette)
                                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    fn danger_zone(&mut self, room: &RoomSummary, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let invited = room.membership == RoomMembership::Invited;
        let verb = if invited {
            "decline invite"
        } else if room.is_direct {
            "leave chat"
        } else {
            "leave channel"
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
                    .child(div().mb(zpx(10.)).text_size(text(12.)).line_height(gpui::relative(1.5)).text_color(palette.text_secondary).child(question))
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
                    "delete {}? everyone is removed, it comes out of its spaces, and nobody can get back in. this can't be undone.",
                    room.name
                );
                v_flex()
                    .child(div().mb(zpx(10.)).text_size(text(12.)).line_height(gpui::relative(1.5)).text_color(palette.text_secondary).child(question))
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
                                    .label(if self.acting { "deleting…" } else { "delete channel" })
                                    .disabled(self.acting)
                                    .on_click(cx.listener(|this, _, _, cx| this.delete(cx))),
                            ),
                    )
                    .into_any_element()
            }
            None => h_flex()
                .items_center()
                .justify_between()
                .gap(space(8.))
                .child(
                    Button::new("leave-ask").danger().small().label(verb).on_click(cx.listener(|this, _, _, cx| {
                        this.asked = Some(Confirm::Leave);
                        cx.notify();
                    })),
                )
                .when(can_delete, |row| {
                    row.child(
                        Button::new("delete-ask").danger().small().label("delete channel").on_click(cx.listener(|this, _, _, cx| {
                            this.asked = Some(Confirm::Delete);
                            cx.notify();
                        })),
                    )
                })
                .into_any_element(),
        };
        v_flex()
            .mt(space(18.))
            .gap(zpx(6.))
            .child(section_label("danger zone", palette))
            .child(
                div()
                    .p(space(12.))
                    .rounded(corner(14.))
                    .bg(Hsla { a: 0.05, ..palette.danger })
                    .border_1()
                    .border_color(Hsla { a: 0.28, ..palette.danger })
                    .child(body),
            )
            .into_any_element()
    }
}

fn section_label(title: &'static str, palette: &Palette) -> AnyElement {
    div()
        .font_family(palette.heading_font.clone())
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(label(9.5))
        .text_color(palette.text_tertiary)
        .child(title)
        .into_any_element()
}

impl Render for RoomSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        let Some(room) = self.room(cx) else { return div().into_any_element() };
        let header = self.header_card(&room, &palette, cx);
        let overview = self.overview_section(&room, &palette, cx);
        let notifications = self.notifications_section(&room, &palette, cx);
        let unsaved = self.unsaved_bar(&palette, cx);
        let danger = self.danger_zone(&room, &palette, cx);
        let body = v_flex()
            .child(header)
            .child(overview)
            .child(notifications)
            .children(unsaved)
            .children(self.error.clone().map(|e| div().mt(zpx(10.)).text_size(text(12.)).text_color(palette.danger).child(e)))
            .child(danger)
            .into_any_element();
        let this = cx.weak_entity();
        modal(
            "room-settings",
            if room.is_direct { "chat settings".into() } else { "channel settings".into() },
            520.,
            &self.focus,
            &palette,
            move |_, cx| {
                let _ = this.update(cx, |_, cx| cx.emit(Closed));
            },
            body,
        )
    }
}
