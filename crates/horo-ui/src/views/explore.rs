//! Browsing public spaces, as ExploreSpaces.tsx: a server's directory,
//! searchable, each space opening to show the rooms inside before you join.
//! Opened from the compass in the spaces rail.

use std::{collections::HashMap, time::Duration};

use gpui::{
    AnyElement, AppContext as _, AsyncApp, Context, Entity, EventEmitter, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Task, WeakEntity, Window, div, prelude::FluentBuilder,
};
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use horo_core::{
    dto::RoomMembership,
    matrix::{
        directory::{self, PublicRoom},
        rooms,
    },
};
use matrix_sdk::ruma::OwnedRoomId;

use crate::theme::zpx;
use crate::{
    backend::Backend,
    store::{Phase, Store},
    theme::{Palette, corner, heading, space, text},
    views::{
        avatar::avatar,
        modal::{accent_button, label, modal, well},
        voice::icon,
    },
};

/// How long typing pauses before the directory is asked again.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(350);

pub struct Closed;

/// What a space's "inside" holds: loading, the rooms, or why not.
enum Inside {
    Loading,
    Rooms(Vec<PublicRoom>),
    Failed(String),
}

pub struct Explore {
    store: Entity<Store>,
    focus: FocusHandle,
    search: Entity<InputState>,
    server: Entity<InputState>,
    /// What the list shows: (search, server), trimmed.
    query: (String, String),
    spaces: Vec<PublicRoom>,
    next_batch: Option<String>,
    loading: bool,
    error: Option<String>,
    expanded: Option<String>,
    inside: HashMap<String, Inside>,
    /// Joins in flight, by room or space id.
    busy: HashMap<String, ()>,
    /// Every fetch takes a ticket; an answer to an older one is dropped.
    ticket: u64,
    debounce: Option<Task<()>>,
    /// Browsing one space's rooms (from its menu in the rail) rather than
    /// the directory.
    browse: Option<PublicRoom>,
}

impl EventEmitter<Closed> for Explore {}

