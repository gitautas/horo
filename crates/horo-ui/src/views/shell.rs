//! The signed-in window: the spaces rail, the room list, and the room.

use gpui::{
    FocusHandle, AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    IconName,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use horo_core::dto::{RoomSummary, SyncState};

use matrix_sdk::ruma::OwnedRoomId;

use crate::{
    OpenSettings,
    presence,
    rooms::{self, View},
    store::{Phase, SettingsSection, Store},
    theme::Palette,
    views::{avatar::{avatar, with_presence}, composer::Composer, profile::{ProfileCard, same_target},
        settings::Settings, timeline::{Timeline, TimelineAction}, verification, lightbox},
};

pub struct Shell {
    store: Entity<Store>,
    /// The open room: which, its timeline and its composer.
    timeline: Option<(String, Entity<Timeline>, Entity<Composer>)>,
    /// Settings, while they're open.
    settings: Option<Entity<Settings>>,
    /// The profile card, while one is open.
    profile: Option<Entity<ProfileCard>>,
    /// Holds the window's focus when nothing inside does, so ⌘, still
    /// reaches the shell.
    focus: FocusHandle,
}

impl Shell {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Shell {
        cx.observe_in(&store, window, |this, _, window, cx| {
            this.follow_room(window, cx);
            this.follow_settings(window, cx);
            this.follow_profile(window, cx);
            cx.notify()
        })
        .detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut shell = Shell { store, timeline: None, settings: None, profile: None, focus };
        shell.follow_room(window, cx);
        shell.follow_settings(window, cx);
        shell
    }

    /// Open the selected room's timeline and composer, closing the last.
    fn follow_room(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let selected = store.room.clone();
        if self.timeline.as_ref().map(|(id, ..)| id) == selected.as_ref() {
            return;
        }
        let name = selected.as_ref().and_then(|id| store.rooms.iter().find(|r| r.id == *id)).map(|r| r.name.clone()).unwrap_or_default();
        if let Some((_, old, _)) = self.timeline.take() {
            old.read(cx).close(cx);
        }
        let Some(id) = selected else { return };
        let Ok(room_id) = OwnedRoomId::try_from(id.as_str()) else { return };
        let store = self.store.clone();
        let timeline = cx.new(|cx| Timeline::new(room_id.clone(), &store, cx));
        let weak = timeline.downgrade();
        let composer = cx.new(|cx| Composer::new(room_id, &name, weak, window, cx));
        // Reply and edit buttons on a message go to the composer under it.
        cx.subscribe_in(&timeline, window, |this, _, action: &TimelineAction, window, cx| {
            if let Some((_, _, composer)) = &this.timeline {
                composer.update(cx, |c, cx| c.take(action, window, cx));
            }
        })
        .detach();
        self.timeline = Some((id, timeline, composer));
    }

    /// Build the profile card for whoever the store says, or drop it.
    fn follow_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.store.read(cx).profile_card.clone() {
            Some(target) => {
                if !self.profile.as_ref().is_some_and(|card| same_target(card, &target, cx)) {
                    let store = self.store.clone();
                    self.profile = Some(cx.new(|cx| ProfileCard::new(store, target, window, cx)));
                }
            }
            None => self.profile = None,
        }
    }

    fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.show_settings(Some(SettingsSection::Account), cx));
    }

    /// Build settings when they open; drop them when they close.
    fn follow_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.store.read(cx).settings.is_some();
        if open && self.settings.is_none() {
            let store = self.store.clone();
            self.settings = Some(cx.new(|cx| Settings::new(store, window, cx)));
        } else if !open {
            self.settings = None;
        }
    }

    fn rail(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let open = store.space.clone();
        let home_selected = open.is_none();
        let spaces = store.spaces.clone();
        let item = |id: SharedString, selected: bool| {
            div()
                .id(id)
                .p(px(3.))
                .rounded(px(14.))
                .border_2()
                .border_color(if selected { palette.accent } else { gpui::transparent_black() })
                .cursor_pointer()
        };
        v_flex()
            .id("rail")
            .w(px(68.))
            .h_full()
            .flex_none()
            .items_center()
            .gap_2()
            .py_3()
            .bg(palette.app)
            .overflow_y_scroll()
            .child(
                item("home".into(), home_selected)
                    .child(
                        div()
                            .size(px(42.))
                            .rounded(px(12.))
                            .bg(palette.raised)
                            .flex()
                            .items_center()
                            .justify_center()
                            .font_family(palette.heading_font.clone())
                            .text_size(px(22.))
                            .text_color(palette.accent)
                            .child("h"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.open_space(None, cx)))),
            )
            .child(div().w(px(32.)).h(px(1.)).bg(palette.border))
            .children(spaces.into_iter().map(|space| {
                let id = space.id.clone();
                let unread = space.notification_count > 0 || space.highlight_count > 0;
                let picture = avatar(&space.id, &space.name, space.avatar_url.as_deref(), px(42.), true, palette.text_on_accent, cx);
                item(SharedString::from(format!("space-{}", space.id)), open.as_deref() == Some(&space.id))
                    .relative()
                    .child(picture)
                    .children(unread.then(|| {
                        div().absolute().top(px(0.)).right(px(0.)).size(px(10.)).rounded_full().bg(palette.accent_secondary)
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.store.update(cx, |s, cx| s.open_space(Some(id), cx))
                    }))
            }))
    }

    fn room_row(&self, room: &RoomSummary, selected: bool, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let id = room.id.clone();
        let unread = room.has_unread || room.is_marked_unread;
        let count = room.notification_count.max(room.highlight_count);
        let preview: Option<String> = if room.membership == horo_core::dto::RoomMembership::Invited {
            Some("invited you".into())
        } else {
            room.latest.as_ref().map(|l| {
                let who = l.sender_name.clone().unwrap_or_else(|| l.sender.clone());
                if room.is_direct { l.body.clone() } else { format!("{who}: {}", l.body) }
            })
        };
        h_flex()
            .id(SharedString::from(format!("room-{}", room.id)))
            .gap_2()
            .px_2()
            .py(px(6.))
            .rounded(palette.radius_md)
            .cursor_pointer()
            .when(selected, |row| row.bg(palette.raised))
            .hover(|row| row.bg(palette.raised))
            .child({
                // A DM is a person: their dot on the room's face.
                let shown = room.dm_user_id.as_ref().and_then(|user| {
                    let store = self.store.read(cx);
                    presence::shown(store.presence.get(user), store.presence_supported)
                });
                let face = avatar(&room.id, &room.name, room.avatar_url.as_deref(), px(32.), !room.is_direct, palette.text_on_accent, cx);
                with_presence(face, shown, if selected { palette.raised } else { palette.card }, palette)
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_color(if unread || selected { palette.text } else { palette.text_secondary })
                            .when(unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .child(room.name.clone()),
                    )
                    .children(preview.map(|p| div().truncate().text_xs().text_color(palette.text_tertiary).child(p.replace('\n', " ")))),
            )
            .children((count > 0).then(|| {
                div()
                    .flex_none()
                    .px(px(6.))
                    .min_w(px(18.))
                    .h(px(18.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .bg(if room.highlight_count > 0 { palette.accent_secondary } else { palette.accent })
                    .text_color(palette.text_on_accent)
                    .child(count.to_string())
            }))
            .children((unread && count == 0).then(|| div().flex_none().size(px(8.)).rounded_full().bg(palette.text_secondary)))
            .on_click(cx.listener(move |this, _, _, cx| {
                let id = id.clone();
                this.store.update(cx, |s, cx| s.open_room(id, cx))
            }))
    }

    fn room_list(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let space = store.current_space().cloned();
        let selected = store.room.clone();
        let title = space.as_ref().map_or_else(|| "home".to_owned(), |s| s.name.clone());
        let order = space.as_ref().map(|s| s.children.clone()).unwrap_or_default();
        let rooms = store.rooms.clone();
        let user = match &store.phase {
            Phase::SignedIn(info) => info.display_name.clone().unwrap_or_else(|| info.user_id.clone()),
            _ => String::new(),
        };
        let user_id = match &store.phase {
            Phase::SignedIn(info) => info.user_id.clone(),
            _ => String::new(),
        };
        let sync = store.sync;
        let user_avatar = match &store.phase {
            Phase::SignedIn(info) => info.avatar_url.clone(),
            _ => None,
        };

        let visible = rooms::filter(&rooms, &View { space: space.as_ref(), ..Default::default() });
        let groups = rooms::group(visible, &order);
        let mut list = v_flex().id("room-list").flex_1().min_h_0().overflow_y_scroll().px_2().pb_2().gap(px(2.));
        for (section, rooms) in &groups {
            list = list.child(
                div()
                    .pt_3()
                    .pb_1()
                    .px_2()
                    .font_family(palette.heading_font.clone())
                    .text_size(px(15.))
                    .text_color(palette.text_tertiary)
                    .child(section.title()),
            );
            for room in rooms {
                let is_selected = selected.as_deref() == Some(room.id.as_str());
                list = list.child(self.room_row(room, is_selected, palette, cx));
            }
        }
        if groups.is_empty() {
            list = list.child(div().p_4().text_sm().text_color(palette.text_tertiary).child(if rooms.is_empty() { "loading rooms…" } else { "nothing here yet" }));
        }

        let status = match sync {
            Some(SyncState::Running) | None => None,
            Some(SyncState::Offline) => Some("offline"),
            Some(SyncState::Error) => Some("connection trouble"),
            Some(SyncState::Idle | SyncState::Terminated) => Some("connecting…"),
        };

        v_flex()
            .w(px(272.))
            .h_full()
            .flex_none()
            .bg(palette.card)
            .border_r_1()
            .border_color(palette.border_subtle)
            .child(
                div()
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .font_family(palette.heading_font.clone())
                    .text_size(px(24.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.text)
                    .truncate()
                    .child(title),
            )
            .children(status.map(|s| div().px_4().text_xs().text_color(palette.idle).child(s)))
            .child(list)
            .child(
                h_flex()
                    .gap_2()
                    .p_2()
                    .border_t_1()
                    .border_color(palette.border_subtle)
                    .child(avatar(&user_id, &user, user_avatar.as_deref(), px(32.), false, palette.text_on_accent, cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().text_sm().text_color(palette.text).child(user.clone()))
                            .child(div().truncate().text_xs().text_color(palette.text_tertiary).child(user_id.clone())),
                    )
                    .child(
                        Button::new("settings")
                            .ghost()
                            .icon(IconName::Settings)
                            .tooltip(if cfg!(target_os = "macos") { "settings (⌘,)" } else { "settings (ctrl+,)" })
                            .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
                    ),
            )
    }

    fn room_pane(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let room = store.room.as_ref().and_then(|id| store.rooms.iter().find(|r| r.id == *id)).cloned();
        let body = match (&room, &self.timeline) {
            (Some(_), Some((_, timeline, composer))) => {
                v_flex().flex_1().min_h_0().child(div().flex_1().min_h_0().child(timeline.clone())).child(composer.clone())
            }
            _ => v_flex().m_auto().text_color(palette.text_tertiary).child("pick a room"),
        };
        v_flex()
            .flex_1()
            .h_full()
            .bg(palette.app)
            .children(room.map(|room| {
                h_flex()
                    .gap_3()
                    .px_4()
                    .h(px(52.))
                    .flex_none()
                    .border_b_1()
                    .border_color(palette.border_subtle)
                    .child(div().text_color(palette.text).font_weight(FontWeight::SEMIBOLD).child(room.name.clone()))
                    .children(room.topic.map(|t| div().flex_1().min_w_0().truncate().text_sm().text_color(palette.text_tertiary).child(t)))
            }))
            .children(verification::banner(&self.store, palette, cx))
            .child(body)
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        h_flex()
            .id("shell")
            .track_focus(&self.focus)
            // ⌘, from anywhere in the window, the menu included. Already
            // open, it stays where it is.
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| {
                if this.store.read(cx).settings.is_none() {
                    this.open_settings(cx);
                }
            }))
            .size_full()
            .bg(palette.app)
            .text_color(palette.text)
            .font_family(palette.body_font.clone())
            .child(self.rail(&palette, cx))
            .child(self.room_list(&palette, cx))
            .child(self.room_pane(&palette, cx))
            .relative()
            .children(self.settings.clone())
            .children(lightbox::lightbox(&self.store, &palette, cx))
            .children(self.profile.clone())
            // Above everything, the security panel included: the other
            // device's timeout is running, and a dialog hidden behind
            // another one is a verification that fails.
            .children(verification::dialog(&self.store, &palette, cx))
    }
}
