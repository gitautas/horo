//! A room's timeline, read-only for now: messages, day dividers, the read
//! marker, newest at the bottom, older pages fetched as you scroll up.
//!
//! Items come from horo-core's timeline (`timeline::open` for the current
//! ones, then `Event::Timeline` diffs through the [`Store`]); gpui's `list`
//! renders only what's on screen and follows the same diffs as splices, so
//! a new message doesn't re-lay-out the whole history.

use std::{collections::{HashMap, HashSet}, rc::Rc};

use chrono::{DateTime, Local, TimeZone};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, Hsla, EventEmitter, InteractiveElement, KeyDownEvent, ListOffset, StatefulInteractiveElement, FontWeight, IntoElement, ListAlignment, ListState, ParentElement, Render, SharedString, Styled,
    WeakEntity, Window, div, img, list, AnimationExt, prelude::FluentBuilder, StyledImage,
};
use gpui_component::{
    Icon, IconName, h_flex,
    input::{Input, InputEvent, InputState, Position, Textarea, TextareaState},
    v_flex,
};
use horo_core::{
    dto::{ContentDto, Diff, EventItemDto, MediaInfoDto, ReactionDto, TimelineItemDto, TimelineRowKind, TimelineUpdate},
    matrix::timeline,
};
use matrix_sdk::ruma::OwnedRoomId;

use crate::theme::zpx;
use crate::{
    backend::Backend,
    media::Media,
    packs,
    rich::{self, Block},
    store::{Lightbox, LightboxItem, Store, apply_diff},
    theme::{Palette, corner, space, text},
    views::{
        avatar::{accent_for, avatar},
        menu,
        content::{self, Open},
        emoji_picker::{EmojiPicker, PickerEvent},
        voice::icon,
    },
};

/// Messages from one sender this close together share a header (Discord's 7m window).
const GROUP_WINDOW_MS: u64 = 7 * 60 * 1000;
/// Fetch older history when this close to the top.
const PAGINATE_WITHIN: usize = 8;
/// Fill pages fetched on open until this many messages show…
const FILL_UNTIL: usize = 30;
/// …but no more than this many, so a room of hidden events can't spin.
const MAX_FILL_PAGES: u32 = 8;

pub struct Timeline {
    room_id: OwnedRoomId,
    /// The thread this is, or `None` for the room's main timeline.
    thread_root: Option<String>,
    /// What the core's updates for this timeline are addressed to.
    key: String,
    store: Entity<Store>,
    /// Parsed `formatted_body`s, by their HTML.
    rich: HashMap<String, Rc<Vec<Block>>>,
    items: Vec<TimelineItemDto>,
    list: ListState,
    /// The zoom the rows were last measured at.
    zoom_epoch: u64,
    /// The initial items are in; until then diffs wait in `early`.
    loaded: bool,
    early: Vec<Vec<Diff<TimelineItemDto>>>,
    paginating: bool,
    at_start: bool,
    /// Pages fetched to fill the screen since the room opened.
    fill_pages: u32,
    error: Option<SharedString>,
    /// A read marker is due to go out shortly.
    marking_read: bool,
    /// The message whose delete button is asking "sure?".
    confirm_delete: Option<String>,
    /// The last action on a message that failed.
    notice: Option<SharedString>,
    /// The emoji picker, open for reacting to a message: which, where.
    picker: Option<(String, gpui::Point<gpui::Pixels>, Entity<EmojiPicker>)>,
    /// The video or sound playing in its message (one at a time).
    playing: Option<InlineMedia>,
    /// A message's right-click menu: which, and where.
    menu: Option<(String, gpui::Point<gpui::Pixels>)>,
    /// Messages whose spoilers the reader has clicked to reveal.
    revealed_spoilers: HashSet<String>,
    /// True when the viewport is scrolled up away from the newest messages.
    scrolled_up: bool,
    /// In-room message search (⌘F / Ctrl+F).
    search: Option<Entity<InputState>>,
    search_matches: Vec<usize>,
    search_cursor: usize,
    /// Pinned messages popover (`m.room.pinned_events`).
    pins_open: bool,
    pins: Option<Vec<TimelineItemDto>>,
    pinned_ids: HashSet<String>,
    /// Cached OpenGraph link previews keyed by URL.
    link_previews: HashMap<String, LinkPreviewState>,
    /// Message currently being edited inline in the timeline.
    editing: Option<InlineEdit>,
}

struct InlineEdit {
    event_id: String,
    input: Entity<TextareaState>,
    error: Option<SharedString>,
    saving: bool,
}

#[derive(Clone)]
enum LinkPreviewState {
    Loading,
    Ready(horo_core::link_preview::LinkPreview),
    None,
}

/// A video or a sound played inside its message: fetched to a file, then
/// played.
struct InlineMedia {
    event_id: String,
    /// The fetched file, for "open in player".
    path: Option<std::path::PathBuf>,
    state: PlayState,
    /// Where the progress bar was last drawn, for a press on it to seek.
    bar: Rc<std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
}

enum PlayState {
    Loading,
    /// AVFoundation's player: video, and audio other than Ogg/Opus.
    #[cfg(target_os = "macos")]
    Playing(crate::video::VideoPlayer),
    /// horo-rtc's, for Ogg/Opus voice messages, on every platform.
    Voice(horo_rtc::voice::VoicePlayer),
    Failed(SharedString),
}

impl PlayState {
    /// Where it's at and how long it is, once something's playing.
    fn position(&self) -> Option<(f64, f64)> {
        match self {
            #[cfg(target_os = "macos")]
            PlayState::Playing(player) => Some(player.position()),
            PlayState::Voice(player) => Some(player.position()),
            _ => None,
        }
    }

    fn is_playing(&self) -> bool {
        match self {
            #[cfg(target_os = "macos")]
            PlayState::Playing(player) => player.is_playing(),
            PlayState::Voice(player) => player.is_playing(),
            _ => false,
        }
    }

    fn toggle(&self) {
        match self {
            #[cfg(target_os = "macos")]
            PlayState::Playing(player) => player.toggle(),
            PlayState::Voice(player) => player.toggle(),
            _ => {}
        }
    }

    fn seek(&self, fraction: f64) {
        match self {
            #[cfg(target_os = "macos")]
            PlayState::Playing(player) => player.seek(fraction),
            PlayState::Voice(player) => player.seek(fraction),
            _ => {}
        }
    }
}

/// What the timeline asks of the composer under it.
#[derive(Debug, Clone)]
pub enum TimelineAction {
    Reply { event_id: String, who: String, preview: String },
    FocusComposer,
}

impl EventEmitter<TimelineAction> for Timeline {}

