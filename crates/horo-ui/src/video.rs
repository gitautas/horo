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

/// A decoded call picture (I420) as the bi-planar buffer gpui's
/// `surface` draws: Y as it is, U and V interleaved.
pub fn pixel_buffer(frame: &horo_rtc::video::Frame) -> Option<CFRetained<CVPixelBuffer>> {
    use objc2_core_video::{CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress};
    let (w, h) = (frame.width as usize, frame.height as usize);
    let (cw, ch) = horo_rtc::video::Frame::chroma_size(frame.width, frame.height);
    let (cw, ch) = (cw as usize, ch as usize);
    // SAFETY: CoreVideo calls on a buffer we create and own; every write
    // stays inside the plane's rows as CoreVideo reports them, between
    // lock and unlock.
    unsafe {
        let metal = NSNumber::new_bool(true);
        let attributes: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(&[ns(kCVPixelBufferMetalCompatibilityKey)], &[metal.as_ref() as &AnyObject]);
        let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
        let attrs = &*(Retained::as_ptr(&attributes) as *const objc2_core_foundation::CFDictionary);
        let status = CVPixelBufferCreate(None, w, h, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, Some(attrs), std::ptr::NonNull::new(&mut out)?);
        if status != 0 {
            return None;
        }
        let buffer = CFRetained::from_raw(std::ptr::NonNull::new(out)?);
        CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags(0));
        let y_base = CVPixelBufferGetBaseAddressOfPlane(&buffer, 0) as *mut u8;
        let y_stride = CVPixelBufferGetBytesPerRowOfPlane(&buffer, 0);
        let uv_base = CVPixelBufferGetBaseAddressOfPlane(&buffer, 1) as *mut u8;
        let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(&buffer, 1);
        if !y_base.is_null() && !uv_base.is_null() {
            let (y, u, v) = (frame.y(), frame.u(), frame.v());
            for row in 0..h {
                std::ptr::copy_nonoverlapping(y[row * w..].as_ptr(), y_base.add(row * y_stride), w);
            }
            for row in 0..ch {
                let line = std::slice::from_raw_parts_mut(uv_base.add(row * uv_stride), cw * 2);
                for col in 0..cw {
                    line[col * 2] = u[row * cw + col];
                    line[col * 2 + 1] = v[row * cw + col];
                }
            }
        }
        CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags(0));
        Some(buffer)
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        // SAFETY: a plain call; the sound stops with the view that shows it.
        unsafe { self.player.pause() };
    }
}
