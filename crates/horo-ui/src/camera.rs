//! The camera, on macOS: an `AVCaptureSession` from the chosen camera
//! into an `AVCaptureVideoDataOutput`, which hands each frame to a small
//! Objective-C delegate on its own queue. The newest frame is kept, in
//! the format gpui's `surface` element draws (bi-planar 4:2:0, full
//! range), for the view to pick up on its next redraw.
//!
//! For now it's the settings preview; video calls will take frames from
//! the same place.

use std::sync::{Arc, Mutex};

use dispatch2::DispatchQueue;
use objc2::{AnyThread, DefinedClass, define_class, msg_send, rc::Retained, runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject}};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceDiscoverySession, AVCaptureDeviceInput, AVCaptureDevicePosition,
    AVCaptureDeviceTypeBuiltInWideAngleCamera, AVCaptureDeviceTypeContinuityCamera, AVCaptureDeviceTypeExternal, AVCaptureOutput, AVCaptureSession,
    AVCaptureVideoDataOutput, AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeVideo,
};
use objc2_core_foundation::{CFRetained, CFString};
use objc2_core_media::CMSampleBuffer;
use objc2_core_video::{CVPixelBuffer, kCVPixelBufferMetalCompatibilityKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

/// A camera the system offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraInfo {
    pub id: String,
    pub name: String,
    /// The one the system would pick.
    pub is_default: bool,
}

/// A frame, moved from the capture queue to the main thread. A pixel
/// buffer is immutable once captured, so sharing it is safe.
struct Frame(CFRetained<CVPixelBuffer>);
// SAFETY: see above; CoreVideo buffers are reference counted atomically.
unsafe impl Send for Frame {}

type Latest = Arc<Mutex<Option<Frame>>>;

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "HoroCameraSink"]
    #[ivars = Latest]
    struct Sink;

    unsafe impl NSObjectProtocol for Sink {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for Sink {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output(&self, _output: &AVCaptureOutput, sample: &CMSampleBuffer, _connection: &AVCaptureConnection) {
            // SAFETY: a plain accessor on the buffer we were handed.
            if let Some(image) = unsafe { sample.image_buffer() }
                && let Ok(mut latest) = self.ivars().lock()
            {
                *latest = Some(Frame(image));
            }
        }
    }
);

impl Sink {
    fn new(latest: Latest) -> Retained<Sink> {
        let this = Sink::alloc().set_ivars(latest);
        // SAFETY: NSObject's init, on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

fn ns(key: &CFString) -> &NSString {
    // SAFETY: CFString and NSString are toll-free bridged.
    unsafe { &*(key as *const CFString as *const NSString) }
}

fn discovery() -> Retained<AVCaptureDeviceDiscoverySession> {
    // SAFETY: framework constants and a class method with valid arguments.
    unsafe {
        let types = NSArray::from_slice(&[AVCaptureDeviceTypeBuiltInWideAngleCamera, AVCaptureDeviceTypeExternal, AVCaptureDeviceTypeContinuityCamera]);
        AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(&types, AVMediaTypeVideo, AVCaptureDevicePosition::Unspecified)
    }
}

/// The cameras plugged in now, the system's preferred one first.
pub fn cameras() -> Vec<CameraInfo> {
    // SAFETY: plain queries.
    unsafe {
        let default = AVMediaTypeVideo.and_then(|v| AVCaptureDevice::defaultDeviceWithMediaType(v)).map(|d| d.uniqueID().to_string());
        discovery()
            .devices()
            .iter()
            .map(|d| {
                let id = d.uniqueID().to_string();
                CameraInfo { is_default: default.as_ref() == Some(&id), id, name: d.localizedName().to_string() }
            })
            .collect()
    }
}

/// Whether we may use the camera: `Some(false)` refused, `None` not
/// asked yet (opening it asks).
pub fn allowed() -> Option<bool> {
    let Some(video) = (unsafe { AVMediaTypeVideo }) else { return Some(false) };
    // SAFETY: a plain class query.
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(video) } {
        AVAuthorizationStatus::Authorized => Some(true),
        AVAuthorizationStatus::NotDetermined => None,
        _ => Some(false),
    }
}

/// Ask for the camera; `done` hears the answer (on some other thread).
pub fn ask(done: impl Fn(bool) + 'static) {
    let Some(video) = (unsafe { AVMediaTypeVideo }) else { return done(false) };
    let block = block2::RcBlock::new(move |granted: objc2::runtime::Bool| done(granted.as_bool()));
    // SAFETY: a class method with a valid block, which AVFoundation copies.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(video, &block) };
}

/// A camera, running.
pub struct Camera {
    session: Retained<AVCaptureSession>,
    latest: Latest,
    /// Kept alive for as long as the output holds it weakly.
    _sink: Retained<Sink>,
    frame: Option<CFRetained<CVPixelBuffer>>,
}

// SAFETY: a capture session may be configured and stopped from any thread
// (Apple recommends doing it off the main one), and the frame slot is
// behind a mutex; the camera is opened on a background thread and then
// only used from the main one.
unsafe impl Send for Camera {}

impl Camera {
    /// Start `id` (the system's preferred camera when `None`, or when
    /// that one isn't plugged in).
    pub fn start(id: Option<&str>) -> Result<Camera, String> {
        // SAFETY: AVFoundation calls with valid, retained arguments.
        unsafe {
            let devices = discovery().devices();
            let device = id
                .and_then(|id| devices.iter().find(|d| d.uniqueID().to_string() == id))
                .or_else(|| AVMediaTypeVideo.and_then(|v| AVCaptureDevice::defaultDeviceWithMediaType(v)))
                .or_else(|| devices.iter().next())
                .ok_or("no camera found")?;
            let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&device).map_err(|e| e.localizedDescription().to_string())?;
            let session = AVCaptureSession::new();
            if !session.canAddInput(&input) {
                return Err("the camera is busy".into());
            }
            session.addInput(&input);

            let output = AVCaptureVideoDataOutput::new();
            let format = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange);
            let metal = NSNumber::new_bool(true);
            let settings: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
                &[ns(kCVPixelBufferPixelFormatTypeKey), ns(kCVPixelBufferMetalCompatibilityKey)],
                &[format.as_ref() as &AnyObject, metal.as_ref() as &AnyObject],
            );
            output.setVideoSettings(Some(&settings));
            output.setAlwaysDiscardsLateVideoFrames(true);
            let latest: Latest = Arc::default();
            let sink = Sink::new(latest.clone());
            let queue = DispatchQueue::new("horo.camera", None);
            output.setSampleBufferDelegate_queue(Some(ProtocolObject::from_ref(&*sink)), Some(&queue));
            if !session.canAddOutput(&output) {
                return Err("the camera can't be read".into());
            }
            session.addOutput(&output);
            session.startRunning();
            Ok(Camera { session, latest, _sink: sink, frame: None })
        }
    }

    /// The newest frame, or the last one shown.
    pub fn frame(&mut self) -> Option<CFRetained<CVPixelBuffer>> {
        if let Ok(mut latest) = self.latest.lock()
            && let Some(Frame(frame)) = latest.take()
        {
            self.frame = Some(frame);
        }
        self.frame.clone()
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        // SAFETY: a plain call; the light goes off with the view.
        unsafe { self.session.stopRunning() };
    }
}