impl Timeline {
    pub fn new(room_id: OwnedRoomId, thread_root: Option<String>, store: &Entity<Store>, cx: &mut Context<Self>) -> Timeline {
        let key = timeline::timeline_key(&room_id, thread_root.as_deref());
        let list = ListState::new(0, ListAlignment::Bottom, zpx(400.));
        let this = cx.weak_entity();
        list.set_scroll_handler(move |event, _, cx| {
            let visible_end = event.visible_range.end;
            let is_scrolled = event.is_scrolled;
            let paginate = event.visible_range.start < PAGINATE_WITHIN;
            let _ = this.update(cx, |timeline, cx| {
                // Count real visible messages below the viewport (ignoring hidden
                // call/state bookkeeping items at the tail).
                let tail = timeline.items.get(visible_end.min(timeline.items.len())..).unwrap_or(&[]);
                let below = tail.iter().filter(|i| seen(i) == Seen::Shown).count();
                let next = if !is_scrolled || below <= 2 {
                    false
                } else if below >= 25 {
                    true
                } else {
                    timeline.scrolled_up
                };
                if timeline.scrolled_up != next {
                    timeline.scrolled_up = next;
                    cx.notify();
                }
                if paginate {
                    timeline.paginate(cx);
                }
            });
        });
        // Presence dots change with the store.
        cx.observe(store, |_, _, cx| cx.notify()).detach();
        cx.subscribe(store, |this, _, update: &TimelineUpdate, cx| {
            if update.room_id == this.key {
                this.on_update(update.diffs.clone(), cx);
            }
        })
        .detach();

        let timeline = Timeline {
            room_id: room_id.clone(),
            thread_root: thread_root.clone(),
            key,
            store: store.clone(),
            rich: HashMap::new(),
            items: Vec::new(),
            list,
            loaded: false,
            early: Vec::new(),
            paginating: false,
            at_start: false,
            fill_pages: 0,
            error: None,
            marking_read: false,
            confirm_delete: None,
            notice: None,
            picker: None,
            playing: None,
            menu: None,
            revealed_spoilers: HashSet::new(),
            zoom_epoch: 0,
            scrolled_up: false,
            search: None,
            search_matches: Vec::new(),
            search_cursor: 0,
            pins_open: false,
            pins: None,
            pinned_ids: HashSet::new(),
            link_previews: HashMap::new(),
            editing: None,
        };

        let backend = Backend::global(cx);
        let task = backend.run({
            let (host, state) = (backend.host.clone(), backend.state.clone());
            async move { timeline::open(&host, &state.core().await?, &room_id, thread_root).await }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |timeline, cx| match outcome {
                Ok(Ok(items)) => timeline.loaded(items, cx),
                Ok(Err(e)) => timeline.failed(e.to_string(), cx),
                Err(e) => timeline.failed(e.to_string(), cx),
            })
        })
        .detach();
        timeline
    }

    fn loaded(&mut self, items: Vec<TimelineItemDto>, cx: &mut Context<Self>) {
        self.list.reset(items.len());
        self.items = items;
        self.loaded = true;
        for diffs in std::mem::take(&mut self.early) {
            self.on_update(diffs, cx);
        }
        // A fresh room often opens with a handful of events: fill the screen.
        if self.shown_messages() < FILL_UNTIL {
            self.fill_pages = 1;
            self.paginate(cx);
        }
        self.mark_read(cx);
        self.report_people(cx);
        cx.notify();
    }

    /// Whether the divider or marker at `ix` has nothing drawn under it.
    fn empty_divider(&self, ix: usize) -> bool {
        empty_divider(self.items.get(ix..).unwrap_or_default().iter().map(seen))
    }

    /// Messages actually drawn: not call bookkeeping, dividers or markers.
    fn shown_messages(&self) -> usize {
        self.items.iter().filter_map(|i| i.event.as_deref()).filter(|e| worth_showing(&e.content)).count()
    }

    /// The room is on screen, so what's in it has been seen: move the read
    /// marker (which clears its unread badge), at most once a second.
    fn mark_read(&mut self, cx: &mut Context<Self>) {
        if self.marking_read {
            return;
        }
        self.marking_read = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
            let Ok(task) = this.update(cx, |timeline, cx| {
                timeline.marking_read = false;
                let backend = Backend::global(cx);
                let (state, room_id) = (backend.state.clone(), timeline.room_id.clone());
                backend.run(async move { timeline::mark_as_read(&*state.core().await?, &room_id).await })
            }) else {
                return;
            };
            if let Ok(Err(e)) = task.await {
                tracing::debug!("marking read: {e}");
            }
        })
        .detach();
    }

    fn failed(&mut self, error: String, cx: &mut Context<Self>) {
        tracing::warn!("timeline {}: {error}", self.room_id);
        self.error = Some(error.into());
        cx.notify();
    }

    fn on_update(&mut self, diffs: Vec<Diff<TimelineItemDto>>, cx: &mut Context<Self>) {
        if !self.loaded {
            self.early.push(diffs);
            return;
        }
        for diff in diffs {
            if let Some((range, count)) = apply_diff(&mut self.items, diff) {
                // The item after a change can gain or lose its header (it may
                // now continue, or no longer continue, the group above).
                let end = (range.start + count + 1).min(self.items.len());
                let start = range.start;
                self.list.splice(range, count);
                if range_after(end, self.items.len()) {
                    self.list.remeasure_items(end - 1..end);
                }
                // A message landing in (or leaving) a day can show or hide
                // its divider, and the "new" marker above it.
                let before = &self.items[..start.min(self.items.len())];
                let day = before.iter().rposition(|i| i.kind == TimelineRowKind::DateDivider);
                let marker = self.items.iter().position(|i| i.kind == TimelineRowKind::ReadMarker);
                for ix in [day, marker].into_iter().flatten() {
                    self.list.remeasure_items(ix..ix + 1);
                }
            }
        }
        self.mark_read(cx);
        self.report_people(cx);
        cx.notify();
    }

    fn paginate(&mut self, cx: &mut Context<Self>) {
        if self.paginating || self.at_start || !self.loaded {
            return;
        }
        self.paginating = true;
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, room_id, thread) = (backend.state.clone(), self.room_id.clone(), self.thread_root.clone());
            async move { timeline::paginate_back(&*state.core().await?, &room_id, thread.as_deref()).await }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |timeline, cx| {
                timeline.paginating = false;
                match outcome {
                    Ok(Ok(at_start)) => {
                        timeline.at_start = at_start;
                        // Keep filling while what's shown can't scroll (a room
                        // of call bookkeeping and joins shows almost nothing),
                        // so the reader never needs to scroll an unscrollable
                        // list to ask for more. Capped, as the WebView build's.
                        if !at_start && timeline.fill_pages < MAX_FILL_PAGES && timeline.shown_messages() < FILL_UNTIL {
                            timeline.fill_pages += 1;
                            timeline.paginate(cx);
                        }
                    }
                    Ok(Err(e)) => tracing::warn!("paginating {}: {e}", timeline.room_id),
                    Err(e) => tracing::warn!("paginating {}: {e}", timeline.room_id),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Close the core's timeline: it streams until closed.
    pub fn close(&self, cx: &App) {
        let backend = Backend::global(cx);
        let (state, room_id, thread) = (backend.state.clone(), self.room_id.clone(), self.thread_root.clone());
        let task = backend.run(async move {
            if let Ok(core) = state.core().await {
                let _ = timeline::close(&core, &room_id, thread.as_deref()).await;
            }
        });
        drop(task);
    }

    /// Open (or refocus) the ⌘F in-room message search bar.
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = &self.search {
            input.update(cx, |s, cx| s.focus(window, cx));
            cx.notify();
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("search messages in this room…"));
        cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => this.update_search(cx),
            InputEvent::PressEnter { .. } => this.step_search(1, cx),
            _ => {}
        })
        .detach();
        input.update(cx, |s, cx| s.focus(window, cx));
        self.search = Some(input);
        self.search_matches.clear();
        self.search_cursor = 0;
        cx.notify();
    }

    fn close_search(&mut self, cx: &mut Context<Self>) {
        self.search = None;
        self.search_matches.clear();
        self.search_cursor = 0;
        cx.notify();
    }

    fn update_search(&mut self, cx: &mut Context<Self>) {
        let Some(input) = &self.search else { return };
        let needle = input.read(cx).value().trim().to_lowercase();
        self.search_matches.clear();
        self.search_cursor = 0;
        if !needle.is_empty() {
            for (ix, item) in self.items.iter().enumerate().rev() {
                if item.kind != TimelineRowKind::Event {
                    continue;
                }
                let Some(event) = item.event.as_deref() else { continue };
                if !worth_showing(&event.content) || is_system(&event.content) {
                    continue;
                }
                let (text_body, _) = body_text(&event.content);
                let sender = event.sender_name.as_deref().unwrap_or(event.sender.as_str());
                if text_body.to_lowercase().contains(&needle) || sender.to_lowercase().contains(&needle) {
                    self.search_matches.push(ix);
                }
            }
        }
        if let Some(&ix) = self.search_matches.first() {
            self.list.scroll_to_reveal_item(ix);
        }
        cx.notify();
    }

    fn step_search(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.search_matches.len();
        if len == 0 {
            return;
        }
        self.search_cursor = (self.search_cursor as isize + delta).rem_euclid(len as isize) as usize;
        if let Some(&ix) = self.search_matches.get(self.search_cursor) {
            self.list.scroll_to_reveal_item(ix);
        }
        cx.notify();
    }

    fn jump_to_present(&mut self, cx: &mut Context<Self>) {
        self.list.scroll_to(ListOffset { item_ix: self.items.len(), offset_in_item: zpx(0.) });
        self.scrolled_up = false;
        cx.notify();
    }

    /// Toggle the Discord-style pinned messages popover at the top-right.
    pub fn toggle_pins(&mut self, cx: &mut Context<Self>) {
        self.pins_open = !self.pins_open;
        if self.pins_open {
            self.load_pins(cx);
        }
        cx.notify();
    }

    fn load_pins(&mut self, cx: &mut Context<Self>) {
        self.pins = None;
        let room_id = self.room_id.clone();
        let backend = Backend::global(cx);
        let state = backend.state.clone();
        let task = backend.run(async move { timeline::pinned_events(&*state.core().await?, &room_id).await });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(Ok(items)) => {
                        this.pinned_ids = items
                            .iter()
                            .filter_map(|i| i.event.as_deref().and_then(|e| e.event_id.clone()))
                            .collect();
                        this.pins = Some(items);
                    }
                    Ok(Err(e)) => {
                        tracing::warn!("couldn't load pinned messages: {e}");
                        this.pins = Some(Vec::new());
                    }
                    Err(e) => {
                        tracing::warn!("couldn't load pinned messages: {e}");
                        this.pins = Some(Vec::new());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_pin(&mut self, event_id: String, pin: bool, cx: &mut Context<Self>) {
        let room_id = self.room_id.clone();
        let backend = Backend::global(cx);
        let state = backend.state.clone();
        let eid = event_id.clone();
        let task = backend.run(async move { timeline::pin_event(&*state.core().await?, &room_id, &eid, pin).await });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| match outcome {
                Ok(Ok(_)) => {
                    if pin {
                        this.pinned_ids.insert(event_id);
                    } else {
                        this.pinned_ids.remove(&event_id);
                    }
                    if this.pins_open {
                        this.load_pins(cx);
                    }
                    cx.notify();
                }
                Ok(Err(e)) => {
                    this.notice = Some(format!("couldn't update pins: {e}").into());
                    cx.notify();
                }
                Err(e) => {
                    this.notice = Some(format!("couldn't update pins: {e}").into());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn pins_popover(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.pins_open {
            return None;
        }
        let body: AnyElement = match &self.pins {
            None => div().py(space(28.)).text_center().text_xs().text_color(palette.text_tertiary).child("loading pinned messages…").into_any_element(),
            Some(items) => {
                let events: Vec<&EventItemDto> = items.iter().filter_map(|i| i.event.as_deref()).collect();
                if events.is_empty() {
                    div()
                        .py(space(28.))
                        .px(space(16.))
                        .text_center()
                        .text_xs()
                        .text_color(palette.text_tertiary)
                        .child("no pinned messages yet — right-click a message to pin it~")
                        .into_any_element()
                } else {
                    v_flex()
                        .gap(space(8.))
                        .p(space(10.))
                        .children(events.into_iter().enumerate().map(|(ix, ev)| {
                            let who = ev.sender_name.clone().unwrap_or_else(|| localpart(&ev.sender).to_owned());
                            let (snippet, _) = body_text(&ev.content);
                            let eid = ev.event_id.clone();
                            let jump_id = ev.event_id.clone();
                            v_flex()
                                .id(("pin-card", ix))
                                .gap(space(4.))
                                .p(space(10.))
                                .rounded(corner(12.))
                                .bg(palette.app)
                                .border_1()
                                .border_color(palette.border_subtle)
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .items_center()
                                        .gap(space(6.))
                                        .child(
                                            h_flex()
                                                .gap(space(6.))
                                                .min_w_0()
                                                .child(avatar(&ev.sender, &who, ev.sender_avatar.as_deref(), zpx(20.), cx))
                                                .child(div().truncate().text_xs().font_weight(FontWeight::BOLD).text_color(accent_for(&ev.sender)).child(who))
                                                .child(div().flex_none().font_family(palette.mono_font.clone()).text_size(text(10.)).text_color(palette.text_tertiary).child(time_label(ev.timestamp))),
                                        )
                                        .child(
                                            h_flex()
                                                .flex_none()
                                                .gap(space(4.))
                                                .when_some(jump_id, |d, target_id| {
                                                    d.child(
                                                        div()
                                                            .id(("pin-jump", ix))
                                                            .px(space(6.))
                                                            .py(zpx(2.))
                                                            .rounded(corner(6.))
                                                            .cursor_pointer()
                                                            .text_size(text(10.5))
                                                            .text_color(palette.accent)
                                                            .hover(|d| d.bg(palette.raised))
                                                            .child("jump")
                                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                                if let Some(pos) = this.items.iter().position(|i| i.event.as_deref().and_then(|e| e.event_id.as_deref()) == Some(target_id.as_str())) {
                                                                    this.list.scroll_to_reveal_item(pos);
                                                                    let below = this.items[pos + 1..].iter().filter(|i| seen(i) == Seen::Shown).count();
                                                                    if below >= 6 {
                                                                        this.scrolled_up = true;
                                                                    }
                                                                    this.pins_open = false;
                                                                    cx.notify();
                                                                }
                                                            })),
                                                    )
                                                })
                                                .when_some(eid, |d, unpin_id| {
                                                    d.child(
                                                        div()
                                                            .id(("pin-unpin", ix))
                                                            .px(space(6.))
                                                            .py(zpx(2.))
                                                            .rounded(corner(6.))
                                                            .cursor_pointer()
                                                            .text_size(text(10.5))
                                                            .text_color(palette.text_tertiary)
                                                            .hover(|d| d.bg(palette.raised).text_color(palette.danger))
                                                            .child("✕")
                                                            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("unpin").build(window, cx))
                                                            .on_click(cx.listener(move |this, _, _, cx| this.set_pin(unpin_id.clone(), false, cx))),
                                                    )
                                                }),
                                        ),
                                )
                                .child(div().text_size(text(12.5)).text_color(palette.text_secondary).child(snippet))
                        }))
                        .into_any_element()
                }
            }
        };

        Some(
            v_flex()
                .id("pins-popover")
                .absolute()
                .top(space(10.))
                .right(space(22.))
                .w(zpx(380.))
                .max_w(gpui::relative(0.85))
                .max_h(zpx(440.))
                .rounded(corner(16.))
                .bg(palette.card)
                .border_1()
                .border_color(palette.border)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.pins_open = false;
                    cx.notify();
                }))
                .child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .px(space(14.))
                        .py(space(10.))
                        .border_b_1()
                        .border_color(palette.border_subtle)
                        .child(div().text_sm().font_weight(FontWeight::BOLD).text_color(palette.text).child("pinned messages"))
                        .child(
                            div()
                                .id("pins-close")
                                .px(space(6.))
                                .py(zpx(2.))
                                .rounded(corner(6.))
                                .cursor_pointer()
                                .text_xs()
                                .text_color(palette.text_tertiary)
                                .hover(|d| d.bg(palette.raised).text_color(palette.text))
                                .child("✕")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.pins_open = false;
                                    cx.notify();
                                })),
                        ),
                )
                .child(div().id("pins-scroll").flex_1().min_h_0().overflow_y_scroll().child(body))
                .into_any_element(),
        )
    }

    fn search_bar(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let input = self.search.as_ref()?;
        let has_query = !input.read(cx).value().trim().is_empty();
        let total = self.search_matches.len();
        let count_label = if !has_query {
            String::new()
        } else if total == 0 {
            "no matches".to_owned()
        } else {
            format!("{} / {}", self.search_cursor + 1, total)
        };
        Some(
            h_flex()
                .flex_none()
                .items_center()
                .gap(space(8.))
                .mx(space(22.))
                .mt(space(10.))
                .px(space(12.))
                .py(space(6.))
                .rounded(corner(12.))
                .bg(palette.card)
                .border_1()
                .border_color(palette.border)
                .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.close_search(cx);
                    }
                }))
                .child(Icon::new(IconName::Search).size(zpx(14.)).text_color(palette.text_tertiary))
                .child(div().flex_1().min_w_0().text_size(text(13.)).child(Input::new(input).appearance(false)))
                .when(!count_label.is_empty(), |d| {
                    d.child(div().flex_none().font_family(palette.mono_font.clone()).text_size(text(11.)).text_color(palette.text_tertiary).child(count_label))
                })
                .child(
                    div()
                        .id("search-prev")
                        .px(space(6.))
                        .py(zpx(2.))
                        .rounded(corner(6.))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(if total > 0 { palette.text_secondary } else { palette.text_tertiary })
                        .hover(|d| d.bg(palette.raised))
                        .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("older match").build(window, cx))
                        .child("↑")
                        .on_click(cx.listener(|this, _, _, cx| this.step_search(1, cx))),
                )
                .child(
                    div()
                        .id("search-next")
                        .px(space(6.))
                        .py(zpx(2.))
                        .rounded(corner(6.))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(if total > 0 { palette.text_secondary } else { palette.text_tertiary })
                        .hover(|d| d.bg(palette.raised))
                        .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("newer match").build(window, cx))
                        .child("↓")
                        .on_click(cx.listener(|this, _, _, cx| this.step_search(-1, cx))),
                )
                .child(
                    div()
                        .id("search-close")
                        .px(space(6.))
                        .py(zpx(2.))
                        .rounded(corner(6.))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(palette.text_tertiary)
                        .hover(|d| d.bg(palette.raised).text_color(palette.text))
                        .child("✕")
                        .on_click(cx.listener(|this, _, _, cx| this.close_search(cx))),
                )
                .into_any_element(),
        )
    }
}

