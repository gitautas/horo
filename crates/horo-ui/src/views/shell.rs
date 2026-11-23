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
use std::{future::Future, pin::Pin, sync::Arc};

use horo_core::{
    dto::{NotifyMode, RoomSummary, SyncState},
    matrix::{MatrixCore, rooms as core_rooms},
};

use matrix_sdk::ruma::OwnedRoomId;

use crate::{
    backend::Backend,
    OpenSettings,
    presence,
    media::Media,
    rooms::{self, Filter, View},
    store::{Phase, SettingsSection, Store, Tone},
    theme::{Palette, corner, heading, label, space, text},
    views::{menu, avatar::{accent_for, avatar, initials, presence_dot, with_presence}, composer::Composer, profile::{ProfileCard, same_target}, room_info::{OpenRoomSettings, RoomInfo}, room_settings::RoomSettings, invite_pane::InvitePane, invite_to_space::InviteToSpace, leave_space::LeaveSpace, explore::{Explore, Closed}, create_room::CreateRoom,
        settings::Settings, timeline::{Timeline, TimelineAction}, verification, lightbox, voice},
};

pub struct Shell {
    store: Entity<Store>,
    /// The open room: which, its timeline and its composer.
    timeline: Option<(String, Entity<Timeline>, Entity<Composer>)>,
    /// The open room's info panel, built while the room is open.
    info: Option<Entity<RoomInfo>>,
    /// Explore spaces, and make-or-join a room, while open.
    explore: Option<Entity<Explore>>,
    create: Option<Entity<CreateRoom>>,
    room_settings: Option<Entity<RoomSettings>>,
    /// An invite, shown where the room would be.
    invite: Option<Entity<InvitePane>>,
    /// A space's right-click menu: which space, its name, and where.
    space_menu: Option<(String, String, gpui::Point<gpui::Pixels>)>,
    /// A room's right-click menu: which, and where.
    room_menu: Option<(String, gpui::Point<gpui::Pixels>)>,
    invite_to_space: Option<Entity<InviteToSpace>>,
    leave_space: Option<Entity<LeaveSpace>>,
    browse: Option<Entity<Explore>>,
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
        let mut shell = Shell { store, timeline: None, info: None, explore: None, create: None, room_settings: None, invite: None, space_menu: None, room_menu: None, invite_to_space: None, leave_space: None, browse: None, settings: None, profile: None, focus, search, filter: Filter::All, filters_open: false };
        shell.follow_room(window, cx);
        shell.follow_settings(window, cx);
        shell
    }

    /// Open the selected room's timeline and composer, closing the last.
    fn follow_room(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let selected = store.room.clone();
        let thread = store.thread.clone();
        // An invite has no timeline to read yet: the invite pane instead.
        let invited = selected.as_ref().is_some_and(|id| {
            store.rooms.iter().any(|r| r.id == *id && r.membership == horo_core::dto::RoomMembership::Invited)
        });
        if invited {
            let id = selected.clone().unwrap_or_default();
            if let Some((_, old, _)) = self.timeline.take() {
                old.read(cx).close(cx);
            }
            self.info = None;
            if self.invite.as_ref().is_none_or(|pane| pane.read(cx).room_id != id) {
                let store = self.store.clone();
                self.invite = Some(cx.new(|cx| InvitePane::new(store, id, cx)));
            }
            return;
        }
        self.invite = None;
        // The timeline is the room's, or one thread's of it.
        let wanted = selected.as_ref().map(|room| match &thread {
            Some(root) => format!("{room}|{root}"),
            None => room.clone(),
        });
        if self.timeline.as_ref().map(|(key, ..)| key) == wanted.as_ref() {
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
        let Some(id) = selected else {
            self.info = None;
            return;
        };
        // The info panel is the room's, whichever thread is open.
        if self.info.as_ref().is_none_or(|info| info.read(cx).room_id != id) {
            let store = self.store.clone();
            let room_id = id.clone();
            let info = cx.new(|cx| RoomInfo::new(store, room_id, cx));
            cx.subscribe_in(&info, window, |this, info, _: &OpenRoomSettings, window, cx| {
                let room_id = info.read(cx).room_id.clone();
                this.open_room_settings(room_id, window, cx);
            })
            .detach();
            self.info = Some(info);
        }
        let Ok(room_id) = OwnedRoomId::try_from(id.as_str()) else { return };
        self.store.update(cx, |s, cx| s.load_packs(&id, cx));
        let store = self.store.clone();
        let timeline = cx.new(|cx| Timeline::new(room_id.clone(), thread.clone(), &store, cx));
        let weak = timeline.downgrade();
        let composer = cx.new(|cx| Composer::new((room_id, thread), &name, encrypted, store.clone(), weak, window, cx));
        // Reply and edit buttons on a message go to the composer under it.
        cx.subscribe_in(&timeline, window, |this, _, action: &TimelineAction, window, cx| {
            if let Some((_, _, composer)) = &this.timeline {
                composer.update(cx, |c, cx| c.take(action, window, cx));
            }
        })
        .detach();
        self.timeline = wanted.map(|key| (key, timeline, composer));
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

    fn open_explore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let explore = cx.new(|cx| Explore::new(store, window, cx));
        cx.subscribe(&explore, |this, _, _: &Closed, cx| {
            this.explore = None;
            cx.notify();
        })
        .detach();
        self.explore = Some(explore);
        cx.notify();
    }

    fn open_room_settings(&mut self, room_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let settings = cx.new(|cx| RoomSettings::new(store, room_id, window, cx));
        cx.subscribe(&settings, |this, _, _: &Closed, cx| {
            this.room_settings = None;
            cx.notify();
        })
        .detach();
        self.room_settings = Some(settings);
        cx.notify();
    }

    fn open_invite_to_space(&mut self, space_id: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.space_menu = None;
        let store = self.store.clone();
        let dialog = cx.new(|cx| InviteToSpace::new(store, space_id, name, window, cx));
        cx.subscribe(&dialog, |this, _, _: &Closed, cx| {
            this.invite_to_space = None;
            cx.notify();
        })
        .detach();
        self.invite_to_space = Some(dialog);
        cx.notify();
    }

    fn open_leave_space(&mut self, space_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.space_menu = None;
        let store = self.store.clone();
        let dialog = cx.new(|cx| LeaveSpace::new(store, space_id, window, cx));
        cx.subscribe(&dialog, |this, _, _: &Closed, cx| {
            this.leave_space = None;
            cx.notify();
        })
        .detach();
        self.leave_space = Some(dialog);
        cx.notify();
    }

    fn open_browse(&mut self, space_id: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.space_menu = None;
        let store = self.store.clone();
        let dialog = cx.new(|cx| Explore::browse(store, space_id, name, window, cx));
        cx.subscribe(&dialog, |this, _, _: &Closed, cx| {
            this.browse = None;
            cx.notify();
        })
        .detach();
        self.browse = Some(dialog);
        cx.notify();
    }

    /// Something done to each joined room of a space, one at a time (push
    /// rules are one account-wide ruleset), off the UI thread.
    fn on_space_rooms(
        &mut self,
        space_id: &str,
        cx: &mut Context<Self>,
        job: impl Fn(Arc<MatrixCore>, OwnedRoomId) -> Pin<Box<dyn Future<Output = horo_core::Result<()>> + Send>> + Send + 'static,
    ) {
        self.space_menu = None;
        let ids: Vec<String> = {
            let store = self.store.read(cx);
            rooms::in_space(&store.rooms, &store.spaces, space_id).into_iter().map(|r| r.id.clone()).collect()
        };
        self.on_rooms(ids, cx, job);
    }

    /// Something done to each of `ids`, one at a time, off the UI thread;
    /// the first failure is said in the banner.
    fn on_rooms(
        &mut self,
        ids: Vec<String>,
        cx: &mut Context<Self>,
        job: impl Fn(Arc<MatrixCore>, OwnedRoomId) -> Pin<Box<dyn Future<Output = horo_core::Result<()>> + Send>> + Send + 'static,
    ) {
        let ids: Vec<OwnedRoomId> = ids.iter().filter_map(|id| OwnedRoomId::try_from(id.as_str()).ok()).collect();
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move {
                let core = state.core().await?;
                // One room that won't is reported, not a reason to stop.
                let mut failed = None;
                for id in ids {
                    if let Err(e) = job(core.clone(), id.clone()).await {
                        tracing::warn!("{id}: {e}");
                        failed.get_or_insert(e.to_string());
                    }
                }
                Ok::<_, horo_core::Error>(failed)
            }
        });
        cx.spawn(async move |this, cx| {
            let failed = match task.await {
                Ok(Ok(failed)) => failed,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            if let Some(e) = failed {
                let _ = this.update(cx, |this, cx| this.store.update(cx, |s, cx| s.show_banner(Tone::Error, e, cx)));
            }
        })
        .detach();
        cx.notify();
    }

    /// A space's right-click menu, as SpacesRail.tsx's (after Discord's
    /// server menu).
    fn space_menu(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, name, at) = self.space_menu.clone()?;
        let (joined, hide_muted) = {
            let store = self.store.read(cx);
            let joined: Vec<RoomSummary> = rooms::in_space(&store.rooms, &store.spaces, &id).into_iter().cloned().collect();
            (joined, store.prefs.hide_muted_in_spaces.contains(&id))
        };
        let any_unread = joined.iter().any(|r| r.has_unread);
        let all_muted = !joined.is_empty() && joined.iter().all(|r| r.is_muted);
        // One mode for all of them, or none to show ("mixed").
        let first = joined.first().map(|r| r.notify_mode.unwrap_or(NotifyMode::All));
        let mode = first.filter(|m| joined.iter().all(|r| r.notify_mode.unwrap_or(NotifyMode::All) == *m));

        // `check`: a toggle or a choice, with its tick (or the space for
        // one) in a column of its own so the words line up.
        let item = |key: SharedString, words: String, enabled: bool, check: Option<bool>| menu::item(key, words, enabled, check, palette);
        let separator = || menu::separator(palette);

        let mut menu = menu::frame("space-menu", palette)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.space_menu = None;
                cx.notify();
            }))
            .child(menu::caption(name.clone(), palette));

        let sid = id.clone();
        menu = menu.child(item("sm-read".into(), "mark as read".into(), any_unread, None).on_click(cx.listener(move |this, _, _, cx| {
            if any_unread {
                this.on_space_rooms(&sid, cx, |core, id| Box::pin(async move { core_rooms::mark_read(&core, &id).await }));
            }
        })));
        menu = menu.child(separator());
        let (sid, sname) = (id.clone(), name.clone());
        menu = menu.child(item("sm-invite".into(), "invite to space".into(), true, None).on_click(cx.listener(move |this, _, window, cx| {
            this.open_invite_to_space(sid.clone(), sname.clone(), window, cx)
        })));
        let sid = id.clone();
        menu = menu.child(
            v_flex()
                .child(item("sm-open-rooms".into(), "let members into its rooms".into(), true, None).on_click(cx.listener(move |this, _, _, cx| {
                    this.space_menu = None;
                    let Ok(space_room) = OwnedRoomId::try_from(sid.as_str()) else { return };
                    let backend = Backend::global(cx);
                    let task = backend.run({
                        let state = backend.state.clone();
                        async move { core_rooms::open_space_rooms(&*state.core().await?, &space_room).await }
                    });
                    cx.spawn(async move |this, cx| {
                        let (tone, message) = match task.await {
                            Ok(Ok(r)) => (Tone::Info, opened_message(r.opened, r.not_allowed)),
                            Ok(Err(e)) => (Tone::Error, e.to_string()),
                            Err(e) => (Tone::Error, e.to_string()),
                        };
                        let _ = this.update(cx, |this, cx| this.store.update(cx, |s, cx| s.show_banner(tone, message, cx)));
                    })
                    .detach();
                    cx.notify();
                })))
                .child(div().px(space(12.)).pb(space(4.)).text_size(text(11.)).text_color(palette.text_tertiary).child("opens invite-only rooms to everyone in the space")),
        );
        menu = menu.child(separator());
        let sid = id.clone();
        menu = menu.child(item("sm-mute".into(), "mute space".into(), !joined.is_empty(), Some(all_muted)).on_click(cx.listener(move |this, _, _, cx| {
            let mute = !all_muted;
            this.on_space_rooms(&sid, cx, move |core, id| {
                Box::pin(async move { if mute { core_rooms::set_notify_mode(&core, &id, NotifyMode::Mute).await } else { core_rooms::set_muted(&core, &id, false).await } })
            });
        })));
        menu = menu.child(
            div()
                .px(space(12.))
                .pt(space(6.))
                .pb(space(2.))
                .text_size(text(11.))
                .text_color(palette.text_tertiary)
                .child(format!("notifications{}", if mode.is_none() && !joined.is_empty() { " · mixed" } else { "" })),
        );
        for (m, words) in [(NotifyMode::All, "all messages"), (NotifyMode::Mentions, "only @mentions"), (NotifyMode::Mute, "nothing")] {
            let sid = id.clone();
            menu = menu.child(
                item(SharedString::from(format!("sm-notify-{words}")), words.into(), !joined.is_empty(), Some(mode == Some(m)))
                    .on_click(cx.listener(move |this, _, _, cx| this.on_space_rooms(&sid, cx, move |core, id| Box::pin(async move { core_rooms::set_notify_mode(&core, &id, m).await })))),
            );
        }
        menu = menu.child(separator());
        let sid = id.clone();
        menu = menu.child(item("sm-hide-muted".into(), "hide muted rooms".into(), true, Some(hide_muted)).on_click(cx.listener(move |this, _, _, cx| {
            this.space_menu = None;
            let sid = sid.clone();
            this.store.update(cx, |s, cx| {
                s.update_prefs(
                    |p| {
                        if p.hide_muted_in_spaces.contains(&sid) {
                            p.hide_muted_in_spaces.retain(|x| *x != sid);
                        } else {
                            p.hide_muted_in_spaces.push(sid);
                        }
                    },
                    cx,
                )
            });
        })));
        let (sid, sname) = (id.clone(), name.clone());
        menu = menu.child(item("sm-browse".into(), "browse rooms".into(), true, None).on_click(cx.listener(move |this, _, window, cx| this.open_browse(sid.clone(), sname.clone(), window, cx))));
        menu = menu.child(separator());
        let sid = id.clone();
        menu = menu.child(item("sm-leave".into(), "leave space".into(), true, None).text_color(palette.danger).on_click(cx.listener(move |this, _, window, cx| this.open_leave_space(sid.clone(), window, cx))));
        menu = menu.child(separator());
        let sid = id.clone();
        menu = menu.child(item("sm-copy".into(), "copy space id".into(), true, None).on_click(cx.listener(move |this, _, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(sid.clone()));
            this.space_menu = None;
            this.store.update(cx, |s, cx| s.show_banner(Tone::Info, "space id copied", cx));
            cx.notify();
        })));

        Some(menu::place(at, menu))
    }

    /// A room's right-click menu, as RoomList.tsx's.
    fn room_menu(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, at) = self.room_menu.clone()?;
        let room = self.store.read(cx).rooms.iter().find(|r| r.id == id).cloned()?;
        let close = |this: &mut Shell, cx: &mut Context<Shell>| {
            this.room_menu = None;
            cx.notify();
        };
        let mut list = menu::frame("room-menu", palette)
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| close(this, cx)))
            .child(menu::caption(room.name.clone(), palette));

        if room.membership == horo_core::dto::RoomMembership::Invited {
            let rid = id.clone();
            list = list.child(menu::item("rm-decline", "decline invite", true, None, palette).text_color(palette.danger).on_click(cx.listener(
                move |this, _, window, cx| {
                    close(this, cx);
                    this.open_room_settings(rid.clone(), window, cx);
                    this.ask_to_leave(cx);
                },
            )));
        } else {
            let (rid, unread) = (id.clone(), room.has_unread);
            list = list.child(menu::item("rm-read", "mark as read", unread, None, palette).on_click(cx.listener(move |this, _, _, cx| {
                close(this, cx);
                if unread {
                    this.on_rooms(vec![rid.clone()], cx, |core, id| Box::pin(async move { core_rooms::mark_read(&core, &id).await }));
                }
            })));
            list = list.child(menu::separator(palette));
            let (rid, muted) = (id.clone(), room.is_muted);
            let mute_words = if room.is_direct { "mute chat" } else { "mute room" };
            list = list.child(menu::item("rm-mute", mute_words, true, Some(muted), palette).on_click(cx.listener(move |this, _, _, cx| {
                close(this, cx);
                this.on_rooms(vec![rid.clone()], cx, move |core, id| {
                    Box::pin(async move { if muted { core_rooms::set_muted(&core, &id, false).await } else { core_rooms::set_notify_mode(&core, &id, NotifyMode::Mute).await } })
                });
            })));
            list = list.child(menu::caption("notifications", palette));
            let mode = room.notify_mode.unwrap_or(NotifyMode::All);
            for (m, words) in [(NotifyMode::All, "all messages"), (NotifyMode::Mentions, "only @mentions"), (NotifyMode::Mute, "nothing")] {
                let rid = id.clone();
                list = list.child(menu::item(SharedString::from(format!("rm-notify-{words}")), words, true, Some(mode == m), palette).on_click(cx.listener(
                    move |this, _, _, cx| {
                        close(this, cx);
                        this.on_rooms(vec![rid.clone()], cx, move |core, id| Box::pin(async move { core_rooms::set_notify_mode(&core, &id, m).await }));
                    },
                )));
            }
            list = list.child(menu::separator(palette));
            let rid = id.clone();
            let settings_words = if room.is_direct { "chat settings" } else { "edit room" };
            list = list.child(menu::item("rm-settings", settings_words, true, None, palette).on_click(cx.listener(move |this, _, window, cx| {
                close(this, cx);
                this.open_room_settings(rid.clone(), window, cx);
            })));
            let rid = id.clone();
            let leave_words = if room.is_direct { "leave chat" } else { "leave room" };
            list = list.child(menu::item("rm-leave", leave_words, true, None, palette).text_color(palette.danger).on_click(cx.listener(
                move |this, _, window, cx| {
                    close(this, cx);
                    this.open_room_settings(rid.clone(), window, cx);
                    this.ask_to_leave(cx);
                },
            )));
        }
        list = list.child(menu::separator(palette));
        let rid = id.clone();
        list = list.child(menu::item("rm-copy", "copy room id", true, None, palette).on_click(cx.listener(move |this, _, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(rid.clone()));
            close(this, cx);
            this.store.update(cx, |s, cx| s.show_banner(Tone::Info, "room id copied", cx));
        })));
        Some(menu::place(at, list))
    }

    /// The room settings just opened, at "leave?" — leaving (and declining
    /// an invite) is confirmed there.
    fn ask_to_leave(&mut self, cx: &mut Context<Self>) {
        if let Some(settings) = &self.room_settings {
            settings.update(cx, |s, cx| s.ask_to_leave(cx));
        }
    }

    fn open_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let create = cx.new(|cx| CreateRoom::new(store, window, cx));
        cx.subscribe(&create, |this, _, _: &Closed, cx| {
            this.create = None;
            cx.notify();
        })
        .detach();
        self.create = Some(create);
        cx.notify();
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
            }))
            .on_mouse_down(gpui::MouseButton::Right, {
                let (id, name) = (space.id.clone(), space.name.clone());
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.space_menu = Some((id.clone(), name.clone(), event.position));
                    cx.notify();
                })
            });
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
                    .children(tiles)
                    .child(
                        div()
                            .id("rail-explore")
                            .size(px(46.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(corner(16.))
                            .cursor_pointer()
                            .border_1()
                            .border_dashed()
                            .border_color(palette.border)
                            .hover(|d| d.bg(palette.raised))
                            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("explore spaces").build(window, cx))
                            .child(voice::icon("icons/compass.svg").size(px(20.)).text_color(palette.text_tertiary))
                            .on_click(cx.listener(|this, _, window, cx| this.open_explore(window, cx))),
                    ),
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
            .on_mouse_down(gpui::MouseButton::Right, {
                let id = room.id.clone();
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.space_menu = None;
                    this.room_menu = Some((id.clone(), event.position));
                    cx.stop_propagation();
                    cx.notify();
                })
            })
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
        let hide_muted = open_space.as_ref().is_some_and(|s| store.prefs.hide_muted_in_spaces.contains(&s.id));
        let visible = rooms::filter(&rooms, &View { space: open_space.as_ref(), filter: self.filter, search: &search, hide_muted });
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
                                h_flex()
                                    .flex_none()
                                    .gap(space(10.))
                                    .pb(px(2.))
                                    .child(
                                        div()
                                            .font_family(palette.mono_font.clone())
                                            .text_size(text(11.))
                                            .text_color(if status.is_some() { palette.idle } else { palette.text_tertiary })
                                            .child(caught_up),
                                    )
                                    .child(
                                        div()
                                            .id("create-room")
                                            .size(px(26.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded_full()
                                            .cursor_pointer()
                                            .bg(palette.card)
                                            .border_1()
                                            .border_color(palette.border)
                                            .hover(|d| d.bg(palette.raised))
                                            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("make or join a room").build(window, cx))
                                            .child(voice::icon("icons/plus.svg").size(px(13.)).text_color(palette.text_secondary))
                                            .on_click(cx.listener(|this, _, window, cx| this.open_create(window, cx))),
                                    ),
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

        let info_open = self.store.read(cx).prefs.show_info_panel;
        let store = self.store.clone();
        let info_button = div()
            .id("header-info")
            .p(space(9.))
            .rounded(corner(12.))
            .cursor_pointer()
            .bg(palette.raised)
            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("room info").build(window, cx))
            .child(voice::icon("icons/info.svg").size(px(17.)).text_color(if info_open { palette.accent } else { palette.text_secondary }))
            .on_click(move |_, _, cx| store.update(cx, |s, cx| s.update_prefs(|p| p.show_info_panel = !p.show_info_panel, cx)));

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
                    .child(call_button)
                    .child(info_button),
            )
            .into_any_element()
    }

    fn room_pane(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let room = store.room.as_ref().and_then(|id| store.rooms.iter().find(|r| r.id == *id)).cloned();
        let (thread, thread_preview) = (store.thread.clone(), store.thread_preview.clone());
        let strip = room.as_ref().and_then(|r| voice::strip(&r.id, &self.store, palette, cx));
        let header = room.as_ref().map(|r| self.room_header(r, palette, cx));
        // A thread replaces the room's timeline, said in a bar above it
        // with the way back, as ChatPane.tsx's ThreadHeader.
        let thread_bar = thread.as_ref().map(|_| {
            let preview = thread_preview.clone();
            h_flex()
                .flex_none()
                .mx(space(22.))
                .mt(space(14.))
                .gap(space(10.))
                .px(space(14.))
                .py(space(10.))
                .rounded(corner(16.))
                .bg(palette.card)
                .relative()
                .overflow_hidden()
                .border_1()
                .border_color(palette.border_subtle)
                // The coloured left edge: gpui borders take one colour.
                .child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(palette.accent_quaternary))
                .child(voice::icon("icons/messages-square.svg").size(px(15.)).text_color(palette.accent_quaternary))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(text(13.))
                        .text_color(palette.text_secondary)
                        .child(match preview {
                            Some(p) => format!("thread · {p}"),
                            None => "thread".to_owned(),
                        }),
                )
                .child(
                    div()
                        .id("thread-close")
                        .px_1()
                        .cursor_pointer()
                        .text_color(palette.text_tertiary)
                        .hover(|d| d.text_color(palette.text))
                        .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("back to the room").build(window, cx))
                        .child("✕")
                        .on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.open_thread(None, cx)))),
                )
        });
        let in_thread = self.store.read(cx).thread.is_some();
        let body = match (&room, &self.timeline) {
            (Some(room), Some((_, timeline, composer))) => v_flex()
                .relative()
                .flex_1()
                .min_h_0()
                .children(thread_bar)
                .children(strip)
                .child(div().flex_1().min_h_0().child(timeline.clone()))
                .child(composer.clone())
                .child(drop_zone(&room.name, in_thread, composer.clone(), palette)),
            _ => v_flex().m_auto().text_color(palette.text_tertiary).child("pick a room"),
        };
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(palette.app)
            .children(header)
            .children(verification::banner(&self.store, palette, cx))
            .child(body)
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        // Closing settings or a dialog drops the element that had the
        // keyboard, and with nothing focused ⌘, (or any action) has no path
        // to the shell's handlers. Take it back: the room's composer, so
        // typing just works, or the shell itself.
        if window.focused(cx).is_none() {
            match &self.timeline {
                Some((_, _, composer)) => composer.update(cx, |c, cx| c.focus(window, cx)),
                None => self.focus.focus(window, cx),
            }
        }
        h_flex()
            .id("shell")
            .track_focus(&self.focus)
            // ⌘, from anywhere in the window, the menu included. Already
            // open, it stays where it is.
            // Escape closes the picture open over everything before
            // anything under it (the composer's reply, say) hears it.
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.store.read(cx).lightbox.is_some() {
                    this.store.update(cx, |s, cx| s.close_lightbox(cx));
                    cx.stop_propagation();
                }
                // And a message's menu before the composer's reply.
                else if event.keystroke.key == "escape"
                    && let Some((_, timeline, _)) = &this.timeline
                    && timeline.update(cx, |t, cx| t.close_menu(cx))
                {
                    cx.stop_propagation();
                }
            }))
            // Escape closes a space's menu, as a click elsewhere does.
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && (this.space_menu.take().is_some() | this.room_menu.take().is_some()) {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
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
            .map(|d| match self.invite.clone() {
                Some(invite) => d.child(invite),
                None => d.child(self.room_pane(&palette, cx)).when(self.store.read(cx).prefs.show_info_panel, |d| d.children(self.info.clone())),
            })
            .relative()
            .children(self.settings.clone())
            .children(lightbox::lightbox(&self.store, &palette, window, cx))
            .children(voice::ringing(&self.store, &palette, cx))
            .children(self.explore.clone())
            .children(self.create.clone())
            .children(self.room_settings.clone())
            .children(self.invite_to_space.clone())
            .children(self.leave_space.clone())
            .children(self.browse.clone())
            .children(self.space_menu(&palette, cx))
            .children(self.room_menu(&palette, cx))
            .children(self.profile.clone())
            .children(banner(&self.store, &palette, cx))
            // Above everything, the security panel included: the other
            // device's timeout is running, and a dialog hidden behind
            // another one is a verification that fails.
            .children(verification::dialog(&self.store, &palette, cx))
    }
}

