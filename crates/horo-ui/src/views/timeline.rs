//! A room's timeline, read-only for now: messages, day dividers, the read
//! marker, newest at the bottom, older pages fetched as you scroll up.
//!
//! Items come from horo-core's timeline (`timeline::open` for the current
//! ones, then `Event::Timeline` diffs through the [`Store`]); gpui's `list`
//! renders only what's on screen and follows the same diffs as splices, so
//! a new message doesn't re-lay-out the whole history.

use std::rc::Rc;

use chrono::{DateTime, Local, TimeZone};
use gpui::{
    AnyElement, App, Context, Entity, FontWeight, IntoElement, ListAlignment, ListState, ParentElement, Render, SharedString, Styled,
    WeakEntity, Window, div, list, prelude::FluentBuilder, px,
};
use gpui_component::{h_flex, v_flex};
use horo_core::{
    dto::{ContentDto, Diff, EventItemDto, ReactionDto, TimelineItemDto, TimelineRowKind, TimelineUpdate},
    matrix::timeline,
};
use matrix_sdk::ruma::OwnedRoomId;

use crate::{
    backend::Backend,
    store::{Store, apply_diff},
    theme::Palette,
    views::avatar::monogram,
};

/// Messages from one sender this close together share a header.
const GROUP_WINDOW_MS: u64 = 5 * 60 * 1000;
/// Fetch older history when this close to the top.
const PAGINATE_WITHIN: usize = 8;

pub struct Timeline {
    room_id: OwnedRoomId,
    items: Vec<TimelineItemDto>,
    list: ListState,
    /// The initial items are in; until then diffs wait in `early`.
    loaded: bool,
    early: Vec<Vec<Diff<TimelineItemDto>>>,
    paginating: bool,
    at_start: bool,
    error: Option<SharedString>,
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
            items: Vec::new(),
            list,
            loaded: false,
            early: Vec::new(),
            paginating: false,
            at_start: false,
            error: None,
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
        cx.notify();
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
                    this.read_with(cx, |timeline, _| timeline.render_item(ix, &palette_for_rows))
                        .unwrap_or_else(|_| div().into_any_element())
                })
                .size_full(),
            )
            .into_any_element()
    }
}

impl Timeline {
    fn render_item(&self, ix: usize, palette: &Palette) -> AnyElement {
        let Some(item) = self.items.get(ix) else { return div().into_any_element() };
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
                    message(event, continues, palette)
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

/// What a message body says, as text, until rich text and media land.
fn body_text(content: &ContentDto) -> (String, bool) {
    match content {
        ContentDto::Text { body, .. } | ContentDto::Notice { body, .. } => (body.clone(), false),
        ContentDto::Emote { body, .. } => (format!("* {body}"), false),
        ContentDto::Image { body, .. } => (format!("🖼  {body}"), true),
        ContentDto::Video { body, .. } => (format!("🎞  {body}"), true),
        ContentDto::Audio { body, media } if media.is_voice => ("🎙  voice message".into(), true),
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

fn message(event: &EventItemDto, continues: bool, palette: &Palette) -> AnyElement {
    let name = event.sender_name.clone().unwrap_or_else(|| event.sender.clone());
    let (text, muted) = body_text(&event.content);

    if is_system(&event.content) {
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

    let body = div()
        .w_full()
        .text_color(if muted { palette.text_tertiary } else { palette.text })
        .when(muted, |d| d.italic())
        .when(event.is_emoji_only, |d| d.text_size(px(32.)))
        .child(text)
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

    let reactions = (!event.reactions.is_empty()).then(|| h_flex().flex_wrap().gap_1().pt_1().children(event.reactions.iter().map(|r| reaction(r, palette))));

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

    h_flex()
        .w_full()
        .items_start()
        .px_4()
        .pt(if continues { px(2.) } else { px(10.) })
        .gap_3()
        .when(event.is_highlighted, |row| row.bg(gpui::Hsla { a: 0.08, ..palette.accent }))
        .child(if continues {
            div().w(px(36.)).flex_none()
        } else {
            monogram(&event.sender, &name, px(36.), false, palette.text_on_accent)
        })
        .child(content)
        .into_any_element()
}

fn reaction(r: &ReactionDto, palette: &Palette) -> impl IntoElement {
    h_flex()
        .gap_1()
        .px(px(6.))
        .py(px(1.))
        .rounded_full()
        .border_1()
        .border_color(if r.mine { palette.accent } else { palette.border })
        .bg(if r.mine { gpui::Hsla { a: 0.15, ..palette.accent } } else { palette.raised })
        .text_xs()
        .child(r.key.clone())
        .child(div().text_color(palette.text_secondary).child(r.count.to_string()))
}
