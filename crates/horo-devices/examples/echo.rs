//! How much of the loudspeaker's own sound the microphone still hears, with
//! Apple's voice processing off and on.
//!
//! ```sh
//! cargo run -p horo-devices --example echo
//! ```
//!
//! **Plays a chirp out loud** for about eight seconds. Use the built-in
//! speakers (headphones leave no echo to cancel) and keep the room quiet.
//!
//! Four phases on one VoiceProcessingIO unit: silence and the chirp, each
//! with processing bypassed and on (gain control lifts a quiet room, so the
//! floor is measured both ways). The gap between the two chirps is the echo
//! cancellation; how close the processed chirp gets to the processed room
//! is how much echo is left.
//!
//! First run, on the development Mac's default speakers: raw echo 16 dB
//! over the room, reduced by 28.5 dB, which leaves it below the room's own
//! noise.

#[cfg(target_os = "macos")]
fn main() {
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        time::Duration,
    };

    use horo_devices::{Duplex, FRAME, SAMPLE_RATE};

    /// A 300 Hz → 3 kHz sweep every half second: speech-band and not
    /// stationary, so noise suppression can't take credit for it.
    fn chirp(n: u64) -> f32 {
        let rate = SAMPLE_RATE as f64;
        let t = (n % (SAMPLE_RATE as u64 / 2)) as f64 / rate;
        let phase = std::f64::consts::TAU * (300.0 * t + 2700.0 * t * t);
        (phase.sin() * 0.5) as f32
    }

    let playing = Arc::new(AtomicBool::new(false));
    let energy = Arc::new(Mutex::new((0f64, 0u64)));
    let played = Arc::new(AtomicU64::new(0));
    let duplex = {
        let (playing, energy, played) = (playing.clone(), energy.clone(), played.clone());
        Duplex::start(
            move |frame: &[f32; FRAME]| {
                let mut e = energy.lock().unwrap();
                e.0 += frame.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>();
                e.1 += FRAME as u64;
            },
            move |frame: &mut [f32; FRAME]| {
                let on = playing.load(Ordering::Relaxed);
                for s in frame.iter_mut() {
                    let n = played.fetch_add(1, Ordering::Relaxed);
                    *s = if on { chirp(n) } else { 0.0 };
                }
            },
        )
        .expect("voice processing unit")
    };

    let measure = |label: &str, play: bool, bypass: bool| -> f64 {
        duplex.set_bypass(bypass).expect("bypass");
        playing.store(play, Ordering::Relaxed);
        // Let the canceller (re)adapt and the ring drain.
        std::thread::sleep(Duration::from_millis(1500));
        *energy.lock().unwrap() = (0.0, 0);
        std::thread::sleep(Duration::from_millis(2500));
        let (sum, n) = *energy.lock().unwrap();
        let db = 20.0 * (sum / n.max(1) as f64).sqrt().max(1e-9).log10();
        eprintln!("{label:<22} {db:>6.1} dBFS  ({n} samples)");
        db
    };

    eprintln!("playing a chirp out loud for ~8 s of the ~16");
    let quiet = measure("room, bypassed", false, true);
    let raw = measure("chirp, bypassed", true, true);
    let processed = measure("chirp, processed", true, false);
    let quiet_processed = measure("room, processed", false, false);
    drop(duplex);
    eprintln!(
        "echo reduced by {:.1} dB (raw echo {:.1} dB over the room; processed {:+.1} dB against the processed room)",
        raw - processed,
        raw - quiet,
        processed - quiet_processed
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("VoiceProcessingIO is Apple-only; on iOS run the phone spike's echo test");
}
