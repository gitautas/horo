//! The signed-in window: the spaces rail, the room list, and the room.

use gpui::{
    AnyElement, Hsla, img, StyledImage, FocusHandle, AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    Icon, IconName,
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use horo_core::dto::{RoomSummary, SyncState};

use matrix_sdk::ruma::OwnedRoomId;

use crate::{
    OpenSettings,
    presence,
    media::Media,
    rooms::{self, Filter, View},
    store::{Phase, SettingsSection, Store},
    theme::{Palette, corner, heading, label, space, text},
    views::{avatar::{accent_for, avatar, initials, presence_dot, with_presence}, composer::Composer, profile::{ProfileCard, same_target},
        settings::Settings, timeline::{Timeline, TimelineAction}, verification, lightbox, voice},
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
    /// The room list's search box and filter.
    search: Entity<InputState>,
    filter: Filter,
    filters_open: bool,
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
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("find a room or friend~"));
        cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        let mut shell = Shell { store, timeline: None, settings: None, profile: None, focus, search, filter: Filter::All, filters_open: false };
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
        let (name, encrypted) = selected
            .as_ref()
            .and_then(|id| store.rooms.iter().find(|r| r.id == *id))
            .map(|r| (r.name.clone(), r.is_encrypted))
            .unwrap_or_default();
        if let Some((_, old, _)) = self.timeline.take() {
            old.read(cx).close(cx);
        }
        let Some(id) = selected else { return };
        let Ok(room_id) = OwnedRoomId::try_from(id.as_str()) else { return };
        let store = self.store.clone();
        let timeline = cx.new(|cx| Timeline::new(room_id.clone(), &store, cx));
        let weak = timeline.downgrade();
        let composer = cx.new(|cx| Composer::new(room_id, &name, encrypted, weak, window, cx));
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

    /// The spaces rail, as SpacesRail.tsx: the wordmark, "all", the
    /// spaces, and settings and you at the foot.
    fn rail(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let open = store.space.clone();
        let spaces = store.spaces.clone();
        let me = match &store.phase {
            Phase::SignedIn(info) => Some(info.clone()),
            _ => None,
        };
        let away = store.is_away();

        let all = space_tile("space-all", "all", None, open.is_none(), palette.accent, 0, palette, cx)
            .on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.open_space(None, cx))));
        let mut tiles = Vec::new();
        for space in spaces {
            let id = space.id.clone();
            let badge = space.notification_count;
            let tile = space_tile(
                SharedString::from(format!("space-{}", space.id)),
                &initials(&space.name),
                space.avatar_url.as_deref(),
                open.as_deref() == Some(&space.id),
                accent_for(&space.id),
                badge,
                palette,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let id = id.clone();
                this.store.update(cx, |s, cx| s.open_space(Some(id), cx))
            }));
            tiles.push(tile);
        }

        let me_face = me.map(|info| {
            let name = info.display_name.clone().unwrap_or_else(|| info.user_id.clone());
            let face = avatar(&info.user_id, &name, info.avatar_url.as_deref(), px(38.), cx);
            let shown = if away { presence::Shown::Away } else { presence::Shown::Online };
            with_presence(face, Some(shown), palette.card, palette)
        });

        v_flex()
            .id("rail")
            .relative()
            .w(px(76.))
            .h_full()
            .flex_none()
            .items_center()
            .gap(space(14.))
            .pt(px(44.))
            .pb(px(16.))
            .bg(palette.card)
            .border_r_1()
            .border_color(palette.border_subtle)
            // The window's title bar is gone; its top is where you drag it.
            .child(drag_strip(px(40.)))
            .child(img(logo()).w(px(18. * 830.3 / 295.9)).h(px(18.)).flex_none())
            .child(div().w(px(26.)).h(px(2.)).rounded_full().bg(palette.border).flex_none())
            .child(all)
            .child(
                v_flex()
                    .id("rail-spaces")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .items_center()
                    .gap(space(12.))
                    .overflow_y_scroll()
                    .children(tiles),
            )
            .child(
                v_flex()
                    .items_center()
                    .gap(space(12.))
                    .child(
                        div()
                            .id("rail-settings")
                            .cursor_pointer()
                            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new(if cfg!(target_os = "macos") { "settings (⌘,)" } else { "settings (ctrl+,)" }).build(window, cx))
                            .child(Icon::new(IconName::Settings).size(px(20.)).text_color(palette.text_tertiary))
                            .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
                    )
                    .children(me_face),
            )
    }

    /// A room in the list, as RoomRow in RoomList.tsx.
    fn room_row(&self, room: &RoomSummary, selected: bool, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let id = room.id.clone();
        let voice = room.is_video_room && room.membership == horo_core::dto::RoomMembership::Joined;
        let accent = accent_for(&room.id);
        let preview = preview_for(room);
        let stamp = rooms::stamp(room.latest.as_ref().map_or(0, |l| l.timestamp), chrono::Local::now());

        // The face, with what kind of room it is in one corner and the
        // person's dot (or a live call, the more urgent) in the other.
        let shown = room.dm_user_id.as_ref().filter(|_| !room.has_active_call).and_then(|user| {
            let store = self.store.read(cx);
            presence::shown(store.presence.get(user), store.presence_supported)
        });
        let picture = avatar(&room.id, &room.name, room.avatar_url.as_deref(), px(36.), cx);
        let face = div()
            .relative()
            .flex_none()
            .child(picture)
            .when(!room.is_direct && !room.is_space, |d| d.child(channel_badge(room.is_video_room, 16., palette.card, palette)))
            .children(shown.map(|s| presence_dot(s, px(12.), palette.app, palette)))
            .when(room.has_active_call, |d| {
                d.child(
                    div()
                        .absolute()
                        .right(px(-3.))
                        .bottom(px(-3.))
                        .size(px(14.))
                        .rounded_full()
                        .bg(palette.online)
                        .border_2()
                        .border_color(palette.card)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(voice::icon("icons/phone-call.svg").size(px(7.)).text_color(palette.app)),
                )
            });

        let name = h_flex()
            .gap(space(5.))
            .min_w_0()
            .child(
                div()
                    .truncate()
                    .font_weight(FontWeight::BOLD)
                    .text_size(text(14.5))
                    .text_color(if room.has_unread { palette.text } else { palette.text_secondary })
                    .child(room.name.clone()),
            )
            .when(room.is_encrypted, |d| d.child(voice::icon("icons/shield-check.svg").size(px(12.)).text_color(palette.accent).flex_none()))
            .when(room.is_muted, |d| d.child(voice::icon("icons/bell-off.svg").size(px(12.)).text_color(palette.text_tertiary).flex_none()));

        let badge = room.has_unread.then(|| {
            let count = room.notification_count;
            div()
                .min_w(px(19.))
                .h(px(19.))
                .px(space(6.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(text(9.5))
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(palette.text_on_accent)
                .bg(if room.highlight_count > 0 { palette.accent_secondary } else { palette.accent })
                .child(match count {
                    0 => "·".to_owned(),
                    n if n > 99 => "99+".to_owned(),
                    n => n.to_string(),
                })
        });

        h_flex()
            .id(SharedString::from(format!("room-{}", room.id)))
            .gap(space(10.))
            .px(space(10.))
            .py(space(9.))
            .mb(px(2.))
            .rounded(corner(16.))
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(Hsla { a: 0.07, ..gpui::white() }).shadow(vec![gpui::BoxShadow {
                    color: accent,
                    offset: gpui::point(px(3.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(0.),
                    inset: true,
                }])
            })
            .when(!selected, |row| row.hover(|row| row.bg(Hsla { a: 0.05, ..gpui::white() })))
            .child(face)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(name)
                    .child(div().mt(px(1.)).truncate().text_size(text(12.)).text_color(palette.text_tertiary).child(preview.replace('\n', " "))),
            )
            .child(
                v_flex()
                    .flex_none()
                    .items_end()
                    .gap(space(4.))
                    .child(div().font_family(palette.mono_font.clone()).text_size(text(10.)).text_color(palette.text_tertiary).child(stamp))
                    .children(badge),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let id = id.clone();
                this.store.update(cx, |s, cx| {
                    s.open_room(id.clone(), cx);
                    // A voice room is a call you walk into, as in Discord:
                    // clicking it joins (already in it, nothing changes).
                    if voice {
                        s.join_call(id, cx);
                    }
                })
            }))
    }

    /// The room list, as RoomList.tsx: the space's name, how much is
    /// unread, search and filters, then the sections.
    fn room_list(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let open_space = store.current_space().cloned();
        let selected = store.room.clone();
        let title = open_space.as_ref().map_or_else(|| "everything".to_owned(), |s| s.name.to_lowercase());
        let order = open_space.as_ref().map(|s| s.children.clone()).unwrap_or_default();
        let rooms = store.rooms.clone();
        let sync = store.sync;

        let search = self.search.read(cx).value().to_string();
        let visible = rooms::filter(&rooms, &View { space: open_space.as_ref(), filter: self.filter, search: &search, ..Default::default() });
        let unread_total: u64 = visible.iter().map(|r| r.notification_count).sum();
        let groups = rooms::group(visible, &order);

        let mut list = v_flex().id("room-list").flex_1().min_h_0().overflow_y_scroll().px(space(10.)).pb(space(16.));
        for (section, rooms) in &groups {
            list = list.child(
                h_flex()
                    .gap(space(8.))
                    .px(space(8.))
                    .pt(space(14.))
                    .pb(space(6.))
                    .font_family(palette.heading_font.clone())
                    .text_size(label(9.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.text_tertiary)
                    .child(section.title())
                    .child(div().flex_1().h(px(1.)).bg(palette.border_subtle))
                    .child(rooms.len().to_string()),
            );
            for room in rooms {
                let is_selected = selected.as_deref() == Some(room.id.as_str());
                list = list.child(self.room_row(room, is_selected, palette, cx));
                if room.is_video_room {
                    list = list.children(voice::roster(room, &self.store, palette, cx));
                }
            }
        }
        if groups.is_empty() {
            let empty = if !search.trim().is_empty() {
                "nothing matches that~".to_owned()
            } else if self.filter != Filter::All {
                format!("no {} rooms right now", filter_name(self.filter))
            } else if rooms.is_empty() {
                "loading rooms…".to_owned()
            } else if open_space.is_some() {
                "nothing here yet — this space has no rooms you've joined".to_owned()
            } else {
                "no rooms yet~".to_owned()
            };
            list = list.child(div().px(space(16.)).py(space(40.)).text_center().text_size(text(13.)).text_color(palette.text_tertiary).child(empty));
        }

        let status = match sync {
            Some(SyncState::Running) | None => None,
            Some(SyncState::Offline) => Some("offline"),
            Some(SyncState::Error) => Some("connection trouble"),
            Some(SyncState::Idle | SyncState::Terminated) => Some("connecting…"),
        };
        let caught_up = match status {
            Some(status) => status.to_owned(),
            None if unread_total > 0 => format!("{unread_total} unread"),
            None => "all caught up".to_owned(),
        };

        let filtered = self.filter != Filter::All;
        let search_box = h_flex()
            .mt(px(12.))
            .gap(space(8.))
            .px(space(14.))
            .py(space(6.))
            .rounded_full()
            .bg(palette.inset)
            .border_1()
            .border_color(palette.border_subtle)
            .child(Icon::new(IconName::Search).size(px(14.)).text_color(palette.text_tertiary))
            .child(div().flex_1().text_size(text(13.)).child(Input::new(&self.search).appearance(false)))
            .child(
                div()
                    .id("room-filters")
                    .relative()
                    .cursor_pointer()
                    .child(voice::icon("icons/funnel.svg").size(px(15.)).text_color(if filtered || self.filters_open { palette.accent } else { palette.text_tertiary }))
                    .when(filtered && !self.filters_open, |d| {
                        d.child(div().absolute().top(px(-2.)).right(px(-2.)).size(px(6.)).rounded_full().bg(palette.accent))
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filters_open = !this.filters_open;
                        cx.notify();
                    })),
            );
        let chips = self.filters_open.then(|| {
            h_flex().gap(space(6.)).mt(px(12.)).children([Filter::All, Filter::Unread, Filter::Dms, Filter::Muted].into_iter().map(|f| {
                let on = self.filter == f;
                div()
                    .id(SharedString::from(format!("filter-{}", filter_name(f))))
                    .px(space(11.))
                    .py(space(6.))
                    .rounded(corner(12.))
                    .cursor_pointer()
                    .text_size(text(9.5))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .border_1()
                    .map(|d| if on { d.bg(palette.accent).text_color(palette.text_on_accent).border_color(palette.app) } else { d.bg(palette.raised).text_color(palette.text_tertiary).border_color(palette.border_subtle) })
                    .child(filter_name(f).to_uppercase())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = f;
                        cx.notify();
                    }))
            }))
        });

        v_flex()
            .relative()
            .w(px(298.))
            .h_full()
            .flex_none()
            .bg(palette.card)
            .border_r_1()
            .border_color(palette.border_subtle)
            .child(drag_strip(px(18.)))
            .child(
                v_flex()
                    .px(space(18.))
                    .pt(space(18.))
                    .pb(space(12.))
                    .child(
                        h_flex()
                            .justify_between()
                            .items_end()
                            .gap(space(8.))
                            .child(
                                div()
                                    .truncate()
                                    .font_family(palette.heading_font.clone())
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .text_size(heading(22.))
                                    .text_color(palette.text)
                                    .child(format!("{title}~")),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .pb(px(4.))
                                    .font_family(palette.mono_font.clone())
                                    .text_size(text(11.))
                                    .text_color(if status.is_some() { palette.idle } else { palette.text_tertiary })
                                    .child(caught_up),
                            ),
                    )
                    .child(search_box)
                    .children(chips),
            )
            .child(list)
            .children(voice::panel(&self.store, palette, cx))
    }

    /// The top of a room, as ChatPane.tsx: its face, name and lock, the
    /// person's presence (a DM) or the topic and alias, then the member
    /// count and the call button.
    fn room_header(&self, room: &RoomSummary, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let shown = room.dm_user_id.as_ref().and_then(|user| {
            let store = self.store.read(cx);
            presence::shown(store.presence.get(user), store.presence_supported).zip(store.presence.get(user).cloned())
        });
        let picture = avatar(&room.id, &room.name, room.avatar_url.as_deref(), px(42.), cx);
        let face = div()
            .relative()
            .flex_none()
            .child(picture)
            .when(!room.is_direct, |d| d.child(channel_badge(room.is_video_room, 18., palette.app, palette)))
            .children(shown.as_ref().map(|(s, _)| presence_dot(*s, px(13.), palette.app, palette)));

        let subtitle: Option<AnyElement> = if room.dm_user_id.is_some() {
            shown.map(|(state, dto)| {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or_default();
                h_flex()
                    .mt(px(3.))
                    .gap(space(6.))
                    .font_family(palette.mono_font.clone())
                    .text_size(text(11.5))
                    .text_color(palette.text_tertiary)
                    .child(div().size(px(7.)).rounded_full().flex_none().bg(match state {
                        presence::Shown::Online => palette.online,
                        presence::Shown::Away => palette.idle,
                        presence::Shown::Offline => palette.offline,
                    }))
                    .child(presence::line(&dto, state, now))
                    .into_any_element()
            })
        } else {
            (room.topic.is_some() || room.canonical_alias.is_some()).then(|| {
                h_flex()
                    .gap(space(6.))
                    .max_w(px(520.))
                    .min_w_0()
                    .children(room.topic.clone().map(|t| div().truncate().text_size(text(12.5)).text_color(palette.text_secondary).child(t.replace('\n', " "))))
                    .children(room.canonical_alias.clone().map(|a| {
                        div().flex_none().font_family(palette.mono_font.clone()).text_size(text(11.5)).text_color(palette.text_tertiary).child(a)
                    }))
                    .into_any_element()
            })
        };

        let in_this_call = self.store.read(cx).call.as_ref().is_some_and(|c| c.room_id == room.id && c.is_live());
        let highlight = in_this_call || room.has_active_call;
        let (store, room_id) = (self.store.clone(), room.id.clone());
        let call_button = div()
            .id("header-call")
            .p(space(9.))
            .rounded(corner(12.))
            .cursor_pointer()
            .bg(if in_this_call { palette.danger } else { palette.raised })
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(if in_this_call { "leave the call" } else { "start a voice call" }).build(window, cx)
            })
            .child(voice::icon(if in_this_call { "icons/phone-off.svg" } else { "icons/phone-call.svg" }).size(px(17.)).text_color(
                if in_this_call {
                    palette.app
                } else if highlight {
                    palette.accent
                } else {
                    palette.text_secondary
                },
            ))
            .on_click(move |_, _, cx| {
                let room_id = room_id.clone();
                store.update(cx, |s, cx| if in_this_call { s.leave_call(cx) } else { s.join_call(room_id, cx) })
            });

        h_flex()
            .relative()
            .flex_none()
            .gap(space(14.))
            .px(space(22.))
            .py(space(14.))
            .border_b_1()
            .border_color(palette.border_subtle)
            .bg(Hsla { a: 0.6, ..palette.card })
            .child(drag_strip(px(12.)))
            .child(face)
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap(space(8.))
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .font_family(palette.heading_font.clone())
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .text_size(heading(19.))
                                    .child(room.name.clone()),
                            )
                            .when(room.is_encrypted, |d| d.child(voice::icon("icons/lock.svg").size(px(14.)).text_color(palette.accent).flex_none())),
                    )
                    .children(subtitle),
            )
            .child(
                h_flex()
                    .ml_auto()
                    .gap(space(8.))
                    .child(div().mr(px(4.)).font_family(palette.mono_font.clone()).text_size(text(11.)).text_color(palette.text_tertiary).child(room.member_count.to_string()))
                    .child(call_button),
            )
            .into_any_element()
    }

    fn room_pane(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let room = store.room.as_ref().and_then(|id| store.rooms.iter().find(|r| r.id == *id)).cloned();
        let strip = room.as_ref().and_then(|r| voice::strip(&r.id, &self.store, palette, cx));
        let header = room.as_ref().map(|r| self.room_header(r, palette, cx));
        let body = match (&room, &self.timeline) {
            (Some(_), Some((_, timeline, composer))) => v_flex()
                .flex_1()
                .min_h_0()
                .children(strip)
                .child(div().flex_1().min_h_0().child(timeline.clone()))
                .child(composer.clone()),
            _ => v_flex().m_auto().text_color(palette.text_tertiary).child("pick a room"),
        };
        v_flex()
            .flex_1()
            .h_full()
            .bg(palette.app)
            .children(header)
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
            .children(voice::ringing(&self.store, &palette, cx))
            .children(self.profile.clone())
            // Above everything, the security panel included: the other
            // device's timeout is running, and a dialog hidden behind
            // another one is a verification that fails.
            .children(verification::dialog(&self.store, &palette, cx))
    }
}

