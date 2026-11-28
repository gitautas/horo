//! Voice calls in the app, on horo's own engine (`horo_rtc::call`), the way
//! src/lib/call.ts runs them in the WebView build: one call at a time,
//! joining another leaves the first; MatrixRTC membership through
//! `horo_rtc::matrixrtc`, refreshed hourly, withdrawn on leaving.
//!
//! The store holds what the UI draws ([`CallView`]) and the engine's
//! handles ([`Live`]); the engine's events come back through a channel and
//! are folded into the view on gpui's executor. Audio only for now: the
//! engine has no video yet.

use std::{sync::{Arc, Mutex}, time::{Duration, Instant}};

use gpui::{AppContext as _, Context};
use horo_rtc::call::{
    Call, CallEvent, CallOptions, Joined, Participant, TokenSource,
    devices::{self, AudioDevices, RunningAudio},
};
use horo_rtc::audio::gate::GateReading;
use matrix_sdk::ruma::OwnedRoomId;

use crate::{backend::Backend, store::Store};

/// A membership advertisement lasts four hours; refresh well inside that.
const MEMBERSHIP_REFRESH: Duration = Duration::from_secs(60 * 60);

/// What our own camera's picture is kept under, among the call's images.
#[cfg(target_os = "linux")]
pub const OWN_CAMERA: &str = "own-camera";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallStatus {
    Connecting,
    Connected,
    Reconnecting,
    Failed(String),
}

/// Louder than this (dBFS), with the gate open, is speech: the floor for
/// when the gate is off and always open.
const SPEAKING_DB: f32 = -50.;
/// How long our ring stays lit after the last loud reading, so it doesn't
/// flicker between words.
const SPEAKING_HOLD: Duration = Duration::from_millis(250);

/// Which tile is shown big in the call stage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum FocusMode {
    /// Follow the first shared screen automatically, or show the grid when
    /// nobody is sharing.
    #[default]
    Auto,
    /// Show this tile big until it leaves or the user clicks it again.
    Pinned(String),
    /// Show everyone alike in a grid, even while a screen is shared.
    Grid,
}

/// Right-click context menu target for a remote call participant.
#[derive(Debug, Clone)]
pub struct ParticipantMenu {
    pub identity: String,
    pub user_id: String,
    pub name: String,
    pub avatar: Option<String>,
    pub at: gpui::Point<gpui::Pixels>,
}

/// Which of the four viewport corners the floating mini call window docks into.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PipCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

/// Which edge or corner of the floating mini call window is being dragged to resize it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipResizeEdge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Default, minimum, and maximum outer `(width, height)` of the floating mini call window (design px).
pub const PIP_DEFAULT_SIZE: (f32, f32) = (360., 296.);
pub const PIP_MIN_SIZE: (f32, f32) = (260., 210.);
pub const PIP_MAX_SIZE: (f32, f32) = (960., 760.);
/// Inset from the viewport's left, right, and bottom edges when docked in a corner (design px).
pub const PIP_CORNER_INSET: f32 = 18.;
/// Inset from the top edge when docked in a top corner (clears macOS traffic lights).
#[cfg(target_os = "macos")]
pub const PIP_TOP_INSET: f32 = 44.;
#[cfg(not(target_os = "macos"))]
pub const PIP_TOP_INSET: f32 = 18.;
/// Minimum margin keeping the floating call window inside the viewport (design px).
pub const PIP_MARGIN: f32 = 12.;

/// The `(right, bottom)` inset for docking a call window of `size` into `corner`.
pub fn pip_corner_pos(corner: PipCorner, size: (f32, f32), viewport: (f32, f32)) -> (f32, f32) {
    let right = match corner {
        PipCorner::TopRight | PipCorner::BottomRight => PIP_CORNER_INSET,
        PipCorner::TopLeft | PipCorner::BottomLeft => (viewport.0 - size.0 - PIP_CORNER_INSET).max(PIP_MARGIN),
    };
    let bottom = match corner {
        PipCorner::BottomLeft | PipCorner::BottomRight => PIP_CORNER_INSET,
        PipCorner::TopLeft | PipCorner::TopRight => (viewport.1 - size.1 - PIP_TOP_INSET).max(PIP_MARGIN),
    };
    (right, bottom)
}

/// Which of the four corners is closest to the floating window's current center.
pub fn nearest_pip_corner(rb: (f32, f32), size: (f32, f32), viewport: (f32, f32)) -> PipCorner {
    let center_x = viewport.0 - rb.0 - size.0 * 0.5;
    let center_y = viewport.1 - rb.1 - size.1 * 0.5;
    match (center_x < viewport.0 * 0.5, center_y < viewport.1 * 0.5) {
        (true, true) => PipCorner::TopLeft,
        (false, true) => PipCorner::TopRight,
        (true, false) => PipCorner::BottomLeft,
        (false, false) => PipCorner::BottomRight,
    }
}

/// Clamp the floating call window's `(right, bottom)` position and outer `(width, height)`
/// so it always stays within the viewport bounds.
pub fn clamp_pip_rect(pos: (f32, f32), size: (f32, f32), viewport: (f32, f32)) -> ((f32, f32), (f32, f32)) {
    let max_w = (viewport.0 - PIP_MARGIN * 2.).min(PIP_MAX_SIZE.0).max(PIP_MIN_SIZE.0);
    let max_h = (viewport.1 - PIP_MARGIN * 2.).min(PIP_MAX_SIZE.1).max(PIP_MIN_SIZE.1);
    let w = size.0.clamp(PIP_MIN_SIZE.0, max_w);
    let h = size.1.clamp(PIP_MIN_SIZE.1, max_h);
    let right = pos.0.clamp(PIP_MARGIN, (viewport.0 - w - PIP_MARGIN).max(PIP_MARGIN));
    let bottom = pos.1.clamp(PIP_MARGIN, (viewport.1 - h - PIP_MARGIN).max(PIP_MARGIN));
    ((right, bottom), (w, h))
}