#[cfg(target_os = "macos")]
fn open_player(path: &std::path::Path, video: bool) -> PlayState {
    match crate::video::VideoPlayer::open(path, video) {
        Some(player) => PlayState::Playing(player),
        None => PlayState::Failed("couldn't play it here".into()),
    }
}

#[cfg(not(target_os = "macos"))]
fn open_player(_: &std::path::Path, _: bool) -> PlayState {
    PlayState::Failed("can't play it here on this system".into())
}

/// "1:05", or "1:02:05" past an hour.
fn clock(seconds: f64) -> String {
    let s = seconds.max(0.) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// How many bars a waveform is drawn with, at most.
const WAVE_BARS: usize = 48;

/// A voice message's waveform (MSC3246: values 0 to 1024) as `WAVE_BARS`
/// rounded bars, each the loudest of the samples it covers; played up to
/// `played` (0 to 1) in the accent.
fn wave_heights(samples: &[u16], bars: usize) -> Vec<f32> {
    if samples.is_empty() || bars == 0 {
        return Vec::new();
    }
    let bars = bars.min(samples.len());
    (0..bars)
        .map(|i| {
            let (start, end) = (i * samples.len() / bars, ((i + 1) * samples.len() / bars).max(i * samples.len() / bars + 1));
            let loudest = samples[start..end].iter().copied().max().unwrap_or(0);
            (f32::from(loudest.min(1024)) / 1024.).max(0.08)
        })
        .collect()
}

fn wave(samples: &[u16], played: f32, palette: &Palette) -> AnyElement {
    let heights = wave_heights(samples, WAVE_BARS);
    let count = heights.len().max(1) as f32;
    let mut row = h_flex().w_full().h(zpx(22.)).items_center().justify_between();
    for (i, height) in heights.into_iter().enumerate() {
        let lit = (i as f32 + 0.5) / count <= played;
        row = row.child(div().w(zpx(2.5)).h(zpx(22. * height)).rounded_full().bg(if lit { palette.accent } else { palette.border_strong }));
    }
    row.into_any_element()
}

fn range_after(end: usize, len: usize) -> bool {
    end > 0 && end <= len
}

/// A row as far as whether a divider over it is worth drawing.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Seen {
    /// A day's divider.
    Day,
    /// The "new" marker.
    Marker,
    /// Something drawn: a message, a system line, the timeline's start.
    Shown,
    /// Not drawn: call bookkeeping.
    Hidden,
}

fn seen(item: &TimelineItemDto) -> Seen {
    match item.kind {
        TimelineRowKind::DateDivider => Seen::Day,
        TimelineRowKind::ReadMarker => Seen::Marker,
        TimelineRowKind::Event => match item.event.as_deref() {
            Some(event) if worth_showing(&event.content) => Seen::Shown,
            _ => Seen::Hidden,
        },
        TimelineRowKind::TimelineStart => Seen::Shown,
    }
}

/// Whether the divider at `ix` stands over nothing: a day whose only
/// events are hidden ones, or a "new" with nothing drawn after it. Drawn,
/// they'd stack up as a ladder of empty days.
/// `rows` starts at the divider itself.
fn empty_divider(mut rows: impl Iterator<Item = Seen>) -> bool {
    match rows.next() {
        // Up to the next day's divider.
        Some(Seen::Day) => !rows.take_while(|r| *r != Seen::Day).any(|r| r == Seen::Shown),
        Some(Seen::Marker) => !rows.any(|r| r == Seen::Shown),
        _ => false,
    }
}

/// How many of your recent reactions the hover bar offers.
const HOVER_REACTIONS: usize = 3;

