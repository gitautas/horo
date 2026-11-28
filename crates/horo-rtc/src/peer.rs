//! One WebRTC peer connection: str0m, its sockets, and the loop that drives
//! them.
//!
//! [`Peer::start`] gathers candidates up front and spawns a task that owns
//! the `Rtc` and every socket. The [`Peer`] handle sends it commands
//! (negotiate, add candidates, write media); everything it learns comes back
//! as [`PeerEvent`]s.
//!
//! Candidates gathered, all at once so ICE can choose rather than falling
//! back on a timer:
//! - host UDP, one socket per configured address;
//! - ICE-TCP, an active candidate, for networks that block UDP;
//! - relayed, one per TURN server (UDP, TCP or TLS to the server), for
//!   networks where only the TURN server is reachable.
//!
//! The Phase 0 spike's rules for str0m are built into the loop:
//! 1. drain `poll_output` after every mutation, including the first;
//! 2. feed `Input::Timeout(now)` after any SDP or candidate change, because
//!    a new candidate pair has no deadline of its own and nothing else
//!    would start the checks;
//! 3. `Disconnected` is a state, not an error: str0m reports it whenever it
//!    has no pair to try.
//!
//! [`Peer::restart`] is for when the network changed under a call (Wi-Fi to
//! Ethernet, a laptop waking up): every socket and TURN allocation is
//! replaced, the old candidates invalidated, and the ICE credentials renewed,
//! while DTLS and SRTP carry on, so the call resumes without renegotiating
//! its media.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

use str0m::{
    Candidate, Event, Input, Output, Rtc,
    change::{SdpAnswer, SdpOffer, SdpPendingOffer},
    format::Codec,
    media::{Direction, MediaKind, MediaTime, Mid},
    net::{Protocol, Receive, TcpType},
};
pub use str0m::IceConnectionState;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
    sync::{mpsc, oneshot},
};

use crate::turn::{Turn, TurnError, TurnEvent, frame_len};

#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    #[error("network: {0}")]
    Io(#[from] std::io::Error),
    #[error("WebRTC: {0}")]
    Rtc(#[from] str0m::RtcError),
    #[error("SDP: {0}")]
    Sdp(String),
    #[error("ICE candidate: {0}")]
    Candidate(String),
    #[error("TURN {server}: {error}")]
    Turn { server: String, error: TurnError },
    #[error("TURN {server}: {reason}")]
    TurnSetup { server: String, reason: String },
    #[error("no candidates: every transport failed to start")]
    NoCandidates,
    #[error("no answer is expected: there is no pending offer")]
    NoPendingOffer,
    #[error("the peer connection is closed")]
    Closed,
}

pub type Result<T, E = PeerError> = std::result::Result<T, E>;

/// A connected path silent this long is taken for dead: see [`PeerEvent::Path`].
pub const PATH_TIMEOUT: Duration = Duration::from_secs(4);

/// A TURN server, as `/voip/turnServer` or LiveKit's join response gives it.
#[derive(Debug, Clone)]
pub struct TurnServer {
    /// `turn:host[:port][?transport=udp|tcp]` or `turns:host[:port]`.
    pub uri: String,
    pub username: String,
    pub credential: String,
}

#[derive(Clone)]
pub struct PeerConfig {
    /// For logs: which connection this is.
    pub name: String,
    /// Addresses to gather host UDP candidates on. Empty means none, which
    /// with `ice_tcp` off and no TURN leaves nothing to connect with.
    pub host_addresses: Vec<IpAddr>,
    /// Gather an active ICE-TCP candidate.
    pub ice_tcp: bool,
    pub turn_servers: Vec<TurnServer>,
    /// Trust for `turns:` servers. Defaults to Mozilla's roots.
    pub tls_roots: Option<Arc<rustls::RootCertStore>>,
    /// Where to connect for a TURN host, instead of resolving it. For tests,
    /// where the certificate's name doesn't resolve.
    pub turn_address_override: Option<IpAddr>,
    /// A bad network, simulated on everything this peer receives. For tests.
    pub impairment: Option<Impairment>,
}

/// A simulated bad network: like Linux's `netem`, but in-process and the
/// same on every OS. Applied to what a peer receives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Impairment {
    /// The share of packets lost, 0 to 1.
    pub loss: f64,
    /// Added to every packet.
    pub delay: Duration,
    /// Up to this much more, uniformly at random, so packets reorder.
    pub jitter: Duration,
    /// For the random numbers: the same seed, the same network.
    pub seed: u64,
}

/// A packet held back by an [`Impairment`] until `at`.
struct Delayed {
    at: Instant,
    seq: u64,
    inbound: Inbound,
}

impl PartialEq for Delayed {
    fn eq(&self, other: &Self) -> bool {
        (self.at, self.seq) == (other.at, other.seq)
    }
}
impl Eq for Delayed {}
impl PartialOrd for Delayed {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Delayed {
    /// Reversed: the heap's top is the earliest.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (other.at, other.seq).cmp(&(self.at, self.seq))
    }
}

impl PeerConfig {
    /// Host UDP on the address of the default route, plus ICE-TCP: what a
    /// client on an ordinary network wants, before TURN.
    pub fn default_for(name: &str) -> PeerConfig {
        PeerConfig {
            name: name.into(),
            host_addresses: default_route_address().into_iter().collect(),
            ice_tcp: true,
            turn_servers: Vec::new(),
            tls_roots: None,
            turn_address_override: None,
            impairment: None,
        }
    }
}

