//! Pictures from PipeWire, on Linux: a camera (one of PipeWire's video
//! sources) or a shared screen (a stream on the PipeWire remote the desktop
//! portal hands over). Each runs a PipeWire loop on a thread of its own and
//! hands every picture over as the I420 frame the VP8 encoder takes, in the
//! video range (16–235) receivers expect, whatever raw format the source
//! offered.
//!
//! Only raw formats are asked for: a camera that only does MJPEG or H.264
//! at some size is offered at the sizes it does raw.

use std::{
    os::fd::OwnedFd,
    sync::mpsc,
    time::{Duration, Instant},
};

use horo_rtc::video::Frame;
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::param::video::VideoFormat;

/// Where the pictures come from.
pub enum Source {
    /// A camera by its PipeWire node name, or (`None`) the one the session
    /// manager picks.
    Camera(Option<String>),
    /// A portal's screen cast: its PipeWire remote and the stream's node.
    Screen { remote: OwnedFd, node: u32 },
}

/// Called with each picture, on the capture's thread.
pub type OnFrame = Box<dyn FnMut(Frame) + Send>;

/// The fastest we take pictures: a call sends 30 a second at most, so a
/// 60 Hz screen's other half would only be converted to be dropped.
const MIN_INTERVAL: Duration = Duration::from_millis(30);

/// A capture running. Dropping it stops it.
pub struct Capture {
    quit: pw::channel::Sender<()>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.quit.send(());
    }
}

/// Start capturing `source`, fitted into `max` (in even numbers, as 4:2:0
/// wants). Returns once the stream runs, or with why it didn't.
pub fn start(source: Source, max: (u32, u32), on_frame: OnFrame) -> Result<Capture, String> {
    let (quit_tx, quit_rx) = pw::channel::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let name = match &source {
        Source::Camera(_) => "horo camera",
        Source::Screen { .. } => "horo screen",
    };
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            if let Err(e) = run(source, max, on_frame, quit_rx, ready_tx.clone()) {
                let _ = ready_tx.send(Err(e));
            }
        })
        .map_err(|e| format!("couldn't start a capture thread: {e}"))?;
    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(Capture { quit: quit_tx }),
        Ok(Err(e)) => {
            let _ = quit_tx.send(());
            Err(e)
        }
        Err(_) => {
            let _ = quit_tx.send(());
            Err("it didn't start sending pictures".into())
        }
    }
}

/// What the stream's callbacks share.
struct State {
    format: spa::param::video::VideoInfoRaw,
    max: (u32, u32),
    on_frame: OnFrame,
    /// Told once the stream runs (or fails first).
    ready: Option<mpsc::Sender<Result<(), String>>>,
    last: Option<Instant>,
    /// A buffer we couldn't read was already logged.
    warned: bool,
}

