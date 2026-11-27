//! Discord's ⌘K / Ctrl+K quick switcher: jump to any DM, channel, or server
//! with the keyboard.

use gpui::{
    AppContext, Context, Entity, EventEmitter, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement, KeyDownEvent, MouseDownEvent,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};
use gpui_component::{
    Icon, IconName, h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use horo_core::dto::{RoomMembership, RoomSummary, SpaceSummary};

use crate::{
    rooms,
    store::Store,
    theme::{Palette, corner, space, text, zpx},
    views::{
        avatar::avatar,
        verification::scrim,
        voice,
    },
};

/// Emitted when the switcher closes (after jumping or on Escape/outside click).
pub struct Closed;

impl EventEmitter<Closed> for QuickSwitcher {}

#[derive(Clone)]
enum Entry {
    Room {
        id: String,
        name: String,
        avatar_url: Option<String>,
        is_direct: bool,
        is_voice: bool,
        space_id: Option<String>,
        space_name: Option<String>,
        unread: u64,
    },
    Space {
        id: String,
        name: String,
        avatar_url: Option<String>,
        unread: u64,
    },
}

pub struct QuickSwitcher {
    store: Entity<Store>,
    focus: FocusHandle,
    query: Entity<InputState>,
    selected: usize,
}

impl QuickSwitcher {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> QuickSwitcher {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("where would you like to go?"));
        cx.subscribe(&query, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                this.selected = 0;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => {
                this.activate(cx);
            }
            _ => {}
        })
        .detach();
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        query.update(cx, |s, cx| s.focus(window, cx));
        QuickSwitcher { store, focus: cx.focus_handle(), query, selected: 0 }
    }

    fn entries(&self, cx: &Context<Self>) -> Vec<Entry> {
        let store = self.store.read(cx);
        let needle = self.query.read(cx).value().trim().to_lowercase();
        let spaces = &store.spaces;
        collect_entries(&store.rooms, spaces, &needle)
    }

    fn activate(&mut self, cx: &mut Context<Self>) {
        let entries = self.entries(cx);
        if let Some(entry) = entries.get(self.selected).or_else(|| entries.first()) {
            self.pick(entry.clone(), cx);
        } else {
            cx.emit(Closed);
        }
    }

    fn pick(&mut self, entry: Entry, cx: &mut Context<Self>) {
        match entry {
            Entry::Room { id, is_direct, space_id, .. } => {
                self.store.update(cx, |s, cx| {
                    if is_direct {
                        s.open_space(None, cx);
                    } else if let Some(sid) = space_id {
                        s.open_space(Some(sid), cx);
                    }
                    s.open_room(id, cx);
                });
            }
            Entry::Space { id, .. } => {
                self.store.update(cx, |s, cx| s.open_space(Some(id), cx));
            }
        }
        cx.emit(Closed);
    }
}

fn collect_entries(all_rooms: &[RoomSummary], spaces: &[SpaceSummary], needle: &str) -> Vec<Entry> {
    let mut scored: Vec<(u8, u64, Entry)> = Vec::new();

    for room in all_rooms {
        if room.is_space || room.is_utility || room.membership != RoomMembership::Joined {
            continue;
        }
        let parent = spaces.iter().find(|s| rooms::is_in_space(room, s));
        let space_id = parent.map(|s| s.id.clone());
        let space_name = parent.map(|s| s.name.clone());
        let recency = room.latest.as_ref().map_or(room.recency, |l| l.timestamp);

        let rank = if needle.is_empty() {
            if room.notification_count > 0 || room.has_unread { 0 } else { 1 }
        } else {
            let name_l = room.name.to_lowercase();
            let space_l = space_name.as_deref().unwrap_or_default().to_lowercase();
            let alias_l = room.canonical_alias.as_deref().unwrap_or_default().to_lowercase();
            if name_l.starts_with(needle) {
                0
            } else if name_l.contains(needle) {
                1
            } else if alias_l.contains(needle) || space_l.contains(needle) {
                2
            } else {
                continue;
            }
        };

        scored.push((
            rank,
            recency,
            Entry::Room {
                id: room.id.clone(),
                name: room.name.clone(),
                avatar_url: room.avatar_url.clone(),
                is_direct: room.is_direct,
                is_voice: room.is_video_room,
                space_id,
                space_name,
                unread: room.notification_count,
            },
        ));
    }

    if !needle.is_empty() {
        for space in spaces {
            let name_l = space.name.to_lowercase();
            let rank = if name_l.starts_with(needle) {
                0
            } else if name_l.contains(needle) {
                2
            } else {
                continue;
            };
            scored.push((
                rank,
                0,
                Entry::Space {
                    id: space.id.clone(),
                    name: space.name.clone(),
                    avatar_url: space.avatar_url.clone(),
                    unread: space.notification_count,
                },
            ));
        }
    }

    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
    scored.into_iter().take(12).map(|(_, _, e)| e).collect()
}

