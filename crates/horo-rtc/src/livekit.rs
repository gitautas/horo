//! LiveKit signalling: the WebSocket half of joining a LiveKit room.
//!
//! [`connect`] opens the signal connection with a room-scoped access token
//! (from [`crate::matrixrtc`]'s OpenID → JWT exchange) and waits for the server's
//! join response. From then on a background task owns the socket:
//! - requests go out through the [`SignalClient`] handle;
//! - everything the server says comes back as [`SignalEvent`]s;
//! - it pings at the interval the server asked for, and reports
//!   [`SignalEvent::Closed`] if the server stops answering.
//!
//! Media is not here. The SDP and ICE candidates in these messages belong to
//! the peer connections in [`crate::peer`]; this module only carries them.
//!
//! LiveKit uses two peer connections per participant: the *publisher*, which
//! this client offers, and the *subscriber*, which the server offers. Every
//! offer, answer and candidate says which one it is for ([`Target`]).

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
pub use livekit_protocol as proto;
use livekit_protocol::{
    JoinResponse, SignalRequest, SignalResponse, SignalTarget, signal_request, signal_response,
};
use prost::Message as _;
use tokio::{
    sync::mpsc,
    time::{Instant, MissedTickBehavior},
};
use tokio_tungstenite::tungstenite::{self, Message as WsMessage};

/// LiveKit signal protocol version. 15 is two peer connections with the
/// subscriber offered by the server, which every server since 1.x speaks.
pub const PROTOCOL_VERSION: i32 = 15;
const SDK: &str = "rust";

/// How long to wait for the join response before giving up.
const JOIN_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("bad LiveKit URL {0:?}")]
    Url(String),
    /// The server refused the WebSocket upgrade: a bad or expired token is a
    /// 401, a room the token doesn't grant is a 403.
    #[error("LiveKit refused the connection ({status}): {body}")]
    Refused { status: u16, body: String },
    #[error("LiveKit signal connection failed: {0}")]
    WebSocket(#[from] tungstenite::Error),
    #[error("LiveKit sent something that isn't a signal message: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("no join response within {0:?}")]
    JoinTimeout(Duration),
    #[error("the signal connection closed before the join response")]
    ClosedBeforeJoin,
    /// The server told us to leave instead of joining.
    #[error("LiveKit asked us to leave: {0:?}")]
    Left(proto::LeaveRequest),
    #[error("the signal connection is closed")]
    Closed,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Which of a participant's two peer connections a message is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// Ours to offer; carries what we send.
    Publisher,
    /// The server's to offer; carries what we receive.
    Subscriber,
}

impl Target {
    fn from_proto(value: i32) -> Target {
        if value == SignalTarget::Subscriber as i32 { Target::Subscriber } else { Target::Publisher }
    }

    fn to_proto(self) -> i32 {
        match self {
            Target::Publisher => SignalTarget::Publisher as i32,
            Target::Subscriber => SignalTarget::Subscriber as i32,
        }
    }
}

/// An SDP offer or answer, with the id the server uses to pair them.
#[derive(Debug, Clone, PartialEq)]
pub struct Sdp {
    pub sdp: String,
    pub id: u32,
}

/// Something the server said, after the join.
#[derive(Debug, Clone)]
pub enum SignalEvent {
    /// The server's offer for the subscriber connection. Answer it with
    /// [`SignalClient::answer`], passing the same id.
    Offer(Sdp),
    /// The server's answer to our publisher offer.
    Answer(Sdp),
    /// A remote ICE candidate, as an SDP `candidate:` line. An empty line is
    /// end-of-candidates.
    Candidate { target: Target, candidate: String },
    /// A track we asked to publish has a server id now.
    TrackPublished { cid: String, track: Option<Box<proto::TrackInfo>> },
    /// Participants joined, left or changed (tracks, metadata).
    Participants(Vec<proto::ParticipantInfo>),
    /// Who is speaking, loudest first.
    Speakers(Vec<proto::SpeakerInfo>),
    /// The server wants us gone; `can_reconnect` says whether to try again.
    Leave(Box<proto::LeaveRequest>),
    /// Anything else the server sends; not every message matters yet.
    Other(Box<signal_response::Message>),
    /// The connection ended: the server closed it, the network went, or
    /// pings went unanswered. Nothing follows this.
    Closed(String),
}

/// What to publish, as LiveKit's `AddTrackRequest` wants it.
#[derive(Debug, Clone)]
pub struct AddTrack {
    /// Our id for the track, matched against the `msid` track id in the
    /// publisher SDP and echoed back in [`SignalEvent::TrackPublished`].
    pub cid: String,
    pub name: String,
    pub kind: proto::TrackType,
    pub source: proto::TrackSource,
    pub muted: bool,
}

