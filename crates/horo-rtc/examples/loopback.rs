//! Hear yourself through horo's whole audio path, with a network in between.
//!
//! ```sh
//! cargo run -p horo-rtc --example loopback -- [seconds] [delay-ms] [jitter-ms] [loss-%] [gate-dB]
//! cargo run -p horo-rtc --example loopback -- 30 400 60 5 -50
//! ```
//!
//! Microphone → noise gate → Opus → a simulated network (delay, jitter,
//! loss) → jitter buffer → loudspeaker. **Wear headphones**: through
//! speakers the microphone hears itself and howls.
//!
//! Prints the gate's level and the jitter buffer's counters once a second.

use std::{
    collections::BinaryHeap,
    cmp::Reverse,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use horo_devices::{Capture, FRAME, Playback};
use horo_rtc::audio::{
    codec::Encoder,
    gate::{GateReading, NoiseGate},
    jitter::JitterBuffer,
};

fn arg(n: usize, default: f64) -> f64 {
    std::env::args().nth(n).and_then(|a| a.parse().ok()).unwrap_or(default)
}

fn main() {
    let seconds = arg(1, 20.0);
    let delay = Duration::from_secs_f64(arg(2, 500.0) / 1000.0);
    let jitter_ms = arg(3, 40.0);
    let loss = arg(4, 2.0) / 100.0;
    let threshold = arg(5, -80.0) as f32;
    eprintln!(
        "loopback for {seconds} s: {delay:?} delay, up to {jitter_ms} ms jitter, {:.0}% loss, gate at {threshold} dB. Wear headphones.",
        loss * 100.0
    );

    // Packets on their way, ordered by when they "arrive".
    let (send, network) = mpsc::channel::<(Instant, u64, Vec<u8>)>();
    let reading = Arc::new(Mutex::new(None::<GateReading>));

    let _capture = {
        let reading = reading.clone();
        let mut gate = NoiseGate::new(48_000);
        gate.set_threshold(threshold);
        let mut encoder = Encoder::new().expect("encoder");
        let mut rtp = 0u64;
        let mut rng = 0x2545_F491_4F6C_DD1Du64;
        let mut uniform = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng >> 11) as f64 / (1u64 << 53) as f64
        };
        Capture::start(None, move |frame: &[f32; FRAME]| {
            let mut frame = *frame;
            if let Some(r) = gate.process(&mut frame) {
                *reading.lock().unwrap() = Some(r);
            }
            let packet = encoder.encode(&frame).expect("encode");
            if uniform() >= loss {
                let arrives = Instant::now() + delay + Duration::from_secs_f64(uniform() * jitter_ms / 1000.0);
                let _ = send.send((arrives, rtp, packet));
            }
            rtp += FRAME as u64;
        })
        .expect("microphone")
    };

    let buffer = Arc::new(Mutex::new(JitterBuffer::new().expect("decoder")));
    let _playback = {
        let buffer = buffer.clone();
        let mut in_flight: BinaryHeap<Reverse<(Instant, u64, Vec<u8>)>> = BinaryHeap::new();
        Playback::start(None, move |out: &mut [f32; FRAME]| {
            let now = Instant::now();
            while let Ok(packet) = network.try_recv() {
                in_flight.push(Reverse(packet));
            }
            let mut buffer = buffer.lock().unwrap();
            while in_flight.peek().is_some_and(|Reverse((at, _, _))| *at <= now) {
                let Reverse((at, rtp, packet)) = in_flight.pop().unwrap();
                buffer.push(rtp, packet, at);
            }
            buffer.pop(out).expect("decode");
        })
        .expect("loudspeaker")
    };

    let start = Instant::now();
    while start.elapsed().as_secs_f64() < seconds {
        std::thread::sleep(Duration::from_secs(1));
        let buffer = buffer.lock().unwrap();
        let s = buffer.stats();
        let level = reading.lock().unwrap().map(|r| format!("{:>6.1} dB {}", r.level, if r.open { "open " } else { "shut " })).unwrap_or_default();
        eprintln!(
            "{level} | buffered {:>3} ms, target {:>3} ms | decoded {} recovered {} concealed {} stretched {} late {} dropped {}",
            buffer.buffered().as_millis(),
            buffer.target_delay().as_millis(),
            s.decoded,
            s.recovered,
            s.concealed,
            s.stretched,
            s.late,
            s.dropped,
        );
    }
}
