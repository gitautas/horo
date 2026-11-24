//! Voice calls in the app, on horo's own engine (`horo_rtc::call`), the way
//! src/lib/call.ts runs them in the WebView build: one call at a time,
//! joining another leaves the first; MatrixRTC membership through
//! `horo_rtc::matrixrtc`, refreshed hourly, withdrawn on leaving.
//!
//! The store holds what the UI draws ([`CallView`]) and the engine's
//! handles ([`Live`]); the engine's events come back through a channel and
//! are folded into the view on gpui's executor. Audio only for now: the
//! engine has no video yet.

use std::{sync::{Arc, Mutex}, time::Duration};

use gpui::{AppContext as _, Context};
use horo_rtc::call::{
    Call, CallEvent, CallOptions, Joined, Participant, TokenSource,
    devices::{self, AudioDevices, RunningAudio},
};
use matrix_sdk::ruma::OwnedRoomId;

use crate::{backend::Backend, store::Store};

/// A membership advertisement lasts four hours; refresh well inside that.
const MEMBERSHIP_REFRESH: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallStatus {
    Connecting,
    Connected,
    Reconnecting,
    Failed(String),
}

/// The call, as the UI draws it.
#[derive(Debug, Clone)]
pub struct CallView {
    pub room_id: String,
    pub status: CallStatus,
    pub muted: bool,
    /// Our camera is on.
    pub camera: bool,
    /// We're sharing our screen.
    pub sharing: bool,
    /// Our identity in the room (`@user:server:DEVICE`), once connected.
    pub identity: Option<String>,
    /// Everyone else.
    pub participants: Vec<Participant>,
    /// Identities speaking now, ourselves included.
    pub speaking: Vec<String>,
}

impl CallView {
    pub fn is_live(&self) -> bool {
        !matches!(self.status, CallStatus::Failed(_))
    }

    /// Whether `user_id` is talking (any of their devices).
    pub fn is_speaking(&self, user_id: &str) -> bool {
        self.speaking.iter().any(|identity| user_of(identity) == user_id)
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
    /// The screen, while it's shared.
    #[cfg(target_os = "macos")]
    pub screen: Option<crate::screen::ScreenShare>,
    /// Our camera, while it's on (and its picture, for our own tile).
    #[cfg(target_os = "macos")]
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
            camera: false,
            sharing: false,
            identity: None,
            participants: Vec::new(),
            speaking: Vec::new(),
        });
        cx.notify();

        let devices = AudioDevices { input: self.prefs.audio_input.clone(), output: self.prefs.audio_output.clone(), cancel_echo: true };
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, room) = (backend.state.clone(), room.clone());
            async move {
                let core = state.core().await.map_err(|e| e.to_string())?;
                let credentials = horo_rtc::join(&core, &room, false).await.map_err(|e| e.to_string())?;
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
                let options = CallOptions { refresh_token: Some(refresh), ..Default::default() };
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
                Ok((call, audio, events, screens, camera, screen))
            }
        });

        cx.spawn(async move |this, cx| {
            let outcome = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            let Ok(Some(mut events)) = this.update(cx, |store, cx| {
                let still_wanted = store.call.as_ref().is_some_and(|c| c.room_id == room_id);
                match outcome {
                    Ok((call, audio, events, screens, feed, screen_feed)) if still_wanted => {
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
                        store.call_live = Some(Live {
                            room_id: room,
                            call,
                            audio,
                            refresher: Some(refresher),
                            screens,
                            feed: Arc::new(Mutex::new(feed)),
                            screen_feed: Arc::new(Mutex::new(screen_feed)),
                            #[cfg(target_os = "macos")]
                            camera: None,
                            #[cfg(target_os = "macos")]
                            screen: None,
                        });
                        cx.notify();
                        Some(events)
                    }
                    // Left (or moved on) while it was connecting.
                    Ok((call, audio, _, screens, feed, screen_feed)) => {
                        store.hang_up(
                            Live {
                                room_id: room,
                                call,
                                audio,
                                refresher: None,
                                screens,
                                feed: Arc::new(Mutex::new(feed)),
                                screen_feed: Arc::new(Mutex::new(screen_feed)),
                                #[cfg(target_os = "macos")]
                                camera: None,
                                #[cfg(target_os = "macos")]
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
            CallEvent::Connected | CallEvent::Reconnected { .. } => view.status = CallStatus::Connected,
            CallEvent::Reconnecting { .. } => view.status = CallStatus::Reconnecting,
            CallEvent::Participants(participants) => view.participants = participants,
            CallEvent::Speaking(speaking) => {
                if view.speaking == speaking {
                    return;
                }
                view.speaking = speaking;
            }
            // The level meter isn't drawn yet.
            CallEvent::Level(_) => return,
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
        self.call = None;
        if let Some(live) = self.call_live.take() {
            self.hang_up(live, cx);
        }
        cx.notify();
    }

    /// Stop the engine and the devices, and withdraw our membership so
    /// other clients stop showing us in the call.
    fn hang_up(&mut self, live: Live, cx: &mut Context<Self>) {
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
        if let Some(live) = &self.call_live {
            live.call.set_muted(muted);
        }
        if let Some(view) = &mut self.call {
            view.muted = muted;
        }
        cx.notify();
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
                let wanted = store.call.as_ref().is_some_and(|c| c.camera);
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
            cx.notify();
            return;
        }
        let feed = live.screen_feed.clone();
        let task = cx.background_spawn(async move {
            let on_frame: crate::screen::OnFrame = Box::new(move |buffer| {
                if let Some(frame) = crate::camera::to_i420(buffer)
                    && let Ok(mut feed) = feed.lock()
                {
                    feed.push(&frame);
                }
            });
            crate::screen::ScreenShare::start(on_frame)
        });
        cx.spawn(async move |this, cx| {
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
}
