//! Opus, the way a voice call uses it: 48 kHz mono, 20 ms frames.

use super::{FRAME, SAMPLE_RATE};

#[derive(Debug, thiserror::Error)]
#[error("Opus: {0}")]
pub struct CodecError(#[from] opus::Error);

/// The largest Opus packet for one 20 ms mono frame is far below this.
const MAX_PACKET: usize = 1500;

pub struct Encoder {
    inner: opus::Encoder,
}

impl Encoder {
    /// Voice mode, 32 kb/s, in-band FEC on: each packet carries a coarse copy
    /// of the previous frame, so one lost packet can be rebuilt from the next
    /// (see [`super::jitter`]). DTX stays off: a sender that goes quiet looks
    /// exactly like loss to the receiver, and the gate already sends silence
    /// cheaply.
    pub fn new() -> Result<Encoder, CodecError> {
        let mut inner = opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)?;
        inner.set_bitrate(opus::Bitrate::Bits(32_000))?;
        inner.set_inband_fec(true)?;
        // FEC is only produced when the encoder expects loss.
        inner.set_packet_loss_perc(10)?;
        inner.set_dtx(false)?;
        Ok(Encoder { inner })
    }

    /// Encode exactly one frame (960 samples, -1.0..=1.0).
    pub fn encode(&mut self, frame: &[f32; FRAME]) -> Result<Vec<u8>, CodecError> {
        let mut packet = vec![0u8; MAX_PACKET];
        let len = self.inner.encode_float(frame, &mut packet)?;
        packet.truncate(len);
        Ok(packet)
    }

    /// Tell the encoder how lossy the path is, so it spends bits on FEC
    /// accordingly (0..=100).
    pub fn set_expected_loss(&mut self, percent: i32) -> Result<(), CodecError> {
        Ok(self.inner.set_packet_loss_perc(percent.clamp(0, 100))?)
    }
}

pub struct Decoder {
    inner: opus::Decoder,
}

impl Decoder {
    pub fn new() -> Result<Decoder, CodecError> {
        Ok(Decoder { inner: opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono)? })
    }

    /// Decode one packet into one frame.
    pub fn decode(&mut self, packet: &[u8], out: &mut [f32; FRAME]) -> Result<(), CodecError> {
        let n = self.inner.decode_float(packet, out, false)?;
        out[n..].fill(0.0);
        Ok(())
    }

    /// Rebuild the frame *before* `next_packet` from the FEC it carries.
    pub fn recover(&mut self, next_packet: &[u8], out: &mut [f32; FRAME]) -> Result<(), CodecError> {
        let n = self.inner.decode_float(next_packet, out, true)?;
        out[n..].fill(0.0);
        Ok(())
    }

    /// Invent a plausible frame for one that never came (packet-loss
    /// concealment). Repeated calls fade to silence.
    pub fn conceal(&mut self, out: &mut [f32; FRAME]) -> Result<(), CodecError> {
        let n = self.inner.decode_float(&[], out, false)?;
        out[n..].fill(0.0);
        Ok(())
    }
}