impl Timeline {
    /// The bar that shows over a message on hover: quick reactions, then
    /// icon buttons for react, reply, thread, edit, and delete (matching
    /// MessageRow.tsx).
    fn actions(&self, event: &EventItemDto, name: &str, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = event.event_id.clone()?;
        let confirming = self.confirm_delete.as_deref() == Some(id.as_str());
        let picking = self.picker.as_ref().is_some_and(|(eid, ..)| eid == &id);
        let text_button = |key: &str, label: &str, colour| {
            div()
                .id(SharedString::from(format!("{key}/{id}")))
                .px_2()
                .py(zpx(2.))
                .rounded(palette.radius_sm)
                .text_xs()
                .text_color(colour)
                .cursor_pointer()
                .hover(|b| b.bg(palette.inset))
                .child(label.to_owned())
        };
        let icon_button = |key: &str, icon_path: &'static str, tip: &'static str, colour: Hsla| {
            div()
                .id(SharedString::from(format!("{key}/{id}")))
                .size(zpx(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(corner(9.))
                .cursor_pointer()
                .hover(|b| b.bg(Hsla { a: 0.08, ..gpui::white() }))
                .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip).build(window, cx))
                .child(icon(icon_path).size(zpx(15.)).text_color(colour))
        };
        let mut bar = h_flex()
            .absolute()
            .top(zpx(-10.))
            .right_4()
            .p(zpx(3.))
            .gap(zpx(2.))
            .rounded(corner(12.))
            .bg(palette.raised)
            .border_1()
            .border_color(palette.border)
            .shadow_sm();
        bar = if confirming || picking { bar.visible() } else { bar.invisible().group_hover("message", |s| s.visible()) };
        let reactable = !matches!(event.content, ContentDto::Redacted);
        let any = reactable || event.can_reply || (event.is_editable && editable_body(&event.content).is_some()) || event.is_own;
        if !any {
            return None;
        }
        // Your recent reactions, one click each, then a divider and the action icons.
        if reactable && !confirming {
            let (recent, known) = {
                let store = self.store.read(cx);
                (store.prefs.recent_reactions.clone(), packs::image_lookup(&store.packs))
            };
            let has_recent = !recent.is_empty();
            // Only the latest few, to keep the bar short; the picker has
            // the rest.
            for (i, key) in recent.into_iter().take(HOVER_REACTIONS).enumerate() {
                // A custom emote is remembered by its address (or a
                // shortcode another client used): drawn as its picture.
                let picture = packs::reaction_image(&key, &known);
                let name = picture.as_ref().map(|(_, name)| name.clone()).unwrap_or_else(|| key.clone());
                let face = match picture.and_then(|(url, _)| Media::image(&url, Some((52, 52)), cx)) {
                    Some(image) => img(image).size(zpx(18.)).object_fit(gpui::ObjectFit::Contain).into_any_element(),
                    None => div().child(key.clone()).into_any_element(),
                };
                let (event_id, tip) = (id.clone(), format!("react with {name}"));
                bar = bar.child(
                    div()
                        .id(SharedString::from(format!("quick-{i}/{id}")))
                        .size(zpx(26.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(corner(9.))
                        .cursor_pointer()
                        .text_size(text(15.))
                        .hover(|b| b.bg(Hsla { a: 0.08, ..gpui::white() }))
                        .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                        .child(face)
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_reaction(event_id.clone(), key.clone(), cx))),
                );
            }
            if has_recent {
                bar = bar.child(div().w(zpx(1.)).h(zpx(16.)).mx(zpx(2.)).bg(palette.border_subtle));
            }
            let event_id = id.clone();
            bar = bar.child(
                icon_button("more-reactions", "icons/smile.svg", "more reactions", palette.text_secondary)
                    .on_click(cx.listener(move |this, click: &gpui::ClickEvent, window, cx| this.open_picker(event_id.clone(), click.position(), window, cx))),
            );
        }

        if event.can_reply && !confirming {
            let (event_id, who) = (id.clone(), name.to_owned());
            let preview = reply_preview(&event.content);
            bar = bar.child(icon_button("reply", "icons/reply.svg", "reply", palette.text_secondary).on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(TimelineAction::Reply { event_id: event_id.clone(), who: who.clone(), preview: preview.clone() })
            })));
        }
        // Inside a thread every reply is already in it.
        if self.thread_root.is_none() && reactable && !confirming {
            let root = event.thread_root.clone().unwrap_or_else(|| id.clone());
            bar = bar.child(icon_button("thread", "icons/messages-square.svg", "reply in thread", palette.text_secondary).on_click(cx.listener(move |this, _, _, cx| {
                this.store.update(cx, |s, cx| s.open_thread(Some(root.clone()), cx))
            })));
        }
        if event.is_editable && !confirming
            && let Some(body) = editable_body(&event.content)
        {
            let event_id = id.clone();
            bar = bar.child(icon_button("edit", "icons/pencil.svg", "edit", palette.text_secondary).on_click(cx.listener(move |this, _, window, cx| {
                this.start_edit(event_id.clone(), body.clone(), window, cx);
            })));
        }
        if event.is_own && !matches!(event.content, ContentDto::Redacted) {
            if confirming {
                let event_id = id.clone();
                bar = bar
                    .child(div().px_2().text_xs().text_color(palette.text_secondary).child("delete it?"))
                    .child(text_button("delete-yes", "delete", palette.danger).on_click(cx.listener(move |this, _, _, cx| this.redact(event_id.clone(), cx))))
                    .child(text_button("delete-no", "keep", palette.text_secondary).on_click(cx.listener(|this, _, _, cx| {
                        this.confirm_delete = None;
                        cx.notify();
                    })));
            } else {
                let event_id = id.clone();
                bar = bar.child(icon_button("delete", "icons/trash.svg", "delete", palette.danger).on_click(cx.listener(move |this, _, _, cx| {
                    this.confirm_delete = Some(event_id.clone());
                    cx.notify();
                })));
            }
        }
        Some(bar.into_any_element())
    }

    fn open_profile(&mut self, user_id: String, at: gpui::Point<gpui::Pixels>, avatar: Option<String>, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.open_profile(user_id, at, avatar, cx));
    }

    /// Tell the store who's in view, so their presence is polled. The
    /// newest few hundred items' senders are plenty.
    fn report_people(&self, cx: &mut Context<Self>) {
        let people = self.items.iter().rev().take(300).filter_map(|i| i.event.as_ref().map(|e| e.sender.clone())).collect();
        self.store.update(cx, |s, cx| s.set_people_on_screen(people, cx));
    }

    fn open_picker(&mut self, event_id: String, at: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        // No stickers: a reaction is a string, and a sticker can't be one.
        let picker = cx.new(|cx| EmojiPicker::new(store, false, window, cx));
        let target = event_id.clone();
        cx.subscribe(&picker, move |this, _, event: &PickerEvent, cx| {
            this.picker = None;
            if let PickerEvent::Pick(picked) = event {
                this.toggle_reaction(target.clone(), packs::reaction_key(picked), cx);
            }
            cx.notify();
        })
        .detach();
        self.picker = Some((event_id, at, picker));
        cx.notify();
    }

    fn toggle_reaction(&mut self, event_id: String, key: String, cx: &mut Context<Self>) {
        // Remembered even if sending fails: picking it is what says what
        // you reach for.
        self.store.update(cx, |s, cx| s.update_prefs(|p| p.note_reaction(&key), cx));
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, room_id, thread) = (backend.state.clone(), self.room_id.clone(), self.thread_root.clone());
            async move { timeline::toggle_reaction(&*state.core().await?, &room_id, thread.as_deref(), &event_id, &key).await }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(_)) => return,
                Ok(Err(e)) => e.to_string(),
                Err(e) => e.to_string(),
            };
            let _ = this.update(cx, |t, cx| t.failed_action(format!("couldn't react: {error}"), cx));
        })
        .detach();
    }

    fn redact(&mut self, event_id: String, cx: &mut Context<Self>) {
        self.confirm_delete = None;
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, room_id, thread) = (backend.state.clone(), self.room_id.clone(), self.thread_root.clone());
            async move { timeline::redact(&*state.core().await?, &room_id, thread.as_deref(), &event_id, None).await }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => return,
                Ok(Err(e)) => e.to_string(),
                Err(e) => e.to_string(),
            };
            let _ = this.update(cx, |t, cx| t.failed_action(format!("couldn't delete: {error}"), cx));
        })
        .detach();
        cx.notify();
    }

    /// An action failed: say so where the user is looking.
    fn failed_action(&mut self, error: String, cx: &mut Context<Self>) {
        tracing::warn!("{error}");
        self.notice = Some(error.into());
        cx.notify();
    }

    /// Open an inline editor right where `event_id` sits in the timeline.
    pub fn start_edit(&mut self, event_id: String, body: String, window: &mut Window, cx: &mut Context<Self>) {
        let last_line = body.matches('\n').count() as u32;
        let input = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx);
            state.set_auto_grow(1, 10, cx);
            state.set_submit_on_enter(true, cx);
            state.set_value(body, window, cx);
            state.set_cursor_position(Position::new(last_line, u32::MAX), window, cx);
            state
        });
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { shift: false, .. } => this.submit_edit(window, cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        })
        .detach();
        input.update(cx, |state, cx| state.focus(window, cx));
        self.menu = None;
        self.confirm_delete = None;
        self.editing = Some(InlineEdit { event_id, input, error: None, saving: false });
        cx.notify();
    }

    /// Start inline-editing the newest editable message of ours (for Up arrow).
    pub fn edit_last(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let found = self.items.iter().enumerate().rev().find_map(|(ix, item)| {
            let e = item.event.as_deref()?;
            if !e.is_editable {
                return None;
            }
            Some((ix, e.event_id.clone()?, editable_body(&e.content)?))
        });
        let Some((ix, event_id, body)) = found else {
            return false;
        };
        self.list.scroll_to_reveal_item(ix);
        self.start_edit(event_id, body, window, cx);
        true
    }

    /// Leave inline edit mode without saving and return focus to the composer.
    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            cx.notify();
            cx.emit(TimelineAction::FocusComposer);
        }
    }

    /// Save the inline edit to Matrix.
    fn submit_edit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = self.editing.as_mut() else { return };
        if editing.saving {
            return;
        }
        let body = editing.input.read(cx).value().trim().to_owned();
        if body.is_empty() {
            self.cancel_edit(cx);
            return;
        }
        let event_id = editing.event_id.clone();
        editing.saving = true;
        editing.error = None;
        cx.notify();

        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, room_id, thread_root, event_id) = (backend.state.clone(), self.room_id.clone(), self.thread_root.clone(), event_id.clone());
            async move {
                let core = state.core().await?;
                timeline::edit(&core, &room_id, thread_root.as_deref(), &event_id, &body, true).await
            }
        });
        cx.spawn(async move |this, cx| {
            let failure = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                let matches = this.editing.as_ref().is_some_and(|e| e.event_id == event_id);
                if !matches {
                    return;
                }
                match failure {
                    None => {
                        this.editing = None;
                        cx.emit(TimelineAction::FocusComposer);
                    }
                    Some(err) => {
                        if let Some(editing) = this.editing.as_mut() {
                            editing.saving = false;
                            editing.error = Some(err.into());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn inline_editor(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(editing) = self.editing.as_ref() else {
            return div().into_any_element();
        };
        let saving = editing.saving;
        let error = editing.error.clone();
        v_flex()
            .w_full()
            .max_w(zpx(640.))
            .mt(zpx(4.))
            .gap(zpx(4.))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.cancel_edit(cx);
                }
            }))
            .child(
                div()
                    .w_full()
                    .px(space(10.))
                    .py(space(6.))
                    .rounded(corner(12.))
                    .bg(palette.card)
                    .border_1()
                    .border_color(palette.accent)
                    .text_size(crate::theme::message_text())
                    .child(Textarea::new(&editing.input).appearance(false)),
            )
            .child(
                h_flex()
                    .gap(zpx(4.))
                    .text_size(text(11.))
                    .text_color(palette.text_tertiary)
                    .when(saving, |row| row.child("saving…"))
                    .when(!saving, |row| {
                        row.child("escape to")
                            .child(
                                div()
                                    .id("inline-edit-cancel")
                                    .cursor_pointer()
                                    .text_color(palette.accent)
                                    .hover(|d| d.underline())
                                    .child("cancel")
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_edit(cx))),
                            )
                            .child("• enter to")
                            .child(
                                div()
                                    .id("inline-edit-save")
                                    .cursor_pointer()
                                    .text_color(palette.accent)
                                    .hover(|d| d.underline())
                                    .child("save")
                                    .on_click(cx.listener(|this, _, window, cx| this.submit_edit(window, cx))),
                            )
                    }),
            )
            .when_some(error, |col, err| col.child(div().text_size(text(11.)).text_color(palette.danger).child(err)))
            .into_any_element()
    }

    /// The newest message from someone else, for the down arrow.
    pub fn last_replyable(&self) -> Option<TimelineAction> {
        self.items.iter().rev().filter_map(|i| i.event.as_deref()).find_map(|e| {
            if e.is_own || !e.can_reply || is_system(&e.content) {
                return None;
            }
            Some(TimelineAction::Reply {
                event_id: e.event_id.clone()?,
                who: e.sender_name.clone().unwrap_or_else(|| e.sender.clone()),
                preview: reply_preview(&e.content),
            })
        })
    }
}

/// The text an edit starts from: the plain body of a text-like message.
fn editable_body(content: &ContentDto) -> Option<String> {
    match content {
        ContentDto::Text { body, .. } | ContentDto::Emote { body, .. } | ContentDto::Notice { body, .. } => Some(body.clone()),
        _ => None,
    }
}

/// One line saying what's being replied to.
fn reply_preview(content: &ContentDto) -> String {
    body_text(content).0.replace('\n', " ")
}

impl Render for Timeline {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Rc::new(cx.global::<Palette>().clone());
        // Zoomed: every row's height changed.
        let epoch = self.store.read(cx).zoom_epoch;
        if epoch != self.zoom_epoch {
            self.zoom_epoch = epoch;
            self.list.remeasure();
        }
        // Something playing wants its next frame (or time) drawn.
        if self.playing.as_ref().is_some_and(|p| p.state.is_playing()) {
            window.request_animation_frame();
        }
        let _ = &window;
        if let Some(error) = &self.error {
            return div().size_full().flex().items_center().justify_center().text_color(palette.danger).child(error.clone()).into_any_element();
        }
        if !self.loaded {
            return div().size_full().flex().items_center().justify_center().text_color(palette.text_tertiary).child("loading…").into_any_element();
        }
        let jump_bar = self.scrolled_up.then(|| {
            h_flex()
                .id("jump-to-present")
                .absolute()
                .bottom(zpx(34.))
                .left(space(22.))
                .right(space(22.))
                .justify_between()
                .items_center()
                .px(space(14.))
                .py(space(7.))
                .rounded(corner(12.))
                .bg(palette.raised)
                .border_1()
                .border_color(palette.border)
                .shadow_md()
                .cursor_pointer()
                .hover(|d| d.bg(palette.card).border_color(palette.accent))
                .on_click(cx.listener(|this, _, _, cx| this.jump_to_present(cx)))
                .child(div().text_xs().text_color(palette.text_secondary).child("you're viewing older messages"))
                .child(div().text_xs().font_weight(FontWeight::BOLD).text_color(palette.accent).child("jump to present ↓"))
        });

        let this: WeakEntity<Self> = cx.weak_entity();
        let palette_for_rows = palette.clone();
        v_flex()
            .relative()
            .size_full()
            .children(self.search_bar(&palette, cx))
            .child(
                list(self.list.clone(), move |ix, _, cx| {
                    this.update(cx, |timeline, cx| timeline.render_item(ix, &palette_for_rows, cx))
                        .unwrap_or_else(|_| div().into_any_element())
                })
                .flex_1()
                .min_h_0()
                .w_full(),
            )
            .child(self.footer(&palette, cx))
            .children(jump_bar)
            .children(self.pins_popover(&palette, cx))
            .children(self.message_menu(&palette, cx))
            .children(self.picker.as_ref().map(|(_, at, picker)| {
                gpui::deferred(
                    gpui::anchored()
                        .position(*at)
                        .anchor(gpui::Anchor::TopRight)
                        .snap_to_window_with_margin(zpx(10.))
                        .child(picker.clone()),
                )
                .with_priority(1)
            }))
            .children(self.notice.clone().map(|notice| {
                h_flex()
                    .id("timeline-notice")
                    .mx_4()
                    .mt_1()
                    .px_3()
                    .py_1()
                    .rounded(palette.radius_md)
                    .bg(gpui::Hsla { a: 0.12, ..palette.danger })
                    .text_xs()
                    .text_color(palette.danger)
                    .cursor_pointer()
                    .child(notice)
                    .child(div().flex_1())
                    .child("dismiss")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.notice = None;
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }
}

impl Timeline {
    /// Close the message menu; whether one was open.
    pub fn close_menu(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.menu.take().is_some();
        if open {
            cx.notify();
        }
        open
    }

    /// A message's right-click menu: the hover bar's actions, and copying.
    fn message_menu(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, at) = self.menu.clone()?;
        let event = self.items.iter().filter_map(|i| i.event.as_deref()).find(|e| e.event_id.as_deref() == Some(id.as_str()))?.clone();
        let close = |this: &mut Timeline, cx: &mut Context<Timeline>| {
            this.menu = None;
            cx.notify();
        };
        let reactable = !matches!(event.content, ContentDto::Redacted);
        let body = editable_body(&event.content);
        let name = event.sender_name.clone().unwrap_or_else(|| event.sender.clone());
        let mut list = menu::frame("message-menu", palette).on_mouse_down_out(cx.listener(move |this, _, _, cx| close(this, cx)));

        if reactable {
            let event_id = id.clone();
            list = list.child(menu::item("mm-react", "react…", true, None, palette).on_click(cx.listener(move |this, _, window, cx| {
                close(this, cx);
                this.open_picker(event_id.clone(), at, window, cx);
            })));
        }
        if event.can_reply {
            let (event_id, who, preview) = (id.clone(), name.clone(), reply_preview(&event.content));
            list = list.child(menu::item("mm-reply", "reply", true, None, palette).on_click(cx.listener(move |this, _, _, cx| {
                close(this, cx);
                cx.emit(TimelineAction::Reply { event_id: event_id.clone(), who: who.clone(), preview: preview.clone() });
            })));
        }
        if self.thread_root.is_none() && reactable {
            let root = event.thread_root.clone().unwrap_or_else(|| id.clone());
            list = list.child(menu::item("mm-thread", "reply in thread", true, None, palette).on_click(cx.listener(move |this, _, _, cx| {
                close(this, cx);
                this.store.update(cx, |s, cx| s.open_thread(Some(root.clone()), cx));
            })));
        }
        if event.is_editable
            && let Some(body) = body.clone()
        {
            let event_id = id.clone();
            list = list.child(menu::item("mm-edit", "edit", true, None, palette).on_click(cx.listener(move |this, _, window, cx| {
                close(this, cx);
                this.start_edit(event_id.clone(), body.clone(), window, cx);
            })));
        }
        list = list.child(menu::separator(palette));
        if let Some(body) = body {
            list = list.child(menu::item("mm-copy", "copy text", true, None, palette).on_click(cx.listener(move |this, _, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(body.clone()));
                close(this, cx);
            })));
        }
        let link = format!("https://matrix.to/#/{}/{}", self.room_id, id);
        list = list.child(menu::item("mm-link", "copy link", true, None, palette).on_click(cx.listener(move |this, _, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(link.clone()));
            close(this, cx);
        })));
        if reactable {
            let pin_id = id.clone();
            let pinned = self.pinned_ids.contains(&id);
            let label = if pinned { "unpin message" } else { "pin message" };
            list = list.child(menu::item("mm-pin", label, true, None, palette).on_click(cx.listener(move |this, _, _, cx| {
                close(this, cx);
                this.set_pin(pin_id.clone(), !pinned, cx);
            })));
        }
        list = list.child(menu::item("mm-unread", "mark as unread", true, None, palette).on_click(cx.listener(move |this, _, _, cx| {
            close(this, cx);
            let room_id = this.room_id.clone();
            let backend = Backend::global(cx);
            let state = backend.state.clone();
            drop(backend.run(async move {
                if let Ok(core) = state.core().await {
                    let _ = horo_core::matrix::rooms::set_marked_unread(&core, &room_id, true).await;
                }
            }));
        })));
        if event.is_own && reactable {
            let event_id = id.clone();
            list = list.child(menu::separator(palette)).child(menu::item("mm-delete", "delete", true, None, palette).text_color(palette.danger).on_click(
                // The bar on the message asks "delete it?", as its own
                // delete button does.
                cx.listener(move |this, _, _, cx| {
                    this.menu = None;
                    this.confirm_delete = Some(event_id.clone());
                    cx.notify();
                }),
            ));
        }
        Some(menu::place(at, list))
    }
}

