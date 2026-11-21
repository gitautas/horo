//! The app's state on gpui's side: who's signed in, the room list, the
//! spaces, the connection. One [`Store`] entity; views observe it.
//!
//! It is fed from two directions: answers to calls it makes through the
//! [`Backend`], and horo-core's [`Event`]s, which arrive on a channel and are
//! folded in on gpui's executor.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use gpui::{AppContext, Context, EventEmitter, Pixels, Point, SharedString};
use horo_core::{
    Event,
    dto::{Diff, RoomSummary, SessionInfo, SpaceSummary, SyncState},
    matrix::{
        auth,
        presence::{self as core_presence, OwnPresence, PresenceDto},
        rooms,
    },
    verification,
};

use crate::{
    alerts::{self, Looking, Watch},
    backend::Backend,
    prefs::Prefs,
    sounds::{self, Kind},
    verify::{self, Flow},
};

/// Where sign-in stands.
#[derive(Debug, Clone)]
pub enum Phase {
    /// Looking for a saved session.
    Restoring,
    SignedOut,
    SigningIn,
    SignedIn(SessionInfo),
}

pub struct Store {
    pub phase: Phase,
    /// The last sign-in failure, for the login screen.
    pub error: Option<SharedString>,
    pub rooms: Vec<RoomSummary>,
    /// The last diff batch folded into `rooms`; `None` until the snapshot.
    rooms_seq: Option<u64>,
    /// Batches that arrived before the snapshot.
    early: Vec<horo_core::dto::RoomsUpdate>,
    pub spaces: Vec<SpaceSummary>,
    spaces_fetched: Option<Instant>,
    /// A refresh is waiting out the throttle.
    spaces_pending: bool,
    pub sync: Option<SyncState>,
    /// The space open in the rail; `None` is home.
    pub space: Option<String>,
    pub room: Option<String>,
    /// This device's cross-signing trust as the core reports it:
    /// `verified`, `unverified` or `unknown`.
    pub device_trust: Option<String>,
    /// The verification the dialog shows.
    pub verification: Option<Flow>,
    /// The last verification action that failed.
    pub verification_error: Option<SharedString>,
    /// Settings, open at this section.
    pub settings: Option<SettingsSection>,
    /// The picture open full-size over everything.
    pub lightbox: Option<Lightbox>,
    /// The app's own settings, saved in the data directory.
    pub prefs: Prefs,
    /// What the room list looked like, to tell what's new.
    watch: Watch,
    /// The window has focus.
    window_active: bool,
    /// When each kind of sound last played: a burst of messages is one
    /// chirp, not ten.
    sounded: [Option<Instant>; 2],
    /// What the server says about the people we watch.
    pub presence: HashMap<String, PresenceDto>,
    /// The server answers presence at all.
    pub presence_supported: bool,
    /// Who the core is polling for us now.
    presence_watched: HashSet<String>,
    /// People on screen besides DM partners: the open timeline's senders.
    presence_on_screen: HashSet<String>,
    /// When the window was last touched, for going away when idle.
    last_activity: Instant,
    /// We've told the server we're away.
    away: bool,
    /// The profile card open, and where it points.
    pub profile_card: Option<ProfileTarget>,
}

/// Someone's profile card, anchored where they were clicked.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileTarget {
    pub user_id: String,
    pub at: Point<Pixels>,
}

/// The least time between two sounds of a kind.
const SOUND_GAP: Duration = Duration::from_millis(800);

/// A picture to show full-size.
#[derive(Debug, Clone)]
pub struct Lightbox {
    /// The original.
    pub mxc: Option<String>,
    /// What the timeline already has, shown while the original loads.
    pub thumbnail: Option<String>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub name: String,
}

/// Settings' sections, in the order the nav lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Account,
    Security,
    Notifications,
    About,
}

impl SettingsSection {
    pub const ALL: [SettingsSection; 4] = [SettingsSection::Account, SettingsSection::Security, SettingsSection::Notifications, SettingsSection::About];

    pub fn label(self) -> &'static str {
        match self {
            SettingsSection::Account => "account",
            SettingsSection::Security => "security",
            SettingsSection::Notifications => "notifications",
            SettingsSection::About => "about",
        }
    }
}

