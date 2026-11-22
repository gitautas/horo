//! The audio path, between the network and the devices.
//!
//! Sending: microphone → [`echo`] → [`gate`] → [`codec::Encoder`] → `Peer::write_audio`.
//! Receiving: `PeerEvent::Audio` → one [`jitter::JitterBuffer`] per speaker
//! → [`mixer`] → loudspeaker.
//!
//! Everything runs at 48 kHz mono in 20 ms frames, the shape WebRTC voice
//! uses end to end; devices at other rates are resampled at the edge.

use std::time::Duration;

pub mod codec;
pub mod echo;
pub mod gate;
pub mod jitter;
pub mod mixer;
pub mod ogg;

pub const SAMPLE_RATE: u32 = 48_000;
/// Samples in one 20 ms frame at 48 kHz.
pub const FRAME: usize = 960;
pub const FRAME_DURATION: Duration = Duration::from_millis(20);