/// The address the OS would send from to reach the internet. Connecting a
/// UDP socket sends nothing; it only picks a route.
pub fn default_route_address() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?; // TEST-NET-1: never reached
    socket.local_addr().ok().map(|a| a.ip())
}

#[derive(Debug, Clone)]
pub enum PeerEvent {
    IceState(IceConnectionState),
    /// DTLS is up: media can flow.
    Connected,
    /// The remote side added a media line (on the subscriber connection:
    /// someone else's track).
    MediaAdded { mid: Mid, kind: MediaKind },
    /// One encoded audio frame (Opus), with its RTP time at 48 kHz.
    Audio { mid: Mid, rtp_time: u64, data: Arc<[u8]> },
    /// One whole encoded video frame (VP8), with its RTP time at 90 kHz.
    /// `contiguous` is false when packets before it went missing: the
    /// picture can't be trusted until the next keyframe.
    Video { mid: Mid, rtp_time: u64, data: Arc<[u8]>, keyframe: bool, contiguous: bool },
    /// The remote wants a full picture on a track we send (PLI or FIR).
    KeyframeRequest { mid: Mid },
    /// A TURN relay failed after the call started; ICE may still have others.
    RelayLost(String),
    /// Nothing has arrived from the remote for [`PATH_TIMEOUT`] (`alive:
    /// false`), or something has again (`true`). Faster than ICE's own
    /// verdict, which can take most of a minute; the remote's keepalives and
    /// RTCP never leave a connected path this quiet.
    Path { alive: bool },
    /// The loop ended. Nothing follows.
    Closed(String),
}

enum Command {
    OfferMedia { kind: MediaKind, stream_id: String, track_id: String, rids: Vec<String>, reply: oneshot::Sender<Result<(String, Mid)>> },
    AcceptAnswer { sdp: String, reply: oneshot::Sender<Result<()>> },
    AcceptOffer { sdp: String, reply: oneshot::Sender<Result<String>> },
    RemoteCandidate(String),
    WriteAudio { mid: Mid, rtp_time: u64, data: Vec<u8>, audio_level: Option<(i8, bool)> },
    RequestKeyframe { mid: Mid },
    WriteVideo { mid: Mid, rid: Option<&'static str>, rtp_time: u64, data: Vec<u8> },
    Restart { config: Box<PeerConfig>, offer: bool, reply: oneshot::Sender<Result<Option<String>>> },
    Sever,
    Close,
}

/// The handle to a running peer connection. Dropping it closes the peer.
pub struct Peer {
    commands: mpsc::UnboundedSender<Command>,
}

impl Peer {
    /// Gather candidates (allocating on every TURN server) and start the
    /// connection's loop.
    pub async fn start(config: PeerConfig) -> Result<(Peer, mpsc::UnboundedReceiver<PeerEvent>)> {
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        // No reordering in str0m: by default it holds audio back for up to
        // 15 packets (or a second) waiting for a lost one, then releases a
        // burst, which the jitter buffer can only take for a stall followed
        // by too much delay. The jitter buffer orders by RTP time itself.
        let mut rtc = Rtc::builder().set_reordering_size_audio(0).build(Instant::now());
        let (transports, candidates) = gather(&config, &inbound_tx, 0).await?;
        for candidate in &candidates {
            rtc.add_local_candidate(candidate.clone());
        }

        let (commands, commands_rx) = mpsc::unbounded_channel();
        let (events, events_rx) = mpsc::unbounded_channel();
        let mut driver = Driver {
            name: config.name,
            rtc,
            transports: transports.into_iter().map(Some).collect(),
            local_candidates: candidates,
            inbound_tx,
            outbox: Vec::new(),
            events,
            pending_offer: None,
            timeout: Instant::now(),
            last_received: None,
            path_alive: true,
            impairment: config.impairment,
            rng: config.impairment.map_or(1, |i| i.seed.max(1)),
            delayed: std::collections::BinaryHeap::new(),
            delayed_seq: 0,
        };
        // Rule 1: drain after the first mutations too.
        driver.drain()?;
        tokio::spawn(driver.run(commands_rx, inbound_rx));
        Ok((Peer { commands }, events_rx))
    }

    async fn ask<T>(&self, make: impl FnOnce(oneshot::Sender<Result<T>>) -> Command) -> Result<T> {
        let (reply, answer) = oneshot::channel();
        self.commands.send(make(reply)).map_err(|_| PeerError::Closed)?;
        answer.await.map_err(|_| PeerError::Closed)?
    }

    /// Offer a send-only audio line (the publisher connection). The SDP's
    /// `msid` carries `stream_id` and `track_id`; LiveKit matches the latter
    /// against the `cid` of the track it was asked to publish.
    pub async fn offer_audio(&self, stream_id: &str, track_id: &str) -> Result<(String, Mid)> {
        self.ask(|reply| Command::OfferMedia { kind: MediaKind::Audio, stream_id: stream_id.into(), track_id: track_id.into(), rids: Vec::new(), reply }).await
    }

