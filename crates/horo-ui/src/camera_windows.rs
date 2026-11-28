//! The camera, on Windows: Media Foundation via `nokhwa`, read on a capture
//! thread and converted to video-range I420 at up to 720p / 30 fps for the
//! VP8 encoder and local preview.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use horo_rtc::video::Frame;
use nokhwa::{
    Camera as NokhwaCamera,
    pixel_format::RgbFormat,
    utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType},
};

/// 720p: plenty for a call, and what the encoder can keep up with.
const MAX_SIZE: (u32, u32) = (1280, 720);
/// Cap capture at ~30 fps.
const MIN_INTERVAL: Duration = Duration::from_millis(30);

/// A camera the system offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraInfo {
    /// Its Media Foundation index or symbolic link.
    pub id: String,
    pub name: String,
    /// The one used when none is chosen.
    pub is_default: bool,
}

/// The cameras plugged in now.
pub fn cameras() -> Vec<CameraInfo> {
    match nokhwa::query(ApiBackend::MediaFoundation) {
        Ok(found) => found
            .into_iter()
            .enumerate()
            .map(|(i, c)| {
                let id = match c.index() {
                    CameraIndex::Index(n) => n.to_string(),
                    CameraIndex::String(s) => s.clone(),
                };
                CameraInfo { id, name: c.human_name(), is_default: i == 0 }
            })
            .collect(),
        Err(e) => {
            tracing::warn!("listing cameras: {e}");
            Vec::new()
        }
    }
}

/// Whether we may use the camera: always allowed to try on Windows.
pub fn allowed() -> Option<bool> {
    Some(true)
}

/// Ask for the camera: Windows prompts automatically when opened.
pub fn ask(done: impl Fn(bool) + 'static) {
    done(true);
}

/// Called with each picture, on the capture's thread (a call's encoder).
pub type OnFrame = Box<dyn Fn(&Frame) + Send + Sync>;

/// The newest picture, numbered so a view can tell it from the last.
type Latest = Arc<Mutex<Option<(u64, Arc<Frame>)>>>;

/// A camera, running. Dropping it stops capture and releases the device.
pub struct Camera {
    stop: Arc<AtomicBool>,
    latest: Latest,
}

impl Drop for Camera {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Camera {
    /// Start `id` (device 0 when `None`). Blocks until the stream opens: off
    /// the UI thread.
    pub fn start(id: Option<&str>) -> Result<Camera, String> {
        Camera::start_with(id, None)
    }

    /// As [`Camera::start`], with `on_frame` hearing every picture.
    pub fn start_with(id: Option<&str>, on_frame: Option<OnFrame>) -> Result<Camera, String> {
        let index = match id {
            Some(s) => match s.parse::<u32>() {
                Ok(n) => CameraIndex::Index(n),
                Err(_) => CameraIndex::String(s.to_owned()),
            },
            None => CameraIndex::Index(0),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let latest: Latest = Arc::default();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let stop_bg = stop.clone();
        let keep = latest.clone();
        let had_id = id.is_some();

        std::thread::Builder::new()
            .name("horo camera".into())
            .spawn(move || {
                let requested = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
                let mut cam = match NokhwaCamera::with_backend(
                    index.clone(),
                    requested,
                    ApiBackend::MediaFoundation,
                )
                .or_else(|_| {
                    let fallback = RequestedFormat::new::<RgbFormat>(RequestedFormatType::None);
                    NokhwaCamera::with_backend(index, fallback, ApiBackend::MediaFoundation)
                }) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                if let Err(e) = cam.open_stream() {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
                let _ = ready_tx.send(Ok(()));

                let seq = AtomicU64::new(0);
                let mut last: Option<Instant> = None;
                while !stop_bg.load(Ordering::Relaxed) {
                    let Ok(buf) = cam.frame() else {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    };
                    let now = Instant::now();
                    if last.is_some_and(|at| now.duration_since(at) < MIN_INTERVAL) {
                        continue;
                    }
                    let Ok(rgb) = buf.decode_image::<RgbFormat>() else {
                        continue;
                    };
                    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
                    if let Some(frame) = rgb_to_i420(rgb.as_raw(), w, h, MAX_SIZE) {
                        last = Some(now);
                        if let Some(cb) = &on_frame {
                            cb(&frame);
                        }
                        if let Ok(mut slot) = keep.lock() {
                            *slot = Some((seq.fetch_add(1, Ordering::Relaxed), Arc::new(frame)));
                        }
                    }
                }
                let _ = cam.stop_stream();
            })
            .map_err(|e| format!("couldn't start a camera thread: {e}"))?;

        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(Camera { stop, latest }),
            Ok(Err(e)) => Err(if had_id { format!("{e} (is the camera still plugged in?)") } else { e }),
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                Err("the camera didn't start sending pictures".into())
            }
        }
    }

