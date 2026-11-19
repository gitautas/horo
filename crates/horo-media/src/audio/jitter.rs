//! The jitter buffer: one remote speaker's packets in, a steady 20 ms frame
//! out, whatever the network did on the way.
//!
//! Packets arrive late, early, out of order, twice, or not at all. The
//! buffer holds them by RTP time and plays them back on the audio device's
//! clock, keeping just enough delay to ride out the jitter it has measured:
//!
//! - **Delay.** The target is one frame plus three times the smoothed
//!   inter-arrival jitter (RFC 3550 §A.8), between 20 and 300 ms. Playback
//!   starts once that much is buffered.
//! - **Running dry.** When the next frame hasn't arrived and nothing after it
//!   has either, it is probably just late: a frame is concealed *without*
//!   moving the playhead, which adds 20 ms of delay (NetEQ's "expand"). That
//!   is how the delay grows when the network gets worse, and it gives the
//!   next packet time to arrive.
//! - **Loss.** A frame that is missing while later ones have arrived is
//!   lost: rebuilt from the FEC in the packet after it if that's there,
//!   concealed by Opus otherwise.
//! - **Too much delay.** When more than the target plus three frames has
//!   piled up (a burst after a stall, or a sender clock running fast), a
//!   frame is dropped to bring the delay back down.
//! - **Silence.** After 200 ms of running dry, playback pauses and
//!   rebuffers rather than inventing sound.
//!
//! Time comes in as an argument everywhere, so a whole call's worth of
//! network can be simulated in a test.
//!
//! Not yet: time-stretching (NetEQ's accelerate/expand), which would change
//! the delay without dropping or inventing whole frames.

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use super::{
    FRAME, FRAME_DURATION, SAMPLE_RATE,
    codec::{CodecError, Decoder},
};

const MIN_TARGET: Duration = FRAME_DURATION;
const MAX_TARGET: Duration = Duration::from_millis(300);
/// How much past the target is tolerated before frames are dropped.
const SLACK: Duration = Duration::from_millis(60);
/// Running dry for longer than this (200 ms) is inventing sound: rebuffer.
const MAX_STRETCHED: u32 = 10;
const FRAME_U64: u64 = FRAME as u64;

/// What came out of one [`JitterBuffer::pop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Played {
    /// Waiting for enough audio to start (or restart): the frame is silence.
    Buffering,
    /// A real frame.
    Decoded,
    /// The frame was lost and rebuilt from the next packet's FEC.
    Recovered,
    /// The frame was lost and invented by packet-loss concealment.
    Concealed,
    /// Nothing had arrived yet: a frame was invented and the playhead held,
    /// adding 20 ms of delay.
    Stretched,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JitterStats {
    pub received: u64,
    pub decoded: u64,
    pub recovered: u64,
    pub concealed: u64,
    /// Frames invented while waiting for late packets (each adds 20 ms).
    pub stretched: u64,
    /// Arrived after their time had already been played (or concealed).
    pub late: u64,
    pub duplicates: u64,
    /// Thrown away to bring the delay down.
    pub dropped: u64,
    /// Times playback stopped to rebuffer.
    pub rebuffers: u64,
}

pub struct JitterBuffer {
    decoder: Decoder,
    /// Packets by RTP time (48 kHz samples).
    packets: BTreeMap<u64, Vec<u8>>,
    /// The RTP time of the next frame to play, once playing.
    next: Option<u64>,
    /// RFC 3550 interarrival jitter, in seconds.
    jitter: f64,
    /// The newest packet's arrival time and RTP time.
    last_arrival: Option<(Instant, u64)>,
    stretched_in_a_row: u32,
    stats: JitterStats,
}

impl JitterBuffer {
    pub fn new() -> Result<JitterBuffer, CodecError> {
        Ok(JitterBuffer {
            decoder: Decoder::new()?,
            packets: BTreeMap::new(),
            next: None,
            jitter: 0.0,
            last_arrival: None,
            stretched_in_a_row: 0,
            stats: JitterStats::default(),
        })
    }

    pub fn stats(&self) -> JitterStats {
        self.stats
    }

    /// The delay the buffer is aiming for.
    pub fn target_delay(&self) -> Duration {
        let target = FRAME_DURATION.as_secs_f64() + 3.0 * self.jitter;
        Duration::from_secs_f64(target).clamp(MIN_TARGET, MAX_TARGET)
    }