    /// Add a send-only video line (a camera), as [`Peer::offer_audio`]
    /// does audio: the new offer, for the server to answer. `rids` names
    /// its simulcast layers, if it has them.
    pub async fn offer_video(&self, stream_id: &str, track_id: &str, rids: &[&str]) -> Result<(String, Mid)> {
        let rids = rids.iter().map(|r| (*r).to_owned()).collect();
        self.ask(|reply| Command::OfferMedia { kind: MediaKind::Video, stream_id: stream_id.into(), track_id: track_id.into(), rids, reply }).await
    }

    pub async fn accept_answer(&self, sdp: &str) -> Result<()> {
        self.ask(|reply| Command::AcceptAnswer { sdp: sdp.into(), reply }).await
    }

    /// Answer the remote side's offer (the subscriber connection).
    pub async fn accept_offer(&self, sdp: &str) -> Result<String> {
        self.ask(|reply| Command::AcceptOffer { sdp: sdp.into(), reply }).await
    }

    /// A remote candidate, as an SDP `candidate:` line. An empty line is
    /// end-of-candidates and is ignored.
    pub fn add_remote_candidate(&self, candidate: &str) -> Result<()> {
        self.commands.send(Command::RemoteCandidate(candidate.into())).map_err(|_| PeerError::Closed)
    }

    /// Send one encoded Opus frame; `rtp_time` counts samples at 48 kHz.
    pub fn write_audio(&self, mid: Mid, rtp_time: u64, data: Vec<u8>) -> Result<()> {
        self.commands.send(Command::WriteAudio { mid, rtp_time, data, audio_level: None }).map_err(|_| PeerError::Closed)
    }

    /// Send one encoded Opus frame with RFC 6464 `ssrc-audio-level` metadata
    /// (`audio_level` in negative dBov `-127..=0`, `voice_activity` true when speaking)
    /// so LiveKit's server-side active-speaker detection lights up for other clients.
    pub fn write_audio_with_level(&self, mid: Mid, rtp_time: u64, data: Vec<u8>, audio_level: i8, voice_activity: bool) -> Result<()> {
        self.commands
            .send(Command::WriteAudio { mid, rtp_time, data, audio_level: Some((audio_level, voice_activity)) })
            .map_err(|_| PeerError::Closed)
    }

    /// One encoded VP8 frame on a send line (on simulcast layer `rid`),
    /// at its RTP time (90 kHz).
    pub fn write_video(&self, mid: Mid, rid: Option<&'static str>, rtp_time: u64, data: Vec<u8>) -> Result<()> {
        self.commands.send(Command::WriteVideo { mid, rid, rtp_time, data }).map_err(|_| PeerError::Closed)
    }

    /// Ask the sender of a received video track for a full picture (PLI).
    pub fn request_keyframe(&self, mid: Mid) -> Result<()> {
        self.commands.send(Command::RequestKeyframe { mid }).map_err(|_| PeerError::Closed)
    }

    /// The network changed: gather afresh from `config` (new TURN
    /// credentials, a new default route) and restart ICE, keeping DTLS and
    /// the media. With `offer` (the publisher), returns the ICE-restart offer
    /// to send; otherwise (the subscriber) the remote's next offer restarts
    /// it, and the answer carries the new candidates.
    pub async fn restart(&self, config: PeerConfig, offer: bool) -> Result<Option<String>> {
        self.ask(|reply| Command::Restart { config: Box::new(config), offer, reply }).await
    }

    pub fn close(&self) {
        let _ = self.commands.send(Command::Close);
    }