/// How many readers' faces show before it's just a number.
const MAX_FACES: usize = 5;

impl Timeline {
    /// Under the messages: who's typing, and who has read the last one.
    fn footer(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let own = store.own_user_id().map(str::to_owned);
        // Typing is per room; a thread shows the room's typists too.
        let typing: Vec<String> = store
            .typing
            .get(self.room_id.as_str())
            .map(|users| users.iter().map(|u| u.display_name.clone().unwrap_or_else(|| localpart(&u.user_id).to_owned())).collect())
            .unwrap_or_default();
        let readers: Vec<String> = self
            .items
            .iter()
            .rev()
            // Receipts hang on messages, not on joins and name changes
            // (the core tracks them that way), so the newest message.
            .filter_map(|i| i.event.as_deref())
            .find(|e| !is_system(&e.content))
            .map(|e| e.read_receipts.iter().filter(|u| own.as_deref() != Some(u.as_str())).cloned().collect())
            .unwrap_or_default();

        let left = (!typing.is_empty()).then(|| {
            let dots = h_flex().gap(space(3.)).children((0..3).map(|i| {
                div()
                    .size(zpx(5.))
                    .rounded_full()
                    .bg(palette.accent_secondary)
                    .with_animation(
                        SharedString::from(format!("typing-dot-{i}")),
                        gpui::Animation::new(std::time::Duration::from_millis(1000)).repeat(),
                        move |dot, t| {
                            // Each dot a fifth of a second behind the last.
                            let phase = (t - i as f32 * 0.2).rem_euclid(1.);
                            dot.opacity(0.3 + 0.7 * (phase * std::f32::consts::PI).sin())
                        },
                    )
            }));
            let verb = if typing.len() == 1 { "is" } else { "are" };
            h_flex()
                .gap(space(9.))
                .text_size(text(12.5))
                .text_color(palette.text_secondary)
                .child(dots)
                .child(format!("{} {verb} typing~", join_names(&typing)))
        });

        let right = (!readers.is_empty()).then(|| {
            // Anyone who has spoken in view brings their own face.
            // Anyone who has spoken in view brings their own face; the rest
            // come from their profiles, fetched once each.
            let profiles = &self.store.read(cx).profiles;
            let mut missing = Vec::new();
            let shown: Vec<(String, String, Option<String>)> = readers
                .iter()
                .take(MAX_FACES)
                .map(|user| {
                    let spoke = self.items.iter().rev().filter_map(|i| i.event.as_deref()).find(|e| &e.sender == user);
                    let (name, picture) = match (spoke, profiles.get(user)) {
                        (Some(e), _) if e.sender_avatar.is_some() => (e.sender_name.clone(), e.sender_avatar.clone()),
                        (spoke, Some(known)) => (known.name.clone().or_else(|| spoke.and_then(|e| e.sender_name.clone())), known.avatar.clone()),
                        (spoke, None) => {
                            missing.push(user.clone());
                            (spoke.and_then(|e| e.sender_name.clone()), None)
                        }
                    };
                    (user.clone(), name.unwrap_or_else(|| localpart(user).to_owned()), picture)
                })
                .collect();
            if !missing.is_empty() {
                let store = self.store.clone();
                cx.defer(move |cx| store.update(cx, |s, cx| s.want_profiles(missing, cx)));
            }
            let faces: Vec<AnyElement> = shown.iter().map(|(user, name, picture)| avatar(user, name, picture.as_deref(), zpx(18.), cx)).collect();
            let overflow = readers.len().saturating_sub(MAX_FACES);
            h_flex()
                .id("read-receipts")
                .gap(space(4.))
                .children(faces)
                .when(overflow > 0, |row| {
                    row.child(div().font_family(palette.mono_font.clone()).text_size(text(10.5)).text_color(palette.text_tertiary).child(format!("+{overflow}")))
                })
                .tooltip({
                    let label = SharedString::from(format!("seen by {}", readers.len()));
                    move |window, cx| gpui_component::tooltip::Tooltip::new(label.clone()).build(window, cx)
                })
        });

        h_flex()
            .flex_none()
            .h(zpx(28.))
            .pl(zpx(40.) + space(30.))
            .pr(space(30.))
            .gap(space(12.))
            .children(left)
            .child(div().flex_1())
            .children(right)
            .into_any_element()
    }

    fn render_item(&mut self, ix: usize, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.items.get(ix).cloned() else { return div().into_any_element() };
        let row = match item.kind {
            TimelineRowKind::DateDivider | TimelineRowKind::ReadMarker if self.empty_divider(ix) => div().into_any_element(),
            TimelineRowKind::DateDivider => divider(item.timestamp.map(day_label).unwrap_or_default(), palette.text_tertiary, palette.border_subtle),
            TimelineRowKind::ReadMarker => divider("new".into(), palette.accent_secondary, palette.accent_secondary),
            TimelineRowKind::TimelineStart => div()
                .w_full()
                .pt(space(14.))
                .pb(space(18.))
                .flex()
                .justify_center()
                .text_size(text(12.5))
                .text_color(palette.text_tertiary)
                .child("this is the very beginning~")
                .into_any_element(),
            TimelineRowKind::Event => match &item.event {
                // Call bookkeeping: the call UI already says who's in it.
                Some(event) if !worth_showing(&event.content) => div().into_any_element(),
                Some(event) => {
                    let continues = self.items[..ix]
                        .iter()
                        .rev()
                        .find(|i| i.kind != TimelineRowKind::ReadMarker && i.event.as_deref().is_none_or(|e| worth_showing(&e.content)))
                        .and_then(|prev| prev.event.as_deref())
                        .is_some_and(|prev| continues_group(prev, event));
                    let search_hit = self.search_matches.get(self.search_cursor) == Some(&ix);
                    self.message(event, continues, search_hit, palette, cx)
                }
                None => div().into_any_element(),
            },
        };
        // While older history loads, say so above the top row (as the
        // WebView build's spinner does); otherwise nothing.
        if ix == 0 && self.paginating && !self.at_start {
            return v_flex()
                .w_full()
                .child(div().w_full().py_2().flex().justify_center().text_xs().text_color(palette.text_tertiary).child("loading earlier messages…"))
                .child(row)
                .into_any_element();
        }
        row
    }
}

/// Same sender, close in time, both ordinary messages, and not a reply: no second header.
fn continues_group(prev: &EventItemDto, event: &EventItemDto) -> bool {
    prev.sender == event.sender
        && event.timestamp.saturating_sub(prev.timestamp) < GROUP_WINDOW_MS
        && event.reply.is_none()
        && !is_system(&prev.content)
        && !is_system(&event.content)
}

/// State events that are pure call bookkeeping: MatrixRTC republishes
/// membership on every join, leave and refresh.
const CALL_STATE_EVENTS: [&str; 3] = ["org.matrix.msc3401.call.member", "m.rtc.member", "m.call.member"];

/// Not call bookkeeping, nor a "started a call" notice left behind.
fn worth_showing(content: &ContentDto) -> bool {
    match content {
        ContentDto::CallInvite | ContentDto::RtcNotification { .. } => false,
        ContentDto::State { event_type, .. } => !CALL_STATE_EVENTS.contains(&event_type.as_str()),
        _ => true,
    }
}

/// Membership, profile and state changes: a quiet line, not a message.
fn is_system(content: &ContentDto) -> bool {
    matches!(content, ContentDto::Membership { .. } | ContentDto::ProfileChange { .. } | ContentDto::State { .. } | ContentDto::CallInvite)
}

fn localpart(user_id: &str) -> &str {
    user_id.trim_start_matches('@').split(':').next().unwrap_or(user_id)
}

/// "a", "a and b", "a, b and c", "a, b and 2 others".
fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} and {b}"),
        [a, b, c] => format!("{a}, {b} and {c}"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} others", rest.len()),
    }
}

fn local(ms: u64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(ms as i64).single()
}

fn day_label(ms: u64) -> String {
    let Some(when) = local(ms) else { return String::new() };
    let today = Local::now().date_naive();
    match (today - when.date_naive()).num_days() {
        0 => "today".into(),
        1 => "yesterday".into(),
        _ => when.format("%A, %-d %B").to_string().to_lowercase(),
    }
}

fn time_label(ms: u64) -> String {
    local(ms).map(|t| t.format("%H:%M").to_string()).unwrap_or_default()
}

/// A day between messages, as TimelineView.tsx: a line, the day, a line.
fn divider(label: String, colour: gpui::Hsla, line: gpui::Hsla) -> AnyElement {
    h_flex()
        .w_full()
        .px(space(22.))
        .pt(zpx(18.))
        .pb(zpx(10.))
        .gap(space(12.))
        .child(div().flex_1().h(zpx(1.)).bg(line))
        .child(div().text_size(text(11.)).font_weight(FontWeight::SEMIBOLD).text_color(colour).child(label))
        .child(div().flex_1().h(zpx(1.)).bg(line))
        .into_any_element()
}

