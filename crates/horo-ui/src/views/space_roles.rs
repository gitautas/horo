//! Space-level Discord-style roles manager (`im.horo.roles` on the Space).
//!
//! Roles defined on a Space apply across every channel in that Space: member
//! name colors, hoisted sidebar groups, and `@RoleName` mentions.

use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};
use gpui_component::{
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use horo_core::{
    dto::RoomMemberDto,
    matrix::{
        roles::{self, RoleDto},
        rooms,
    },
};
use matrix_sdk::ruma::OwnedRoomId;

use crate::theme::zpx;
use crate::{
    backend::Backend,
    store::Store,
    theme::{Palette, corner, label, space, text},
    views::{
        avatar::avatar,
        explore::Closed,
        modal::{accent_button, modal, well},
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

pub struct SpaceRoles {
    store: Entity<Store>,
    space_id: String,
    space_name: String,
    focus: FocusHandle,
    can_manage: bool,
    members: Option<Vec<RoomMemberDto>>,
    selected_role: Option<String>,
    new_role_name: Entity<InputState>,
    new_role_color: u32,
    saving: bool,
    error: Option<SharedString>,
}

impl EventEmitter<Closed> for SpaceRoles {}

impl SpaceRoles {
    pub fn new(store: Entity<Store>, space_id: String, space_name: String, window: &mut Window, cx: &mut Context<Self>) -> SpaceRoles {
        let new_role_name = cx.new(|cx| InputState::new(window, cx).placeholder("new role name (e.g. Gamers)"));
        cx.subscribe(&new_role_name, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        cx.observe(&store, |_, _, cx| cx.notify()).detach();

        store.update(cx, |s, cx| s.load_roles(&space_id, true, cx));

        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let view = SpaceRoles {
            store,
            space_id,
            space_name,
            focus,
            can_manage: false,
            members: None,
            selected_role: None,
            new_role_name,
            new_role_color: ROLE_COLORS[0],
            saving: false,
            error: None,
        };
        view.load_context(cx);
        view
    }

    fn load_context(&self, cx: &mut Context<Self>) {
        let Ok(space) = OwnedRoomId::try_from(self.space_id.as_str()) else { return };
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move {
                let core = state.core().await?;
                let can_manage = roles::can_manage(&core, &space).await.unwrap_or(false);
                let members = rooms::members(&core, &space).await.unwrap_or_default();
                Ok::<_, horo_core::Error>((can_manage, members))
            }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok((can_manage, members))) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.can_manage = can_manage;
                    this.members = Some(members.into_iter().filter(|m| m.membership == "join").collect());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn persist(&mut self, updated: Vec<RoleDto>, cx: &mut Context<Self>) {
        let Ok(space) = OwnedRoomId::try_from(self.space_id.as_str()) else { return };
        let space_id = self.space_id.clone();
        self.store.update(cx, |s, cx| s.set_roles_cached(space_id, updated.clone(), cx));
        self.saving = true;
        self.error = None;
        cx.notify();

        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { roles::save(&*state.core().await?, &space, &updated).await }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
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
        if name.is_empty() || !self.can_manage {
            return;
        }
        let mut current = self.store.read(cx).roles_for_space(&self.space_id).to_vec();
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
        self.persist(current, cx);
    }

    fn delete_role(&mut self, role_id: &str, cx: &mut Context<Self>) {
        if !self.can_manage {
            return;
        }
        let mut current = self.store.read(cx).roles_for_space(&self.space_id).to_vec();
        current.retain(|r| r.id != role_id);
        if self.selected_role.as_deref() == Some(role_id) {
            self.selected_role = current.first().map(|r| r.id.clone());
        }
        self.persist(current, cx);
    }

    fn update_role(&mut self, role_id: &str, mutate: impl FnOnce(&mut RoleDto), cx: &mut Context<Self>) {
        if !self.can_manage {
            return;
        }
        let mut current = self.store.read(cx).roles_for_space(&self.space_id).to_vec();
        if let Some(role) = current.iter_mut().find(|r| r.id == role_id) {
            mutate(role);
            self.persist(current, cx);
        }
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

impl Render for SpaceRoles {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        let roles = self.store.read(cx).roles_for_space(&self.space_id).to_vec();
        let selected_id = self
            .selected_role
            .clone()
            .filter(|id| roles.iter().any(|r| &r.id == id))
            .or_else(|| roles.first().map(|r| r.id.clone()));
        let selected = selected_id.as_deref().and_then(|id| roles.iter().find(|r| r.id == id).cloned());
        let can_manage = self.can_manage;
        let can_create = can_manage && !self.new_role_name.read(cx).value().trim().is_empty() && !self.saving;

        let create_box = can_manage.then(|| {
            v_flex()
                .gap(space(8.))
                .child(section_label("create role", &palette))
                .child(
                    h_flex()
                        .gap(space(8.))
                        .items_center()
                        .child(div().flex_1().child(well(&palette).child(div().flex_1().child(Input::new(&self.new_role_name).appearance(false)))))
                        .child(
                            accent_button("role-create", "create", can_create, true, &palette)
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
                    "no roles yet — create one above to color names, group member lists, and mention @RoleName across every channel in this space~"
                } else {
                    "no roles have been set up for this space yet."
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
                .child(section_label("space members with this role", &palette))
                .child(
                    v_flex()
                        .id("role-members-scroll")
                        .max_h(zpx(210.))
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

        let body = v_flex()
            .children(create_box)
            .child(list)
            .children(detail)
            .when(self.saving, |d| d.child(div().mt(zpx(6.)).text_size(text(11.)).text_color(palette.text_tertiary).child("saving roles…")))
            .children(self.error.clone().map(|e| div().mt(zpx(8.)).text_size(text(12.)).text_color(palette.danger).child(e)))
            .into_any_element();

        let this = cx.weak_entity();
        modal(
            "space-roles",
            format!("{} — roles", self.space_name).into(),
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
