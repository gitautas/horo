//! A room's timeline, read-only for now: messages, day dividers, the read
//! marker, newest at the bottom, older pages fetched as you scroll up.
//!
//! Items come from horo-core's timeline (`timeline::open` for the current
//! ones, then `Event::Timeline` diffs through the [`Store`]); gpui's `list`
//! renders only what's on screen and follows the same diffs as splices, so
//! a new message doesn't re-lay-out the whole history.

use std::{collections::HashMap, rc::Rc};

use chrono::{DateTime, Local, TimeZone};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, Hsla, EventEmitter, InteractiveElement, StatefulInteractiveElement, FontWeight, IntoElement, ListAlignment, ListState, ParentElement, Render, SharedString, Styled,
    WeakEntity, Window, div, img, list, prelude::FluentBuilder, px, StyledImage,
};
use gpui_component::{h_flex, v_flex};
use horo_core::{
    dto::{ContentDto, Diff, EventItemDto, ReactionDto, TimelineItemDto, TimelineRowKind, TimelineUpdate},
    matrix::timeline,
};
use matrix_sdk::ruma::OwnedRoomId;

use crate::{
    backend::Backend,
    media::Media,
    packs,
    rich::{self, Block},
    store::{Store, apply_diff},
    theme::{Palette, corner, space, text},
    views::{
        avatar::{accent_for, avatar},
        content::{self, Open},
        emoji_picker::{EmojiPicker, PickerEvent},
        voice::icon,
    },
};

/// Messages from one sender this close together share a header.
const GROUP_WINDOW_MS: u64 = 5 * 60 * 1000;
/// Fetch older history when this close to the top.
const PAGINATE_WITHIN: usize = 8;

pub struct Timeline {
    room_id: OwnedRoomId,
    store: Entity<Store>,
    /// Parsed `formatted_body`s, by their HTML.
    rich: HashMap<String, Rc<Vec<Block>>>,
    items: Vec<TimelineItemDto>,
    list: ListState,
    /// The initial items are in; until then diffs wait in `early`.
    loaded: bool,
    early: Vec<Vec<Diff<TimelineItemDto>>>,
    paginating: bool,
    at_start: bool,
    error: Option<SharedString>,
    /// A read marker is due to go out shortly.
    marking_read: bool,
    /// The message whose delete button is asking "sure?".
    confirm_delete: Option<String>,
    /// The last action on a message that failed.
    notice: Option<SharedString>,
    /// The emoji picker, open for reacting to a message: which, where.
    picker: Option<(String, gpui::Point<gpui::Pixels>, Entity<EmojiPicker>)>,
}

/// What the timeline asks of the composer under it.
#[derive(Debug, Clone)]
pub enum TimelineAction {
    Reply { event_id: String, who: String, preview: String },
    Edit { event_id: String, body: String },
}

impl EventEmitter<TimelineAction> for Timeline {}