    /// Close every socket without a word to anyone: the network gone. For
    /// testing reconnects; [`Peer::restart`] brings it back.
    #[doc(hidden)]
    pub fn sever(&self) {
        let _ = self.commands.send(Command::Sever);
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.close();
    }
}

/// A datagram or stream message for the loop, from whichever transport.
struct Inbound {
    transport: usize,
    /// The remote address; `None` for TURN, whose messages say it themselves.
    source: Option<SocketAddr>,
    data: Vec<u8>,
}

enum Transport {
    Udp { socket: Arc<UdpSocket>, local: SocketAddr, _reader: Reader },
    Tcp { local: SocketAddr, streams: HashMap<SocketAddr, (tokio::net::tcp::OwnedWriteHalf, Reader)> },
    Relay(Box<Relay>),
}

/// A TURN allocation and the connection to its server.
struct Relay {
    uri: String,
    turn: Turn,
    write: ServerWrite,
    _reader: Option<Reader>,
}

enum ServerWrite {
    Udp(Arc<UdpSocket>, SocketAddr),
    Stream(Box<dyn AsyncWrite + Unpin + Send>),
}

impl ServerWrite {
    async fn send(&mut self, msg: &[u8]) -> std::io::Result<()> {
        match self {
            ServerWrite::Udp(socket, server) => {
                // A full socket buffer loses a packet; RTP and TURN's own
                // retransmissions survive that.
                let _ = socket.try_send_to(msg, *server);
                Ok(())
            }
            ServerWrite::Stream(write) => write.write_all(msg).await,
        }
    }
}

/// Whole messages from a TURN server.
enum ServerRead {
    Udp(Arc<UdpSocket>),
    Stream { read: Box<dyn AsyncRead + Unpin + Send>, pending: Vec<u8> },
}

impl ServerRead {
    async fn next(&mut self) -> Option<Vec<u8>> {
        match self {
            ServerRead::Udp(socket) => {
                let mut buf = vec![0u8; 2048];
                let (n, _) = socket.recv_from(&mut buf).await.ok()?;
                buf.truncate(n);
                Some(buf)
            }
            ServerRead::Stream { read, pending } => loop {
                if let Some(len) = frame_len(pending).filter(|l| pending.len() >= *l) {
                    return Some(pending.drain(..len).collect());
                }
                let mut chunk = [0u8; 4096];
                match read.read(&mut chunk).await {
                    Ok(0) | Err(_) => return None,
                    Ok(n) => pending.extend_from_slice(&chunk[..n]),
                }
            },
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum TurnTransport {
    Udp,
    Tcp,
    Tls,
}

/// `turn:host[:port][?transport=…]` / `turns:host[:port]` → host, port, transport.
fn parse_turn_uri(uri: &str) -> Option<(String, u16, TurnTransport)> {
    let (scheme, rest) = uri.split_once(':')?;
    let (host_port, query) = rest.split_once('?').unwrap_or((rest, ""));
    let transport = match (scheme, query.contains("transport=tcp")) {
        ("turns", _) => TurnTransport::Tls,
        ("turn", true) => TurnTransport::Tcp,
        ("turn", false) => TurnTransport::Udp,
        _ => return None,
    };
    // `[v6]:port`, `host:port` or bare `host`.
    let (host, port) = if let Some(rest) = host_port.strip_prefix('[') {
        let (h, p) = rest.split_once(']')?;
        (h.to_owned(), p.strip_prefix(':').map(str::parse).transpose().ok()?)
    } else {
        match host_port.rsplit_once(':') {
            Some((h, p)) => (h.to_owned(), Some(p.parse().ok()?)),
            None => (host_port.to_owned(), None),
        }
    };
    let port = port.unwrap_or(if transport == TurnTransport::Tls { 5349 } else { 3478 });
    Some((host, port, transport))
}

/// Every candidate `config` asks for, with the transports behind them.
/// Transport indices start at `first`, so readers report where they belong.
async fn gather(
    config: &PeerConfig,
    inbound_tx: &mpsc::UnboundedSender<Inbound>,
    first: usize,
) -> Result<(Vec<Transport>, Vec<Candidate>)> {
    let mut transports = Vec::new();
    let mut candidates = Vec::new();
    let candidate_error = |e: str0m::error::IceError| PeerError::Candidate(e.to_string());

    for ip in &config.host_addresses {
        match UdpSocket::bind(SocketAddr::new(*ip, 0)).await {
            Ok(socket) => {
                let socket = Arc::new(socket);
                let local = socket.local_addr()?;
                candidates.push(Candidate::host(local, "udp").map_err(candidate_error)?);
                let reader = spawn_udp_reader(first + transports.len(), socket.clone(), inbound_tx.clone());
                transports.push(Transport::Udp { socket, local, _reader: reader });
            }
            Err(e) => tracing::warn!("{}: no UDP candidate on {ip}: {e}", config.name),
        }
    }

    if config.ice_tcp {
        // Active candidates never accept, so their port is the discard
        // port (RFC 6544 §4.5). The address is only a label for routing.
        let ip = config
            .host_addresses
            .first()
            .copied()
            .or_else(default_route_address)
            .unwrap_or(IpAddr::from([127, 0, 0, 1]));
        let local = SocketAddr::new(ip, 9);
        let candidate = Candidate::builder().tcp().host(local).tcptype(TcpType::Active).build().map_err(candidate_error)?;
        candidates.push(candidate);
        transports.push(Transport::Tcp { local, streams: HashMap::new() });
    }

    // All TURN allocations at once: each can take a few round trips.
    let allocations = config.turn_servers.iter().map(|server| {
        let config = config.clone();
        let server = server.clone();
        async move { (server.uri.clone(), allocate(&config, &server).await) }
    });
    for (uri, outcome) in futures_join_all(allocations).await {
        match outcome {
            Ok((mut relay, mut reader)) => {
                let relayed = relay.turn.relayed().expect("allocated");
                candidates.push(Candidate::relayed(relayed, relayed, "udp").map_err(candidate_error)?);
                let index = first + transports.len();
                let tx = inbound_tx.clone();
                relay._reader = Some(Reader::spawn(async move {
                    while let Some(msg) = reader.next().await {
                        if tx.send(Inbound { transport: index, source: None, data: msg }).is_err() {
                            break;
                        }
                    }
                }));
                tracing::info!("{}: relayed candidate {relayed} via {uri}", config.name);
                transports.push(Transport::Relay(Box::new(relay)));
            }
            Err(e) => tracing::warn!("{}: {e}", config.name),
        }
    }

    if transports.is_empty() {
        return Err(PeerError::NoCandidates);
    }
    Ok((transports, candidates))
}

/// A reader task, stopped when its transport goes.
struct Reader(tokio::task::AbortHandle);

impl Reader {
    fn spawn(task: impl std::future::Future<Output = ()> + Send + 'static) -> Reader {
        Reader(tokio::spawn(task).abort_handle())
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Connect to a TURN server and allocate, within a deadline.
async fn allocate(config: &PeerConfig, server: &TurnServer) -> Result<(Relay, ServerRead)> {
    let setup = |reason: String| PeerError::TurnSetup { server: server.uri.clone(), reason };
    let (host, port, transport) = parse_turn_uri(&server.uri).ok_or_else(|| setup("unrecognised URI".into()))?;
    let address = match config.turn_address_override {
        Some(ip) => SocketAddr::new(ip, port),
        None => tokio::net::lookup_host((host.as_str(), port))
            .await?
            .next()
            .ok_or_else(|| setup(format!("{host} doesn't resolve")))?,
    };

    let (mut write, mut read) = match transport {
        TurnTransport::Udp => {
            let any = if address.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
            let socket = Arc::new(UdpSocket::bind(any).await?);
            (ServerWrite::Udp(socket.clone(), address), ServerRead::Udp(socket))
        }
        TurnTransport::Tcp => {
            let tcp = TcpStream::connect(address).await?;
            tcp.set_nodelay(true)?;
            let (r, w) = tcp.into_split();
            (ServerWrite::Stream(Box::new(w)), ServerRead::Stream { read: Box::new(r), pending: Vec::new() })
        }
        TurnTransport::Tls => {
            let roots = config.tls_roots.clone().unwrap_or_else(|| {
                let mut roots = rustls::RootCertStore::empty();
                roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                Arc::new(roots)
            });
            let tls_config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|e| setup(e.to_string()))?
                .with_root_certificates(roots)
                .with_no_client_auth();
            let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|e| setup(e.to_string()))?;
            let tcp = TcpStream::connect(address).await?;
            tcp.set_nodelay(true)?;
            let tls = tokio_rustls::TlsConnector::from(Arc::new(tls_config))
                .connect(name, tcp)
                .await
                .map_err(|e| setup(format!("TLS: {e}")))?;
            let (r, w) = tokio::io::split(tls);
            (ServerWrite::Stream(Box::new(w)), ServerRead::Stream { read: Box::new(r), pending: Vec::new() })
        }
    };

    let mut turn = Turn::new(&server.username, &server.credential, transport != TurnTransport::Udp);
    write.send(&turn.allocate(Instant::now())).await?;
    let turn_error = |error| PeerError::Turn { server: server.uri.clone(), error };
    loop {
        let wake = turn.next_timeout().unwrap_or_else(|| Instant::now() + Duration::from_secs(1));
        tokio::select! {
            msg = read.next() => {
                let msg = msg.ok_or_else(|| setup("the server closed the connection".into()))?;
                let event = turn.handle(&msg, Instant::now()).map_err(turn_error)?;
                for out in std::mem::take(&mut turn.outgoing) {
                    write.send(&out).await?;
                }
                if let TurnEvent::Allocated { .. } = event {
                    break;
                }
            }
            _ = tokio::time::sleep_until(wake.into()) => {
                for out in turn.poll(Instant::now()).map_err(turn_error)? {
                    write.send(&out).await?;
                }
            }
        }
    }
    Ok((Relay { uri: server.uri.clone(), turn, write, _reader: None }, read))
}

fn spawn_udp_reader(index: usize, socket: Arc<UdpSocket>, tx: mpsc::UnboundedSender<Inbound>) -> Reader {
    Reader::spawn(async move {
        let mut buf = vec![0u8; 2048];
        while let Ok((n, source)) = socket.recv_from(&mut buf).await {
            if tx.send(Inbound { transport: index, source: Some(source), data: buf[..n].to_vec() }).is_err() {
                break;
            }
        }
    })
}

/// `join_all` without pulling in `futures`: tasks on the current runtime.
async fn futures_join_all<F, T>(futures: impl Iterator<Item = F>) -> Vec<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handles: Vec<_> = futures.map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok(value) = handle.await {
            out.push(value);
        }
    }
    out
}

struct Driver {
    name: String,
    rtc: Rtc,
    /// Indexed as the readers report; a restart retires the old ones to
    /// `None` so a late packet from them can't land on a new transport.
    transports: Vec<Option<Transport>>,
    /// Ours, to invalidate on a restart.
    local_candidates: Vec<Candidate>,
    inbound_tx: mpsc::UnboundedSender<Inbound>,
    /// Datagrams str0m wants sent: (proto, from, to, bytes).
    outbox: Vec<(Protocol, SocketAddr, SocketAddr, Vec<u8>)>,
    events: mpsc::UnboundedSender<PeerEvent>,
    pending_offer: Option<SdpPendingOffer>,
    timeout: Instant,
    /// When anything last came in, once something has.
    last_received: Option<Instant>,
    path_alive: bool,
    impairment: Option<Impairment>,
    rng: u64,
    delayed: std::collections::BinaryHeap<Delayed>,
    delayed_seq: u64,
}

impl Driver {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>, mut inbound: mpsc::UnboundedReceiver<Inbound>) {
        let reason = loop {
            let wake = self.next_wake();
            let step: Result<bool> = tokio::select! {
                command = commands.recv() => match command {
                    None | Some(Command::Close) => break "closed".to_owned(),
                    Some(Command::Restart { config, offer, reply }) => {
                        let _ = reply.send(self.restart(*config, offer).await);
                        Ok(true)
                    }
                    Some(command) => self.command(command).map(|_| true),
                },
                Some(inbound) = inbound.recv() => self.arrive(inbound).map(|_| true),
                _ = tokio::time::sleep_until(wake.into()) => self.tick(),
            };
            match step {
                Ok(true) => {}
                Ok(false) => break "the connection ended".to_owned(),
                Err(e) => break e.to_string(),
            }
            if let Err(e) = self.flush().await {
                break e.to_string();
            }
        };
        self.shut_down().await;
        tracing::debug!("{}: closed: {reason}", self.name);
        let _ = self.events.send(PeerEvent::Closed(reason));
    }

