//! Sharing the screen, on macOS: a ScreenCaptureKit stream of the main
//! display, scaled to fit 1080p, at up to 30 pictures a second, in the
//! camera's format (bi-planar 4:2:0, full range), so the same conversion
//! and encoder take it from there.
//!
//! The first time, macOS asks the user to allow screen recording; until
//! they have, starting fails and says so.

use std::{sync::mpsc, time::Duration};

use dispatch2::DispatchQueue;
use objc2::{AnyThread, DefinedClass, define_class, msg_send, rc::Retained, runtime::{NSObject, NSObjectProtocol, ProtocolObject}};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{CVPixelBuffer, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType};

/// Called with each picture, on the capture queue.
pub type OnFrame = Box<dyn Fn(&CVPixelBuffer) + Send + Sync>;

/// The biggest picture sent: a screen is scaled down to fit.
const MAX_SIZE: (f64, f64) = (1920., 1080.);

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "HoroScreenSink"]
    #[ivars = OnFrame]
    struct Sink;

    unsafe impl NSObjectProtocol for Sink {}

    unsafe impl SCStreamOutput for Sink {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind != SCStreamOutputType::Screen {
                return;
            }
            // A screen that hasn't changed sends buffers with no picture.
            // SAFETY: a plain accessor on the buffer we were handed.
            if let Some(image) = unsafe { sample.image_buffer() } {
                (self.ivars())(&image);
            }
        }
    }
);

impl Sink {
    fn new(on_frame: OnFrame) -> Retained<Sink> {
        let this = Sink::alloc().set_ivars(on_frame);
        // SAFETY: NSObject's init, on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

fn describe(error: *mut NSError) -> String {
    // SAFETY: a non-null error ScreenCaptureKit handed over.
    unsafe { error.as_ref().map(|e| e.localizedDescription().to_string()).unwrap_or_else(|| "unknown error".into()) }
}

/// The main display being captured. Dropping it stops.
pub struct ScreenShare {
    stream: Retained<SCStream>,
    _sink: Retained<Sink>,
}

// SAFETY: an SCStream is driven from any thread (its callbacks come on
// its own queues); this one is started off the main thread and then only
// dropped.
unsafe impl Send for ScreenShare {}

impl ScreenShare {
    /// Start sharing the main display, `on_frame` hearing each picture.
    /// Blocks while macOS answers (run it off the UI thread).
    pub fn start(on_frame: OnFrame) -> Result<ScreenShare, String> {
        // What there is to share.
        let (tx, rx) = mpsc::channel();
        let block = block2::RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
            // SAFETY: a valid content object or null, per the API.
            let found = unsafe { content.as_ref() }.map(|c| unsafe { Retained::retain(c as *const _ as *mut SCShareableContent) });
            let _ = tx.send(match found.flatten() {
                Some(content) => Ok(content),
                None => Err(describe(error)),
            });
        });
        // SAFETY: a class method with a valid block, which it copies.
        unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
        let content = rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "macOS didn't say what can be shared".to_owned())?
            .map_err(|e| format!("can't see the screen ({e}) — allow screen recording for horo in system settings › privacy & security"))?;

        // SAFETY: AVFoundation-style calls with valid, retained arguments.
        unsafe {
            let display = content.displays().firstObject().ok_or("no display to share")?;
            let filter = SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &NSArray::new());
            let config = SCStreamConfiguration::new();
            // The display's size in pixels (points × 2 on a Retina screen),
            // fitted into MAX_SIZE, in even numbers as 4:2:0 wants.
            let (w, h) = (display.width() as f64 * 2., display.height() as f64 * 2.);
            let scale = (MAX_SIZE.0 / w).min(MAX_SIZE.1 / h).min(1.);
            let even = |v: f64| ((v * scale) as usize) & !1;
            config.setWidth(even(w).max(2));
            config.setHeight(even(h).max(2));
            config.setMinimumFrameInterval(CMTime::new(1, 30));
            config.setPixelFormat(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange);
            config.setShowsCursor(true);
            config.setQueueDepth(5);

            let stream = SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, None);
            let sink = Sink::new(on_frame);
            let queue = DispatchQueue::new("horo.screen", None);
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(ProtocolObject::from_ref(&*sink), SCStreamOutputType::Screen, Some(&queue))
                .map_err(|e| e.localizedDescription().to_string())?;

            let (tx, rx) = mpsc::channel();
            let started = block2::RcBlock::new(move |error: *mut NSError| {
                let _ = tx.send(if error.is_null() { Ok(()) } else { Err(describe(error)) });
            });
            stream.startCaptureWithCompletionHandler(Some(&started));
            rx.recv_timeout(Duration::from_secs(10))
                .map_err(|_| "the screen didn't start".to_owned())?
                .map_err(|e| format!("couldn't share the screen: {e}"))?;
            Ok(ScreenShare { stream, _sink: sink })
        }
    }
}

impl Drop for ScreenShare {
    fn drop(&mut self) {
        // SAFETY: a plain call; nothing to hear back.
        unsafe { self.stream.stopCaptureWithCompletionHandler(None) };
    }
}
