//! Voice messages: recording one from the microphone, and playing one back,
//! with the same audio pipeline calls use, so it works wherever horo-rtc's
//! devices do (desktop now, phones later).
//!
//! - [`Recorder`] captures 20 ms frames, encodes each as Opus, and keeps a
//!   level per frame; [`Recorder::finish`] makes the Ogg/Opus file with its
//!   length and the waveform a voice message carries (MSC3245/MSC3246).
//! - [`decode`] turns an Ogg/Opus file back into 48 kHz mono samples, and
//!   [`VoicePlayer`] plays them, with pause, position and seek.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{
    audio::{
        FRAME, SAMPLE_RATE,
        codec::{CodecError, Encoder},
        ogg::{self, DEFAULT_PRE_SKIP, OggError},
    },
    devices::{Capture, DeviceError, Playback},
};

#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error("Opus: {0}")]
    Opus(#[from] opus::Error),
    #[error(transparent)]
    Ogg(#[from] OggError),
    #[error("couldn't encode the recording: {0}")]
    Encoding(String),
}

pub type Result<T, E = VoiceError> = std::result::Result<T, E>;

/// How many points a waveform has at most (Element sends about this many).
pub const WAVEFORM_POINTS: usize = 100;

/// A finished recording, ready to send.
#[derive(Debug, Clone)]
pub struct VoiceNote {
    /// An Ogg/Opus file.
    pub bytes: Vec<u8>,
    pub duration: Duration,
    /// Loudness over time, each 0 to 1.
    pub waveform: Vec<f32>,
}

#[derive(Default)]
struct Taken {
    packets: Vec<Vec<u8>>,
    /// Each frame's RMS level.
    levels: Vec<f32>,
    error: Option<String>,
}

/// The microphone, being recorded.
pub struct Recorder {
    capture: Option<Capture>,
    taken: Arc<Mutex<Taken>>,
}

impl Recorder {
    /// Start recording from `device` (the system default when `None`).
    pub fn start(device: Option<&str>) -> Result<Recorder> {
        let taken = Arc::new(Mutex::new(Taken::default()));
        let mut encoder = Encoder::new()?;
        let sink = taken.clone();
        let capture = Capture::start(device, move |frame: &[f32; FRAME]| {
            let level = (frame.iter().map(|s| s * s).sum::<f32>() / FRAME as f32).sqrt();
            let packet = encoder.encode(frame);
            let Ok(mut taken) = sink.lock() else { return };
            match packet {
                Ok(packet) => {
                    taken.packets.push(packet);
                    taken.levels.push(level);
                }
                Err(e) => taken.error = Some(e.to_string()),
            }
        })?;
        Ok(Recorder { capture: Some(capture), taken })
    }

    /// How long it's been recording.
    pub fn elapsed(&self) -> Duration {
        let frames = self.taken.lock().map(|t| t.packets.len()).unwrap_or(0);
        crate::audio::FRAME_DURATION * frames as u32
    }

    /// The last ~200 ms's level, 0 to 1, for a live meter.
    pub fn level(&self) -> f32 {
        let taken = self.taken.lock();
        let Ok(taken) = taken else { return 0. };
        let recent = taken.levels.iter().rev().take(10);
        let peak = recent.fold(0f32, |a, &b| a.max(b));
        loudness(peak)
    }

    /// Stop and make the file. `None` when nothing was recorded.
    pub fn finish(mut self) -> Result<Option<VoiceNote>> {
        // Dropping the capture stops the microphone before we read.
        drop(self.capture.take());
        let taken = match self.taken.lock() {
            Ok(mut taken) => std::mem::take(&mut *taken),
            Err(_) => return Err(VoiceError::Encoding("the recording was interrupted".into())),
        };
        if let Some(error) = taken.error {
            return Err(VoiceError::Encoding(error));
        }
        if taken.packets.is_empty() {
            return Ok(None);
        }
        let duration = crate::audio::FRAME_DURATION * taken.packets.len() as u32;
        Ok(Some(VoiceNote {
            bytes: ogg::write(&taken.packets, FRAME as u64, DEFAULT_PRE_SKIP),
            duration,
            waveform: waveform(&taken.levels, WAVEFORM_POINTS),
        }))
    }
}

/// An RMS level as a 0-to-1 loudness: a square root lifts quiet speech
/// into view, the way voice-message waveforms are usually drawn.
fn loudness(rms: f32) -> f32 {
    (rms * 4.).sqrt().clamp(0., 1.)
}

/// `levels` (one per frame) squeezed to at most `points`, each the loudest
/// of the frames it covers, as loudness 0 to 1.
pub fn waveform(levels: &[f32], points: usize) -> Vec<f32> {
    if levels.is_empty() || points == 0 {
        return Vec::new();
    }
    let points = points.min(levels.len());
    (0..points)
        .map(|i| {
            let start = i * levels.len() / points;
            let end = ((i + 1) * levels.len() / points).max(start + 1);
            loudness(levels[start..end].iter().copied().fold(0., f32::max))
        })
        .collect()
}

/// An Ogg/Opus file as 48 kHz mono samples, the encoder's pre-skip dropped.
/// Packets of any Opus frame size; stereo is mixed down.
pub fn decode(bytes: &[u8]) -> Result<Vec<f32>> {
    let stream = ogg::read(bytes)?;
    let channels = if stream.channels >= 2 { opus::Channels::Stereo } else { opus::Channels::Mono };
    let count = if stream.channels >= 2 { 2 } else { 1 };
    let mut decoder = opus::Decoder::new(SAMPLE_RATE, channels)?;
    // 120 ms, the longest Opus packet, per channel.
    let mut buffer = vec![0f32; 5760 * count];
    let mut out = Vec::new();
    for packet in &stream.packets {
        let samples = decoder.decode_float(packet, &mut buffer, false)?;
        if count == 1 {
            out.extend_from_slice(&buffer[..samples]);
        } else {
            out.extend(buffer[..samples * 2].as_chunks::<2>().0.iter().map(|[l, r]| (l + r) / 2.));
        }
    }
    let skip = usize::from(stream.pre_skip).min(out.len());
    out.drain(..skip);
    Ok(out)
}

struct Playhead {
    samples: Arc<Vec<f32>>,
    at: usize,
    playing: bool,
    volume: f32,
}

/// Decoded audio, playing through the speaker.
pub struct VoicePlayer {
    head: Arc<Mutex<Playhead>>,
    _playback: Playback,
}

impl VoicePlayer {
    /// Start playing `samples` (48 kHz mono) on `device` (the default when
    /// `None`).
    pub fn start(samples: Vec<f32>, device: Option<&str>) -> Result<VoicePlayer> {
        let head = Arc::new(Mutex::new(Playhead { samples: Arc::new(samples), at: 0, playing: true, volume: 1. }));
        let source = head.clone();
        let playback = Playback::start(device, move |frame: &mut [f32; FRAME]| {
            frame.fill(0.);
            let Ok(mut head) = source.lock() else { return };
            if !head.playing {
                return;
            }
            let (at, len, volume) = (head.at, head.samples.len(), head.volume);
            let take = FRAME.min(len.saturating_sub(at));
            for (out, sample) in frame.iter_mut().zip(&head.samples[at..at + take]) {
                *out = sample * volume;
            }
            head.at += take;
            if head.at >= len {
                head.playing = false;
            }
        })?;
        Ok(VoicePlayer { head, _playback: playback })
    }

    pub fn is_playing(&self) -> bool {
        self.head.lock().map(|h| h.playing).unwrap_or(false)
    }

    /// Pause, or play; from the end, play starts over.
    pub fn toggle(&self) {
        if let Ok(mut head) = self.head.lock() {
            if !head.playing && head.at >= head.samples.len() {
                head.at = 0;
            }
            head.playing = !head.playing;
        }
    }

    /// Where it's at and how long it is, in seconds.
    pub fn position(&self) -> (f64, f64) {
        let rate = f64::from(SAMPLE_RATE);
        self.head.lock().map(|h| (h.at as f64 / rate, h.samples.len() as f64 / rate)).unwrap_or((0., 0.))
    }

    /// Jump to `fraction` (0 to 1) of the way through.
    pub fn seek(&self, fraction: f64) {
        if let Ok(mut head) = self.head.lock() {
            head.at = ((head.samples.len() as f64) * fraction.clamp(0., 1.)) as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveforms_take_the_loudest_and_stay_in_range() {
        let levels = [0.0, 0.25, 0.0, 1.0];
        assert_eq!(waveform(&levels, 2), vec![loudness(0.25), 1.0]);
        assert!(waveform(&[0.5; 1000], WAVEFORM_POINTS).iter().all(|&p| (0. ..=1.).contains(&p)));
        assert_eq!(waveform(&[0.5; 1000], WAVEFORM_POINTS).len(), WAVEFORM_POINTS);
        assert!(waveform(&[], 10).is_empty());
    }

    #[test]
    fn a_recording_survives_ogg_and_opus() {
        // A second of a 440 Hz tone, encoded as the recorder does, written
        // as Ogg, read and decoded back.
        let mut encoder = Encoder::new().unwrap();
        let mut packets = Vec::new();
        for f in 0..50 {
            let mut frame = [0f32; FRAME];
            for (i, s) in frame.iter_mut().enumerate() {
                let t = (f * FRAME + i) as f32 / SAMPLE_RATE as f32;
                *s = 0.3 * (2. * std::f32::consts::PI * 440. * t).sin();
            }
            packets.push(encoder.encode(&frame).unwrap());
        }
        let file = ogg::write(&packets, FRAME as u64, DEFAULT_PRE_SKIP);
        // For checking with another decoder: `HORO_OGG_OUT=x.ogg cargo test`.
        if let Some(path) = std::env::var_os("HORO_OGG_OUT") {
            std::fs::write(path, &file).unwrap();
        }
        let samples = decode(&file).unwrap();
        assert_eq!(samples.len(), 50 * FRAME - DEFAULT_PRE_SKIP as usize);
        let rms = (samples[FRAME..].iter().map(|s| s * s).sum::<f32>() / (samples.len() - FRAME) as f32).sqrt();
        assert!(rms > 0.1, "the tone came back too quiet: {rms}");
    }
}