/// What the user can do to the verification on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyAction {
    Accept,
    /// The emoji match.
    Confirm,
    /// They don't: a security signal, sent as a mismatch, not a cancel.
    Mismatch,
    Cancel,
}

impl EventEmitter<horo_core::dto::TimelineUpdate> for Store {}

/// Spaces are recomputed from the room list, at most this often.
const SPACES_EVERY: Duration = Duration::from_secs(2);

impl Store {
    pub fn new(cx: &mut Context<Self>) -> Store {
        let mut store = Store {
            phase: Phase::Restoring,
            error: None,
            rooms: Vec::new(),
            rooms_seq: None,
            early: Vec::new(),
            spaces: Vec::new(),
            spaces_fetched: None,
            spaces_pending: false,
            sync: None,
            space: None,
            room: None,
            device_trust: None,
            verification: None,
            verification_error: None,
            settings: None,
            lightbox: None,
            prefs: Prefs::load(&Backend::global(cx).data_dir),
            watch: Watch::default(),
            window_active: true,
            sounded: [None, None],
            presence: HashMap::new(),
            presence_supported: false,
            presence_watched: HashSet::new(),
            presence_on_screen: HashSet::new(),
            last_activity: Instant::now(),
            away: false,
            profile_card: None,
        };
        store.pump_events(cx);
        store.watch_idle(cx);
        store.restore(cx);
        store
    }

    /// Fold the core's events in as they come.
    fn pump_events(&mut self, cx: &mut Context<Self>) {
        let Some(mut events) = Backend::global(cx).take_events() else { return };
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this.update(cx, |store, cx| store.apply(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Rooms(update) => {
                match self.rooms_seq {
                    None => self.early.push(update),
                    Some(seq) if update.seq <= seq => {}
                    Some(_) => {
                        self.rooms_seq = Some(update.seq);
                        apply_diffs(&mut self.rooms, update.diffs);
                        self.alert();
                    }
                }
                self.watch_presence(cx);
                self.refresh_spaces(cx);
            }
            Event::SyncStatus(status) => self.sync = Some(status.state),
            Event::Presence(update) => {
                self.presence_supported = update.supported;
                for user in update.users {
                    self.presence.insert(user.user_id.clone(), user);
                }
            }
            Event::VerificationState(state) => self.device_trust = Some(state),
            Event::VerificationRequest(request) => {
                if !verify::is_live(&self.verification) {
                    self.verification_error = None;
                }
                verify::offer(&mut self.verification, request);
            }
            Event::VerificationRequestUpdate(request) => verify::update_request(&mut self.verification, request),
            Event::SasUpdate(sas) => verify::update_sas(&mut self.verification, sas),
            // For whichever timeline view has that room open.
            Event::Timeline(update) => {
                cx.emit(update);
                return;
            }
            _ => return,
        }
        cx.notify();
    }

    fn restore(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let task = backend.run({
            let (host, state, dir) = (backend.host.clone(), backend.state.clone(), backend.data_dir.clone());
            async move { auth::restore(host, &state, &dir).await }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |store, cx| match outcome {
                Ok(Ok(Some(info))) => store.signed_in(info, cx),
                Ok(Ok(None)) => store.set_phase(Phase::SignedOut, cx),
                Ok(Err(e)) => store.failed(e.to_string(), cx),
                Err(e) => store.failed(e.to_string(), cx),
            })
        })
        .detach();
    }