/// An empty strip at the top of a column that drags the window, since the
/// title bar is hidden; a double click zooms, as a title bar's would.
fn drag_strip(height: gpui::Pixels) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(height)
        .on_mouse_down(gpui::MouseButton::Left, |event: &gpui::MouseDownEvent, window, _| {
            if event.click_count >= 2 {
                window.titlebar_double_click();
            } else {
                window.start_window_move();
            }
        })
}

fn logo() -> std::sync::Arc<gpui::Image> {
    static LOGO: std::sync::OnceLock<std::sync::Arc<gpui::Image>> = std::sync::OnceLock::new();
    LOGO.get_or_init(|| std::sync::Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Svg, crate::LOGO.to_vec()))).clone()
}

/// A tile in the rail: a space's picture, or its initials on its accent
/// when it's the open one (on the raised surface otherwise).
#[allow(clippy::too_many_arguments)]
fn space_tile(
    id: impl Into<gpui::ElementId>,
    label_text: &str,
    mxc: Option<&str>,
    active: bool,
    accent: Hsla,
    badge: u64,
    palette: &Palette,
    cx: &mut gpui::App,
) -> gpui::Stateful<gpui::Div> {
    let picture = mxc.and_then(|mxc| Media::image(mxc, Some((92, 92)), cx));
    let tile = div()
        .size(px(46.))
        .rounded(corner(16.))
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .text_size(text(13.))
        .font_weight(FontWeight::EXTRA_BOLD)
        .map(|d| {
            if active {
                d.bg(if picture.is_some() { gpui::transparent_black() } else { accent })
                    .text_color(palette.text_on_accent)
                    .border_2()
                    .border_color(palette.app)
                    .shadow(vec![gpui::BoxShadow {
                        color: Hsla { h: 0., s: 0., l: 0., a: 0.28 },
                        offset: gpui::point(px(0.), px(2.)),
                        blur_radius: px(0.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
            } else {
                d.bg(palette.raised).text_color(palette.text_secondary).border_1().border_color(palette.border_subtle)
            }
        })
        .map(|d| match picture {
            Some(image) => d.child(img(image).size_full().object_fit(gpui::ObjectFit::Cover)),
            None => d.child(label_text.to_uppercase()),
        });
    div()
        .id(id)
        .relative()
        .flex_none()
        .cursor_pointer()
        .child(tile)
        .when(badge > 0, |d| {
            d.child(
                div()
                    .absolute()
                    .top(px(-4.))
                    .right(px(-4.))
                    .min_w(px(18.))
                    .h(px(18.))
                    .px(space(5.))
                    .rounded_full()
                    .bg(palette.accent_secondary)
                    .text_color(palette.text_on_accent)
                    .border_2()
                    .border_color(palette.card)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(text(9.))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(if badge > 99 { "99+".to_owned() } else { badge.to_string() }),
            )
        })
}

/// What kind of room this is, in the avatar's corner: `#` for text, a
/// camera for a video (voice) room. DMs get none.
fn channel_badge(video: bool, size: f32, ring: Hsla, palette: &Palette) -> impl IntoElement {
    div()
        .absolute()
        .left(px(-4.))
        .bottom(px(-4.))
        .size(px(size))
        .rounded_full()
        .bg(if video { palette.accent_tertiary } else { palette.inset })
        .border_2()
        .border_color(ring)
        .flex()
        .items_center()
        .justify_center()
        .map(|d| {
            if video {
                d.child(voice::icon("icons/video.svg").size(px(size * 0.5)).text_color(palette.app))
            } else {
                d.child(div().text_size(px(size * 0.62)).font_weight(FontWeight::EXTRA_BOLD).text_color(palette.text_secondary).child("#"))
            }
        })
}

/// The preview line, as `previewFor`.
fn preview_for(room: &RoomSummary) -> String {
    if room.membership == horo_core::dto::RoomMembership::Invited {
        return "invited you~".into();
    }
    match &room.latest {
        None => room.topic.clone().unwrap_or_else(|| "no messages yet".into()),
        Some(latest) => {
            if room.is_direct {
                latest.body.clone()
            } else {
                let who = latest.sender_name.clone().unwrap_or_else(|| latest.sender.trim_start_matches('@').split(':').next().unwrap_or_default().to_owned());
                format!("{who}: {}", latest.body)
            }
        }
    }
}

fn filter_name(filter: Filter) -> &'static str {
    match filter {
        Filter::All => "all",
        Filter::Unread => "unread",
        Filter::Dms => "dms",
        Filter::Muted => "muted",
    }
}
