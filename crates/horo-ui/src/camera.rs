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
    AVCaptureDeviceTypeBuiltInWideAngleCamera, AVCaptureSessionPreset1280x720, AVCaptureDeviceTypeContinuityCamera, AVCaptureDeviceTypeExternal, AVCaptureOutput, AVCaptureSession,
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

/// Called with each picture, on the capture queue (a call's encoder).
pub type OnFrame = Box<dyn Fn(&CVPixelBuffer) + Send + Sync>;

/// What the delegate holds: the newest picture for the preview, and
/// whoever else wants every one.
struct SinkState {
    latest: Latest,
    on_frame: Option<OnFrame>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "HoroCameraSink"]
    #[ivars = SinkState]
    struct Sink;

    unsafe impl NSObjectProtocol for Sink {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for Sink {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output(&self, _output: &AVCaptureOutput, sample: &CMSampleBuffer, _connection: &AVCaptureConnection) {
            // SAFETY: a plain accessor on the buffer we were handed.
            let Some(image) = (unsafe { sample.image_buffer() }) else { return };
            if let Some(on_frame) = &self.ivars().on_frame {
                on_frame(&image);
            }
            if let Ok(mut latest) = self.ivars().latest.lock() {
                *latest = Some(Frame(image));
            }
        }
    }
);

impl Sink {
    fn new(latest: Latest, on_frame: Option<OnFrame>) -> Retained<Sink> {
        let this = Sink::alloc().set_ivars(SinkState { latest, on_frame });
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
    /// `frame`, flipped, for showing you yourself.
    mirrored: Option<CFRetained<CVPixelBuffer>>,
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
        Camera::start_with(id, None)
    }

    /// As [`Camera::start`], with `on_frame` hearing every picture.
    pub fn start_with(id: Option<&str>, on_frame: Option<OnFrame>) -> Result<Camera, String> {
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
            // 720p: plenty for a call, and what the encoder can keep up with.
            if session.canSetSessionPreset(AVCaptureSessionPreset1280x720) {
                session.setSessionPreset(AVCaptureSessionPreset1280x720);
            }

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
            let sink = Sink::new(latest.clone(), on_frame);
            let queue = DispatchQueue::new("horo.camera", None);
            output.setSampleBufferDelegate_queue(Some(ProtocolObject::from_ref(&*sink)), Some(&queue));
            if !session.canAddOutput(&output) {
                return Err("the camera can't be read".into());
            }
            session.addOutput(&output);
            session.startRunning();
            Ok(Camera { session, latest, _sink: sink, frame: None, mirrored: None })
        }
    }

    /// The newest frame, or the last one shown.
    pub fn frame(&mut self) -> Option<CFRetained<CVPixelBuffer>> {
        if let Ok(mut latest) = self.latest.lock()
            && let Some(Frame(frame)) = latest.take()
        {
            self.frame = Some(frame);
            self.mirrored = None;
        }
        self.frame.clone()
    }

    /// The newest frame as a mirror shows it: how people expect to see
    /// themselves (what's sent stays the right way round).
    pub fn mirrored_frame(&mut self) -> Option<CFRetained<CVPixelBuffer>> {
        let frame = self.frame()?;
        if self.mirrored.is_none() {
            self.mirrored = mirror(&frame);
        }
        self.mirrored.clone().or(Some(frame))
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        // SAFETY: a plain call; the light goes off with the view.
        unsafe { self.session.stopRunning() };
    }
}

