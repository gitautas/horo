//! The camera, on Linux: one of PipeWire's video sources (V4L2 or
//! libcamera behind it), read as raw video at up to 720p and turned into
//! I420 on the capture's thread (see `pw_video`). The newest picture is
//! kept for our own tile and the settings preview.
//!
//! Nothing to ask first: a desktop app reaches PipeWire's cameras directly.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use horo_rtc::video::Frame;

use crate::pw_video;

/// 720p: plenty for a call, and what the encoder can keep up with.
const SIZE: (u32, u32) = (1280, 720);

/// A camera the system offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraInfo {
    /// Its PipeWire node name.
    pub id: String,
    pub name: String,
    /// The one used when none is chosen.
    pub is_default: bool,
}

/// The cameras plugged in now; the first is what PipeWire's session
/// manager would pick on its own.
pub fn cameras() -> Vec<CameraInfo> {
    match pw_video::cameras() {
        Ok(found) => found
            .into_iter()
            .enumerate()
            .map(|(i, c)| CameraInfo { id: c.name, name: c.description, is_default: i == 0 })
            .collect(),
        Err(e) => {
            tracing::warn!("listing cameras: {e}");
            Vec::new()
        }
    }
}

/// Whether we may use the camera: always, on Linux.
pub fn allowed() -> Option<bool> {
    Some(true)
}

/// Ask for the camera: nothing to ask, on Linux.
pub fn ask(done: impl Fn(bool) + 'static) {
    done(true);
}

/// Called with each picture, on the capture's thread (a call's encoder).
pub type OnFrame = Box<dyn Fn(&Frame) + Send + Sync>;

/// The newest picture, numbered so a view can tell it from the last.
type Latest = Arc<Mutex<Option<(u64, Arc<Frame>)>>>;

/// A camera, running. Dropping it turns it off.
pub struct Camera {
    _capture: pw_video::Capture,
    latest: Latest,
}

impl Camera {
    /// Start `id` (PipeWire's pick when `None`). Blocks until it runs: off
    /// the UI thread.
    pub fn start(id: Option<&str>) -> Result<Camera, String> {
        Camera::start_with(id, None)
    }

    /// As [`Camera::start`], with `on_frame` hearing every picture.
    pub fn start_with(id: Option<&str>, on_frame: Option<OnFrame>) -> Result<Camera, String> {
        let latest: Latest = Arc::default();
        let seq = AtomicU64::new(0);
        let keep = latest.clone();
        let deliver: pw_video::OnFrame = Box::new(move |frame| {
            if let Some(on_frame) = &on_frame {
                on_frame(&frame);
            }
            if let Ok(mut latest) = keep.lock() {
                *latest = Some((seq.fetch_add(1, Ordering::Relaxed), Arc::new(frame)));
            }
        });
        let capture = pw_video::start(pw_video::Source::Camera(id.map(str::to_owned)), SIZE, deliver)
            .map_err(|e| if id.is_some() { format!("{e} (is the camera still plugged in?)") } else { e })?;
        Ok(Camera { _capture: capture, latest })
    }

    /// The newest picture and its number.
    pub fn latest(&self) -> Option<(u64, Arc<Frame>)> {
        self.latest.lock().ok()?.clone()
    }

    /// The newest picture as a mirror shows it (how people expect to see
    /// themselves; what's sent stays the right way round), drawn under
    /// `key`.
    pub fn mirrored_image(&self, key: &str, cx: &mut gpui::App) -> Option<Arc<gpui::RenderImage>> {
        let (seq, frame) = self.latest()?;
        crate::picture::image(key, seq, &frame, true, cx)
    }
}
