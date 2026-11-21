//! Echo cancellation, noise suppression and gain control in software, the
//! same on every platform: WebRTC's audio processing (AEC3, NS, AGC2) as
//! ported to Rust by `sonora`.
//!
//! The canceller has to see what the loudspeaker plays to remove it from
//! what the microphone hears. [`EchoCanceller::render`] takes each frame on
//! its way to the loudspeaker; [`EchoCanceller::capture`] cleans each frame
//! from the microphone. They're usually called from two threads (playback
//! and capture), so one canceller is shared between them, behind a lock
//! held for one frame's processing.
//!
//! AEC3 finds the echo's delay itself, but starts from a hint: how long a
//! frame takes from `render` to the loudspeaker plus from the microphone to
//! `capture` ([`EchoCanceller::set_delay`]).

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use sonora::{
    AudioProcessing, Config, StreamConfig,
    config::{EchoCanceller as Aec, GainController2, HighPassFilter, NoiseSuppression},
};

use super::{FRAME, SAMPLE_RATE};

/// AEC3 works on 10 ms: two per frame.
const CHUNK: usize = FRAME / 2;

/// What runs on the microphone besides echo cancellation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Processing {
    pub noise_suppression: bool,
    /// Levels the voice: quiet microphones louder, loud ones capped.
    pub gain_control: bool,
}

impl Default for Processing {
    fn default() -> Self {
        Processing { noise_suppression: true, gain_control: false }
    }
}

/// How well the canceller is doing, for a call's stats screen.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EchoStats {
    /// Echo return loss enhancement: how much echo the canceller removes, dB.
    pub erle_db: Option<f64>,
    /// The echo path's delay as AEC3 found it.
    pub delay: Option<Duration>,
}

struct Inner {
    apm: AudioProcessing,
    /// Off: capture passes through untouched, render still feeds the model so
    /// turning it back on needs no re-convergence.
    enabled: bool,
    out: [f32; CHUNK],
}

/// One call's canceller. Cheap to clone; clones share it.
#[derive(Clone)]
pub struct EchoCanceller {
    inner: Arc<Mutex<Inner>>,
}

impl EchoCanceller {
    /// `delay`: the starting guess for render-to-capture time (see the
    /// module docs).
    pub fn new(processing: Processing, delay: Duration) -> EchoCanceller {
        let config = Config {
            echo_canceller: Some(Aec::default()),
            high_pass_filter: Some(HighPassFilter::default()),
            noise_suppression: processing.noise_suppression.then(NoiseSuppression::default),
            gain_controller2: processing.gain_control.then(GainController2::default),
            ..Default::default()
        };
        let stream = StreamConfig::new(SAMPLE_RATE, 1);
        let mut apm = AudioProcessing::builder().config(config).capture_config(stream).render_config(stream).build();
        let _ = apm.set_stream_delay_ms(delay.as_millis() as i32);
        EchoCanceller { inner: Arc::new(Mutex::new(Inner { apm, enabled: true, out: [0.0; CHUNK] })) }
    }

    /// A frame on its way to the loudspeaker.
    pub fn render(&self, frame: &[f32; FRAME]) {
        let mut inner = self.inner.lock().unwrap();
        let Inner { apm, out, .. } = &mut *inner;
        for chunk in frame.as_chunks::<CHUNK>().0 {
            if let Err(e) = apm.process_render_f32(&[chunk], &mut [&mut out[..]]) {
                tracing::warn!("echo canceller, render: {e:?}");
            }
        }
    }

    /// A frame from the microphone, cleaned in place.
    pub fn capture(&self, frame: &mut [f32; FRAME]) {
        let mut inner = self.inner.lock().unwrap();
        let Inner { apm, out, enabled } = &mut *inner;
        for chunk in frame.as_chunks_mut::<CHUNK>().0 {
            // The stream delay is per capture call in the WebRTC API.
            let delay = apm.stream_delay_ms();
            let _ = apm.set_stream_delay_ms(delay);
            match apm.process_capture_f32(&[chunk], &mut [&mut out[..]]) {
                Ok(()) if *enabled => chunk.copy_from_slice(&out[..]),
                Ok(()) => {}
                Err(e) => tracing::warn!("echo canceller, capture: {e:?}"),
            }
        }
    }

    /// The render-to-capture delay hint.
    pub fn set_delay(&self, delay: Duration) {
        let _ = self.inner.lock().unwrap().apm.set_stream_delay_ms(delay.as_millis() as i32);
    }

    /// Bypass the processing (for an A/B), keeping the model adapting.
    pub fn set_enabled(&self, enabled: bool) {
        self.inner.lock().unwrap().enabled = enabled;
    }

