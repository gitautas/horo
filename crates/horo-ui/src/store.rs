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

use horo_core::matrix::packs::ImagePackDto;

use crate::{
    alerts::{self, Looking, Watch},
    call::CallView,
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
    /// Custom emotes and stickers usable in the open room: your own pack,
    /// the room's, and the ones you carry everywhere, in that order.
    pub packs: Vec<ImagePackDto>,
    /// Which room `packs` was loaded for.
    packs_for: Option<String>,
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
    /// The thread open in that room, by its root event, in place of the
    /// room's timeline; and the root's text, for the bar above it.
    pub thread: Option<String>,
    pub thread_preview: Option<String>,
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
    /// A line at the foot of the window saying how something went, until
    /// it's dismissed.
    pub banner: Option<Banner>,
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
    /// The members listed in the room info panel.
    presence_in_panel: HashSet<String>,
    /// When the window was last touched, for going away when idle.
    last_activity: Instant,
    /// We've told the server we're away.
    away: bool,
    /// The profile card open, and where it points.
    pub profile_card: Option<ProfileTarget>,
    /// The call we're in (or joining, or that just failed).
    pub call: Option<CallView>,
    /// The engine behind `call`, once connected.
    pub(crate) call_live: Option<crate::call::Live>,
    /// A DM calling us, ringing until answered, declined or given up.
    pub incoming: Option<Incoming>,
    /// Who's typing in each open room, as the server last said (never us).
    pub typing: HashMap<String, Vec<horo_core::dto::TypingUser>>,
    /// Names and pictures of people met outside a member list (in calls),
    /// by user ID; ourselves from sign-in.
    pub profiles: HashMap<String, Known>,
    profiles_asked: HashSet<String>,
    /// What the last look for a new version found.
    pub update: UpdateState,
    /// The periodic look has been started.
    update_watch: bool,
}

/// Looking for a newer horo.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    /// Newer than this build.
    Available(horo_core::update::Release),
    UpToDate,
    Failed(String),
}

/// What we know of someone's profile.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Known {
    pub name: Option<String>,
    pub avatar: Option<String>,
}

/// A call ringing here.
#[derive(Debug, Clone)]
pub struct Incoming {
    pub room_id: String,
    pub since: Instant,
}

/// How long a call rings before giving up: RING_TIMEOUT_MS in call.ts, and
/// the lifetime the ring notification carries, so every client stops
/// together.
const RING_FOR: Duration = Duration::from_secs(45);
/// Between rings.
const RING_EVERY: Duration = Duration::from_millis(3200);

/// Someone's profile card, anchored where they were clicked.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileTarget {
    pub user_id: String,
    pub at: Point<Pixels>,
    /// The face that was clicked (often a room's own avatar for them),
    /// shown when their global profile has none.
    pub avatar_hint: Option<String>,
}

/// When to first look for a new version, and how often after.
const UPDATE_FIRST: Duration = Duration::from_secs(20);
const UPDATE_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// The least time between two sounds of a kind.
const SOUND_GAP: Duration = Duration::from_millis(800);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Error,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Banner {
    pub tone: Tone,
    pub message: SharedString,
}

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
    Voice,
    Notifications,
    Packs,
    About,
}

