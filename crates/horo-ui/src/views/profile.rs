//! Someone's profile card, as ProfileCard.tsx draws it: a popover where
//! they were clicked, with their cover, avatar, presence, status, bio and
//! the rooms you share, and a way to message them, verify them, or (on your
//! own card) edit your profile. Click away or Escape closes it.

use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, Context, Entity, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, StyledImage, Window, anchored, deferred, div, img, prelude::FluentBuilder,
};
use gpui_component::{
    Disableable,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use horo_core::{
    dto::Verification,
    matrix::profile::{self, ProfileDto, UserContextDto},
    verification,
};

use crate::theme::zpx;
use crate::{
    backend::Backend,
    media::Media,
    presence,
    store::{ProfileTarget, SettingsSection, Store},
    theme::Palette,
    views::avatar::{avatar, presence_dot},
};

const WIDTH: f32 = 320.;
const COVER: f32 = 88.;
const FACE: f32 = 66.;
/// The card's corner radius.
const CORNER: f32 = 20.;

pub struct ProfileCard {
    store: Entity<Store>,
    pub target: ProfileTarget,
    focus: FocusHandle,
    profile: Option<ProfileDto>,
    context: Option<UserContextDto>,
    busy: bool,
    error: Option<SharedString>,
}

impl ProfileCard {
    pub fn new(store: Entity<Store>, target: ProfileTarget, window: &mut Window, cx: &mut Context<Self>) -> ProfileCard {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        cx.observe(&store, |_, _, cx| cx.notify()).detach();

        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, user) = (backend.state.clone(), target.user_id.clone());
            async move {
                let core = state.core().await?;
                // Either can fail alone (a server that 404s profiles); the
                // card shows what it got.
                let profile = profile::get_profile(&core, Some(user.clone())).await.ok();
                let context = profile::get_user_context(&core, user).await.ok();
                Ok::<_, horo_core::Error>((profile, context))
            }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok((profile, context))) = task.await {
                let _ = this.update(cx, |card, cx| {
                    card.profile = profile;
                    card.context = context;
                    cx.notify();
                });
            }
        })
        .detach();

        ProfileCard { store, target, focus, profile: None, context: None, busy: false, error: None }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.close_profile(cx));
    }

    fn jump_to(&mut self, room_id: String, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            s.close_profile(cx);
            s.open_room(room_id, cx);
        });
    }

    /// Open (or make) the DM and go there.
    fn message(&mut self, cx: &mut Context<Self>) {
        self.busy = true;
        self.error = None;
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, user) = (backend.state.clone(), self.target.user_id.clone());
            async move { profile::open_dm(&*state.core().await?, user).await }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |card, cx| {
                card.busy = false;
                match outcome {
                    Ok(Ok(room_id)) => card.jump_to(room_id, cx),
                    Ok(Err(e)) => card.error = Some(e.to_string().into()),
                    Err(e) => card.error = Some(e.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Ask them to verify, in your DM; the dialog follows the request.
    fn verify(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let task = backend.run({
            let (host, state, user) = (backend.host.clone(), backend.state.clone(), self.target.user_id.clone());
            async move { verification::request(&host, &state.core().await?, Some(user)).await }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            let _ = this.update(cx, |card, cx| match error {
                None => card.close(cx),
                Some(e) => {
                    card.error = Some(format!("couldn't start verifying: {e}").into());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn header(&mut self, name: &str, verified: bool, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let user = self.target.user_id.clone();
        let profile = self.profile.clone();
        let cover = profile.as_ref().and_then(|p| p.cover_url.clone()).and_then(|mxc| Media::image(&mxc, Some(((WIDTH * 2.) as u32, (COVER * 2.) as u32)), cx));
        // The card's top corners, inside its border: its scroll clips to a
        // rectangle, so the cover rounds itself.
        let top = zpx(CORNER - 1.);
        let cover = div().h(zpx(COVER)).w_full().rounded_t(top).bg(palette.raised).map(|d| match cover {
            // The frame's shape, not the photo's: see `avatar`.
            Some(image) => d.child(img(image).size_full().aspect_ratio(WIDTH / COVER).rounded_t(top).object_fit(gpui::ObjectFit::Cover)),
            None => d,
        });

        let shown = {
            let store = self.store.read(cx);
            presence::shown(store.presence.get(&user), store.presence_supported)
        };
        let mxc = profile.as_ref().and_then(|p| p.avatar_url.clone()).or_else(|| self.target.avatar_hint.clone());
        let picture = avatar(&user, name, mxc.as_deref(), zpx(FACE), cx);
        let face = div()
            .relative()
            .p(zpx(3.))
            .rounded(zpx(FACE / 3. + 3.))
            .bg(palette.card)
            .child(picture)
            .children(shown.map(|s| presence_dot(s, zpx(16.), palette.card, palette)));

        v_flex()
            .child(cover)
            .child(
                h_flex()
                    .px_4()
                    .mt(zpx(-FACE / 2.))
                    .items_end()
                    .justify_between()
                    .child(face)
                    .when(verified, |row| {
                        row.child(
                            div()
                                .mb_1()
                                .px_2()
                                .py(zpx(2.))
                                .rounded_full()
                                .bg(Hsla { a: 0.15, ..palette.online })
                                .text_xs()
                                .text_color(palette.online)
                                .child("verified"),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl Render for ProfileCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        let user = self.target.user_id.clone();
        let name = self.profile.as_ref().and_then(|p| p.display_name.clone()).unwrap_or_else(|| user.clone());
        let verified = self.context.as_ref().is_some_and(|c| c.verification == Verification::Verified);
        let is_me = self.context.as_ref().is_some_and(|c| c.is_me);

        let header = self.header(&name, verified, &palette, cx);

        let presence_line = {
            let store = self.store.read(cx);
            let dto = store.presence.get(&user).cloned();
            presence::shown(dto.as_ref(), store.presence_supported).zip(dto).map(|(shown, dto)| {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or_default();
                h_flex()
                    .gap_2()
                    .font_family(palette.mono_font.clone())
                    .text_xs()
                    .text_color(palette.text_tertiary)
                    .child(div().size(zpx(7.)).rounded_full().bg(match shown {
                        presence::Shown::Online => palette.online,
                        presence::Shown::Away => palette.idle,
                        presence::Shown::Offline => palette.offline,
                    }))
                    .child(presence::line(&dto, shown, now))
            })
        };

        let status = self.profile.as_ref().and_then(|p| p.status.clone());
        let bio = self.profile.as_ref().and_then(|p| p.bio.clone());
        let shared = self.context.as_ref().map(|c| c.shared_rooms.clone()).unwrap_or_default();
        let label = |text: &'static str| div().pt_2().font_family(palette.heading_font.clone()).text_size(zpx(14.)).text_color(palette.text_tertiary).child(text);

        let mut rooms = Vec::new();
        for room in shared.iter().take(8) {
            let id = room.id.clone();
            let face = avatar(&room.id, &room.name, room.avatar_url.as_deref(), zpx(20.), cx);
            rooms.push(
                h_flex()
                    .id(SharedString::from(format!("shared-{}", room.id)))
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(palette.radius_md)
                    .cursor_pointer()
                    .hover(|d| d.bg(palette.raised))
                    .child(face)
                    .child(div().truncate().text_sm().child(room.name.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| this.jump_to(id.clone(), cx)))
                    .into_any_element(),
            );
        }

        // Held back until the context lands, so nobody sees "message" on
        // their own card for a moment.
        let actions = self.context.as_ref().map(|context| {
            if is_me {
                h_flex().child(Button::new("profile-edit").ghost().label("edit your profile").on_click(cx.listener(|this, _, _, cx| {
                    this.store.update(cx, |s, cx| {
                        s.close_profile(cx);
                        s.show_settings(Some(SettingsSection::Account), cx);
                    })
                })))
            } else {
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("profile-message")
                            .primary()
                            .label(if context.dm_room_id.is_some() { "open dm" } else { "message" })
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.message(cx))),
                    )
                    .when(!verified, |row| row.child(Button::new("profile-verify").ghost().label("verify").on_click(cx.listener(|this, _, _, cx| this.verify(cx)))))
            }
        });

        let loading = self.profile.is_none() && self.context.is_none();
        let card = v_flex()
            .id("profile-card")
            .track_focus(&self.focus)
            .occlude()
            .w(zpx(WIDTH))
            .max_h(zpx(560.))
            .overflow_y_scroll()
            .rounded(zpx(CORNER))
            .bg(palette.card)
            .border_1()
            .border_color(palette.border_strong)
            .shadow_lg()
            .text_color(palette.text)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close(cx)))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close(cx);
                    cx.stop_propagation();
                }
            }))
            .child(header)
            .child(
                v_flex()
                    .px_4()
                    .pb_4()
                    .pt_2()
                    .gap_1()
                    .child(div().font_family(palette.heading_font.clone()).text_size(zpx(22.)).font_weight(FontWeight::SEMIBOLD).child(name))
                    .child(div().text_xs().text_color(palette.text_tertiary).child(user.clone()))
                    .children(presence_line)
                    .children(status.map(|s| div().pt_1().text_sm().text_color(palette.text_secondary).child(s)))
                    .when(loading, |c| c.child(div().py_3().text_sm().text_color(palette.text_tertiary).child("loading…")))
                    .when_some(bio, |c, bio| c.child(label("about")).child(div().text_sm().text_color(palette.text_secondary).child(bio)))
                    .when(!rooms.is_empty(), |c| {
                        c.child(label(if shared.len() == 1 { "1 shared room" } else { "shared rooms" })).children(rooms)
                    })
                    .children(self.error.clone().map(|e| div().text_sm().text_color(palette.danger).child(e)))
                    .children(actions.map(|a| a.pt_3())),
            );

        // Anchored where they were clicked, kept inside the window.
        deferred(anchored().position(self.target.at).snap_to_window_with_margin(zpx(12.)).child(card)).with_priority(1)
    }
}

/// Whether this card is already showing `target`.
pub fn same_target(card: &Entity<ProfileCard>, target: &ProfileTarget, cx: &gpui::App) -> bool {
    card.read(cx).target == *target
}