/// The handle to a live signal connection. Cheap to clone; dropping every
/// clone leaves the room.
#[derive(Clone)]
pub struct SignalClient {
    requests: mpsc::UnboundedSender<signal_request::Message>,
    pump: Arc<tokio::task::AbortHandle>,
}

impl SignalClient {
    fn send(&self, message: signal_request::Message) -> Result<()> {
        self.requests.send(message).map_err(|_| Error::Closed)
    }

    /// Offer the publisher connection.
    pub fn offer(&self, sdp: String, id: u32) -> Result<()> {
        self.send(signal_request::Message::Offer(proto::SessionDescription {
            r#type: "offer".into(),
            sdp,
            id,
            ..Default::default()
        }))
    }

    /// Answer the server's subscriber offer; `id` is the offer's.
    pub fn answer(&self, sdp: String, id: u32) -> Result<()> {
        self.send(signal_request::Message::Answer(proto::SessionDescription {
            r#type: "answer".into(),
            sdp,
            id,
            ..Default::default()
        }))
    }

    /// Send a local ICE candidate (an SDP `candidate:` line). LiveKit expects
    /// the browser's `RTCIceCandidateInit` JSON.
    pub fn candidate(&self, target: Target, candidate: &str, sdp_mid: Option<&str>) -> Result<()> {
        let init = serde_json::json!({
            "candidate": candidate,
            "sdpMid": sdp_mid,
            "sdpMLineIndex": 0,
        });
        self.send(signal_request::Message::Trickle(proto::TrickleRequest {
            candidate_init: init.to_string(),
            target: target.to_proto(),
            r#final: false,
        }))
    }

    /// Ask to publish a track; the server answers with
    /// [`SignalEvent::TrackPublished`] carrying the same `cid`.
    pub fn add_track(&self, track: AddTrack) -> Result<()> {
        self.send(signal_request::Message::AddTrack(proto::AddTrackRequest {
            cid: track.cid,
            name: track.name,
            r#type: track.kind as i32,
            source: track.source as i32,
            muted: track.muted,
            ..Default::default()
        }))
    }

    /// Mute or unmute a published track, by the server's track id.
    pub fn mute(&self, track_sid: &str, muted: bool) -> Result<()> {
        self.send(signal_request::Message::Mute(proto::MuteTrackRequest { sid: track_sid.into(), muted }))
    }

    /// Leave the room. The connection closes after this.
    /// Drop the connection on the floor, saying nothing: what a dead network
    /// looks like to the server. For testing reconnects. The event stream
    /// ends without a [`SignalEvent::Closed`].
    #[doc(hidden)]
    pub fn sever(&self) {
        self.pump.abort();
    }

    pub fn leave(&self) -> Result<()> {
        self.send(signal_request::Message::Leave(proto::LeaveRequest {
            reason: proto::DisconnectReason::ClientInitiated as i32,
            ..Default::default()
        }))
    }
}

/// The signal endpoint for a LiveKit URL: `/rtc` with the token and options
/// in the query, over `ws`/`wss` whichever scheme the URL arrived with.
pub fn signal_url(livekit_url: &str, token: &str, auto_subscribe: bool) -> Result<url::Url> {
    let mut url = url::Url::parse(livekit_url).map_err(|_| Error::Url(livekit_url.into()))?;
    let scheme = match url.scheme() {
        "ws" | "http" => "ws",
        "wss" | "https" => "wss",
        _ => return Err(Error::Url(livekit_url.into())),
    };
    url.set_scheme(scheme).map_err(|_| Error::Url(livekit_url.into()))?;
    {
        let mut path = url.path_segments_mut().map_err(|_| Error::Url(livekit_url.into()))?;
        path.pop_if_empty().push("rtc");
    }
    url.query_pairs_mut()
        .clear()
        .append_pair("access_token", token)
        .append_pair("auto_subscribe", if auto_subscribe { "1" } else { "0" })
        .append_pair("protocol", &PROTOCOL_VERSION.to_string())
        .append_pair("sdk", SDK)
        .append_pair("version", env!("CARGO_PKG_VERSION"));
    Ok(url)
}

/// Join a room: open the signal connection and wait for the join response.
///
/// Returns the handle for requests, the join response (ICE servers, who is
/// already here, whether the subscriber connection comes first), and the
/// stream of everything the server says afterwards.
pub async fn connect(
    livekit_url: &str,
    token: &str,
    auto_subscribe: bool,
) -> Result<(SignalClient, JoinResponse, mpsc::UnboundedReceiver<SignalEvent>)> {
    let url = signal_url(livekit_url, token, auto_subscribe)?;
    let mut ws = open(&url).await?;

    let join = tokio::time::timeout(JOIN_TIMEOUT, async {
        loop {
            let Some(frame) = ws.next().await else { return Err(Error::ClosedBeforeJoin) };
            let WsMessage::Binary(bytes) = frame? else { continue };
            match SignalResponse::decode(bytes.as_ref())?.message {
                Some(signal_response::Message::Join(join)) => return Ok(join),
                Some(signal_response::Message::Leave(leave)) => return Err(Error::Left(leave)),
                other => tracing::debug!("before join: {other:?}"),
            }
        }
    })
    .await
    .map_err(|_| Error::JoinTimeout(JOIN_TIMEOUT))??;

    let (client, events) = spawn(ws, join.ping_interval, join.ping_timeout, Vec::new());
    Ok((client, join, events))
}

