//! Two call engines in one LiveKit room: what goes into Alice's
//! [`Microphone`] has to come out of Bob's [`Speaker`], through the gate,
//! Opus, the SFU, the jitter buffer and the mixer.
//!
//! ```sh
//! scripts/test-livekit/start.sh
//! HORO_TEST_LIVEKIT=ws://127.0.0.1:7880 cargo test -p horo-call --test engine
//! ```

use std::{
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use horo_call::{Call, CallEvent, CallOptions, Joined, Network};
use horo_media::{
    PeerConfig,
    audio::{FRAME, SAMPLE_RATE},
};
use tokio::sync::mpsc;

const KEY: &str = "devkey";
const DEV_SECRET: &str = "secret";
const TONE_HZ: f64 = 440.0;

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

fn token(identity: &str, room: &str) -> String {
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
    jsonwebtoken::encode(&Default::default(), &claims, &jsonwebtoken::EncodingKey::from_secret(DEV_SECRET.as_bytes())).unwrap()
}

fn loopback() -> Network {
    Arc::new(|name, _| PeerConfig { host_addresses: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)], ice_tcp: false, ..PeerConfig::default_for(name) })
}

fn options() -> CallOptions {
    CallOptions { network: loopback(), ..Default::default() }
}

/// Goertzel power at one frequency.
fn power_at(samples: &[f32], hz: f64) -> f64 {
    let w = std::f64::consts::TAU * hz / SAMPLE_RATE as f64;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for &x in samples {
        let s0 = x as f64 + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

/// How much the tone stands out from its neighbours.
fn purity(samples: &[f32]) -> f64 {
    let tone = power_at(samples, TONE_HZ);
    let others = [0.8, 0.9, 1.1, 1.25].map(|k| power_at(samples, TONE_HZ * k));
    tone / others.into_iter().fold(f64::MIN_POSITIVE, f64::max)
}

fn rms(samples: &[f32]) -> f64 {
    (samples.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / samples.len().max(1) as f64).sqrt()
}

async fn wait_for<T>(events: &mut mpsc::UnboundedReceiver<CallEvent>, what: &str, mut check: impl FnMut(&CallEvent) -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
            .unwrap_or_else(|| panic!("events ended waiting for {what}"));
        if let CallEvent::Ended(reason) = &event {
            panic!("call ended waiting for {what}: {reason}");
        }
        if let Some(found) = check(&event) {
            return found;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn what_alice_says_bob_hears_until_she_mutes() {
    let Some(url) = server() else { return };
    let room = format!("engine-{:x}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());

    let Joined { call: bob, microphone: _, speaker: mut bob_speaker, events: mut bob_events } =
        Call::connect(&url, &token("bob", &room), options()).await.expect("bob joins");
    let Joined { call: alice, microphone: mut alice_mic, speaker: _, events: mut alice_events } =
        Call::connect(&url, &token("alice", &room), options()).await.expect("alice joins");
    assert_eq!((alice.identity.as_str(), bob.identity.as_str()), ("alice", "bob"));

    wait_for(&mut bob_events, "bob to see alice", |e| match e {
        CallEvent::Participants(p) if p.iter().any(|p| p.identity == "alice") => Some(()),
        _ => None,
    })
    .await;
    wait_for(&mut alice_events, "alice's media", |e| (*e == CallEvent::Connected).then_some(())).await;

    // A real-time 20 ms clock: Alice's tone in, Bob's mix out.
    let started = Instant::now();
    let mut clock = tokio::time::interval(Duration::from_millis(20));
    let mut sent = 0u64;
    let mut heard: Vec<f32> = Vec::new();
    let mut first_sound = None;
    let mut frame = [0f32; FRAME];
    let mut muted_at = None;
    let mut after_mute: Vec<f32> = Vec::new();
    while started.elapsed() < Duration::from_secs(12) {
        clock.tick().await;
        for (i, s) in frame.iter_mut().enumerate() {
            let t = ((sent + i as u64) % SAMPLE_RATE as u64) as f64 / SAMPLE_RATE as f64;
            *s = ((t * TONE_HZ * std::f64::consts::TAU).sin() * 0.25) as f32;
        }
        sent += FRAME as u64;
        alice_mic.push(&frame);

        bob_speaker.pull(&mut frame);
        match muted_at {
            None => {
                if first_sound.is_none() && rms(&frame) > 0.05 {
                    first_sound = Some(started.elapsed());
                }
                if first_sound.is_some() {
                    heard.extend_from_slice(&frame);
                }
                // Two seconds of sound, then mute.
                if heard.len() >= 2 * SAMPLE_RATE as usize {
                    alice.set_muted(true);
                    muted_at = Some(Instant::now());
                }
            }
            // Past the jitter buffer's tail and the idle cut-off.
            Some(at) if at.elapsed() > Duration::from_millis(1500) => {
                after_mute.extend_from_slice(&frame);
                if after_mute.len() >= SAMPLE_RATE as usize {
                    break;
                }
            }
            Some(_) => {}
        }
        // Drain what the UI would show; the level has to move.
        while let Ok(event) = alice_events.try_recv() {
            if let CallEvent::Ended(reason) = event {
                panic!("alice's call ended: {reason}");
            }
        }
    }

    let first_sound = first_sound.expect("bob never heard anything");
    let tail = &heard[heard.len() / 2..];
    let (purity, level) = (purity(tail), rms(tail));
    eprintln!("first sound after {first_sound:?}; tone purity {purity:.0}, rms {level:.3}; after mute rms {:.4}", rms(&after_mute));
    assert!(purity > 100.0, "bob heard something other than the tone (purity {purity:.1})");
    // A sine of amplitude 0.25 has an RMS of ~0.177; Opus keeps it close.
    assert!((0.12..0.25).contains(&level), "tone at the wrong level: {level:.3}");
    assert!(first_sound < Duration::from_secs(4), "first sound after {first_sound:?}");
    assert!(after_mute.len() >= SAMPLE_RATE as usize, "the run ended before a second after muting");
    assert!(rms(&after_mute) < 0.001, "still hear alice after she muted: {:.4}", rms(&after_mute));

    alice.leave().await;
    wait_for(&mut bob_events, "bob to see alice go", |e| match e {
        CallEvent::Participants(p) if p.is_empty() => Some(()),
        _ => None,
    })
    .await;
    bob.leave().await;
}