    fn next_wake(&self) -> Instant {
        let relays = self.transports.iter().flatten().filter_map(|t| match t {
            Transport::Relay(relay) => relay.turn.next_timeout(),
            _ => None,
        });
        // At least once a second, for the path check in `tick`.
        let wake = relays.fold(self.timeout, Instant::min).min(Instant::now() + Duration::from_secs(1));
        self.delayed.peek().map_or(wake, |d| wake.min(d.at))
    }

    /// Rule 1: poll until str0m has nothing left but a timeout.
    fn drain(&mut self) -> Result<()> {
        loop {
            match self.rtc.poll_output()? {
                Output::Timeout(t) => {
                    self.timeout = t;
                    return Ok(());
                }
                Output::Transmit(t) => self.outbox.push((t.proto, t.source, t.destination, t.contents.to_vec())),
                Output::Event(event) => self.event(event),
            }
        }
    }

    /// Rule 2: advance str0m's clock now, so new pairs get checked.
    fn nudge(&mut self) -> Result<()> {
        self.rtc.handle_input(Input::Timeout(Instant::now()))?;
        self.drain()
    }

    fn event(&mut self, event: Event) {
        let out = match event {
            // Rule 3: report, never fail on, Disconnected.
            Event::IceConnectionStateChange(state) => PeerEvent::IceState(state),
            Event::Connected => PeerEvent::Connected,
            Event::MediaAdded(added) => PeerEvent::MediaAdded { mid: added.mid, kind: added.kind },
            Event::MediaData(data) if data.params.spec().codec == Codec::Opus => {
                let rtp_time = data.time.rebase(str0m::media::Frequency::FORTY_EIGHT_KHZ).numer();
                PeerEvent::Audio { mid: data.mid, rtp_time, data: data.data }
            }
            Event::KeyframeRequest(request) => PeerEvent::KeyframeRequest { mid: request.mid },
            Event::MediaData(data) if data.params.spec().codec == Codec::Vp8 => {
                let rtp_time = data.time.rebase(str0m::media::Frequency::NINETY_KHZ).numer();
                let keyframe = data.is_keyframe();
                PeerEvent::Video { mid: data.mid, rtp_time, keyframe, contiguous: data.contiguous, data: data.data }
            }
            _ => return,
        };
        let _ = self.events.send(out);
    }