fn run(source: Source, max: (u32, u32), on_frame: OnFrame, quit: pw::channel::Receiver<()>, ready: mpsc::Sender<Result<(), String>>) -> Result<(), String> {
    pw::init();
    let fail = |what: &str, e: pw::Error| format!("{what}: {e}");
    let mainloop = pw::main_loop::MainLoop::new(None).map_err(|e| fail("PipeWire", e))?;
    let context = pw::context::Context::new(&mainloop).map_err(|e| fail("PipeWire", e))?;
    let (core, role, target, formats) = match source {
        Source::Camera(name) => {
            let core = context.connect(None).map_err(|e| fail("couldn't reach PipeWire", e))?;
            (core, "Camera", name.map(Target::Name), CAMERA_FORMATS)
        }
        Source::Screen { remote, node } => {
            let core = context.connect_fd(remote, None).map_err(|e| fail("couldn't reach the shared screen", e))?;
            (core, "Screen", Some(Target::Node(node)), SCREEN_FORMATS)
        }
    };

    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Video",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => role,
    };
    if let Some(Target::Name(name)) = &target {
        props.insert("target.object", name.as_str());
    }
    let stream = pw::stream::Stream::new(&core, "horo", props).map_err(|e| fail("couldn't open a stream", e))?;

    let state = State { format: Default::default(), max, on_frame, ready: Some(ready), last: None, warned: false };
    let quitter = mainloop.clone();
    let _listener = stream
        .add_local_listener_with_user_data(state)
        .state_changed(move |_, state, _, new| match new {
            pw::stream::StreamState::Streaming => {
                if let Some(ready) = state.ready.take() {
                    let _ = ready.send(Ok(()));
                }
            }
            pw::stream::StreamState::Error(e) => {
                tracing::warn!("video capture: {e}");
                if let Some(ready) = state.ready.take() {
                    let _ = ready.send(Err(e));
                }
                quitter.quit();
            }
            _ => {}
        })
        .param_changed(|stream, state, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((kind, subtype)) = spa::param::format_utils::parse_format(param) else { return };
            if kind != spa::param::format::MediaType::Video || subtype != spa::param::format::MediaSubtype::Raw {
                return;
            }
            if state.format.parse(param).is_ok() {
                let size = state.format.size();
                tracing::info!("video capture: {:?} {}x{}", state.format.format(), size.width, size.height);
            }
            // Which buffers we can read: mapped memory, which the source
            // allocates. Left unsaid, a V4L2 camera is handed buffers it
            // can't use and the stream fails ("error use input buffers").
            let buffers = buffers_pod();
            if let Some(pod) = spa::pod::Pod::from_bytes(&buffers)
                && let Err(e) = stream.update_params(&mut [pod])
            {
                tracing::warn!("video capture: couldn't set the buffers: {e}");
            }
        })
        .process(|stream, state| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let now = Instant::now();
            if state.last.is_some_and(|at| now.duration_since(at) < MIN_INTERVAL) {
                return;
            }
            let size = state.format.size();
            let (w, h) = (size.width as usize, size.height as usize);
            if w == 0 || h == 0 {
                return;
            }
            let planes: Vec<Plane<'_>> = buffer
                .datas_mut()
                .iter_mut()
                .filter_map(|data| {
                    let chunk = data.chunk();
                    let (offset, stride, length) = (chunk.offset() as usize, chunk.stride().max(0) as usize, chunk.size() as usize);
                    let bytes = data.data()?;
                    let end = if length == 0 { bytes.len() } else { (offset + length).min(bytes.len()) };
                    Some(Plane { bytes: bytes.get(offset..end)?, stride })
                })
                .collect();
            let Some(frame) = convert(state.format.format(), &planes, w, h, state.max) else {
                if !state.warned {
                    state.warned = true;
                    tracing::warn!("video capture: couldn't read a {:?} picture ({} planes)", state.format.format(), planes.len());
                }
                return;
            };
            state.last = Some(now);
            (state.on_frame)(frame);
        })
        .register()
        .map_err(|e| fail("couldn't listen to the stream", e))?;

    // A camera is asked for no more than `max`; a screen comes at its own
    // size (the portal won't scale it) and is shrunk as it's converted.
    let default = spa::utils::Rectangle { width: max.0, height: max.1 };
    let ceiling = if role == "Camera" { default } else { spa::utils::Rectangle { width: 8192, height: 8192 } };
    let format = format_pod(formats, default, ceiling);
    let mut params = [spa::pod::Pod::from_bytes(&format).ok_or("couldn't describe the formats")?];
    let node = match target {
        Some(Target::Node(node)) => Some(node),
        _ => None,
    };
    stream
        .connect(spa::utils::Direction::Input, node, pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS, &mut params)
        .map_err(|e| fail("couldn't connect the stream", e))?;

    let _quit = quit.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |()| mainloop.quit()
    });
    mainloop.run();
    Ok(())
}

enum Target {
    Name(String),
    Node(u32),
}

/// What a camera is asked for, best first: the 4:2:0 ones need no
/// conversion but the range.
const CAMERA_FORMATS: &[VideoFormat] = &[VideoFormat::NV12, VideoFormat::I420, VideoFormat::YUY2, VideoFormat::BGRx, VideoFormat::RGBx];