impl Render for QuickSwitcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        let entries = self.entries(cx);
        let count = entries.len();
        if count > 0 && self.selected >= count {
            self.selected = count - 1;
        }

        let rows: Vec<_> = entries
            .into_iter()
            .enumerate()
            .map(|(ix, entry)| {
                let active = ix == self.selected;
                let clicked = entry.clone();
                let (icon_el, title, subtitle, unread) = match &entry {
                    Entry::Room { id, name, avatar_url, is_direct, is_voice, space_name, unread, .. } => {
                        let leading = if *is_direct {
                            avatar(id, name, avatar_url.as_deref(), zpx(24.), cx).into_any_element()
                        } else if *is_voice {
                            div()
                                .size(zpx(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(voice::icon("icons/headphones.svg").size(zpx(15.)).text_color(palette.text_tertiary))
                                .into_any_element()
                        } else {
                            div()
                                .size(zpx(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .font_weight(FontWeight::BOLD)
                                .text_size(text(15.))
                                .text_color(palette.text_tertiary)
                                .child("#")
                                .into_any_element()
                        };
                        let hint = if *is_direct {
                            "direct message".to_owned()
                        } else {
                            space_name.clone().unwrap_or_else(|| "group chat".to_owned())
                        };
                        (leading, name.clone(), hint, *unread)
                    }
                    Entry::Space { id, name, avatar_url, unread } => {
                        let leading = avatar(id, name, avatar_url.as_deref(), zpx(24.), cx).into_any_element();
                        (leading, name.clone(), "server".to_owned(), *unread)
                    }
                };

                h_flex()
                    .id(("qs-row", ix))
                    .items_center()
                    .gap(space(10.))
                    .px(space(12.))
                    .py(space(8.))
                    .rounded(corner(10.))
                    .cursor_pointer()
                    .when(active, |d| d.bg(palette.raised))
                    .hover(|d| d.bg(if active { palette.raised } else { Hsla { a: 0.5, ..palette.raised } }))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick(clicked.clone(), cx)))
                    .child(icon_el)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(text(14.))
                            .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                            .text_color(if active { palette.text } else { palette.text_secondary })
                            .child(title),
                    )
                    .child(div().flex_none().text_xs().text_color(palette.text_tertiary).child(subtitle))
                    .when(unread > 0, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .px(space(6.))
                                .py(zpx(1.))
                                .rounded_full()
                                .bg(palette.accent_secondary)
                                .text_color(palette.text_on_accent)
                                .text_size(text(10.))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .child(if unread > 99 { "99+".to_owned() } else { unread.to_string() }),
                        )
                    })
            })
            .collect();

        scrim(&palette)
            .items_start()
            .pt(zpx(96.))
            .px(space(24.))
            .child(
                v_flex()
                    .id("quick-switcher")
                    .track_focus(&self.focus)
                    .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| match event.keystroke.key.as_str() {
                        "escape" => {
                            cx.stop_propagation();
                            cx.emit(Closed);
                        }
                        "down" => {
                            if count > 0 {
                                this.selected = (this.selected + 1) % count;
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }
                        "up" => {
                            if count > 0 {
                                this.selected = if this.selected == 0 { count - 1 } else { this.selected - 1 };
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }
                        "enter" => {
                            cx.stop_propagation();
                            this.activate(cx);
                        }
                        _ => {}
                    }))
                    .on_mouse_down_out(cx.listener(|_, _: &MouseDownEvent, _, cx| cx.emit(Closed)))
                    .w(zpx(540.))
                    .max_w_full()
                    .rounded(corner(18.))
                    .bg(palette.card)
                    .border_1()
                    .border_color(palette.border)
                    .shadow_lg()
                    .overflow_hidden()
                    .text_color(palette.text)
                    .child(
                        h_flex()
                            .items_center()
                            .gap(space(10.))
                            .px(space(16.))
                            .py(space(12.))
                            .border_b_1()
                            .border_color(palette.border_subtle)
                            .child(Icon::new(IconName::Search).size(zpx(16.)).text_color(palette.text_tertiary))
                            .child(div().flex_1().text_size(text(15.)).child(Input::new(&self.query).appearance(false))),
                    )
                    .child(
                        v_flex()
                            .id("qs-results")
                            .max_h(zpx(360.))
                            .overflow_y_scroll()
                            .p(space(8.))
                            .gap(zpx(2.))
                            .when(rows.is_empty(), |d| {
                                d.child(
                                    div()
                                        .py(space(28.))
                                        .text_center()
                                        .text_size(text(13.))
                                        .text_color(palette.text_tertiary)
                                        .child("no matching conversations or channels~"),
                                )
                            })
                            .children(rows),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .px(space(14.))
                            .py(space(8.))
                            .bg(palette.inset)
                            .border_t_1()
                            .border_color(palette.border_subtle)
                            .font_family(palette.mono_font.clone())
                            .text_size(text(10.5))
                            .text_color(palette.text_tertiary)
                            .child("↑↓ navigate · ↵ jump · esc close")
                            .child(SharedString::from(if cfg!(target_os = "macos") { "⌘K" } else { "Ctrl+K" })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(id: &str, name: &str, is_direct: bool, unread: u64, recency: u64) -> RoomSummary {
        RoomSummary {
            id: id.into(),
            name: name.into(),
            topic: None,
            canonical_alias: None,
            avatar_url: None,
            is_direct,
            dm_user_id: None,
            is_encrypted: false,
            is_space: false,
            is_utility: false,
            is_muted: false,
            notify_mode: None,
            membership: RoomMembership::Joined,
            notification_count: unread,
            highlight_count: 0,
            has_unread: unread > 0,
            is_marked_unread: false,
            member_count: 2,
            latest: None,
            recency,
            parent_spaces: Vec::new(),
            has_active_call: false,
            call_participants: Vec::new(),
            is_video_room: false,
        }
    }

    #[test]
    fn quick_switcher_ranks_unreads_and_prefix_matches_first() {
        let rooms = vec![
            room("!old", "general", false, 0, 10),
            room("!unread", "random", false, 3, 5),
            room("!dm", "gintas", true, 0, 20),
        ];
        let empty = collect_entries(&rooms, &[], "");
        let ids: Vec<&str> = empty
            .iter()
            .map(|e| match e {
                Entry::Room { id, .. } | Entry::Space { id, .. } => id.as_str(),
            })
            .collect();
        assert_eq!(ids, vec!["!unread", "!dm", "!old"]);

        let found = collect_entries(&rooms, &[], "gen");
        assert_eq!(found.len(), 1);
    }
}
