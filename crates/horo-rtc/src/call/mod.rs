//! horo's call engine: one participant in a LiveKit room, built from horo's
//! own pieces.
//!
//! [`Call::connect`] joins with [`crate::livekit`], opens the two peer
//! connections LiveKit wants ([`crate::peer`]), publishes a microphone track
//! and subscribes to everyone else's. What it hands back is UI- and
//! device-agnostic:
//!
//! - a [`Microphone`] to push 20 ms frames into (gate → Opus → publisher),
//! - a [`Speaker`] to pull 20 ms frames from (subscriber → one jitter
//!   buffer per remote track → mixer),
//! - [`CallEvent`]s for the UI: connected, who's here, who's speaking, the
//!   microphone's level, and the end.
//!
//! [`devices::start`] wires the two to real hardware: cpal devices with
//! horo's software echo canceller, the same on every platform. Tests wire
//! them to tones instead.

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use crate::livekit::{AddTrack, SignalClient, SignalEvent, Target, proto, proto::ReconnectReason};
use crate::{
    IceConnectionState, MediaKind, Mid, Peer, PeerConfig, PeerEvent, TurnServer,
    peer::PATH_TIMEOUT,
    audio::{
        FRAME,
        codec::Encoder,
        echo::EchoCanceller,
        gate::{GATE_OFF_DB, GateReading, NoiseGate},
        jitter::{JitterBuffer, JitterStats},
        mixer::mix,
    },
};
use tokio::sync::mpsc;

pub mod devices;
pub mod video;

pub use video::{Picture, Screens, Source, VideoFeed, screen_key};

/// The client track id of our microphone, echoed back by the server.
const MIC_CID: &str = "microphone";
/// And of our camera, and our shared screen.
const CAMERA_CID: &str = "camera";
const SCREEN_CID: &str = "screen";
/// The screen size we tell the server we publish.
const SCREEN_SIZE: (u32, u32) = (1920, 1080);
/// The camera size we tell the server we publish (what it plans layers by).
const CAMERA_SIZE: (u32, u32) = (1280, 720);
/// Our camera's simulcast layers, smallest first, as LiveKit names them:
/// a quarter, half and the whole picture.
pub const CAMERA_LAYERS: [&str; 3] = ["q", "h", "f"];
/// A remote track silent this long is left out of the mix; when it speaks
/// again its jitter buffer starts afresh rather than "catching up".
const IDLE: Duration = Duration::from_secs(1);

