//! The noise gate and level meter, ported from the WebView build's
//! `noiseGate.worklet.js` so a threshold means the same thing in both.
//!
//! One level per block (the block's RMS in dBFS). At or above the threshold
//! the gate opens at once and holds for 250 ms after the level drops, so the
//! gaps between words don't chop a sentence; then it fades shut over ~80 ms
//! so closing doesn't click. Opening takes ~3 ms so the first syllable gets
//! through.

use std::time::Duration;

const HOLD: Duration = Duration::from_millis(250);
const ATTACK: Duration = Duration::from_millis(3);
const RELEASE: Duration = Duration::from_millis(80);
/// How often a reading comes out: a meter that looks live.
const REPORT: Duration = Duration::from_micros(33_333);
/// Below this, report silence rather than an ever-larger negative number.
pub const FLOOR_DB: f32 = -100.0;
/// At or below this threshold the gate never closes: the slider's bottom.
pub const GATE_OFF_DB: f32 = -80.0;

/// The gate's state as the meter in settings draws it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateReading {
    /// Peak block level since the last reading, in dBFS.
    pub level: f32,
    /// Whether the gate is letting sound through.
    pub open: bool,
}

pub struct NoiseGate {
    sample_rate: f32,
    /// dBFS; `None` for a gate that never closes.
    threshold: Option<f32>,
    open: bool,
    gain: f32,
    hold_left: f32,
    peak: f32,
    since_report: f32,
    attack: f32,
    release: f32,
}

impl NoiseGate {
    pub fn new(sample_rate: u32) -> NoiseGate {
        let rate = sample_rate as f32;
        NoiseGate {
            sample_rate: rate,
            threshold: None,
            open: true,
            gain: 1.0,
            hold_left: 0.0,
            peak: FLOOR_DB,
            since_report: 0.0,
            attack: 1.0 - (-1.0 / (ATTACK.as_secs_f32() * rate)).exp(),
            release: 1.0 - (-1.0 / (RELEASE.as_secs_f32() * rate)).exp(),
        }
    }

    /// The settings slider's value. At [`GATE_OFF_DB`] or below, nothing is
    /// ever cut.
    pub fn set_threshold(&mut self, db: f32) {
        self.threshold = (db > GATE_OFF_DB).then_some(db);
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Gate one block of mono samples in place. Returns a reading when one is
    /// due (about 30 times a second).
    pub fn process(&mut self, samples: &mut [f32]) -> Option<GateReading> {
        if samples.is_empty() {
            return None;
        }
        let sum: f32 = samples.iter().map(|s| s * s).sum();
        let rms = (sum / samples.len() as f32).sqrt();
        let db = if rms > 0.0 { (20.0 * rms.log10()).max(FLOOR_DB) } else { FLOOR_DB };

        match self.threshold {
            Some(threshold) if db < threshold => {
                if self.hold_left > 0.0 {
                    self.hold_left -= samples.len() as f32;
                } else {
                    self.open = false;
                }
            }
            _ => {
                self.open = true;
                self.hold_left = HOLD.as_secs_f32() * self.sample_rate;
            }
        }

        let (target, rate) = if self.open { (1.0, self.attack) } else { (0.0, self.release) };
        for sample in samples.iter_mut() {
            self.gain += (target - self.gain) * rate;
            *sample *= self.gain;
        }

        self.peak = self.peak.max(db);
        self.since_report += samples.len() as f32;
        if self.since_report >= REPORT.as_secs_f32() * self.sample_rate {
            let reading = GateReading { level: self.peak, open: self.open };
            self.peak = FLOOR_DB;
            self.since_report = 0.0;
            return Some(reading);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// 128-sample blocks, like a Web Audio render quantum, of a constant level.
    fn blocks(gate: &mut NoiseGate, db: f32, millis: u32) -> (f32, Vec<GateReading>) {
        let amplitude = 10f32.powf(db / 20.0) * std::f32::consts::SQRT_2; // sine RMS = A/√2
        let mut readings = Vec::new();
        let mut last_gain = 0.0;
        let total = (RATE * millis / 1000) as usize;
        let mut n = 0usize;
        while n < total {
            let mut block: Vec<f32> =
                (0..128).map(|i| amplitude * ((n + i) as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin()).collect();
            let peak_in = block.iter().fold(0f32, |m, s| m.max(s.abs()));
            readings.extend(gate.process(&mut block));
            let peak_out = block.iter().fold(0f32, |m, s| m.max(s.abs()));
            last_gain = if peak_in > 0.0 { peak_out / peak_in } else { 0.0 };
            n += 128;
        }
        (last_gain, readings)
    }

    #[test]
    fn off_lets_everything_through() {
        let mut gate = NoiseGate::new(RATE);
        gate.set_threshold(GATE_OFF_DB);
        let (gain, _) = blocks(&mut gate, -70.0, 500);
        assert!(gain > 0.99, "{gain}");
    }

    #[test]
    fn quiet_noise_is_cut_after_the_hold() {
        let mut gate = NoiseGate::new(RATE);
        gate.set_threshold(-40.0);
        // Speech banks the hold; a fresh gate has none, as in the worklet.
        blocks(&mut gate, -20.0, 100);
        // Still open during the hold...
        let (gain, _) = blocks(&mut gate, -60.0, 200);
        assert!(gate.is_open() && gain > 0.9, "held open: {gain}");
        // ...then shut, and faded fully out.
        let (gain, _) = blocks(&mut gate, -60.0, 500);
        assert!(!gate.is_open() && gain < 0.01, "closed: {gain}");
    }

    #[test]
    fn speech_opens_it_within_a_block() {
        let mut gate = NoiseGate::new(RATE);
        gate.set_threshold(-40.0);
        blocks(&mut gate, -60.0, 600);
        assert!(!gate.is_open());
        // A few ms at speaking level: the first syllable gets through.
        let (gain, _) = blocks(&mut gate, -20.0, 10);
        assert!(gate.is_open() && gain > 0.9, "{gain}");
    }

    #[test]
    fn readings_come_about_thirty_times_a_second_with_the_peak() {
        let mut gate = NoiseGate::new(RATE);
        gate.set_threshold(-40.0);
        let (_, readings) = blocks(&mut gate, -30.0, 1000);
        assert!((28..=31).contains(&readings.len()), "{}", readings.len());
        let level = readings.last().unwrap().level;
        assert!((level + 30.0).abs() < 0.5, "a -30 dBFS tone reads {level}");
    }
}