    /// The newest picture and its number.
    pub fn latest(&self) -> Option<(u64, Arc<Frame>)> {
        self.latest.lock().ok()?.clone()
    }

    /// The newest picture mirrored left-to-right for self-preview.
    pub fn mirrored_image(&self, key: &str, cx: &mut gpui::App) -> Option<Arc<gpui::RenderImage>> {
        let (seq, frame) = self.latest()?;
        crate::picture::image(key, seq, &frame, true, cx)
    }
}

/// Convert packed RGB24 (`w × h`) to video-range BT.601 I420, scaled down to
/// fit `max` in even dimensions.
fn rgb_to_i420(rgb: &[u8], w: usize, h: usize, max: (u32, u32)) -> Option<Frame> {
    if w == 0 || h == 0 || rgb.len() < w * h * 3 {
        return None;
    }
    let scale = (max.0 as f64 / w as f64).min(max.1 as f64 / h as f64).min(1.);
    let even = |v: usize| ((v as f64 * scale) as usize & !1).max(2);
    let (ow, oh) = (even(w), even(h));
    let mut out = Frame::black(ow as u32, oh as u32);
    let (ocw, och) = (ow.div_ceil(2), oh.div_ceil(2));
    let (y, rest) = out.data.split_at_mut(ow * oh);
    let (u, v) = rest.split_at_mut(ocw * och);
    let xs: Vec<usize> = (0..ow).map(|x| x * w / ow).collect();
    let ys: Vec<usize> = (0..oh).map(|y| y * h / oh).collect();
    let cxs: Vec<usize> = (0..ocw).map(|x| (x * 2 * w / ow).min(w - 1)).collect();
    let cys: Vec<usize> = (0..och).map(|y| (y * 2 * h / oh).min(h - 1)).collect();
    let px = |sx: usize, sy: usize| -> (i32, i32, i32) {
        let at = (sy * w + sx) * 3;
        (i32::from(rgb[at]), i32::from(rgb[at + 1]), i32::from(rgb[at + 2]))
    };
    for (row, &sy) in ys.iter().enumerate() {
        for (col, &sx) in xs.iter().enumerate() {
            let (r, g, b) = px(sx, sy);
            y[row * ow + col] = (16 + ((66 * r + 129 * g + 25 * b + 128) >> 8)).clamp(16, 235) as u8;
        }
    }
    for (row, &sy) in cys.iter().enumerate() {
        let sy1 = (sy + 1).min(h - 1);
        for (col, &sx) in cxs.iter().enumerate() {
            let sx1 = (sx + 1).min(w - 1);
            let (mut sr, mut sg, mut sb) = (0, 0, 0);
            for (x, y) in [(sx, sy), (sx1, sy), (sx, sy1), (sx1, sy1)] {
                let (r, g, b) = px(x, y);
                (sr, sg, sb) = (sr + r, sg + g, sb + b);
            }
            let (r, g, b) = ((sr + 2) / 4, (sg + 2) / 4, (sb + 2) / 4);
            u[row * ocw + col] = (128 + ((-38 * r - 74 * g + 112 * b + 128) >> 8)).clamp(16, 240) as u8;
            v[row * ocw + col] = (128 + ((112 * r - 94 * g - 18 * b + 128) >> 8)).clamp(16, 240) as u8;
        }
    }
    Some(out)
}