/// Compute the new `((right, bottom), (width, height))` while dragging `edge` from
/// `start_mouse` to `mouse`, keeping the opposite edge(s) anchored in place.
pub fn resize_pip_rect(
    edge: PipResizeEdge,
    start_mouse: (f32, f32),
    start_rb: (f32, f32),
    start_size: (f32, f32),
    mouse: (f32, f32),
    viewport: (f32, f32),
) -> ((f32, f32), (f32, f32)) {
    let dx = mouse.0 - start_mouse.0;
    let dy = mouse.1 - start_mouse.1;
    let max_w = (viewport.0 - PIP_MARGIN * 2.).min(PIP_MAX_SIZE.0).max(PIP_MIN_SIZE.0);
    let max_h = (viewport.1 - PIP_MARGIN * 2.).min(PIP_MAX_SIZE.1).max(PIP_MIN_SIZE.1);

    let (new_w, new_right) = match edge {
        PipResizeEdge::Left | PipResizeEdge::TopLeft | PipResizeEdge::BottomLeft => {
            let avail_w = (viewport.0 - start_rb.0 - PIP_MARGIN).min(max_w).max(PIP_MIN_SIZE.0);
            let w = (start_size.0 - dx).clamp(PIP_MIN_SIZE.0, avail_w);
            (w, start_rb.0)
        }
        PipResizeEdge::Right | PipResizeEdge::TopRight | PipResizeEdge::BottomRight => {
            let avail_w = (start_size.0 + start_rb.0 - PIP_MARGIN).min(max_w).max(PIP_MIN_SIZE.0);
            let w = (start_size.0 + dx).clamp(PIP_MIN_SIZE.0, avail_w);
            let dw = w - start_size.0;
            (w, (start_rb.0 - dw).max(PIP_MARGIN))
        }
        PipResizeEdge::Top | PipResizeEdge::Bottom => (start_size.0, start_rb.0),
    };

    let (new_h, new_bottom) = match edge {
        PipResizeEdge::Top | PipResizeEdge::TopLeft | PipResizeEdge::TopRight => {
            let avail_h = (viewport.1 - start_rb.1 - PIP_MARGIN).min(max_h).max(PIP_MIN_SIZE.1);
            let h = (start_size.1 - dy).clamp(PIP_MIN_SIZE.1, avail_h);
            (h, start_rb.1)
        }
        PipResizeEdge::Bottom | PipResizeEdge::BottomLeft | PipResizeEdge::BottomRight => {
            let avail_h = (start_size.1 + start_rb.1 - PIP_MARGIN).min(max_h).max(PIP_MIN_SIZE.1);
            let h = (start_size.1 + dy).clamp(PIP_MIN_SIZE.1, avail_h);
            let dh = h - start_size.1;
            (h, (start_rb.1 - dh).max(PIP_MARGIN))
        }
        PipResizeEdge::Left | PipResizeEdge::Right => (start_size.1, start_rb.1),
    };

    ((new_right, new_bottom), (new_w, new_h))
}

/// The call, as the UI draws it.
#[derive(Debug, Clone)]
pub struct CallView {
    pub room_id: String,
    pub status: CallStatus,
    pub muted: bool,
    /// We hear nobody (and, while it lasts, send nothing).
    pub deafened: bool,
    /// Which tile is shown big (auto-focusing a screen share by default).
    pub focus: FocusMode,
    /// The call fills the window.
    pub fullscreen: bool,
    /// Our camera is on.
    pub camera: bool,
    /// We're sharing our screen.
    pub sharing: bool,
    /// Our identity in the room (`@user:server:DEVICE`), once connected.
    pub identity: Option<String>,
    /// Everyone else.
    pub participants: Vec<Participant>,
    /// Per-participant local playback volume (`0.0` = muted, `1.0` = 100%, up to `2.0`).
    pub participant_volumes: std::collections::HashMap<String, f32>,
    /// Whether the per-user call volume mixer popover is open.
    pub mixer_open: bool,
    /// User minimized the bottom-right mini call modal.
    pub pip_hidden: bool,
    /// Which of the four viewport corners the floating mini call window rests in.
    pub pip_corner: PipCorner,
    /// Live `(right, bottom)` offset in design pixels while dragging or resizing.
    pub pip_pos: Option<(f32, f32)>,
    /// Custom outer `(width, height)` in design pixels for the resizable mini call window.
    pub pip_size: Option<(f32, f32)>,
    /// Drag anchor `((start_mouse_x, start_mouse_y), (start_right, start_bottom))` in design pixels.
    pip_drag_anchor: Option<((f32, f32), (f32, f32))>,
    /// Resize anchor `(start_mouse, start_rb, start_size)` in design pixels.
    pip_resize_anchor: Option<((f32, f32), (f32, f32), (f32, f32))>,
    /// Identities speaking now, by the server's ear (ours is ignored: it
    /// arrives late, see `self_speaking`).
    pub speaking: Vec<String>,
    /// We're talking, by our own microphone: at once, where the server's
    /// word lags by a few hundred milliseconds.
    pub self_speaking: bool,
    /// When the microphone last sounded like speech.
    loud_at: Option<Instant>,
    /// The microphone's latest level and whether the gate lets it through
    /// (for the meter in settings; read without a redraw of its own).
    pub mic: Option<horo_rtc::audio::gate::GateReading>,
}

impl CallView {
    pub fn is_live(&self) -> bool {
        !matches!(self.status, CallStatus::Failed(_))
    }

    /// True while the user is actively dragging the floating call window to another corner.
    pub fn is_pip_dragging(&self) -> bool {
        self.pip_drag_anchor.is_some() && self.pip_pos.is_some()
    }

