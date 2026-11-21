//! What's worth interrupting you for, as src/lib/notify.ts decides it.
//!
//! The trigger is the room list, not the timelines: a room's notification
//! count comes from the homeserver, which has already run your push rules
//! (muted rooms, keywords, mentions only). [`Watch::inspect`] compares each
//! room with how it was last time and says which banners to show and which
//! sound to play. [`show`] puts a banner on screen.

use std::collections::HashMap;

use horo_core::dto::{RoomMembership, RoomSummary};

use crate::{prefs::Prefs, sounds::Kind};

/// What we knew about a room last time we looked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seen {
    count: u64,
    call: bool,
    invited: bool,
}

impl Seen {
    fn of(room: &RoomSummary) -> Seen {
        Seen { count: room.notification_count, call: room.has_active_call, invited: room.membership == RoomMembership::Invited }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub title: String,
    pub body: String,
    pub room_id: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub alerts: Vec<Alert>,
    /// One sound for the whole batch; a call outranks a message.
    pub sound: Option<Kind>,
}

/// Where the user is, for "don't tell me about the room I'm reading".
#[derive(Debug, Clone, Copy)]
pub struct Looking<'a> {
    pub room: Option<&'a str>,
    pub window_active: bool,
}

/// The longest body put in a banner.
const MAX_BODY: usize = 160;

#[derive(Default)]
pub struct Watch {
    /// `None` until the first list, which is the baseline: its unread
    /// counts are history, not news.
    seen: Option<HashMap<String, Seen>>,
}

impl Watch {
    pub fn reset(&mut self) {
        self.seen = None;
    }

    pub fn inspect(&mut self, rooms: &[RoomSummary], prefs: &Prefs, looking: Looking) -> Outcome {
        let Some(seen) = &mut self.seen else {
            self.seen = Some(rooms.iter().map(|r| (r.id.clone(), Seen::of(r))).collect());
            return Outcome::default();
        };
        let mut outcome = Outcome::default();
        let mut ring = false;
        let mut chirp = false;
        for room in rooms {
            let now = Seen::of(room);
            // A room we've only just heard of has nothing to compare with.
            let Some(before) = seen.insert(room.id.clone(), now) else { continue };

            if prefs.notify_messages && !room.is_muted {
                let reading = !prefs.notify_when_focused && looking.window_active && looking.room == Some(room.id.as_str());
                if now.count > before.count && !reading {
                    outcome.alerts.push(describe(room));
                    chirp = true;
                } else if now.invited && !before.invited {
                    outcome.alerts.push(Alert { title: room.name.clone(), body: "invited you~".into(), room_id: room.id.clone() });
                    chirp = true;
                }
            }

            // A call starting in a room you're in; a voice room's call is the
            // room itself, and people come and go from it all day.
            if prefs.notify_calls && !room.is_muted && !room.is_video_room && now.call && !before.call {
                let body = if room.is_direct { "calling~" } else { "call starting~" };
                outcome.alerts.push(Alert { title: room.name.clone(), body: body.into(), room_id: room.id.clone() });
                ring = true;
            }
        }
        outcome.sound = if ring {
            Some(Kind::Call)
        } else if chirp {
            Some(Kind::Message)
        } else {
            None
        };
        outcome
    }
}

/// In a DM the room is the person, so they aren't named twice.
fn describe(room: &RoomSummary) -> Alert {
    let (sender, body) = match &room.latest {
        Some(latest) => (latest.sender_name.clone().unwrap_or_else(|| latest.sender.clone()), truncate(&latest.body)),
        None => ("someone".into(), "new message".into()),
    };
    let body = if room.is_direct { body } else { format!("{sender}: {body}") };
    Alert { title: room.name.clone(), body, room_id: room.id.clone() }
}

fn truncate(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > MAX_BODY {
        format!("{}…", line.chars().take(MAX_BODY - 1).collect::<String>())
    } else {
        line
    }
}

/// Which app the banners come from on macOS: this bundle's identifier,
/// read from its Info.plist, so they carry horo's name and icon. Elsewhere
/// the system works it out.
pub fn init() {
    #[cfg(target_os = "macos")]
    {
        let id = std::env::current_exe()
            .ok()
            .and_then(|exe| std::fs::read_to_string(exe.parent()?.parent()?.join("Info.plist")).ok())
            .and_then(|plist| {
                let after = plist.split("<key>CFBundleIdentifier</key>").nth(1)?;
                let start = after.find("<string>")? + "<string>".len();
                let end = after[start..].find("</string>")? + start;
                Some(after[start..end].trim().to_owned())
            });
        match id {
            Some(id) => {
                if let Err(e) = notify_rust::set_application(&id) {
                    tracing::warn!("notifications won't carry horo's name: {e}");
                }
            }
            None => tracing::info!("not running from an app bundle; banners will name another app"),
        }
    }
}