    fn command(&mut self, command: Command) -> Result<()> {
        match command {
            Command::OfferMedia { kind, stream_id, track_id, rids, reply } => {
                let result = (|| {
                    let mut change = self.rtc.sdp_api();
                    let simulcast = (!rids.is_empty()).then(|| {
                        let mut simulcast = str0m::media::Simulcast::new();
                        for rid in &rids {
                            simulcast.add_send_layer(str0m::media::SimulcastLayer::new(rid));
                        }
                        simulcast
                    });
                    let mid = change.add_media(kind, Direction::SendOnly, Some(stream_id), Some(track_id), simulcast);
                    let (offer, pending) = change.apply().ok_or_else(|| PeerError::Sdp("nothing to offer".into()))?;
                    self.pending_offer = Some(pending);
                    self.drain()?;
                    Ok((offer.to_sdp_string(), mid))
                })();
                let _ = reply.send(result);
            }
            Command::AcceptAnswer { sdp, reply } => {
                let result = (|| {
                    let pending = self.pending_offer.take().ok_or(PeerError::NoPendingOffer)?;
                    let answer = SdpAnswer::from_sdp_string(&sdp).map_err(|e| PeerError::Sdp(e.to_string()))?;
                    self.rtc.sdp_api().accept_answer(pending, answer)?;
                    self.drain()?;
                    self.nudge()
                })();
                let _ = reply.send(result);
            }
            Command::AcceptOffer { sdp, reply } => {
                let result = (|| {
                    let offer = SdpOffer::from_sdp_string(&sdp).map_err(|e| PeerError::Sdp(e.to_string()))?;
                    let answer = self.rtc.sdp_api().accept_offer(offer)?;
                    self.drain()?;
                    self.nudge()?;
                    Ok(answer.to_sdp_string())
                })();
                let _ = reply.send(result);
            }
            Command::RemoteCandidate(line) => {
                let line = line.trim();
                if line.is_empty() {
                    return Ok(());
                }
                match Candidate::from_sdp_string(line) {
                    Ok(candidate) => {
                        self.rtc.add_remote_candidate(candidate);
                        self.drain()?;
                        self.nudge()?;
                    }
                    // A candidate type str0m doesn't handle shouldn't end the call.
                    Err(e) => tracing::debug!("{}: skipping candidate {line:?}: {e}", self.name),
                }
            }
            Command::WriteAudio { mid, rtp_time, data, audio_level } => {
                let Some(mut writer) = self.rtc.writer(mid) else { return Ok(()) };
                let Some(pt) = writer.payload_params().find(|p| p.spec().codec == Codec::Opus).map(|p| p.pt()) else {
                    return Ok(()); // not negotiated yet
                };
                if let Some((level, vad)) = audio_level {
                    writer = writer.audio_level(level, vad);
                }
                writer.write(pt, Instant::now(), MediaTime::new(rtp_time, str0m::media::Frequency::FORTY_EIGHT_KHZ), data)?;
                self.drain()?;
            }
            Command::WriteVideo { mid, rid, rtp_time, data } => {
                let Some(writer) = self.rtc.writer(mid) else { return Ok(()) };
                let Some(pt) = writer.payload_params().find(|p| p.spec().codec == Codec::Vp8).map(|p| p.pt()) else {
                    return Ok(()); // not negotiated (yet)
                };
                let writer = match rid {
                    Some(rid) => writer.rid(str0m::media::Rid::from(rid)),
                    None => writer,
                };
                writer.write(pt, Instant::now(), MediaTime::new(rtp_time, str0m::media::Frequency::NINETY_KHZ), data)?;
                self.drain()?;
            }
            Command::RequestKeyframe { mid } => {
                if let Some(mut writer) = self.rtc.writer(mid)
                    && let Err(e) = writer.request_keyframe(None, str0m::media::KeyframeRequestKind::Pli)
                {
                    tracing::debug!("{}: keyframe request on {mid}: {e}", self.name);
                }
                self.drain()?;
            }
            Command::Sever => {
                for slot in &mut self.transports {
                    *slot = None;
                }
            }
            Command::Close | Command::Restart { .. } => {}
        }
        Ok(())
    }