/// Pick a dropped session back up: a new signal connection for the same
/// participant (`participant_sid`, from the join response), keeping its
/// tracks and subscriptions on the server. What follows is an ICE restart
/// on both peer connections: the server offers one for the subscriber; the
/// publisher's offer is the caller's to send.
///
/// Fails with [`Error::Left`] when the server can't resume (the session
/// timed out, or it moved): then only a fresh [`connect`] will do.
pub async fn resume(
    livekit_url: &str,
    token: &str,
    participant_sid: &str,
    reason: proto::ReconnectReason,
    ping: (i32, i32),
) -> Result<(SignalClient, Option<proto::ReconnectResponse>, mpsc::UnboundedReceiver<SignalEvent>)> {
    let mut url = signal_url(livekit_url, token, true)?;
    url.query_pairs_mut()
        .append_pair("reconnect", "1")
        .append_pair("sid", participant_sid)
        .append_pair("reconnect_reason", &(reason as i32).to_string());
    let mut ws = open(&url).await?;

    // The server answers with a reconnect response; one that predates it
    // just carries on, and its first message is already the session's.
    let (response, first) = tokio::time::timeout(JOIN_TIMEOUT, async {
        loop {
            let Some(frame) = ws.next().await else { return Err(Error::ClosedBeforeJoin) };
            let WsMessage::Binary(bytes) = frame? else { continue };
            match SignalResponse::decode(bytes.as_ref())?.message {
                Some(signal_response::Message::Reconnect(response)) => return Ok((Some(response), None)),
                Some(signal_response::Message::Leave(leave)) => return Err(Error::Left(leave)),
                Some(other) => return Ok((None, Some(other))),
                None => {}
            }
        }
    })
    .await
    .map_err(|_| Error::JoinTimeout(JOIN_TIMEOUT))??;

    let (client, events) = spawn(ws, ping.0, ping.1, first.into_iter().map(event).collect());
    Ok((client, response, events))
}

/// Open the WebSocket, turning an HTTP refusal into its status and body.
async fn open(url: &url::Url) -> Result<Socket> {
    match tokio_tungstenite::connect_async(url.as_str()).await {
        Ok((ws, _)) => Ok(ws),
        // The status and body are the only explanation a refusal comes with.
        Err(tungstenite::Error::Http(response)) => {
            let body = response.body().as_ref().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
            Err(Error::Refused { status: response.status().as_u16(), body: body.trim().to_owned() })
        }
        Err(e) => Err(e.into()),
    }
}