/// A one-line description, for what isn't drawn any richer.
fn body_text(content: &ContentDto) -> (String, bool) {
    match content {
        ContentDto::Text { body, .. } | ContentDto::Notice { body, .. } => (body.clone(), false),
        ContentDto::Emote { body, .. } => (format!("* {body}"), false),
        ContentDto::Image { body, .. } => (format!("🖼  {body}"), true),
        ContentDto::Video { body, .. } => (format!("🎞  {body}"), true),
        ContentDto::Audio { media, .. } if media.is_voice => ("🎙  voice message".into(), true),
        ContentDto::Audio { body, .. } => (format!("🎵  {body}"), true),
        ContentDto::File { body, .. } => (format!("📎  {body}"), true),
        ContentDto::Sticker { body, .. } => (format!("sticker: {body}"), true),
        ContentDto::Location { body, .. } => (format!("📍  {body}"), true),
        ContentDto::Poll { question, answers, ended } => {
            (format!("📊  {question}{}\n{}", if *ended { " (ended)" } else { "" }, answers.join(" · ")), true)
        }
        ContentDto::Redacted => ("message deleted".into(), true),
        ContentDto::UnableToDecrypt { .. } => ("can't decrypt this message yet".into(), true),
        ContentDto::Membership { change, display_name, user_id } => {
            (format!("{} {change}", display_name.as_deref().unwrap_or(user_id)), true)
        }
        ContentDto::ProfileChange { summary } | ContentDto::State { summary, .. } => (summary.clone(), true),
        ContentDto::CallInvite => ("started a call".into(), true),
        #[allow(unreachable_patterns)]
        _ => ("unsupported message".into(), true),
    }
}

/// A row's stable id, for element ids inside it.
fn event_key(event: &EventItemDto) -> String {
    event.event_id.clone().or_else(|| event.transaction_id.clone()).unwrap_or_else(|| format!("{}-{}", event.sender, event.timestamp))
}

impl Timeline {
    /// The parsed blocks of `html`, parsed once.
    fn blocks(&mut self, html: &str) -> Rc<Vec<Block>> {
        if self.rich.len() > 2000 {
            self.rich.clear();
        }
        self.rich.entry(html.to_owned()).or_insert_with(|| Rc::new(rich::parse(html))).clone()
    }