impl Explore {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Explore {
        let own_server = match &store.read(cx).phase {
            Phase::SignedIn(info) => info.user_id.split_once(':').map(|(_, s)| s.to_owned()).unwrap_or_default(),
            _ => String::new(),
        };
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("name or topic"));
        let server = cx.new(|cx| InputState::new(window, cx).placeholder(if own_server.is_empty() { "yours".to_owned() } else { own_server }));
        for input in [&search, &server] {
            cx.subscribe(input, |this: &mut Self, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.typed(cx);
                }
            })
            .detach();
        }
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        search.update(cx, |s, cx| s.focus(window, cx));
        let mut explore = Explore {
            store,
            focus: cx.focus_handle(),
            search,
            server,
            query: (String::new(), String::new()),
            spaces: Vec::new(),
            next_batch: None,
            loading: false,
            error: None,
            expanded: None,
            inside: HashMap::new(),
            busy: HashMap::new(),
            ticket: 0,
            debounce: None,
            browse: None,
        };
        explore.load(None, cx);
        explore
    }

    /// Everything in one space, rooms not joined yet included, as
    /// SpaceDialogs.tsx's BrowseRooms.
    pub fn browse(store: Entity<Store>, space_id: String, name: String, window: &mut Window, cx: &mut Context<Self>) -> Explore {
        let mut explore = Explore {
            store,
            focus: cx.focus_handle(),
            search: cx.new(|cx| InputState::new(window, cx)),
            server: cx.new(|cx| InputState::new(window, cx)),
            query: (String::new(), String::new()),
            spaces: Vec::new(),
            next_batch: None,
            loading: false,
            error: None,
            expanded: None,
            inside: HashMap::new(),
            busy: HashMap::new(),
            ticket: 0,
            debounce: None,
            browse: Some(PublicRoom {
                id: space_id.clone(),
                name: Some(name),
                topic: None,
                alias: None,
                avatar_url: None,
                members: 0,
                is_space: true,
                is_voice: false,
                join_rule: "invite".into(),
                via: Vec::new(),
            }),
        };
        explore.focus.focus(window, cx);
        explore.toggle(space_id, cx);
        explore
    }

    fn typed(&mut self, cx: &mut Context<Self>) {
        // Replacing the task drops (cancels) the last one.
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                let next = (this.search.read(cx).value().trim().to_owned(), this.server.read(cx).value().trim().to_owned());
                if next != this.query {
                    this.query = next;
                    this.expanded = None;
                    this.load(None, cx);
                }
            });
        }));
    }

    fn load(&mut self, since: Option<String>, cx: &mut Context<Self>) {
        self.ticket += 1;
        let mine = self.ticket;
        self.loading = true;
        self.error = None;
        let (search, server) = self.query.clone();
        let more = since.is_some();
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move {
                let core = state.core().await?;
                directory::public_spaces(&core, Some(server).filter(|s| !s.is_empty()), Some(search).filter(|s| !s.is_empty()), since).await
            }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                if mine != this.ticket {
                    return;
                }
                this.loading = false;
                match outcome {
                    Ok(page) => {
                        if more {
                            this.spaces.extend(page.rooms);
                        } else {
                            this.spaces = page.rooms;
                        }
                        this.next_batch = page.next_batch;
                    }
                    Err(e) => {
                        if !more {
                            this.spaces.clear();
                        }
                        this.next_batch = None;
                        this.error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn toggle(&mut self, space_id: String, cx: &mut Context<Self>) {
        if self.expanded.as_deref() == Some(space_id.as_str()) {
            self.expanded = None;
            cx.notify();
            return;
        }
        self.expanded = Some(space_id.clone());
        if !self.inside.contains_key(&space_id) {
            self.inside.insert(space_id.clone(), Inside::Loading);
            let Ok(id) = OwnedRoomId::try_from(space_id.as_str()) else { return };
            let backend = Backend::global(cx);
            let task = backend.run({
                let state = backend.state.clone();
                async move { directory::space_rooms(&*state.core().await?, &id).await }
            });
            cx.spawn(async move |this, cx| {
                let outcome = task.await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
                let _ = this.update(cx, |this, cx| {
                    this.inside.insert(space_id, match outcome {
                        Ok(rooms) => Inside::Rooms(rooms),
                        Err(e) => Inside::Failed(e),
                    });
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(Closed);
    }

    /// Join a space (its public rooms come too, as joining a Discord server
    /// does), wait for it to reach the rail, and open it.
    fn join_space(&mut self, space: &PublicRoom, cx: &mut Context<Self>) {
        let Ok(id) = OwnedRoomId::try_from(space.id.as_str()) else { return };
        let (space_id, via) = (space.id.clone(), space.via.clone());
        self.busy.insert(space_id.clone(), ());
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { rooms::join_space(&*state.core().await?, &id, &via).await }
        });
        let store = self.store.downgrade();
        cx.spawn(async move |this, cx| {
            let outcome = task.await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
            if outcome.is_ok() {
                let id = space_id.clone();
                wait_for(&store, cx, true, move |s| s.spaces.iter().any(|sp| sp.id == id)).await;
            }
            let _ = this.update(cx, |this, cx| {
                this.busy.remove(&space_id);
                match outcome {
                    Ok(_) => {
                        this.store.update(cx, |s, cx| s.open_space(Some(space_id), cx));
                        this.close(cx);
                    }
                    Err(e) => this.error = Some(format!("couldn't join: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Join one room out of a space. The dialog stays: picking several out
    /// of a space is the usual case, and the row turns to "open" once sync
    /// has it.
    fn join_room(&mut self, room: &PublicRoom, fallback_via: Vec<String>, cx: &mut Context<Self>) {
        let (room_id, is_space) = (room.id.clone(), room.is_space);
        let via = if room.via.is_empty() { fallback_via } else { room.via.clone() };
        self.busy.insert(room_id.clone(), ());
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, id) = (backend.state.clone(), room_id.clone());
            async move { rooms::join(&*state.core().await?, &id, &via).await }
        });
        let store = self.store.downgrade();
        cx.spawn(async move |this, cx| {
            let outcome = task.await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
            if outcome.is_ok() {
                let id = room_id.clone();
                wait_for(&store, cx, is_space, move |s| {
                    if is_space { s.spaces.iter().any(|sp| sp.id == id) } else { s.rooms.iter().any(|r| r.id == id) }
                })
                .await;
            }
            let _ = this.update(cx, |this, cx| {
                this.busy.remove(&room_id);
                if let Err(e) = outcome {
                    this.error = Some(format!("couldn't join: {e}"));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open(&mut self, room: &PublicRoom, cx: &mut Context<Self>) {
        let id = room.id.clone();
        if room.is_space {
            self.store.update(cx, |s, cx| s.open_space(Some(id), cx));
        } else {
            self.store.update(cx, |s, cx| s.open_room(id, cx));
        }
        self.close(cx);
    }

    fn joined(&self, room: &PublicRoom, cx: &Context<Self>) -> bool {
        let store = self.store.read(cx);
        if room.is_space {
            store.spaces.iter().any(|s| s.id == room.id)
        } else {
            store.rooms.iter().any(|r| r.id == room.id && r.membership == RoomMembership::Joined)
        }
    }

    /// Open, join, or why you can't.
    fn action(&mut self, room: &PublicRoom, top_level: bool, fallback_via: &[String], palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = SharedString::from(format!("act-{}", room.id));
        if self.joined(room, cx) {
            let room = room.clone();
            return Button::new(id).ghost().small().label("open").on_click(cx.listener(move |this, _, _, cx| this.open(&room, cx))).into_any_element();
        }
        if room.join_rule != "public" {
            let words = match room.join_rule.as_str() {
                "knock" | "knock_restricted" => "ask to join",
                "restricted" => "members only",
                _ => "invite only",
            };
            return h_flex()
                .gap(space(4.))
                .px(space(8.))
                .py(space(3.))
                .rounded_full()
                .border_1()
                .border_color(palette.border_subtle)
                .text_size(text(10.5))
                .text_color(palette.text_tertiary)
                .child(icon("icons/lock.svg").size(zpx(10.)).text_color(palette.text_tertiary))
                .child(words)
                .into_any_element();
        }
        let busy = self.busy.contains_key(&room.id);
        let (room, via) = (room.clone(), fallback_via.to_vec());
        let words = if busy { "joining…" } else { "join" };
        let button = if top_level {
            accent_button(id, words, !busy, true, palette)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.busy.contains_key(&room.id) {
                        this.join_space(&room, cx)
                    }
                }))
                .into_any_element()
        } else {
            Button::new(id)
                .ghost()
                .small()
                .label(words)
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| this.join_room(&room, via.clone(), cx)))
                .into_any_element()
        };
        v_flex()
            .items_end()
            .child(button)
            // Joining a big space on another server can take minutes; a
            // spinner that long reads as stuck.
            .when(busy && top_level, |d| {
                d.child(div().mt(zpx(6.)).max_w(zpx(110.)).text_right().text_size(text(10.5)).text_color(palette.text_tertiary).child("big spaces can take a minute~"))
            })
            .into_any_element()
    }

    fn space_row(&mut self, sp: &PublicRoom, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let expanded = self.expanded.as_deref() == Some(sp.id.as_str());
        let name = display_name(sp);
        let face = avatar(&sp.id, &name, sp.avatar_url.as_deref(), zpx(44.), cx);
        let action = self.action(sp, true, &[], palette, cx);
        let inside = expanded.then(|| self.inside_list(sp, palette, cx));
        let space_id = sp.id.clone();
        v_flex()
            .rounded(corner(16.))
            .border_1()
            .border_color(if expanded { palette.border } else { gpui::transparent_black() })
            .when(expanded, |d| d.bg(palette.inset))
            .child(
                h_flex()
                    .id(SharedString::from(format!("space-row-{}", sp.id)))
                    .items_start()
                    .gap(space(12.))
                    .p(space(10.))
                    .rounded(corner(16.))
                    .cursor_pointer()
                    .hover(|d| d.bg(Hsla { a: 0.04, ..gpui::white() }))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(space_id.clone(), cx)))
                    .child(div().flex_none().child(face))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap(space(6.))
                                    .child(div().truncate().font_family(palette.heading_font.clone()).font_weight(FontWeight::EXTRA_BOLD).text_size(heading(14.5)).child(name))
                                    .child(icon(if expanded { "icons/chevron-up.svg" } else { "icons/chevron-down.svg" }).size(zpx(12.)).text_color(palette.text_tertiary)),
                            )
                            .child(
                                h_flex()
                                    .mt(zpx(2.))
                                    .gap(space(8.))
                                    .min_w_0()
                                    .text_size(text(11.5))
                                    .text_color(palette.text_tertiary)
                                    .child(div().flex_none().child(member_count(sp.members)))
                                    .children(sp.alias.clone().map(|a| div().truncate().font_family(palette.mono_font.clone()).child(a))),
                            )
                            .children(sp.topic.clone().map(|t| {
                                let t = if expanded { t } else { first_lines(&t, 2, 160) };
                                div().mt(zpx(5.)).text_size(text(12.5)).line_height(gpui::relative(1.45)).text_color(palette.text_secondary).child(t)
                            })),
                    )
                    .child(div().flex_none().pt(space(2.)).child(action)),
            )
            .children(inside.map(|list| div().pl(zpx(66.)).pr(space(10.)).pt(zpx(2.)).pb(space(12.)).child(list)))
            .into_any_element()
    }

    fn inside_list(&mut self, sp: &PublicRoom, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let note = |words: String| div().text_size(text(12.)).text_color(palette.text_tertiary).child(words);
        let body = match self.inside.get(&sp.id) {
            None | Some(Inside::Loading) => note("looking inside…".into()).into_any_element(),
            Some(Inside::Failed(e)) => note(format!("couldn't look inside — {e}")).into_any_element(),
            Some(Inside::Rooms(rooms)) if rooms.is_empty() => note("nothing public in here yet".into()).into_any_element(),
            Some(Inside::Rooms(rooms)) => {
                let rooms = rooms.clone();
                let mut list = v_flex().gap(space(2.));
                for room in &rooms {
                    let name = display_name(room);
                    let face = avatar(&room.id, &name, room.avatar_url.as_deref(), zpx(28.), cx);
                    let action = self.action(room, false, &sp.via, palette, cx);
                    list = list.child(
                        h_flex()
                            .gap(space(10.))
                            .py(space(6.))
                            .child(div().flex_none().child(face))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        h_flex()
                                            .gap(space(5.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_size(text(13.))
                                            .when(room.is_space, |d| d.child(icon("icons/orbit.svg").size(zpx(12.)).text_color(palette.text_tertiary)))
                                            .child(div().truncate().child(name)),
                                    )
                                    .child(div().text_size(text(11.)).text_color(palette.text_tertiary).child(member_count(room.members))),
                            )
                            .child(action),
                    );
                }
                list.into_any_element()
            }
        };
        v_flex().child(label(palette, "INSIDE")).child(body).into_any_element()
    }
}

/// Wait (five seconds at most) for the store to have what was just joined;
/// sync brings it, and opening a room the list doesn't know yet fails.
/// Spaces are re-read on a poll, so they're asked for as it waits.
async fn wait_for(store: &WeakEntity<Store>, cx: &mut AsyncApp, spaces: bool, there: impl Fn(&Store) -> bool) {
    for _ in 0..(5000 / 250) {
        let Ok(done) = store.update(cx, |s, cx| {
            if spaces && !there(s) {
                s.refresh_spaces_now(cx);
            }
            there(s)
        }) else {
            return;
        };
        if done {
            return;
        }
        cx.background_executor().timer(Duration::from_millis(250)).await;
    }
}

fn display_name(room: &PublicRoom) -> String {
    room.name.clone().or_else(|| room.alias.clone()).unwrap_or_else(|| room.id.clone())
}

fn member_count(n: u64) -> String {
    if n == 1 { "1 member".into() } else { format!("{n} members") }
}

/// A topic cut to about `lines` lines, for the collapsed row.
fn first_lines(topic: &str, lines: usize, chars: usize) -> String {
    let joined = topic.lines().take(lines).collect::<Vec<_>>().join(" ");
    if joined.chars().count() > chars || topic.lines().count() > lines {
        format!("{}…", joined.chars().take(chars).collect::<String>().trim_end())
    } else {
        joined
    }
}

impl Render for Explore {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        if let Some(space) = self.browse.clone() {
            let body = v_flex()
                .child(div().mb(zpx(12.)).text_size(text(12.5)).text_color(palette.text_tertiary).child("everything in the space, including rooms you haven't joined yet."))
                .child(self.inside_list(&space, &palette, cx))
                .into_any_element();
            let this = cx.weak_entity();
            return modal(
                "browse-rooms",
                format!("rooms in {}", space.name.clone().unwrap_or_default()),
                460.,
                &self.focus,
                &palette,
                move |_, cx| {
                    let _ = this.update(cx, |this, cx| this.close(cx));
                },
                body,
            );
        }
        let _ = window;
        let spaces = self.spaces.clone();
        let rows: Vec<AnyElement> = spaces.iter().map(|s| self.space_row(s, &palette, cx)).collect();
        let (search, server) = &self.query;
        let empty = (self.error.is_none() && !self.loading && spaces.is_empty()).then(|| {
            let words = if !search.is_empty() {
                format!("no public spaces match \"{search}\"")
            } else {
                format!("{} doesn't list any public spaces. try another server~", if server.is_empty() { "your server" } else { server })
            };
            div().py(space(28.)).px(space(8.)).text_center().text_size(text(13.)).text_color(palette.text_tertiary).child(words)
        });
        let error = self.error.clone().map(|e| {
            let words = if server.is_empty() { format!("couldn't load the directory — {e}") } else { format!("couldn't read {server}'s directory — {e}") };
            div()
                .mb(zpx(12.))
                .px(space(13.))
                .py(space(10.))
                .rounded(corner(14.))
                .bg(Hsla { a: 0.12, ..palette.danger })
                .text_size(text(12.5))
                .text_color(palette.text_secondary)
                .child(words)
        });
        let more = (!self.loading && self.next_batch.is_some()).then(|| {
            h_flex().justify_center().mt(zpx(12.)).child(Button::new("explore-more").ghost().small().label("show more").on_click(cx.listener(|this, _, _, cx| {
                let since = this.next_batch.clone();
                this.load(since, cx)
            })))
        });

        let body = v_flex()
            .child(
                h_flex()
                    .gap(space(8.))
                    .mb(zpx(16.))
                    .child(
                        v_flex()
                            .flex_grow(1.)
                            .flex_basis(zpx(220.))
                            .min_w_0()
                            .child(label(&palette, "SEARCH"))
                            .child(
                                well(&palette)
                                    .child(icon("icons/search.svg").size(zpx(14.)).text_color(palette.text_tertiary))
                                    .child(div().flex_1().min_w_0().child(Input::new(&self.search).appearance(false))),
                            ),
                    )
                    .child(
                        v_flex()
                            .w(zpx(170.))
                            .flex_none()
                            .child(label(&palette, "SERVER"))
                            .child(well(&palette).child(div().flex_1().min_w_0().child(Input::new(&self.server).appearance(false)))),
                    ),
            )
            .child(
                v_flex()
                    // A floor, so the dialog doesn't shrink and jump under
                    // the pointer as a search narrows.
                    .min_h(zpx(320.))
                    .children(error)
                    .children(empty)
                    .child(v_flex().gap(space(4.)).children(rows))
                    .when(self.loading, |d| d.child(div().py(space(16.)).text_center().text_size(text(12.)).text_color(palette.text_tertiary).child("loading…")))
                    .children(more),
            )
            .into_any_element();

        let this = cx.weak_entity();
        modal(
            "explore",
            "explore spaces".into(),
            560.,
            &self.focus,
            &palette,
            move |_, cx| {
                let _ = this.update(cx, |this, cx| this.close(cx));
            },
            body,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_topics_are_cut_for_the_collapsed_row() {
        assert_eq!(first_lines("short", 2, 160), "short");
        assert_eq!(first_lines("one\ntwo\nthree", 2, 160), "one two…");
        assert_eq!(first_lines(&"x".repeat(200), 2, 10), format!("{}…", "x".repeat(10)));
        assert_eq!(member_count(1), "1 member");
        assert_eq!(member_count(12), "12 members");
    }
}