#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("signalling: {0}")]
    Signal(#[from] crate::livekit::Error),
    #[error("media: {0}")]
    Peer(#[from] crate::PeerError),
    #[error("codec: {0}")]
    Codec(#[from] crate::audio::codec::CodecError),
    #[error("devices: {0}")]
    Devices(#[from] crate::devices::DeviceError),
}

pub type Result<T, E = CallError> = std::result::Result<T, E>;

/// How to reach the SFU: which candidates each peer connection gathers,
/// given the TURN servers LiveKit offered.
pub type Network = Arc<dyn Fn(&str, &[TurnServer]) -> PeerConfig + Send + Sync>;

/// Host UDP on the default route, ICE-TCP, and every TURN server LiveKit
/// offers: what a client on an unknown network wants. Asked afresh on every
/// reconnect, so a new default route (Wi-Fi to Ethernet) is picked up.
pub fn default_network() -> Network {
    Arc::new(|name, turn| PeerConfig { turn_servers: turn.to_vec(), ..PeerConfig::default_for(name) })
}

/// A fresh access token, for a rejoin after the call's own has expired
/// (MatrixRTC's last about an hour).
pub type TokenSource = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>> + Send + Sync>;

pub struct CallOptions {
    pub network: Network,
    /// The noise gate's threshold in dBFS; [`GATE_OFF_DB`] never cuts.
    pub gate_threshold: f32,
    /// Start muted.
    pub muted: bool,
    /// Where a rejoin gets a new token; without one it reuses the first.
    pub refresh_token: Option<TokenSource>,
}

impl Default for CallOptions {
    fn default() -> Self {
        CallOptions { network: default_network(), gate_threshold: GATE_OFF_DB, muted: false, refresh_token: None }
    }
}

/// Someone else in the room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Participant {
    /// LiveKit's id for this session in the room.
    pub sid: String,
    /// Who they are: in a MatrixRTC call, `@user:server:DEVICE`.
    pub identity: String,
    pub name: String,
    /// They have no live microphone track (muted, or listen-only).
    pub muted: bool,
    /// Their camera is on.
    pub camera: bool,
    /// The server's id for their camera track, if they publish one.
    pub camera_sid: Option<String>,
    /// They're sharing their screen, and the server's id for that track.
    pub screen: bool,
    pub screen_sid: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallEvent {
    /// Our media connection is up: the microphone is being sent.
    Connected,
    /// Everyone else in the room, whenever that changes.
    Participants(Vec<Participant>),
    /// The identities of who is speaking now, loudest first (ourselves
    /// included, under our own identity).
    Speaking(Vec<String>),
    /// The microphone's level, about 30 times a second.
    Level(GateReading),
    /// The connection dropped and the engine is getting it back; audio stops
    /// meanwhile. `attempt` counts from 1.
    Reconnecting { attempt: u32, reason: String },
    /// Back. `resumed`: the session survived and nobody saw us go;
    /// otherwise we rejoined as a new participant.
    Reconnected { resumed: bool },
    /// The call is over; nothing follows.
    Ended(String),
}

enum Command {
    Mute(bool),
    Video(Source, bool),
    Reconnect,
    Sever { signalling: bool },
    KillPublisher,
    Leave,
}

/// The remote side of the mix: one jitter buffer per subscribed track, plus
/// per-participant local gain (0.0 = locally muted, 1.0 = normal, up to 2.0).
#[derive(Default)]
struct Remotes {
    tracks: HashMap<Mid, Remote>,
    volumes: HashMap<String, f32>,
}

struct Remote {
    buffer: JitterBuffer,
    last_packet: Instant,
    identity: Option<String>,
    /// When this remote track last sounded like speech (for immediate local
    /// speaking-ring detection independent of server `SpeakersChanged`).
    loud_at: Option<Instant>,
}

/// Speech threshold (dBFS) for RTP voice-activity and local remote speaker detection.
const SPEAKING_DB: f32 = -50.0;
/// Hold time so speaking indicators don't flicker between words.
const SPEAKING_HOLD: Duration = Duration::from_millis(250);

/// RMS loudness of a 20 ms PCM frame in dBFS (`-127.0..=0.0`).
fn frame_dbfs(frame: &[f32; FRAME]) -> f32 {
    let mean_sq = frame.iter().map(|&s| s * s).sum::<f32>() / FRAME as f32;
    let rms = mean_sq.sqrt();
    if rms > 1e-6 { (20.0 * rms.log10()).clamp(-127.0, 0.0) } else { -127.0 }
}

/// A joined call. Dropping it leaves.
pub struct Call {
    commands: mpsc::UnboundedSender<Command>,
    muted: Arc<AtomicBool>,
    deafened: Arc<AtomicBool>,
    remotes: Arc<Mutex<Remotes>>,
    /// The noise gate's threshold (dBFS, as f32 bits), read by the
    /// microphone each frame.
    gate: Arc<AtomicU32>,
    /// Our own identity in the room.
    pub identity: String,
    task: tokio::task::JoinHandle<()>,
}

/// Everything [`Call::connect`] hands back.
pub struct Joined {
    pub call: Call,
    pub microphone: Microphone,
    pub speaker: Speaker,
    pub events: mpsc::UnboundedReceiver<CallEvent>,
    /// Everyone's camera, newest picture each.
    pub screens: Screens,
    /// Where our own camera's pictures go, once [`Call::set_camera`]
    /// has turned it on.
    pub camera: VideoFeed,
    /// And our screen's, once [`Call::set_screen`] has.
    pub screen: VideoFeed,
}

/// The connection to the room: signalling and both peers, with their event
/// streams. A rejoin replaces all of it; a resume replaces the signalling.
struct Link {
    signal: SignalClient,
    signal_events: mpsc::UnboundedReceiver<SignalEvent>,
    publisher: Peer,
    pub_events: mpsc::UnboundedReceiver<PeerEvent>,
    subscriber: Peer,
    sub_events: mpsc::UnboundedReceiver<PeerEvent>,
    mic_mid: Mid,
}

fn turn_servers(servers: &[proto::IceServer]) -> Vec<TurnServer> {
    servers
        .iter()
        .flat_map(|s| s.urls.iter().map(|uri| TurnServer { uri: uri.clone(), username: s.username.clone(), credential: s.credential.clone() }))
        .filter(|s| s.uri.starts_with("turn"))
        .collect()
}

/// Join from scratch: signalling, both peers, the microphone track offered.
async fn open_link(url: &str, token: &str, network: &Network, muted: bool) -> Result<(Link, proto::JoinResponse, Vec<TurnServer>)> {
    let (signal, join, signal_events) = crate::livekit::connect(url, token, true).await?;
    let identity = join.participant.as_ref().map(|p| p.identity.clone()).unwrap_or_default();
    let turn = turn_servers(&join.ice_servers);
    let (publisher, pub_events) = Peer::start(network(&format!("{identity}/pub"), &turn)).await?;
    let (subscriber, sub_events) = Peer::start(network(&format!("{identity}/sub"), &turn)).await?;
    signal.add_track(AddTrack {
        cid: MIC_CID.into(),
        name: MIC_CID.into(),
        kind: proto::TrackType::Audio,
        source: proto::TrackSource::Microphone,
        muted,
        width: 0,
        height: 0,
        layers: Vec::new(),
    })?;
    let (sdp, mic_mid) = publisher.offer_audio(&format!("{identity}-mic"), MIC_CID).await?;
    signal.offer(sdp, 0)?;
    Ok((Link { signal, signal_events, publisher, pub_events, subscriber, sub_events, mic_mid }, join, turn))
}

impl Call {
    /// Join the LiveKit room `token` grants, at `livekit_url`.
    pub async fn connect(livekit_url: &str, token: &str, options: CallOptions) -> Result<Joined> {
        let (link, join, turn) = open_link(livekit_url, token, &options.network, options.muted).await?;
        let identity = join.participant.as_ref().map(|p| p.identity.clone()).unwrap_or_default();

        let (events_tx, events) = mpsc::unbounded_channel();
        let (packets_tx, packets) = mpsc::unbounded_channel();
        let (local_speaking_tx, local_speaking_rx) = mpsc::unbounded_channel();
        let (commands, commands_rx) = mpsc::unbounded_channel();
        let muted = Arc::new(AtomicBool::new(options.muted));
        let remotes = Arc::new(Mutex::new(Remotes::default()));
        let screens = Screens::default();
        let (broken_tx, broken) = mpsc::unbounded_channel();
        let (video_tx, video_packets) = mpsc::unbounded_channel::<video::VideoPacket>();
        let outgoing = [Outgoing::new(Source::Camera), Outgoing::new(Source::Screen)];
        let camera = VideoFeed::new(Source::Camera, video_tx.clone(), outgoing[0].on.clone(), outgoing[0].keyframe.clone());
        let screen = VideoFeed::new(Source::Screen, video_tx, outgoing[1].on.clone(), outgoing[1].keyframe.clone());

        let mut engine = Engine {
            url: livekit_url.to_owned(),
            token: token.to_owned(),
            network: options.network,
            refresh_token: options.refresh_token,
            muted: muted.clone(),
            link,
            turn,
            mic_sid: None,
            publishing: false,
            ever_connected: false,
            identity: identity.clone(),
            own_sid: String::new(),
            ping: (0, 0),
            participants: HashMap::new(),
            speakers: HashMap::new(),
            local_speaking: Vec::new(),
            remotes: remotes.clone(),
            video: video::Receiver::new(screens.clone(), broken_tx),
            outgoing,
            owners: HashMap::new(),
            sized: std::collections::HashSet::new(),
            events: events_tx.clone(),
            pub_ice: IceConnectionState::New,
            sub_ice: IceConnectionState::New,
            ice_down_since: None,
            recovery: None,
            route: crate::peer::default_route_address(),
            route_checked: Instant::now(),
        };
        engine.joined(&join);
        let task = tokio::spawn(engine.run(packets, commands_rx, broken, video_packets, local_speaking_rx));

        let mut gate = NoiseGate::new(crate::audio::SAMPLE_RATE);
        gate.set_threshold(options.gate_threshold);
        let threshold = Arc::new(AtomicU32::new(options.gate_threshold.to_bits()));
        let microphone = Microphone {
            gate,
            threshold: threshold.clone(),
            applied: options.gate_threshold,
            encoder: Encoder::new()?,
            rtp: 0,
            packets: packets_tx,
            muted: muted.clone(),
            events: events_tx,
            echo: None,
        };
        let deafened = Arc::new(AtomicBool::new(false));
        let speaker = Speaker {
            remotes: remotes.clone(),
            frames: Vec::new(),
            echo: None,
            deafened: deafened.clone(),
            local_speaking: local_speaking_tx,
            last_speaking: Vec::new(),
        };
        Ok(Joined { call: Call { commands, muted, deafened, remotes, gate: threshold, identity, task }, microphone, speaker, events, screens, camera, screen })
    }

    /// Stop sending the microphone, and tell the room.
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
        let _ = self.commands.send(Command::Mute(muted));
    }

    /// Turn our camera on in the call (publishing it the first time) or
    /// off (muted: it stops being sent).
    pub fn set_camera(&self, on: bool) {
        let _ = self.commands.send(Command::Video(Source::Camera, on));
    }

    /// Share our screen in the call, or stop.
    pub fn set_screen(&self, on: bool) {
        let _ = self.commands.send(Command::Video(Source::Screen, on));
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    /// Stop playing everyone else (the microphone is the caller's to mute
    /// Stop (or start again) hearing the call (deafening).
    pub fn set_deafened(&self, deafened: bool) {
        self.deafened.store(deafened, Ordering::Relaxed);
    }

    /// Set a remote participant's local playback volume (`0.0` = local mute,
    /// `1.0` = 100%, up to `2.0` = 200%).
    pub fn set_participant_volume(&self, identity: &str, volume: f32) {
        let clamped = volume.clamp(0.0, 2.0);
        let mut remotes = self.remotes.lock().unwrap();
        if (clamped - 1.0).abs() < 0.001 {
            remotes.volumes.remove(identity);
        } else {
            remotes.volumes.insert(identity.to_owned(), clamped);
        }
    }

    /// Change the noise gate's threshold mid-call (dBFS; at
    /// [`GATE_OFF_DB`] or below nothing is cut).
    pub fn set_gate_threshold(&self, db: f32) {
        self.gate.store(db.to_bits(), Ordering::Relaxed);
    }

    pub fn is_deafened(&self) -> bool {
        self.deafened.load(Ordering::Relaxed)
    }

    /// The network changed (the OS said so): reconnect now, on the new one,
    /// rather than waiting for the old path to be noticed dead.
    pub fn reconnect(&self) {
        let _ = self.commands.send(Command::Reconnect);
    }

    /// Cut signalling and media without a word to the server, as a dead
    /// network would. For testing reconnects.
    #[doc(hidden)]
    pub fn sever(&self) {
        let _ = self.commands.send(Command::Sever { signalling: true });
    }

    /// Cut only the media, leaving signalling up: a network that lost UDP,
    /// or a WebSocket that hasn't noticed yet. For testing reconnects.
    /// End the publisher connection outright, which no resume can mend: the
    /// engine has to rejoin. For testing.
    #[doc(hidden)]
    pub fn kill_publisher(&self) {
        let _ = self.commands.send(Command::KillPublisher);
    }

    #[doc(hidden)]
    pub fn sever_media(&self) {
        let _ = self.commands.send(Command::Sever { signalling: false });
    }

    /// Leave the room and wait for the engine to finish.
    pub async fn leave(self) {
        let _ = self.commands.send(Command::Leave);
        let _ = tokio::time::timeout(Duration::from_secs(2), &mut { self }.task).await;
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Leave);
    }
}

fn participant(p: &proto::ParticipantInfo) -> Participant {
    // Someone can have more than one track of a kind (republished, or an
    // old one muted): the one that's on is the one to show.
    let find = |source: proto::TrackSource| {
        let of = || p.tracks.iter().filter(move |t| t.r#type == proto::TrackType::Video as i32 && t.source == source as i32);
        of().find(|t| !t.muted).or_else(|| of().next())
    };
    let (camera, screen) = (find(proto::TrackSource::Camera), find(proto::TrackSource::ScreenShare));
    let muted = !p.tracks.iter().any(|t| t.r#type == proto::TrackType::Audio as i32 && !t.muted);
    Participant {
        sid: p.sid.clone(),
        identity: p.identity.clone(),
        name: p.name.clone(),
        muted,
        camera: camera.is_some_and(|t| !t.muted),
        camera_sid: camera.map(|t| t.sid.clone()),
        screen: screen.is_some_and(|t| !t.muted),
        screen_sid: screen.map(|t| t.sid.clone()),
    }
}

/// One of our video tracks (camera or screen): whether it's on, its send
/// line once added, and the server's id for it once published.
struct Outgoing {
    source: Source,
    on: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    mid: Option<Mid>,
    sid: Option<String>,
}

impl Outgoing {
    fn new(source: Source) -> Outgoing {
        Outgoing { source, on: Arc::new(AtomicBool::new(false)), keyframe: Arc::new(AtomicBool::new(true)), mid: None, sid: None }
    }

    fn cid(&self) -> &'static str {
        match self.source {
            Source::Camera => CAMERA_CID,
            Source::Screen => SCREEN_CID,
        }
    }
}

/// The size we ask other people's cameras for: enough for a call tile on a
/// high-density screen, and the server sends the layer that fits.
const VIEW_SIZE: (u32, u32) = (640, 360);

/// Fold a speakers update into who's speaking: the server sends only those
/// whose state changed (started, stopped, or got louder or quieter).
fn apply_speakers(speakers: &mut HashMap<String, f32>, updates: &[proto::SpeakerInfo]) {
    for s in updates {
        if s.active {
            speakers.insert(s.sid.clone(), s.level);
        } else {
            speakers.remove(&s.sid);
        }
    }
}

fn sorted(participants: &HashMap<String, Participant>) -> Vec<Participant> {
    let mut list: Vec<Participant> = participants.values().cloned().collect();
    list.sort_by(|a, b| a.identity.cmp(&b.identity));
    list
}

/// ICE down this long (no path left, and none came back) is a lost
/// connection, not a blip.
const ICE_GRACE: Duration = Duration::from_secs(4);
/// How long an attempt gets to bring the media back before the next.
const ATTEMPT_WAIT: Duration = Duration::from_secs(10);
/// Resume attempts before falling back to a fresh join.
const RESUME_ATTEMPTS: u32 = 2;
/// Give up when nothing has worked for this long.
const GIVE_UP: Duration = Duration::from_secs(90);

/// What went wrong, and what it calls for.
enum Trouble {
    /// The path is gone; the session may well be alive on the server.
    Lost(String, ReconnectReason),
    /// The session is gone: only a fresh join will do.
    Rejoin(String),
    /// Over.
    End(String),
}

/// A reconnect in progress.
struct Recovery {
    started: Instant,
    attempt: u32,
    /// When to try again if the media isn't back by then.
    next: Instant,
    reason: ReconnectReason,
    rejoin: bool,
}

/// The task that owns the link and drives the call.
struct Engine {
    url: String,
    token: String,
    network: Network,
    refresh_token: Option<TokenSource>,
    muted: Arc<AtomicBool>,
    link: Link,
    /// TURN servers as last handed out (with their credentials).
    turn: Vec<TurnServer>,
    /// The server's id for our microphone track, once it's published.
    mic_sid: Option<String>,
    /// The publisher's DTLS is up: packets can be written.
    publishing: bool,
    ever_connected: bool,
    identity: String,
    own_sid: String,
    /// The server's ping interval and timeout, for a resumed connection.
    ping: (i32, i32),
    participants: HashMap<String, Participant>,
    /// Who's speaking now, by sid, with their level. The server only says
    /// whose state changed, so this keeps the rest.
    speakers: HashMap<String, f32>,
    /// Remote identities heard speaking directly from decoded audio frames.
    local_speaking: Vec<String>,
    remotes: Arc<Mutex<Remotes>>,
    /// Remote cameras.
    video: video::Receiver,
    /// Our camera: whether it's on, its send line once added, and the
    /// server's id for it once published.
    outgoing: [Outgoing; 2],
    /// Which participant each subscriber media line belongs to (by mid).
    owners: HashMap<String, (String, String)>,
    /// Camera tracks we've told the server what size we want.
    sized: std::collections::HashSet<String>,
    events: mpsc::UnboundedSender<CallEvent>,
    pub_ice: IceConnectionState,
    sub_ice: IceConnectionState,
    ice_down_since: Option<Instant>,
    recovery: Option<Recovery>,
    /// The address the OS would send from, as last seen: when it changes,
    /// the machine moved networks and the old path is as good as gone.
    route: Option<std::net::IpAddr>,
    route_checked: Instant,
}

/// How often to look at the default route.
const ROUTE_CHECK: Duration = Duration::from_secs(2);

fn ice_up(state: IceConnectionState) -> bool {
    matches!(state, IceConnectionState::Connected | IceConnectionState::Completed)
}

impl Engine {
    /// Ask for each new camera track at [`VIEW_SIZE`] (by default the
    /// server sends the biggest layer).
    fn size_cameras(&mut self) {
        let new: Vec<String> = self.participants.values().filter_map(|p| p.camera_sid.clone()).filter(|sid| !self.sized.contains(sid)).collect();
        for sid in new {
            if self.link.signal.track_settings(&sid, VIEW_SIZE.0, VIEW_SIZE.1).is_ok() {
                self.sized.insert(sid);
            }
        }
    }

    /// Who's speaking, loudest first, by identity (merging server active-speaker
    /// updates with locally detected speech from decoded remote audio).
    fn send_speaking(&self) {
        let mut loudest: Vec<(&String, f32)> = self.speakers.iter().map(|(sid, level)| (sid, *level)).collect();
        loudest.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut speaking: Vec<String> = loudest
            .into_iter()
            .filter_map(|(sid, _)| if *sid == self.own_sid { Some(self.identity.clone()) } else { self.participants.get(sid).map(|p| p.identity.clone()) })
            .collect();
        for id in &self.local_speaking {
            if !speaking.iter().any(|s| s == id) {
                speaking.push(id.clone());
            }
        }
        let _ = self.events.send(CallEvent::Speaking(speaking));
    }

    /// Take in a join response: who we are, who's here.
    fn joined(&mut self, join: &proto::JoinResponse) {
        self.own_sid = join.participant.as_ref().map(|p| p.sid.clone()).unwrap_or_default();
        self.ping = (join.ping_interval, join.ping_timeout);
        self.participants = join.other_participants.iter().map(|p| (p.sid.clone(), participant(p))).collect();
        // A new session: whoever was speaking will be said again.
        self.speakers.clear();
        let _ = self.events.send(CallEvent::Speaking(Vec::new()));
        self.size_cameras();
        let _ = self.events.send(CallEvent::Participants(sorted(&self.participants)));
    }

    async fn run(
        mut self,
        mut packets: mpsc::UnboundedReceiver<(u64, Vec<u8>, i8, bool)>,
        mut commands: mpsc::UnboundedReceiver<Command>,
        mut broken: mpsc::UnboundedReceiver<Mid>,
        mut video_packets: mpsc::UnboundedReceiver<video::VideoPacket>,
        mut local_speaking: mpsc::UnboundedReceiver<Vec<String>>,
    ) {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let reason = loop {
            let step: Result<(), Trouble> = tokio::select! {
                event = self.link.signal_events.recv() => match event {
                    Some(event) => self.on_signal(event).await,
                    // Severed, or the pump died: the path is gone.
                    None => Err(Trouble::Lost("signalling lost".into(), ReconnectReason::RrSignalDisconnected)),
                },
                Some(mid) = broken.recv() => {
                    if self.video.broke(mid) {
                        let _ = self.link.subscriber.request_keyframe(mid);
                    }
                    Ok(())
                }
                event = self.link.pub_events.recv() => match event {
                    Some(event) => self.on_publisher(event),
                    None => Err(Trouble::Rejoin("publisher gone".into())),
                },
                event = self.link.sub_events.recv() => match event {
                    Some(event) => self.on_subscriber(event),
                    None => Err(Trouble::Rejoin("subscriber gone".into())),
                },
                Some((source, rid, rtp, packet)) = video_packets.recv() => {
                    if self.publishing
                        && let Some(mid) = self.outgoing.iter().find(|o| o.source == source).and_then(|o| o.mid)
                    {
                        let _ = self.link.publisher.write_video(mid, rid, rtp, packet);
                    }
                    Ok(())
                }
                Some((rtp, packet, audio_level, vad)) = packets.recv() => {
                    if self.publishing {
                        // A failed write means the peer is closing; its event says why.
                        let _ = self.link.publisher.write_audio_with_level(self.link.mic_mid, rtp, packet, audio_level, vad);
                    }
                    Ok(())
                }
                Some(now_speaking) = local_speaking.recv() => {
                    if self.local_speaking != now_speaking {
                        self.local_speaking = now_speaking;
                        self.send_speaking();
                    }
                    Ok(())
                }
                command = commands.recv() => match command {
                    Some(Command::Mute(muted)) => {
                        if let Some(sid) = &self.mic_sid {
                            let _ = self.link.signal.mute(sid, muted);
                        }
                        Ok(())
                    }
                    Some(Command::Video(source, on)) => self.set_video(source, on).await,
                    Some(Command::Reconnect) => Err(Trouble::Lost("the network changed".into(), ReconnectReason::RrSwitchCandidate)),
                    Some(Command::KillPublisher) => {
                        self.link.publisher.close();
                        Ok(())
                    }
                    Some(Command::Sever { signalling }) => {
                        if signalling {
                            self.link.signal.sever();
                        }
                        self.link.publisher.sever();
                        self.link.subscriber.sever();
                        Ok(())
                    }
                    Some(Command::Leave) | None => {
                        let _ = self.link.signal.leave();
                        Err(Trouble::End("left".into()))
                    }
                },
                _ = tick.tick() => self.on_tick().await,
            };
            match step {
                Ok(()) => {}
                Err(Trouble::End(reason)) => break reason,
                Err(Trouble::Lost(why, reason)) => self.recover(why, reason, false).await,
                Err(Trouble::Rejoin(why)) => self.recover(why, ReconnectReason::RrUnknown, true).await,
            }
            if let Some(recovery) = &self.recovery
                && recovery.started.elapsed() > GIVE_UP
            {
                break format!("couldn't reconnect in {GIVE_UP:?}");
            }
        };
        self.link.publisher.close();
        self.link.subscriber.close();
        tracing::info!("{}: call ended: {reason}", self.identity);
        let _ = self.events.send(CallEvent::Ended(reason));
    }

    /// Start (or escalate) a reconnect, and make an attempt.
    async fn recover(&mut self, why: String, reason: ReconnectReason, rejoin: bool) {
        match &mut self.recovery {
            // Already at it: a further failure only matters if it says the
            // session is gone.
            Some(recovery) => {
                if rejoin && !recovery.rejoin {
                    recovery.rejoin = true;
                    recovery.next = Instant::now();
                } else {
                    return;
                }
            }
            None => {
                tracing::info!("{}: connection lost ({why}); reconnecting", self.identity);
                self.recovery = Some(Recovery { started: Instant::now(), attempt: 0, next: Instant::now(), reason, rejoin });
            }
        }
        self.attempt(&why).await;
    }

    /// One attempt: resume the session, or join afresh. Either way the media
    /// comes back through the ordinary loop; `on_tick` notices it did (or
    /// that it didn't, and tries again).
    async fn attempt(&mut self, why: &str) {
        let Some(recovery) = &mut self.recovery else { return };
        recovery.attempt += 1;
        recovery.next = Instant::now() + ATTEMPT_WAIT;
        if recovery.attempt > RESUME_ATTEMPTS {
            recovery.rejoin = true;
        }
        let (attempt, rejoin, reason) = (recovery.attempt, recovery.rejoin, recovery.reason);
        let _ = self.events.send(CallEvent::Reconnecting { attempt, reason: why.to_owned() });
        self.publishing = false;
        self.pub_ice = IceConnectionState::New;
        self.sub_ice = IceConnectionState::New;
        self.ice_down_since = None;

        let outcome = if rejoin { self.rejoin().await } else { self.resume(reason).await };
        match outcome {
            Ok(()) => {}
            // The server won't resume this session: straight to a fresh join.
            Err(CallError::Signal(crate::livekit::Error::Left(_) | crate::livekit::Error::Refused { .. })) if !rejoin => {
                tracing::info!("{}: the session can't be resumed; rejoining", self.identity);
                if let Some(recovery) = &mut self.recovery {
                    recovery.rejoin = true;
                    recovery.next = Instant::now();
                }
            }
            // Probably no network yet: wait a little and try again.
            Err(e) => {
                tracing::info!("{}: reconnect attempt {attempt} failed: {e}", self.identity);
                if let Some(recovery) = &mut self.recovery {
                    recovery.next = Instant::now() + Duration::from_secs(1 << attempt.min(3));
                }
            }
        }
    }

    /// Same session, new path: signalling reconnects as this participant,
    /// both peers gather afresh and restart ICE.
    async fn resume(&mut self, reason: ReconnectReason) -> Result<()> {
        let (signal, response, signal_events) = crate::livekit::resume(&self.url, &self.token, &self.own_sid, reason, self.ping).await?;
        if let Some(response) = response
            && !response.ice_servers.is_empty()
        {
            self.turn = turn_servers(&response.ice_servers);
        }
        // The server's subscriber offer (an ICE restart) is already on its
        // way; it waits in `signal_events` until both peers have new
        // candidates, so the answer carries them.
        let publisher = (self.network)(&format!("{}/pub", self.identity), &self.turn);
        let subscriber = (self.network)(&format!("{}/sub", self.identity), &self.turn);
        self.link.subscriber.restart(subscriber, false).await?;
        let offer = self.link.publisher.restart(publisher, true).await?;
        if let Some(sdp) = offer {
            signal.offer(sdp, 0)?;
        }
        self.link.signal = signal;
        self.link.signal_events = signal_events;
        Ok(())
    }

    /// A new session: everything rebuilt, the microphone republished.
    async fn rejoin(&mut self) -> Result<()> {
        let _ = self.link.signal.leave();
        if let Some(refresh) = &self.refresh_token {
            match refresh().await {
                Ok(token) => self.token = token,
                Err(e) => tracing::warn!("{}: no fresh token ({e}); trying the old one", self.identity),
            }
        }
        let (link, join, turn) = open_link(&self.url, &self.token, &self.network, self.muted.load(Ordering::Relaxed)).await?;
        self.link.publisher.close();
        self.link.subscriber.close();
        self.link = link;
        self.turn = turn;
        self.mic_sid = None;
        self.remotes.lock().unwrap().tracks.clear();
        self.video.reset();
        self.owners.clear();
        self.sized.clear();
        for out in &mut self.outgoing {
            out.mid = None;
            out.sid = None;
        }
        self.joined(&join);
        // A new session has no video tracks: publish again what's on.
        for source in [Source::Camera, Source::Screen] {
            if self.outgoing(source).on.load(Ordering::Relaxed)
                && let Err(e) = self.publish(source).await
            {
                tracing::warn!("{}: republishing {source:?}: {e}", self.identity);
            }
        }
        Ok(())
    }

    async fn on_tick(&mut self) -> Result<(), Trouble> {
        let now = Instant::now();
        match &self.recovery {
            Some(recovery) => {
                if ice_up(self.pub_ice) && ice_up(self.sub_ice) && self.publishing {
                    let resumed = !recovery.rejoin;
                    tracing::info!("{}: reconnected after {:?} ({})", self.identity, recovery.started.elapsed(), if resumed { "resumed" } else { "rejoined" });
                    self.recovery = None;
                    let _ = self.events.send(CallEvent::Reconnected { resumed });
                } else if now >= recovery.next {
                    self.attempt("still not connected").await;
                }
            }
            None => {
                if now.duration_since(self.route_checked) >= ROUTE_CHECK {
                    self.route_checked = now;
                    let route = crate::peer::default_route_address();
                    if route != self.route {
                        let why = format!("the network changed ({:?} → {:?})", self.route, route);
                        self.route = route;
                        // No route at all: nothing to reconnect over yet. The
                        // path check notices; the next route change retries.
                        if route.is_some() {
                            return Err(Trouble::Lost(why, ReconnectReason::RrSwitchCandidate));
                        }
                    }
                }
                if let Some(since) = self.ice_down_since
                    && since.elapsed() > ICE_GRACE
                {
                    let reason = if ice_up(self.pub_ice) { ReconnectReason::RrSubscriberFailed } else { ReconnectReason::RrPublisherFailed };
                    return Err(Trouble::Lost(format!("no media path for {ICE_GRACE:?}"), reason));
                }
            }
        }
        Ok(())
    }

    fn on_ice(&mut self) {
        let up = ice_up(self.pub_ice) && (ice_up(self.sub_ice) || !self.ever_connected);
        if up {
            self.ice_down_since = None;
        } else if self.ever_connected && self.ice_down_since.is_none() {
            self.ice_down_since = Some(Instant::now());
        }
    }

    fn outgoing(&mut self, source: Source) -> &mut Outgoing {
        match source {
            Source::Camera => &mut self.outgoing[0],
            Source::Screen => &mut self.outgoing[1],
        }
    }

    /// Add a video track and its send line (the first time it's turned on
    /// in a session): the camera in three sizes, the screen in one.
    async fn publish(&mut self, source: Source) -> Result<()> {
        let cid = self.outgoing(source).cid();
        let (kind, size, layers): (proto::TrackSource, (u32, u32), &[&str]) = match source {
            Source::Camera => (proto::TrackSource::Camera, CAMERA_SIZE, &CAMERA_LAYERS),
            Source::Screen => (proto::TrackSource::ScreenShare, SCREEN_SIZE, &[]),
        };
        self.link.signal.add_track(AddTrack {
            cid: cid.into(),
            name: cid.into(),
            kind: proto::TrackType::Video,
            source: kind,
            muted: false,
            width: size.0,
            height: size.1,
            layers: layers.iter().zip([4, 2, 1]).map(|(rid, d)| ((*rid).to_owned(), size.0 / d, size.1 / d)).collect(),
        })?;
        let (sdp, mid) = self.link.publisher.offer_video(&format!("{}-{cid}", self.identity), cid, layers).await?;
        self.link.signal.offer(sdp, 1)?;
        let out = self.outgoing(source);
        out.mid = Some(mid);
        out.keyframe.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn set_video(&mut self, source: Source, on: bool) -> Result<(), Trouble> {
        let out = self.outgoing(source);
        out.on.store(on, Ordering::Relaxed);
        if on && out.mid.is_none() {
            if let Err(e) = self.publish(source).await {
                tracing::warn!("{}: publishing {source:?}: {e}", self.identity);
            }
        } else if let Some(sid) = out.sid.clone() {
            if on {
                out.keyframe.store(true, Ordering::Relaxed);
            }
            let _ = self.link.signal.mute(&sid, !on);
        }
        Ok(())
    }

    fn on_publisher(&mut self, event: PeerEvent) -> Result<(), Trouble> {
        match event {
            PeerEvent::KeyframeRequest { mid } => {
                if let Some(out) = self.outgoing.iter().find(|o| o.mid == Some(mid)) {
                    out.keyframe.store(true, Ordering::Relaxed);
                }
            }
            PeerEvent::Connected => {
                self.publishing = true;
                if !self.ever_connected {
                    self.ever_connected = true;
                    let _ = self.events.send(CallEvent::Connected);
                }
            }
            PeerEvent::IceState(state) => {
                self.pub_ice = state;
                // ICE back after a restart: DTLS never went, so writing resumes.
                if ice_up(state) && self.ever_connected {
                    self.publishing = true;
                }
                self.on_ice();
            }
            PeerEvent::Path { alive: false } if self.ever_connected && self.recovery.is_none() => {
                return Err(Trouble::Lost(format!("nothing from the server for {PATH_TIMEOUT:?}"), ReconnectReason::RrPublisherFailed));
            }
            PeerEvent::Closed(reason) => return Err(Trouble::Rejoin(format!("publisher: {reason}"))),
            _ => {}
        }
        Ok(())
    }

    async fn on_signal(&mut self, event: SignalEvent) -> Result<(), Trouble> {
        let lost = |what: &str, e: &dyn std::fmt::Display| Trouble::Lost(format!("{what}: {e}"), ReconnectReason::RrSignalDisconnected);
        match event {
            SignalEvent::Answer(answer) => {
                if let Err(e) = self.link.publisher.accept_answer(&answer.sdp).await {
                    return Err(Trouble::Rejoin(format!("publisher answer: {e}")));
                }
            }
            SignalEvent::Offer(offer) => {
                self.owners.extend(video::owners(&offer.sdp));
                let answer = match self.link.subscriber.accept_offer(&offer.sdp).await {
                    Ok(answer) => answer,
                    Err(e) => return Err(Trouble::Rejoin(format!("subscriber offer: {e}"))),
                };
                self.link.signal.answer(answer, offer.id).map_err(|e| lost("answering", &e))?;
            }
            SignalEvent::Candidate { target: Target::Publisher, candidate } => {
                if let Err(e) = self.link.publisher.add_remote_candidate(&candidate) {
                    tracing::debug!("publisher candidate {candidate:?}: {e}");
                }
            }
            SignalEvent::Candidate { target: Target::Subscriber, candidate } => {
                if let Err(e) = self.link.subscriber.add_remote_candidate(&candidate) {
                    tracing::debug!("subscriber candidate {candidate:?}: {e}");
                }
            }
            SignalEvent::TrackPublished { cid, track } if cid == CAMERA_CID || cid == SCREEN_CID => {
                let source = if cid == CAMERA_CID { Source::Camera } else { Source::Screen };
                self.outgoing(source).sid = track.map(|t| t.sid);
            }
            SignalEvent::TrackPublished { cid, track } if cid == MIC_CID => {
                self.mic_sid = track.map(|t| t.sid);
                // Muted while the track was being republished after a rejoin.
                if let Some(sid) = &self.mic_sid
                    && self.muted.load(Ordering::Relaxed)
                {
                    let _ = self.link.signal.mute(sid, true);
                }
            }
            SignalEvent::Participants(updates) => {
                let mut changed = false;
                for p in updates {
                    if p.sid == self.own_sid {
                        continue;
                    }
                    changed = true;
                    let now = participant(&p);
                    let left = p.state == proto::participant_info::State::Disconnected as i32;
                    if left || !now.camera {
                        self.video.gone(&now.identity);
                    }
                    if left || !now.screen {
                        self.video.gone(&screen_key(&now.identity));
                    }
                    if p.state == proto::participant_info::State::Disconnected as i32 {
                        self.participants.remove(&p.sid);
                        if self.speakers.remove(&p.sid).is_some() {
                            self.send_speaking();
                        }
                    } else {
                        self.participants.insert(p.sid.clone(), now);
                    }
                }
                if changed {
                    for p in self.participants.values() {
                        tracing::info!("{}: {} camera {:?} (on: {}), screen {:?} (on: {})", self.identity, p.identity, p.camera_sid, p.camera, p.screen_sid, p.screen);
                    }
                    self.size_cameras();
                    let _ = self.events.send(CallEvent::Participants(sorted(&self.participants)));
                }
            }
            SignalEvent::Speakers(speakers) => {
                apply_speakers(&mut self.speakers, &speakers);
                self.send_speaking();
            }
            SignalEvent::Leave(leave) => {
                use proto::leave_request::Action;
                let why = format!("the server asked us to leave ({:?})", leave.reason());
                return Err(match leave.action() {
                    Action::Resume => Trouble::Lost(why, ReconnectReason::RrUnknown),
                    Action::Reconnect => Trouble::Rejoin(why),
                    Action::Disconnect if leave.can_reconnect => Trouble::Rejoin(why),
                    Action::Disconnect => Trouble::End(why),
                });
            }
            SignalEvent::Closed(reason) => return Err(Trouble::Lost(format!("signalling closed: {reason}"), ReconnectReason::RrSignalDisconnected)),
            _ => {}
        }
        Ok(())
    }

    fn on_subscriber(&mut self, event: PeerEvent) -> Result<(), Trouble> {
        match event {
            PeerEvent::Audio { mid, rtp_time, data } => {
                let now = Instant::now();
                let owner = self
                    .owners
                    .get(&mid.to_string())
                    .and_then(|(sid, _)| self.participants.get(sid))
                    .map(|p| p.identity.clone());
                let mut remotes = self.remotes.lock().unwrap();
                let fresh = || JitterBuffer::new().map(|buffer| Remote { buffer, last_packet: now, identity: owner.clone(), loud_at: None });
                let remote = match remotes.tracks.entry(mid) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => match fresh() {
                        Ok(remote) => entry.insert(remote),
                        Err(e) => return Err(Trouble::End(format!("decoder: {e}"))),
                    },
                };
                if now.duration_since(remote.last_packet) > IDLE {
                    match fresh() {
                        Ok(fresh) => *remote = fresh,
                        Err(e) => return Err(Trouble::End(format!("decoder: {e}"))),
                    }
                }
                if remote.identity.is_none() {
                    remote.identity = owner;
                }
                remote.last_packet = now;
                remote.buffer.push(rtp_time, data.to_vec(), now);
            }
            PeerEvent::Video { mid, data, keyframe, contiguous, .. } => {
                // A person's screen is kept apart from their camera.
                let key = self.owners.get(&mid.to_string()).and_then(|(sid, track)| {
                    let p = self.participants.get(sid)?;
                    Some(if p.screen_sid.as_deref() == Some(track.as_str()) { screen_key(&p.identity) } else { p.identity.clone() })
                });
                if let Some(key) = key
                    && self.video.frame(mid, &key, data, keyframe, contiguous)
                {
                    let _ = self.link.subscriber.request_keyframe(mid);
                }
            }
            PeerEvent::IceState(state) => {
                self.sub_ice = state;
                self.on_ice();
            }
            PeerEvent::MediaAdded { mid, kind: MediaKind::Audio } => tracing::debug!("{}: remote audio on {mid}", self.identity),
            PeerEvent::Path { alive: false } if self.ever_connected && self.recovery.is_none() => {
                return Err(Trouble::Lost(format!("nothing from the server for {PATH_TIMEOUT:?}"), ReconnectReason::RrSubscriberFailed));
            }
            PeerEvent::Closed(reason) => return Err(Trouble::Rejoin(format!("subscriber: {reason}"))),
            _ => {}
        }
        Ok(())
    }
}

/// Where the microphone's frames go. Lives on the audio thread.
pub struct Microphone {
    gate: NoiseGate,
    /// The threshold the call wants (f32 bits), and the one the gate has.
    threshold: Arc<AtomicU32>,
    applied: f32,
    encoder: Encoder,
    rtp: u64,
    packets: mpsc::UnboundedSender<(u64, Vec<u8>, i8, bool)>,
    muted: Arc<AtomicBool>,
    events: mpsc::UnboundedSender<CallEvent>,
    echo: Option<EchoCanceller>,
}

impl Microphone {
    /// Clean every frame with `echo` first. Give the [`Speaker`] the same
    /// canceller, so it knows what was played.
    pub fn set_echo_canceller(&mut self, echo: Option<EchoCanceller>) {
        self.echo = echo;
    }

    pub fn set_gate_threshold(&mut self, db: f32) {
        self.threshold.store(db.to_bits(), Ordering::Relaxed);
    }

    /// One 20 ms frame of microphone: echo-cancelled, gated, encoded and
    /// sent (unless muted).
    pub fn push(&mut self, frame: &[f32; FRAME]) {
        let mut frame = *frame;
        let wanted = f32::from_bits(self.threshold.load(Ordering::Relaxed));
        if wanted != self.applied {
            self.applied = wanted;
            self.gate.set_threshold(wanted);
        }
        // Even while muted: the canceller keeps adapting, so unmuting
        // doesn't let a burst of echo through.
        if let Some(echo) = &self.echo {
            echo.capture(&mut frame);
        }
        if let Some(reading) = self.gate.process(&mut frame) {
            let _ = self.events.send(CallEvent::Level(reading));
        }
        // Muted time still passes in RTP: the far end sees a gap, not a
        // stretch of audio squeezed together.
        let rtp = self.rtp;
        self.rtp += FRAME as u64;
        if self.muted.load(Ordering::Relaxed) {
            return;
        }
        let db = frame_dbfs(&frame);
        let audio_level = db.round().clamp(-127.0, 0.0) as i8;
        let vad = db > SPEAKING_DB;
        match self.encoder.encode(&frame) {
            Ok(packet) => {
                let _ = self.packets.send((rtp, packet, audio_level, vad));
            }
            Err(e) => tracing::warn!("encoding the microphone: {e}"),
        }
    }
}

/// Where the loudspeaker's frames come from. Lives on the audio thread.
pub struct Speaker {
    remotes: Arc<Mutex<Remotes>>,
    frames: Vec<[f32; FRAME]>,
    echo: Option<EchoCanceller>,
    /// Deafened: everyone else is still received (and their buffers kept
    /// moving) but not played.
    deafened: Arc<AtomicBool>,
    /// Reports locally detected remote speakers to the engine.
    local_speaking: mpsc::UnboundedSender<Vec<String>>,
    last_speaking: Vec<String>,
}

impl Speaker {
    /// Tell `echo` about every frame played; see
    /// [`Microphone::set_echo_canceller`].
    pub fn set_echo_canceller(&mut self, echo: Option<EchoCanceller>) {
        self.echo = echo;
    }

    /// Each live remote track's jitter buffer: target delay and counters.
    pub fn stats(&self) -> Vec<(Duration, JitterStats)> {
        let now = Instant::now();
        let remotes = self.remotes.lock().unwrap();
        remotes
            .tracks
            .values()
            .filter(|r| now.duration_since(r.last_packet) <= IDLE)
            .map(|r| (r.buffer.target_delay(), r.buffer.stats()))
            .collect()
    }

    /// The next 20 ms of everyone else, mixed.
    pub fn pull(&mut self, out: &mut [f32; FRAME]) {
        let now = Instant::now();
        let mut remotes = self.remotes.lock().unwrap();
        let volumes = remotes.volumes.clone();
        let live = remotes.tracks.values_mut().filter(|r| now.duration_since(r.last_packet) <= IDLE);
        let mut used = 0;
        let mut speaking_now = Vec::new();
        for remote in live {
            if used == self.frames.len() {
                self.frames.push([0.0; FRAME]);
            }
            if remote.buffer.pop(&mut self.frames[used]).is_ok() {
                if frame_dbfs(&self.frames[used]) > SPEAKING_DB {
                    remote.loud_at = Some(now);
                }
                if remote.loud_at.is_some_and(|at| now.duration_since(at) < SPEAKING_HOLD)
                    && let Some(id) = &remote.identity
                    && !speaking_now.contains(id)
                {
                    speaking_now.push(id.clone());
                }
                let gain = remote.identity.as_deref().and_then(|id| volumes.get(id).copied()).unwrap_or(1.0);
                if gain <= 0.001 {
                    continue;
                }
                if (gain - 1.0).abs() > 0.001 {
                    for s in &mut self.frames[used] {
                        *s *= gain;
                    }
                }
                used += 1;
            }
        }
        drop(remotes);
        speaking_now.sort();
        if speaking_now != self.last_speaking {
            self.last_speaking = speaking_now.clone();
            let _ = self.local_speaking.send(speaking_now);
        }
        if self.deafened.load(Ordering::Relaxed) {
            used = 0;
        }
        mix(self.frames[..used].iter(), out);
        if let Some(echo) = &self.echo {
            echo.render(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(sid: &str, level: f32, active: bool) -> proto::SpeakerInfo {
        proto::SpeakerInfo { sid: sid.into(), level, active }
    }

    #[test]
    fn speakers_updates_are_changes_not_the_whole_list() {
        let mut speakers = HashMap::new();
        apply_speakers(&mut speakers, &[info("a", 0.5, true)]);
        // b starting doesn't stop a.
        apply_speakers(&mut speakers, &[info("b", 0.8, true)]);
        assert_eq!(speakers.len(), 2);
        // a stopping leaves b, with a's level forgotten.
        apply_speakers(&mut speakers, &[info("a", 0.0, false)]);
        assert_eq!(speakers.keys().collect::<Vec<_>>(), [&"b".to_owned()]);
        // A louder b is still b.
        apply_speakers(&mut speakers, &[info("b", 0.9, true)]);
        assert_eq!(speakers.get("b"), Some(&0.9));
    }
}
