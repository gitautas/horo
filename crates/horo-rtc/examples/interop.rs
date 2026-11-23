//! horo's call engine against livekit-client: the horo half of the interop
//! check (scripts/interop has the browser half and run.sh to drive both).
//!
//! ```sh
//! cargo run -p horo-rtc --example interop -- <livekit-url> <token> [send-hz] [listen-hz] [seconds]
//! ```
//!
//! Publishes a sine of `send-hz` (default 440) and listens for `listen-hz`
//! (default 660) in what the room plays us. Once a second: how loud the mix
//! is, how pure the expected tone is in it, and each remote track's jitter
//! buffer. No devices: the frames go straight in and out on a 20 ms clock.

use std::time::{Duration, Instant};

use horo_rtc::call::{Call, CallEvent, CallOptions, Joined};
use horo_rtc::audio::{FRAME, SAMPLE_RATE};

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

/// How much `hz` stands out from its neighbours, in dB.
fn purity_db(samples: &[f32], hz: f64) -> f64 {
    let tone = power_at(samples, hz);
    let others = [0.8, 0.9, 1.1, 1.25].map(|k| power_at(samples, hz * k));
    10.0 * (tone / others.into_iter().fold(f64::MIN_POSITIVE, f64::max)).log10()
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into())).init();
    let args: Vec<String> = std::env::args().collect();
    let (url, token) = (&args[1], &args[2]);
    let send_hz: f64 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(440.0);
    let listen_hz: f64 = args.get(4).and_then(|a| a.parse().ok()).unwrap_or(660.0);
    let seconds: f64 = args.get(5).and_then(|a| a.parse().ok()).unwrap_or(30.0);

    let Joined { call, mut microphone, mut speaker, mut events, .. } =
        Call::connect(url, token, CallOptions::default()).await.expect("join");
    eprintln!("in the room as {}, sending {send_hz} Hz, listening for {listen_hz} Hz", call.identity);

    let started = Instant::now();
    let mut clock = tokio::time::interval(Duration::from_millis(20));
    let mut sent = 0u64;
    let mut frame = [0f32; FRAME];
    let mut second: Vec<f32> = Vec::with_capacity(SAMPLE_RATE as usize);
    while started.elapsed().as_secs_f64() < seconds {
        tokio::select! {
            _ = clock.tick() => {
                for (i, s) in frame.iter_mut().enumerate() {
                    let t = ((sent + i as u64) % SAMPLE_RATE as u64) as f64 / SAMPLE_RATE as f64;
                    *s = ((t * send_hz * std::f64::consts::TAU).sin() * 0.25) as f32;
                }
                sent += FRAME as u64;
                microphone.push(&frame);
                speaker.pull(&mut frame);
                second.extend_from_slice(&frame);
                if second.len() >= SAMPLE_RATE as usize {
                    let rms = (second.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / second.len() as f64).sqrt();
                    let tracks: Vec<String> = speaker
                        .stats()
                        .iter()
                        .map(|(target, s)| format!(
                            "target {}ms recv {} dec {} dtx {} fec {} plc {} stretch {} late {} drop {} rebuf {}",
                            target.as_millis(), s.received, s.decoded, s.silent, s.recovered, s.concealed, s.stretched, s.late, s.dropped, s.rebuffers
                        ))
                        .collect();
                    eprintln!(
                        "{:>4.1}s heard {:>6.1} dBFS, {listen_hz} Hz purity {:>5.1} dB | {}",
                        started.elapsed().as_secs_f64(),
                        20.0 * rms.max(1e-9).log10(),
                        purity_db(&second, listen_hz),
                        if tracks.is_empty() { "no remote audio".into() } else { tracks.join(" ; ") }
                    );
                    second.clear();
                }
            }
            Some(event) = events.recv() => match event {
                CallEvent::Level(_) => {}
                CallEvent::Ended(reason) => { eprintln!("ended: {reason}"); return; }
                other => eprintln!("{other:?}"),
            },
        }
    }
    call.leave().await;
}
