//! Presence as the UI shows it: the dot on an avatar and the "last seen"
//! line, as src/components/Presence.tsx and src/lib/display.ts draw them.
//!
//! Nothing is drawn without an answer (a homeserver with presence off, or
//! before the first poll lands): a grey "offline" dot for everyone would be
//! a confident lie, and no dot isn't. Pure, so it's tested here.

use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use horo_core::matrix::presence::{Availability, PresenceDto};

/// The states that get a dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    Online,
    Away,
    Offline,
}

impl Shown {
    pub fn word(self) -> &'static str {
        match self {
            Shown::Online => "online",
            Shown::Away => "away",
            Shown::Offline => "offline",
        }
    }
}

/// What to draw for someone, or nothing.
pub fn shown(presence: Option<&PresenceDto>, supported: bool) -> Option<Shown> {
    if !supported {
        return None;
    }
    match presence?.presence {
        Availability::Online => Some(Shown::Online),
        Availability::Unavailable => Some(Shown::Away),
        Availability::Offline => Some(Shown::Offline),
        Availability::Unknown => None,
    }
}

/// "just now", "12m ago", "3h ago", "2d ago", or a date a week on.
pub fn last_seen(last_active_ms: Option<u64>, now_ms: u64) -> String {
    let Some(last) = last_active_ms.filter(|ms| *ms > 0) else { return String::new() };
    let seconds = now_ms.saturating_sub(last) as f64 / 1000.0;
    if seconds < 90.0 {
        return "just now".into();
    }
    let minutes = (seconds / 60.0).round();
    if minutes < 60.0 {
        return format!("{minutes}m ago");
    }
    let hours = (minutes / 60.0).round();
    if hours < 24.0 {
        return format!("{hours}h ago");
    }
    let days = (hours / 24.0).round();
    if days < 7.0 {
        return format!("{days}d ago");
    }
    match Local.timestamp_millis_opt(last as i64).single() {
        Some(when) => format!("on {}", when.format("%-d %b %Y").to_string().to_lowercase()),
        None => String::new(),
    }
}

/// The line under a name: "online", or "away · last seen 12m ago".
pub fn line(presence: &PresenceDto, shown: Shown, now_ms: u64) -> String {
    let seen = if shown == Shown::Online { String::new() } else { last_seen(presence.last_active, now_ms) };
    if seen.is_empty() { shown.word().to_owned() } else { format!("{} · last seen {seen}", shown.word()) }
}

/// Our own presence: away after this long without touching the window.
pub const IDLE_AFTER: Duration = Duration::from_secs(5 * 60);

/// Online or away, from when the window was last touched.
pub fn idle(last_activity: Instant) -> bool {
    last_activity.elapsed() >= IDLE_AFTER
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dto(presence: Availability, last_active: Option<u64>) -> PresenceDto {
        PresenceDto { user_id: "@a:x".into(), presence, status_msg: None, last_active, currently_active: false }
    }

    #[test]
    fn no_answer_and_no_support_draw_nothing() {
        assert_eq!(shown(None, true), None);
        assert_eq!(shown(Some(&dto(Availability::Online, None)), false), None);
        assert_eq!(shown(Some(&dto(Availability::Unknown, None)), true), None);
        assert_eq!(shown(Some(&dto(Availability::Unavailable, None)), true), Some(Shown::Away));
    }

    #[test]
    fn last_seen_reads_naturally() {
        let now = 10_000_000_000;
        assert_eq!(last_seen(Some(now - 30_000), now), "just now");
        assert_eq!(last_seen(Some(now - 12 * 60_000), now), "12m ago");
        assert_eq!(last_seen(Some(now - 3 * 3_600_000), now), "3h ago");
        assert_eq!(last_seen(Some(now - 2 * 86_400_000), now), "2d ago");
        assert!(last_seen(Some(now - 30 * 86_400_000), now).starts_with("on "));
        assert_eq!(last_seen(None, now), "");
    }

    #[test]
    fn the_line_only_says_when_for_people_who_arent_here() {
        let now = 10_000_000_000;
        let away = dto(Availability::Unavailable, Some(now - 12 * 60_000));
        assert_eq!(line(&away, Shown::Away, now), "away · last seen 12m ago");
        let online = dto(Availability::Online, Some(now - 12 * 60_000));
        assert_eq!(line(&online, Shown::Online, now), "online");
    }
}