    /// A packet in: through the impairment, if there is one.
    fn arrive(&mut self, inbound: Inbound) -> Result<()> {
        let Some(impairment) = self.impairment else { return self.receive(inbound) };
        let mut uniform = || {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            (self.rng >> 11) as f64 / (1u64 << 53) as f64
        };
        if uniform() < impairment.loss {
            return Ok(());
        }
        let at = Instant::now() + impairment.delay + impairment.jitter.mul_f64(uniform());
        self.delayed_seq += 1;
        self.delayed.push(Delayed { at, seq: self.delayed_seq, inbound });
        Ok(())
    }

    fn receive(&mut self, inbound: Inbound) -> Result<()> {
        let now = Instant::now();
        let Some(Some(transport)) = self.transports.get_mut(inbound.transport) else {
            return Ok(()); // from a transport a restart retired
        };
        let (proto, source, destination, data) = match transport {
            Transport::Udp { local, .. } => (Protocol::Udp, inbound.source.expect("UDP has a source"), *local, inbound.data),
            Transport::Tcp { local, .. } => (Protocol::Tcp, inbound.source.expect("TCP has a source"), *local, inbound.data),
            Transport::Relay(relay) => match relay.turn.handle(&inbound.data, now) {
                Ok(TurnEvent::Data { peer, payload }) => {
                    let relayed = relay.turn.relayed().expect("allocated");
                    (Protocol::Udp, peer, relayed, payload)
                }
                Ok(_) => return Ok(()),
                Err(e) => {
                    let _ = self.events.send(PeerEvent::RelayLost(format!("{}: {e}", relay.uri)));
                    return Ok(());
                }
            },
        };
        self.last_received = Some(now);
        if !self.path_alive {
            self.path_alive = true;
            let _ = self.events.send(PeerEvent::Path { alive: true });
        }
        // Not STUN, DTLS, RTP or RTCP: nothing str0m wants. Drop it.
        let Ok(contents) = data.as_slice().try_into() else { return Ok(()) };
        self.rtc.handle_input(Input::Receive(now, Receive { proto, source, destination, contents }))?;
        self.drain()
    }

    /// Timers: str0m's own, and each relay's retransmissions and refreshes.
    /// `false` once str0m says the connection is over.
    fn tick(&mut self) -> Result<bool> {
        let now = Instant::now();
        while self.delayed.peek().is_some_and(|d| d.at <= now) {
            let delayed = self.delayed.pop().expect("peeked");
            self.receive(delayed.inbound)?;
        }
        for transport in self.transports.iter_mut().flatten() {
            if let Transport::Relay(relay) = transport
                && let Err(e) = relay.turn.poll(now).map(|out| relay.turn.outgoing.extend(out))
            {
                let _ = self.events.send(PeerEvent::RelayLost(format!("{}: {e}", relay.uri)));
            }
        }
        if !self.rtc.is_alive() {
            return Ok(false);
        }
        if self.path_alive && self.last_received.is_some_and(|t| now.duration_since(t) > PATH_TIMEOUT) {
            self.path_alive = false;
            let _ = self.events.send(PeerEvent::Path { alive: false });
        }
        self.rtc.handle_input(Input::Timeout(now))?;
        self.drain()?;
        Ok(true)
    }

