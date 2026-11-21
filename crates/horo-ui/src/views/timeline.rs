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
    AnyElement, App, Context, Entity, FontWeight, IntoElement, ListAlignment, ListState, ParentElement, Render, SharedString, Styled,
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
    rich::{self, Block},
    store::{Store, apply_diff},
    theme::Palette,
    views::{
        avatar::avatar,
        content::{self, Open},
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
}

impl Timeline {
    pub fn new(room_id: OwnedRoomId, store: &Entity<Store>, cx: &mut Context<Self>) -> Timeline {
        let list = ListState::new(0, ListAlignment::Bottom, px(400.));
        let this = cx.weak_entity();
        list.set_scroll_handler(move |event, _, cx| {
            if event.visible_range.start < PAGINATE_WITHIN {
                let _ = this.update(cx, |timeline, cx| timeline.paginate(cx));
            }
        });
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
        };

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

impl Render for Timeline {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Rc::new(cx.global::<Palette>().clone());
        if let Some(error) = &self.error {
            return div().m_auto().text_color(palette.danger).child(error.clone()).into_any_element();
        }
        if !self.loaded {
            return div().m_auto().text_color(palette.text_tertiary).child("loading…").into_any_element();
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
                .size_full(),
            )
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
        _ => when.format("%A, %-d %B %Y").to_string().to_lowercase(),
    }
}

fn time_label(ms: u64) -> String {
    local(ms).map(|t| t.format("%H:%M").to_string()).unwrap_or_default()
}

fn divider(label: String, text: gpui::Hsla, line: gpui::Hsla) -> AnyElement {
    h_flex()
        .px_4()
        .py_2()
        .gap_3()
        .child(div().flex_1().h(px(1.)).bg(line))
        .child(div().text_xs().text_color(text).child(label))
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
                content::rich(&id, &blocks, colour, palette)
            }
            ContentDto::Emote { formatted: Some(html), .. } => {
                let blocks = self.blocks(&format!("* {html}"));
                div().italic().child(content::rich(&id, &blocks, palette.text, palette)).into_any_element()
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

    fn message(&mut self, event: &EventItemDto, continues: bool, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let name = event.sender_name.clone().unwrap_or_else(|| event.sender.clone());

        if is_system(&event.content) {
            let (text, _) = body_text(&event.content);
            return h_flex()
                .w_full()
                .px_4()
                .py(px(3.))
                .gap_2()
                .text_xs()
                .text_color(palette.text_tertiary)
                .child(div().w(px(36.)).flex_none())
                .child(div().flex_1().min_w_0().child(text))
                .child(div().flex_none().child(time_label(event.timestamp)))
                .into_any_element();
        }

        let body = v_flex()
            .w_full()
            .child(self.body(event, palette, cx))
            .when(event.is_edited, |d| d.child(div().text_xs().text_color(palette.text_tertiary).child("(edited)")));

        let reply = event.reply.as_ref().map(|reply| {
            let who = reply.sender_name.clone().or_else(|| reply.sender.clone()).unwrap_or_default();
            h_flex()
                .gap_1()
                .pl_2()
                .border_l_2()
                .border_color(palette.border_strong)
                .text_xs()
                .text_color(palette.text_tertiary)
                .child(div().font_weight(FontWeight::SEMIBOLD).child(who))
                .child(div().truncate().child(reply.body.clone().unwrap_or_else(|| "…".into()).replace('\n', " ")))
        });

        let reactions = (!event.reactions.is_empty()).then(|| {
            let chips: Vec<AnyElement> = event.reactions.iter().map(|r| reaction(r, palette, cx)).collect();
            h_flex().flex_wrap().gap_1().pt_1().children(chips)
        });

        let thread = event.thread_summary.as_ref().map(|t| {
            div()
                .pt_1()
                .text_xs()
                .text_color(palette.accent)
                .child(format!("{} {} in thread", t.num_replies, if t.num_replies == 1 { "reply" } else { "replies" }))
        });

        let sending = event.send_state.as_ref().is_some_and(|s| !matches!(s, horo_core::dto::SendStateDto::Sent { .. }));

        let content = v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(2.))
            .when(!continues, |c| {
                c.child(
                    h_flex()
                        .gap_2()
                        .child(div().font_weight(FontWeight::SEMIBOLD).text_color(palette.text).child(name.clone()))
                        .child(div().text_xs().text_color(palette.text_tertiary).child(time_label(event.timestamp))),
                )
            })
            .children(reply)
            .child(body)
            .children(reactions)
            .children(thread)
            .when(sending, |c| c.opacity(0.6));

        let face = if continues {
            div().w(px(36.)).flex_none().into_any_element()
        } else {
            avatar(&event.sender, &name, event.sender_avatar.as_deref(), px(36.), false, palette.text_on_accent, cx)
        };

        h_flex()
            .w_full()
            .items_start()
            .px_4()
            .pt(if continues { px(2.) } else { px(10.) })
            .gap_3()
            .when(event.is_highlighted, |row| row.bg(gpui::Hsla { a: 0.08, ..palette.accent }))
            .child(face)
            .child(content)
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
        div().child(r.key.clone()).into_any_element()
    };
    h_flex()
        .gap_1()
        .px(px(6.))
        .py(px(1.))
        .rounded_full()
        .border_1()
        .border_color(if r.mine { palette.accent } else { palette.border })
        .bg(if r.mine { gpui::Hsla { a: 0.15, ..palette.accent } } else { palette.raised })
        .text_xs()
        .child(key)
        .child(div().text_color(palette.text_secondary).child(r.count.to_string()))
        .into_any_element()
}