    pub fn stats(&self) -> EchoStats {
        let inner = self.inner.lock().unwrap();
        let stats = inner.apm.statistics();
        EchoStats {
            erle_db: stats.echo_return_loss_enhancement,
            delay: stats.delay_ms.and_then(|ms| u64::try_from(ms).ok()).map(Duration::from_millis),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A room: the loudspeaker's sound back at the microphone `delay` later,
    /// quieter and smeared by a couple of reflections.
    struct Room {
        history: Vec<f32>,
        delay: usize,
    }

    impl Room {
        fn new(delay: Duration) -> Room {
            let delay = (delay.as_secs_f64() * SAMPLE_RATE as f64) as usize;
            Room { history: vec![0.0; delay + 2000], delay }
        }

        fn echo(&mut self, played: &[f32; FRAME]) -> [f32; FRAME] {
            self.history.extend_from_slice(played);
            let end = self.history.len();
            let mut echo = [0.0; FRAME];
            for (i, e) in echo.iter_mut().enumerate() {
                let at = end - FRAME + i - self.delay;
                *e = 0.4 * self.history[at] + 0.2 * self.history[at - 300] - 0.1 * self.history[at - 1100];
            }
            self.history.drain(..FRAME);
            echo
        }
    }

    /// Speech-band noise: white noise through a gentle band-pass, so the
    /// canceller has something broadband and unpredictable to model.
    struct Talker {
        rng: u64,
        low: f32,
        high: f32,
    }

    impl Talker {
        fn new(seed: u64) -> Talker {
            Talker { rng: seed, low: 0.0, high: 0.0 }
        }

        fn frame(&mut self, level: f32) -> [f32; FRAME] {
            let mut frame = [0.0; FRAME];
            for s in &mut frame {
                self.rng ^= self.rng << 13;
                self.rng ^= self.rng >> 7;
                self.rng ^= self.rng << 17;
                let white = (self.rng >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0;
                self.low += 0.3 * (white - self.low);
                self.high += 0.02 * (self.low - self.high);
                *s = (self.low - self.high) * level;
            }
            frame
        }
    }

    fn energy(frames: &[[f32; FRAME]]) -> f64 {
        frames.iter().flatten().map(|&s| (s as f64).powi(2)).sum::<f64>() / (frames.len() * FRAME) as f64
    }

    fn db(ratio: f64) -> f64 {
        10.0 * ratio.max(1e-20).log10()
    }

    /// Far end talks, near end is silent: what the microphone sends back
    /// should be mostly gone once AEC3 has converged.
    #[test]
    fn the_far_ends_voice_is_removed_from_the_microphone() {
        let delay = Duration::from_millis(80);
        let aec = EchoCanceller::new(Processing { noise_suppression: false, gain_control: false }, delay);
        let mut room = Room::new(delay);
        let mut far = Talker::new(0x9E37_79B9_7F4A_7C15);
        let (mut before, mut after) = (Vec::new(), Vec::new());
        // Six seconds: converge in the first four, measure the last two.
        for n in 0..300 {
            let played = far.frame(0.5);
            aec.render(&played);
            let mut mic = room.echo(&played);
            let raw = mic;
            aec.capture(&mut mic);
            if n >= 200 {
                before.push(raw);
                after.push(mic);
            }
        }
        let removed = db(energy(&before) / energy(&after));
        eprintln!("echo removed: {removed:.1} dB; {:?}", aec.stats());
        assert!(removed > 25.0, "only {removed:.1} dB of echo removed");
    }

    /// Near end talks, far end is silent: the voice goes through.
    #[test]
    fn the_near_ends_voice_goes_through() {
        let aec = EchoCanceller::new(Processing { noise_suppression: false, gain_control: false }, Duration::from_millis(80));
        let mut near = Talker::new(0x2545_F491_4F6C_DD1D);
        let (mut before, mut after) = (Vec::new(), Vec::new());
        for n in 0..150 {
            aec.render(&[0.0; FRAME]);
            let mut mic = near.frame(0.3);
            let raw = mic;
            aec.capture(&mut mic);
            if n >= 50 {
                before.push(raw);
                after.push(mic);
            }
        }
        let change = db(energy(&after) / energy(&before));
        eprintln!("near end changed by {change:.1} dB");
        assert!(change.abs() < 3.0, "near-end voice changed by {change:.1} dB");
    }

    /// Bypassed, the microphone is untouched.
    #[test]
    fn bypass_passes_the_microphone_through() {
        let aec = EchoCanceller::new(Processing::default(), Duration::from_millis(50));
        aec.set_enabled(false);
        let mut near = Talker::new(7);
        for _ in 0..20 {
            let frame = near.frame(0.3);
            aec.render(&frame);
            let mut mic = frame;
            aec.capture(&mut mic);
            assert_eq!(mic, frame);
        }
    }
}