    pub fn sign_in(&mut self, server: String, username: String, password: String, cx: &mut Context<Self>) {
        self.error = None;
        self.set_phase(Phase::SigningIn, cx);
        let backend = Backend::global(cx);
        let task = backend.run({
            let (host, state, dir) = (backend.host.clone(), backend.state.clone(), backend.data_dir.clone());
            async move { auth::login_password(host, &state, &dir, &server, &username, &password).await }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |store, cx| match outcome {
                Ok(Ok(info)) => store.signed_in(info, cx),
                Ok(Err(e)) => store.failed(e.to_string(), cx),
                Err(e) => store.failed(e.to_string(), cx),
            })
        })
        .detach();
    }

    /// Sign out; `wipe` also deletes this device's encrypted store.
    pub fn sign_out(&mut self, wipe: bool, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, dir) = (backend.state.clone(), backend.data_dir.clone());
            async move { auth::logout(&state, &dir, wipe).await }
        });
        self.rooms.clear();
        self.rooms_seq = None;
        self.spaces.clear();
        self.room = None;
        self.space = None;
        self.device_trust = None;
        self.verification = None;
        self.verification_error = None;
        self.settings = None;
        self.lightbox = None;
        self.watch.reset();
        self.presence.clear();
        self.presence_watched.clear();
        self.presence_on_screen.clear();
        self.profile_card = None;
        self.set_phase(Phase::SignedOut, cx);
        cx.spawn(async move |this, cx| {
            if let Ok(Err(e)) = task.await {
                let _ = this.update(cx, |store, cx| store.failed(format!("signing out: {e}"), cx));
            }
        })
        .detach();
    }

    fn failed(&mut self, error: String, cx: &mut Context<Self>) {
        tracing::warn!("{error}");
        self.error = Some(error.into());
        if !matches!(self.phase, Phase::SignedIn(_)) {
            self.phase = Phase::SignedOut;
        }
        cx.notify();
    }

    fn set_phase(&mut self, phase: Phase, cx: &mut Context<Self>) {
        self.phase = phase;
        cx.notify();
    }

    /// Signed in: take the room list as it stands, then follow its diffs.
    fn signed_in(&mut self, info: SessionInfo, cx: &mut Context<Self>) {
        self.error = None;
        self.set_phase(Phase::SignedIn(info), cx);
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move {
                let core = state.core().await?;
                let mirror = core.rooms.lock().await;
                Ok::<_, horo_core::Error>((mirror.seq, mirror.rooms.clone()))
            }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |store, cx| match outcome {
                Ok(Ok((seq, rooms))) => {
                    store.rooms = rooms;
                    store.rooms_seq = Some(seq);
                    for update in std::mem::take(&mut store.early) {
                        if update.seq > seq {
                            store.rooms_seq = Some(update.seq);
                            apply_diffs(&mut store.rooms, update.diffs);
                        }
                    }
                    store.watch.reset();
                    store.alert();
                    store.watch_presence(cx);
                    store.spaces_fetched = None;
                    store.refresh_spaces(cx);
                    cx.notify();
                }
                Ok(Err(e)) => store.failed(e.to_string(), cx),
                Err(e) => store.failed(e.to_string(), cx),
            })
        })
        .detach();
    }

    /// Spaces come from the client, not the room list's diffs; ask again
    /// when the list moves, no more often than [`SPACES_EVERY`].
    fn refresh_spaces(&mut self, cx: &mut Context<Self>) {
        if let Some(fetched) = self.spaces_fetched
            && fetched.elapsed() < SPACES_EVERY
        {
            // Throttled: one more fetch once the interval is up, so the last
            // change in a burst (the initial sync's, say) is never missed.
            if !self.spaces_pending {
                self.spaces_pending = true;
                let wait = SPACES_EVERY - fetched.elapsed();
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    let _ = this.update(cx, |store, cx| {
                        store.spaces_pending = false;
                        store.refresh_spaces(cx);
                    });
                })
                .detach();
            }
            return;
        }
        self.spaces_fetched = Some(Instant::now());
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { rooms::spaces(&*state.core().await?).await }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(spaces)) = task.await {
                let _ = this.update(cx, |store, cx| {
                    store.spaces = spaces;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn open_space(&mut self, space: Option<String>, cx: &mut Context<Self>) {
        self.space = space;
        cx.notify();
    }

    pub fn open_room(&mut self, room: String, cx: &mut Context<Self>) {
        self.room = Some(room);
        cx.notify();
    }

    /// Banners and a sound for what's new in the room list.
    fn alert(&mut self) {
        let looking = Looking { room: self.room.as_deref(), window_active: self.window_active };
        let outcome = self.watch.inspect(&self.rooms, &self.prefs, looking);
        for alert in outcome.alerts {
            alerts::show(alert);
        }
        if let Some(kind) = outcome.sound {
            let slot = &mut self.sounded[kind as usize];
            if slot.is_none_or(|at| at.elapsed() >= SOUND_GAP) {
                *slot = Some(Instant::now());
                let id = match kind {
                    Kind::Message => &self.prefs.message_sound,
                    Kind::Call => &self.prefs.call_sound,
                };
                sounds::play(kind, id, self.prefs.notification_volume);
            }
        }
    }

    /// The open timeline's senders, to watch their presence too.
    pub fn set_people_on_screen(&mut self, people: HashSet<String>, cx: &mut Context<Self>) {
        if people != self.presence_on_screen {
            self.presence_on_screen = people;
            self.watch_presence(cx);
        }
    }

    /// Tell the core whose presence to poll: DM partners, people on screen,
    /// and whoever's card is open. Only when the set changes.
    fn watch_presence(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.phase, Phase::SignedIn(_)) {
            return;
        }
        let mut wanted: HashSet<String> = self.rooms.iter().filter_map(|r| r.dm_user_id.clone()).collect();
        wanted.extend(self.presence_on_screen.iter().cloned());
        wanted.extend(self.profile_card.as_ref().map(|c| c.user_id.clone()));
        if wanted == self.presence_watched {
            return;
        }
        self.presence_watched = wanted.clone();
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { core_presence::watch(&*state.core().await?, wanted.into_iter().collect()).await }
        });
        cx.background_spawn(async move {
            if let Ok(Err(e)) = task.await {
                tracing::debug!("couldn't watch presence: {e}");
            }
        })
        .detach();
    }

    /// Someone touched the window: back to online if we'd gone away.
    pub fn activity(&mut self, cx: &mut Context<Self>) {
        self.last_activity = Instant::now();
        if self.away {
            self.away = false;
            self.publish_own(OwnPresence::Online, cx);
        }
    }

    /// Away once the window has sat untouched for a while; the core keeps
    /// "online" alive itself, so only the changes go out.
    fn watch_idle(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(30)).await;
                let Ok(()) = this.update(cx, |store, cx| {
                    if !store.away && matches!(store.phase, Phase::SignedIn(_)) && crate::presence::idle(store.last_activity) {
                        store.away = true;
                        store.publish_own(OwnPresence::Unavailable, cx);
                    }
                }) else {
                    break;
                };
            }
        })
        .detach();
    }

    fn publish_own(&mut self, presence: OwnPresence, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { core_presence::set_own(&*state.core().await?, presence).await }
        });
        cx.background_spawn(async move {
            if let Ok(Err(e)) = task.await {
                tracing::debug!("couldn't set presence: {e}");
            }
        })
        .detach();
    }

    pub fn open_profile(&mut self, user_id: String, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.profile_card = Some(ProfileTarget { user_id, at });
        self.watch_presence(cx);
        cx.notify();
    }

    pub fn close_profile(&mut self, cx: &mut Context<Self>) {
        if self.profile_card.take().is_some() {
            cx.notify();
        }
    }

    pub fn set_window_active(&mut self, active: bool) {
        self.window_active = active;
    }

    /// Change the app's settings and save them.
    pub fn update_prefs(&mut self, change: impl FnOnce(&mut Prefs), cx: &mut Context<Self>) {
        change(&mut self.prefs);
        self.prefs.save(&Backend::global(cx).data_dir);
        cx.notify();
    }

    pub fn open_lightbox(&mut self, lightbox: Lightbox, cx: &mut Context<Self>) {
        self.lightbox = Some(lightbox);
        cx.notify();
    }

    pub fn close_lightbox(&mut self, cx: &mut Context<Self>) {
        self.lightbox = None;
        cx.notify();
    }

    /// Open settings at `section`, or close them with `None`.
    pub fn show_settings(&mut self, section: Option<SettingsSection>, cx: &mut Context<Self>) {
        self.settings = section;
        cx.notify();
    }

    /// The user closes the dialog (a finished flow, or "later" on the banner).
    pub fn close_verification(&mut self, cx: &mut Context<Self>) {
        self.verification = None;
        self.verification_error = None;
        cx.notify();
    }

    /// Accept, confirm, deny or cancel the verification on screen.
    pub fn act_on_verification(&mut self, action: VerifyAction, cx: &mut Context<Self>) {
        let Some(flow) = &self.verification else { return };
        let (user, flow_id) = (flow.request.other_user_id.clone(), flow.request.flow_id.clone());
        self.verification_error = None;
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move {
                let core = state.core().await?;
                match action {
                    VerifyAction::Accept => verification::accept(&core, &user, &flow_id).await,
                    VerifyAction::Confirm => verification::confirm(&core, &user, &flow_id).await,
                    VerifyAction::Mismatch => verification::mismatch(&core, &user, &flow_id).await,
                    VerifyAction::Cancel => verification::cancel(&core, &user, &flow_id).await,
                }
            }
        });
        // "later" on an incoming request: decline it and get out of the way.
        if action == VerifyAction::Cancel {
            self.verification = None;
        }
        self.report_verification(task, cx);
        cx.notify();
    }

    /// Ask to verify this device: `None` asks our other devices, `Some`
    /// one of them in particular. The request comes back as an event and
    /// opens the dialog.
    pub fn start_verification(&mut self, device_id: Option<String>, cx: &mut Context<Self>) {
        self.verification_error = None;
        let backend = Backend::global(cx);
        let task = backend.run({
            let (host, state) = (backend.host.clone(), backend.state.clone());
            async move {
                let core = state.core().await?;
                match device_id {
                    Some(id) => verification::verify_device(&host, &core, &id).await,
                    None => verification::request(&host, &core, None).await,
                }
            }
        });
        self.report_verification(task, cx);
    }

    fn report_verification(&mut self, task: impl std::future::Future<Output = anyhow::Result<horo_core::Result<()>>> + 'static, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => return,
                Ok(Err(e)) => e.to_string(),
                Err(e) => e.to_string(),
            };
            tracing::warn!("verification: {error}");
            let _ = this.update(cx, |store, cx| {
                store.verification_error = Some(error.into());
                cx.notify();
            });
        })
        .detach();
    }

    pub fn current_space(&self) -> Option<&SpaceSummary> {
        let id = self.space.as_ref()?;
        self.spaces.iter().find(|s| s.id == *id)
    }
}

