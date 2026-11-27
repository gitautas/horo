//! Channel / room settings modal: Discord-style Overview & Roles tabs
//! (header card, copyable address, labeled name & topic fields, segmented
//! notification mode selector, custom roles manager, unsaved changes bar,
//! and styled danger zone).

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
    dto::{NotifyMode, RoomMemberDto, RoomMembership, RoomSummary},
    matrix::{
        roles::{self, RoleDto},
        rooms::{self, RoomPermissions},
    },
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

const ROLE_COLORS: [u32; 8] = [
    0x5865F2, // blurple
    0xEB459E, // fuchsia
    0xED4245, // coral
    0xE67E22, // tangerine
    0xFEE75C, // gold
    0x57F287, // mint
    0x1ABC9C, // cyan
    0x9B59B6, // lavender
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Roles,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Confirm {
    Leave,
    Delete,
}

pub struct RoomSettings {
    store: Entity<Store>,
    pub room_id: String,
    focus: FocusHandle,
    tab: Tab,
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
    /// Custom roles management state.
    can_manage_roles: bool,
    members: Option<Vec<RoomMemberDto>>,
    selected_role: Option<String>,
    new_role_name: Entity<InputState>,
    new_role_color: u32,
    saving_roles: bool,
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
        let new_role_name = cx.new(|cx| InputState::new(window, cx).placeholder("new role name (e.g. Gamers)"));
        cx.subscribe(&name, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        cx.subscribe(&topic, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        cx.subscribe(&new_role_name, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        cx.observe(&store, |_, _, cx| cx.notify()).detach();

        store.update(cx, |s, cx| s.load_roles(&room_id, false, cx));

        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let settings = RoomSettings {
            store,
            room_id,
            focus,
            tab: Tab::Overview,
            permissions: None,
            name,
            topic,
            saving: false,
            copied_address: false,
            asked: None,
            acting: false,
            error: None,
            can_manage_roles: false,
            members: None,
            selected_role: None,
            new_role_name,
            new_role_color: ROLE_COLORS[0],
            saving_roles: false,
        };
        settings.load_permissions(cx);
        settings.load_roles_context(cx);
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

    fn load_roles_context(&self, cx: &mut Context<Self>) {
        let Ok(room) = OwnedRoomId::try_from(self.room_id.as_str()) else { return };
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move {
                let core = state.core().await?;
                let can_manage = roles::can_manage(&core, &room).await.unwrap_or(false);
                let members = rooms::members(&core, &room).await.unwrap_or_default();
                Ok::<_, horo_core::Error>((can_manage, members))
            }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok((can_manage, members))) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.can_manage_roles = can_manage;
                    this.members = Some(members.into_iter().filter(|m| m.membership == "join").collect());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn room(&self, cx: &Context<Self>) -> Option<RoomSummary> {
        self.store.read(cx).rooms.iter().find(|r| r.id == self.room_id).cloned()
    }

    fn persist_roles(&mut self, updated: Vec<RoleDto>, cx: &mut Context<Self>) {
        let Ok(room) = OwnedRoomId::try_from(self.room_id.as_str()) else { return };
        let room_id = self.room_id.clone();
        self.store.update(cx, |s, cx| s.set_roles_cached(room_id, updated.clone(), cx));
        self.saving_roles = true;
        self.error = None;
        cx.notify();

        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { roles::save(&*state.core().await?, &room, &updated).await }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                this.saving_roles = false;
                if let Some(err) = error {
                    this.error = Some(format!("couldn't save roles: {err}").into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn create_role(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_role_name.read(cx).value().trim().to_owned();
        if name.is_empty() || !self.can_manage_roles {
            return;
        }
        let mut current = self.store.read(cx).roles_for_room(&self.room_id).to_vec();
        let base_id = roles::slug(&name);
        let mut id = base_id.clone();
        let mut suffix = 2usize;
        while current.iter().any(|r| r.id == id) {
            id = format!("{base_id}-{suffix}");
            suffix += 1;
        }
        current.push(RoleDto {
            id: id.clone(),
            name,
            color: self.new_role_color,
            mentionable: true,
            hoist: true,
            members: Vec::new(),
        });
        self.new_role_name.update(cx, |s, cx| s.set_value("", window, cx));
        self.selected_role = Some(id);
        self.persist_roles(current, cx);
    }

    fn delete_role(&mut self, role_id: &str, cx: &mut Context<Self>) {
        if !self.can_manage_roles {
            return;
        }
        let mut current = self.store.read(cx).roles_for_room(&self.room_id).to_vec();
        current.retain(|r| r.id != role_id);
        if self.selected_role.as_deref() == Some(role_id) {
            self.selected_role = current.first().map(|r| r.id.clone());
        }
        self.persist_roles(current, cx);
    }

    fn update_role(&mut self, role_id: &str, mutate: impl FnOnce(&mut RoleDto), cx: &mut Context<Self>) {
        if !self.can_manage_roles {
            return;
        }
        let mut current = self.store.read(cx).roles_for_room(&self.room_id).to_vec();
        if let Some(role) = current.iter_mut().find(|r| r.id == role_id) {
            mutate(role);
            self.persist_roles(current, cx);
        }
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
        self.tab = Tab::Overview;
        self.asked = Some(Confirm::Leave);
        cx.notify();
    }

    /// Open at the "delete?" question (from a room's right-click menu).
    pub fn ask_to_delete(&mut self, cx: &mut Context<Self>) {
        self.tab = Tab::Overview;
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

    fn tab_bar(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let roles_count = self.store.read(cx).roles_for_room(&self.room_id).len();
        let roles_label = if roles_count > 0 { format!("roles ({roles_count})") } else { "roles".to_owned() };
        h_flex()
            .mt(space(12.))
            .gap(space(6.))
            .p(zpx(4.))
            .rounded(corner(12.))
            .bg(palette.app)
            .border_1()
            .border_color(palette.border_subtle)
            .children([
                (Tab::Overview, "overview".to_owned()),
                (Tab::Roles, roles_label),
            ].into_iter().enumerate().map(|(ix, (tab, title))| {
                let active = self.tab == tab;
                div()
                    .id(("room-tab", ix))
                    .flex_1()
                    .py(space(6.))
                    .rounded(corner(9.))
                    .text_center()
                    .cursor_pointer()
                    .text_size(text(12.))
                    .font_weight(if active { FontWeight::BOLD } else { FontWeight::MEDIUM })
                    .bg(if active { palette.card } else { gpui::transparent_black() })
                    .text_color(if active { palette.text } else { palette.text_tertiary })
                    .when(!active, |d| d.hover(|d| d.text_color(palette.text_secondary)))
                    .child(title)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = tab;
                        cx.notify();
                    }))
            }))
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

    fn roles_tab(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let roles = self.store.read(cx).roles_for_room(&self.room_id).to_vec();
        let selected_id = self
            .selected_role
            .clone()
            .filter(|id| roles.iter().any(|r| &r.id == id))
            .or_else(|| roles.first().map(|r| r.id.clone()));
        let selected = selected_id.as_deref().and_then(|id| roles.iter().find(|r| r.id == id).cloned());
        let can_manage = self.can_manage_roles;
        let can_create = can_manage && !self.new_role_name.read(cx).value().trim().is_empty() && !self.saving_roles;

        let create_box = can_manage.then(|| {
            v_flex()
                .mt(space(14.))
                .gap(space(8.))
                .child(section_label("create role", palette))
                .child(
                    h_flex()
                        .gap(space(8.))
                        .items_center()
                        .child(div().flex_1().child(well(palette).child(div().flex_1().child(Input::new(&self.new_role_name).appearance(false)))))
                        .child(
                            accent_button("role-create", "create", can_create, true, palette)
                                .on_click(cx.listener(|this, _, window, cx| this.create_role(window, cx))),
                        ),
                )
                .child(
                    h_flex().gap(space(6.)).children(ROLE_COLORS.into_iter().enumerate().map(|(ix, hex)| {
                        let picked = self.new_role_color == hex;
                        let c: Hsla = gpui::rgb(hex).into();
                        div()
                            .id(("new-role-color", ix))
                            .size(zpx(18.))
                            .rounded_full()
                            .cursor_pointer()
                            .bg(c)
                            .when(picked, |d| d.border_2().border_color(palette.text))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.new_role_color = hex;
                                cx.notify();
                            }))
                    })),
                )
        });

        let list = if roles.is_empty() {
            div()
                .mt(space(14.))
                .p(space(14.))
                .rounded(corner(12.))
                .bg(palette.app)
                .border_1()
                .border_color(palette.border_subtle)
                .text_size(text(12.))
                .text_color(palette.text_tertiary)
                .child(if can_manage {
                    "no roles yet — create one above to color names, group the member list, and mention @RoleName~"
                } else {
                    "no roles have been set up for this channel yet."
                })
                .into_any_element()
        } else {
            h_flex()
                .mt(space(12.))
                .flex_wrap()
                .gap(space(6.))
                .children(roles.iter().enumerate().map(|(ix, role)| {
                    let active = selected_id.as_deref() == Some(role.id.as_str());
                    let c: Hsla = gpui::rgb(role.color).into();
                    let select_id = role.id.clone();
                    let del_id = role.id.clone();
                    h_flex()
                        .id(("role-pill", ix))
                        .items_center()
                        .gap(space(6.))
                        .pl(space(10.))
                        .pr(space(if can_manage { 6. } else { 10. }))
                        .py(space(5.))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(if active { Hsla { a: 0.16, ..c } } else { palette.app })
                        .border_1()
                        .border_color(if active { c } else { palette.border_subtle })
                        .child(div().size(zpx(8.)).rounded_full().bg(c))
                        .child(div().text_size(text(12.)).font_weight(FontWeight::BOLD).text_color(if active { c } else { palette.text }).child(format!("@{}", role.name)))
                        .child(div().font_family(palette.mono_font.clone()).text_size(text(10.)).text_color(palette.text_tertiary).child(role.members.len().to_string()))
                        .when(can_manage, |d| {
                            d.child(
                                div()
                                    .id(("role-del", ix))
                                    .px(zpx(4.))
                                    .rounded_full()
                                    .text_size(text(10.))
                                    .text_color(palette.text_tertiary)
                                    .hover(|d| d.text_color(palette.danger))
                                    .child("✕")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.delete_role(&del_id, cx);
                                    })),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_role = Some(select_id.clone());
                            cx.notify();
                        }))
                }))
                .into_any_element()
        };

        let detail = selected.map(|role| {
            let role_id = role.id.clone();
            let c: Hsla = gpui::rgb(role.color).into();
            let members_list = self.members.clone().unwrap_or_default();
            let toggle_chip = |id: &'static str, label_text: &'static str, on: bool| {
                div()
                    .id(id)
                    .px(space(10.))
                    .py(space(5.))
                    .rounded(corner(10.))
                    .border_1()
                    .border_color(if on { palette.accent } else { palette.border_subtle })
                    .bg(if on { Hsla { a: 0.16, ..palette.accent } } else { palette.card })
                    .text_size(text(11.5))
                    .font_weight(if on { FontWeight::BOLD } else { FontWeight::MEDIUM })
                    .text_color(if on { palette.accent } else { palette.text_secondary })
                    .when(can_manage, |d| d.cursor_pointer())
                    .child(label_text)
            };
            let rid_m = role_id.clone();
            let rid_h = role_id.clone();
            v_flex()
                .mt(space(14.))
                .p(space(12.))
                .gap(space(10.))
                .rounded(corner(14.))
                .bg(palette.app)
                .border_1()
                .border_color(palette.border_subtle)
                .child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .child(
                            h_flex()
                                .gap(space(8.))
                                .items_center()
                                .child(div().size(zpx(10.)).rounded_full().bg(c))
                                .child(div().text_size(text(13.)).font_weight(FontWeight::BOLD).text_color(c).child(format!("@{}", role.name))),
                        )
                        .when(can_manage, |d| {
                            d.child(
                                h_flex().gap(zpx(5.)).children(ROLE_COLORS.into_iter().enumerate().map(|(ix, hex)| {
                                    let rid = role_id.clone();
                                    let swatch: Hsla = gpui::rgb(hex).into();
                                    div()
                                        .id(("edit-role-color", ix))
                                        .size(zpx(14.))
                                        .rounded_full()
                                        .cursor_pointer()
                                        .bg(swatch)
                                        .when(role.color == hex, |d| d.border_2().border_color(palette.text))
                                        .on_click(cx.listener(move |this, _, _, cx| this.update_role(&rid, |r| r.color = hex, cx)))
                                })),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .gap(space(8.))
                        .child(toggle_chip("role-mentionable", if role.mentionable { "mentionable ✓" } else { "not mentionable" }, role.mentionable).when(can_manage, |d| {
                            d.on_click(cx.listener(move |this, _, _, cx| this.update_role(&rid_m, |r| r.mentionable = !r.mentionable, cx)))
                        }))
                        .child(toggle_chip("role-hoist", if role.hoist { "grouped in sidebar ✓" } else { "not grouped" }, role.hoist).when(can_manage, |d| {
                            d.on_click(cx.listener(move |this, _, _, cx| this.update_role(&rid_h, |r| r.hoist = !r.hoist, cx)))
                        })),
                )
                .child(section_label("members with this role", palette))
                .child(
                    v_flex()
                        .id("role-members-scroll")
                        .max_h(zpx(180.))
                        .overflow_y_scroll()
                        .gap(zpx(4.))
                        .children(members_list.into_iter().enumerate().map(|(ix, member)| {
                            let has = role.members.iter().any(|id| id == &member.user_id);
                            let name = member.display_name.clone().unwrap_or_else(|| member.user_id.clone());
                            let uid = member.user_id.clone();
                            let rid = role_id.clone();
                            h_flex()
                                .id(("role-member", ix))
                                .items_center()
                                .justify_between()
                                .gap(space(8.))
                                .px(space(8.))
                                .py(space(5.))
                                .rounded(corner(10.))
                                .bg(if has { Hsla { a: 0.12, ..c } } else { palette.card })
                                .when(can_manage, |d| d.cursor_pointer().hover(|d| d.bg(palette.raised)))
                                .child(
                                    h_flex()
                                        .gap(space(8.))
                                        .min_w_0()
                                        .child(avatar(&member.user_id, &name, member.avatar_url.as_deref(), zpx(22.), cx))
                                        .child(div().truncate().text_size(text(12.)).text_color(if has { palette.text } else { palette.text_secondary }).child(name)),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(text(11.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(if has { c } else { palette.text_tertiary })
                                        .child(if has { "✓" } else { "+" }),
                                )
                                .when(can_manage, |d| {
                                    d.on_click(cx.listener(move |this, _, _, cx| {
                                        let uid = uid.clone();
                                        this.update_role(
                                            &rid,
                                            move |r| {
                                                if let Some(pos) = r.members.iter().position(|id| id == &uid) {
                                                    r.members.remove(pos);
                                                } else {
                                                    r.members.push(uid);
                                                }
                                            },
                                            cx,
                                        );
                                    }))
                                })
                        })),
                )
        });

        v_flex()
            .children(create_box)
            .child(list)
            .children(detail)
            .when(self.saving_roles, |d| d.child(div().mt(zpx(6.)).text_size(text(11.)).text_color(palette.text_tertiary).child("saving roles…")))
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
        let tabs = (!room.is_direct).then(|| self.tab_bar(&palette, cx));
        let tab_body = match self.tab {
            Tab::Overview => {
                let overview = self.overview_section(&room, &palette, cx);
                let notifications = self.notifications_section(&room, &palette, cx);
                let unsaved = self.unsaved_bar(&palette, cx);
                let danger = self.danger_zone(&room, &palette, cx);
                v_flex().child(overview).child(notifications).children(unsaved).child(danger).into_any_element()
            }
            Tab::Roles => self.roles_tab(&palette, cx),
        };
        let body = v_flex()
            .child(header)
            .children(tabs)
            .child(tab_body)
            .children(self.error.clone().map(|e| div().mt(zpx(10.)).text_size(text(12.)).text_color(palette.danger).child(e)))
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