/// What a screen is asked for: compositors hand out RGB.
const SCREEN_FORMATS: &[VideoFormat] = &[
    VideoFormat::BGRx,
    VideoFormat::BGRA,
    VideoFormat::RGBx,
    VideoFormat::RGBA,
    VideoFormat::xRGB,
    VideoFormat::ARGB,
    VideoFormat::xBGR,
    VideoFormat::ABGR,
    VideoFormat::NV12,
    VideoFormat::I420,
];

/// The EnumFormat parameter: raw video in one of `formats`, `default` in
/// size (up to `max`), about 30 pictures a second.
fn format_pod(formats: &[VideoFormat], default: spa::utils::Rectangle, max: spa::utils::Rectangle) -> Vec<u8> {
    use spa::{
        param::format::{FormatProperties, MediaSubtype, MediaType},
        pod::{ChoiceValue, Object, Property, PropertyFlags, Value, serialize::PodSerializer},
        utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Id, Rectangle, SpaTypes},
    };
    let id = |value: u32| Value::Id(Id(value));
    let property = |key: FormatProperties, value: Value| Property { key: key.as_raw(), flags: PropertyFlags::empty(), value };
    let object = Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: vec![
            property(FormatProperties::MediaType, id(MediaType::Video.as_raw())),
            property(FormatProperties::MediaSubtype, id(MediaSubtype::Raw.as_raw())),
            property(
                FormatProperties::VideoFormat,
                Value::Choice(ChoiceValue::Id(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Enum { default: Id(formats[0].as_raw()), alternatives: formats.iter().map(|f| Id(f.as_raw())).collect() },
                ))),
            ),
            property(
                FormatProperties::VideoSize,
                Value::Choice(ChoiceValue::Rectangle(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range { default, min: Rectangle { width: 1, height: 1 }, max },
                ))),
            ),
            property(
                FormatProperties::VideoFramerate,
                Value::Choice(ChoiceValue::Fraction(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range { default: Fraction { num: 30, denom: 1 }, min: Fraction { num: 0, denom: 1 }, max: Fraction { num: 60, denom: 1 } },
                ))),
            ),
        ],
    };
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object)).map(|(cursor, _)| cursor.into_inner()).unwrap_or_default()
}

/// The Buffers parameter: a few buffers, in memory we can map (a pointer
/// or a memfd), however many planes and bytes the source wants.
fn buffers_pod() -> Vec<u8> {
    use spa::{
        pod::{ChoiceValue, Object, Property, PropertyFlags, Value, serialize::PodSerializer},
        sys,
        utils::{Choice, ChoiceEnum, ChoiceFlags, SpaTypes},
    };
    let property = |key: u32, value: Value| Property { key, flags: PropertyFlags::empty(), value };
    let mappable = (1 << sys::SPA_DATA_MemPtr) | (1 << sys::SPA_DATA_MemFd);
    let object = Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![
            property(sys::SPA_PARAM_BUFFERS_buffers, Value::Choice(ChoiceValue::Int(Choice(ChoiceFlags::empty(), ChoiceEnum::Range { default: 8, min: 2, max: 16 })))),
            property(sys::SPA_PARAM_BUFFERS_dataType, Value::Choice(ChoiceValue::Int(Choice(ChoiceFlags::empty(), ChoiceEnum::Flags { default: mappable, flags: vec![] })))),
        ],
    };
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object)).map(|(cursor, _)| cursor.into_inner()).unwrap_or_default()
}

/// One plane of a picture as PipeWire handed it: its bytes from its
/// offset, and its stride (0 when the producer didn't say).
pub struct Plane<'a> {
    pub bytes: &'a [u8],
    pub stride: usize,
}

