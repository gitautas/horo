//! Between a device's sample rate and 48 kHz frames. Pure: no devices, so
//! it can be tested exactly.

use rubato::{Fft, FixedSync, Resampler, audioadapter_buffers::direct::InterleavedSlice};

use super::{FRAME, SAMPLE_RATE};

/// Device-rate mono samples in, 20 ms frames at 48 kHz out.
pub struct ToFrames {
    resampler: Option<Fft<f32>>,
    pending: Vec<f32>,
    frame: [f32; FRAME],
}

impl ToFrames {
    pub fn new(device_rate: u32) -> Result<ToFrames, String> {
        let resampler = (device_rate != SAMPLE_RATE)
            .then(|| Fft::<f32>::new(device_rate as usize, SAMPLE_RATE as usize, FRAME, 1, FixedSync::Output))
            .transpose()
            .map_err(|e| e.to_string())?;
        Ok(ToFrames { resampler, pending: Vec::new(), frame: [0.0; FRAME] })
    }

    /// Add samples; `on_frame` runs once per complete frame they finish.
    pub fn push(&mut self, samples: &[f32], mut on_frame: impl FnMut(&[f32; FRAME])) {
        self.pending.extend_from_slice(samples);
        loop {
            let needed = self.resampler.as_ref().map_or(FRAME, |r| r.input_frames_next());
            if self.pending.len() < needed {
                return;
            }
            match &mut self.resampler {
                None => self.frame.copy_from_slice(&self.pending[..FRAME]),
                Some(resampler) => {
                    let input = InterleavedSlice::new(&self.pending[..needed], 1, needed).expect("sized to fit");
                    let mut output = InterleavedSlice::new_mut(&mut self.frame[..], 1, FRAME).expect("sized to fit");
                    if let Err(e) = resampler.process_into_buffer(&input, &mut output, None) {
                        tracing::warn!("resampling the microphone: {e}");
                    }
                }
            }
            self.pending.drain(..needed);
            on_frame(&self.frame);
        }
    }
}

/// 20 ms frames at 48 kHz in, device-rate mono samples out.
pub struct FromFrames {
    resampler: Option<Fft<f32>>,
    out: Vec<f32>,
}

impl FromFrames {
    pub fn new(device_rate: u32) -> Result<FromFrames, String> {
        let resampler = (device_rate != SAMPLE_RATE)
            .then(|| Fft::<f32>::new(SAMPLE_RATE as usize, device_rate as usize, FRAME, 1, FixedSync::Input))
            .transpose()
            .map_err(|e| e.to_string())?;
        let capacity = resampler.as_ref().map_or(FRAME, |r| r.output_frames_max());
        Ok(FromFrames { resampler, out: vec![0.0; capacity] })
    }

    /// Convert one frame; `on_samples` gets the device-rate result (its
    /// length varies a little from frame to frame at fractional ratios).
    pub fn push(&mut self, frame: &[f32; FRAME], mut on_samples: impl FnMut(&[f32])) {
        match &mut self.resampler {
            None => on_samples(frame),
            Some(resampler) => {
                let produced = resampler.output_frames_next();
                let input = InterleavedSlice::new(&frame[..], 1, FRAME).expect("one frame");
                let mut output = InterleavedSlice::new_mut(&mut self.out[..produced], 1, produced).expect("sized to fit");
                match resampler.process_into_buffer(&input, &mut output, None) {
                    Ok((_, written)) => on_samples(&self.out[..written]),
                    Err(e) => tracing::warn!("resampling for the loudspeaker: {e}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f64, seconds: f64) -> Vec<f32> {
        (0..(rate as f64 * seconds) as usize).map(|n| ((n as f64 * hz * std::f64::consts::TAU / rate as f64).sin() * 0.5) as f32).collect()
    }

    /// Goertzel power at one frequency.
    fn power_at(samples: &[f32], rate: u32, hz: f64) -> f64 {
        let w = std::f64::consts::TAU * hz / rate as f64;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0, 0.0);
        for &x in samples {
            let s0 = x as f64 + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        s1 * s1 + s2 * s2 - coeff * s1 * s2
    }

    fn purity(samples: &[f32], rate: u32, hz: f64) -> f64 {
        let tone = power_at(samples, rate, hz);
        let others = [hz * 0.8, hz * 0.9, hz * 1.1, hz * 1.25].map(|f| power_at(samples, rate, f));
        tone / others.into_iter().fold(f64::MIN_POSITIVE, f64::max)
    }

    #[test]
    fn a_44_1_khz_microphone_becomes_48_khz_frames_at_the_same_pitch() {
        let input = sine(44_100, 440.0, 2.0);
        let mut frames = ToFrames::new(44_100).unwrap();
        let mut out = Vec::new();
        // Device callbacks come in odd sizes.
        for chunk in input.chunks(441) {
            frames.push(chunk, |f| out.extend_from_slice(f));
        }
        // Two seconds in, about two seconds out (less the resampler's delay).
        assert!((90..=100).contains(&(out.len() / FRAME)), "{} frames", out.len() / FRAME);
        let middle = &out[out.len() / 4..out.len() * 3 / 4];
        assert!(purity(middle, SAMPLE_RATE, 440.0) > 1000.0, "pitch moved");
    }

    #[test]
    fn a_48_khz_microphone_is_only_cut_into_frames() {
        let input = sine(48_000, 440.0, 1.0);
        let mut frames = ToFrames::new(48_000).unwrap();
        let mut out = Vec::new();
        for chunk in input.chunks(512) {
            frames.push(chunk, |f| out.extend_from_slice(f));
        }
        assert_eq!(out.len(), 50 * FRAME);
        assert_eq!(&out[..], &input[..], "untouched");
    }

    #[test]
    fn frames_play_on_a_44_1_khz_speaker_at_the_same_pitch() {
        let input = sine(48_000, 440.0, 2.0);
        let mut frames = FromFrames::new(44_100).unwrap();
        let mut out = Vec::new();
        for frame in input.as_chunks::<FRAME>().0 {
            frames.push(frame, |s| out.extend_from_slice(s));
        }
        let expected = 2 * 44_100;
        assert!(out.len().abs_diff(expected) < 2 * FRAME, "{} samples for {expected}", out.len());
        let middle = &out[out.len() / 4..out.len() * 3 / 4];
        assert!(purity(middle, 44_100, 440.0) > 1000.0, "pitch moved");
    }
}
