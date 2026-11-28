//! Sharing the screen, on Windows: the system's `GraphicsCapturePicker` asks
//! what to share (a monitor or a window), then Windows Graphics Capture
//! streams it via `windows-capture`, scaled to the chosen quality preset and
//! converted from BGRA8 to video-range BT.601 I420 for the VP8 encoder.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use horo_rtc::video::Frame;
use tokio::sync::oneshot;
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame as CaptureFrame,
    graphics_capture_api::InternalCaptureControl,
    graphics_capture_picker::{Error as PickerError, GraphicsCapturePicker},
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
};

const WM_QUIT: u32 = 0x0012;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn PostThreadMessageW(id_thread: u32, msg: u32, wparam: usize, lparam: isize) -> i32;
}

/// Called with each picture, on the capture's thread.
pub type OnFrame = Box<dyn FnMut(Frame) + Send>;

type ReadyTx = Arc<Mutex<Option<oneshot::Sender<Result<bool, String>>>>>;

struct HandlerFlags {
    on_frame: OnFrame,
    max_size: (u32, u32),
    min_interval: Duration,
    stop: Arc<AtomicBool>,
    ready: ReadyTx,
}

struct ScreenHandler {
    on_frame: OnFrame,
    max_size: (u32, u32),
    min_interval: Duration,
    stop: Arc<AtomicBool>,
    last: Option<Instant>,
}

impl GraphicsCaptureApiHandler for ScreenHandler {
    type Flags = HandlerFlags;
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let f = ctx.flags;
        if let Some(tx) = f.ready.lock().ok().and_then(|mut s| s.take()) {
            let _ = tx.send(Ok(true));
        }
        Ok(Self {
            on_frame: f.on_frame,
            max_size: f.max_size,
            min_interval: f.min_interval,
            stop: f.stop,
            last: None,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut CaptureFrame,
        capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if self.stop.load(Ordering::Relaxed) {
            capture_control.stop();
            return Ok(());
        }
        let now = Instant::now();
        if self.last.is_some_and(|at| now.duration_since(at) < self.min_interval) {
            return Ok(());
        }
        let mut buf = frame.buffer().map_err(|e| e.to_string())?;
        let (w, h, row_pitch) = (buf.width() as usize, buf.height() as usize, buf.row_pitch() as usize);
        let raw = buf.as_raw_buffer();
        if let Some(i420) = bgra_to_i420(raw, w, h, row_pitch, self.max_size) {
            self.last = Some(now);
            (self.on_frame)(i420);
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// What's being shared. Dropping it stops capture and removes the Windows
/// capture border.
pub struct ScreenShare {
    stop: Arc<AtomicBool>,
    thread_id: Arc<AtomicU32>,
}

impl Drop for ScreenShare {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let tid = self.thread_id.load(Ordering::Relaxed);
        if tid != 0 {
            // Wake the capture thread's message loop immediately even if the
            // shared window isn't redrawing.
            unsafe {
                PostThreadMessageW(tid, WM_QUIT, 0, 0);
            }
        }
    }
}

impl ScreenShare {
    /// Ask Windows' Graphics Capture Picker what to share and start sending
    /// it, `on_frame` hearing each picture. `Ok(None)` if the picker was
    /// cancelled. Waits for the user: run it off the UI thread.
    pub async fn start(quality: crate::prefs::ScreenQuality, on_frame: OnFrame) -> Result<Option<ScreenShare>, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_id = Arc::new(AtomicU32::new(0));
        let (ready_tx, ready_rx) = oneshot::channel::<Result<bool, String>>();
        let ready: ReadyTx = Arc::new(Mutex::new(Some(ready_tx)));
        let stop_bg = stop.clone();
        let tid_bg = thread_id.clone();
        let max_size = quality.max_size();
        let min_interval = Duration::from_millis((1000 / quality.fps().max(1)).saturating_sub(2) as u64);

        std::thread::Builder::new()
            .name("horo screen".into())
            .spawn(move || {
                let send_ready = |res: Result<bool, String>| {
                    if let Some(tx) = ready.lock().ok().and_then(|mut s| s.take()) {
                        let _ = tx.send(res);
                    }
                };
                let item = match GraphicsCapturePicker::pick_item() {
                    Ok(Some(item)) => item,
                    Ok(None) | Err(PickerError::Canceled) => {
                        send_ready(Ok(false));
                        return;
                    }
                    Err(e) => {
                        send_ready(Err(format!("couldn't open the screen picker: {e}")));
                        return;
                    }
                };
                // Record our Win32 thread id so Drop can post WM_QUIT to exit
                // the capture message loop promptly.
                tid_bg.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
                let flags = HandlerFlags {
                    on_frame,
                    max_size,
                    min_interval,
                    stop: stop_bg,
                    ready: ready.clone(),
                };
                let settings = Settings::new(
                    item,
                    CursorCaptureSettings::Default,
                    DrawBorderSettings::Default,
                    SecondaryWindowSettings::Default,
                    MinimumUpdateIntervalSettings::Default,
                    DirtyRegionSettings::Default,
                    ColorFormat::Bgra8,
                    flags,
                );
                let result = ScreenHandler::start(settings);
                tid_bg.store(0, Ordering::Relaxed);
                if let Err(e) = result {
                    send_ready(Err(format!("couldn't share the screen: {e}")));
                }
            })
            .map_err(|e| format!("couldn't start a screen-share thread: {e}"))?;

        match ready_rx.await {
            Ok(Ok(true)) => Ok(Some(ScreenShare { stop, thread_id })),
            Ok(Ok(false)) => Ok(None),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("couldn't start screen sharing".into()),
        }
    }
}

/// Convert strided BGRA8 (`w × h`, `row_pitch` bytes per row) to video-range
/// BT.601 I420, scaled down to fit `max` in even dimensions.
fn bgra_to_i420(bgra: &[u8], w: usize, h: usize, row_pitch: usize, max: (u32, u32)) -> Option<Frame> {
    if w == 0 || h == 0 || row_pitch < w * 4 || bgra.len() < (h - 1) * row_pitch + w * 4 {
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
        let at = sy * row_pitch + sx * 4;
        (i32::from(bgra[at + 2]), i32::from(bgra[at + 1]), i32::from(bgra[at]))
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
