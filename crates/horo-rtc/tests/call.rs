//! A call through a real LiveKit server, on horo's own crates end to end:
//! [`horo_rtc::livekit`] for signalling, [`horo_rtc::peer`] for the media.
//!
//! Alice publishes a 440 Hz tone, Opus-encoded; Bob subscribes and has to
//! hear exactly that tone. Run once per way of reaching the SFU.
//!
//! ```sh
//! scripts/test-livekit/start.sh              # direct: UDP and ICE-TCP
//! eval "$(scripts/test-livekit/start-turn.sh)"  # relayed: TURN/UDP, TURN/TLS
//! HORO_TEST_LIVEKIT=ws://127.0.0.1:7880 cargo test -p horo-rtc --test call
//! ```

use std::{
    net::{IpAddr, Ipv4Addr},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use horo_rtc::livekit::{AddTrack, SignalEvent, Target, connect, proto};
use horo_rtc::{Peer, PeerConfig, PeerEvent, TurnServer};

const KEY: &str = "devkey";
/// `livekit-server --dev`'s secret.
const DEV_SECRET: &str = "secret";
/// start-turn.sh's: outside --dev, LiveKit wants 32 characters or more.
const TURN_SECRET: &str = "horo-test-secret-that-is-at-least-32-characters";
const TONE_HZ: f64 = 440.0;
const RATE: u32 = 48_000;
/// 20 ms of audio at 48 kHz, the frame every WebRTC client sends.
const FRAME: usize = 960;
const LOCALHOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// A LiveKit server to call through.
#[derive(Clone)]
struct Server {
    url: String,
    secret: &'static str,
    /// The CA that signed the TURN/TLS certificate, for the relayed server.
    turn_ca: Option<String>,
}

fn env(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Some(value),
        _ if std::env::var_os("CI").is_some() => panic!("{name} must be set in CI (see scripts/test-livekit)"),
        _ => {
            eprintln!("{name} not set; skipping");
            None
        }
    }
}

fn direct_server() -> Option<Server> {
    Some(Server { url: env("HORO_TEST_LIVEKIT")?, secret: DEV_SECRET, turn_ca: None })
}

fn turn_server() -> Option<Server> {
    Some(Server { url: env("HORO_TEST_LIVEKIT_TURN")?, secret: TURN_SECRET, turn_ca: Some(env("HORO_TEST_TURN_CA")?) })
}

fn token(identity: &str, room: &str, secret: &str) -> String {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Grant<'a> {
        room: &'a str,
        room_join: bool,
        can_publish: bool,
        can_subscribe: bool,
    }
    #[derive(serde::Serialize)]
    struct Claims<'a> {
        iss: &'a str,
        sub: &'a str,
        nbf: u64,
        exp: u64,
        video: Grant<'a>,
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let claims = Claims {
        iss: KEY,
        sub: identity,
        nbf: now - 10,
        exp: now + 600,
        video: Grant { room, room_join: true, can_publish: true, can_subscribe: true },
    };
    jsonwebtoken::encode(&Default::default(), &claims, &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes())).unwrap()
}

#[derive(Clone, Copy, PartialEq)]
enum Role {
    Publish,
    Listen,
}

/// What a listener heard.
#[derive(Debug)]
struct Heard {
    hz: f64,
    rms: f64,
    first_audio_after: Duration,
}