impl SettingsSection {
    pub const ALL: [SettingsSection; 6] = [
        SettingsSection::Account,
        SettingsSection::Security,
        SettingsSection::Voice,
        SettingsSection::Notifications,
        SettingsSection::Packs,
        SettingsSection::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsSection::Account => "account",
            SettingsSection::Security => "security",
            SettingsSection::Voice => "media",
            SettingsSection::Notifications => "notifications",
            SettingsSection::Packs => "emotes & stickers",
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
            packs: Vec::new(),
            packs_for: None,
            rooms_seq: None,
            early: Vec::new(),
            spaces: Vec::new(),
            spaces_fetched: None,
            spaces_pending: false,
            sync: None,
            space: None,
            room: None,
            thread: None,
            thread_preview: None,
            device_trust: None,
            verification: None,
            verification_error: None,
            settings: None,
            lightbox: None,
            banner: None,
            prefs: Prefs::load(&Backend::global(cx).data_dir),
            watch: Watch::default(),
            window_active: true,
            sounded: [None, None],
            presence: HashMap::new(),
            presence_supported: false,
            presence_watched: HashSet::new(),
            presence_on_screen: HashSet::new(),
            presence_in_panel: HashSet::new(),
            typing: HashMap::new(),
            profiles: HashMap::new(),
            profiles_asked: HashSet::new(),
            update: UpdateState::Idle,
            update_watch: false,
            last_activity: Instant::now(),
            away: false,
            profile_card: None,
            call: None,
            call_live: None,
            incoming: None,
        };
        store.pump_events(cx);
        store.watch_idle(cx);
        // Quitting mid-call leaves it properly: otherwise our membership
        // sits there and others see us in the call for hours.
        cx.on_app_quit(|store, cx| {
            let live = store.call_live.take();
            let task = live.map(|live| Store::hang_up_task(live, cx));
            async move {
                if let Some(task) = task {
                    task.await;
                }
            }
        })
        .detach();
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
                        self.alert(cx);
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
            Event::Typing(update) => {
                if update.users.is_empty() {
                    self.typing.remove(&update.room_id);
                } else {
                    self.typing.insert(update.room_id, update.users);
                }
            }
            // For whichever timeline view has that room open.
            Event::Timeline(update) => {
                cx.emit(update);
                return;
            }
        }
        cx.notify();
    }

    /// Look for a new version a little after starting, then every few
    /// hours, as the WebView build does.
    fn watch_updates(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.update_watch, true) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(UPDATE_FIRST).await;
            loop {
                if this.update(cx, |s, cx| s.check_for_update(false, cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(UPDATE_EVERY).await;
            }
        })
        .detach();
    }

    /// Ask whether there's a newer release. `asked` is someone pressing
    /// "check now": they hear "you're on the latest" and failures; a
    /// background look that finds nothing says nothing.
    pub fn check_for_update(&mut self, asked: bool, cx: &mut Context<Self>) {
        if self.update == UpdateState::Checking {
            return;
        }
        let before = std::mem::replace(&mut self.update, UpdateState::Checking);
        cx.notify();
        let task = Backend::global(cx).run(horo_core::update::latest_release());
        cx.spawn(async move |this, cx| {
            let found = match task.await {
                Ok(found) => found.map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = this.update(cx, |s, cx| {
                s.update = match found {
                    Ok(release) if horo_core::update::is_newer(&release.version, env!("CARGO_PKG_VERSION")) => UpdateState::Available(release),
                    Ok(_) => UpdateState::UpToDate,
                    Err(e) if asked => UpdateState::Failed(e),
                    // A quiet look that failed leaves what was known.
                    Err(e) => {
                        tracing::warn!("looking for a new version: {e}");
                        match before {
                            UpdateState::Available(r) => UpdateState::Available(r),
                            _ => UpdateState::Idle,
                        }
                    }
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Hide the notice for the version on offer.
    pub fn dismiss_update(&mut self, cx: &mut Context<Self>) {
        if let UpdateState::Available(release) = &self.update {
            let version = release.version.clone();
            self.update_prefs(|p| p.dismissed_update = Some(version), cx);
        }
    }

    /// Fetch the profiles of `users` we know nothing of yet, once each.
    pub fn want_profiles(&mut self, users: impl IntoIterator<Item = String>, cx: &mut Context<Self>) {
        let missing: Vec<String> = users.into_iter().filter(|u| !self.profiles.contains_key(u) && self.profiles_asked.insert(u.clone())).collect();
        if missing.is_empty() {
            return;
        }
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move {
                let core = state.core().await?;
                let mut out = Vec::new();
                for user in missing {
                    if let Ok(p) = horo_core::matrix::profile::get_profile(&core, Some(user.clone())).await {
                        out.push((user, Known { name: p.display_name, avatar: p.avatar_url }));
                    }
                }
                Ok::<_, horo_core::Error>(out)
            }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(found)) = task.await {
                let _ = this.update(cx, |store, cx| {
                    store.profiles.extend(found);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Who we're signed in as.
    pub fn own_user_id(&self) -> Option<&str> {
        match &self.phase {
            Phase::SignedIn(info) => Some(&info.user_id),
            _ => None,
        }
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
        self.packs.clear();
        self.packs_for = None;
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
        self.presence_in_panel.clear();
        self.profile_card = None;
        self.incoming = None;
        self.leave_call(cx);
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
    pub fn signed_in(&mut self, info: SessionInfo, cx: &mut Context<Self>) {
        self.error = None;
        self.watch_updates(cx);
        if info.avatar_url.is_some() {
            self.profiles.insert(info.user_id.clone(), Known { name: info.display_name.clone(), avatar: info.avatar_url.clone() });
        }
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
                    store.alert(cx);
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

    /// Re-read the spaces now, past the throttle: a space just joined or
    /// created should be in the rail without waiting for the next poll.
    pub fn refresh_spaces_now(&mut self, cx: &mut Context<Self>) {
        self.spaces_fetched = None;
        self.refresh_spaces(cx);
    }

    pub fn open_space(&mut self, space: Option<String>, cx: &mut Context<Self>) {
        self.space = space;
        cx.notify();
    }

    /// Load the packs usable in `room_id`, unless they're already here.
    /// A failure leaves none: a picker without custom packs is still a
    /// picker.
    pub fn load_packs(&mut self, room_id: &str, cx: &mut Context<Self>) {
        if self.packs_for.as_deref() == Some(room_id) {
            return;
        }
        self.packs_for = Some(room_id.to_owned());
        let Ok(room) = matrix_sdk::ruma::OwnedRoomId::try_from(room_id) else { return };
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { horo_core::matrix::packs::available(&*state.core().await?, Some(&room)).await }
        });
        let wanted = room_id.to_owned();
        cx.spawn(async move |this, cx| {
            let packs = match task.await {
                Ok(Ok(packs)) => packs,
                Ok(Err(e)) => {
                    tracing::debug!("couldn't load emote packs: {e}");
                    Vec::new()
                }
                Err(_) => Vec::new(),
            };
            let _ = this.update(cx, |store, cx| {
                if store.packs_for.as_deref() == Some(wanted.as_str()) {
                    store.packs = packs;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Read the open room's packs again, after they were edited.
    pub fn reload_packs(&mut self, cx: &mut Context<Self>) {
        self.packs_for = None;
        if let Some(room) = self.room.clone() {
            self.load_packs(&room, cx);
        }
    }

    pub fn open_room(&mut self, room: String, cx: &mut Context<Self>) {
        if self.room.as_deref() != Some(room.as_str()) {
            self.thread = None;
            self.thread_preview = None;
        }
        self.room = Some(room);
        cx.notify();
    }

    /// Stop showing `room_id` (it was just left).
    pub fn close_room(&mut self, room_id: &str, cx: &mut Context<Self>) {
        if self.room.as_deref() == Some(room_id) {
            self.room = None;
            self.thread = None;
            self.thread_preview = None;
            cx.notify();
        }
    }

    /// Show a thread of the open room in place of its timeline, or (`None`)
    /// go back to the room.
    pub fn open_thread(&mut self, root: Option<String>, cx: &mut Context<Self>) {
        if self.thread == root {
            return;
        }
        self.thread = root.clone();
        self.thread_preview = None;
        cx.notify();
        let (Some(root), Some(room)) = (root, self.room.clone()) else { return };
        let Ok(room_id) = matrix_sdk::ruma::OwnedRoomId::try_from(room.as_str()) else { return };
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, root) = (backend.state.clone(), root.clone());
            async move { horo_core::matrix::timeline::event_body(&*state.core().await?, &room_id, &root).await }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(body))) = task.await {
                let _ = this.update(cx, |store, cx| {
                    if store.thread.as_deref() == Some(root.as_str()) {
                        store.thread_preview = Some(body.replace('\n', " "));
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    /// Banners and a sound for what's new in the room list.
    fn alert(&mut self, cx: &mut Context<Self>) {
        let in_call = self.call.as_ref().map(|c| c.room_id.clone());
        let looking = Looking { room: self.room.as_deref(), window_active: self.window_active, in_call: in_call.as_deref() };
        let outcome = self.watch.inspect(&self.rooms, &self.prefs, looking);
        for alert in outcome.alerts {
            alerts::show(alert);
        }
        if let Some(room_id) = outcome.ring {
            self.start_ringing(room_id, cx);
        }
        // A call that ended stops ringing, however it ended (they hung up,
        // or another of your devices answered).
        if let Some(incoming) = &self.incoming
            && !self.rooms.iter().any(|r| r.id == incoming.room_id && r.has_active_call)
        {
            self.stop_ringing(cx);
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

    /// Ring for a DM call until it's answered, declined or given up on.
    fn start_ringing(&mut self, room_id: String, cx: &mut Context<Self>) {
        // The first ring wins: a second call mustn't swap the room behind
        // the answer button.
        if self.incoming.is_some() {
            return;
        }
        let since = Instant::now();
        self.incoming = Some(Incoming { room_id: room_id.clone(), since });
        cx.spawn(async move |this, cx| {
            loop {
                let Ok(ringing) = this.update(cx, |store, cx| {
                    let still = store.incoming.as_ref().is_some_and(|i| i.room_id == room_id && i.since == since);
                    if still && since.elapsed() >= RING_FOR {
                        store.stop_ringing(cx);
                        return false;
                    }
                    if still {
                        sounds::play(Kind::Call, &store.prefs.call_sound, store.prefs.notification_volume);
                    }
                    still
                }) else {
                    break;
                };
                if !ringing {
                    break;
                }
                cx.background_executor().timer(RING_EVERY).await;
            }
        })
        .detach();
        cx.notify();
    }

    pub fn stop_ringing(&mut self, cx: &mut Context<Self>) {
        if self.incoming.take().is_some() {
            cx.notify();
        }
    }

    /// Answer the ringing call: join it and go to its room.
    pub fn answer(&mut self, cx: &mut Context<Self>) {
        let Some(incoming) = self.incoming.take() else { return };
        self.room = Some(incoming.room_id.clone());
        self.join_call(incoming.room_id, cx);
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
    /// Who the room info panel lists, for their presence dots.
    pub fn set_people_in_panel(&mut self, people: HashSet<String>, cx: &mut Context<Self>) {
        if people != self.presence_in_panel {
            self.presence_in_panel = people;
            self.watch_presence(cx);
        }
    }

    fn watch_presence(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.phase, Phase::SignedIn(_)) {
            return;
        }
        let mut wanted: HashSet<String> = self.rooms.iter().filter_map(|r| r.dm_user_id.clone()).collect();
        wanted.extend(self.presence_on_screen.iter().cloned());
        wanted.extend(self.presence_in_panel.iter().cloned());
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

    pub fn open_profile(&mut self, user_id: String, at: Point<Pixels>, avatar_hint: Option<String>, cx: &mut Context<Self>) {
        self.profile_card = Some(ProfileTarget { user_id, at, avatar_hint });
        self.watch_presence(cx);
        cx.notify();
    }

    pub fn close_profile(&mut self, cx: &mut Context<Self>) {
        if self.profile_card.take().is_some() {
            cx.notify();
        }
    }

    /// We've told the server we're away (idle).
    pub fn is_away(&self) -> bool {
        self.away
    }

    pub fn set_window_active(&mut self, active: bool) {
        self.window_active = active;
    }

    /// Change the app's settings and save them.
    /// Change prefs for now, without writing them out (a column being
    /// dragged: saved once, when it's let go).
    pub fn preview_prefs(&mut self, change: impl FnOnce(&mut Prefs), cx: &mut Context<Self>) {
        change(&mut self.prefs);
        cx.notify();
    }

    pub fn update_prefs(&mut self, change: impl FnOnce(&mut Prefs), cx: &mut Context<Self>) {
        change(&mut self.prefs);
        self.prefs.save(&Backend::global(cx).data_dir);
        cx.notify();
    }

    pub fn show_banner(&mut self, tone: Tone, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.banner = Some(Banner { tone, message: message.into() });
        cx.notify();
    }

    pub fn dismiss_banner(&mut self, cx: &mut Context<Self>) {
        if self.banner.take().is_some() {
            cx.notify();
        }
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
