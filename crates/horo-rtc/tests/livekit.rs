//! LiveKit signalling against a real LiveKit server.
//!
//! ```sh
//! scripts/test-livekit/start.sh
//! HORO_TEST_LIVEKIT=ws://127.0.0.1:7880 cargo test -p horo-rtc --test livekit
//! ```
//!
//! Without `HORO_TEST_LIVEKIT` these pass without doing anything; in CI a
//! missing server is a failure.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use horo_rtc::livekit::{AddTrack, Error, SignalEvent, connect, proto};
use tokio::sync::mpsc::UnboundedReceiver;

/// `livekit-server --dev`'s credentials.
const KEY: &str = "devkey";
const SECRET: &str = "secret";

fn server() -> Option<String> {
    match std::env::var("HORO_TEST_LIVEKIT") {
        Ok(url) if !url.is_empty() => Some(url),
        _ if std::env::var_os("CI").is_some() => panic!("HORO_TEST_LIVEKIT must be set in CI (see scripts/test-livekit)"),
        _ => {
            eprintln!("HORO_TEST_LIVEKIT not set; skipping");
            None
        }
    }
}

/// A room name no other test (or earlier run) uses.
fn room(name: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("{name}-{nanos:x}")
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

/// Wait for the first event `pick` accepts, failing after `within`.
async fn expect<T>(
    events: &mut UnboundedReceiver<SignalEvent>,
    what: &str,
    within: Duration,
    mut pick: impl FnMut(&SignalEvent) -> Option<T>,
) -> T {
    tokio::time::timeout(within, async {
        loop {
            let event = events.recv().await.unwrap_or_else(|| panic!("events ended waiting for {what}"));
            if let Some(found) = pick(&event) {
                return found;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

const WAIT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn joining_shows_who_is_already_there() {
    let Some(url) = server() else { return };
    let room = room("join");

    let (_alice, alice_join, mut alice_events) = connect(&url, &token("alice", &room, SECRET), true).await.unwrap();
    assert_eq!(alice_join.participant.as_ref().unwrap().identity, "alice");
    assert!(alice_join.other_participants.is_empty());
    assert_eq!(alice_join.room.as_ref().unwrap().name, room);

    let (_bob, bob_join, _) = connect(&url, &token("bob", &room, SECRET), true).await.unwrap();
    let others: Vec<_> = bob_join.other_participants.iter().map(|p| p.identity.as_str()).collect();
    assert_eq!(others, ["alice"]);

    // Alice hears about Bob arriving.
    expect(&mut alice_events, "bob in a participant update", WAIT, |e| match e {
        SignalEvent::Participants(ps) => ps.iter().any(|p| p.identity == "bob").then_some(()),
        _ => None,
    })
    .await;
}

#[tokio::test]
async fn the_server_offers_the_subscriber_connection() {
    let Some(url) = server() else { return };
    let room = room("offer");

    let (_client, join, mut events) = connect(&url, &token("carol", &room, SECRET), true).await.unwrap();
    assert!(join.subscriber_primary, "protocol 15 servers offer the subscriber first");

    let offer = expect(&mut events, "a subscriber offer", WAIT, |e| match e {
        SignalEvent::Offer(sdp) => Some(sdp.clone()),
        _ => None,
    })
    .await;
    assert!(offer.sdp.starts_with("v=0"), "{}", offer.sdp);
    assert!(offer.sdp.contains("m=application"), "the offer carries the data channels");
}

#[tokio::test]
async fn publishing_a_track_is_acknowledged() {
    let Some(url) = server() else { return };
    let room = room("publish");

    let (client, _, mut events) = connect(&url, &token("dave", &room, SECRET), true).await.unwrap();
    client
        .add_track(AddTrack {
            cid: "mic-1".into(),
            name: "microphone".into(),
            kind: proto::TrackType::Audio,
            source: proto::TrackSource::Microphone,
            muted: false,
            width: 0,
            height: 0,
            layers: Vec::new(),
        })
        .unwrap();

    let track = expect(&mut events, "TrackPublished", WAIT, |e| match e {
        SignalEvent::TrackPublished { cid, track } if cid == "mic-1" => track.clone(),
        _ => None,
    })
    .await;
    assert!(track.sid.starts_with("TR_"), "{}", track.sid);
    assert_eq!(track.r#type, proto::TrackType::Audio as i32);
}

#[tokio::test]
async fn a_bad_token_is_refused_with_the_reason() {
    let Some(url) = server() else { return };
    let room = room("refused");

    match connect(&url, &token("mallory", &room, "not-the-secret"), true).await {
        Err(Error::Refused { status, .. }) => assert_eq!(status, 401),
        Err(other) => panic!("expected a 401, got {other}"),
        Ok(_) => panic!("a forged token was accepted"),
    }
}

#[tokio::test]
async fn pings_keep_the_connection_open() {
    let Some(url) = server() else { return };
    let room = room("keepalive");

    let (_client, join, mut events) = connect(&url, &token("erin", &room, SECRET), true).await.unwrap();
    // Past the server's own ping timeout, after which it drops a client that
    // has gone quiet: still being here proves the pings are going out.
    assert!(join.ping_timeout > 0, "the server should state its ping timeout");
    let hold = Duration::from_secs((join.ping_timeout + join.ping_interval) as u64);
    let closed = tokio::time::timeout(hold, async {
        loop {
            match events.recv().await {
                Some(SignalEvent::Closed(reason)) => return reason,
                Some(_) => continue,
                None => return "events ended".into(),
            }
        }
    })
    .await;
    if let Ok(reason) = closed {
        panic!("connection closed within {hold:?}: {reason}");
    }
}

#[tokio::test]
async fn leaving_closes_the_connection_and_tells_the_room() {
    let Some(url) = server() else { return };
    let room = room("leave");

    let (_alice, _, mut alice_events) = connect(&url, &token("alice", &room, SECRET), true).await.unwrap();
    let (bob, _, mut bob_events) = connect(&url, &token("bob", &room, SECRET), true).await.unwrap();
    expect(&mut alice_events, "bob joining", WAIT, |e| match e {
        SignalEvent::Participants(ps) => ps.iter().any(|p| p.identity == "bob").then_some(()),
        _ => None,
    })
    .await;

    bob.leave().unwrap();
    let reason = expect(&mut bob_events, "bob's connection closing", WAIT, |e| match e {
        SignalEvent::Closed(reason) => Some(reason.clone()),
        _ => None,
    })
    .await;
    assert_eq!(reason, "left the room");

    expect(&mut alice_events, "bob leaving", WAIT, |e| match e {
        SignalEvent::Participants(ps) => ps
            .iter()
            .any(|p| p.identity == "bob" && p.state == proto::participant_info::State::Disconnected as i32)
            .then_some(()),
        _ => None,
    })
    .await;
}
