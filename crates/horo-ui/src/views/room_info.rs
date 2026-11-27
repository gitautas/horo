//! The room's details beside its timeline, as RoomInfo.tsx draws them:
//!
//! - a DM shows the person (their cover, face, presence and bio); any other
//!   room shows its picture, name, alias and topic;
//! - whether it's encrypted, and how many members are verified;
//! - the members, each with presence, their id and a verify button;
//! - in the corner, the room's mute toggle.
//!
//! The header's ⓘ button opens and closes it; whether it's open is a
//! preference, so it stays the way you left it.

use gpui::{
    AnyElement, ClickEvent, Context, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, StyledImage, Window, div, img, prelude::FluentBuilder,
};
use gpui_component::{h_flex, v_flex};
use horo_core::{
    dto::{RoomMemberDto, RoomSummary, Verification},
    matrix::{
        profile::{self, ProfileDto},
        rooms,
    },
    verification,
};
use matrix_sdk::ruma::OwnedRoomId;

use crate::theme::zpx;
use crate::{
    backend::Backend,
    media::Media,
    presence,
    store::{Lightbox, LightboxItem, Phase, Store},
    theme::{Palette, corner, heading, label, space, text},
    views::{
        avatar::{accent_for, avatar, presence_dot},
        voice::icon,
    },
};

/// React's default panel width.
pub const WIDTH: f32 = 296.;
const COVER: f32 = 104.;
const FACE: f32 = 72.;
/// Presence is watched for this many of the members at most; a room of
/// hundreds would otherwise subscribe to hundreds of people.
const PRESENCE_FOR: usize = 100;
/// Members listed at a time; "show more" adds as many again. Every row is
/// drawn (and its picture fetched) whether it's scrolled to or not.
const PAGE: usize = 100;

/// The pencil was clicked: the shell opens the room's settings over the
/// window (the panel is too narrow to hold them).
pub struct OpenRoomSettings;

impl gpui::EventEmitter<OpenRoomSettings> for RoomInfo {}

pub struct RoomInfo {
    store: Entity<Store>,
    pub room_id: String,
    /// `None` while loading.
    members: Option<Vec<RoomMemberDto>>,
    /// The DM partner's profile, for the cover and bio.
    partner_profile: Option<ProfileDto>,
    error: Option<SharedString>,
    /// How many members are listed.
    shown: usize,
}

impl RoomInfo {
    pub fn new(store: Entity<Store>, room_id: String, cx: &mut Context<Self>) -> RoomInfo {
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        let info = RoomInfo { store, room_id, members: None, partner_profile: None, error: None, shown: PAGE };
        info.load(cx);
        info
    }