impl Timeline {
    pub fn new(room_id: OwnedRoomId, store: &Entity<Store>, cx: &mut Context<Self>) -> Timeline {
        let list = ListState::new(0, ListAlignment::Bottom, px(400.));
        let this = cx.weak_entity();
        list.set_scroll_handler(move |event, _, cx| {
            if event.visible_range.start < PAGINATE_WITHIN {
                let _ = this.update(cx, |timeline, cx| timeline.paginate(cx));
            }
        });
        // Presence dots change with the store.
        cx.observe(store, |_, _, cx| cx.notify()).detach();
        cx.subscribe(store, |this, _, update: &TimelineUpdate, cx| {
            if update.room_id == this.room_id.as_str() {
                this.on_update(update.diffs.clone(), cx);
            }
        })
        .detach();

        let timeline = Timeline {
            room_id: room_id.clone(),
            store: store.clone(),
            rich: HashMap::new(),
            items: Vec::new(),
            list,
            loaded: false,
            early: Vec::new(),
            paginating: false,
            at_start: false,
            error: None,
            marking_read: false,
            confirm_delete: None,
            notice: None, picker: None };

        let backend = Backend::global(cx);
        let task = backend.run({
            let (host, state) = (backend.host.clone(), backend.state.clone());
            async move { timeline::open(&host, &state.core().await?, &room_id, None).await }
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
        if self.items.len() < 30 {
            self.paginate(cx);
        }
        self.mark_read(cx);
        self.report_people(cx);
        cx.notify();
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
                self.list.splice(range, count);
                if range_after(end, self.items.len()) {
                    self.list.remeasure_items(end - 1..end);
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
            let (state, room_id) = (backend.state.clone(), self.room_id.clone());
            async move { timeline::paginate_back(&*state.core().await?, &room_id, None).await }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |timeline, cx| {
                timeline.paginating = false;
                match outcome {
                    Ok(Ok(at_start)) => timeline.at_start = at_start,
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
        let (state, room_id) = (backend.state.clone(), self.room_id.clone());
        let task = backend.run(async move {
            if let Ok(core) = state.core().await {
                let _ = timeline::close(&core, &room_id, None).await;
            }
        });
        drop(task);
    }
}

fn range_after(end: usize, len: usize) -> bool {
    end > 0 && end <= len
}

impl Timeline {
    /// The bar that shows over a message on hover: reply, edit, delete.
    fn actions(&self, event: &EventItemDto, name: &str, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = event.event_id.clone()?;
        let confirming = self.confirm_delete.as_deref() == Some(id.as_str());
        let button = |key: &str, label: &str, colour| {
            div()
                .id(SharedString::from(format!("{key}/{id}")))
                .px_2()
                .py(px(2.))
                .rounded(palette.radius_sm)
                .text_xs()
                .text_color(colour)
                .cursor_pointer()
                .hover(|b| b.bg(palette.inset))
                .child(label.to_owned())
        };
        let mut bar = h_flex()
            .absolute()
            .top(px(-8.))
            .right_4()
            .p(px(2.))
            .gap(px(2.))
            .rounded(palette.radius_md)
            .bg(palette.raised)
            .border_1()
            .border_color(palette.border)
            .shadow_sm();
        bar = if confirming { bar.visible() } else { bar.invisible().group_hover("message", |s| s.visible()) };
        let reactable = !matches!(event.content, ContentDto::Redacted);
        let any = reactable || event.can_reply || (event.is_editable && editable_body(&event.content).is_some()) || event.is_own;
        if !any {
            return None;
        }
        // Your recent reactions, one click each, then the picker for more.
        if reactable && !confirming {
            let (recent, known) = {
                let store = self.store.read(cx);
                (store.prefs.recent_reactions.clone(), packs::image_lookup(&store.packs))
            };
            for (i, key) in recent.into_iter().enumerate() {
                // A custom emote is remembered by its address (or a
                // shortcode another client used): drawn as its picture.
                let picture = packs::reaction_image(&key, &known);
                let name = picture.as_ref().map(|(_, name)| name.clone()).unwrap_or_else(|| key.clone());
                let face = match picture.and_then(|(url, _)| Media::image(&url, Some((52, 52)), cx)) {
                    Some(image) => img(image).size(px(18.)).object_fit(gpui::ObjectFit::Contain).into_any_element(),
                    None => div().child(key.clone()).into_any_element(),
                };
                let (event_id, tip) = (id.clone(), format!("react with {name}"));
                bar = bar.child(
                    div()
                        .id(SharedString::from(format!("quick-{i}/{id}")))
                        .size(px(26.))
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
            let event_id = id.clone();
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("more-reactions/{id}")))
                    .size(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(corner(9.))
                    .cursor_pointer()
                    .hover(|b| b.bg(Hsla { a: 0.08, ..gpui::white() }))
                    .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("more reactions").build(window, cx))
                    .child(icon("icons/smile.svg").size(px(15.)).text_color(palette.text_secondary))
                    .on_click(cx.listener(move |this, click: &gpui::ClickEvent, window, cx| this.open_picker(event_id.clone(), click.position(), window, cx))),
            );
        }

        if event.can_reply && !confirming {
            let (event_id, who) = (id.clone(), name.to_owned());
            let preview = reply_preview(&event.content);
            bar = bar.child(button("reply", "reply", palette.text_secondary).on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(TimelineAction::Reply { event_id: event_id.clone(), who: who.clone(), preview: preview.clone() })
            })));
        }
        if event.is_editable && !confirming
            && let Some(body) = editable_body(&event.content)
        {
            let event_id = id.clone();
            bar = bar.child(button("edit", "edit", palette.text_secondary).on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(TimelineAction::Edit { event_id: event_id.clone(), body: body.clone() })
            })));
        }
        if event.is_own && !matches!(event.content, ContentDto::Redacted) {
            if confirming {
                let event_id = id.clone();
                bar = bar
                    .child(div().px_2().text_xs().text_color(palette.text_secondary).child("delete it?"))
                    .child(button("delete-yes", "delete", palette.danger).on_click(cx.listener(move |this, _, _, cx| this.redact(event_id.clone(), cx))))
                    .child(button("delete-no", "keep", palette.text_secondary).on_click(cx.listener(|this, _, _, cx| {
                        this.confirm_delete = None;
                        cx.notify();
                    })));
            } else {
                let event_id = id.clone();
                bar = bar.child(button("delete", "delete", palette.danger).on_click(cx.listener(move |this, _, _, cx| {
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
            let (state, room_id) = (backend.state.clone(), self.room_id.clone());
            async move { timeline::toggle_reaction(&*state.core().await?, &room_id, None, &event_id, &key).await }
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
            let (state, room_id) = (backend.state.clone(), self.room_id.clone());
            async move { timeline::redact(&*state.core().await?, &room_id, None, &event_id, None).await }
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

    /// The newest message of ours that can still be edited, for the up arrow.
    pub fn last_editable(&self) -> Option<(String, String)> {
        self.items.iter().rev().filter_map(|i| i.event.as_deref()).find_map(|e| {
            if !e.is_editable {
                return None;
            }
            Some((e.event_id.clone()?, editable_body(&e.content)?))
        })
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Rc::new(cx.global::<Palette>().clone());
        if let Some(error) = &self.error {
            return div().size_full().flex().items_center().justify_center().text_color(palette.danger).child(error.clone()).into_any_element();
        }
        if !self.loaded {
            return div().size_full().flex().items_center().justify_center().text_color(palette.text_tertiary).child("loading…").into_any_element();
        }
        let this: WeakEntity<Self> = cx.weak_entity();
        let palette_for_rows = palette.clone();
        v_flex()
            .size_full()
            .child(
                list(self.list.clone(), move |ix, _, cx| {
                    this.update(cx, |timeline, cx| timeline.render_item(ix, &palette_for_rows, cx))
                        .unwrap_or_else(|_| div().into_any_element())
                })
                .flex_1()
                .min_h_0()
                .w_full(),
            )
            .children(self.picker.as_ref().map(|(_, at, picker)| {
                gpui::deferred(
                    gpui::anchored()
                        .position(*at)
                        .anchor(gpui::Anchor::TopRight)
                        .snap_to_window_with_margin(px(10.))
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
    fn render_item(&mut self, ix: usize, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.items.get(ix).cloned() else { return div().into_any_element() };
        let row = match item.kind {
            TimelineRowKind::DateDivider => divider(item.timestamp.map(day_label).unwrap_or_default(), palette.text_tertiary, palette.border_subtle),
            TimelineRowKind::ReadMarker => divider("new".into(), palette.accent_secondary, palette.accent_secondary),
            TimelineRowKind::TimelineStart => div()
                .py_6()
                .flex()
                .justify_center()
                .text_sm()
                .text_color(palette.text_tertiary)
                .child("the start of the room")
                .into_any_element(),
            TimelineRowKind::Event => match &item.event {
                Some(event) => {
                    let continues = self.items[..ix]
                        .iter()
                        .rev()
                        .find(|i| i.kind != TimelineRowKind::ReadMarker)
                        .and_then(|prev| prev.event.as_deref())
                        .is_some_and(|prev| continues_group(prev, event));
                    self.message(event, continues, palette, cx)
                }
                None => div().into_any_element(),
            },
        };
        // The top row says what's going on above it.
        if ix == 0 && !self.at_start {
            return v_flex()
                .child(div().py_2().flex().justify_center().text_xs().text_color(palette.text_tertiary).child(if self.paginating {
                    "loading earlier messages…"
                } else {
                    "scroll for earlier messages"
                }))
                .child(row)
                .into_any_element();
        }
        row
    }
}

/// Same sender, close in time, both ordinary messages: no second header.
fn continues_group(prev: &EventItemDto, event: &EventItemDto) -> bool {
    prev.sender == event.sender
        && event.timestamp.saturating_sub(prev.timestamp) < GROUP_WINDOW_MS
        && !is_system(&prev.content)
        && !is_system(&event.content)
}

/// Membership, profile and state changes: a quiet line, not a message.
fn is_system(content: &ContentDto) -> bool {
    matches!(content, ContentDto::Membership { .. } | ContentDto::ProfileChange { .. } | ContentDto::State { .. } | ContentDto::CallInvite)
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
        .pt(px(18.))
        .pb(px(10.))
        .gap(space(12.))
        .child(div().flex_1().h(px(1.)).bg(line))
        .child(div().text_size(text(11.)).font_weight(FontWeight::SEMIBOLD).text_color(colour).child(label))
        .child(div().flex_1().h(px(1.)).bg(line))
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

    /// The message's body: formatted text, a picture, a card, or a line.
    fn body(&mut self, event: &EventItemDto, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = event_key(event);
        let store = self.store.clone();
        match &event.content {
            ContentDto::Text { formatted: Some(html), .. } | ContentDto::Notice { formatted: Some(html), .. } if !event.is_emoji_only => {
                let colour = if matches!(event.content, ContentDto::Notice { .. }) { palette.text_secondary } else { palette.text };
                let blocks = self.blocks(html);
                content::rich(&id, &blocks, colour, palette, cx)
            }
            ContentDto::Emote { formatted: Some(html), .. } => {
                let blocks = self.blocks(&format!("* {html}"));
                div().italic().child(content::rich(&id, &blocks, palette.text, palette, cx)).into_any_element()
            }
            ContentDto::Image { body, media } => content::picture(&id, body, media, false, &store, palette, cx),
            ContentDto::Sticker { body, media } => content::picture(&id, body, media, true, &store, palette, cx),
            ContentDto::Video { body, media } => content::card(&id, "🎞", body, media, Open::Play, palette, cx),
            ContentDto::Audio { media, .. } if media.is_voice => content::card(&id, "🎙", "voice message", media, Open::Play, palette, cx),
            ContentDto::Audio { body, media } => content::card(&id, "🎵", body, media, Open::Play, palette, cx),
            ContentDto::File { body, media } => content::card(&id, "📎", body, media, Open::Save, palette, cx),
            content => {
                let (text, muted) = body_text(content);
                div()
                    .w_full()
                    .text_color(if muted { palette.text_tertiary } else { palette.text })
                    .when(muted, |d| d.italic())
                    .when(event.is_emoji_only, |d| d.text_size(px(32.)))
                    .child(text)
                    .into_any_element()
            }
        }
    }

    /// One message, as MessageRow.tsx lays it out: a 40px column with the
    /// face (or, continuing a group, the time on hover), then the sender in
    /// their accent, the time and any device warning, the reply, the body,
    /// reactions and the thread.
    fn message(&mut self, event: &EventItemDto, continues: bool, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let name = event.sender_name.clone().unwrap_or_else(|| event.sender.trim_start_matches('@').split(':').next().unwrap_or_default().to_owned());

        if is_system(&event.content) {
            let (line, _) = body_text(&event.content);
            return h_flex()
                .w_full()
                .px(space(22.))
                .child(
                    h_flex()
                        .w_full()
                        .gap(space(10.))
                        .py(space(6.))
                        .pl(px(40.) + space(12.) + space(8.))
                        .text_size(text(12.))
                        .text_color(palette.text_tertiary)
                        .child(div().flex_1().min_w_0().child(line))
                        .child(div().flex_none().font_family(palette.mono_font.clone()).text_size(text(10.5)).child(time_label(event.timestamp))),
                )
                .into_any_element();
        }

        let body = v_flex()
            .w_full()
            .max_w(px(640.))
            .text_size(px(14.5))
            .line_height(gpui::relative(1.55))
            .child(self.body(event, palette, cx))
            .when(event.is_edited, |d| d.child(div().text_size(text(10.)).text_color(palette.text_tertiary).child("(edited)")));

        let reply = event.reply.as_ref().map(|reply| {
            let who = reply.sender_name.clone().or_else(|| reply.sender.clone()).unwrap_or_default();
            h_flex()
                .mb(px(5.))
                .gap(space(8.))
                .px(space(11.))
                .py(space(5.))
                .max_w(px(520.))
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
            let chips: Vec<AnyElement> = event
                .reactions
                .iter()
                .map(|r| {
                    let chip = reaction(r, palette, cx);
                    match &event.event_id {
                        Some(id) => {
                            let (id, key) = (id.clone(), r.key.clone());
                            div()
                                .id(SharedString::from(format!("react/{id}/{key}")))
                                .cursor_pointer()
                                .child(chip)
                                .on_click(cx.listener(move |this, _, _, cx| this.toggle_reaction(id.clone(), key.clone(), cx)))
                                .into_any_element()
                        }
                        None => chip,
                    }
                })
                .collect();
            h_flex().flex_wrap().gap(space(6.)).mt(px(7.)).children(chips)
        });

        let thread = event.thread_summary.as_ref().map(|t| {
            h_flex()
                .mt(px(8.))
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
                .child(format!("{} {} in thread", t.num_replies, if t.num_replies == 1 { "reply" } else { "replies" }))
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
                        .size(px(12.))
                        .text_color(if shield.colour == horo_core::dto::ShieldColour::Red { palette.danger } else { palette.text_tertiary }),
                )
        });

        let header = (!continues).then(|| {
            let (sender, hint) = (event.sender.clone(), event.sender_avatar.clone());
            h_flex()
                .mb(px(3.))
                .gap(space(8.))
                .flex_wrap()
                .child(
                    div()
                        .id(SharedString::from(format!("name/{}", event_key(event))))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_size(text(14.5))
                        .text_color(accent_for(&event.sender))
                        .cursor_pointer()
                        .hover(|d| d.underline())
                        .child(name.clone())
                        .on_click(cx.listener(move |this, click: &gpui::ClickEvent, _, cx| this.open_profile(sender.clone(), click.position(), hint.clone(), cx))),
                )
                .child(div().font_family(palette.mono_font.clone()).text_size(text(10.5)).text_color(palette.text_tertiary).child(time_label(event.timestamp)))
                .children(shield)
        });

        let content = v_flex()
            .flex_1()
            .min_w_0()
            .children(header)
            .children(reply)
            .child(body)
            .children(reactions)
            .children(thread)
            .when(failed, |c| {
                c.child(div().mt(px(6.)).text_size(text(11.5)).text_color(palette.danger).child("couldn't send — it'll retry when you're back online"))
            });

        let column = if continues {
            // The time, quietly, when the pointer is on the row.
            div()
                .w(px(40.))
                .flex_none()
                .flex()
                .justify_center()
                .pt(space(4.))
                .font_family(palette.mono_font.clone())
                .text_size(text(9.5))
                .text_color(palette.text_tertiary)
                .invisible()
                .group_hover("message", |s| s.visible())
                .child(time_label(event.timestamp))
                .into_any_element()
        } else {
            let picture = avatar(&event.sender, &name, event.sender_avatar.as_deref(), px(38.), cx);
            let (sender, hint) = (event.sender.clone(), event.sender_avatar.clone());
            div()
                .id(SharedString::from(format!("face/{}", event_key(event))))
                .w(px(40.))
                .flex_none()
                .flex()
                .justify_center()
                .pt(space(2.))
                .cursor_pointer()
                .child(picture)
                .on_click(cx.listener(move |this, click: &gpui::ClickEvent, _, cx| this.open_profile(sender.clone(), click.position(), hint.clone(), cx)))
                .into_any_element()
        };

        let actions = self.actions(event, &name, palette, cx);

        h_flex()
            .w_full()
            .px(space(22.))
            .pt(space(3.))
            .child(
                h_flex()
                    .group("message")
                    .relative()
                    .w_full()
                    .items_start()
                    .gap(space(12.))
                    .px(space(8.))
                    .py(space(5.))
                    .rounded(corner(16.))
                    .when(!event.is_highlighted, |row| row.hover(|row| row.bg(gpui::Hsla { a: 0.035, ..gpui::white() })))
                    .when(event.is_highlighted, |row| row.bg(gpui::Hsla { a: 0.08, ..palette.accent_secondary }))
                    .when(pending, |row| row.opacity(0.72))
                    .children(actions)
                    .child(column)
                    .child(content),
            )
            .into_any_element()
    }
}

fn reaction(r: &ReactionDto, palette: &Palette, cx: &mut App) -> AnyElement {
    // A custom emoji reacts with its picture's mxc URI as the key.
    let key: AnyElement = if r.key.starts_with("mxc://") {
        match Media::image(&r.key, Some((40, 40)), cx) {
            Some(image) => img(image).size(px(16.)).object_fit(gpui::ObjectFit::Contain).into_any_element(),
            None => div().size(px(16.)).into_any_element(),
        }
    } else {
        div().text_size(px(13.)).child(r.key.clone()).into_any_element()
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