    /// How much audio is waiting, counted from the next frame to play.
    pub fn buffered(&self) -> Duration {
        let (Some(&first), Some(&last)) = (self.packets.keys().next(), self.packets.keys().next_back()) else {
            return Duration::ZERO;
        };
        let start = self.next.unwrap_or(first);
        if last < start {
            return Duration::ZERO;
        }
        samples_to_duration(last - start + FRAME_U64)
    }

    /// A packet arrived. `rtp_time` counts samples at 48 kHz.
    pub fn push(&mut self, rtp_time: u64, packet: Vec<u8>, now: Instant) {
        self.stats.received += 1;
        if self.next.is_some_and(|next| rtp_time < next) {
            self.stats.late += 1;
            return;
        }
        if self.packets.contains_key(&rtp_time) {
            self.stats.duplicates += 1;
            return;
        }

        // RFC 3550 §A.8, on packets newer than any seen: how much the
        // spacing of arrivals differs from the spacing of RTP times.
        match self.last_arrival {
            Some((then, last_rtp)) if rtp_time > last_rtp => {
                let arrival = now.saturating_duration_since(then).as_secs_f64();
                let sent = (rtp_time - last_rtp) as f64 / SAMPLE_RATE as f64;
                let d = (arrival - sent).abs();
                self.jitter += (d - self.jitter) / 16.0;
                self.last_arrival = Some((now, rtp_time));
            }
            None => self.last_arrival = Some((now, rtp_time)),
            _ => {}
        }

        self.packets.insert(rtp_time, packet);
    }

    /// The next 20 ms of this speaker, on the audio device's clock.
    pub fn pop(&mut self, out: &mut [f32; FRAME]) -> Result<Played, CodecError> {
        let Some(next) = self.next else {
            // Not playing: start once the target delay is buffered.
            if self.buffered() >= self.target_delay() {
                self.next = self.packets.keys().next().copied();
                return self.pop(out);
            }
            out.fill(0.0);
            return Ok(Played::Buffering);
        };

        // Too much waiting: skip a frame to bring the delay back down.
        let mut next = next;
        if self.buffered() > self.target_delay() + SLACK && self.packets.contains_key(&next) {
            self.packets.remove(&next);
            self.stats.dropped += 1;
            next += FRAME_U64;
        }

        let ahead = self.packets.range(next + 1..).next().is_some();
        let played = if let Some(packet) = self.packets.remove(&next) {
            self.decoder.decode(&packet, out)?;
            self.stats.decoded += 1;
            Played::Decoded
        } else if let Some(following) = self.packets.get(&(next + FRAME_U64)) {
            self.decoder.recover(following, out)?;
            self.stats.recovered += 1;
            Played::Recovered
        } else if ahead {
            // Later frames are here and this one isn't: it's lost.
            self.decoder.conceal(out)?;
            self.stats.concealed += 1;
            Played::Concealed
        } else if self.stretched_in_a_row >= MAX_STRETCHED {
            // Nothing for 200 ms: the speaker stopped or the path is gone.
            // Stop inventing sound and wait for real audio again.
            self.stats.rebuffers += 1;
            self.next = None;
            self.stretched_in_a_row = 0;
            self.packets.clear();
            out.fill(0.0);
            return Ok(Played::Buffering);
        } else {
            // Running dry: the frame is probably late. Hold the playhead.
            self.decoder.conceal(out)?;
            self.stats.stretched += 1;
            self.stretched_in_a_row += 1;
            self.next = Some(next);
            return Ok(Played::Stretched);
        };

        self.stretched_in_a_row = 0;
        let next = next + FRAME_U64;
        self.next = Some(next);
        // Anything older than what's next will never play.
        self.packets = self.packets.split_off(&next);
        Ok(played)
    }
}