    /// Put everything queued on the wire.
    async fn flush(&mut self) -> Result<()> {
        let now = Instant::now();
        let tcp_index = self.transports.iter().position(|t| matches!(t, Some(Transport::Tcp { .. })));
        for (proto, source, destination, data) in std::mem::take(&mut self.outbox) {
            let Some(transport) = self.transports.iter_mut().flatten().find(|t| match t {
                Transport::Udp { local, .. } => proto == Protocol::Udp && *local == source,
                Transport::Tcp { .. } => proto == Protocol::Tcp,
                Transport::Relay(relay) => proto == Protocol::Udp && relay.turn.relayed() == Some(source),
            }) else {
                tracing::debug!("{}: no transport for {proto:?} from {source}", self.name);
                continue;
            };
            match transport {
                Transport::Udp { socket, .. } => {
                    let _ = socket.try_send_to(&data, destination);
                }
                Transport::Tcp { streams, .. } => {
                    let stream = match streams.entry(destination) {
                        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            // Connect on first use; ICE-TCP's checks are what open it.
                            let connect = tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(destination));
                            let Ok(Ok(stream)) = connect.await else { continue };
                            stream.set_nodelay(true)?;
                            let (read, write) = stream.into_split();
                            let index = tcp_index.expect("this is the TCP transport");
                            let reader = spawn_rfc4571_reader(index, destination, read, self.inbound_tx.clone());
                            entry.insert((write, reader))
                        }
                    };
                    let stream = &mut stream.0;
                    // RFC 4571: each packet prefixed with its length.
                    let mut framed = Vec::with_capacity(data.len() + 2);
                    framed.extend_from_slice(&(data.len() as u16).to_be_bytes());
                    framed.extend_from_slice(&data);
                    if stream.write_all(&framed).await.is_err() {
                        streams.remove(&destination);
                    }
                }
                Transport::Relay(relay) => {
                    let wire = relay.turn.send_to(destination, &data, now);
                    for msg in wire {
                        relay.write.send(&msg).await?;
                    }
                }
            }
        }
        // Whatever TURN produced on its own: auth retries, refreshes, rebinds.
        for transport in self.transports.iter_mut().flatten() {
            if let Transport::Relay(relay) = transport {
                for msg in std::mem::take(&mut relay.turn.outgoing) {
                    relay.write.send(&msg).await?;
                }
            }
        }
        Ok(())
    }

    /// New sockets, allocations and candidates for a changed network; the
    /// old ones retired. With `offer`, an ICE-restart offer for the remote.
    async fn restart(&mut self, config: PeerConfig, offer: bool) -> Result<Option<String>> {
        self.shut_down().await;
        for slot in &mut self.transports {
            *slot = None;
        }
        for candidate in std::mem::take(&mut self.local_candidates) {
            self.rtc.direct_api().invalidate_candidate(&candidate);
        }
        let (transports, candidates) = gather(&config, &self.inbound_tx, self.transports.len()).await?;
        for candidate in &candidates {
            self.rtc.add_local_candidate(candidate.clone());
        }
        self.local_candidates = candidates;
        self.transports.extend(transports.into_iter().map(Some));
        tracing::info!("{}: restarted on {} new candidates", self.name, self.local_candidates.len());

        let sdp = if offer {
            let mut change = self.rtc.sdp_api();
            change.ice_restart(true);
            let (offer, pending) = change.apply().ok_or_else(|| PeerError::Sdp("nothing to offer".into()))?;
            self.pending_offer = Some(pending);
            Some(offer.to_sdp_string())
        } else {
            None
        };
        self.drain()?;
        self.nudge()?;
        Ok(sdp)
    }

    /// Release TURN allocations so the server doesn't hold them for ten minutes.
    async fn shut_down(&mut self) {
        let now = Instant::now();
        for transport in self.transports.iter_mut().flatten() {
            if let Transport::Relay(relay) = transport
                && let Some(msg) = relay.turn.deallocate(now)
            {
                let _ = relay.write.send(&msg).await;
            }
        }
    }
}

fn spawn_rfc4571_reader(
    index: usize,
    remote: SocketAddr,
    mut read: tokio::net::tcp::OwnedReadHalf,
    tx: mpsc::UnboundedSender<Inbound>,
) -> Reader {
    Reader::spawn(async move {
        while let Ok(len) = read.read_u16().await {
            let mut data = vec![0u8; len as usize];
            if read.read_exact(&mut data).await.is_err() {
                break;
            }
            if tx.send(Inbound { transport: index, source: Some(remote), data }).is_err() {
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_uris_in_the_shapes_servers_hand_out() {
        let cases = [
            ("turn:turn.example.org", ("turn.example.org", 3478, TurnTransport::Udp)),
            ("turn:turn.example.org?transport=udp", ("turn.example.org", 3478, TurnTransport::Udp)),
            ("turn:turn.example.org:3479?transport=tcp", ("turn.example.org", 3479, TurnTransport::Tcp)),
            ("turns:turn.example.org:443?transport=tcp", ("turn.example.org", 443, TurnTransport::Tls)),
            ("turns:turn.example.org", ("turn.example.org", 5349, TurnTransport::Tls)),
            ("turn:[2001:db8::1]:3478", ("2001:db8::1", 3478, TurnTransport::Udp)),
        ];
        for (uri, (host, port, transport)) in cases {
            assert_eq!(parse_turn_uri(uri), Some((host.to_owned(), port, transport)), "{uri}");
        }
        assert_eq!(parse_turn_uri("stun:stun.example.org"), None);
        assert_eq!(parse_turn_uri("turn:host:notaport"), None);
    }

    #[test]
    fn the_default_route_address_is_not_loopback() {
        // A machine with no network at all has none, which is fine.
        if let Some(ip) = default_route_address() {
            assert!(!ip.is_loopback() && !ip.is_unspecified(), "{ip}");
        }
    }
}
