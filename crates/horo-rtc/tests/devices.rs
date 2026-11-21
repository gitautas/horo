//! The default devices of the machine running the tests, if it has any.
//!
//! These check the plumbing (devices open, frames come at the right pace,
//! the real-time side doesn't glitch), not what the microphone hears: that
//! needs a person. Machines without audio devices, like CI runners, skip.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use horo_rtc::devices::{Capture, FRAME, Playback, inputs, outputs};

const RUN: Duration = Duration::from_secs(2);

#[test]
fn devices_are_listed_with_one_default() {
    for (kind, list) in [("input", inputs()), ("output", outputs())] {
        let Ok(list) = list else { continue };
        eprintln!("{kind}s: {:?}", list.iter().map(|d| (&d.name, d.is_default)).collect::<Vec<_>>());
        if !list.is_empty() {
            assert!(list.iter().filter(|d| d.is_default).count() <= 1, "{kind}: {list:?}");
            assert!(list.iter().all(|d| !d.id.is_empty() && !d.name.is_empty()));
        }
    }
}

#[test]
fn the_speaker_asks_for_fifty_frames_a_second() {
    let asked = Arc::new(AtomicUsize::new(0));
    let playback = {
        let asked = asked.clone();
        // Silence: nothing audible while the tests run.
        match Playback::start(None, move |frame: &mut [f32; FRAME]| {
            frame.fill(0.0);
            asked.fetch_add(1, Ordering::Relaxed);
        }) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("no loudspeaker here ({e}); skipping");
                return;
            }
        }
    };
    let primed = asked.load(Ordering::Relaxed);
    std::thread::sleep(RUN);
    let asked = asked.load(Ordering::Relaxed) - primed;
    let glitches = playback.stats.glitches.load(Ordering::Relaxed);
    drop(playback);
    eprintln!("speaker: {asked} frames in {RUN:?}, {glitches} glitched samples");
    // 100 frames in two seconds, give or take scheduling.
    assert!((90..=110).contains(&asked), "{asked} frames");
    // The device can start a moment before the first fill lands.
    assert!(glitches < 4800, "{glitches} samples of silence padding");
}

#[test]
fn the_microphone_delivers_fifty_frames_a_second() {
    let got = Arc::new(AtomicUsize::new(0));
    let capture = {
        let got = got.clone();
        match Capture::start(None, move |_frame: &[f32; FRAME]| {
            got.fetch_add(1, Ordering::Relaxed);
        }) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("no microphone here ({e}); skipping");
                return;
            }
        }
    };
    std::thread::sleep(RUN);
    let got = got.load(Ordering::Relaxed);
    let glitches = capture.stats.glitches.load(Ordering::Relaxed);
    let rate = capture.device_rate;
    drop(capture);
    eprintln!("microphone at {rate} Hz: {got} frames in {RUN:?}, {glitches} dropped samples");
    assert!((85..=105).contains(&got), "{got} frames");
    assert_eq!(glitches, 0);
}
