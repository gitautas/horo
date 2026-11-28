//! What's worth interrupting you for, as src/lib/notify.ts decides it.
//!
//! The trigger is the room list, not the timelines: a room's notification
//! count comes from the homeserver, which has already run your push rules
//! (muted rooms, keywords, mentions only). [`Watch::inspect`] compares each
//! room with how it was last time and says which banners to show and which
//! sound to play. [`show`] puts a banner on screen; clicking one opens its
//! room ([`clicks`] says which).

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
    /// A DM call that just started: one person calling you, which rings
    /// until it's dealt with (a group call only chimes; forty ringing
    /// phones is how a room gets muted).
    pub ring: Option<String>,
}

/// Where the user is, for "don't tell me about the room I'm reading".
#[derive(Debug, Clone, Copy)]
pub struct Looking<'a> {
    pub room: Option<&'a str>,
    pub window_active: bool,
    /// The call we're in, which is no news to us.
    pub in_call: Option<&'a str>,
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

            let now_secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let locally_muted = prefs.room_notify_overrides.get(&room.id).is_some_and(|o| o.is_muted_now(now_secs));
            if prefs.notify_messages && !room.is_muted && !locally_muted {
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
            if prefs.notify_calls
                && !room.is_muted
                && !room.is_video_room
                && now.call
                && !before.call
                && looking.in_call != Some(room.id.as_str())
            {
                let body = if room.is_direct { "calling~" } else { "call starting~" };
                outcome.alerts.push(Alert { title: room.name.clone(), body: body.into(), room_id: room.id.clone() });
                if room.is_direct {
                    outcome.ring = Some(room.id.clone());
                } else {
                    ring = true;
                }
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

/// Rooms whose banner was clicked, for the window to open.
static CLICKED: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>> = std::sync::Mutex::new(None);

/// Hear which room's banner gets clicked. One listener: a second call
/// replaces the first.
pub fn clicks() -> tokio::sync::mpsc::UnboundedReceiver<String> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    if let Ok(mut clicked) = CLICKED.lock() {
        *clicked = Some(tx);
    }
    rx
}

fn clicked(room_id: String) {
    if let Some(tx) = CLICKED.lock().ok().and_then(|c| c.clone()) {
        let _ = tx.send(room_id);
    }
}

/// Set up banners. On macOS, inside an app bundle, they go through the
/// User Notifications framework, which says when one is clicked (the first
/// time, macOS asks whether horo may send them). Outside a bundle (a bare
/// `cargo run`) that framework won't start, so banners go the old way and
/// clicking one only brings the app forward.
pub fn init() {
    #[cfg(target_os = "macos")]
    {
        if !mac::start() {
            tracing::info!("not running from an app bundle; banners won't open their room");
        }
    }
}

/// Put a banner up.
pub fn show(alert: Alert) {
    #[cfg(target_os = "macos")]
    if mac::show(&alert) {
        return;
    }
    // Off the UI thread: the call can block.
    let spawned = std::thread::Builder::new().name("horo-notify".into()).spawn(move || {
        if let Err(e) = notify_rust::Notification::new().summary(&alert.title).body(&alert.body).show() {
            tracing::warn!("couldn't show a notification: {e}");
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("couldn't show a notification: {e}");
    }
}

/// The User Notifications side: a delegate that hears clicks, and banners
/// whose identifier carries their room.
#[cfg(target_os = "macos")]
mod mac {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    };

    use objc2::{AnyThread, define_class, msg_send, rc::Retained, runtime::{NSObject, NSObjectProtocol, ProtocolObject}};
    use objc2_foundation::{NSBundle, NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
        UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };

    use super::Alert;

    /// Between the room id and a counter in a banner's identifier (room ids
    /// don't contain one).
    const SEPARATOR: char = '|';

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "HoroNotificationDelegate"]
        #[ivars = ()]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            /// Clicked: open its room.
            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(&self, _center: &UNUserNotificationCenter, response: &UNNotificationResponse, done: &block2::DynBlock<dyn Fn()>) {
                let id = response.notification().request().identifier().to_string();
                if let Some((room, _)) = id.rsplit_once(SEPARATOR) {
                    super::clicked(room.to_owned());
                }
                done.call(());
            }

            /// Shown even while horo is in front: whether to interrupt was
            /// already decided (not for the room you're reading).
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(&self, _center: &UNUserNotificationCenter, _notification: &UNNotification, done: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>) {
                done.call((UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List,));
            }
        }
    );

    /// The delegate, kept alive (the center holds it weakly).
    struct Kept(#[allow(dead_code)] Retained<Delegate>);
    // SAFETY: only stored here so it lives; never touched again.
    unsafe impl Send for Kept {}
    static DELEGATE: Mutex<Option<Kept>> = Mutex::new(None);
    static STARTED: AtomicBool = AtomicBool::new(false);
    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// Become the notification center's delegate and ask to show banners.
    /// False outside an app bundle, where the framework can't be used.
    pub fn start() -> bool {
        // The framework throws without a bundle identifier.
        if NSBundle::mainBundle().bundleIdentifier().is_none() {
            return false;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate: Retained<Delegate> = {
            let this = Delegate::alloc().set_ivars(());
            // SAFETY: NSObject's init, on a freshly allocated object.
            unsafe { msg_send![super(this), init] }
        };
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        if let Ok(mut kept) = DELEGATE.lock() {
            *kept = Some(Kept(delegate));
        }
        let answered = block2::RcBlock::new(|granted: objc2::runtime::Bool, error: *mut NSError| {
            if !granted.as_bool() {
                // SAFETY: a non-null error the framework handed over.
                let why = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string()).unwrap_or_else(|| "turned off in system settings".into());
                tracing::info!("banners aren't allowed: {why}");
            }
        });
        // Our own sounds play: the banner only needs to show.
        center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert, &answered);
        STARTED.store(true, Ordering::Relaxed);
        true
    }

    /// Post `alert`. False if the framework isn't in use (show it the old
    /// way).
    pub fn show(alert: &Alert) -> bool {
        if !STARTED.load(Ordering::Relaxed) {
            return false;
        }
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&alert.title));
        content.setBody(&NSString::from_str(&alert.body));
        // One stack per room in notification centre.
        content.setThreadIdentifier(&NSString::from_str(&alert.room_id));
        let id = format!("{}{SEPARATOR}{}", alert.room_id, NEXT.fetch_add(1, Ordering::Relaxed));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(&id), &content, None);
        let posted = block2::RcBlock::new(|error: *mut NSError| {
            // SAFETY: a non-null error the framework handed over.
            if let Some(e) = unsafe { error.as_ref() } {
                tracing::warn!("couldn't show a notification: {}", e.localizedDescription());
            }
        });
        UNUserNotificationCenter::currentNotificationCenter().addNotificationRequest_withCompletionHandler(&request, Some(&posted));
        true
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

    const AWAY: Looking = Looking { room: None, window_active: false, in_call: None };

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
        let reading = Looking { room: Some("!a"), window_active: true, in_call: None };
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
        assert_eq!(outcome.ring, None);
    }

    #[test]
    fn a_dm_call_rings_and_our_own_call_is_no_news() {
        let prefs = Prefs::default();
        let mut watch = Watch::default();
        let mut dm = room("!d", 0);
        dm.is_direct = true;
        let ours = room("!o", 0);
        watch.inspect(&[dm.clone(), ours.clone()], &prefs, AWAY);

        dm.has_active_call = true;
        let mut ours = ours;
        ours.has_active_call = true;
        let in_ours = Looking { in_call: Some("!o"), ..AWAY };
        let outcome = watch.inspect(&[dm, ours], &prefs, in_ours);
        assert_eq!(outcome.ring, Some("!d".to_owned()));
        assert_eq!(outcome.alerts.len(), 1);
        assert_eq!(outcome.alerts[0].body, "calling~");
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