/// A camera picture (bi-planar 4:2:0, full range) as the I420 frame the
/// VP8 encoder takes, in the video range (16–235) receivers expect.
pub fn to_i420(buffer: &CVPixelBuffer) -> Option<horo_rtc::video::Frame> {
    use objc2_core_video::{CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress};
    static LUMA: std::sync::OnceLock<[u8; 256]> = std::sync::OnceLock::new();
    static CHROMA: std::sync::OnceLock<[u8; 256]> = std::sync::OnceLock::new();
    let luma = LUMA.get_or_init(|| std::array::from_fn(|v| (16 + v * 219 / 255) as u8));
    let chroma = CHROMA.get_or_init(|| std::array::from_fn(|v| (128 + (v as i32 - 128) * 224 / 255) as u8));

    let (w, h) = (CVPixelBufferGetWidth(buffer), CVPixelBufferGetHeight(buffer));
    if w == 0 || h == 0 {
        return None;
    }
    let mut frame = horo_rtc::video::Frame::black(w as u32, h as u32);
    let (cw, ch) = horo_rtc::video::Frame::chroma_size(w as u32, h as u32);
    let (cw, ch) = (cw as usize, ch as usize);
    // SAFETY: reads stay inside each plane's rows as CoreVideo reports
    // them, between lock and unlock.
    unsafe {
        CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly);
        let y_base = CVPixelBufferGetBaseAddressOfPlane(buffer, 0) as *const u8;
        let uv_base = CVPixelBufferGetBaseAddressOfPlane(buffer, 1) as *const u8;
        let (y_stride, uv_stride) = (CVPixelBufferGetBytesPerRowOfPlane(buffer, 0), CVPixelBufferGetBytesPerRowOfPlane(buffer, 1));
        if !y_base.is_null() && !uv_base.is_null() {
            let (y, rest) = frame.data.split_at_mut(w * h);
            let (u, v) = rest.split_at_mut(cw * ch);
            for row in 0..h {
                let src = std::slice::from_raw_parts(y_base.add(row * y_stride), w);
                for (out, &value) in y[row * w..(row + 1) * w].iter_mut().zip(src) {
                    *out = luma[value as usize];
                }
            }
            for row in 0..ch {
                let src = std::slice::from_raw_parts(uv_base.add(row * uv_stride), cw * 2);
                for col in 0..cw {
                    u[row * cw + col] = chroma[src[col * 2] as usize];
                    v[row * cw + col] = chroma[src[col * 2 + 1] as usize];
                }
            }
        }
        CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly);
    }
    Some(frame)
}

/// A camera picture (bi-planar 4:2:0) flipped left to right.
fn mirror(source: &CVPixelBuffer) -> Option<CFRetained<CVPixelBuffer>> {
    use objc2_core_video::{
        CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane,
        CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVPixelBufferGetWidthOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress,
    };
    let (w, h) = (CVPixelBufferGetWidth(source), CVPixelBufferGetHeight(source));
    // SAFETY: CoreVideo calls on a buffer we were handed and one we create;
    // every read and write stays inside the planes' rows as CoreVideo
    // reports them, between lock and unlock.
    unsafe {
        let metal = NSNumber::new_bool(true);
        let attributes: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(&[ns(kCVPixelBufferMetalCompatibilityKey)], &[metal.as_ref() as &AnyObject]);
        let attrs = &*(Retained::as_ptr(&attributes) as *const objc2_core_foundation::CFDictionary);
        let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
        if CVPixelBufferCreate(None, w, h, CVPixelBufferGetPixelFormatType(source), Some(attrs), std::ptr::NonNull::new(&mut out)?) != 0 {
            return None;
        }
        let target = CFRetained::from_raw(std::ptr::NonNull::new(out)?);
        CVPixelBufferLockBaseAddress(source, CVPixelBufferLockFlags::ReadOnly);
        CVPixelBufferLockBaseAddress(&target, CVPixelBufferLockFlags(0));
        // Plane 0 is one byte a pixel; plane 1 two (U and V together).
        for (plane, bytes) in [(0, 1usize), (1, 2usize)] {
            let (from, to) = (CVPixelBufferGetBaseAddressOfPlane(source, plane) as *const u8, CVPixelBufferGetBaseAddressOfPlane(&target, plane) as *mut u8);
            if from.is_null() || to.is_null() {
                continue;
            }
            let (pw, ph) = (CVPixelBufferGetWidthOfPlane(source, plane), CVPixelBufferGetHeightOfPlane(source, plane));
            let (from_stride, to_stride) = (CVPixelBufferGetBytesPerRowOfPlane(source, plane), CVPixelBufferGetBytesPerRowOfPlane(&target, plane));
            for row in 0..ph {
                let src = std::slice::from_raw_parts(from.add(row * from_stride), pw * bytes);
                let dst = std::slice::from_raw_parts_mut(to.add(row * to_stride), pw * bytes);
                for (out, pixel) in dst.chunks_exact_mut(bytes).zip(src.chunks_exact(bytes).rev()) {
                    out.copy_from_slice(pixel);
                }
            }
        }
        CVPixelBufferUnlockBaseAddress(&target, CVPixelBufferLockFlags(0));
        CVPixelBufferUnlockBaseAddress(source, CVPixelBufferLockFlags::ReadOnly);
        Some(target)
    }
}
