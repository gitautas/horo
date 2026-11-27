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

/// Bluetooth headphones change sample rate whenever any app opens or closes
/// their microphone, and cpal then pauses every stream on them for good. A
/// capture has to notice and open the device again, or the call goes on
/// without our voice.
///
/// Changes a real microphone's sample rate, so it's run by hand:
/// `HORO_TEST_INPUT="MacBook Air Microphone" cargo test -p horo-rtc --test devices -- --ignored`
/// (any input whose name contains it; the first that offers two rates if unset).
#[test]
#[ignore = "changes a microphone's sample rate"]
fn the_microphone_comes_back_after_its_sample_rate_changes() {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let wanted = std::env::var("HORO_TEST_INPUT").ok();
    let host = cpal::default_host();
    let Some((device, other_rate)) = host.input_devices().unwrap().find_map(|d| {
        let name = d.description().map(|n| n.to_string()).unwrap_or_default();
        if wanted.as_deref().is_some_and(|w| !name.contains(w)) {
            return None;
        }
        let now = d.default_input_config().ok()?.sample_rate();
        let other = [48_000, 44_100, 24_000, 16_000]
            .into_iter()
            .find(|&r| r != now && d.supported_input_configs().is_ok_and(|mut c| c.any(|c| (c.min_sample_rate()..=c.max_sample_rate()).contains(&r))))?;
        Some((d, other))
    }) else {
        eprintln!("no microphone here offers a second rate; skipping");
        return;
    };
    let id = device.id().unwrap().to_string();

    let got = Arc::new(AtomicUsize::new(0));
    let capture = {
        let got = got.clone();
        Capture::start(Some(&id), move |_frame: &[f32; FRAME]| {
            got.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap()
    };
    std::thread::sleep(Duration::from_secs(1));

    // Another app (here, us) opens it at another rate.
    let mut config = device.default_input_config().unwrap().config();
    config.sample_rate = other_rate;
    let other = device.build_input_stream(config, |_: &[f32], _| {}, |_| {}, None).unwrap();
    other.play().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    drop(other);

    let before = got.load(Ordering::Relaxed);
    std::thread::sleep(RUN);
    let after = got.load(Ordering::Relaxed) - before;
    drop(capture);
    eprintln!("after switching to {other_rate} Hz: {after} frames in {RUN:?}");
    assert!(after >= 80, "the microphone stayed silent: {after} frames");
}
