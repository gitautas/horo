//! Video played inside the window, on macOS: AVFoundation's `AVPlayer`
//! decodes the file and plays its sound; an `AVPlayerItemVideoOutput` hands
//! over each frame as a `CVPixelBuffer`, in the one format gpui's `surface`
//! element draws (bi-planar 4:2:0, full range, Metal-compatible).
//!
//! Everything here runs on the main thread, as AppKit and gpui's views do.
//! The view asks [`VideoPlayer::frame`] for the frame due now on every
//! redraw, and asks for another redraw while the video plays.

use std::path::Path;

use objc2::{AnyThread, MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_av_foundation::{AVPlayer, AVPlayerActionAtItemEnd, AVPlayerItem, AVPlayerItemVideoOutput};
use objc2_core_foundation::{CFRetained, CFString};
use objc2_core_media::CMTime;
use objc2_core_video::{CVPixelBuffer, kCVPixelBufferMetalCompatibilityKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

/// A video or a sound, playing (or paused) in the window.
pub struct VideoPlayer {
    player: Retained<AVPlayer>,
    item: Retained<AVPlayerItem>,
    /// Where frames come from; none for audio.
    output: Option<Retained<AVPlayerItemVideoOutput>>,
    /// The last frame, kept for redraws between new ones and while paused.
    frame: Option<CFRetained<CVPixelBuffer>>,
}

/// A CoreFoundation string as the Foundation one it's toll-free bridged to.
fn ns(key: &CFString) -> &NSString {
    // SAFETY: CFString and NSString are toll-free bridged: the same object.
    unsafe { &*(key as *const CFString as *const NSString) }
}

impl VideoPlayer {
    /// Open a local file, playing from the start; `video` asks for its
    /// frames too. `None` off the main thread.
    pub fn open(path: &Path, video: bool) -> Option<VideoPlayer> {
        let mtm = MainThreadMarker::new()?;
        let url = NSURL::from_file_path(path)?;
        // SAFETY: AVFoundation calls with valid, retained arguments, on the
        // main thread (`mtm`).
        unsafe {
            let item = AVPlayerItem::playerItemWithURL(&url, mtm);
            let output = video.then(|| {
                let format = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange);
                let metal = NSNumber::new_bool(true);
                let attributes: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
                    &[ns(kCVPixelBufferPixelFormatTypeKey), ns(kCVPixelBufferMetalCompatibilityKey)],
                    &[format.as_ref() as &AnyObject, metal.as_ref() as &AnyObject],
                );
                let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(AVPlayerItemVideoOutput::alloc(), Some(&attributes));
                item.addOutput(&output);
                output
            });
            let player = AVPlayer::playerWithPlayerItem(Some(&item), mtm);
            // Stay on the last frame at the end rather than going blank.
            player.setActionAtItemEnd(AVPlayerActionAtItemEnd::Pause);
            player.play();
            Some(VideoPlayer { player, item, output, frame: None })
        }
    }

    /// The frame due now, or the last one shown.
    pub fn frame(&mut self) -> Option<CFRetained<CVPixelBuffer>> {
        // SAFETY: plain queries on objects this player owns.
        unsafe {
            let output = self.output.as_ref()?;
            let now = self.item.currentTime();
            if output.hasNewPixelBufferForItemTime(now)
                && let Some(buffer) = output.copyPixelBufferForItemTime_itemTimeForDisplay(now, std::ptr::null_mut())
            {
                // The same +1 reference, moved from objc2's wrapper to
                // CoreFoundation's, which is what gpui takes.
                let raw = std::ptr::NonNull::new(Retained::into_raw(buffer));
                self.frame = raw.map(|raw| CFRetained::from_raw(raw));
            }
        }
        self.frame.clone()
    }

    pub fn is_playing(&self) -> bool {
        // SAFETY: a plain query.
        unsafe { self.player.rate() > 0. }
    }

    pub fn toggle(&self) {
        // SAFETY: plain calls on the player; replaying from the end starts
        // over, as a player's play button does.
        unsafe {
            if self.is_playing() {
                self.player.pause();
            } else {
                if self.at_end() {
                    self.player.seekToTime(CMTime::with_seconds(0., 600));
                }
                self.player.play();
            }
        }
    }

    /// Where it's at and how long it is, in seconds (the length is 0 until
    /// the file has been read).
    pub fn position(&self) -> (f64, f64) {
        // SAFETY: plain queries.
        unsafe {
            let at = self.item.currentTime().seconds();
            let length = self.item.duration().seconds();
            (if at.is_finite() { at.max(0.) } else { 0. }, if length.is_finite() { length.max(0.) } else { 0. })
        }
    }

    fn at_end(&self) -> bool {
        let (at, length) = self.position();
        length > 0. && at >= length - 0.05
    }

    /// Jump to `fraction` (0 to 1) of the way through.
    pub fn seek(&self, fraction: f64) {
        let (_, length) = self.position();
        if length > 0. {
            // SAFETY: a plain call on the item.
            unsafe { self.player.seekToTime(CMTime::with_seconds(length * fraction.clamp(0., 1.), 600)) };
        }
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        // SAFETY: a plain call; the sound stops with the view that shows it.
        unsafe { self.player.pause() };
    }
}

/// "1:05", or "1:02:05" past an hour.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_like_a_player() {
        assert_eq!(clock(0.), "0:00");
        assert_eq!(clock(65.4), "1:05");
        assert_eq!(clock(3725.), "1:02:05");
        assert_eq!(clock(-3.), "0:00");
    }
}
