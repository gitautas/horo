//! Sharing the screen, on macOS: [`pick`] shows the system's picker (a
//! window, an app or a whole display, as other apps' sharing does), and
//! [`ScreenShare`] streams what was picked with ScreenCaptureKit, scaled
//! to fit 1080p, at up to 30 pictures a second, in the camera's format
//! (bi-planar 4:2:0, full range), so the same conversion and encoder take
//! it from there.
//!
//! The first time, macOS asks the user to allow screen recording; until
//! they have, starting fails and says so.

use std::{
    sync::{Mutex, mpsc},
    time::Duration,
};

use dispatch2::DispatchQueue;
use objc2::{AnyThread, DefinedClass, define_class, msg_send, rc::Retained, runtime::{NSObject, NSObjectProtocol, ProtocolObject}};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{CVPixelBuffer, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange};
use objc2_foundation::NSError;
use objc2_screen_capture_kit::{
    SCContentFilter, SCContentSharingPicker, SCContentSharingPickerMode, SCContentSharingPickerObserver, SCStream, SCStreamConfiguration,
    SCStreamOutput, SCStreamOutputType,
};

/// Called with each picture, on the capture queue.
pub type OnFrame = Box<dyn Fn(&CVPixelBuffer) + Send + Sync>;

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

/// What the user picked to share.
pub struct Picked(Retained<SCContentFilter>);
// SAFETY: a content filter is an immutable description of what to
// capture, read by ScreenCaptureKit from any thread.
unsafe impl Send for Picked {}

/// The picker's answer: what to share, or `None` if they cancelled.
pub type PickResult = Result<Option<Picked>, String>;
type Answer = tokio::sync::oneshot::Sender<PickResult>;

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "HoroPickerObserver"]
    #[ivars = Mutex<Option<Answer>>]
    struct PickerObserver;

    unsafe impl NSObjectProtocol for PickerObserver {}

    unsafe impl SCContentSharingPickerObserver for PickerObserver {
        #[unsafe(method(contentSharingPicker:didCancelForStream:))]
        fn did_cancel(&self, _picker: &SCContentSharingPicker, _stream: Option<&SCStream>) {
            self.answer(Ok(None));
        }

        #[unsafe(method(contentSharingPicker:didUpdateWithFilter:forStream:))]
        fn did_pick(&self, _picker: &SCContentSharingPicker, filter: &SCContentFilter, _stream: Option<&SCStream>) {
            // SAFETY: retaining an object we were handed.
            let filter = unsafe { Retained::retain(filter as *const _ as *mut SCContentFilter) };
            self.answer(filter.map(|f| Some(Picked(f))).ok_or_else(|| "nothing was picked".to_owned()));
        }

        #[unsafe(method(contentSharingPickerStartDidFailWithError:))]
        fn did_fail(&self, error: &NSError) {
            self.answer(Err(error.localizedDescription().to_string()));
        }
    }
);

impl PickerObserver {
    /// Send the answer, once.
    fn answer(&self, result: PickResult) {
        if let Some(answer) = self.ivars().lock().ok().and_then(|mut a| a.take()) {
            let _ = answer.send(result);
        }
    }
}

/// The observer of the picker that's up, kept alive until it answers.
struct Observing(Retained<PickerObserver>);
// SAFETY: only touched under the mutex, to register and unregister it.
unsafe impl Send for Observing {}
static OBSERVING: Mutex<Option<Observing>> = Mutex::new(None);

/// Show the system's picker: a window, an app or a display. The answer
/// comes on the returned channel. On the main thread.
pub fn pick() -> tokio::sync::oneshot::Receiver<PickResult> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    // SAFETY: AppKit-side calls with valid, retained arguments, on the main
    // thread (where the UI calls this).
    unsafe {
        let picker = SCContentSharingPicker::sharedPicker();
        if let Ok(mut observing) = OBSERVING.lock()
            && let Some(old) = observing.take()
        {
            picker.removeObserver(ProtocolObject::from_ref(&*old.0));
        }
        let config = picker.defaultConfiguration();
        config.setAllowedPickerModes(SCContentSharingPickerMode::SingleWindow | SCContentSharingPickerMode::SingleApplication | SCContentSharingPickerMode::SingleDisplay);
        picker.setDefaultConfiguration(&config);
        let observer = PickerObserver::alloc().set_ivars(Mutex::new(Some(tx)));
        let observer: Retained<PickerObserver> = msg_send![super(observer), init];
        picker.addObserver(ProtocolObject::from_ref(&*observer));
        if let Ok(mut observing) = OBSERVING.lock() {
            *observing = Some(Observing(observer));
        }
        picker.setActive(true);
        picker.present();
    }
    rx
}

/// Put the picker away (sharing stopped, or never started).
pub fn done_picking() {
    // SAFETY: plain calls on the shared picker.
    unsafe {
        let picker = SCContentSharingPicker::sharedPicker();
        if let Ok(mut observing) = OBSERVING.lock()
            && let Some(old) = observing.take()
        {
            picker.removeObserver(ProtocolObject::from_ref(&*old.0));
        }
        picker.setActive(false);
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
    /// Start sharing what was picked, `on_frame` hearing each picture.
    /// Blocks while macOS answers (run it off the UI thread).
    pub fn start(picked: Picked, quality: crate::prefs::ScreenQuality, on_frame: OnFrame) -> Result<ScreenShare, String> {
        let filter = picked.0;
        // SAFETY: plain queries on the filter.
        let (w, h) = unsafe {
            let rect = filter.contentRect();
            let scale = f64::from(filter.pointPixelScale()).max(1.);
            (rect.size.width * scale, rect.size.height * scale)
        };
        Self::stream(&filter, (w, h), quality, on_frame)
    }

    /// Stream `filter`'s content, `(w, h)` pixels, fitted into `quality.max_size()`.
    fn stream(filter: &SCContentFilter, (w, h): (f64, f64), quality: crate::prefs::ScreenQuality, on_frame: OnFrame) -> Result<ScreenShare, String> {
        let (max_w, max_h) = quality.max_size();
        let fps = quality.fps() as i32;
        // SAFETY: ScreenCaptureKit calls with valid, retained arguments.
        unsafe {
            let config = SCStreamConfiguration::new();
            // In even numbers, as 4:2:0 wants.
            let scale = (f64::from(max_w) / w).min(f64::from(max_h) / h).min(1.);
            let even = |v: f64| ((v * scale) as usize) & !1;
            config.setWidth(even(w).max(2));
            config.setHeight(even(h).max(2));
            config.setMinimumFrameInterval(CMTime::new(1, fps));
            config.setPixelFormat(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange);
            config.setShowsCursor(true);
            config.setQueueDepth(5);

            let stream = SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), filter, &config, None);
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
