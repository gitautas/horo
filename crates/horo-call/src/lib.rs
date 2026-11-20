//! horo's call engine: one participant in a LiveKit room, built from horo's
//! own pieces.
//!
//! [`Call::connect`] joins with `horo-livekit`, opens the two peer
//! connections LiveKit wants (`horo-media`), publishes a microphone track
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
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use horo_livekit::{AddTrack, SignalClient, SignalEvent, Target, proto};
use horo_media::{
    MediaKind, Mid, Peer, PeerConfig, PeerEvent, TurnServer,
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

/// The client track id of our microphone, echoed back by the server.
const MIC_CID: &str = "microphone";
/// A remote track silent this long is left out of the mix; when it speaks
/// again its jitter buffer starts afresh rather than "catching up".
const IDLE: Duration = Duration::from_secs(1);

#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("signalling: {0}")]
    Signal(#[from] horo_livekit::Error),
    #[error("media: {0}")]
    Peer(#[from] horo_media::PeerError),
    #[error("codec: {0}")]
    Codec(#[from] horo_media::audio::codec::CodecError),
    #[error("devices: {0}")]
    Devices(#[from] horo_devices::DeviceError),
}

pub type Result<T, E = CallError> = std::result::Result<T, E>;

/// How to reach the SFU: which candidates each peer connection gathers,
/// given the TURN servers LiveKit offered.
pub type Network = Arc<dyn Fn(&str, &[TurnServer]) -> PeerConfig + Send + Sync>;

/// Host UDP on the default route, ICE-TCP, and every TURN server LiveKit
/// offers: what a client on an unknown network wants.
pub fn default_network() -> Network {
    Arc::new(|name, turn| PeerConfig { turn_servers: turn.to_vec(), ..PeerConfig::default_for(name) })
}

pub struct CallOptions {
    pub network: Network,
    /// The noise gate's threshold in dBFS; [`GATE_OFF_DB`] never cuts.
    pub gate_threshold: f32,
    /// Start muted.
    pub muted: bool,
}

impl Default for CallOptions {
    fn default() -> Self {
        CallOptions { network: default_network(), gate_threshold: GATE_OFF_DB, muted: false }
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
    /// The call is over; nothing follows.
    Ended(String),
}

enum Command {
    Mute(bool),
    Leave,
}

/// The remote side of the mix: one jitter buffer per subscribed track.
#[derive(Default)]
struct Remotes {
    tracks: HashMap<Mid, Remote>,
}

struct Remote {
    buffer: JitterBuffer,
    last_packet: Instant,
}

/// A joined call. Dropping it leaves.
pub struct Call {
    commands: mpsc::UnboundedSender<Command>,
    muted: Arc<AtomicBool>,
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
}

impl Call {
    /// Join the LiveKit room `token` grants, at `livekit_url`.
    pub async fn connect(livekit_url: &str, token: &str, options: CallOptions) -> Result<Joined> {
        let (signal, join, signal_events) = horo_livekit::connect(livekit_url, token, true).await?;
        let identity = join.participant.as_ref().map(|p| p.identity.clone()).unwrap_or_default();

        let turn: Vec<TurnServer> = join
            .ice_servers
            .iter()
            .flat_map(|s| {
                s.urls.iter().map(|uri| TurnServer { uri: uri.clone(), username: s.username.clone(), credential: s.credential.clone() })
            })
            .filter(|s| s.uri.starts_with("turn"))
            .collect();
        let (publisher, pub_events) = Peer::start((options.network)(&format!("{identity}/pub"), &turn)).await?;
        let (subscriber, sub_events) = Peer::start((options.network)(&format!("{identity}/sub"), &turn)).await?;

        signal.add_track(AddTrack {
            cid: MIC_CID.into(),
            name: MIC_CID.into(),
            kind: proto::TrackType::Audio,
            source: proto::TrackSource::Microphone,
            muted: options.muted,
        })?;
        let (sdp, mic_mid) = publisher.offer_audio(&format!("{identity}-mic"), MIC_CID).await?;
        signal.offer(sdp, 0)?;

        let (events_tx, events) = mpsc::unbounded_channel();
        let (packets_tx, packets) = mpsc::unbounded_channel();
        let (commands, commands_rx) = mpsc::unbounded_channel();
        let muted = Arc::new(AtomicBool::new(options.muted));
        let remotes = Arc::new(Mutex::new(Remotes::default()));

        let mut participants: HashMap<String, Participant> = HashMap::new();
        for p in &join.other_participants {
            participants.insert(p.sid.clone(), participant(p));
        }
        let own_sid = join.participant.as_ref().map(|p| p.sid.clone()).unwrap_or_default();
        let _ = events_tx.send(CallEvent::Participants(sorted(&participants)));

        let engine = Engine {
            signal,
            publisher,
            subscriber,
            mic_mid,
            mic_sid: None,
            publishing: false,
            identity: identity.clone(),
            own_sid,
            participants,
            remotes: remotes.clone(),
            events: events_tx.clone(),
        };
        let task = tokio::spawn(engine.run(signal_events, pub_events, sub_events, packets, commands_rx));

        let mut gate = NoiseGate::new(horo_media::audio::SAMPLE_RATE);
        gate.set_threshold(options.gate_threshold);
        let microphone = Microphone { gate, encoder: Encoder::new()?, rtp: 0, packets: packets_tx, muted: muted.clone(), events: events_tx, echo: None };
        let speaker = Speaker { remotes, frames: Vec::new(), echo: None };
        Ok(Joined { call: Call { commands, muted, identity, task }, microphone, speaker, events })
    }

    /// Stop sending the microphone, and tell the room.
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
        let _ = self.commands.send(Command::Mute(muted));
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
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
    Participant { sid: p.sid.clone(), identity: p.identity.clone(), name: p.name.clone() }
}

fn sorted(participants: &HashMap<String, Participant>) -> Vec<Participant> {
    let mut list: Vec<Participant> = participants.values().cloned().collect();
    list.sort_by(|a, b| a.identity.cmp(&b.identity));
    list
}

/// The task that owns the signal connection and both peers.
struct Engine {
    signal: SignalClient,
    publisher: Peer,
    subscriber: Peer,
    mic_mid: Mid,
    /// The server's id for our microphone track, once it's published.
    mic_sid: Option<String>,
    publishing: bool,
    identity: String,
    own_sid: String,
    participants: HashMap<String, Participant>,
    remotes: Arc<Mutex<Remotes>>,
    events: mpsc::UnboundedSender<CallEvent>,
}

impl Engine {
    async fn run(
        mut self,
        mut signal_events: mpsc::UnboundedReceiver<SignalEvent>,
        mut pub_events: mpsc::UnboundedReceiver<PeerEvent>,
        mut sub_events: mpsc::UnboundedReceiver<PeerEvent>,
        mut packets: mpsc::UnboundedReceiver<(u64, Vec<u8>)>,
        mut commands: mpsc::UnboundedReceiver<Command>,
    ) {
        let reason = loop {
            let step = tokio::select! {
                event = signal_events.recv() => match event {
                    Some(event) => self.on_signal(event).await,
                    None => Err("signalling closed".to_owned()),
                },
                event = pub_events.recv() => match event {
                    Some(PeerEvent::Connected) => {
                        self.publishing = true;
                        let _ = self.events.send(CallEvent::Connected);
                        Ok(())
                    }
                    Some(PeerEvent::Closed(reason)) => Err(format!("publisher: {reason}")),
                    Some(_) => Ok(()),
                    None => Err("publisher gone".to_owned()),
                },
                event = sub_events.recv() => match event {
                    Some(event) => self.on_subscriber(event),
                    None => Err("subscriber gone".to_owned()),
                },
                Some((rtp, packet)) = packets.recv() => {
                    if self.publishing {
                        // A failed write means the peer is closing; its event says why.
                        let _ = self.publisher.write_audio(self.mic_mid, rtp, packet);
                    }
                    Ok(())
                }
                command = commands.recv() => match command {
                    Some(Command::Mute(muted)) => {
                        if let Some(sid) = &self.mic_sid {
                            let _ = self.signal.mute(sid, muted);
                        }
                        Ok(())
                    }
                    Some(Command::Leave) | None => {
                        let _ = self.signal.leave();
                        Err("left".to_owned())
                    }
                },
            };
            if let Err(reason) = step {
                break reason;
            }
        };
        self.publisher.close();
        self.subscriber.close();
        tracing::info!("{}: call ended: {reason}", self.identity);
        let _ = self.events.send(CallEvent::Ended(reason));
    }

    async fn on_signal(&mut self, event: SignalEvent) -> Result<(), String> {
        match event {
            SignalEvent::Answer(answer) => self.publisher.accept_answer(&answer.sdp).await.map_err(|e| format!("publisher answer: {e}"))?,
            SignalEvent::Offer(offer) => {
                let answer = self.subscriber.accept_offer(&offer.sdp).await.map_err(|e| format!("subscriber offer: {e}"))?;
                self.signal.answer(answer, offer.id).map_err(|e| e.to_string())?;
            }
            SignalEvent::Candidate { target: Target::Publisher, candidate } => {
                if let Err(e) = self.publisher.add_remote_candidate(&candidate) {
                    tracing::debug!("publisher candidate {candidate:?}: {e}");
                }
            }
            SignalEvent::Candidate { target: Target::Subscriber, candidate } => {
                if let Err(e) = self.subscriber.add_remote_candidate(&candidate) {
                    tracing::debug!("subscriber candidate {candidate:?}: {e}");
                }
            }
            SignalEvent::TrackPublished { cid, track } if cid == MIC_CID => self.mic_sid = track.map(|t| t.sid),
            SignalEvent::Participants(updates) => {
                let mut changed = false;
                for p in updates {
                    if p.sid == self.own_sid {
                        continue;
                    }
                    changed = true;
                    if p.state == proto::participant_info::State::Disconnected as i32 {
                        self.participants.remove(&p.sid);
                    } else {
                        self.participants.insert(p.sid.clone(), participant(&p));
                    }
                }
                if changed {
                    let _ = self.events.send(CallEvent::Participants(sorted(&self.participants)));
                }
            }
            SignalEvent::Speakers(speakers) => {
                let speaking = speakers
                    .iter()
                    .filter(|s| s.active)
                    .filter_map(|s| {
                        if s.sid == self.own_sid {
                            Some(self.identity.clone())
                        } else {
                            self.participants.get(&s.sid).map(|p| p.identity.clone())
                        }
                    })
                    .collect();
                let _ = self.events.send(CallEvent::Speaking(speaking));
            }
            SignalEvent::Leave(leave) => return Err(format!("the server ended the call ({:?})", leave.reason())),
            SignalEvent::Closed(reason) => return Err(format!("signalling closed: {reason}")),
            _ => {}
        }
        Ok(())
    }

    fn on_subscriber(&mut self, event: PeerEvent) -> Result<(), String> {
        match event {
            PeerEvent::Audio { mid, rtp_time, data } => {
                let now = Instant::now();
                let mut remotes = self.remotes.lock().unwrap();
                let fresh = || JitterBuffer::new().map(|buffer| Remote { buffer, last_packet: now });
                let remote = match remotes.tracks.entry(mid) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => entry.insert(fresh().map_err(|e| e.to_string())?),
                };
                if now.duration_since(remote.last_packet) > IDLE {
                    *remote = fresh().map_err(|e| e.to_string())?;
                }
                remote.last_packet = now;
                remote.buffer.push(rtp_time, data.to_vec(), now);
            }
            PeerEvent::MediaAdded { mid, kind: MediaKind::Audio } => tracing::debug!("{}: remote audio on {mid}", self.identity),
            PeerEvent::Closed(reason) => return Err(format!("subscriber: {reason}")),
            _ => {}
        }
        Ok(())
    }
}

/// Where the microphone's frames go. Lives on the audio thread.
pub struct Microphone {
    gate: NoiseGate,
    encoder: Encoder,
    rtp: u64,
    packets: mpsc::UnboundedSender<(u64, Vec<u8>)>,
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
        self.gate.set_threshold(db);
    }

    /// One 20 ms frame of microphone: echo-cancelled, gated, encoded and
    /// sent (unless muted).
    pub fn push(&mut self, frame: &[f32; FRAME]) {
        let mut frame = *frame;
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
        match self.encoder.encode(&frame) {
            Ok(packet) => {
                let _ = self.packets.send((rtp, packet));
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
        let live = remotes.tracks.values_mut().filter(|r| now.duration_since(r.last_packet) <= IDLE);
        let mut used = 0;
        for remote in live {
            if used == self.frames.len() {
                self.frames.push([0.0; FRAME]);
            }
            if remote.buffer.pop(&mut self.frames[used]).is_ok() {
                used += 1;
            }
        }
        drop(remotes);
        mix(self.frames[..used].iter(), out);
        if let Some(echo) = &self.echo {
            echo.render(out);
        }
    }
}