/// An empty strip at the top of a column that drags the window, since the
/// title bar is hidden; a double click zooms, as a title bar's would.
/// What "let members into its rooms" did, as SpacesRail.tsx says it.
fn opened_message(opened: u32, not_allowed: u32) -> String {
    let rooms = |n: u32| if n == 1 { "1 room".to_owned() } else { format!("{n} rooms") };
    match (opened, not_allowed) {
        (0, 0) => "everyone in the space can already get into its rooms".into(),
        (0, 1) => "you can't change who gets into its invite-only room".into(),
        (0, n) => format!("you can't change who gets into its {n} invite-only rooms"),
        (o, 0) => format!("opened {} to the space", rooms(o)),
        (o, n) => format!("opened {} to the space; {} need someone with more power", rooms(o), rooms(n)),
    }
}

/// The line at the foot of the window, as App.tsx draws it.
fn banner(store: &Entity<Store>, palette: &Palette, cx: &mut Context<Shell>) -> Option<AnyElement> {
    let banner = store.read(cx).banner.clone()?;
    let error = banner.tone == Tone::Error;
    let store = store.clone();
    Some(
        div()
            .absolute()
            .bottom(px(22.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                h_flex()
                    .occlude()
                    .max_w(px(520.))
                    .gap(space(12.))
                    .px(space(18.))
                    .py(space(12.))
                    .rounded(corner(18.))
                    .bg(palette.raised)
                    .border_1()
                    .border_color(if error { Hsla { a: 0.4, ..palette.danger } } else { palette.border })
                    .shadow_lg()
                    .child(
                        voice::icon(if error { "icons/circle-alert.svg" } else { "icons/info.svg" })
                            .flex_none()
                            .size(px(16.))
                            .text_color(if error { palette.danger } else { palette.accent_quaternary }),
                    )
                    .child(div().text_size(text(13.)).line_height(gpui::relative(1.45)).text_color(palette.text_secondary).child(banner.message))
                    .child(
                        div()
                            .id("banner-dismiss")
                            .flex_none()
                            .px_1()
                            .cursor_pointer()
                            .text_color(palette.text_tertiary)
                            .hover(|d| d.text_color(palette.text))
                            .child("✕")
                            .on_click(move |_, _, cx| store.update(cx, |s, cx| s.dismiss_banner(cx))),
                    ),
            )
            .into_any_element(),
    )
}

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

pub(crate) fn logo() -> std::sync::Arc<gpui::Image> {
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

/// Files dragged over the room: a card saying where they'll go, shown
/// only while a drag from outside the app hovers; dropping puts them in
/// the message box, to go with the next message.
/// Unseen and click-through the rest of the time.
fn drop_zone(room: &str, in_thread: bool, composer: Entity<Composer>, palette: &Palette) -> impl IntoElement {
    let where_to = if in_thread {
        format!("it'll wait in the box, to go to the thread you have open in {room}")
    } else {
        format!("it'll wait in the box, to go to {room}")
    };
    div()
        .id("drop-zone")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .opacity(0.)
        .drag_over::<gpui::ExternalPaths>(|style, _, _, _| style.opacity(1.))
        .bg(gpui::Hsla { a: 0.78, ..palette.app })
        .on_drop(move |paths: &gpui::ExternalPaths, _, cx| {
            let paths = paths.paths().to_vec();
            composer.update(cx, |c, cx| c.stage_paths(paths, cx));
        })
        .child(
            v_flex()
                .items_center()
                .gap(px(14.))
                .px(px(64.))
                .py(px(44.))
                .rounded(px(28.))
                .border_2()
                .border_dashed()
                .border_color(palette.accent)
                .bg(palette.raised)
                .shadow_lg()
                .child(div().font_family(palette.heading_font.clone()).font_weight(gpui::FontWeight::EXTRA_BOLD).text_size(px(22.)).child("drop it here~"))
                .child(div().max_w(px(320.)).text_center().text_size(px(13.)).text_color(palette.text_secondary).child(where_to)),
        )
}

#[cfg(test)]
mod tests {
    use super::opened_message;

    #[test]
    fn opening_a_spaces_rooms_says_what_happened() {
        assert_eq!(opened_message(0, 0), "everyone in the space can already get into its rooms");
        assert_eq!(opened_message(0, 1), "you can't change who gets into its invite-only room");
        assert_eq!(opened_message(2, 0), "opened 2 rooms to the space");
        assert_eq!(opened_message(1, 3), "opened 1 room to the space; 3 rooms need someone with more power");
    }
}

