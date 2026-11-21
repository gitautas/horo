//! How much of the loudspeaker's own sound the microphone still hears, with
//! horo's software echo canceller (sonora's AEC3) off and on, on plain
//! cpal devices: the path every desktop OS can take.
//!
//! ```sh
//! cargo run -p horo-rtc --example echo_ab -- [delay-ms] [ns|agc|both|none]
//! ```
//!
//! **Plays a chirp out loud** for about eight seconds. Use the built-in
//! speakers (headphones leave no echo to cancel) and keep the room quiet.
//!
//! Four phases: silence and the chirp, each with processing bypassed and on
//! (noise suppression lowers a quiet room, so the floor is measured both
//! ways). The gap between the two chirps is the echo cancellation; how close
//! the processed chirp gets to the processed room is how much echo is left.
//!
//! On a MacBook Air's own speakers and microphone the processed chirp lands
//! at the room's noise floor (26.5 dB below the raw echo in that room).

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use horo_devices::{Capture, FRAME, Playback, SAMPLE_RATE};
use horo_rtc::audio::echo::{EchoCanceller, Processing};

/// A 300 Hz → 3 kHz sweep every half second: speech-band and not
/// stationary, so noise suppression can't take credit for it.
fn chirp(n: u64) -> f32 {
    let t = (n % (SAMPLE_RATE as u64 / 2)) as f64 / SAMPLE_RATE as f64;
    let phase = std::f64::consts::TAU * (300.0 * t + 2700.0 * t * t);
    (phase.sin() * 0.5) as f32
}

fn main() {
    let delay = Duration::from_millis(std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(100));
    let processing = match std::env::args().nth(2).as_deref() {
        Some("none") => Processing { noise_suppression: false, gain_control: false },
        Some("agc") => Processing { noise_suppression: false, gain_control: true },
        Some("both") => Processing { noise_suppression: true, gain_control: true },
        _ => Processing::default(),
    };
    let aec = EchoCanceller::new(processing, delay);

    let playing = Arc::new(AtomicBool::new(false));
    let played = Arc::new(AtomicU64::new(0));
    let energy = Arc::new(Mutex::new((0f64, 0u64)));

    let playback = {
        let (aec, playing, played) = (aec.clone(), playing.clone(), played.clone());
        Playback::start(None, move |frame: &mut [f32; FRAME]| {
            let on = playing.load(Ordering::Relaxed);
            for s in frame.iter_mut() {
                let n = played.fetch_add(1, Ordering::Relaxed);
                *s = if on { chirp(n) } else { 0.0 };
            }
            aec.render(frame);
        })
        .expect("loudspeaker")
    };
    let capture = {
        let (aec, energy) = (aec.clone(), energy.clone());
        Capture::start(None, move |frame: &[f32; FRAME]| {
            let mut frame = *frame;
            aec.capture(&mut frame);
            let mut e = energy.lock().unwrap();
            e.0 += frame.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>();
            e.1 += FRAME as u64;
        })
        .expect("microphone")
    };
    eprintln!(
        "microphone at {} Hz, loudspeaker at {} Hz; delay hint {delay:?}; {processing:?}",
        capture.device_rate, playback.device_rate
    );

    let measure = |label: &str, play: bool, enabled: bool| -> f64 {
        aec.set_enabled(enabled);
        playing.store(play, Ordering::Relaxed);
        // Let the canceller (re)adapt and the queues drain.
        std::thread::sleep(Duration::from_millis(1500));
        *energy.lock().unwrap() = (0.0, 0);
        std::thread::sleep(Duration::from_millis(2500));
        let (sum, n) = *energy.lock().unwrap();
        let db = 20.0 * (sum / n.max(1) as f64).sqrt().max(1e-9).log10();
        let stats = aec.stats();
        eprintln!(
            "{label:<22} {db:>6.1} dBFS  (erle {}, delay {})",
            stats.erle_db.map_or("-".into(), |e| format!("{e:.1} dB")),
            stats.delay.map_or("-".into(), |d| format!("{d:?}"))
        );
        db
    };

    eprintln!("playing a chirp out loud for ~8 s of the ~16");
    let quiet = measure("room, bypassed", false, false);
    let raw = measure("chirp, bypassed", true, false);
    let processed = measure("chirp, processed", true, true);
    let quiet_processed = measure("room, processed", false, true);
    drop((capture, playback));
    eprintln!(
        "echo reduced by {:.1} dB (raw echo {:.1} dB over the room; processed {:+.1} dB against the processed room)",
        raw - processed,
        raw - quiet,
        processed - quiet_processed
    );
}