/// Start the connection's pump; `early` are events that arrived during the
/// handshake, delivered first.
fn spawn(ws: Socket, ping_interval: i32, ping_timeout: i32, early: Vec<SignalEvent>) -> (SignalClient, mpsc::UnboundedReceiver<SignalEvent>) {
    let (requests, requests_rx) = mpsc::unbounded_channel();
    let (events, events_rx) = mpsc::unbounded_channel();
    for e in early {
        let _ = events.send(e);
    }
    let interval = Duration::from_secs(ping_interval.max(1) as u64);
    let timeout = Duration::from_secs(ping_timeout.max(ping_interval * 2).max(2) as u64);
    let pump = Arc::new(tokio::spawn(run(ws, requests_rx, events, interval, timeout)).abort_handle());
    (SignalClient { requests, pump }, events_rx)
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The connection's life after the join. Ends by sending exactly one
/// [`SignalEvent::Closed`], unless nobody is listening any more.
async fn run(
    mut ws: Socket,
    mut requests: mpsc::UnboundedReceiver<signal_request::Message>,
    events: mpsc::UnboundedSender<SignalEvent>,
    ping_interval: Duration,
    ping_timeout: Duration,
) {
    let mut ping = tokio::time::interval(ping_interval);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_pong = Instant::now();

    let reason = loop {
        tokio::select! {
            request = requests.recv() => {
                // Every handle dropped: leave politely, then stop.
                let (message, leaving) = match request {
                    Some(message) => {
                        let leaving = matches!(message, signal_request::Message::Leave(_));
                        (message, leaving)
                    }
                    None => (
                        signal_request::Message::Leave(proto::LeaveRequest {
                            reason: proto::DisconnectReason::ClientInitiated as i32,
                            ..Default::default()
                        }),
                        true,
                    ),
                };
                let bytes = SignalRequest { message: Some(message) }.encode_to_vec();
                if let Err(e) = ws.send(WsMessage::Binary(bytes.into())).await {
                    break format!("send failed: {e}");
                }
                if leaving {
                    let _ = ws.close(None).await;
                    break "left the room".to_owned();
                }
            }
            frame = ws.next() => {
                let frame = match frame {
                    None => break "the server closed the connection".to_owned(),
                    Some(Err(e)) => break format!("connection lost: {e}"),
                    Some(Ok(frame)) => frame,
                };
                let bytes = match frame {
                    WsMessage::Binary(bytes) => bytes,
                    WsMessage::Close(close) => break format!("the server closed the connection: {close:?}"),
                    _ => continue,
                };
                let message = match SignalResponse::decode(bytes.as_ref()) {
                    Ok(SignalResponse { message: Some(message) }) => message,
                    Ok(_) => continue,
                    Err(e) => {
                        tracing::warn!("undecodable signal message: {e}");
                        continue;
                    }
                };
                if let signal_response::Message::Pong(_) = message {
                    last_pong = Instant::now();
                    continue;
                }
                let leave = matches!(message, signal_response::Message::Leave(_));
                if events.send(event(message)).is_err() {
                    // Nobody is listening; there is no one to report to.
                    let _ = ws.close(None).await;
                    return;
                }
                if leave {
                    let _ = ws.close(None).await;
                    break "the server asked us to leave".to_owned();
                }
            }
            _ = ping.tick() => {
                if last_pong.elapsed() > ping_timeout {
                    break format!("no pong for {:?}", last_pong.elapsed());
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or_default();
                let bytes = SignalRequest { message: Some(signal_request::Message::Ping(now)) }.encode_to_vec();
                if let Err(e) = ws.send(WsMessage::Binary(bytes.into())).await {
                    break format!("ping failed: {e}");
                }
            }
        }
    };

    tracing::debug!("signal connection ended: {reason}");
    let _ = events.send(SignalEvent::Closed(reason));
}

fn event(message: signal_response::Message) -> SignalEvent {
    use signal_response::Message as M;
    match message {
        M::Offer(o) => SignalEvent::Offer(Sdp { sdp: o.sdp, id: o.id }),
        M::Answer(a) => SignalEvent::Answer(Sdp { sdp: a.sdp, id: a.id }),
        M::Trickle(t) => {
            // `candidate_init` is the browser's RTCIceCandidateInit JSON.
            let candidate = serde_json::from_str::<serde_json::Value>(&t.candidate_init)
                .ok()
                .and_then(|v| v.get("candidate").and_then(|c| c.as_str()).map(str::to_owned))
                .unwrap_or_default();
            SignalEvent::Candidate { target: Target::from_proto(t.target), candidate }
        }
        M::TrackPublished(p) => SignalEvent::TrackPublished { cid: p.cid, track: p.track.map(Box::new) },
        M::Update(u) => SignalEvent::Participants(u.participants),
        M::SpeakersChanged(s) => SignalEvent::Speakers(s.speakers),
        M::Leave(l) => SignalEvent::Leave(Box::new(l)),
        other => SignalEvent::Other(Box::new(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_url_from_the_shapes_jwt_services_return() {
        for (given, expected) in [
            ("wss://lk.example.org", "wss://lk.example.org/rtc"),
            ("wss://lk.example.org/", "wss://lk.example.org/rtc"),
            ("https://lk.example.org/livekit/sfu", "wss://lk.example.org/livekit/sfu/rtc"),
            ("http://127.0.0.1:7880", "ws://127.0.0.1:7880/rtc"),
        ] {
            let url = signal_url(given, "t0k", true).unwrap();
            assert_eq!(&url.as_str()[..url.as_str().find('?').unwrap()], expected, "{given}");
            let query: Vec<_> = url.query_pairs().collect();
            assert!(query.iter().any(|(k, v)| k == "access_token" && v == "t0k"));
            assert!(query.iter().any(|(k, v)| k == "protocol" && v == "15"));
        }
        assert!(matches!(signal_url("ftp://nope", "t", true), Err(Error::Url(_))));
    }

    #[test]
    fn trickle_candidates_unwrap_the_browser_json() {
        let message = signal_response::Message::Trickle(proto::TrickleRequest {
            candidate_init: r#"{"candidate":"candidate:1 1 udp 2130706431 10.0.0.1 7882 typ host","sdpMid":"0"}"#.into(),
            target: SignalTarget::Subscriber as i32,
            r#final: false,
        });
        match event(message) {
            SignalEvent::Candidate { target, candidate } => {
                assert_eq!(target, Target::Subscriber);
                assert!(candidate.starts_with("candidate:1 1 udp"));
            }
            other => panic!("{other:?}"),
        }
    }
}