/// `eyeball_im`'s diffs, applied to a plain list.
pub fn apply_diffs<T: Clone>(list: &mut Vec<T>, diffs: Vec<Diff<T>>) {
    for diff in diffs {
        apply_diff(list, diff);
    }
}

/// One diff, applied; returns what changed as a splice (the old range
/// replaced, and how many items took its place), for a list view to follow.
/// `None` when the diff fell outside the list and changed nothing.
pub fn apply_diff<T: Clone>(list: &mut Vec<T>, diff: Diff<T>) -> Option<(std::ops::Range<usize>, usize)> {
    let len = list.len();
    Some(match diff {
        Diff::Append { values } => {
            let n = values.len();
            list.extend(values);
            (len..len, n)
        }
        Diff::Clear => {
            list.clear();
            (0..len, 0)
        }
        Diff::PushFront { value } => {
            list.insert(0, value);
            (0..0, 1)
        }
        Diff::PushBack { value } => {
            list.push(value);
            (len..len, 1)
        }
        Diff::PopFront => {
            if len == 0 {
                return None;
            }
            list.remove(0);
            (0..1, 0)
        }
        Diff::PopBack => {
            list.pop()?;
            (len - 1..len, 0)
        }
        Diff::Insert { index, value } => {
            let index = index.min(len);
            list.insert(index, value);
            (index..index, 1)
        }
        Diff::Set { index, value } => {
            *list.get_mut(index)? = value;
            (index..index + 1, 1)
        }
        Diff::Remove { index } => {
            if index >= len {
                return None;
            }
            list.remove(index);
            (index..index + 1, 0)
        }
        Diff::Truncate { length } => {
            if length >= len {
                return None;
            }
            list.truncate(length);
            (length..len, 0)
        }
        Diff::Reset { values } => {
            let n = values.len();
            *list = values;
            (0..len, n)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffs_apply_in_order_and_out_of_range_ones_are_ignored() {
        let mut list = vec![1, 2, 3];
        apply_diffs(
            &mut list,
            vec![
                Diff::PushFront { value: 0 },
                Diff::Set { index: 3, value: 30 },
                Diff::Remove { index: 1 },
                Diff::Insert { index: 99, value: 9 },
                Diff::Set { index: 99, value: 0 },
                Diff::Remove { index: 99 },
                Diff::PopBack,
                Diff::Append { values: vec![7, 8] },
            ],
        );
        assert_eq!(list, vec![0, 2, 30, 7, 8]);
        apply_diffs(&mut list, vec![Diff::Truncate { length: 2 }, Diff::PopFront]);
        assert_eq!(list, vec![2]);
        apply_diffs(&mut list, vec![Diff::Reset { values: vec![5] }, Diff::PushBack { value: 6 }]);
        assert_eq!(list, vec![5, 6]);
    }
}