fn samples_to_duration(samples: u64) -> Duration {
    Duration::from_secs_f64(samples as f64 / SAMPLE_RATE as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::codec::Encoder;

    /// A deterministic network: per-packet delay and loss from a seeded LCG.
    struct Network {
        state: u64,
    }

    impl Network {
        fn new(seed: u64) -> Network {
            Network { state: seed }
        }
        fn uniform(&mut self) -> f64 {
            self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.state >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    struct Outcome {
        stats: JitterStats,
        /// How strongly the played audio is 440 Hz: its energy there over the
        /// strongest of its neighbours. Concealment's small phase jumps barely
        /// touch this, where they would fool a zero-crossing count.
        purity: f64,
        rms: f64,
        max_delay: Duration,
        buffering_after_start: usize,
    }

    /// Goertzel: the signal's power at one frequency.
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

    /// Send `seconds` of a 440 Hz tone across a simulated network and play
    /// it out on a 20 ms clock.
    fn simulate(seconds: u32, base_delay_ms: f64, jitter_ms: f64, loss: f64, seed: u64) -> Outcome {
        let mut encoder = Encoder::new().unwrap();
        let mut buffer = JitterBuffer::new().unwrap();
        let mut net = Network::new(seed);
        let start = Instant::now();
        let frames = (seconds * 50) as u64;

        // Every packet's arrival, then delivered in arrival order.
        let mut arrivals = Vec::new();
        for i in 0..frames {
            let pcm: [f32; FRAME] = std::array::from_fn(|n| {
                let t = ((i * FRAME_U64 + n as u64) % SAMPLE_RATE as u64) as f64 / SAMPLE_RATE as f64;
                ((t * 440.0 * std::f64::consts::TAU).sin() * 0.25) as f32
            });
            let packet = encoder.encode(&pcm).unwrap();
            if net.uniform() < loss {
                continue;
            }
            let sent = Duration::from_millis(i * 20);
            let delay = base_delay_ms + net.uniform() * jitter_ms;
            arrivals.push((sent + Duration::from_secs_f64(delay / 1000.0), i * FRAME_U64, packet));
        }
        arrivals.sort_by_key(|a| a.0);

        let mut out = [0f32; FRAME];
        let mut played = Vec::new();
        let mut started = false;
        let mut buffering_after_start = 0;
        let mut max_delay = Duration::ZERO;
        let mut next_arrival = 0;
        // The device clock: a pull every 20 ms, until the stream has played out.
        for tick in 0u64.. {
            if next_arrival == arrivals.len() && buffer.buffered() == Duration::ZERO {
                break;
            }
            let now = Duration::from_millis(tick * 20);
            while next_arrival < arrivals.len() && arrivals[next_arrival].0 <= now {
                let (at, rtp, packet) = arrivals[next_arrival].clone();
                buffer.push(rtp, packet, start + at);
                next_arrival += 1;
            }
            match buffer.pop(&mut out).unwrap() {
                Played::Buffering if started => buffering_after_start += 1,
                Played::Buffering => {}
                _ => {
                    started = true;
                    played.extend_from_slice(&out);
                }
            }
            if started {
                max_delay = max_delay.max(buffer.buffered());
            }
        }

        // Judge the middle: skip the decoder's ramp-up and the tail.
        let middle = &played[played.len() / 4..played.len() * 3 / 4];
        let tone = power_at(middle, 440.0);
        let neighbours = [330.0, 400.0, 480.0, 550.0, 880.0].map(|f| power_at(middle, f));
        let purity = tone / neighbours.iter().cloned().fold(f64::MIN_POSITIVE, f64::max);
        let rms = (middle.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / middle.len() as f64).sqrt();
        Outcome { stats: buffer.stats(), purity, rms, max_delay, buffering_after_start }
    }

    #[test]
    fn a_clean_network_plays_every_frame_with_minimal_delay() {
        let o = simulate(5, 30.0, 0.0, 0.0, 1);
        assert_eq!(o.stats.decoded, 250);
        assert_eq!((o.stats.concealed, o.stats.recovered, o.stats.late, o.stats.dropped), (0, 0, 0, 0), "{:?}", o.stats);
        assert!(o.purity > 100.0, "not a clean 440 Hz: purity {:.0}", o.purity);
        assert!(o.max_delay <= Duration::from_millis(60), "{:?}", o.max_delay);
    }

    #[test]
    fn jitter_raises_the_delay_instead_of_breaking_up() {
        // Up to 80 ms of jitter: packets reorder and bunch.
        let o = simulate(10, 30.0, 80.0, 0.0, 2);
        assert!(o.purity > 100.0, "not a clean 440 Hz: purity {:.0}", o.purity);
        // The delay grows to cover the jitter: almost nothing arrives too
        // late to play, and nothing that arrived is wasted on the way there.
        assert!(o.stats.late <= 5, "{:?}", o.stats);
        assert!(o.stats.decoded >= 490, "{:?}", o.stats);
        assert_eq!(o.buffering_after_start, 0, "it never stalls");
        assert!(o.max_delay <= Duration::from_millis(300), "{:?}", o.max_delay);
    }

    #[test]
    fn loss_is_rebuilt_or_concealed_and_the_tone_survives() {
        let o = simulate(10, 30.0, 10.0, 0.05, 3);
        let lost = 500 - o.stats.received;
        assert!(lost > 10, "the network should have lost some: {lost}");
        assert_eq!(o.stats.recovered + o.stats.concealed, lost, "every lost frame is filled: {:?}", o.stats);
        assert!(o.stats.recovered > 0, "FEC rebuilds some: {:?}", o.stats);
        assert!(o.purity > 30.0, "not a 440 Hz tone any more: purity {:.0}", o.purity);
        eprintln!("5% loss: {:?}, purity {:.0}, rms {:.3}", o.stats, o.purity, o.rms);
        // 0.25 amplitude sine: RMS ~0.177. Concealment keeps it close.
        assert!(o.rms > 0.14, "{}", o.rms);
        assert_eq!(o.buffering_after_start, 0, "once started, a frame every tick");
    }

    #[test]
    fn duplicates_and_late_packets_are_ignored() {
        let mut encoder = Encoder::new().unwrap();
        let mut buffer = JitterBuffer::new().unwrap();
        let now = Instant::now();
        let packet = |encoder: &mut Encoder| encoder.encode(&[0.1; FRAME]).unwrap();
        let p0 = packet(&mut encoder);
        buffer.push(0, p0.clone(), now);
        buffer.push(0, p0, now);
        assert_eq!(buffer.stats().duplicates, 1);

        let mut out = [0f32; FRAME];
        assert_eq!(buffer.pop(&mut out).unwrap(), Played::Decoded);
        // Frame 0 has played: another copy of it is late, not a duplicate.
        buffer.push(0, packet(&mut encoder), now);
        assert_eq!(buffer.stats().late, 1);
    }

    #[test]
    fn a_long_silence_rebuffers_instead_of_inventing_sound() {
        let mut encoder = Encoder::new().unwrap();
        let mut buffer = JitterBuffer::new().unwrap();
        let now = Instant::now();
        for i in 0..5u64 {
            buffer.push(i * FRAME_U64, encoder.encode(&[0.1; FRAME]).unwrap(), now + Duration::from_millis(i * 20));
        }
        let mut out = [0f32; FRAME];
        let mut kinds = Vec::new();
        for _ in 0..20 {
            kinds.push(buffer.pop(&mut out).unwrap());
        }
        let stretched = kinds.iter().filter(|k| **k == Played::Stretched).count();
        assert_eq!(stretched as u32, MAX_STRETCHED, "{kinds:?}");
        assert_eq!(*kinds.last().unwrap(), Played::Buffering);
        assert_eq!(buffer.stats().rebuffers, 1);
    }

    #[test]
    fn a_burst_after_a_stall_is_trimmed_back_to_the_target() {
        let mut encoder = Encoder::new().unwrap();
        let mut buffer = JitterBuffer::new().unwrap();
        let start = Instant::now();
        let mut out = [0f32; FRAME];
        // Steady for a second...
        for i in 0..50u64 {
            buffer.push(i * FRAME_U64, encoder.encode(&[0.1; FRAME]).unwrap(), start + Duration::from_millis(i * 20 + 20));
            buffer.pop(&mut out).unwrap();
        }
        // ...then 400 ms of packets land at once.
        let burst = start + Duration::from_millis(1100);
        for i in 50..70u64 {
            buffer.push(i * FRAME_U64, encoder.encode(&[0.1; FRAME]).unwrap(), burst);
        }
        let before = buffer.buffered();
        for _ in 0..15 {
            buffer.pop(&mut out).unwrap();
        }
        assert!(buffer.stats().dropped > 0, "{:?}", buffer.stats());
        assert!(buffer.buffered() < before, "{:?} -> {:?}", before, buffer.buffered());
        assert!(buffer.buffered() <= buffer.target_delay() + SLACK + FRAME_DURATION, "{:?}", buffer.buffered());
    }
}