    fn load(&self, cx: &mut Context<Self>) {
        let Ok(room) = OwnedRoomId::try_from(self.room_id.as_str()) else { return };
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { rooms::members(&*state.core().await?, &room).await }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
            let _ = this.update(cx, |info, cx| {
                match outcome {
                    Ok(members) => {
                        let people = members.iter().take(PRESENCE_FOR).map(|m| m.user_id.clone()).collect();
                        info.store.update(cx, |s, cx| s.set_people_in_panel(people, cx));
                        info.members = Some(members);
                        info.load_partner(cx);
                    }
                    Err(e) => {
                        info.members = Some(Vec::new());
                        info.error = Some(format!("couldn't load the members: {e}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Who a DM is with: only when it's obvious (exactly one other member).
    fn partner(&self, room: &RoomSummary, me: Option<&str>) -> Option<RoomMemberDto> {
        if !room.is_direct {
            return None;
        }
        let members = self.members.as_ref()?;
        let mut others = members.iter().filter(|m| Some(m.user_id.as_str()) != me);
        let one = others.next()?;
        others.next().is_none().then(|| one.clone())
    }

    fn load_partner(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let me = own_id(store);
        let Some(room) = store.rooms.iter().find(|r| r.id == self.room_id).cloned() else { return };
        let Some(partner) = self.partner(&room, me.as_deref()) else { return };
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { profile::get_profile(&*state.core().await?, Some(partner.user_id)).await }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(profile)) = task.await {
                let _ = this.update(cx, |info, cx| {
                    info.partner_profile = Some(profile);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn toggle_muted(&mut self, muted: bool, cx: &mut Context<Self>) {
        let Ok(room) = OwnedRoomId::try_from(self.room_id.as_str()) else { return };
        let backend = Backend::global(cx);
        let task = backend.run({
            let (host, state) = (backend.host.clone(), backend.state.clone());
            async move {
                let core = state.core().await?;
                rooms::set_muted(&core, &room, muted).await?;
                // Muting writes push rules, which the room-list stream doesn't
                // report on its own.
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
            let _ = this.update(cx, |info, cx| {
                info.error = error.map(|e| format!("couldn't change notifications: {e}").into());
                cx.notify();
            });
        })
        .detach();
    }

    fn verify(&mut self, user_id: String, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let task = backend.run({
            let (host, state) = (backend.host.clone(), backend.state.clone());
            async move { verification::request(&host, &state.core().await?, Some(user_id)).await }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            let _ = this.update(cx, |info, cx| {
                info.error = error.map(|e| format!("couldn't start verifying: {e}").into());
                cx.notify();
            });
        })
        .detach();
    }

    fn room_heading(&self, room: &RoomSummary, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let picture = avatar(&room.id, &room.name, room.avatar_url.as_deref(), zpx(76.), cx);
        let (store, mxc, name) = (self.store.clone(), room.avatar_url.clone(), room.name.clone());
        v_flex()
            .items_center()
            .pt(space(8.))
            .pb(space(16.))
            .child(
                div()
                    .id("info-room-picture")
                    .when(mxc.is_some(), |d| d.cursor_pointer())
                    .child(picture)
                    .on_click(move |_, _, cx| {
                        if let Some(mxc) = mxc.clone() {
                            let lightbox = Lightbox::single(LightboxItem { mxc: Some(mxc), thumbnail: None, width: None, height: None, name: name.clone() });
                            store.update(cx, |s, cx| s.open_lightbox(lightbox, cx));
                        }
                    }),
            )
            .child(
                div()
                    .mt(zpx(12.))
                    .max_w_full()
                    .truncate()
                    .font_family(palette.heading_font.clone())
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_size(heading(19.))
                    .child(room.name.clone()),
            )
            .child(
                div()
                    .max_w_full()
                    .truncate()
                    .font_family(palette.mono_font.clone())
                    .text_size(text(11.))
                    .text_color(palette.text_tertiary)
                    .child(room.canonical_alias.clone().unwrap_or_else(|| room.id.clone())),
            )
            .into_any_element()
    }

    fn partner_heading(&self, partner: &RoomMemberDto, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let name = partner.display_name.clone().unwrap_or_else(|| partner.user_id.clone());
        let profile = self.partner_profile.as_ref();
        let cover = profile
            .and_then(|p| p.cover_url.clone())
            .and_then(|mxc| Media::image(&mxc, Some(((WIDTH * 2.) as u32, (COVER * 2.) as u32)), cx));
        let cover = div().h(zpx(COVER)).w_full().bg(palette.raised).map(|d| match cover {
            // The frame's shape, not the photo's: see `avatar`.
            Some(image) => d.child(img(image).size_full().aspect_ratio(WIDTH / COVER).object_fit(gpui::ObjectFit::Cover)),
            None => d,
        });
        let (shown, line) = {
            let store = self.store.read(cx);
            let dto = store.presence.get(&partner.user_id).cloned();
            let shown = presence::shown(dto.as_ref(), store.presence_supported);
            let line = shown.zip(dto).map(|(s, dto)| presence::line(&dto, s, now_ms()));
            (shown, line)
        };
        let mxc = profile.and_then(|p| p.avatar_url.clone()).or_else(|| partner.avatar_url.clone());
        let face = div()
            .relative()
            .self_start()
            .p(zpx(3.))
            .rounded(zpx(FACE / 3. + 3.))
            .bg(palette.app)
            .child(avatar(&partner.user_id, &name, mxc.as_deref(), zpx(FACE), cx))
            .children(shown.map(|s| presence_dot(s, zpx(16.), palette.app, palette)));
        let bio = profile.and_then(|p| p.bio.clone()).filter(|b| !b.trim().is_empty());

        v_flex()
            // Out to the panel's edges, over its padding.
            .mx(-space(16.))
            .mt(-space(18.))
            .child(cover)
            .child(
                v_flex()
                    .px(space(16.))
                    .mt(zpx(-FACE / 2.))
                    .child(face)
                    .child(
                        div()
                            .mt(zpx(8.))
                            .truncate()
                            .font_family(palette.heading_font.clone())
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .text_size(heading(19.))
                            .text_color(accent_for(&partner.user_id))
                            .child(name),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(palette.mono_font.clone())
                            .text_size(text(11.))
                            .text_color(palette.text_tertiary)
                            .child(partner.user_id.clone()),
                    )
                    .children(line.map(|l| div().mt(zpx(4.)).text_size(text(11.5)).text_color(palette.text_secondary).child(l)))
                    .children(bio.map(|b| div().pt(space(12.)).text_size(text(12.5)).line_height(gpui::relative(1.55)).text_color(palette.text_secondary).child(b))),
            )
            .child(div().h(zpx(16.)))
            .into_any_element()
    }

    fn member_row(&self, member: &RoomMemberDto, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let name = member.display_name.clone().unwrap_or_else(|| member.user_id.clone());
        let shown = {
            let store = self.store.read(cx);
            presence::shown(store.presence.get(&member.user_id), store.presence_supported)
        };
        let face = div()
            .relative()
            .flex_none()
            .child(avatar(&member.user_id, &name, member.avatar_url.as_deref(), zpx(34.), cx))
            .children(shown.map(|s| presence_dot(s, zpx(11.), palette.app, palette)));
        let (store, user, hint) = (self.store.clone(), member.user_id.clone(), member.avatar_url.clone());
        let open = move |click: &ClickEvent, _: &mut Window, cx: &mut gpui::App| {
            let (user, hint) = (user.clone(), hint.clone());
            store.update(cx, |s, cx| s.open_profile(user, click.position(), hint, cx));
        };
        let badge = if member.verification == Verification::Verified {
            icon("icons/badge-check.svg").size(zpx(14.)).text_color(palette.accent).into_any_element()
        } else {
            let user = member.user_id.clone();
            div()
                .id(SharedString::from(format!("verify-{}", member.user_id)))
                .cursor_pointer()
                .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("verify this person").build(window, cx))
                .child(icon("icons/badge-question-mark.svg").size(zpx(14.)).text_color(palette.text_tertiary))
                .on_click(cx.listener(move |this, _, _, cx| this.verify(user.clone(), cx)))
                .into_any_element()
        };
        let name_color = self
            .store
            .read(cx)
            .role_color_for(&self.room_id, &member.user_id)
            .unwrap_or_else(|| accent_for(&member.user_id));
        h_flex()
            .id(SharedString::from(format!("member-{}", member.user_id)))
            .gap(space(10.))
            .px(space(6.))
            .py(space(8.))
            .rounded(corner(14.))
            .cursor_pointer()
            .hover(|d| d.bg(Hsla { a: 0.05, ..gpui::white() }))
            .on_click(open)
            .child(face)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::BOLD)
                            .text_size(text(13.5))
                            .text_color(name_color)
                            .child(name),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(palette.mono_font.clone())
                            .text_size(text(10.5))
                            .text_color(palette.text_tertiary)
                            .child(member.user_id.clone()),
                    ),
            )
            .child(badge)
            .into_any_element()
    }
}

fn own_id(store: &Store) -> Option<String> {
    match &store.phase {
        Phase::SignedIn(info) => Some(info.user_id.clone()),
        _ => None,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or_default()
}

fn encryption_card(room: &RoomSummary, unverified: usize, total: usize, palette: &Palette) -> AnyElement {
    let card = v_flex().bg(palette.card).rounded(corner(20.)).p(space(14.)).border_1();
    let title = |icon_path: &'static str, colour: Hsla, words: String| {
        h_flex()
            .gap(space(8.))
            .child(icon(icon_path).size(zpx(16.)).text_color(colour))
            .child(div().font_family(palette.heading_font.clone()).font_weight(FontWeight::BOLD).text_size(heading(14.)).child(words))
    };
    let body = |words: String| div().mt(zpx(5.)).text_size(text(12.5)).line_height(gpui::relative(1.5)).text_color(palette.text_secondary).child(words);
    if !room.is_encrypted {
        return card
            .border_color(Hsla { a: 0.3, ..palette.accent })
            .child(title("icons/lock-open.svg", palette.idle, "not encrypted".into()))
            .child(body("messages here are readable by the homeserver.".into()))
            .into_any_element();
    }
    let all_verified = total > 0 && unverified == 0;
    card.border_color(palette.border_subtle)
        .shadow_sm()
        .child(if all_verified {
            title("icons/shield-check.svg", palette.accent, "everyone verified~".into())
        } else {
            title("icons/shield-alert.svg", palette.idle, "some devices unverified".into())
        })
        .child(body(if all_verified {
            format!("all {total} members' devices are cross-signed. nothing to worry about.")
        } else {
            format!("{unverified} of {total} members haven't been verified yet.")
        }))
        .child(
            h_flex()
                .gap(space(6.))
                .mt(zpx(10.))
                .font_family(palette.mono_font.clone())
                .text_size(text(10.5))
                .text_color(palette.text_tertiary)
                .child("megolm · v1")
                .child("·")
                .child("end-to-end"),
        )
        .into_any_element()
}

impl Render for RoomInfo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        let (room, me) = {
            let store = self.store.read(cx);
            (store.rooms.iter().find(|r| r.id == self.room_id).cloned(), own_id(store))
        };
        let Some(room) = room else { return div().into_any_element() };

        let partner = self.partner(&room, me.as_deref());
        let heading_part = match &partner {
            Some(partner) => self.partner_heading(partner, &palette, cx),
            None => self.room_heading(&room, &palette, cx),
        };
        let members = self.members.clone();
        let total = members.as_ref().map_or(0, Vec::len);
        let unverified = members.iter().flatten().filter(|m| m.verification != Verification::Verified).count();

        let muted = room.is_muted;
        let action_strip = h_flex()
            .gap(space(8.))
            .mb(space(16.))
            .child(
                v_flex()
                    .id("info-mute")
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap(zpx(5.))
                    .py(space(10.))
                    .px(space(8.))
                    .rounded(corner(14.))
                    .cursor_pointer()
                    .bg(palette.card)
                    .hover(|d| d.bg(palette.raised))
                    .border_1()
                    .border_color(if muted { palette.idle } else { palette.border_subtle })
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(if muted { "unmute notifications" } else { "mute notifications" }).build(window, cx)
                    })
                    .child(icon(if muted { "icons/bell-off.svg" } else { "icons/bell.svg" }).size(zpx(15.)).text_color(if muted {
                        palette.idle
                    } else {
                        palette.text_secondary
                    }))
                    .child(
                        div()
                            .text_size(text(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if muted { palette.idle } else { palette.text_secondary })
                            .child(if muted { "muted" } else { "notifications" }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_muted(!muted, cx))),
            )
            .child(
                v_flex()
                    .id("info-settings")
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap(zpx(5.))
                    .py(space(10.))
                    .px(space(8.))
                    .rounded(corner(14.))
                    .cursor_pointer()
                    .bg(palette.card)
                    .hover(|d| d.bg(palette.raised))
                    .border_1()
                    .border_color(palette.border_subtle)
                    .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("room settings").build(window, cx))
                    .child(gpui_component::Icon::new(gpui_component::IconName::Settings).size(zpx(15.)).text_color(palette.text_secondary))
                    .child(
                        div()
                            .text_size(text(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette.text_secondary)
                            .child(if room.is_direct { "chat settings" } else { "settings" }),
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(OpenRoomSettings))),
            );

        let roles = self.store.read(cx).roles_for_room(&self.room_id).to_vec();
        let shown_members: Vec<&RoomMemberDto> = members.iter().flatten().take(self.shown).collect();
        let mut grouped_sections: Vec<AnyElement> = Vec::new();
        let mut placed: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for role in roles.iter().filter(|r| r.hoist) {
            let group: Vec<&RoomMemberDto> = shown_members
                .iter()
                .copied()
                .filter(|m| !placed.contains(m.user_id.as_str()) && role.members.iter().any(|id| id == &m.user_id))
                .collect();
            if !group.is_empty() {
                for m in &group {
                    placed.insert(m.user_id.as_str());
                }
                let role_color: Hsla = gpui::rgb(role.color).into();
                grouped_sections.push(
                    h_flex()
                        .items_center()
                        .gap(space(6.))
                        .px(space(4.))
                        .pt(space(18.))
                        .pb(space(6.))
                        .child(div().size(zpx(7.)).rounded_full().bg(role_color))
                        .child(
                            div()
                                .font_family(palette.heading_font.clone())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(label(9.5))
                                .text_color(role_color)
                                .child(format!("{} · {}", role.name.to_lowercase(), group.len())),
                        )
                        .into_any_element(),
                );
                for m in group {
                    grouped_sections.push(self.member_row(m, &palette, cx));
                }
            }
        }
        let ungrouped: Vec<&RoomMemberDto> = shown_members.into_iter().filter(|m| !placed.contains(m.user_id.as_str())).collect();
        if !ungrouped.is_empty() || members.is_none() || grouped_sections.is_empty() {
            let count_label = if members.is_none() {
                "…".to_owned()
            } else if grouped_sections.is_empty() {
                total.to_string()
            } else {
                ungrouped.len().to_string()
            };
            grouped_sections.push(
                div()
                    .px(space(4.))
                    .pt(space(18.))
                    .pb(space(8.))
                    .font_family(palette.heading_font.clone())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(label(9.5))
                    .text_color(palette.text_tertiary)
                    .child(format!("members · {count_label}"))
                    .into_any_element(),
            );
            for m in ungrouped {
                grouped_sections.push(self.member_row(m, &palette, cx));
            }
        }
        let hidden = total.saturating_sub(self.shown);
        let more = (hidden > 0).then(|| {
            div()
                .id("members-more")
                .mt(space(6.))
                .py(space(8.))
                .rounded(corner(14.))
                .text_center()
                .cursor_pointer()
                .text_size(text(12.))
                .text_color(palette.text_secondary)
                .hover(|d| d.bg(Hsla { a: 0.05, ..gpui::white() }))
                .child(format!("show {} more", hidden.min(PAGE)))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.shown += PAGE;
                    cx.notify();
                }))
        });

        let width = crate::views::shell::store_width(&self.store, crate::views::shell::Pane::InfoPanel, cx);
        div()
            .relative()
            .flex_none()
            .w(zpx(width))
            .child(crate::views::shell::resize_handle(crate::views::shell::Pane::InfoPanel, &palette))
            .h_full()
            .border_l_1()
            .border_color(palette.border_subtle)
            .bg(Hsla { a: 0.72, ..palette.app })
            .child(
                v_flex()
                    .id("room-info")
                    .size_full()
                    .overflow_y_scroll()
                    .pt(space(18.))
                    .px(space(16.))
                    .pb(space(24.))
                    .child(heading_part)
                    .child(action_strip)
                    .when(partner.is_none(), |d| {
                        d.children(room.topic.clone().map(|t| {
                            div().px(space(4.)).pb(space(16.)).text_size(text(12.5)).line_height(gpui::relative(1.55)).text_color(palette.text_secondary).child(t)
                        }))
                    })
                    .child(encryption_card(&room, unverified, total, &palette))
                    .children(self.error.clone().map(|e| div().mt(zpx(10.)).text_size(text(12.)).text_color(palette.danger).child(e)))
                    .children(grouped_sections)
                    .when(members.is_none(), |d| d.child(div().py(space(20.)).text_center().text_size(text(12.)).text_color(palette.text_tertiary).child("loading…")))
                    .children(more),
            )
            .child(
                div()
                    .id("info-close")
                    .absolute()
                    .top(zpx(14.))
                    .left(zpx(14.))
                    .size(zpx(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(palette.card)
                    .hover(|d| d.bg(palette.raised).text_color(palette.text))
                    .border_1()
                    .border_color(palette.border_subtle)
                    .text_size(text(11.))
                    .text_color(palette.text_secondary)
                    .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("close sidebar").build(window, cx))
                    .child("✕")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.store.update(cx, |s, cx| s.update_prefs(|p| p.show_info_panel = false, cx));
                    })),
            )
            .into_any_element()
    }
}