/// One participant: signal connection plus publisher and subscriber peers.
/// A publisher sends the tone until it is dropped; a listener returns once
/// it has heard a second of audio.
async fn participant(server: &Server, room: &str, identity: &str, role: Role, config: impl Fn(&str, &[TurnServer]) -> PeerConfig) -> Heard {
    let started = Instant::now();
    let (signal, join, mut signal_events) = connect(&server.url, &token(identity, room, server.secret), true).await.expect("join");

    // The ICE servers LiveKit hands out, for configs that want TURN.
    let turn: Vec<TurnServer> = join
        .ice_servers
        .iter()
        .flat_map(|s| s.urls.iter().map(|uri| TurnServer { uri: uri.clone(), username: s.username.clone(), credential: s.credential.clone() }))
        .filter(|s| s.uri.starts_with("turn"))
        .collect();

    let tls_roots = server.turn_ca.as_ref().map(|path| {
        use rustls::pki_types::{CertificateDer, pem::PemObject};
        let mut roots = rustls::RootCertStore::empty();
        for cert in CertificateDer::pem_file_iter(path).expect("the test CA") {
            roots.add(cert.unwrap()).unwrap();
        }
        std::sync::Arc::new(roots)
    });
    let with_roots = |config: PeerConfig| PeerConfig { tls_roots: tls_roots.clone(), ..config };
    let pub_config = with_roots(config(&format!("{identity}/pub"), &turn));
    let sub_config = with_roots(config(&format!("{identity}/sub"), &turn));
    let (publisher, mut pub_events) = Peer::start(pub_config).await.expect("publisher peer");
    let (subscriber, mut sub_events) = Peer::start(sub_config).await.expect("subscriber peer");

    let mut tone_mid = None;
    if role == Role::Publish {
        signal
            .add_track(AddTrack {
                cid: "tone".into(),
                name: "tone".into(),
                kind: proto::TrackType::Audio,
                source: proto::TrackSource::Microphone,
                muted: false,
            })
            .unwrap();
        let (sdp, mid) = publisher.offer_audio("tone-stream", "tone").await.expect("offer");
        signal.offer(sdp, 0).unwrap();
        tone_mid = Some(mid);
    }

    let mut encoder = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip).unwrap();
    let mut decoder = opus::Decoder::new(RATE, opus::Channels::Mono).unwrap();
    let mut publishing = false;
    let mut samples_sent: u64 = 0;
    let mut clock = tokio::time::interval(Duration::from_millis(20));
    let mut heard: Vec<i16> = Vec::new();
    let mut first_audio = None;

    loop {
        tokio::select! {
            event = signal_events.recv() => match event.expect("signal events") {
                SignalEvent::Answer(answer) => publisher.accept_answer(&answer.sdp).await.expect("accept answer"),
                SignalEvent::Offer(offer) => {
                    let answer = subscriber.accept_offer(&offer.sdp).await.expect("answer");
                    signal.answer(answer, offer.id).unwrap();
                }
                SignalEvent::Candidate { target: Target::Publisher, candidate } => publisher.add_remote_candidate(&candidate).unwrap(),
                SignalEvent::Candidate { target: Target::Subscriber, candidate } => subscriber.add_remote_candidate(&candidate).unwrap(),
                SignalEvent::Closed(reason) => panic!("{identity}: signal closed: {reason}"),
                _ => {}
            },
            event = pub_events.recv() => match event.expect("publisher events") {
                PeerEvent::Connected => publishing = true,
                PeerEvent::Closed(reason) => panic!("{identity}: publisher closed: {reason}"),
                _ => {}
            },
            event = sub_events.recv() => match event.expect("subscriber events") {
                PeerEvent::Audio { data, .. } if role == Role::Listen => {
                    first_audio.get_or_insert_with(|| started.elapsed());
                    let mut pcm = vec![0i16; FRAME * 6];
                    let n = decoder.decode(&data, &mut pcm, false).expect("decode");
                    heard.extend_from_slice(&pcm[..n]);
                    // A second of audio; judge the last half (the decoder ramps up).
                    if heard.len() >= RATE as usize {
                        let tail = &heard[heard.len() - RATE as usize / 2..];
                        let crossings = tail.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
                        let hz = crossings as f64 / 2.0 / (tail.len() as f64 / RATE as f64);
                        let rms = (tail.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / tail.len() as f64).sqrt();
                        return Heard { hz, rms, first_audio_after: first_audio.unwrap() };
                    }
                }
                PeerEvent::Closed(reason) => panic!("{identity}: subscriber closed: {reason}"),
                _ => {}
            },
            _ = clock.tick(), if publishing => {
                let pcm: Vec<i16> = (0..FRAME as u64)
                    .map(|i| {
                        // Phase wrapped every second, in f64: an f32 time from a
                        // raw sample count bends the tone after ~2 minutes.
                        let t = ((samples_sent + i) % RATE as u64) as f64 / RATE as f64;
                        ((t * TONE_HZ * std::f64::consts::TAU).sin() * 8000.0) as i16
                    })
                    .collect();
                let mut packet = [0u8; 400];
                let len = encoder.encode(&pcm, &mut packet).unwrap();
                publisher.write_audio(tone_mid.unwrap(), samples_sent, packet[..len].to_vec()).unwrap();
                samples_sent += FRAME as u64;
            }
        }
    }
}

