//! The notification sounds, synthesised rather than shipped as files, as
//! src/lib/sounds.ts does with Web Audio: each is a few tones with an
//! envelope, written as numbers. Same recipes, same names, so a setting
//! means the same sound in both apps.
//!
//! [`render`] turns a recipe into 48 kHz samples the way Web Audio would
//! play it (an 8 ms exponential attack, then an exponential tail; slides
//! are exponential in pitch). [`play`] sends them to the default speaker
//! through `horo_rtc::devices`, on a thread of its own.

use std::{
    f32::consts::{PI, TAU},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use horo_rtc::devices::{FRAME, Playback, SAMPLE_RATE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wave {
    Sine,
    Triangle,
    Square,
}

#[derive(Debug, Clone, Copy)]
pub struct Note {
    /// Seconds after the sound starts.
    pub at: f32,
    pub freq: f32,
    /// Seconds the tone rings for, envelope included.
    pub dur: f32,
    /// Slide to this frequency over the note's life.
    pub to: Option<f32>,
    pub wave: Wave,
    /// Relative loudness before the volume.
    pub level: f32,
    /// Hold at full level and only fade at the very end.
    pub sustain: bool,
}

const fn note(at: f32, freq: f32, dur: f32, wave: Wave, level: f32) -> Note {
    Note { at, freq, dur, to: None, wave, level, sustain: false }
}

const fn slide(at: f32, freq: f32, to: f32, dur: f32, level: f32) -> Note {
    Note { at, freq, dur, to: Some(to), wave: Wave::Sine, level, sustain: false }
}

/// Equal temperament from A4, so the tunes read as note names.
fn hz(semitones: f32) -> f32 {
    440.0 * 2f32.powf(semitones / 12.0)
}

pub struct Sound {
    pub id: &'static str,
    pub notes: fn() -> Vec<Note>,
}

use Wave::*;

/// Short, quiet, over before you've finished reading the message.
pub const MESSAGE_SOUNDS: &[Sound] = &[
    Sound { id: "blip", notes: || vec![note(0.0, hz(7.0), 0.09, Triangle, 1.0), note(0.075, hz(14.0), 0.13, Triangle, 1.0)] },
    Sound { id: "pop", notes: || vec![slide(0.0, 620.0, 240.0, 0.13, 1.1)] },
    Sound { id: "bubble", notes: || vec![slide(0.0, 300.0, 900.0, 0.12, 1.1)] },
    Sound {
        id: "chime",
        notes: || vec![note(0.0, hz(4.0), 0.5, Sine, 1.0), note(0.02, hz(11.0), 0.55, Sine, 0.7), note(0.04, hz(16.0), 0.6, Sine, 0.5)],
    },
    Sound {
        id: "sparkle",
        notes: || vec![note(0.0, hz(16.0), 0.08, Sine, 1.0), note(0.06, hz(21.0), 0.08, Sine, 0.8), note(0.12, hz(28.0), 0.22, Sine, 0.6)],
    },
    Sound { id: "knock", notes: || vec![slide(0.0, 180.0, 90.0, 0.1, 1.3), slide(0.13, 180.0, 90.0, 0.1, 1.3)] },
    Sound {
        id: "uwu",
        notes: || vec![note(0.0, hz(9.0), 0.11, Square, 0.5), note(0.1, hz(16.0), 0.11, Square, 0.5), note(0.2, hz(21.0), 0.26, Square, 0.5)],
    },
];

/// Longer and more insistent: a call is asking you a question.
pub const CALL_SOUNDS: &[Sound] = &[
    Sound {
        id: "ring",
        // A desk phone's warble: 440 and 480 Hz together, twice.
        notes: || [0.0, 0.9].iter().flat_map(|&at| [note(at, 440.0, 0.55, Sine, 0.8), note(at, 480.0, 0.55, Sine, 0.8)]).collect(),
    },
    Sound {
        id: "arcade",
        notes: || {
            [0.0, 0.7]
                .iter()
                .flat_map(|&offset| [9.0, 16.0, 21.0, 28.0].iter().enumerate().map(move |(i, &step)| note(offset + i as f32 * 0.09, hz(step), 0.1, Square, 0.55)))
                .collect()
        },
    },
    Sound {
        id: "bell",
        notes: || [0.0, 0.55, 1.1].iter().flat_map(|&at| [note(at, hz(12.0), 0.7, Triangle, 1.0), note(at + 0.01, hz(24.0), 0.5, Sine, 0.45)]).collect(),
    },
    Sound { id: "chirp", notes: || [0.0, 0.28, 0.56, 0.84].iter().map(|&at| slide(at, 700.0, 1500.0, 0.12, 0.9)).collect() },
];

/// Silence, in either list.
pub const NONE: &str = "none";

fn find(list: &[Sound], id: &str) -> Option<Vec<Note>> {
    list.iter().find(|s| s.id == id).map(|s| (s.notes)())
}

const FLOOR: f32 = 0.0001;
const ATTACK: f32 = 0.008;

/// A recipe as samples: every note mixed in, clipped to ±1.
pub fn render(notes: &[Note], volume: f32) -> Vec<f32> {
    let rate = SAMPLE_RATE as f32;
    let length = notes.iter().map(|n| n.at + n.dur).fold(0.0, f32::max) + 0.03;
    let mut out = vec![0.0f32; (length * rate) as usize];
    if volume <= 0.0 {
        return out;
    }
    for n in notes {
        let peak = (0.22 * volume * n.level).max(FLOOR);
        let start = (n.at * rate) as usize;
        let samples = ((n.dur + 0.02) * rate) as usize;
        let mut phase = 0.0f32;
        for i in 0..samples {
            let Some(slot) = out.get_mut(start + i) else { break };
            let t = i as f32 / rate;
            let freq = match n.to {
                Some(to) => n.freq * (to.max(1.0) / n.freq).powf((t / n.dur).min(1.0)),
                None => n.freq,
            };
            phase = (phase + TAU * freq / rate) % TAU;
            let tone = match n.wave {
                Sine => phase.sin(),
                Square => phase.sin().signum(),
                Triangle => 2.0 / PI * phase.sin().asin(),
            };
            *slot += tone * gain(n, peak, t);
        }
    }
    for s in &mut out {
        *s = s.clamp(-1.0, 1.0);
    }
    out
}

/// Web Audio's envelope: exponential up to `peak` over the attack, then
/// exponential down to the floor by the note's end (held first, for a
/// sustained note), silent after.
fn gain(n: &Note, peak: f32, t: f32) -> f32 {
    let exp = |from: f32, to: f32, x: f32| from * (to / from).powf(x.clamp(0.0, 1.0));
    if t < ATTACK {
        return exp(FLOOR, peak, t / ATTACK);
    }
    if t >= n.dur {
        return 0.0;
    }
    if n.sustain {
        let hold = (n.dur - 0.04).max(ATTACK);
        if t < hold {
            return peak;
        }
        return exp(peak, FLOOR, (t - hold) / (n.dur - hold));
    }
    exp(peak, FLOOR, (t - ATTACK) / (n.dur - ATTACK))
}

/// Which list a sound comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Message,
    Call,
}

/// What a caller hears while the other end rings: 440 and 480 Hz held steady
/// for two seconds, then four of silence, matching `RINGBACK` in
/// `src/lib/sounds.ts`.
fn ringback_notes() -> Vec<Note> {
    [440.0, 480.0]
        .into_iter()
        .map(|freq| Note { at: 0.0, freq, dur: 2.0, to: None, wave: Sine, level: 0.35, sustain: true })
        .collect()
}

/// Silence between ringback bursts, in seconds.
const RINGBACK_GAP_SECS: f32 = 4.0;
/// Stop ringback after this long even if nobody answers (`RING_TIMEOUT_MS`).
const RINGBACK_TIMEOUT: Duration = Duration::from_secs(45);

/// A ringback tone playing while waiting for someone to pick up. Dropping it
/// silences the tone immediately.
pub struct Ringback {
    stop: Arc<AtomicBool>,
}

impl Drop for Ringback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Play ringback on `output` (or the default speaker) until the handle is
/// dropped or [`RINGBACK_TIMEOUT`] passes. Played even when call notification
/// sounds are set to `"none"`: it's how a caller knows the call went out.
pub fn start_ringback(volume: f32, output: Option<String>) -> Ringback {
    let stop = Arc::new(AtomicBool::new(false));
    if volume <= 0.0 {
        return Ringback { stop };
    }
    let mut cycle = render(&ringback_notes(), volume);
    let silence = (RINGBACK_GAP_SECS * SAMPLE_RATE as f32) as usize;
    cycle.resize(cycle.len() + silence, 0.0);
    let samples = Arc::new(cycle);
    let flag = stop.clone();
    let spawned = std::thread::Builder::new().name("horo-ringback".into()).spawn(move || {
        let mut at = 0usize;
        let len = samples.len().max(1);
        let source = samples.clone();
        let cb_stop = flag.clone();
        let playback = Playback::start(output.as_deref(), move |frame: &mut [f32; FRAME]| {
            if cb_stop.load(Ordering::Relaxed) {
                frame.fill(0.0);
                return;
            }
            for slot in frame.iter_mut() {
                *slot = source[at];
                at += 1;
                if at >= len {
                    at = 0;
                }
            }
        });
        match playback {
            Ok(_playback) => {
                let started = Instant::now();
                while !flag.load(Ordering::Relaxed) && started.elapsed() < RINGBACK_TIMEOUT {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            Err(e) => tracing::warn!("couldn't play ringback: {e}"),
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("couldn't start ringback thread: {e}");
    }
    Ringback { stop }
}

/// Short synthesized cues for voice call events (Discord-style).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallCue {
    Join,
    Leave,
    PeerJoin,
    PeerLeave,
    Mute,
    Unmute,
    Deafen,
    Undeafen,
}

fn call_cue_notes(cue: CallCue) -> Vec<Note> {
    match cue {
        // Warm rising fifth when you connect.
        CallCue::Join => vec![note(0.0, hz(0.0), 0.09, Triangle, 0.75), note(0.065, hz(7.0), 0.14, Triangle, 0.85)],
        // Falling fifth when you disconnect.
        CallCue::Leave => vec![note(0.0, hz(7.0), 0.09, Triangle, 0.75), note(0.065, hz(0.0), 0.14, Triangle, 0.7)],
        // Soft rising two-note pop when someone joins the call.
        CallCue::PeerJoin => vec![note(0.0, hz(5.0), 0.07, Sine, 0.65), note(0.055, hz(12.0), 0.11, Sine, 0.7)],
        // Soft falling two-note pop when someone leaves the call.
        CallCue::PeerLeave => vec![note(0.0, hz(10.0), 0.07, Sine, 0.6), note(0.055, hz(3.0), 0.11, Sine, 0.55)],
        // Low muted click-drop.
        CallCue::Mute => vec![slide(0.0, 440.0, 260.0, 0.08, 0.65)],
        // Crisp upward click.
        CallCue::Unmute => vec![slide(0.0, 300.0, 520.0, 0.08, 0.65)],
        // Deeper double drop for deafen.
        CallCue::Deafen => vec![note(0.0, hz(2.0), 0.07, Triangle, 0.7), note(0.055, hz(-5.0), 0.11, Triangle, 0.7)],
        // Crisp double rise for undeafen.
        CallCue::Undeafen => vec![note(0.0, hz(-5.0), 0.07, Triangle, 0.7), note(0.055, hz(2.0), 0.11, Triangle, 0.75)],
    }
}

/// Play a short call feedback cue on `output` (or the default speaker).
pub fn play_call_cue(cue: CallCue, volume: f32, output: Option<String>) {
    if volume <= 0.0 {
        return;
    }
    let notes = call_cue_notes(cue);
    let samples = Arc::new(render(&notes, volume * 0.85));
    let spawned = std::thread::Builder::new().name("horo-cue".into()).spawn(move || {
        let length = Duration::from_secs_f32(samples.len() as f32 / SAMPLE_RATE as f32);
        let mut at = 0;
        let source = samples.clone();
        let playback = Playback::start(output.as_deref(), move |frame: &mut [f32; FRAME]| {
            for slot in frame.iter_mut() {
                *slot = source.get(at).copied().unwrap_or(0.0);
                at += 1;
            }
        });
        match playback {
            Ok(_playback) => std::thread::sleep(length + Duration::from_millis(40)),
            Err(e) => tracing::warn!("couldn't play call cue: {e}"),
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("couldn't start cue thread: {e}");
    }
}

/// Play a sound by id on the default speaker. Unknown ids and `none` are
/// silence: a settings file from another build shouldn't fail every message.
pub fn play(kind: Kind, id: &str, volume: f32) {
    if id == NONE || volume <= 0.0 {
        return;
    }
    let list = match kind {
        Kind::Message => MESSAGE_SOUNDS,
        Kind::Call => CALL_SOUNDS,
    };
    let Some(notes) = find(list, id) else { return };
    let samples = Arc::new(render(&notes, volume));
    let spawned = std::thread::Builder::new().name("horo-sound".into()).spawn(move || {
        let length = Duration::from_secs_f32(samples.len() as f32 / SAMPLE_RATE as f32);
        let mut at = 0;
        let source = samples.clone();
        let playback = Playback::start(None, move |frame: &mut [f32; FRAME]| {
            for slot in frame.iter_mut() {
                *slot = source.get(at).copied().unwrap_or(0.0);
                at += 1;
            }
        });
        match playback {
            // Held open for the sound and a little after, so the device's
            // own buffer drains before the stream closes.
            Ok(_playback) => std::thread::sleep(length + Duration::from_millis(250)),
            Err(e) => tracing::warn!("couldn't play a sound: {e}"),
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("couldn't play a sound: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_renders_audible_and_unclipped() {
        for sound in MESSAGE_SOUNDS.iter().chain(CALL_SOUNDS) {
            let samples = render(&(sound.notes)(), 1.0);
            let peak = samples.iter().fold(0.0f32, |p, s| p.max(s.abs()));
            assert!(peak > 0.05, "{} is nearly silent ({peak})", sound.id);
            assert!(peak < 1.0, "{} clips", sound.id);
        }
    }

    #[test]
    fn notes_fade_out_by_their_end() {
        let n = note(0.0, 440.0, 0.1, Sine, 1.0);
        let samples = render(&[n], 1.0);
        let tail = &samples[(0.099 * SAMPLE_RATE as f32) as usize..];
        assert!(tail.iter().all(|s| s.abs() < 0.001), "the tail rings on");
        let middle = &samples[(0.01 * SAMPLE_RATE as f32) as usize..(0.03 * SAMPLE_RATE as f32) as usize];
        assert!(middle.iter().any(|s| s.abs() > 0.05));
    }

    #[test]
    fn volume_zero_and_unknown_ids_are_silent() {
        assert!(render(&(MESSAGE_SOUNDS[0].notes)(), 0.0).iter().all(|s| *s == 0.0));
        assert!(find(MESSAGE_SOUNDS, "not-a-sound").is_none());
    }

    #[test]
    fn ringback_renders_audible_and_unclipped() {
        let samples = render(&ringback_notes(), 1.0);
        let peak = samples.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        assert!(peak > 0.05 && peak < 1.0, "unexpected ringback peak {peak}");
    }
}