/// Put a banner up, off the UI thread (macOS's call can block).
pub fn show(alert: Alert) {
    let spawned = std::thread::Builder::new().name("horo-notify".into()).spawn(move || {
        if let Err(e) = notify_rust::Notification::new().summary(&alert.title).body(&alert.body).show() {
            tracing::warn!("couldn't show a notification: {e}");
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("couldn't show a notification: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horo_core::dto::LatestEvent;

    fn room(id: &str, count: u64) -> RoomSummary {
        RoomSummary {
            id: id.into(),
            name: format!("room {id}"),
            topic: None,
            canonical_alias: None,
            avatar_url: None,
            is_direct: false,
            dm_user_id: None,
            is_encrypted: false,
            is_space: false,
            is_utility: false,
            is_muted: false,
            notify_mode: None,
            membership: RoomMembership::Joined,
            notification_count: count,
            highlight_count: 0,
            has_unread: count > 0,
            is_marked_unread: false,
            member_count: 2,
            latest: Some(LatestEvent { sender: "@bob:x".into(), sender_name: Some("bob".into()), body: "hi   there".into(), timestamp: 1 }),
            recency: 0,
            parent_spaces: Vec::new(),
            has_active_call: false,
            call_participants: Vec::new(),
            is_video_room: false,
        }
    }

    const AWAY: Looking = Looking { room: None, window_active: false };

    #[test]
    fn the_first_list_is_a_baseline_and_new_messages_after_it_notify() {
        let prefs = Prefs::default();
        let mut watch = Watch::default();
        assert_eq!(watch.inspect(&[room("!a", 5)], &prefs, AWAY), Outcome::default());
        let outcome = watch.inspect(&[room("!a", 6)], &prefs, AWAY);
        assert_eq!(outcome.alerts, vec![Alert { title: "room !a".into(), body: "bob: hi there".into(), room_id: "!a".into() }]);
        assert_eq!(outcome.sound, Some(Kind::Message));
        // Reading it down is not news.
        assert_eq!(watch.inspect(&[room("!a", 0)], &prefs, AWAY), Outcome::default());
    }

    #[test]
    fn muted_rooms_the_open_room_and_switched_off_messages_stay_quiet() {
        let mut watch = Watch::default();
        let prefs = Prefs::default();
        watch.inspect(&[room("!a", 0)], &prefs, AWAY);
        let reading = Looking { room: Some("!a"), window_active: true };
        assert!(watch.inspect(&[room("!a", 1)], &prefs, reading).alerts.is_empty());

        let mut muted = room("!a", 2);
        muted.is_muted = true;
        assert!(watch.inspect(&[muted], &prefs, AWAY).alerts.is_empty());

        let off = Prefs { notify_messages: false, ..Prefs::default() };
        assert!(watch.inspect(&[room("!a", 3)], &off, AWAY).alerts.is_empty());

        let always = Prefs { notify_when_focused: true, ..Prefs::default() };
        assert_eq!(watch.inspect(&[room("!a", 4)], &always, reading).alerts.len(), 1);
    }

    #[test]
    fn dms_dont_name_the_sender_twice_and_calls_ring_louder() {
        let prefs = Prefs::default();
        let mut watch = Watch::default();
        let mut dm = room("!d", 0);
        dm.is_direct = true;
        watch.inspect(&[dm.clone(), room("!g", 0)], &prefs, AWAY);

        dm.notification_count = 1;
        let mut group = room("!g", 0);
        group.has_active_call = true;
        let outcome = watch.inspect(&[dm, group], &prefs, AWAY);
        assert_eq!(outcome.alerts[0].body, "hi there");
        assert_eq!(outcome.alerts[1].body, "call starting~");
        assert_eq!(outcome.sound, Some(Kind::Call));
    }

    #[test]
    fn voice_rooms_dont_announce_their_calls_and_invites_do_notify() {
        let prefs = Prefs::default();
        let mut watch = Watch::default();
        let mut voice = room("!v", 0);
        voice.is_video_room = true;
        let mut invite = room("!i", 0);
        invite.membership = RoomMembership::Left;
        watch.inspect(&[voice.clone(), invite.clone()], &prefs, AWAY);

        voice.has_active_call = true;
        invite.membership = RoomMembership::Invited;
        let outcome = watch.inspect(&[voice, invite], &prefs, AWAY);
        assert_eq!(outcome.alerts, vec![Alert { title: "room !i".into(), body: "invited you~".into(), room_id: "!i".into() }]);
    }

    #[test]
    fn long_bodies_are_cut_with_an_ellipsis() {
        let long = "word ".repeat(100);
        let cut = truncate(&long);
        assert_eq!(cut.chars().count(), MAX_BODY);
        assert!(cut.ends_with('…'));
    }
}
