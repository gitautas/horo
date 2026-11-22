//! Ogg/Opus files (RFC 7845), the container Matrix voice messages use:
//! [`write`] wraps Opus packets in one, [`read`] takes them back out.
//!
//! A file is a stream of Ogg pages (RFC 3533). The first page carries the
//! `OpusHead` header (channels, the encoder's pre-skip, the input rate),
//! the second `OpusTags`, and the rest the audio, each page stamped with
//! the 48 kHz sample count reached by its end. Packets are laced into
//! 255-byte segments; one can run on into the next page.

use super::SAMPLE_RATE;

/// What [`read`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusStream {
    pub channels: u8,
    /// Samples (at 48 kHz) the decoder should drop from the start.
    pub pre_skip: u16,
    pub packets: Vec<Vec<u8>>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OggError {
    #[error("not an Ogg file")]
    NotOgg,
    #[error("an Ogg file, but not Opus")]
    NotOpus,
    #[error("the file ends in the middle of a page")]
    Truncated,
}

/// The pre-skip libopus asks for at 48 kHz (its encoder's lookahead).
pub const DEFAULT_PRE_SKIP: u16 = 312;
/// Packets per audio page: a second of 20 ms frames.
const PACKETS_PER_PAGE: usize = 50;

/// Ogg's CRC-32: polynomial 0x04C11DB7, unreflected, starting at zero.
fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            let mut r = (i as u32) << 24;
            for _ in 0..8 {
                r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04C1_1DB7 } else { r << 1 };
            }
            *entry = r;
        }
        table
    });
    data.iter().fold(0u32, |crc, &b| (crc << 8) ^ table[((crc >> 24) as u8 ^ b) as usize])
}

/// One page holding whole `packets`.
fn page(out: &mut Vec<u8>, packets: &[&[u8]], flags: u8, granule: u64, serial: u32, sequence: u32) {
    let mut lacing = Vec::new();
    for packet in packets {
        let mut left = packet.len();
        while left >= 255 {
            lacing.push(255u8);
            left -= 255;
        }
        lacing.push(left as u8);
    }
    let start = out.len();
    out.extend_from_slice(b"OggS");
    out.push(0); // version
    out.push(flags);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&sequence.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // the CRC, filled in below
    out.push(lacing.len() as u8);
    out.extend_from_slice(&lacing);
    for packet in packets {
        out.extend_from_slice(packet);
    }
    let crc = crc32(&out[start..]);
    out[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
}

/// An Ogg/Opus file of mono 20 ms `packets` (as [`super::codec::Encoder`]
/// makes them). `frame_samples` is the samples each packet decodes to.
pub fn write(packets: &[Vec<u8>], frame_samples: u64, pre_skip: u16) -> Vec<u8> {
    let serial = 0x686f_726f; // "horo"
    let mut out = Vec::new();

    let mut head = b"OpusHead".to_vec();
    head.push(1); // version
    head.push(1); // channels
    head.extend_from_slice(&pre_skip.to_le_bytes());
    head.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes()); // output gain
    head.push(0); // channel mapping family
    page(&mut out, &[&head], 0x02, 0, serial, 0);

    let vendor = b"horo";
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes()); // no comments
    page(&mut out, &[&tags], 0, 0, serial, 1);

    let chunks: Vec<&[Vec<u8>]> = packets.chunks(PACKETS_PER_PAGE).collect();
    let mut granule = u64::from(pre_skip);
    for (i, chunk) in chunks.iter().enumerate() {
        granule += frame_samples * chunk.len() as u64;
        let last = i + 1 == chunks.len();
        let refs: Vec<&[u8]> = chunk.iter().map(Vec::as_slice).collect();
        page(&mut out, &refs, if last { 0x04 } else { 0 }, granule, serial, 2 + i as u32);
    }
    if chunks.is_empty() {
        page(&mut out, &[], 0x04, u64::from(pre_skip), serial, 2);
    }
    out
}

/// The Opus packets of an Ogg/Opus file (its first logical stream).
pub fn read(bytes: &[u8]) -> Result<OpusStream, OggError> {
    if !bytes.starts_with(b"OggS") {
        return Err(OggError::NotOgg);
    }
    let mut packets: Vec<Vec<u8>> = Vec::new();
    let mut partial: Vec<u8> = Vec::new();
    let mut serial = None;
    let mut at = 0;
    while at < bytes.len() {
        let header = bytes.get(at..at + 27).ok_or(OggError::Truncated)?;
        if &header[..4] != b"OggS" {
            return Err(OggError::NotOgg);
        }
        let page_serial = u32::from_le_bytes([header[14], header[15], header[16], header[17]]);
        let segments = header[26] as usize;
        let lacing = bytes.get(at + 27..at + 27 + segments).ok_or(OggError::Truncated)?;
        let body_len: usize = lacing.iter().map(|&l| l as usize).sum();
        let mut body_at = at + 27 + segments;
        let next = body_at + body_len;
        if next > bytes.len() {
            return Err(OggError::Truncated);
        }
        // Only the first stream: a file with several is unusual for voice.
        if *serial.get_or_insert(page_serial) == page_serial {
            for &l in lacing {
                partial.extend_from_slice(&bytes[body_at..body_at + l as usize]);
                body_at += l as usize;
                if l < 255 {
                    packets.push(std::mem::take(&mut partial));
                }
            }
        }
        at = next;
    }
    let head = packets.first().ok_or(OggError::NotOpus)?;
    if !head.starts_with(b"OpusHead") || head.len() < 19 {
        return Err(OggError::NotOpus);
    }
    let channels = head[9];
    let pre_skip = u16::from_le_bytes([head[10], head[11]]);
    // Past the head and the tags is the audio.
    let audio = packets.into_iter().skip(2).filter(|p| !p.is_empty()).collect();
    Ok(OpusStream { channels, pre_skip, packets: audio })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crc_is_oggs() {
        // The known check value of this CRC variant (CRC-32/MPEG-2 without
        // the final xor and init): "123456789" → 0x89A1897F.
        assert_eq!(crc32(b"123456789"), 0x89A1_897F);
    }

    #[test]
    fn packets_round_trip_across_pages_and_long_lacing() {
        let packets: Vec<Vec<u8>> = (0..130).map(|i| vec![i as u8; (i * 7) % 600 + 1]).collect();
        let file = write(&packets, 960, DEFAULT_PRE_SKIP);
        let stream = read(&file).unwrap();
        assert_eq!(stream.channels, 1);
        assert_eq!(stream.pre_skip, DEFAULT_PRE_SKIP);
        assert_eq!(stream.packets, packets);
    }

    #[test]
    fn pages_carry_valid_crcs_and_the_last_ends_the_stream() {
        let file = write(&[vec![1, 2, 3]], 960, DEFAULT_PRE_SKIP);
        let mut at = 0;
        let mut last_flags = 0;
        while at < file.len() {
            let segments = file[at + 26] as usize;
            let len = 27 + segments + file[at + 27..at + 27 + segments].iter().map(|&l| l as usize).sum::<usize>();
            let mut page = file[at..at + len].to_vec();
            let stored = u32::from_le_bytes(page[22..26].try_into().unwrap());
            page[22..26].copy_from_slice(&[0; 4]);
            assert_eq!(crc32(&page), stored);
            last_flags = file[at + 5];
            at += len;
        }
        assert_eq!(last_flags & 0x04, 0x04);
    }

    #[test]
    fn other_files_are_refused() {
        assert_eq!(read(b"ID3...."), Err(OggError::NotOgg));
        assert_eq!(read(b"OggS"), Err(OggError::Truncated));
    }
}
