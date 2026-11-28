//! The signed-in window: the spaces rail, the room list, and the room.

use gpui::{
    AnyElement, Hsla, img, StyledImage, FocusHandle, AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Window, div, prelude::FluentBuilder,
};
use gpui_component::{
    Icon, IconName, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use std::{collections::HashSet, future::Future, pin::Pin, sync::Arc};

use horo_core::{
    dto::{NotifyMode, RoomSummary, SyncState},
    matrix::{MatrixCore, directory::{self, PublicRoom}, rooms as core_rooms},
};

use matrix_sdk::ruma::OwnedRoomId;

use crate::theme::zpx;
use crate::{
    backend::Backend,
    FindInRoom, NextRoom, NextUnreadRoom, OpenQuickSwitcher, OpenSettings, PrevRoom, PrevUnreadRoom,
    presence,
    media::Media,
    rooms::{self, Filter, View},
    store::{Phase, SettingsSection, Store, Tone},
    theme::{Palette, corner, heading, label, space, text},
    views::{menu, avatar::{accent_for, avatar, initials, presence_dot, with_presence}, composer::Composer, profile::{ProfileCard, same_target}, quick_switcher::{self, QuickSwitcher}, room_info::{OpenRoomSettings, RoomInfo}, room_settings::RoomSettings, space_settings::{self, SpaceSection, SpaceSettings}, invite_pane::InvitePane, invite_to_space::InviteToSpace, leave_space::LeaveSpace, explore::{Explore, Closed}, create_room::CreateRoom,
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
    /// Whether the room whose menu is open may be deleted by this account.
    room_menu_can_delete: Option<(String, bool)>,
    invite_to_space: Option<Entity<InviteToSpace>>,
    space_settings: Option<Entity<SpaceSettings>>,
    leave_space: Option<Entity<LeaveSpace>>,
    browse: Option<Entity<Explore>>,
    /// Settings, while they're open.
    settings: Option<Entity<Settings>>,
    /// The ⌘K quick switcher, while it's open.
    quick_switcher: Option<Entity<QuickSwitcher>>,
    /// The bottom-left status picker popover, when open.
    status_menu: Option<gpui::Point<gpui::Pixels>>,
    /// The profile card, while one is open.
    profile: Option<Entity<ProfileCard>>,
    /// Holds the window's focus when nothing inside does, so ⌘, still
    /// reaches the shell.
    focus: FocusHandle,
    /// The room list's search box and filter.
    search: Entity<InputState>,
    filter: Filter,
    filters_open: bool,
    /// Rooms in the open space's hierarchy (for listing unjoined channels),
    /// keyed by `(space_id, children_snapshot)`.
    space_hierarchy: Option<(String, Vec<String>, Vec<PublicRoom>)>,
    /// Unjoined rooms currently being joined.
    joining: HashSet<String>,
}

impl Shell {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Shell {
        cx.observe_in(&store, window, |this, _, window, cx| {
            this.follow_room(window, cx);
            this.follow_space(cx);
            this.follow_settings(window, cx);
            this.follow_profile(window, cx);
            cx.notify()
        })
        .detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("find or start a conversation"));
        cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        let mut shell = Shell { store, timeline: None, info: None, explore: None, create: None, room_settings: None, invite: None, space_menu: None, room_menu: None, room_menu_can_delete: None, invite_to_space: None, space_settings: None, leave_space: None, browse: None, settings: None, quick_switcher: None, status_menu: None, profile: None, focus, search, filter: Filter::All, filters_open: false, space_hierarchy: None, joining: HashSet::new() };
        shell.follow_room(window, cx);
        shell.follow_space(cx);
        shell.follow_settings(window, cx);
        shell
    }

    /// Fetch the open space's hierarchy whenever the space or its children
    /// change, so joinable rooms we aren't in yet show up in the sidebar.
    fn follow_space(&mut self, cx: &mut Context<Self>) {
        let open = self.store.read(cx).current_space().cloned();
        let Some(space) = open else {
            self.space_hierarchy = None;
            return;
        };
        if self.space_hierarchy.as_ref().is_some_and(|(id, children, _)| id == &space.id && children == &space.children) {
            return;
        }
        let Ok(space_room) = OwnedRoomId::try_from(space.id.as_str()) else { return };
        let (sid, children) = (space.id.clone(), space.children.clone());
        // Record the key immediately so we don't re-fetch while in flight,
        // keeping any already-fetched rooms for this space until the refresh lands.
        let prev = self.space_hierarchy.as_ref().filter(|(id, _, _)| id == &sid).map(|(_, _, r)| r.clone()).unwrap_or_default();
        self.space_hierarchy = Some((sid.clone(), children.clone(), prev));
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { directory::space_rooms(&*state.core().await?, &space_room).await }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(found)) = task.await {
                let _ = this.update(cx, |this, cx| {
                    if this.space_hierarchy.as_ref().is_some_and(|(id, c, _)| id == &sid && c == &children) {
                        this.space_hierarchy = Some((sid, children, found));
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    fn join_unjoined(&mut self, room: PublicRoom, cx: &mut Context<Self>) {
        if !self.joining.insert(room.id.clone()) {
            return;
        }
        let (id, via, voice) = (room.id.clone(), room.via.clone(), room.is_voice);
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, id, via) = (backend.state.clone(), id.clone(), via);
            async move { core_rooms::join(&*state.core().await?, &id, &via).await }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                this.joining.remove(&id);
                match outcome {
                    Ok(Ok(joined_id)) => {
                        this.store.update(cx, |s, cx| {
                            s.open_room(joined_id.clone(), cx);
                            if voice {
                                s.join_call(joined_id, cx);
                            }
                        });
                    }
                    Ok(Err(e)) => this.store.update(cx, |s, cx| s.show_banner(Tone::Error, e.to_string(), cx)),
                    Err(e) => this.store.update(cx, |s, cx| s.show_banner(Tone::Error, e.to_string(), cx)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
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
        self.store.update(cx, |s, cx| {
            s.load_packs(&id, cx);
            if let Some(sid) = s.space_for_room(&id) {
                s.load_roles(&sid, false, cx);
            }
        });
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

    fn open_space_settings(&mut self, space_id: String, name: String, section: SpaceSection, window: &mut Window, cx: &mut Context<Self>) {
        self.space_menu = None;
        let store = self.store.clone();
        let dialog = cx.new(|cx| SpaceSettings::new(store, space_id, name, section, window, cx));
        cx.subscribe(&dialog, |this, _, _: &Closed, cx| {
            this.space_settings = None;
            cx.notify();
        })
        .detach();
        self.space_settings = Some(dialog);
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
    /// each room is refreshed in the sidebar afterward, and the first failure
    /// is said in the banner.
    fn on_rooms(
        &mut self,
        ids: Vec<String>,
        cx: &mut Context<Self>,
        job: impl Fn(Arc<MatrixCore>, OwnedRoomId) -> Pin<Box<dyn Future<Output = horo_core::Result<()>> + Send>> + Send + 'static,
    ) {
        let ids: Vec<OwnedRoomId> = ids.iter().filter_map(|id| OwnedRoomId::try_from(id.as_str()).ok()).collect();
        let backend = Backend::global(cx);
        let task = backend.run({
            let (host, state) = (backend.host.clone(), backend.state.clone());
            async move {
                let core = state.core().await?;
                // One room that won't is reported, not a reason to stop.
                let mut failed = None;
                for id in ids {
                    if let Err(e) = job(core.clone(), id.clone()).await {
                        tracing::warn!("{id}: {e}");
                        failed.get_or_insert(e.to_string());
                    }
                    core_rooms::emit_room_refresh(&host, &core, &id).await;
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
    /// Discord-style status picker popover above the bottom-left user bar.
    fn status_popover(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let at = self.status_menu?;
        let (info, current_shown) = {
            let store = self.store.read(cx);
            let Phase::SignedIn(info) = &store.phase else { return None };
            (info.clone(), store.shown_own_presence().0)
        };
        let close = |this: &mut Shell, cx: &mut Context<Shell>| {
            this.status_menu = None;
            cx.notify();
        };
        use horo_core::matrix::presence::OwnPresence;
        let mut list = menu::frame("status-menu", palette)
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| close(this, cx)))
            .child(menu::caption("status", palette));

        for (id, label_text, target, shown_match) in [
            ("st-online", "online", OwnPresence::Online, presence::Shown::Online),
            ("st-away", "away", OwnPresence::Unavailable, presence::Shown::Away),
            ("st-offline", "invisible", OwnPresence::Offline, presence::Shown::Offline),
        ] {
            list = list.child(menu::item(id, label_text, true, Some(current_shown == shown_match), palette).on_click(cx.listener(move |this, _, _, cx| {
                close(this, cx);
                this.store.update(cx, |s, cx| s.set_manual_presence(target, cx));
            })));
        }

        let (user_id, avatar_hint) = (info.user_id.clone(), info.avatar_url.clone());
        list = list
            .child(menu::separator(palette))
            .child(menu::item("st-profile", "view profile", true, None, palette).on_click(cx.listener(move |this, _, _, cx| {
                close(this, cx);
                this.store.update(cx, |s, cx| s.open_profile(user_id.clone(), at, avatar_hint.clone(), cx));
            })))
            .child(menu::item("st-edit", "edit status & profile…", true, None, palette).on_click(cx.listener(move |this, _, _, cx| {
                close(this, cx);
                this.store.update(cx, |s, cx| s.show_settings(Some(SettingsSection::Account), cx));
            })));

        Some(menu::place(at, list))
    }

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
        let (sid, sname) = (id.clone(), name.clone());
        menu = menu.child(item("sm-settings".into(), "space settings…".into(), true, None).on_click(cx.listener(move |this, _, window, cx| {
            this.open_space_settings(sid.clone(), sname.clone(), SpaceSection::Profile, window, cx)
        })));
        let (sid, sname) = (id.clone(), name.clone());
        menu = menu.child(item("sm-roles".into(), "roles…".into(), true, None).on_click(cx.listener(move |this, _, window, cx| {
            this.open_space_settings(sid.clone(), sname.clone(), SpaceSection::Roles, window, cx)
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
            if unread {
                list = list.child(menu::item("rm-read", "mark as read", true, None, palette).on_click(cx.listener(move |this, _, _, cx| {
                    close(this, cx);
                    this.on_rooms(vec![rid.clone()], cx, |core, id| Box::pin(async move { core_rooms::mark_read(&core, &id).await }));
                })));
            } else {
                list = list.child(menu::item("rm-unread", "mark as unread", true, None, palette).on_click(cx.listener(move |this, _, _, cx| {
                    close(this, cx);
                    this.on_rooms(vec![rid.clone()], cx, |core, id| Box::pin(async move { core_rooms::set_marked_unread(&core, &id, true).await }));
                })));
            }
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
            if self.room_menu_can_delete.as_ref().is_some_and(|(for_id, can)| for_id == &id && *can) {
                let rid = id.clone();
                list = list.child(menu::item("rm-delete", "delete room", true, None, palette).text_color(palette.danger).on_click(cx.listener(
                    move |this, _, window, cx| {
                        close(this, cx);
                        this.open_room_settings(rid.clone(), window, cx);
                        this.ask_to_delete(cx);
                    },
                )));
            }
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

    /// Open a room's context menu and check whether this account may delete it.
    fn open_room_menu(&mut self, room: &RoomSummary, at: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        self.space_menu = None;
        self.room_menu = Some((room.id.clone(), at));
        self.room_menu_can_delete = None;
        if !room.is_direct && room.membership == horo_core::dto::RoomMembership::Joined
            && let Ok(room_id) = OwnedRoomId::try_from(room.id.as_str())
        {
            let wanted = room.id.clone();
            let backend = Backend::global(cx);
            let task = backend.run({
                let state = backend.state.clone();
                async move { core_rooms::permissions(&*state.core().await?, &room_id).await }
            });
            cx.spawn(async move |this, cx| {
                if let Ok(Ok(perms)) = task.await {
                    let _ = this.update(cx, |this, cx| {
                        if this.room_menu.as_ref().is_some_and(|(id, _)| id == &wanted) {
                            this.room_menu_can_delete = Some((wanted, perms.can_delete));
                            cx.notify();
                        }
                    });
                }
            })
            .detach();
        }
        cx.notify();
    }

    /// The room settings just opened, at "leave?" — leaving (and declining
    /// an invite) is confirmed there.
    fn ask_to_leave(&mut self, cx: &mut Context<Self>) {
        if let Some(settings) = &self.room_settings {
            settings.update(cx, |s, cx| s.ask_to_leave(cx));
        }
    }

    /// The room settings just opened, at "delete?".
    fn ask_to_delete(&mut self, cx: &mut Context<Self>) {
        if let Some(settings) = &self.room_settings {
            settings.update(cx, |s, cx| s.ask_to_delete(cx));
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

    fn open_quick_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_switcher.is_some() {
            self.quick_switcher = None;
            cx.notify();
            return;
        }
        let store = self.store.clone();
        let switcher = cx.new(|cx| QuickSwitcher::new(store, window, cx));
        cx.subscribe(&switcher, |this, _, _: &quick_switcher::Closed, cx| {
            this.quick_switcher = None;
            cx.notify();
        })
        .detach();
        self.quick_switcher = Some(switcher);
        cx.notify();
    }

    /// Discord-style channel hopping: `Alt+↑`/`↓` through the open sidebar, or
    /// `Alt+Shift+↑`/`↓` through unread rooms across all servers and DMs.
    fn step_room(&mut self, delta: isize, unread_only: bool, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let current = store.room.clone();
        let target: Option<(Option<String>, String)> = if unread_only {
            let unread: Vec<&RoomSummary> = store
                .rooms
                .iter()
                .filter(|r| !r.is_space && !r.is_utility && r.membership == horo_core::dto::RoomMembership::Joined && r.has_unread)
                .collect();
            if unread.is_empty() {
                None
            } else {
                let pos = current.as_deref().and_then(|id| unread.iter().position(|r| r.id == id));
                let next_ix = match pos {
                    Some(i) => (i as isize + delta).rem_euclid(unread.len() as isize) as usize,
                    None => if delta >= 0 { 0 } else { unread.len() - 1 },
                };
                let r = unread[next_ix];
                let space_id = if r.is_direct { None } else { store.spaces.iter().find(|s| rooms::is_in_space(r, s)).map(|s| s.id.clone()) };
                Some((space_id, r.id.clone()))
            }
        } else {
            let open_space = store.current_space().cloned();
            let order = open_space.as_ref().map(|s| s.children.clone()).unwrap_or_default();
            let search = self.search.read(cx).value().to_string();
            let hide_muted = open_space.as_ref().is_some_and(|s| store.prefs.hide_muted_in_spaces.contains(&s.id));
            let visible = rooms::filter(&store.rooms, &View { space: open_space.as_ref(), spaces: &store.spaces, filter: self.filter, search: &search, hide_muted });
            let flat: Vec<&RoomSummary> = rooms::group(visible, &order).into_iter().flat_map(|(_, rs)| rs).collect();
            if flat.is_empty() {
                None
            } else {
                let pos = current.as_deref().and_then(|id| flat.iter().position(|r| r.id == id));
                let next_ix = match pos {
                    Some(i) => (i as isize + delta).rem_euclid(flat.len() as isize) as usize,
                    None => if delta >= 0 { 0 } else { flat.len() - 1 },
                };
                Some((store.space.clone(), flat[next_ix].id.clone()))
            }
        };
        if let Some((space_id, room_id)) = target {
            self.store.update(cx, |s, cx| {
                if s.space != space_id {
                    s.open_space(space_id, cx);
                }
                s.open_room(room_id, cx);
            });
        }
    }

    /// Reorder a dragged server before `target_id` in `prefs.space_order`.
    fn reorder_space(&mut self, dragged_id: &str, target_id: &str, cx: &mut Context<Self>) {
        if dragged_id == target_id {
            return;
        }
        let mut order: Vec<String> = {
            let store = self.store.read(cx);
            let mut ids: Vec<String> = store.spaces.iter().map(|s| s.id.clone()).collect();
            let saved = &store.prefs.space_order;
            ids.sort_by_key(|id| saved.iter().position(|s| s == id).unwrap_or(usize::MAX));
            ids
        };
        if let Some(from) = order.iter().position(|id| id == dragged_id) {
            let moved = order.remove(from);
            if let Some(to) = order.iter().position(|id| id == target_id) {
                order.insert(to, moved);
                self.store.update(cx, |s, cx| s.update_prefs(|p| p.space_order = order, cx));
            }
        }
    }

    /// The spaces rail, as Discord's server list: the horo logo (Direct
    /// Messages), the joined spaces (drag-to-reorder), and Explore.
    fn rail(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let open = store.space.clone();
        let mut spaces = store.spaces.clone();
        let saved_order = store.prefs.space_order.clone();
        spaces.sort_by_key(|s| saved_order.iter().position(|id| id == &s.id).unwrap_or(usize::MAX));

        let home_badge: u64 = rooms::filter(&store.rooms, &View { spaces: &spaces, ..Default::default() })
            .into_iter()
            .map(|r| r.notification_count)
            .sum();
        let home_active = open.is_none();
        let home = rail_pill(
            home_active,
            home_badge > 0,
            palette,
            div()
                .id("space-home")
                .relative()
                .flex_none()
                .cursor_pointer()
                .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("direct messages").build(window, cx))
                .child(
                    div()
                        .size(zpx(46.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .map(|d| {
                            if home_active {
                                d.rounded(corner(14.))
                                    .bg(palette.raised)
                                    .border_2()
                                    .border_color(palette.accent)
                            } else {
                                d.rounded_full()
                                    .bg(palette.raised)
                                    .border_1()
                                    .border_color(palette.border_subtle)
                                    .hover(|d| d.rounded(corner(14.)).border_color(palette.accent))
                            }
                        })
                        .child(img(logo()).w(zpx(34.)).h(zpx(34. * 295.9 / 830.3)).flex_none()),
                )
                .when(home_badge > 0, |d| d.child(unread_badge(home_badge, palette)))
                .on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.open_space(None, cx)))),
        );
        let mut tiles = Vec::new();
        for space in spaces {
            let id = space.id.clone();
            let badge = space.notification_count;
            let active = open.as_deref() == Some(&space.id);
            let tile = rail_pill(
                active,
                badge > 0,
                palette,
                space_tile(
                    SharedString::from(format!("space-{}", space.id)),
                    &initials(&space.name),
                    space.avatar_url.as_deref(),
                    active,
                    accent_for(&space.id),
                    badge,
                    palette,
                    cx,
                )
                .tooltip({
                    let name = space.name.clone();
                    move |window, cx| gpui_component::tooltip::Tooltip::new(name.clone()).build(window, cx)
                })
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
                })
                .on_drag(DraggedSpace(space.id.clone()), {
                    let name = space.name.clone();
                    let bg = palette.raised;
                    let border = palette.accent;
                    let fg = palette.text;
                    move |_, _, _, cx| {
                        let name = name.clone();
                        cx.new(move |_| DragPill { label: name, bg, border, fg })
                    }
                })
                .drag_over::<DraggedSpace>(|style, _, _, _| style.opacity(0.6))
                .on_drop(cx.listener({
                    let target_id = space.id.clone();
                    move |this, dragged: &DraggedSpace, _, cx| {
                        this.reorder_space(&dragged.0, &target_id, cx);
                    }
                })),
            );
            tiles.push(tile);
        }

        v_flex()
            .id("rail")
            .relative()
            .w(zpx(72.))
            .h_full()
            .flex_none()
            .items_center()
            .gap(space(12.))
            .pt(zpx(44.))
            .pb(zpx(16.))
            .bg(palette.card)
            .border_r_1()
            .border_color(palette.border_subtle)
            // The window's title bar is gone; its top is where you drag it.
            .child(drag_strip(zpx(40.)))
            .child(home)
            .child(div().w(zpx(32.)).h(zpx(2.)).rounded_full().bg(palette.border_subtle).flex_none())
            .child(
                v_flex()
                    .id("rail-spaces")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .items_center()
                    .gap(space(10.))
                    .overflow_y_scroll()
                    .children(tiles)
                    .child(
                        div()
                            .id("rail-explore")
                            .size(zpx(46.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .border_1()
                            .border_dashed()
                            .border_color(palette.border)
                            .hover(|d| d.rounded(corner(14.)).bg(palette.raised))
                            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("explore spaces").build(window, cx))
                            .child(voice::icon("icons/compass.svg").size(zpx(20.)).text_color(palette.text_tertiary))
                            .on_click(cx.listener(|this, _, window, cx| this.open_explore(window, cx))),
                    ),
            )
    }

    /// Discord-style bottom-left user control bar: your avatar + status, audio
    /// settings shortcut, and settings gear.
    fn user_bar(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (info, shown, status_label) = {
            let store = self.store.read(cx);
            let Phase::SignedIn(info) = &store.phase else { return None };
            let (shown, label) = store.shown_own_presence();
            (info.clone(), shown, label)
        };
        let name = info.display_name.clone().unwrap_or_else(|| info.user_id.clone());
        let face = avatar(&info.user_id, &name, info.avatar_url.as_deref(), zpx(32.), cx);

        Some(
            h_flex()
                .flex_none()
                .items_center()
                .gap(space(6.))
                .px(space(10.))
                .py(space(8.))
                .bg(palette.inset)
                .border_t_1()
                .border_color(palette.border_subtle)
                .child(
                    h_flex()
                        .id("user-bar-me")
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap(space(8.))
                        .px(space(6.))
                        .py(space(4.))
                        .rounded(corner(10.))
                        .cursor_pointer()
                        .hover(|d| d.bg(palette.raised))
                        .on_click(cx.listener(|this, click: &gpui::ClickEvent, _, cx| {
                            this.status_menu = if this.status_menu.is_some() { None } else { Some(click.position()) };
                            cx.notify();
                        }))
                        .child(with_presence(face, Some(shown), palette.inset, palette))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().truncate().text_size(text(12.5)).font_weight(FontWeight::BOLD).text_color(palette.text).child(name))
                                .child(div().truncate().text_xs().text_color(palette.text_tertiary).child(status_label)),
                        ),
                )
                .child(
                    div()
                        .id("user-bar-audio")
                        .size(zpx(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(corner(8.))
                        .cursor_pointer()
                        .text_color(palette.text_tertiary)
                        .hover(|d| d.bg(palette.raised).text_color(palette.text))
                        .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("voice & media settings").build(window, cx))
                        .child(voice::icon("icons/headphones.svg").size(zpx(16.)).text_color(palette.text_tertiary))
                        .on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.show_settings(Some(SettingsSection::Voice), cx)))),
                )
                .child(
                    div()
                        .id("rail-settings")
                        .size(zpx(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(corner(8.))
                        .cursor_pointer()
                        .hover(|d| d.bg(palette.raised))
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new(if cfg!(target_os = "macos") { "settings (⌘,)" } else { "settings (ctrl+,)" }).build(window, cx)
                        })
                        .child(Icon::new(IconName::Settings).size(zpx(16.)).text_color(palette.text_tertiary))
                        .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
                )
                .into_any_element(),
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
        let picture = avatar(&room.id, &room.name, room.avatar_url.as_deref(), zpx(36.), cx);
        let face = div()
            .relative()
            .flex_none()
            .child(picture)
            .when(!room.is_direct && !room.is_space, |d| d.child(channel_badge(room.is_video_room, 16., palette.card, palette)))
            .children(shown.map(|s| presence_dot(s, zpx(12.), palette.app, palette)))
            .when(room.has_active_call, |d| {
                d.child(
                    div()
                        .absolute()
                        .right(zpx(-3.))
                        .bottom(zpx(-3.))
                        .size(zpx(14.))
                        .rounded_full()
                        .bg(palette.online)
                        .border_2()
                        .border_color(palette.card)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(voice::icon("icons/phone-call.svg").size(zpx(7.)).text_color(palette.app)),
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
            .when(room.is_encrypted, |d| d.child(voice::icon("icons/shield-check.svg").size(zpx(12.)).text_color(palette.accent).flex_none()))
            .when(room.is_muted, |d| d.child(voice::icon("icons/bell-off.svg").size(zpx(12.)).text_color(palette.text_tertiary).flex_none()));

        let badge = room.has_unread.then(|| {
            let count = room.notification_count;
            div()
                .min_w(zpx(19.))
                .h(zpx(19.))
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
            .mb(zpx(2.))
            .rounded(corner(16.))
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(Hsla { a: 0.07, ..gpui::white() }).shadow(vec![gpui::BoxShadow {
                    color: accent,
                    offset: gpui::point(zpx(3.), zpx(0.)),
                    blur_radius: zpx(0.),
                    spread_radius: zpx(0.),
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
                    .child(div().mt(zpx(1.)).truncate().text_size(text(12.)).text_color(palette.text_tertiary).child(preview.replace('\n', " "))),
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
                let room = room.clone();
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.open_room_menu(&room, event.position, cx);
                    cx.stop_propagation();
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

    /// A room in the space we haven't joined yet, as UnjoinedRow in RoomList.tsx.
    fn unjoined_row(&self, room: &PublicRoom, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let name = room.name.clone().or_else(|| room.alias.clone()).unwrap_or_else(|| "a room".into());
        let busy = self.joining.contains(&room.id);
        let picture = avatar(&room.id, &name, room.avatar_url.as_deref(), zpx(36.), cx);
        let face = div()
            .relative()
            .flex_none()
            .opacity(0.55)
            .child(picture)
            .child(channel_badge(room.is_voice, 16., palette.card, palette));
        let subtitle = if busy {
            "joining…"
        } else if room.is_voice {
            "click to join and hop in"
        } else {
            "click to join"
        };
        let target = room.clone();
        h_flex()
            .id(SharedString::from(format!("unjoined-{}", room.id)))
            .gap(space(10.))
            .px(space(10.))
            .py(space(9.))
            .mb(zpx(2.))
            .rounded(corner(16.))
            .cursor_pointer()
            .hover(|row| row.bg(Hsla { a: 0.05, ..gpui::white() }))
            .child(face)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().font_weight(FontWeight::BOLD).text_size(text(14.5)).text_color(palette.text_tertiary).child(name))
                    .child(div().mt(zpx(1.)).truncate().text_size(text(12.)).text_color(palette.text_tertiary).child(subtitle)),
            )
            .child(voice::icon("icons/circle-plus.svg").size(zpx(16.)).text_color(palette.text_tertiary).flex_none())
            .on_click(cx.listener(move |this, _, _, cx| this.join_unjoined(target.clone(), cx)))
    }

    /// The room list, as RoomList.tsx: the space's name, how much is
    /// unread, search and filters, then the sections.
    fn room_list(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let open_space = store.current_space().cloned();
        let spaces = store.spaces.clone();
        let selected = store.room.clone();
        let title = open_space.as_ref().map_or_else(|| "direct messages".to_owned(), |s| s.name.clone());
        let order = open_space.as_ref().map(|s| s.children.clone()).unwrap_or_default();
        let rooms = store.rooms.clone();
        let sync = store.sync;

        let search = self.search.read(cx).value().to_string();
        let hide_muted = open_space.as_ref().is_some_and(|s| store.prefs.hide_muted_in_spaces.contains(&s.id));
        let visible = rooms::filter(&rooms, &View { space: open_space.as_ref(), spaces: &spaces, filter: self.filter, search: &search, hide_muted });
        let unread_total: u64 = visible.iter().map(|r| r.notification_count).sum();
        let joined_groups = rooms::group(visible, &order);
        let hierarchy = open_space
            .as_ref()
            .and_then(|s| self.space_hierarchy.as_ref().filter(|(id, _, _)| id == &s.id).map(|(_, _, h)| h.as_slice()))
            .unwrap_or(&[]);
        let (unjoined_text, unjoined_voice) = match &open_space {
            Some(space) => rooms::unjoined_in_space(space, hierarchy, &rooms, self.filter, &search),
            None => (Vec::new(), Vec::new()),
        };

        // Fold unjoined channels into the matching sections (rooms before voice),
        // matching `withUnjoined` in src/store/index.ts.
        let mut groups: Vec<(rooms::Section, Vec<&RoomSummary>, Vec<&PublicRoom>)> =
            joined_groups.into_iter().map(|(s, r)| (s, r, Vec::new())).collect();
        for (section, extra) in [(rooms::Section::Rooms, unjoined_text), (rooms::Section::Voice, unjoined_voice)] {
            if extra.is_empty() {
                continue;
            }
            if let Some(existing) = groups.iter_mut().find(|(s, _, _)| *s == section) {
                existing.2 = extra;
            } else if section == rooms::Section::Rooms
                && let Some(at) = groups.iter().position(|(s, _, _)| *s == rooms::Section::Voice)
            {
                groups.insert(at, (section, Vec::new(), extra));
            } else {
                groups.push((section, Vec::new(), extra));
            }
        }

        let mut list = v_flex().id("room-list").flex_1().min_h_0().overflow_y_scroll().px(space(10.)).pb(space(16.));
        for (section, rooms, unjoined) in &groups {
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
                    .child(section.title(open_space.is_some()))
                    .child(div().flex_1().h(zpx(1.)).bg(palette.border_subtle))
                    .child((rooms.len() + unjoined.len()).to_string()),
            );
            for room in rooms {
                let is_selected = selected.as_deref() == Some(room.id.as_str());
                list = list.child(self.room_row(room, is_selected, palette, cx));
                if room.is_video_room {
                    list = list.children(voice::roster(room, &self.store, palette, cx));
                }
            }
            for room in unjoined {
                list = list.child(self.unjoined_row(room, palette, cx));
            }
        }
        if groups.is_empty() {
            let show_create = search.trim().is_empty() && self.filter == Filter::All && !rooms.is_empty();
            let empty = if !search.trim().is_empty() {
                "nothing matches that~".to_owned()
            } else if self.filter != Filter::All {
                format!("no {} rooms right now", filter_name(self.filter))
            } else if rooms.is_empty() {
                "loading rooms…".to_owned()
            } else if open_space.is_some() {
                "nothing here yet — this space has no rooms you've joined".to_owned()
            } else {
                "no direct messages yet~".to_owned()
            };
            list = list.child(
                v_flex()
                    .items_center()
                    .gap(space(14.))
                    .px(space(16.))
                    .py(space(40.))
                    .text_center()
                    .text_size(text(13.))
                    .text_color(palette.text_tertiary)
                    .child(empty)
                    .when(show_create, |d| {
                        d.child(Button::new("empty-create-room").small().primary().label("make or join a room").on_click(cx.listener(|this, _, window, cx| this.open_create(window, cx))))
                    }),
            );
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
            .mt(zpx(12.))
            .gap(space(8.))
            .px(space(14.))
            .py(space(6.))
            .rounded_full()
            .bg(palette.inset)
            .border_1()
            .border_color(palette.border_subtle)
            .child(Icon::new(IconName::Search).size(zpx(14.)).text_color(palette.text_tertiary))
            .child(div().flex_1().text_size(text(13.)).child(Input::new(&self.search).appearance(false)))
            .child(
                div()
                    .id("room-filters")
                    .relative()
                    .cursor_pointer()
                    .child(voice::icon("icons/funnel.svg").size(zpx(15.)).text_color(if filtered || self.filters_open { palette.accent } else { palette.text_tertiary }))
                    .when(filtered && !self.filters_open, |d| {
                        d.child(div().absolute().top(zpx(-2.)).right(zpx(-2.)).size(zpx(6.)).rounded_full().bg(palette.accent))
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filters_open = !this.filters_open;
                        cx.notify();
                    })),
            );
        let chips = self.filters_open.then(|| {
            h_flex().gap(space(6.)).mt(zpx(12.)).children([Filter::All, Filter::Unread, Filter::Dms, Filter::Muted].into_iter().map(|f| {
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

        let space_banner: Option<AnyElement> = open_space.as_ref().and_then(|sp| {
            let profile = self.store.read(cx).space_profile(&sp.id).cloned()?;
            let bg = if let Some(image) = profile.banner_url.as_deref().and_then(|mxc| crate::media::Media::image(mxc, Some((640, 280)), cx)) {
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .h(zpx(128.))
                    .overflow_hidden()
                    .child(gpui::StyledImage::object_fit(gpui::img(image).size_full(), gpui::ObjectFit::Cover))
            } else if let Some((a, b)) = profile.banner.as_deref().and_then(space_settings::banner_gradient) {
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .h(zpx(128.))
                    .bg(gpui::linear_gradient(135., gpui::linear_color_stop(gpui::rgb(a), 0.), gpui::linear_color_stop(gpui::rgb(b), 1.)))
            } else {
                return None;
            };
            Some(
                bg.child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(gpui::linear_gradient(
                            180.,
                            gpui::linear_color_stop(Hsla { a: 0.12, ..gpui::black() }, 0.),
                            gpui::linear_color_stop(palette.card, 1.),
                        )),
                )
                .into_any_element(),
            )
        });
        let has_banner = space_banner.is_some();

        let width = store_width(&self.store, Pane::RoomList, cx);
        v_flex()
            .relative()
            .w(zpx(width))
            .h_full()
            .flex_none()
            .bg(palette.card)
            .child(resize_handle(Pane::RoomList, palette))
            .border_r_1()
            .border_color(palette.border_subtle)
            .children(space_banner)
            .child(drag_strip(zpx(18.)))
            .child(
                v_flex()
                    .relative()
                    .px(space(18.))
                    .pt(if has_banner { zpx(76.) } else { space(18.) })
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
                                    .text_size(heading(20.))
                                    .text_color(palette.text)
                                    .child(title),
                            )
                            .child(
                                h_flex()
                                    .flex_none()
                                    .gap(space(10.))
                                    .pb(zpx(2.))
                                    .child(
                                        div()
                                            .font_family(palette.mono_font.clone())
                                            .text_size(text(11.))
                                            .text_color(if status.is_some() { palette.idle } else { palette.text_tertiary })
                                            .child(caught_up),
                                    )
                                    .when_some(open_space.clone(), |row, sp| {
                                        row.child(
                                            div()
                                                .id("space-settings-btn")
                                                .size(zpx(26.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded_full()
                                                .cursor_pointer()
                                                .bg(palette.card)
                                                .border_1()
                                                .border_color(palette.border)
                                                .hover(|d| d.bg(palette.raised))
                                                .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("space settings").build(window, cx))
                                                .child(Icon::new(IconName::Settings).size(zpx(13.)).text_color(palette.text_secondary))
                                                .on_click(cx.listener(move |this, _, window, cx| this.open_space_settings(sp.id.clone(), sp.name.clone(), SpaceSection::Profile, window, cx))),
                                        )
                                    })
                                    .child(
                                        div()
                                            .id("create-room")
                                            .size(zpx(26.))
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
                                            .child(voice::icon("icons/plus.svg").size(zpx(13.)).text_color(palette.text_secondary))
                                            .on_click(cx.listener(|this, _, window, cx| this.open_create(window, cx))),
                                    ),
                            ),
                    )
                    .child(search_box)
                    .children(chips),
            )
            .child(list)
            .children(voice::panel(&self.store, palette, cx))
            .children(self.user_bar(palette, cx))
    }

    /// The top of a room, as ChatPane.tsx: its face, name and lock, the
    /// person's presence (a DM) or the topic and alias, then the member
    /// count and the call button.
    fn room_header(&self, room: &RoomSummary, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let shown = room.dm_user_id.as_ref().and_then(|user| {
            let store = self.store.read(cx);
            presence::shown(store.presence.get(user), store.presence_supported).zip(store.presence.get(user).cloned())
        });
        let info_open = self.store.read(cx).prefs.show_info_panel;
        let face_store = self.store.clone();
        let picture = avatar(&room.id, &room.name, room.avatar_url.as_deref(), zpx(42.), cx);
        let face = div()
            .id("header-face")
            .relative()
            .flex_none()
            .cursor_pointer()
            .hover(|d| d.opacity(0.85))
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(if info_open { "hide room info" } else { "show room info" }).build(window, cx)
            })
            .on_click(move |_, _, cx| face_store.update(cx, |s, cx| s.update_prefs(|p| p.show_info_panel = !p.show_info_panel, cx)))
            .child(picture)
            .when(!room.is_direct, |d| d.child(channel_badge(room.is_video_room, 18., palette.app, palette)))
            .children(shown.as_ref().map(|(s, _)| presence_dot(*s, zpx(13.), palette.app, palette)));

        let subtitle: Option<AnyElement> = if room.dm_user_id.is_some() {
            shown.map(|(state, dto)| {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or_default();
                h_flex()
                    .mt(zpx(3.))
                    .gap(space(6.))
                    .font_family(palette.mono_font.clone())
                    .text_size(text(11.5))
                    .text_color(palette.text_tertiary)
                    .child(div().size(zpx(7.)).rounded_full().flex_none().bg(match state {
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
                    .max_w(zpx(520.))
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
            .child(voice::icon(if in_this_call { "icons/phone-off.svg" } else { "icons/phone-call.svg" }).size(zpx(17.)).text_color(
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

        let muted = room.is_muted;
        let mute_room_id = room.id.clone();
        let mute_button = div()
            .id("header-mute")
            .p(space(9.))
            .rounded(corner(12.))
            .cursor_pointer()
            .bg(palette.raised)
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(if muted { "unmute notifications" } else { "mute notifications" }).build(window, cx)
            })
            .child(voice::icon(if muted { "icons/bell-off.svg" } else { "icons/bell.svg" }).size(zpx(17.)).text_color(if muted {
                palette.idle
            } else {
                palette.text_secondary
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.on_rooms(vec![mute_room_id.clone()], cx, move |core, id| {
                    Box::pin(async move {
                        if muted {
                            core_rooms::set_muted(&core, &id, false).await
                        } else {
                            core_rooms::set_notify_mode(&core, &id, NotifyMode::Mute).await
                        }
                    })
                });
            }));

        let pins_button = div()
            .id("header-pins")
            .p(space(9.))
            .rounded(corner(12.))
            .cursor_pointer()
            .bg(palette.raised)
            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("pinned messages").build(window, cx))
            .child(voice::icon("icons/flag.svg").size(zpx(17.)).text_color(palette.text_secondary))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some((_, timeline, _)) = &this.timeline {
                    timeline.update(cx, |t, cx| t.toggle_pins(cx));
                }
            }));

        let search_button = div()
            .id("header-search")
            .p(space(9.))
            .rounded(corner(12.))
            .cursor_pointer()
            .bg(palette.raised)
            .tooltip(|window, cx| {
                gpui_component::tooltip::Tooltip::new(if cfg!(target_os = "macos") { "search messages (⌘F)" } else { "search messages (ctrl+f)" }).build(window, cx)
            })
            .child(voice::icon("icons/search.svg").size(zpx(17.)).text_color(palette.text_secondary))
            .on_click(cx.listener(|this, _, window, cx| {
                if let Some((_, timeline, _)) = &this.timeline {
                    timeline.update(cx, |t, cx| t.open_search(window, cx));
                }
            }));

        let info_open = self.store.read(cx).prefs.show_info_panel;
        let store = self.store.clone();
        let info_button = div()
            .id("header-info")
            .p(space(9.))
            .rounded(corner(12.))
            .cursor_pointer()
            .bg(palette.raised)
            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("room info").build(window, cx))
            .child(voice::icon("icons/info.svg").size(zpx(17.)).text_color(if info_open { palette.accent } else { palette.text_secondary }))
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
            .child(drag_strip(zpx(12.)))
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
                            .when(room.is_encrypted, |d| d.child(voice::icon("icons/lock.svg").size(zpx(14.)).text_color(palette.accent).flex_none())),
                    )
                    .children(subtitle),
            )
            .child(
                h_flex()
                    .ml_auto()
                    .gap(space(8.))
                    .child(div().mr(zpx(4.)).font_family(palette.mono_font.clone()).text_size(text(11.)).text_color(palette.text_tertiary).child(room.member_count.to_string()))
                    .child(mute_button)
                    .child(pins_button)
                    .child(search_button)
                    .child(call_button)
                    .child(info_button),
            )
            .into_any_element()
    }

    fn room_pane(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let room = store.room.as_ref().and_then(|id| store.rooms.iter().find(|r| r.id == *id)).cloned();
        let (thread, thread_preview) = (store.thread.clone(), store.thread_preview.clone());

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
                .child(div().absolute().left_0().top_0().bottom_0().w(zpx(2.)).bg(palette.accent_quaternary))
                .child(voice::icon("icons/messages-square.svg").size(zpx(15.)).text_color(palette.accent_quaternary))
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
        let waiting = {
            let rooms = &self.store.read(cx).rooms;
            if rooms.is_empty() {
                "still syncing your rooms…".to_owned()
            } else {
                let count = rooms.iter().filter(|r| !r.is_space && !r.is_utility).count();
                format!("{count} rooms waiting for you")
            }
        };
        let body = match (&room, &self.timeline) {
            (Some(room), Some((_, timeline, composer))) => v_flex()
                .relative()
                .flex_1()
                .min_h_0()
                .children(thread_bar)
                .child(div().flex_1().min_h_0().child(timeline.clone()))
                .child(composer.clone())
                .child(drop_zone(&room.name, in_thread, composer.clone(), palette)),
            _ => v_flex()
                .m_auto()
                .items_center()
                .gap(space(14.))
                .text_color(palette.text_tertiary)
                .child(
                    div()
                        .size(zpx(84.))
                        .rounded(corner(28.))
                        .bg(palette.card)
                        .border_2()
                        .border_dashed()
                        .border_color(palette.border_strong)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(voice::icon("icons/messages-square.svg").size(zpx(34.)).text_color(palette.text_tertiary)),
                )
                .child(div().font_family(palette.heading_font.clone()).font_weight(FontWeight::EXTRA_BOLD).text_size(heading(19.)).text_color(palette.text_secondary).child("pick a room~"))
                .child(div().text_size(text(13.)).child(waiting)),
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
                if this.store.read(cx).lightbox.is_some() {
                    match event.keystroke.key.as_str() {
                        "escape" => {
                            this.store.update(cx, |s, cx| s.close_lightbox(cx));
                            cx.stop_propagation();
                            return;
                        }
                        "left" => {
                            this.store.update(cx, |s, cx| s.step_lightbox(-1, cx));
                            cx.stop_propagation();
                            return;
                        }
                        "right" => {
                            this.store.update(cx, |s, cx| s.step_lightbox(1, cx));
                            cx.stop_propagation();
                            return;
                        }
                        _ => {}
                    }
                }
                // Then the call filling the window.
                else if event.keystroke.key == "escape" && this.store.read(cx).call.as_ref().is_some_and(|c| c.fullscreen) {
                    this.store.update(cx, |s, cx| s.set_call_fullscreen(false, cx));
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
            // Escape closes open menus / call mixer, as a click elsewhere does.
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    let closed_local = this.space_menu.take().is_some() | this.room_menu.take().is_some();
                    let closed_call = this.store.read(cx).call_participant_menu.is_some()
                        || this.store.read(cx).call.as_ref().is_some_and(|c| c.mixer_open);
                    if closed_call {
                        this.store.update(cx, |s, cx| {
                            s.close_participant_menu(cx);
                            s.close_call_mixer(cx);
                        });
                    }
                    if closed_local || closed_call {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| {
                if this.store.read(cx).settings.is_none() {
                    this.open_settings(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenQuickSwitcher, window, cx| {
                this.open_quick_switcher(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FindInRoom, window, cx| {
                if let Some((_, timeline, _)) = &this.timeline {
                    timeline.update(cx, |t, cx| t.open_search(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &PrevRoom, _, cx| this.step_room(-1, false, cx)))
            .on_action(cx.listener(|this, _: &NextRoom, _, cx| this.step_room(1, false, cx)))
            .on_action(cx.listener(|this, _: &PrevUnreadRoom, _, cx| this.step_room(-1, true, cx)))
            .on_action(cx.listener(|this, _: &NextUnreadRoom, _, cx| this.step_room(1, true, cx)))
            .size_full()
            .bg(palette.app)
            .text_color(palette.text)
            .font_family(palette.body_font.clone())
            // A column's edge being dragged: follow the pointer, save the
            // width when it's let go.
            .on_drag_move(cx.listener(|this, event: &gpui::DragMoveEvent<Pane>, window, cx| {
                let pane = *event.drag(cx);
                // The pointer is in screen pixels; widths are kept in the
                // design's, before zoom.
                let (x, zoom) = (f32::from(event.event.position.x), crate::theme::zoom());
                let width = match pane {
                    Pane::RoomList => x / zoom - RAIL_WIDTH,
                    Pane::InfoPanel => (f32::from(window.viewport_size().width) - x) / zoom,
                };
                this.store.update(cx, |s, cx| s.preview_prefs(|p| pane.set(p, clamp_pane(width)), cx));
            }))
            .on_drop(cx.listener(|this, _: &Pane, _, cx| this.store.update(cx, |s, cx| s.update_prefs(|_| {}, cx))))
            .on_drag_move(cx.listener(|this, event: &gpui::DragMoveEvent<voice::DraggedCallPip>, window, cx| {
                let zoom = crate::theme::zoom();
                let pos = (f32::from(event.event.position.x) / zoom, f32::from(event.event.position.y) / zoom);
                let vp = window.viewport_size();
                let viewport = (f32::from(vp.width) / zoom, f32::from(vp.height) / zoom);
                this.store.update(cx, |s, cx| s.move_call_pip_drag(pos, viewport, cx));
            }))
            .on_drop(cx.listener(|this, _: &voice::DraggedCallPip, window, cx| {
                let zoom = crate::theme::zoom();
                let vp = window.viewport_size();
                let viewport = (f32::from(vp.width) / zoom, f32::from(vp.height) / zoom);
                this.store.update(cx, |s, cx| s.end_call_pip_drag(viewport, cx));
            }))
            .on_drag_move(cx.listener(|this, event: &gpui::DragMoveEvent<voice::ResizedCallPip>, window, cx| {
                let edge = event.drag(cx).0;
                let zoom = crate::theme::zoom();
                let pos = (f32::from(event.event.position.x) / zoom, f32::from(event.event.position.y) / zoom);
                let vp = window.viewport_size();
                let viewport = (f32::from(vp.width) / zoom, f32::from(vp.height) / zoom);
                this.store.update(cx, |s, cx| s.move_call_pip_resize(edge, pos, viewport, cx));
            }))
            .on_drop(cx.listener(|this, _: &voice::ResizedCallPip, _, cx| {
                this.store.update(cx, |s, cx| s.end_call_pip_resize(cx));
            }))
            .child(self.rail(&palette, cx))
            .child(self.room_list(&palette, cx))
            // Cameras move: draw again next frame while one is in the call.
            .when(voice::any_camera(&self.store, cx), |d| {
                window.request_animation_frame();
                d
            })
            .map(|d| match self.invite.clone() {
                Some(invite) => d.child(invite),
                None => d
                    .child(self.room_pane(&palette, cx)).when(self.store.read(cx).prefs.show_info_panel, |d| d.children(self.info.clone())),
            })
            .relative()
            .children(self.settings.clone())
            .children(self.quick_switcher.clone())
            .children(voice::floating(&self.store, &palette, window, cx))
            .children(voice::fullscreen(&self.store, &palette, window, cx))
            .children(voice::mixer(&self.store, &palette, window, cx))
            .children(voice::participant_menu(&self.store, &palette, window, cx))
            .children(lightbox::lightbox(&self.store, &palette, window, cx))
            .children(voice::ringing(&self.store, &palette, cx))
            .children(self.explore.clone())
            .children(self.create.clone())
            .children(self.room_settings.clone())
            .children(self.invite_to_space.clone())
            .children(self.space_settings.clone())
            .children(self.leave_space.clone())
            .children(self.browse.clone())
            .children(self.space_menu(&palette, cx))
            .children(self.room_menu(&palette, cx))
            .children(self.status_popover(&palette, cx))
            .children(self.profile.clone())
            .children(banner(&self.store, &palette, cx))
            .children(update_banner(&self.store, &palette, cx))
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
            .bottom(zpx(22.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                h_flex()
                    .occlude()
                    .max_w(zpx(520.))
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
                            .size(zpx(16.))
                            .text_color(if error { palette.danger } else { palette.accent_quaternary }),
                    )
                    // Shrinks and wraps, so a long message can't push the ✕
                    // into the padding.
                    .child(div().flex_1().min_w_0().text_size(text(13.)).line_height(gpui::relative(1.45)).text_color(palette.text_secondary).child(banner.message))
                    .child(
                        div()
                            .id("banner-dismiss")
                            .flex_none()
                            .size(zpx(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .text_color(palette.text_tertiary)
                            .hover(|d| d.text_color(palette.text).bg(palette.inset))
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

/// A new version is out: at the top, in the middle (the bottom is the
/// app's own banner's), until it's waved away for that version.
fn update_banner(store: &Entity<Store>, palette: &Palette, cx: &mut gpui::App) -> Option<AnyElement> {
    let state = store.read(cx);
    let release = state.update.release()?;
    // Waving it away hides an offer; one being installed stays.
    if matches!(state.update, crate::store::UpdateState::Available(_)) && state.prefs.dismissed_update.as_deref() == Some(release.version.as_str()) {
        return None;
    }
    let (line, action) = update_offer(&state.update.clone(), store, "update-banner")?;
    let dismiss = store.clone();
    Some(
        div()
            .absolute()
            .top(zpx(14.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                h_flex()
                    .id("update-banner")
                    .occlude()
                    .max_w(zpx(520.))
                    .gap(space(12.))
                    .px(space(16.))
                    .py(space(10.))
                    .rounded(corner(18.))
                    .bg(palette.raised)
                    .border_1()
                    .border_color(palette.accent)
                    .shadow_lg()
                    .child(voice::icon("icons/sparkles.svg").size(zpx(15.)).text_color(palette.accent))
                    .child(
                        div()
                            .text_size(text(13.))
                            .line_height(gpui::relative(1.45))
                            .text_color(palette.text_secondary)
                            .child(line),
                    )
                    .child(action)
                    .child(
                        div()
                            .id("update-dismiss")
                            .cursor_pointer()
                            .text_size(text(12.))
                            .text_color(palette.text_tertiary)
                            .hover(|d| d.text_color(palette.text))
                            .child("✕")
                            .on_click(move |_, _, cx| dismiss.update(cx, |s, cx| s.dismiss_update(cx))),
                    ),
            )
            .into_any_element(),
    )
}

/// What to say about a release on offer, and the button that moves it
/// along: install → installing… → restart where this install can update
/// itself ([`crate::updater::installable`]), the release page otherwise.
pub fn update_offer(state: &crate::store::UpdateState, store: &Entity<Store>, id: &'static str) -> Option<(String, Button)> {
    use crate::store::UpdateState;
    let release = state.release()?;
    let button = Button::new(SharedString::from(format!("{id}-action"))).small().primary();
    Some(match state {
        UpdateState::Available(_) if crate::updater::installable() => {
            let store = store.clone();
            (format!("horo {} is ready to install", release.version), button.label("install").on_click(move |_, _, cx| store.update(cx, |s, cx| s.install_update(cx))))
        }
        UpdateState::Available(_) => {
            let url = release.url.clone();
            (
                format!("horo {} is out — this build updates from where you installed it", release.version),
                button.label("open release").on_click(move |_, _, cx| cx.open_url(&url)),
            )
        }
        UpdateState::Installing(_) => (format!("installing horo {}…", release.version), gpui_component::Disableable::disabled(button.label("installing…"), true)),
        UpdateState::Installed(_) => {
            (format!("horo {} is installed — restart to finish", release.version), button.label("restart").on_click(|_, _, _| crate::updater::relaunch()))
        }
        _ => return None,
    })
}

/// The rail's width: where the room list starts.
const RAIL_WIDTH: f32 = 76.;

/// A column whose width can be dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    RoomList,
    InfoPanel,
}

impl Pane {
    fn set(self, prefs: &mut crate::prefs::Prefs, width: f32) {
        match self {
            Pane::RoomList => prefs.room_list_width = width,
            Pane::InfoPanel => prefs.info_panel_width = width,
        }
    }
}

fn clamp_pane(width: f32) -> f32 {
    width.clamp(crate::prefs::PANE_MIN, crate::prefs::PANE_MAX).round()
}

/// A column's width as saved, kept in range (an old or hand-edited
/// prefs file can say anything).
pub fn store_width(store: &Entity<Store>, pane: Pane, cx: &gpui::App) -> f32 {
    let prefs = &store.read(cx).prefs;
    clamp_pane(match pane {
        Pane::RoomList => prefs.room_list_width,
        Pane::InfoPanel => prefs.info_panel_width,
    })
}

/// The grabbable strip on a column's inner edge.
pub fn resize_handle(pane: Pane, palette: &Palette) -> impl IntoElement {
    let strip = div()
        .id(match pane {
            Pane::RoomList => "resize-room-list",
            Pane::InfoPanel => "resize-info-panel",
        })
        .absolute()
        .top_0()
        .bottom_0()
        .w(zpx(8.))
        .cursor_col_resize()
        .hover(|d| d.bg(Hsla { a: 0.35, ..palette.accent }))
        .on_drag(pane, |_, _, _, cx| cx.new(|_| gpui::EmptyView))
        // Not a click on whatever is under it.
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation());
    match pane {
        Pane::RoomList => strip.right(zpx(-4.)),
        Pane::InfoPanel => strip.left(zpx(-4.)),
    }
}

pub(crate) fn logo() -> std::sync::Arc<gpui::Image> {
    static LOGO: std::sync::OnceLock<std::sync::Arc<gpui::Image>> = std::sync::OnceLock::new();
    LOGO.get_or_init(|| std::sync::Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Svg, crate::LOGO.to_vec()))).clone()
}

/// A tile in the rail: a space's picture, or its initials on its accent
/// when it's the open one (on the raised surface otherwise).
#[allow(clippy::too_many_arguments)]
struct DraggedSpace(String);

struct DragPill {
    label: String,
    bg: Hsla,
    border: Hsla,
    fg: Hsla,
}

impl Render for DragPill {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(space(10.))
            .py(space(6.))
            .rounded(corner(10.))
            .bg(self.bg)
            .border_1()
            .border_color(self.border)
            .text_size(text(12.))
            .font_weight(FontWeight::BOLD)
            .text_color(self.fg)
            .shadow_md()
            .child(self.label.clone())
    }
}

/// A rail entry wrapped with Discord's left-edge active/unread pill indicator.
fn rail_pill(active: bool, unread: bool, palette: &Palette, child: impl IntoElement) -> impl IntoElement {
    div()
        .relative()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .when(active || unread, |d| {
            d.child(
                div()
                    .absolute()
                    .left_0()
                    .w(zpx(4.))
                    .h(zpx(if active { 34. } else { 8. }))
                    .rounded_r_full()
                    .bg(if active { palette.accent } else { palette.text }),
            )
        })
        .child(child)
}

fn unread_badge(badge: u64, palette: &Palette) -> impl IntoElement {
    div()
        .absolute()
        .top(zpx(-4.))
        .right(zpx(-4.))
        .min_w(zpx(18.))
        .h(zpx(18.))
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
        .child(if badge > 99 { "99+".to_owned() } else { badge.to_string() })
}

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
        .size(zpx(46.))
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .text_size(text(13.))
        .font_weight(FontWeight::EXTRA_BOLD)
        .map(|d| {
            if active {
                d.rounded(corner(14.))
                    .bg(if picture.is_some() { gpui::transparent_black() } else { accent })
                    .text_color(palette.text_on_accent)
                    .border_2()
                    .border_color(palette.app)
                    .shadow(vec![gpui::BoxShadow {
                        color: Hsla { h: 0., s: 0., l: 0., a: 0.28 },
                        offset: gpui::point(zpx(0.), zpx(2.)),
                        blur_radius: zpx(0.),
                        spread_radius: zpx(0.),
                        inset: false,
                    }])
            } else {
                d.rounded_full()
                    .bg(palette.raised)
                    .text_color(palette.text_secondary)
                    .border_1()
                    .border_color(palette.border_subtle)
                    .hover(|d| d.rounded(corner(14.)))
            }
        })
        .map(|d| match picture {
            // Rounded itself, inside the border (2 px open, 1 otherwise):
            // gpui clips children to a rectangle.
            Some(image) => {
                let border = zpx(if active { 2. } else { 1. });
                let r = if active { corner(14.) - border } else { zpx(23.) };
                // Square, so a portrait photo can't grow: see `avatar`.
                d.child(img(image).size(zpx(46.) - border * 2.).aspect_square().rounded(r).object_fit(gpui::ObjectFit::Cover))
            }
            None => d.child(label_text.to_uppercase()),
        });
    div()
        .id(id)
        .relative()
        .flex_none()
        .cursor_pointer()
        .child(tile)
        .when(badge > 0, |d| d.child(unread_badge(badge, palette)))
}

/// What kind of room this is, in the avatar's corner: `#` for text, a
/// camera for a video (voice) room. DMs get none.
fn channel_badge(video: bool, size: f32, ring: Hsla, palette: &Palette) -> impl IntoElement {
    div()
        .absolute()
        .left(zpx(-4.))
        .bottom(zpx(-4.))
        .size(zpx(size))
        .rounded_full()
        .bg(if video { palette.accent_tertiary } else { palette.inset })
        .border_2()
        .border_color(ring)
        .flex()
        .items_center()
        .justify_center()
        .map(|d| {
            if video {
                d.child(voice::icon("icons/video.svg").size(zpx(size * 0.5)).text_color(palette.app))
            } else {
                d.child(div().text_size(zpx(size * 0.62)).font_weight(FontWeight::EXTRA_BOLD).text_color(palette.text_secondary).child("#"))
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
                .gap(zpx(14.))
                .px(zpx(64.))
                .py(zpx(44.))
                .rounded(zpx(28.))
                .border_2()
                .border_dashed()
                .border_color(palette.accent)
                .bg(palette.raised)
                .shadow_lg()
                .child(div().font_family(palette.heading_font.clone()).font_weight(gpui::FontWeight::EXTRA_BOLD).text_size(zpx(22.)).child("drop it here~"))
                .child(div().max_w(zpx(320.)).text_center().text_size(zpx(13.)).text_color(palette.text_secondary).child(where_to)),
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