    /// Whether `user_id` is talking (any of their devices).
    pub fn is_speaking(&self, user_id: &str) -> bool {
        if self.identity.as_deref().map(user_of) == Some(user_id) {
            return self.self_speaking;
        }
        self.speaking.iter().any(|identity| user_of(identity) == user_id)
    }

    /// Take in a microphone reading; whether our speaking changed.
    fn heard(&mut self, reading: GateReading, now: Instant) -> bool {
        if reading.open && reading.level > SPEAKING_DB && !self.muted && !self.deafened {
            self.loud_at = Some(now);
        }
        let speaking = self.loud_at.is_some_and(|at| now.duration_since(at) < SPEAKING_HOLD);
        std::mem::replace(&mut self.self_speaking, speaking) != speaking
    }
}

/// The engine's handles for the call in progress.
pub struct Live {
    room_id: OwnedRoomId,
    call: Call,
    audio: RunningAudio,
    refresher: Option<tokio::task::AbortHandle>,
    /// Everyone's camera, newest picture each.
    pub screens: horo_rtc::call::Screens,
    /// Where our camera's and screen's pictures go.
    feed: Arc<Mutex<horo_rtc::call::VideoFeed>>,
    screen_feed: Arc<Mutex<horo_rtc::call::VideoFeed>>,
    /// We rang the room; play ringback until someone joins or we hang up.
    ringing: bool,
    ringback: Option<crate::sounds::Ringback>,
    /// The screen, while it's shared.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub screen: Option<crate::screen::ScreenShare>,
    /// Our camera, while it's on (and its picture, for our own tile).
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub camera: Option<crate::camera::Camera>,
}

/// The Matrix user in a MatrixRTC identity: `@user:server:DEVICE` minus
/// the device. User ids have a colon of their own, so it's the last one.
pub fn user_of(identity: &str) -> &str {
    match identity.rsplit_once(':') {
        Some((user, device)) if user.contains(':') && !device.is_empty() => user,
        _ => identity,
    }
}

