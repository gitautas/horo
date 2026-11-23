//! Other people's cameras: each remote video track decoded on a thread of
//! its own, the newest picture per person kept in [`Screens`] for the UI.
//!
//! Which track is whose comes from the subscriber offer: LiveKit marks
//! each media line `a=msid:<participant sid>|<track sid> …`. A track that
//! lost packets (or never had a keyframe) shows nothing new until the
//! sender, asked with a PLI, sends a full picture.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc as std_mpsc},
    time::{Duration, Instant},
};

use tokio::sync::mpsc;

use crate::{
    Mid,
    video::{Frame, Vp8Decoder},
};

/// The least time between two keyframe requests on one track: a sender
/// asked again before it could answer just wastes bandwidth.
const KEYFRAME_EVERY: Duration = Duration::from_millis(500);

/// A decoded picture, numbered so a view can tell a new one from the one
/// it already drew.
#[derive(Clone)]
pub struct Picture {
    pub frame: Arc<Frame>,
    pub seq: u64,
}

/// The newest picture from each person's camera, by identity. Cheap to
/// clone; read from any thread.
#[derive(Clone, Default)]
pub struct Screens(Arc<Mutex<HashMap<String, Picture>>>);

impl Screens {
    pub fn get(&self, identity: &str) -> Option<Picture> {
        self.0.lock().ok()?.get(identity).cloned()
    }

    fn set(&self, identity: &str, frame: Frame) {
        if let Ok(mut map) = self.0.lock() {
            let seq = map.get(identity).map_or(0, |p| p.seq + 1);
            map.insert(identity.to_owned(), Picture { frame: Arc::new(frame), seq });
        }
    }

    pub(super) fn remove(&self, identity: &str) {
        if let Ok(mut map) = self.0.lock() {
            map.remove(identity);
        }
    }

    pub(super) fn clear(&self) {
        if let Ok(mut map) = self.0.lock() {
            map.clear();
        }
    }
}

/// The participant sid each media line belongs to, from an SDP offer.
pub fn owners(sdp: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut mid = None;
    for line in sdp.lines() {
        let line = line.trim();
        if line.starts_with("m=") {
            mid = None;
        } else if let Some(m) = line.strip_prefix("a=mid:") {
            mid = Some(m.to_owned());
        } else if let Some(msid) = line.strip_prefix("a=msid:")
            && let Some(mid) = &mid
            && let Some(stream) = msid.split_whitespace().next()
        {
            let participant = stream.split('|').next().unwrap_or(stream);
            out.insert(mid.clone(), participant.to_owned());
        }
    }
    out
}

struct Track {
    /// Whose it is.
    identity: String,
    frames: std_mpsc::Sender<Arc<[u8]>>,
    /// Waiting for a keyframe: nothing decodes until one comes.
    awaiting_key: bool,
    last_request: Option<Instant>,
}

/// Every remote video track on the subscriber connection.
pub(super) struct Receiver {
    tracks: HashMap<Mid, Track>,
    pub screens: Screens,
    /// Decode threads ask for a keyframe here (a frame that wouldn't
    /// decode); the engine passes it on.
    pub broken: mpsc::UnboundedSender<Mid>,
}

impl Receiver {
    pub fn new(screens: Screens, broken: mpsc::UnboundedSender<Mid>) -> Receiver {
        Receiver { tracks: HashMap::new(), screens, broken }
    }

    /// One frame from `mid`, sent by `identity`. Returns whether a
    /// keyframe should be asked for now.
    pub fn frame(&mut self, mid: Mid, identity: &str, data: Arc<[u8]>, keyframe: bool, contiguous: bool) -> bool {
        let now = Instant::now();
        let broken = self.broken.clone();
        let screens = self.screens.clone();
        let track = self.tracks.entry(mid).or_insert_with(|| Track {
            identity: identity.to_owned(),
            frames: spawn_decoder(mid, identity.to_owned(), screens, broken),
            awaiting_key: true,
            last_request: None,
        });
        if keyframe {
            track.awaiting_key = false;
        } else if !contiguous {
            track.awaiting_key = true;
        }
        if track.awaiting_key {
            return Self::due(track, now);
        }
        let _ = track.frames.send(data);
        false
    }

    /// A decode failed: wait for, and ask for, a keyframe.
    pub fn broke(&mut self, mid: Mid) -> bool {
        match self.tracks.get_mut(&mid) {
            Some(track) => {
                track.awaiting_key = true;
                Self::due(track, Instant::now())
            }
            None => false,
        }
    }

    fn due(track: &mut Track, now: Instant) -> bool {
        if track.last_request.is_some_and(|at| now.duration_since(at) < KEYFRAME_EVERY) {
            return false;
        }
        track.last_request = Some(now);
        true
    }

    /// `identity` has no camera on any more (or left): forget its picture.
    pub fn gone(&mut self, identity: &str) {
        self.tracks.retain(|_, t| t.identity != identity);
        self.screens.remove(identity);
    }

    /// A new subscriber connection: every track starts over.
    pub fn reset(&mut self) {
        self.tracks.clear();
        self.screens.clear();
    }
}

/// A thread decoding one track's frames in order, ending when the track
/// is dropped.
fn spawn_decoder(mid: Mid, identity: String, screens: Screens, broken: mpsc::UnboundedSender<Mid>) -> std_mpsc::Sender<Arc<[u8]>> {
    let (tx, rx) = std_mpsc::channel::<Arc<[u8]>>();
    std::thread::Builder::new()
        .name(format!("vp8 {identity}"))
        .spawn(move || {
            let mut decoder = match Vp8Decoder::new() {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("{identity}: no VP8 decoder: {e}");
                    return;
                }
            };
            while let Ok(data) = rx.recv() {
                match decoder.decode(&data) {
                    Ok(Some(frame)) => screens.set(&identity, frame),
                    Ok(None) => {}
                    Err(e) => {
                        tracing::debug!("{identity}: {e}");
                        let _ = broken.send(mid);
                    }
                }
            }
        })
        .expect("spawning a decoder thread");
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_lines_are_matched_to_participants() {
        let sdp = "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\na=msid:PA_one|TR_a TR_a\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\na=mid:1\r\na=msid:PA_two|TR_b TR_b\r\nm=application 9 x\r\na=mid:2\r\n";
        let owners = owners(sdp);
        assert_eq!(owners.get("0").map(String::as_str), Some("PA_one"));
        assert_eq!(owners.get("1").map(String::as_str), Some("PA_two"));
        assert!(!owners.contains_key("2"));
    }
}