/// The planes of a 4:2:0 or 4:2:2 picture: separate buffers, or (as V4L2
/// hands them) one after another in a single one.
fn split<'a>(planes: &'a [Plane<'a>], count: usize, strides: &[usize], heights: &[usize]) -> Option<Vec<(&'a [u8], usize)>> {
    if planes.len() >= count {
        return Some(planes[..count].iter().zip(strides).map(|(p, &s)| (p.bytes, if p.stride == 0 { s } else { p.stride })).collect());
    }
    let first = planes.first()?;
    // One buffer: the stride it gives is the luma's, the rest scale with it.
    let luma = if first.stride == 0 { strides[0] } else { first.stride };
    let mut out = Vec::with_capacity(count);
    let mut at = 0;
    for i in 0..count {
        let stride = luma * strides[i] / strides[0];
        let length = stride * heights[i];
        out.push((first.bytes.get(at..at + length)?, stride));
        at += length;
    }
    Some(out)
}

/// A raw picture of `w × h` as an I420 frame, scaled down to fit `max`.
pub fn convert(format: VideoFormat, planes: &[Plane<'_>], w: usize, h: usize, max: (u32, u32)) -> Option<Frame> {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let (ow, oh) = fit((w, h), max);
    let mut out = Frame::black(ow as u32, oh as u32);
    let (ocw, och) = (ow.div_ceil(2), oh.div_ceil(2));
    {
        let (y, rest) = out.data.split_at_mut(ow * oh);
        let (u, v) = rest.split_at_mut(ocw * och);
        // Which source pixel each output one samples (a box shrink would be
        // kinder to text, but a screen's usually no bigger than what's sent).
        let xs: Vec<usize> = (0..ow).map(|x| x * w / ow).collect();
        let ys: Vec<usize> = (0..oh).map(|y| y * h / oh).collect();
        let cxs: Vec<usize> = (0..ocw).map(|x| (x * 2 * w / ow).min(w - 1)).collect();
        let cys: Vec<usize> = (0..och).map(|y| (y * 2 * h / oh).min(h - 1)).collect();
        match format {
            VideoFormat::I420 | VideoFormat::NV12 => {
                let nv12 = format == VideoFormat::NV12;
                let planes = if nv12 { split(planes, 2, &[w, w], &[h, ch])? } else { split(planes, 3, &[w, cw, cw], &[h, ch, ch])? };
                let (yp, ys_) = planes[0];
                for (row, &sy) in ys.iter().enumerate() {
                    let line = yp.get(sy * ys_..sy * ys_ + w)?;
                    for (col, &sx) in xs.iter().enumerate() {
                        y[row * ow + col] = line[sx];
                    }
                }
                for (row, &sy) in cys.iter().enumerate() {
                    let sy = sy / 2;
                    for (col, &sx) in cxs.iter().enumerate() {
                        let sx = sx / 2;
                        let (pu, pv) = if nv12 {
                            let (uv, s) = planes[1];
                            (*uv.get(sy * s + sx * 2)?, *uv.get(sy * s + sx * 2 + 1)?)
                        } else {
                            let ((up, us), (vp, vs)) = (planes[1], planes[2]);
                            (*up.get(sy * us + sx)?, *vp.get(sy * vs + sx)?)
                        };
                        u[row * ocw + col] = pu;
                        v[row * ocw + col] = pv;
                    }
                }
            }
            VideoFormat::YUY2 => {
                let first = planes.first()?;
                let stride = if first.stride == 0 { w * 2 } else { first.stride };
                let px = first.bytes;
                for (row, &sy) in ys.iter().enumerate() {
                    for (col, &sx) in xs.iter().enumerate() {
                        y[row * ow + col] = *px.get(sy * stride + sx * 2)?;
                    }
                }
                for (row, &sy) in cys.iter().enumerate() {
                    for (col, &sx) in cxs.iter().enumerate() {
                        let pair = sy * stride + (sx & !1) * 2;
                        u[row * ocw + col] = *px.get(pair + 1)?;
                        v[row * ocw + col] = *px.get(pair + 3)?;
                    }
                }
            }
            _ => {
                // Packed RGB: where R, G and B sit in each 4-byte pixel.
                let (r, g, b) = match format {
                    VideoFormat::BGRx | VideoFormat::BGRA => (2, 1, 0),
                    VideoFormat::RGBx | VideoFormat::RGBA => (0, 1, 2),
                    VideoFormat::xRGB | VideoFormat::ARGB => (1, 2, 3),
                    VideoFormat::xBGR | VideoFormat::ABGR => (3, 2, 1),
                    _ => return None,
                };
                let first = planes.first()?;
                let stride = if first.stride == 0 { w * 4 } else { first.stride };
                let px = first.bytes;
                let rgb = |sx: usize, sy: usize| -> Option<(i32, i32, i32)> {
                    let at = sy * stride + sx * 4;
                    let p = px.get(at..at + 4)?;
                    Some((i32::from(p[r]), i32::from(p[g]), i32::from(p[b])))
                };
                for (row, &sy) in ys.iter().enumerate() {
                    for (col, &sx) in xs.iter().enumerate() {
                        let (r, g, b) = rgb(sx, sy)?;
                        y[row * ow + col] = luma(r, g, b);
                    }
                }
                for (row, &sy) in cys.iter().enumerate() {
                    let sy1 = (sy + 1).min(h - 1);
                    for (col, &sx) in cxs.iter().enumerate() {
                        let sx1 = (sx + 1).min(w - 1);
                        // The four pixels the chroma sample covers, averaged.
                        let (mut sr, mut sg, mut sb) = (0, 0, 0);
                        for (x, y) in [(sx, sy), (sx1, sy), (sx, sy1), (sx1, sy1)] {
                            let (r, g, b) = rgb(x, y)?;
                            (sr, sg, sb) = (sr + r, sg + g, sb + b);
                        }
                        let (cu, cv) = chroma((sr + 2) / 4, (sg + 2) / 4, (sb + 2) / 4);
                        u[row * ocw + col] = cu;
                        v[row * ocw + col] = cv;
                    }
                }
            }
        }
    }
    Some(out)
}

/// `size` scaled down (never up) to fit `max`, in even numbers.
pub fn fit((w, h): (usize, usize), max: (u32, u32)) -> (usize, usize) {
    let scale = (max.0 as f64 / w as f64).min(max.1 as f64 / h as f64).min(1.);
    let even = |v: usize| ((v as f64 * scale) as usize & !1).max(2);
    (even(w), even(h))
}

/// BT.601 luma in the video range.
fn luma(r: i32, g: i32, b: i32) -> u8 {
    (16 + ((66 * r + 129 * g + 25 * b + 128) >> 8)).clamp(16, 235) as u8
}

/// BT.601 chroma (U, V) in the video range.
fn chroma(r: i32, g: i32, b: i32) -> (u8, u8) {
    let u = 128 + ((-38 * r - 74 * g + 112 * b + 128) >> 8);
    let v = 128 + ((112 * r - 94 * g - 18 * b + 128) >> 8);
    (u.clamp(16, 240) as u8, v.clamp(16, 240) as u8)
}

/// A camera PipeWire offers.
#[derive(Debug, Clone)]
pub struct CameraNode {
    /// Its node name: stable across restarts, and what a stream targets.
    pub name: String,
    pub description: String,
}

/// The cameras PipeWire has now, as its registry lists them.
pub fn cameras() -> Result<Vec<CameraNode>, String> {
    use std::{cell::RefCell, rc::Rc};
    pw::init();
    let fail = |e: pw::Error| format!("PipeWire: {e}");
    let mainloop = pw::main_loop::MainLoop::new(None).map_err(fail)?;
    let context = pw::context::Context::new(&mainloop).map_err(fail)?;
    let core = context.connect(None).map_err(fail)?;
    let registry = core.get_registry().map_err(fail)?;
    let found = Rc::new(RefCell::new(Vec::new()));
    let _registry = registry
        .add_listener_local()
        .global({
            let found = found.clone();
            move |global| {
                if global.type_ != pw::types::ObjectType::Node {
                    return;
                }
                let Some(props) = global.props else { return };
                let camera = props.get("media.class") == Some("Video/Source")
                    && (props.get("media.role") == Some("Camera") || props.get("api.v4l2.path").is_some() || props.get("api.libcamera.path").is_some());
                if let (true, Some(name)) = (camera, props.get("node.name")) {
                    let description = props.get("node.description").or(props.get("node.nick")).unwrap_or(name);
                    // "Integrated Camera (V4L2)": which API is no news to anyone.
                    let description = description.trim_end_matches(" (V4L2)").trim_end_matches(" (libcamera)").to_owned();
                    found.borrow_mut().push(CameraNode { name: name.to_owned(), description });
                }
            }
        })
        .register();
    // Everything that exists now has been announced once the server
    // answers a sync sent after asking.
    let pending = core.sync(0).map_err(fail)?;
    let _core = core
        .add_listener_local()
        .done({
            let mainloop = mainloop.clone();
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending {
                    mainloop.quit();
                }
            }
        })
        .register();
    mainloop.run();
    Ok(found.take())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_pictures_shrink_to_fit_in_even_numbers() {
        assert_eq!(fit((2560, 1440), (1920, 1080)), (1920, 1080));
        assert_eq!(fit((3840, 1600), (1920, 1080)), (1920, 800));
        assert_eq!(fit((1281, 721), (1920, 1080)), (1280, 720));
    }

    #[test]
    fn rgb_becomes_video_range_yuv() {
        // White, black and pure red, in BGRx.
        let mut px = Vec::new();
        for (b, g, r) in [(255u8, 255u8, 255u8), (0, 0, 0), (0, 0, 255), (0, 0, 255)] {
            px.extend_from_slice(&[b, g, r, 255]);
        }
        let frame = convert(VideoFormat::BGRx, &[Plane { bytes: &px, stride: 16 }], 4, 1, (1920, 1080)).unwrap();
        assert_eq!((frame.width, frame.height), (4, 2));
        assert_eq!(frame.y()[0], 235);
        assert_eq!(frame.y()[1], 16);
        assert_eq!(frame.y()[2], 82);
        // Red's chroma: V well above the middle.
        assert!(frame.v()[1] > 200);
    }

    #[test]
    fn nv12_in_one_buffer_is_split_into_its_planes() {
        let (w, h) = (4, 2);
        let mut bytes = vec![100u8; w * h];
        bytes.extend_from_slice(&[90, 160, 91, 161]);
        let frame = convert(VideoFormat::NV12, &[Plane { bytes: &bytes, stride: w }], w, h, (1920, 1080)).unwrap();
        assert!(frame.y().iter().all(|&v| v == 100));
        assert_eq!(frame.u(), &[90, 91]);
        assert_eq!(frame.v(), &[160, 161]);
    }

    #[test]
    #[ignore = "a timing, not a check"]
    fn how_long_pictures_take() {
        let (w, h) = (1280usize, 720usize);
        let nv12 = vec![128u8; w * h * 3 / 2];
        let start = Instant::now();
        for _ in 0..20 {
            convert(VideoFormat::NV12, &[Plane { bytes: &nv12, stride: w }], w, h, (1280, 720)).unwrap();
        }
        println!("NV12 720p: {:?} each", start.elapsed() / 20);
        let bgrx = vec![200u8; 2560 * 1440 * 4];
        let start = Instant::now();
        for _ in 0..5 {
            convert(VideoFormat::BGRx, &[Plane { bytes: &bgrx, stride: 2560 * 4 }], 2560, 1440, (1920, 1080)).unwrap();
        }
        println!("BGRx 1440p to 1080p: {:?} each", start.elapsed() / 5);
        let frame = Frame::black(1280, 720);
        let start = Instant::now();
        for _ in 0..20 {
            crate::picture::render(&frame, true).unwrap();
        }
        println!("I420 720p to BGRA: {:?} each", start.elapsed() / 20);
    }

    #[test]
    fn yuy2_keeps_its_luma_and_shares_chroma_in_pairs() {
        // Two pixels: Y0 U Y1 V.
        let bytes = [50u8, 110, 60, 140];
        let frame = convert(VideoFormat::YUY2, &[Plane { bytes: &bytes, stride: 4 }], 2, 1, (1920, 1080)).unwrap();
        assert_eq!(&frame.y()[..2], &[50, 60]);
        assert_eq!(frame.u(), &[110]);
        assert_eq!(frame.v(), &[140]);
    }
}