impl Store {
    /// Join the call in `room_id`, leaving any other first. Already in it,
    /// this does nothing.
    pub fn join_call(&mut self, room_id: String, cx: &mut Context<Self>) {
        if self.call.as_ref().is_some_and(|c| c.room_id == room_id && c.is_live()) {
            return;
        }
        self.leave_call(cx);
        self.stop_ringing(cx);
        let Ok(room) = OwnedRoomId::try_from(room_id.as_str()) else { return };
        self.call = Some(CallView {
            room_id: room_id.clone(),
            status: CallStatus::Connecting,
            muted: false,
            deafened: false,
            focus: FocusMode::Auto,
            fullscreen: false,
            camera: false,
            sharing: false,
            identity: None,
            participants: Vec::new(),
            participant_volumes: std::collections::HashMap::new(),
            mixer_open: false,
            pip_hidden: false,
            pip_corner: PipCorner::default(),
            pip_pos: None,
            pip_size: None,
            pip_drag_anchor: None,
            pip_resize_anchor: None,
            speaking: Vec::new(),
            self_speaking: false,
            loud_at: None,
            mic: None,
        });
        cx.notify();

        let gate_threshold = self.prefs.input_sensitivity;
        let devices = AudioDevices { input: self.prefs.audio_input.clone(), output: self.prefs.audio_output.clone(), cancel_echo: true };
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, room) = (backend.state.clone(), room.clone());
            async move {
                let core = state.core().await.map_err(|e| e.to_string())?;
                let credentials = horo_rtc::join(&core, &room, false).await.map_err(|e| e.to_string())?;
                let ringing = credentials.ringing;
                // A rejoin after the token expires gets a fresh one the way
                // the first came; our membership is still there, so this
                // rings nobody.
                let refresh: TokenSource = {
                    let (core, room) = (core.clone(), room.clone());
                    Arc::new(move || {
                        let (core, room) = (core.clone(), room.clone());
                        Box::pin(async move { horo_rtc::join(&core, &room, false).await.map(|c| c.jwt).map_err(|e| e.to_string()) })
                    })
                };
                let options = CallOptions { refresh_token: Some(refresh), gate_threshold, ..Default::default() };
                let joined = match Call::connect(&credentials.livekit_url, &credentials.jwt, options).await {
                    Ok(joined) => joined,
                    Err(e) => {
                        let _ = horo_rtc::leave(&core, &room).await;
                        return Err(format!("couldn't connect to the call: {e}"));
                    }
                };
                let Joined { call, microphone, speaker, events, screens, camera, screen } = joined;
                let audio = match devices::start(microphone, speaker, devices) {
                    Ok(audio) => audio,
                    Err(e) => {
                        call.leave().await;
                        let _ = horo_rtc::leave(&core, &room).await;
                        return Err(format!("couldn't start your microphone or speaker: {e}"));
                    }
                };
                Ok((call, audio, events, screens, camera, screen, ringing))
            }
        });

        cx.spawn(async move |this, cx| {
            let outcome = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            let Ok(Some(mut events)) = this.update(cx, |store, cx| {
                let still_wanted = store.call.as_ref().is_some_and(|c| c.room_id == room_id);
                match outcome {
                    Ok((call, audio, events, screens, feed, screen_feed, ringing)) if still_wanted => {
                        let backend = Backend::global(cx);
                        let refresher = backend.spawn({
                            let (state, room) = (backend.state.clone(), room.clone());
                            async move {
                                let mut every = tokio::time::interval(MEMBERSHIP_REFRESH);
                                every.tick().await;
                                loop {
                                    every.tick().await;
                                    let Ok(core) = state.core().await else { break };
                                    if let Err(e) = horo_rtc::refresh(&core, &room).await {
                                        tracing::warn!("refreshing the call membership: {e}");
                                    }
                                }
                            }
                        });
                        if let Some(view) = &mut store.call {
                            view.identity = Some(call.identity.clone());
                        }
                        for gone in audio.fallen_back() {
                            tracing::info!("the chosen {} wouldn't open ({}); using the system default", gone.which, gone.reason);
                            store.show_banner(
                                crate::store::Tone::Info,
                                format!("your chosen {} isn't available, so the call is using the system default.", gone.which),
                                cx,
                            );
                        }
                        store.call_live = Some(Live {
                            room_id: room,
                            call,
                            audio,
                            refresher: Some(refresher),
                            screens,
                            feed: Arc::new(Mutex::new(feed)),
                            screen_feed: Arc::new(Mutex::new(screen_feed)),
                            ringing,
                            ringback: None,
                            #[cfg(any(target_os = "macos", target_os = "linux"))]
                            camera: None,
                            #[cfg(any(target_os = "macos", target_os = "linux"))]
                            screen: None,
                        });
                        cx.notify();
                        Some(events)
                    }
                    // Left (or moved on) while it was connecting.
                    Ok((call, audio, _, screens, feed, screen_feed, _)) => {
                        store.hang_up(
                            Live {
                                room_id: room,
                                call,
                                audio,
                                refresher: None,
                                screens,
                                feed: Arc::new(Mutex::new(feed)),
                                screen_feed: Arc::new(Mutex::new(screen_feed)),
                                ringing: false,
                                ringback: None,
                                #[cfg(any(target_os = "macos", target_os = "linux"))]
                                camera: None,
                                #[cfg(any(target_os = "macos", target_os = "linux"))]
                                screen: None,
                            },
                            cx,
                        );
                        None
                    }
                    Err(error) => {
                        tracing::warn!("{error}");
                        if let Some(view) = store.call.as_mut().filter(|c| c.room_id == room_id) {
                            view.status = CallStatus::Failed(error);
                        }
                        cx.notify();
                        None
                    }
                }
            }) else {
                return;
            };
            while let Some(event) = events.recv().await {
                if this.update(cx, |store, cx| store.on_call_event(&room_id, event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_call_event(&mut self, room_id: &str, event: CallEvent, cx: &mut Context<Self>) {
        let Some(view) = self.call.as_mut().filter(|c| c.room_id == room_id) else { return };
        match event {
            CallEvent::Connected => {
                let first_connect = view.status != CallStatus::Connected;
                view.status = CallStatus::Connected;
                if first_connect {
                    crate::sounds::play_call_cue(crate::sounds::CallCue::Join, self.prefs.notification_volume, self.prefs.audio_output.clone());
                }
                // We rang them; play ringback until they pick up.
                if view.participants.is_empty()
                    && let Some(live) = &mut self.call_live
                    && live.ringing
                    && live.ringback.is_none()
                {
                    live.ringback = Some(crate::sounds::start_ringback(self.prefs.notification_volume, self.prefs.audio_output.clone()));
                }
            }
            CallEvent::Reconnected { .. } => view.status = CallStatus::Connected,
            CallEvent::Reconnecting { .. } => view.status = CallStatus::Reconnecting,
            CallEvent::Participants(participants) => {
                if !participants.is_empty()
                    && let Some(live) = &mut self.call_live
                {
                    // Someone picked up (or was already there): stop ringing.
                    live.ringing = false;
                    live.ringback = None;
                }
                if view.status == CallStatus::Connected {
                    let joined = participants.iter().any(|p| !view.participants.iter().any(|old| old.identity == p.identity));
                    let left = view.participants.iter().any(|old| !participants.iter().any(|p| p.identity == old.identity));
                    if joined {
                        crate::sounds::play_call_cue(crate::sounds::CallCue::PeerJoin, self.prefs.notification_volume, self.prefs.audio_output.clone());
                    } else if left {
                        crate::sounds::play_call_cue(crate::sounds::CallCue::PeerLeave, self.prefs.notification_volume, self.prefs.audio_output.clone());
                    }
                }
                // Once every screen share stops, return to Auto so the next one
                // takes focus automatically.
                if view.focus == FocusMode::Grid && !participants.iter().any(|p| p.screen) {
                    view.focus = FocusMode::Auto;
                }
                view.participants = participants;
            }
            CallEvent::Speaking(speaking) => {
                if view.speaking == speaking {
                    return;
                }
                view.speaking = speaking;
            }
            // Settings draws it every frame while it's open.
            // Settings draws it every frame while it's open; a redraw only
            // when our ring lights or goes out.
            CallEvent::Level(reading) => {
                view.mic = Some(reading);
                if !view.heard(reading, Instant::now()) {
                    return;
                }
            }
            CallEvent::Ended(reason) => {
                tracing::info!("call ended: {reason}");
                view.status = CallStatus::Failed(reason);
                if let Some(live) = self.call_live.take() {
                    self.hang_up(live, cx);
                }
            }
        }
        cx.notify();
    }

    /// Leave the call, if there is one.
    pub fn leave_call(&mut self, cx: &mut Context<Self>) {
        let was_live = self.call.as_ref().is_some_and(|c| c.is_live());
        self.call = None;
        self.call_volume_sliders.clear();
        self.call_participant_menu = None;
        if let Some(live) = self.call_live.take() {
            self.hang_up(live, cx);
        }
        if was_live {
            crate::sounds::play_call_cue(crate::sounds::CallCue::Leave, self.prefs.notification_volume, self.prefs.audio_output.clone());
        }
        cx.notify();
    }

    /// Stop the engine and the devices, and withdraw our membership so
    /// other clients stop showing us in the call.
    fn hang_up(&mut self, live: Live, cx: &mut Context<Self>) {
        // The call's pictures go with it.
        #[cfg(target_os = "linux")]
        crate::picture::forget_all(cx);
        let task = Self::hang_up_task(live, cx);
        cx.background_spawn(task).detach();
    }

    /// Hanging up as a future, for quitting, which waits for it.
    pub(crate) fn hang_up_task(live: Live, cx: &mut Context<Self>) -> impl std::future::Future<Output = ()> + use<> {
        let Live { room_id, call, audio, refresher, .. } = live;
        if let Some(refresher) = refresher {
            refresher.abort();
        }
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move {
                call.leave().await;
                drop(audio);
                if let Ok(core) = state.core().await
                    && let Err(e) = horo_rtc::leave(&core, &room_id).await
                {
                    tracing::warn!("withdrawing the call membership: {e}");
                }
            }
        });
        async move {
            let _ = task.await;
        }
    }

    pub fn set_call_muted(&mut self, muted: bool, cx: &mut Context<Self>) {
        let changed = self.call.as_ref().is_some_and(|v| v.muted != muted);
        if let Some(live) = &self.call_live {
            live.call.set_muted(muted);
        }
        if let Some(view) = &mut self.call {
            view.muted = muted;
            view.self_speaking &= !muted;
            view.loud_at = None;
        }
        if changed {
            let cue = if muted { crate::sounds::CallCue::Mute } else { crate::sounds::CallCue::Unmute };
            crate::sounds::play_call_cue(cue, self.prefs.notification_volume, self.prefs.audio_output.clone());
        }
        cx.notify();
    }

    /// Stop (or start again) hearing the call. Deafening mutes us too, as
    /// someone who can't hear shouldn't be heard; undeafening leaves the
    /// microphone as it is.
    pub fn set_call_deafened(&mut self, deafened: bool, cx: &mut Context<Self>) {
        let changed = self.call.as_ref().is_some_and(|v| v.deafened != deafened);
        if let Some(live) = &self.call_live {
            live.call.set_deafened(deafened);
        }
        if let Some(view) = &mut self.call {
            view.deafened = deafened;
            view.self_speaking &= !deafened;
            view.loud_at = None;
        }
        if deafened {
            if let Some(live) = &self.call_live {
                live.call.set_muted(true);
            }
            if let Some(view) = &mut self.call {
                view.muted = true;
            }
        }
        if changed {
            let cue = if deafened { crate::sounds::CallCue::Deafen } else { crate::sounds::CallCue::Undeafen };
            crate::sounds::play_call_cue(cue, self.prefs.notification_volume, self.prefs.audio_output.clone());
        }
        cx.notify();
    }

    /// Adjust a remote participant's local volume (`0.0`..=`2.0`) in the active call.
    pub fn set_participant_volume(&mut self, identity: String, volume: f32, cx: &mut Context<Self>) {
        let clamped = volume.clamp(0.0, 2.0);
        if let Some(live) = &self.call_live {
            live.call.set_participant_volume(&identity, clamped);
        }
        if let Some(view) = &mut self.call {
            if (clamped - 1.0).abs() < 0.001 {
                view.participant_volumes.remove(&identity);
            } else {
                view.participant_volumes.insert(identity, clamped);
            }
        }
        cx.notify();
    }

    /// Move the call to a microphone or speaker just chosen (`before` is
    /// what prefs said until now). One that won't open leaves the call on
    /// the old one, and says so.
    pub(crate) fn follow_audio_devices(&mut self, before: (Option<String>, Option<String>), cx: &mut Context<Self>) {
        let Some(live) = &mut self.call_live else { return };
        let (input, output) = (self.prefs.audio_input.clone(), self.prefs.audio_output.clone());
        let mut failed = Vec::new();
        if input != before.0
            && let Err(e) = live.audio.set_input(input)
        {
            failed.push(format!("couldn't switch microphones: {e}"));
        }
        if output != before.1
            && let Err(e) = live.audio.set_output(output)
        {
            failed.push(format!("couldn't switch speakers: {e}"));
        }
        for why in failed {
            tracing::warn!("{why}");
            self.show_banner(crate::store::Tone::Error, why, cx);
        }
    }

    /// Move the noise gate's threshold: in the call now, and (`save`) for
    /// calls to come.
    pub fn set_input_sensitivity(&mut self, db: f32, save: bool, cx: &mut Context<Self>) {
        if let Some(live) = &self.call_live {
            live.call.set_gate_threshold(db);
        }
        if save {
            self.update_prefs(|p| p.input_sensitivity = db, cx);
        } else {
            self.preview_prefs(|p| p.input_sensitivity = db, cx);
        }
    }

    /// Show a tile big, or go back to everyone alike.
    pub fn set_call_focus(&mut self, focus: FocusMode, cx: &mut Context<Self>) {
        if let Some(view) = &mut self.call {
            view.focus = focus;
            cx.notify();
        }
    }

    pub fn set_call_fullscreen(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Some(view) = &mut self.call {
            view.fullscreen = on;
            cx.notify();
        }
    }

    /// Get or create the draggable volume slider state (`0.0..=2.0`) for a remote participant.
    pub fn call_volume_slider(&mut self, identity: &str, cx: &mut Context<Self>) -> gpui::Entity<gpui_component::slider::SliderState> {
        if let Some(existing) = self.call_volume_sliders.get(identity) {
            return existing.clone();
        }
        let vol = self
            .call
            .as_ref()
            .and_then(|c| c.participant_volumes.get(identity).copied())
            .unwrap_or(1.0);
        let slider = cx.new(|_| gpui_component::slider::SliderState::new().min(0.0).max(2.0).step(0.01).default_value(vol));
        let id = identity.to_owned();
        cx.subscribe(&slider, move |store, _, event: &gpui_component::slider::SliderEvent, cx| {
            let val = match event {
                gpui_component::slider::SliderEvent::Change(v) | gpui_component::slider::SliderEvent::Release(v) => v.start(),
            };
            store.set_participant_volume(id.clone(), val, cx);
        })
        .detach();
        self.call_volume_sliders.insert(identity.to_owned(), slider.clone());
        slider
    }

    pub fn open_participant_menu(&mut self, menu: ParticipantMenu, cx: &mut Context<Self>) {
        let _ = self.call_volume_slider(&menu.identity, cx);
        self.call_participant_menu = Some(menu);
        cx.notify();
    }

    pub fn close_participant_menu(&mut self, cx: &mut Context<Self>) {
        if self.call_participant_menu.take().is_some() {
            cx.notify();
        }
    }

    pub fn open_call_mixer(&mut self, cx: &mut Context<Self>) {
        self.call_participant_menu = None;
        if let Some(view) = &mut self.call {
            view.mixer_open = true;
            cx.notify();
        }
    }

    pub fn toggle_call_mixer(&mut self, cx: &mut Context<Self>) {
        self.call_participant_menu = None;
        if let Some(view) = &mut self.call {
            view.mixer_open = !view.mixer_open;
            cx.notify();
        }
    }

    pub fn set_call_pip_hidden(&mut self, hidden: bool, cx: &mut Context<Self>) {
        if let Some(view) = &mut self.call {
            view.pip_hidden = hidden;
            cx.notify();
        }
    }

    pub fn begin_call_pip_drag(&mut self, mouse: (f32, f32), viewport: (f32, f32)) {
        if let Some(view) = &mut self.call {
            let size = view.pip_size.unwrap_or(PIP_DEFAULT_SIZE);
            let raw_pos = view.pip_pos.unwrap_or_else(|| pip_corner_pos(view.pip_corner, size, viewport));
            let (rb, _) = clamp_pip_rect(raw_pos, size, viewport);
            view.pip_drag_anchor = Some((mouse, rb));
        }
    }

    pub fn move_call_pip_drag(&mut self, mouse: (f32, f32), viewport: (f32, f32), cx: &mut Context<Self>) {
        if let Some(view) = &mut self.call
            && let Some((start_mouse, start_rb)) = view.pip_drag_anchor
        {
            let size = view.pip_size.unwrap_or(PIP_DEFAULT_SIZE);
            let dx = mouse.0 - start_mouse.0;
            let dy = mouse.1 - start_mouse.1;
            let (rb, _) = clamp_pip_rect((start_rb.0 - dx, start_rb.1 - dy), size, viewport);
            view.pip_pos = Some(rb);
            cx.notify();
        }
    }

    /// Snap the floating call window into whichever of the four corners is closest.
    pub fn end_call_pip_drag(&mut self, viewport: (f32, f32), cx: &mut Context<Self>) {
        if let Some(view) = &mut self.call {
            if let Some(pos) = view.pip_pos.take() {
                let (_, size) = clamp_pip_rect(pos, view.pip_size.unwrap_or(PIP_DEFAULT_SIZE), viewport);
                view.pip_corner = nearest_pip_corner(pos, size, viewport);
            }
            view.pip_drag_anchor = None;
            cx.notify();
        }
    }

    pub fn begin_call_pip_resize(&mut self, mouse: (f32, f32), viewport: (f32, f32)) {
        if let Some(view) = &mut self.call {
            let size = view.pip_size.unwrap_or(PIP_DEFAULT_SIZE);
            let raw_pos = view.pip_pos.unwrap_or_else(|| pip_corner_pos(view.pip_corner, size, viewport));
            let (rb, size) = clamp_pip_rect(raw_pos, size, viewport);
            view.pip_resize_anchor = Some((mouse, rb, size));
        }
    }

    pub fn move_call_pip_resize(&mut self, edge: PipResizeEdge, mouse: (f32, f32), viewport: (f32, f32), cx: &mut Context<Self>) {
        if let Some(view) = &mut self.call
            && let Some((start_mouse, start_rb, start_size)) = view.pip_resize_anchor
        {
            let (rb, size) = resize_pip_rect(edge, start_mouse, start_rb, start_size, mouse, viewport);
            view.pip_size = Some(size);
            view.pip_pos = Some(rb);
            cx.notify();
        }
    }

    pub fn end_call_pip_resize(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = &mut self.call {
            view.pip_pos = None;
            view.pip_resize_anchor = None;
            cx.notify();
        }
    }

    pub fn close_call_mixer(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = &mut self.call
            && view.mixer_open
        {
            view.mixer_open = false;
            cx.notify();
        }
    }

    pub fn reset_participant_volumes(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .call
            .as_ref()
            .map(|v| v.participant_volumes.keys().cloned().collect())
            .unwrap_or_default();
        if let Some(live) = &self.call_live {
            for id in &ids {
                live.call.set_participant_volume(id, 1.0);
            }
        }
        if let Some(view) = &mut self.call {
            view.participant_volumes.clear();
        }
        cx.notify();
    }

    /// Move an active call's camera to a newly chosen device (`before` is
    /// what prefs said until now). Drops the old camera first so hardware /
    /// PipeWire releases it before the new one opens.
    pub(crate) fn follow_video_device(&mut self, before: Option<String>, cx: &mut Context<Self>) {
        if self.prefs.video_input == before {
            return;
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if self.call_live.is_some() && self.call.as_ref().is_some_and(|c| c.camera) {
            if let Some(live) = &mut self.call_live {
                live.camera = None;
            }
            self.set_call_camera(true, cx);
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let _ = cx;
    }

    /// Turn our camera on or off in the call. It opens off the UI thread
    /// (that takes a moment); each picture goes to the encoder from the
    /// camera's own queue.
    #[cfg(target_os = "macos")]
    pub fn set_call_camera(&mut self, on: bool, cx: &mut Context<Self>) {
        tracing::info!("camera {} (call live: {})", if on { "on" } else { "off" }, self.call_live.is_some());
        let Some(live) = &mut self.call_live else { return };
        if let Some(view) = &mut self.call {
            view.camera = on;
        }
        live.call.set_camera(on);
        if !on {
            live.camera = None;
            cx.notify();
            return;
        }
        let device = self.prefs.video_input.clone();
        let wanted_device = device.clone();
        let feed = live.feed.clone();
        let task = cx.background_spawn(async move {
            let first = std::sync::atomic::AtomicBool::new(true);
            let on_frame: crate::camera::OnFrame = Box::new(move |buffer| {
                if first.swap(false, std::sync::atomic::Ordering::Relaxed) {
                    tracing::info!("camera: first picture");
                }
                if let Some(frame) = crate::camera::to_i420(buffer)
                    && let Ok(mut feed) = feed.lock()
                {
                    feed.push(&frame);
                }
            });
            crate::camera::Camera::start_with(device.as_deref(), Some(on_frame))
        });
        cx.spawn(async move |this, cx| {
            let opened = task.await;
            let _ = this.update(cx, |store, cx| {
                let wanted = store.call.as_ref().is_some_and(|c| c.camera) && store.prefs.video_input == wanted_device;
                match &opened {
                    Ok(_) => tracing::info!("camera opened (still wanted: {wanted})"),
                    Err(e) => tracing::warn!("camera didn't open: {e}"),
                }
                match opened {
                    Ok(camera) if wanted => {
                        if let Some(live) = &mut store.call_live {
                            live.camera = Some(camera);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        if let Some(live) = &store.call_live {
                            live.call.set_camera(false);
                        }
                        if let Some(view) = &mut store.call {
                            view.camera = false;
                        }
                        store.show_banner(crate::store::Tone::Error, format!("couldn't open the camera: {e}"), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Share the main screen in the call, or stop. Starting asks macOS,
    /// which can take a moment (and fails until screen recording is
    /// allowed): off the UI thread.
    #[cfg(target_os = "macos")]
    pub fn set_call_screen(&mut self, on: bool, cx: &mut Context<Self>) {
        tracing::info!("screen share {}", if on { "on" } else { "off" });
        let Some(live) = &mut self.call_live else { return };
        if let Some(view) = &mut self.call {
            view.sharing = on;
        }
        live.call.set_screen(on);
        if !on {
            live.screen = None;
            crate::screen::done_picking();
            cx.notify();
            return;
        }
        let feed = live.screen_feed.clone();
        // The system's picker first: a window, an app or a display.
        let picked = crate::screen::pick();
        cx.spawn(async move |this, cx| {
            let picked = match picked.await {
                Ok(Ok(Some(picked))) => picked,
                // Cancelled: nothing's shared.
                Ok(Ok(None)) | Err(_) => {
                    crate::screen::done_picking();
                    let _ = this.update(cx, |store, cx| store.set_call_screen(false, cx));
                    return;
                }
                Ok(Err(e)) => {
                    crate::screen::done_picking();
                    let _ = this.update(cx, |store, cx| {
                        store.set_call_screen(false, cx);
                        store.show_banner(crate::store::Tone::Error, format!("couldn't share: {e}"), cx);
                    });
                    return;
                }
            };
            let task = cx.background_spawn(async move {
                let on_frame: crate::screen::OnFrame = Box::new(move |buffer| {
                    if let Some(frame) = crate::camera::to_i420(buffer)
                        && let Ok(mut feed) = feed.lock()
                    {
                        feed.push(&frame);
                    }
                });
                crate::screen::ScreenShare::start(picked, on_frame)
            });
            let started = task.await;
            let _ = this.update(cx, |store, cx| {
                let wanted = store.call.as_ref().is_some_and(|c| c.sharing);
                match started {
                    Ok(share) if wanted => {
                        if let Some(live) = &mut store.call_live {
                            live.screen = Some(share);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("{e}");
                        if let Some(live) = &store.call_live {
                            live.call.set_screen(false);
                        }
                        if let Some(view) = &mut store.call {
                            view.sharing = false;
                        }
                        store.show_banner(crate::store::Tone::Error, e, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Turn our camera on or off in the call, on Linux: a PipeWire camera,
    /// opened off the UI thread; each picture goes to the encoder from the
    /// capture's own thread.
    #[cfg(target_os = "linux")]
    pub fn set_call_camera(&mut self, on: bool, cx: &mut Context<Self>) {
        tracing::info!("camera {} (call live: {})", if on { "on" } else { "off" }, self.call_live.is_some());
        let Some(live) = &mut self.call_live else { return };
        if let Some(view) = &mut self.call {
            view.camera = on;
        }
        live.call.set_camera(on);
        if !on {
            live.camera = None;
            crate::picture::forget(OWN_CAMERA, cx);
            cx.notify();
            return;
        }
        let device = self.prefs.video_input.clone();
        let wanted_device = device.clone();
        let feed = live.feed.clone();
        let task = cx.background_spawn(async move {
            let first = std::sync::atomic::AtomicBool::new(true);
            let on_frame: crate::camera::OnFrame = Box::new(move |frame| {
                if first.swap(false, std::sync::atomic::Ordering::Relaxed) {
                    tracing::info!("camera: first picture ({}x{})", frame.width, frame.height);
                }
                if let Ok(mut feed) = feed.lock() {
                    feed.push(frame);
                }
            });
            crate::camera::Camera::start_with(device.as_deref(), Some(on_frame))
        });
        cx.spawn(async move |this, cx| {
            let opened = task.await;
            let _ = this.update(cx, |store, cx| {
                let wanted = store.call.as_ref().is_some_and(|c| c.camera) && store.prefs.video_input == wanted_device;
                match opened {
                    Ok(camera) if wanted => {
                        if let Some(live) = &mut store.call_live {
                            live.camera = Some(camera);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("camera didn't open: {e}");
                        if let Some(live) = &store.call_live {
                            live.call.set_camera(false);
                        }
                        if let Some(view) = &mut store.call {
                            view.camera = false;
                        }
                        store.show_banner(crate::store::Tone::Error, format!("couldn't open the camera: {e}"), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Share a screen or window in the call, or stop, on Linux: the desktop
    /// portal asks which (its picker waits on the user), off the UI thread.
    #[cfg(target_os = "linux")]
    pub fn set_call_screen(&mut self, on: bool, cx: &mut Context<Self>) {
        tracing::info!("screen share {}", if on { "on" } else { "off" });
        let Some(live) = &mut self.call_live else { return };
        if let Some(view) = &mut self.call {
            view.sharing = on;
        }
        live.call.set_screen(on);
        if !on {
            live.screen = None;
            cx.notify();
            return;
        }
        let feed = live.screen_feed.clone();
        let task = cx.background_spawn(async move {
            let on_frame: crate::screen::OnFrame = Box::new(move |frame| {
                if let Ok(mut feed) = feed.lock() {
                    feed.push(&frame);
                }
            });
            crate::screen::ScreenShare::start(on_frame).await
        });
        cx.spawn(async move |this, cx| {
            let started = task.await;
            let _ = this.update(cx, |store, cx| {
                let wanted = store.call.as_ref().is_some_and(|c| c.sharing);
                match started {
                    Ok(Some(share)) if wanted => {
                        if let Some(live) = &mut store.call_live {
                            live.screen = Some(share);
                        }
                    }
                    // Stopped meanwhile: dropping it ends the share.
                    Ok(Some(_)) => {}
                    // Cancelled in the picker: nothing's shared.
                    Ok(None) => store.set_call_screen(false, cx),
                    Err(e) => {
                        tracing::warn!("{e}");
                        store.set_call_screen(false, cx);
                        store.show_banner(crate::store::Tone::Error, e, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Dismiss a failed call's notice.
    pub fn clear_failed_call(&mut self, cx: &mut Context<Self>) {
        if self.call.as_ref().is_some_and(|c| !c.is_live()) {
            self.call = None;
            cx.notify();
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_lose_their_device_not_their_server() {
        assert_eq!(user_of("@alice:example.org:ABCDEF"), "@alice:example.org");
        assert_eq!(user_of("@alice:example.org"), "@alice:example.org");
        assert_eq!(user_of("@bob:matrix.org:XYZ"), "@bob:matrix.org");
    }

    #[test]
    fn pip_resize_keeps_opposite_edges_anchored() {
        let vp = (1200., 800.);
        let start_rb = (100., 100.);
        let start_size = (360., 296.);
        let start_mouse = (500., 500.);

        // Dragging Left edge 40px left widens by 40px without moving right edge or height.
        let (rb, sz) = resize_pip_rect(PipResizeEdge::Left, start_mouse, start_rb, start_size, (460., 450.), vp);
        assert_eq!(rb, (100., 100.));
        assert_eq!(sz, (400., 296.));

        // Dragging Right edge 40px right widens by 40px and shifts right inset by -40px so left edge stays fixed.
        let (rb, sz) = resize_pip_rect(PipResizeEdge::Right, start_mouse, start_rb, start_size, (540., 450.), vp);
        assert_eq!(rb, (60., 100.));
        assert_eq!(sz, (400., 296.));

        // Dragging Top edge 50px up grows height by 50px without moving bottom edge or width.
        let (rb, sz) = resize_pip_rect(PipResizeEdge::Top, start_mouse, start_rb, start_size, (550., 450.), vp);
        assert_eq!(rb, (100., 100.));
        assert_eq!(sz, (360., 346.));

        // Dragging Bottom edge 50px down grows height by 50px and shifts bottom inset by -50px so top edge stays fixed.
        let (rb, sz) = resize_pip_rect(PipResizeEdge::Bottom, start_mouse, start_rb, start_size, (550., 550.), vp);
        assert_eq!(rb, (100., 50.));
        assert_eq!(sz, (360., 346.));

        // Dragging BottomRight past the viewport right/bottom margin clamps without sliding the opposite edges.
        let (rb, sz) = resize_pip_rect(PipResizeEdge::BottomRight, start_mouse, start_rb, start_size, (900., 900.), vp);
        assert_eq!(rb, (PIP_MARGIN, PIP_MARGIN));
        assert_eq!(sz, (start_size.0 + start_rb.0 - PIP_MARGIN, start_size.1 + start_rb.1 - PIP_MARGIN));

        // Shrinking below minimum clamps cleanly at PIP_MIN_SIZE.
        let (rb, sz) = resize_pip_rect(PipResizeEdge::TopLeft, start_mouse, start_rb, start_size, (900., 900.), vp);
        assert_eq!(rb, start_rb);
        assert_eq!(sz, PIP_MIN_SIZE);
    }

    #[test]
    fn pip_snaps_to_nearest_of_four_corners() {
        let vp = (1200., 800.);
        let sz = PIP_DEFAULT_SIZE;

        for corner in [PipCorner::TopLeft, PipCorner::TopRight, PipCorner::BottomLeft, PipCorner::BottomRight] {
            let pos = pip_corner_pos(corner, sz, vp);
            assert_eq!(nearest_pip_corner(pos, sz, vp), corner);
        }

        // Dragging past the horizontal midpoint from BottomRight switches to BottomLeft.
        assert_eq!(nearest_pip_corner((500., 18.), sz, vp), PipCorner::BottomLeft);
        // Dragging into the upper-right quadrant switches to TopRight.
        assert_eq!(nearest_pip_corner((40., 450.), sz, vp), PipCorner::TopRight);
    }
}