async fn call(name: &str, server: Option<Server>, config: impl Fn(&str, &[TurnServer]) -> PeerConfig + Clone + Send + 'static) {
    let Some(server) = server else { return };
    let room = format!("{name}-{:x}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());

    // Bob is in the room first, so the SFU has to forward a track published later.
    let bob = {
        let (server, room, config) = (server.clone(), room.clone(), config.clone());
        tokio::spawn(async move { participant(&server, &room, "bob", Role::Listen, config).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    let alice = {
        let (server, room) = (server.clone(), room.clone());
        tokio::spawn(async move { participant(&server, &room, "alice", Role::Publish, config).await })
    };

    let heard = tokio::time::timeout(Duration::from_secs(30), bob).await.expect("bob heard nothing in 30 s").unwrap();
    alice.abort();
    eprintln!("{name}: {heard:?}");
    assert!((heard.hz - TONE_HZ).abs() < 5.0, "{name}: heard {:.1} Hz", heard.hz);
    // A sine of amplitude 8000 has an RMS of ~5657.
    assert!(heard.rms > 4000.0, "{name}: tone too quiet (rms {:.0})", heard.rms);
    // Connecting is ICE + DTLS twice over plus the SFU renegotiating Bob's
    // subscription: about a second locally. Five means something stalled.
    assert!(heard.first_audio_after < Duration::from_secs(5), "{name}: first audio after {:?}", heard.first_audio_after);
}

/// Host UDP only: the ordinary case.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_over_udp() {
    call("udp", direct_server(), |name, _| PeerConfig { host_addresses: vec![LOCALHOST], ice_tcp: false, ..PeerConfig::default_for(name) })
        .await;
}

/// ICE-TCP only: a network that blocks UDP.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_over_tcp_only() {
    call("tcp", direct_server(), |name, _| PeerConfig { host_addresses: vec![], ice_tcp: true, ..PeerConfig::default_for(name) }).await;
}

/// UDP and TCP gathered together, as the app does: ICE picks.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_with_every_candidate() {
    call("all", direct_server(), |name, _| PeerConfig { host_addresses: vec![LOCALHOST], ice_tcp: true, ..PeerConfig::default_for(name) })
        .await;
}

/// Only relayed candidates, from the TURN servers LiveKit's join response
/// names, filtered by scheme. The SFU advertises the container's address,
/// which the host can't reach directly, so audio arriving at all proves the
/// relay carried it.
fn relayed_only(name: &str, servers: &[TurnServer], scheme: &str) -> PeerConfig {
    let servers: Vec<TurnServer> = servers.iter().filter(|s| s.uri.starts_with(scheme)).cloned().collect();
    assert!(!servers.is_empty(), "LiveKit offered no {scheme} server");
    PeerConfig {
        host_addresses: vec![],
        ice_tcp: false,
        turn_servers: servers,
        // The servers are named for the container; they're published here.
        turn_address_override: Some(LOCALHOST),
        ..PeerConfig::default_for(name)
    }
}

/// TURN over UDP.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_relayed_over_turn_udp() {
    call("turn-udp", turn_server(), |name, servers| relayed_only(name, servers, "turn:")).await;
}

/// TURN over TLS on 443: the network that allows only HTTPS.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_relayed_over_turn_tls() {
    call("turn-tls", turn_server(), |name, servers| relayed_only(name, servers, "turns:")).await;
}