    /// Play a video or a sound in its message. Ogg/Opus voice messages play
    /// here everywhere; anything else here on macOS, and elsewhere in the
    /// system's player.
    fn start_media(&mut self, event: &EventItemDto, body: &str, media: &MediaInfoDto, video: bool, cx: &mut Context<Self>) {
        let (Some(event_id), Some(mxc)) = (event.event_id.clone(), media.mxc.clone()) else { return };
        let (name, mime) = (body.to_owned(), media.mimetype.clone());
        let ogg = mime.as_deref().is_some_and(|m| m.starts_with("audio/ogg") || m.starts_with("audio/opus"));
        if !cfg!(target_os = "macos") && !ogg {
            content::play_externally(mxc, name, mime, cx);
            return;
        }
        let speaker = self.store.read(cx).prefs.audio_output.clone();
        self.playing = Some(InlineMedia { event_id: event_id.clone(), path: None, state: PlayState::Loading, bar: Rc::new(std::cell::Cell::new(None)) });
        cx.notify();
        let download = content::download_temp(mxc, &name, mime.as_deref(), cx);
        cx.spawn(async move |this, cx| {
            let outcome = download.await;
            // An Ogg file (by its first bytes, whatever its label says) is
            // decoded here, off the UI thread, and played by horo-rtc.
            let decoded = match &outcome {
                Ok(path) if !video => {
                    let path = path.clone();
                    cx.background_spawn(async move {
                        let bytes = std::fs::read(&path).ok()?;
                        bytes.starts_with(b"OggS").then(|| horo_rtc::voice::decode(&bytes).map_err(|e| e.to_string()))
                    })
                    .await
                }
                _ => None,
            };
            let _ = this.update(cx, |timeline, cx| {
                // Moved on to something else (or closed this) meanwhile.
                let Some(playing) = timeline.playing.as_mut().filter(|p| p.event_id == event_id) else { return };
                playing.state = match (outcome, decoded) {
                    (Ok(path), Some(samples)) => {
                        playing.path = Some(path);
                        match samples.and_then(|s| horo_rtc::voice::VoicePlayer::start(s, speaker.as_deref()).map_err(|e| e.to_string())) {
                            Ok(player) => PlayState::Voice(player),
                            Err(e) => PlayState::Failed(format!("couldn't play it: {e}").into()),
                        }
                    }
                    (Ok(path), None) => {
                        let state = open_player(&path, video);
                        playing.path = Some(path);
                        state
                    }
                    (Err(e), _) => PlayState::Failed(format!("couldn't fetch it: {e}").into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Whether `event` is the one playing.
    fn is_playing(&self, event: &EventItemDto) -> bool {
        self.playing.as_ref().is_some_and(|p| Some(&p.event_id) == event.event_id.as_ref())
    }

    /// Play/pause, a bar to see and seek with, the time, and (for video)
    /// open in player and close.
    fn controls(&mut self, duration_ms: Option<u64>, waveform: Option<&[u16]>, video: bool, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(playing) = self.playing.as_mut() else { return div().into_any_element() };
        let path = playing.path.clone();
        let bar = playing.bar.clone();
        let (state, fraction, time): (Option<bool>, f32, String) = match &mut playing.state {
            PlayState::Loading => (None, 0., "fetching it…".into()),
            PlayState::Failed(why) => return div().text_size(text(12.)).text_color(palette.danger).child(why.clone()).into_any_element(),
            ready => {
                let (at, length) = ready.position().unwrap_or((0., 0.));
                let length = if length > 0. { length } else { duration_ms.unwrap_or(0) as f64 / 1000. };
                let time = if length > 0. { format!("{} / {}", clock(at), clock(length)) } else { clock(at) };
                (Some(ready.is_playing()), if length > 0. { (at / length).clamp(0., 1.) as f32 } else { 0. }, time)
            }
        };
        let button = |key: &'static str| {
            div().id(key).flex_none().size(zpx(28.)).flex().items_center().justify_center().rounded_full().cursor_pointer().hover(|d| d.bg(palette.inset))
        };
        let toggle = button("media-toggle")
            .child(icon(if state == Some(true) { "icons/pause.svg" } else { "icons/play.svg" }).size(zpx(15.)).text_color(palette.text))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(playing) = &this.playing {
                    playing.state.toggle();
                }
                cx.notify();
            }));
        let record = bar.clone();
        let track = div()
            .id("media-bar")
            .relative()
            .flex_1()
            .min_w(zpx(60.))
            .h(zpx(if waveform.is_some() { 24. } else { 14. }))
            .flex()
            .items_center()
            .cursor_pointer()
            .child(match waveform {
                Some(samples) => wave(samples, fraction, palette),
                None => div().w_full().h(zpx(4.)).rounded_full().bg(palette.inset).child(div().h_full().w(gpui::relative(fraction)).rounded_full().bg(palette.accent)).into_any_element(),
            })
            // Where the bar was drawn, so a press on it knows how far along.
            .child(gpui::canvas(move |bounds, _, _| record.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0())
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                let Some(bounds) = bar.get() else { return };
                let along = f32::from(event.position.x - bounds.origin.x) / f32::from(bounds.size.width).max(1.);
                if let Some(playing) = &this.playing {
                    playing.state.seek(along.clamp(0., 1.) as f64);
                }
                cx.stop_propagation();
                cx.notify();
            }));
        h_flex()
            .w_full()
            .gap(space(8.))
            .child(toggle)
            .child(track)
            .child(div().flex_none().font_family(palette.mono_font.clone()).text_size(text(10.5)).text_color(palette.text_tertiary).child(time))
            .when(video, |row| {
                row.children(path.map(|path| {
                    div()
                        .id("media-external")
                        .flex_none()
                        .px(space(6.))
                        .rounded(corner(8.))
                        .cursor_pointer()
                        .text_size(text(11.))
                        .text_color(palette.text_secondary)
                        .hover(|d| d.bg(palette.inset))
                        .child("open in player")
                        .on_click(move |_, _, cx| cx.open_with_system(&path))
                }))
                .child(button("media-close").text_size(text(12.)).text_color(palette.text_tertiary).child("✕").on_click(cx.listener(|this, _, _, cx| {
                    this.playing = None;
                    cx.notify();
                })))
            })
            .into_any_element()
    }

    /// A video, as TimelineView's <video> shows it: a frame the size it
    /// plays at, with its poster (when it has one), a play button and its
    /// length; once playing, its frames and the controls under them.
    fn video(&mut self, event: &EventItemDto, body: &str, media: &MediaInfoDto, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        const MAX: (f32, f32) = (400., 320.);
        // Without a size in the event, a 16:9 frame (what most clips are)
        // rather than a tall 4:3 box, then the poster's own shape once in.
        let (w, h) = match media.width.zip(media.height) {
            Some(_) => crate::media::fit(media.width, media.height, MAX, 120.),
            None => (MAX.0, (MAX.0 * 9. / 16.).round()),
        };
        let poster = media.thumbnail_mxc.as_deref().and_then(|mxc| Media::image(mxc, Some(((w * 2.) as u32, (h * 2.) as u32)), cx));
        let (w, h) = match (&poster, media.width.zip(media.height)) {
            (Some(poster), None) => imagesize::blob_size(poster.bytes())
                .ok()
                .map(|s| crate::media::fit(Some(s.width as u64), Some(s.height as u64), MAX, 120.))
                .unwrap_or((w, h)),
            _ => (w, h),
        };
        let frame = div().relative().w(zpx(w)).h(zpx(h)).bg(gpui::black()).flex().items_center().justify_center().overflow_hidden();
        if self.is_playing(event) {
            #[cfg(target_os = "macos")]
            let picture = match &mut self.playing {
                Some(InlineMedia { state: PlayState::Playing(player), .. }) => player.frame().map(|f| gpui::surface(f).w(zpx(w)).h(zpx(h)).into_any_element()),
                _ => None,
            };
            #[cfg(not(target_os = "macos"))]
            let picture: Option<AnyElement> = None;
            let controls = self.controls(media.duration_ms, None, true, palette, cx);
            // Wide enough for the controls; a narrow video sits centred.
            let wide = w.max(300.);
            return v_flex()
                .w(zpx(wide))
                .rounded(corner(10.))
                .overflow_hidden()
                .bg(palette.card)
                // Its black rounds itself: gpui clips to a rectangle.
                .child(frame.w(zpx(wide)).rounded_t(corner(10.)).children(picture))
                .child(div().px(space(8.)).py(space(5.)).child(controls))
                .into_any_element();
        }
        let length = media.duration_ms.map(crate::views::content::duration);
        let size = media.size.map(|n| match n {
            n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
            n => format!("{} KB", (n >> 10).max(1)),
        });
        let has_poster = poster.is_some();
        let (event, body, media) = (event.clone(), body.to_owned(), media.clone());
        frame
            .id(SharedString::from(format!("video/{}", event_key(&event))))
            .rounded(corner(10.))
            .when(!has_poster, |d| d.bg(palette.card).border_1().border_color(palette.border_subtle))
            .cursor_pointer()
            .children(poster.map(|poster| img(poster).absolute().inset_0().size_full().aspect_ratio(w / h).rounded(corner(10.)).object_fit(gpui::ObjectFit::Cover)))
            .child(
                div()
                    .size(zpx(48.))
                    .rounded_full()
                    .bg(Hsla { a: 0.72, ..gpui::black() })
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|d| d.bg(Hsla { a: 0.85, ..gpui::black() }))
                    .child(icon("icons/play.svg").size(zpx(20.)).text_color(gpui::white())),
            )
            // No poster: what it is and how big, along the foot, as a file
            // would say it, instead of a name floating in a black box.
            .when(!has_poster, |d| {
                d.child(
                    h_flex()
                        .absolute()
                        .left(zpx(12.))
                        .right(zpx(12.))
                        .bottom(zpx(10.))
                        .gap(space(8.))
                        .child(icon("icons/video.svg").flex_none().size(zpx(14.)).text_color(palette.text_tertiary))
                        .child(div().flex_1().min_w_0().truncate().text_size(text(12.)).text_color(palette.text_secondary).child(body.clone()))
                        .children(size.map(|s| div().flex_none().font_family(palette.mono_font.clone()).text_size(text(10.5)).text_color(palette.text_tertiary).child(s))),
                )
            })
            .children(length.map(|length| {
                div()
                    .absolute()
                    .top(zpx(8.))
                    .right(zpx(8.))
                    .px(space(6.))
                    .py(zpx(1.))
                    .rounded(corner(6.))
                    .bg(Hsla { a: 0.72, ..gpui::black() })
                    .font_family(palette.mono_font.clone())
                    .text_size(text(10.5))
                    .text_color(gpui::white())
                    .child(length)
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.start_media(&event, &body, &media, true, cx)))
            .into_any_element()
    }

    /// A sound or a voice message, as AudioBody: its icon, then play and
    /// the bar (or, once playing, the controls), then its length.
    fn audio(&mut self, event: &EventItemDto, body: &str, media: &MediaInfoDto, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let glyph = icon(if media.is_voice { "icons/mic.svg" } else { "icons/music.svg" }).flex_none().size(zpx(16.)).text_color(palette.accent_secondary);
        let middle = if self.is_playing(event) {
            self.controls(media.duration_ms, media.waveform.as_deref().filter(|w| !w.is_empty()), false, palette, cx)
        } else {
            let length = media.duration_ms;
            let waveform = media.waveform.clone().filter(|w| !w.is_empty());
            let (event, body, media) = (event.clone(), body.to_owned(), media.clone());
            h_flex()
                .w_full()
                .gap(space(8.))
                .child(
                    div()
                        .id(SharedString::from(format!("audio/{}", event_key(&event))))
                        .flex_none()
                        .size(zpx(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .hover(|d| d.bg(palette.inset))
                        .child(icon("icons/play.svg").size(zpx(15.)).text_color(palette.text))
                        .on_click(cx.listener(move |this, _, _, cx| this.start_media(&event, &body, &media, false, cx))),
                )
                .child(match waveform {
                    Some(samples) => div().flex_1().min_w(zpx(60.)).h(zpx(24.)).flex().items_center().child(wave(&samples, 0., palette)).into_any_element(),
                    None => div().flex_1().min_w(zpx(60.)).h(zpx(4.)).rounded_full().bg(palette.inset).into_any_element(),
                })
                .children(length.map(|ms| {
                    div().flex_none().font_family(palette.mono_font.clone()).text_size(text(10.5)).text_color(palette.text_tertiary).child(crate::views::content::duration(ms))
                }))
                .into_any_element()
        };
        h_flex()
            .mt(zpx(4.))
            .w(zpx(340.))
            .max_w_full()
            .gap(space(10.))
            .px(space(12.))
            .py(space(7.))
            .rounded(corner(16.))
            .bg(palette.card)
            .border_1()
            .border_color(palette.border_subtle)
            .child(glyph)
            .child(div().flex_1().min_w_0().child(middle))
            .into_any_element()
    }

    /// The message's body: formatted text, a picture, a card, or a line.
    fn body(&mut self, event: &EventItemDto, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = event_key(event);
        let store = self.store.clone();
        let this = cx.weak_entity();
        let reveal_key = id.clone();
        let rich_ctx = content::RichContext {
            store: store.clone(),
            spoilers_revealed: self.revealed_spoilers.contains(&id),
            on_reveal_spoiler: Rc::new(move |cx: &mut App| {
                let key = reveal_key.clone();
                let _ = this.update(cx, |t, cx| {
                    t.revealed_spoilers.insert(key);
                    cx.notify();
                });
            }),
        };
        match &event.content {
            ContentDto::Text { formatted: Some(html), .. } | ContentDto::Notice { formatted: Some(html), .. } if !event.is_emoji_only => {
                let colour = if matches!(event.content, ContentDto::Notice { .. }) { palette.text_secondary } else { palette.text };
                let blocks = self.blocks(html);
                content::rich(&id, &blocks, colour, &rich_ctx, palette, cx)
            }
            ContentDto::Emote { formatted: Some(html), .. } => {
                let blocks = self.blocks(&format!("* {html}"));
                div().italic().child(content::rich(&id, &blocks, palette.text, &rich_ctx, palette, cx)).into_any_element()
            }
            ContentDto::Text { body, .. } | ContentDto::Notice { body, .. } if !event.is_emoji_only => {
                let colour = if matches!(event.content, ContentDto::Notice { .. }) { palette.text_secondary } else { palette.text };
                div().w_full().text_color(colour).child(content::linked_plain(&id, body, colour, &rich_ctx, palette)).into_any_element()
            }
            ContentDto::Emote { body, .. } => {
                div().w_full().italic().text_color(palette.text).child(content::linked_plain(&id, &format!("* {body}"), palette.text, &rich_ctx, palette)).into_any_element()
            }
            ContentDto::Image { body, media } => {
                let entity = cx.entity().clone();
                let store = store.clone();
                let key = event_key(event);
                let fallback = LightboxItem {
                    mxc: media.mxc.clone(),
                    thumbnail: media.thumbnail_mxc.clone().or_else(|| media.mxc.clone()),
                    width: media.width,
                    height: media.height,
                    name: body.clone(),
                };
                content::picture(&id, media, false, palette, cx, move |cx| {
                    let lightbox = entity.read(cx).lightbox_for(&key, fallback.clone());
                    store.update(cx, |s, cx| s.open_lightbox(lightbox, cx));
                })
            }
            ContentDto::Sticker { body, media } => {
                let store = store.clone();
                let item = LightboxItem {
                    mxc: media.mxc.clone(),
                    thumbnail: media.thumbnail_mxc.clone().or_else(|| media.mxc.clone()),
                    width: media.width,
                    height: media.height,
                    name: body.clone(),
                };
                content::picture(&id, media, true, palette, cx, move |cx| {
                    store.update(cx, |s, cx| s.open_lightbox(Lightbox::single(item.clone()), cx));
                })
            }
            ContentDto::Video { body, media } => self.video(event, body, media, palette, cx),
            ContentDto::Audio { body, media } => {
                self.audio(event, body, media, palette, cx)
            }
            ContentDto::File { body, media } => content::card(&id, "📎", body, media, Open::Save, palette, cx),
            content => {
                let (text, muted) = body_text(content);
                div()
                    .w_full()
                    .text_color(if muted { palette.text_tertiary } else { palette.text })
                    .when(muted, |d| d.italic())
                    .when(event.is_emoji_only, |d| d.text_size(crate::theme::emoji_message_text()))
                    .child(content::plain(format!("select/{}", event_key(event)), text, palette))
                    .into_any_element()
            }
        }
    }

    /// Collect every picture in the loaded timeline in chronological order and
    /// point the lightbox at the clicked one so the user can step through
    /// previously/subsequently sent media with arrows.
    fn lightbox_for(&self, clicked_key: &str, fallback: LightboxItem) -> Lightbox {
        let mut items = Vec::new();
        let mut index = None;
        for ev in self.items.iter().filter_map(|i| i.event.as_deref()) {
            if let ContentDto::Image { body, media } = &ev.content {
                if event_key(ev) == clicked_key {
                    index = Some(items.len());
                }
                items.push(LightboxItem {
                    mxc: media.mxc.clone(),
                    thumbnail: media.thumbnail_mxc.clone().or_else(|| media.mxc.clone()),
                    width: media.width,
                    height: media.height,
                    name: body.clone(),
                });
            }
        }
        match index {
            Some(index) => Lightbox { items, index },
            None => Lightbox::single(fallback),
        }
    }

    /// One message, as MessageRow.tsx lays it out: a 40px column with the
    /// face (or, continuing a group, the time on hover), then the sender in
    /// their accent, the time and any device warning, the reply, the body,
    /// reactions and the thread.
    fn link_preview(&mut self, event: &EventItemDto, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.store.read(cx).prefs.link_previews {
            return None;
        }
        let body = match &event.content {
            ContentDto::Text { body, .. } => body,
            _ => return None,
        };
        let url = horo_core::link_preview::first_url(body)?;
        match self.link_previews.get(&url).cloned() {
            Some(LinkPreviewState::Ready(preview)) => {
                let open_url = preview.url.clone();
                let embed_image = preview.image.as_deref().and_then(|img_url| Media::remote_image(img_url, cx));
                if preview.is_direct_image {
                    let image = embed_image?;
                    return Some(
                        div()
                            .id(SharedString::from(format!("lp-img/{}", event_key(event))))
                            .mt(zpx(6.))
                            .max_w(zpx(380.))
                            .max_h(zpx(280.))
                            .rounded(corner(10.))
                            .overflow_hidden()
                            .cursor_pointer()
                            .on_click(move |_, _, cx| cx.open_url(&open_url))
                            .child(img(image).max_w(zpx(380.)).max_h(zpx(280.)).object_fit(gpui::ObjectFit::Contain))
                            .into_any_element(),
                    );
                }
                let bar_color: Hsla = preview.color.map(|c| gpui::rgb(c).into()).unwrap_or(palette.accent);
                Some(
                    h_flex()
                        .id(SharedString::from(format!("lp/{}", event_key(event))))
                        .mt(zpx(6.))
                        .max_w(zpx(440.))
                        .rounded(corner(10.))
                        .overflow_hidden()
                        .bg(palette.card)
                        .border_1()
                        .border_color(palette.border_subtle)
                        .cursor_pointer()
                        .hover(|d| d.bg(palette.raised))
                        .on_click(move |_, _, cx| cx.open_url(&open_url))
                        .child(div().flex_none().w(zpx(3.5)).self_stretch().bg(bar_color))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(zpx(3.))
                                .px(space(12.))
                                .py(space(9.))
                                .child(div().text_size(text(11.)).text_color(palette.text_tertiary).child(preview.site))
                                .child(div().text_size(text(13.5)).font_weight(FontWeight::BOLD).text_color(palette.accent).child(preview.title))
                                .children(preview.description.map(|desc| div().text_size(text(12.)).text_color(palette.text_secondary).child(desc)))
                                .children(embed_image.map(|image| {
                                    div()
                                        .mt(zpx(6.))
                                        .max_w(zpx(380.))
                                        .max_h(zpx(210.))
                                        .rounded(corner(8.))
                                        .overflow_hidden()
                                        .child(img(image).w_full().max_h(zpx(210.)).object_fit(gpui::ObjectFit::Cover))
                                })),
                        )
                        .into_any_element(),
                )
            }
            Some(LinkPreviewState::Loading | LinkPreviewState::None) => None,
            None => {
                self.link_previews.insert(url.clone(), LinkPreviewState::Loading);
                let fetch_url = url.clone();
                let backend = Backend::global(cx);
                let task = backend.run(async move { horo_core::link_preview::fetch(&fetch_url).await });
                cx.spawn(async move |this, cx| {
                    let result = task.await.ok().flatten();
                    let _ = this.update(cx, |this, cx| {
                        this.link_previews.insert(
                            url,
                            match result {
                                Some(preview) => LinkPreviewState::Ready(preview),
                                None => LinkPreviewState::None,
                            },
                        );
                        cx.notify();
                    });
                })
                .detach();
                None
            }
        }
    }

    fn message(&mut self, event: &EventItemDto, continues: bool, search_hit: bool, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let name = event.sender_name.clone().unwrap_or_else(|| event.sender.trim_start_matches('@').split(':').next().unwrap_or_default().to_owned());

        if is_system(&event.content) {
            let (raw_line, _) = body_text(&event.content);
            let line = match &event.content {
                ContentDto::State { .. } | ContentDto::CallInvite => format!("{name} {raw_line}"),
                _ => raw_line,
            };
            return h_flex()
                .w_full()
                .px(space(22.))
                .child(
                    h_flex()
                        .group("system-msg")
                        .w_full()
                        .gap(space(10.))
                        .py(space(6.))
                        .pl(zpx(40.) + space(12.) + space(8.))
                        .text_size(text(12.))
                        .text_color(palette.text_tertiary)
                        .child(div().flex_1().min_w_0().child(line))
                        .child(
                            div()
                                .flex_none()
                                .font_family(palette.mono_font.clone())
                                .text_size(text(10.5))
                                .invisible()
                                .group_hover("system-msg", |s| s.visible())
                                .child(time_label(event.timestamp)),
                        ),
                )
                .into_any_element();
        }

        let is_editing = self.editing.as_ref().is_some_and(|e| event.event_id.as_deref() == Some(e.event_id.as_str()));

        let body = if is_editing {
            self.inline_editor(palette, cx)
        } else {
            v_flex()
                .w_full()
                .max_w(zpx(640.))
                .text_size(crate::theme::message_text())
                .line_height(gpui::relative(1.375))
                .child(self.body(event, palette, cx))
                .when(event.is_edited, |d| d.child(div().text_size(text(10.)).text_color(palette.text_tertiary).child("(edited)")))
                .into_any_element()
        };

        let reply = event.reply.as_ref().map(|reply| {
            let who = reply.sender_name.clone().or_else(|| reply.sender.clone()).unwrap_or_default();
            h_flex()
                .mb(zpx(5.))
                .gap(space(8.))
                .px(space(11.))
                .py(space(5.))
                .max_w(zpx(520.))
                .rounded(corner(12.))
                .bg(gpui::Hsla { a: 0.04, ..gpui::white() })
                .border_l_2()
                .border_color(palette.accent_tertiary)
                .text_size(text(12.))
                .text_color(palette.text_secondary)
                .child(div().flex_none().font_weight(FontWeight::BOLD).text_color(accent_for(reply.sender.as_deref().unwrap_or_default())).child(who))
                .child(div().min_w_0().truncate().child(reply.body.clone().unwrap_or_else(|| "…".into()).replace('\n', " ")))
        });

        let reactions = (!event.reactions.is_empty()).then(|| {
            let known = packs::image_lookup(&self.store.read(cx).packs);
            let chips: Vec<AnyElement> = event
                .reactions
                .iter()
                .map(|r| {
                    let chip = reaction(r, &known, palette, cx);
                    let tip: SharedString = r.senders.iter().map(|u| localpart(u)).collect::<Vec<_>>().join(", ").into();
                    match &event.event_id {
                        Some(id) => {
                            let (id, key) = (id.clone(), r.key.clone());
                            div()
                                .id(SharedString::from(format!("react/{id}/{key}")))
                                .cursor_pointer()
                                .when(!tip.is_empty(), |d| d.tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)))
                                .child(chip)
                                .on_click(cx.listener(move |this, _, _, cx| this.toggle_reaction(id.clone(), key.clone(), cx)))
                                .into_any_element()
                        }
                        None => chip,
                    }
                })
                .collect();
            h_flex().flex_wrap().gap(space(6.)).mt(zpx(7.)).children(chips)
        });

        let thread_target = event.event_id.clone();
        let thread = event.thread_summary.as_ref().filter(|_| self.thread_root.is_none()).map(|t| {
            let latest = t.latest_body.clone().map(|b| b.replace('\n', " "));
            h_flex()
                .id(SharedString::from(format!("thread-chip/{}", thread_target.clone().unwrap_or_default())))
                .cursor_pointer()
                .hover(|d| d.bg(palette.raised))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(root) = thread_target.clone() {
                        this.store.update(cx, |s, cx| s.open_thread(Some(root), cx));
                    }
                }))
                .max_w(zpx(460.))
                .mt(zpx(8.))
                .gap(space(8.))
                .pl(space(8.))
                .pr(space(12.))
                .py(space(6.))
                .rounded_full()
                .bg(palette.card)
                .border_1()
                .border_color(palette.border_subtle)
                .font_weight(FontWeight::BOLD)
                .text_size(text(12.5))
                .text_color(palette.accent_quaternary)
                .child(div().flex_none().child(format!("{} {} in thread", t.num_replies, if t.num_replies == 1 { "reply" } else { "replies" })))
                .children(latest.map(|b| div().min_w_0().truncate().font_weight(FontWeight::NORMAL).text_color(palette.text_tertiary).child(b)))
        });

        let pending = event.send_state.as_ref().is_some_and(|s| matches!(s, horo_core::dto::SendStateDto::NotSentYet { .. }));
        let failed = event.send_state.as_ref().is_some_and(|s| matches!(s, horo_core::dto::SendStateDto::Failed { .. }));

        let shield = event.shield.as_ref().map(|shield| {
            let message = shield.message.clone();
            div()
                .id(SharedString::from(format!("shield/{}", event_key(event))))
                .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(message.clone()).build(window, cx))
                .child(
                    gpui_component::Icon::new(gpui_component::IconName::TriangleAlert)
                        .size(zpx(12.))
                        .text_color(if shield.colour == horo_core::dto::ShieldColour::Red { palette.danger } else { palette.text_tertiary }),
                )
        });

        let sender_color = self
            .store
            .read(cx)
            .role_color_for(self.room_id.as_str(), &event.sender)
            .unwrap_or_else(|| accent_for(&event.sender));
        let header = (!continues).then(|| {
            let (sender, hint) = (event.sender.clone(), event.sender_avatar.clone());
            h_flex()
                .mb(zpx(1.))
                .gap(space(8.))
                .flex_wrap()
                .child(
                    div()
                        .id(SharedString::from(format!("name/{}", event_key(event))))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_size(text(14.5))
                        .text_color(sender_color)
                        .cursor_pointer()
                        .hover(|d| d.underline())
                        .child(name.clone())
                        .on_click(cx.listener(move |this, click: &gpui::ClickEvent, _, cx| this.open_profile(sender.clone(), click.position(), hint.clone(), cx))),
                )
                .child(
                    div()
                        .font_family(palette.mono_font.clone())
                        .text_size(text(10.5))
                        .text_color(palette.text_tertiary)
                        .invisible()
                        .group_hover("message", |s| s.visible())
                        .child(time_label(event.timestamp)),
                )
                .children(shield)
        });

        let preview_card = if is_editing { None } else { self.link_preview(event, palette, cx) };
        let content = v_flex()
            .flex_1()
            .min_w_0()
            .children(header)
            .children(reply)
            .child(body)
            .children(preview_card)
            .children(reactions)
            .children(thread)
            .when(failed, |c| {
                c.child(div().mt(zpx(6.)).text_size(text(11.5)).text_color(palette.danger).child("couldn't send — it'll retry when you're back online"))
            });

        let column = if continues {
            // The time, quietly, when the pointer is on the row.
            div()
                .w(zpx(40.))
                .flex_none()
                .flex()
                .justify_center()
                .pt(zpx(3.))
                .font_family(palette.mono_font.clone())
                .text_size(text(9.5))
                .text_color(palette.text_tertiary)
                .invisible()
                .group_hover("message", |s| s.visible())
                .child(time_label(event.timestamp))
                .into_any_element()
        } else {
            let picture = avatar(&event.sender, &name, event.sender_avatar.as_deref(), zpx(38.), cx);
            let (sender, hint) = (event.sender.clone(), event.sender_avatar.clone());
            div()
                .id(SharedString::from(format!("face/{}", event_key(event))))
                .w(zpx(40.))
                .flex_none()
                .flex()
                .justify_center()
                .pt(zpx(2.))
                .cursor_pointer()
                .child(picture)
                .on_click(cx.listener(move |this, click: &gpui::ClickEvent, _, cx| this.open_profile(sender.clone(), click.position(), hint.clone(), cx)))
                .into_any_element()
        };

        let actions = if is_editing { None } else { self.actions(event, &name, palette, cx) };

        h_flex()
            .w_full()
            .px(space(22.))
            .when(!continues, |d| d.pt(space(14.)))
            .child(
                h_flex()
                    .group("message")
                    .relative()
                    .w_full()
                    .items_start()
                    .gap(space(12.))
                    .px(space(8.))
                    .py(zpx(2.))
                    .rounded(corner(8.))
                    .when(!event.is_highlighted && !search_hit && !is_editing, |row| row.hover(|row| row.bg(gpui::Hsla { a: 0.035, ..gpui::white() })))
                    .when(event.is_highlighted && !search_hit && !is_editing, |row| row.bg(gpui::Hsla { a: 0.08, ..palette.accent_secondary }))
                    .when(is_editing, |row| row.bg(gpui::Hsla { a: 0.06, ..palette.accent }))
                    .when(search_hit && !is_editing, |row| row.bg(gpui::Hsla { a: 0.16, ..palette.accent }))
                    .when(pending, |row| row.opacity(0.72))
                    .when_some(event.event_id.clone(), |row, id| {
                        row.on_mouse_down(
                            gpui::MouseButton::Right,
                            cx.listener(move |this, down: &gpui::MouseDownEvent, _, cx| {
                                this.menu = Some((id.clone(), down.position));
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                    })
                    .children(actions)
                    .child(column)
                    .child(content),
            )
            .into_any_element()
    }
}

fn reaction(r: &ReactionDto, known: &HashMap<String, horo_core::matrix::packs::PackImageDto>, palette: &Palette, cx: &mut App) -> AnyElement {
    // A custom emoji reacts with its mxc URI or a shortcode known from a pack.
    let key: AnyElement = match packs::reaction_image(&r.key, known) {
        Some((url, _)) => match Media::image(&url, Some((40, 40)), cx) {
            Some(image) => img(image).size(zpx(16.)).object_fit(gpui::ObjectFit::Contain).into_any_element(),
            None => div().size(zpx(16.)).into_any_element(),
        },
        None => div().text_size(zpx(13.)).child(r.key.clone()).into_any_element(),
    };
    h_flex()
        .gap(space(6.))
        .px(space(10.))
        .py(space(3.))
        .rounded_full()
        .border_1()
        .font_family(palette.mono_font.clone())
        .text_size(text(11.5))
        .when(r.pending, |d| d.opacity(0.55))
        .map(|d| {
            if r.mine {
                d.bg(gpui::Hsla { a: 0.16, ..palette.accent }).border_color(palette.accent).text_color(palette.accent)
            } else {
                d.bg(palette.raised).border_color(palette.border_subtle).text_color(palette.text_secondary)
            }
        })
        .child(key)
        .child(div().font_weight(FontWeight::BOLD).child(r.count.to_string()))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Seen, clock, empty_divider, wave_heights};

    #[test]
    fn dividers_over_nothing_drawn_are_dropped() {
        use Seen::*;
        let rows = [Day, Shown, Day, Hidden, Marker, Day, Hidden, Day, Day, Shown];
        let empty = |ix: usize| empty_divider(rows[ix..].iter().copied());
        // A day with a message keeps its divider.
        assert!(!empty(0));
        // A day of only call bookkeeping (and the marker) loses it…
        assert!(empty(2));
        assert!(empty(5));
        assert!(empty(7));
        // …and the last day, with a message, keeps it.
        assert!(!empty(8));
        // "new" stays while anything drawn comes after it.
        assert!(!empty(4));
        assert!(empty_divider([Marker, Hidden, Day, Hidden].into_iter()));
        // Messages aren't dividers.
        assert!(!empty(1));
    }

    #[test]
    fn times_read_like_a_player() {
        assert_eq!(clock(0.), "0:00");
        assert_eq!(clock(65.4), "1:05");
        assert_eq!(clock(3725.), "1:02:05");
        assert_eq!(clock(-3.), "0:00");
    }

    #[test]
    fn waveforms_are_squeezed_to_their_loudest() {
        assert_eq!(wave_heights(&[0, 1024, 512, 256], 2), vec![1.0, 0.5]);
        assert_eq!(wave_heights(&[1024, 0], 8), vec![1.0, 0.08]);
        assert!(wave_heights(&[], 8).is_empty());
        assert_eq!(wave_heights(&vec![2048; 1000], 48).len(), 48);
    }
}
