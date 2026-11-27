//! Microphones and loudspeakers, as horo's audio path wants them: 48 kHz
//! mono in 20 ms frames, whatever the device does natively.
//!
//! [`Capture`] calls back with one frame at a time from a microphone;
//! [`Playback`] asks for one frame at a time for a loudspeaker. Each owns a
//! thread that holds the cpal stream and does the conversion, so the
//! real-time device callbacks only ever downmix (or duplicate) channels and
//! move samples through a lock-free ring. Nothing in them allocates, locks
//! or waits; when the ring is full or empty they count it and carry on.
//!
//! Backends come from cpal (CoreAudio, WASAPI, ALSA). Echo cancellation
//! isn't done here but in software on the frames, the same on every
//! platform ([`crate::audio::echo`]).

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};

use cpal::{
    FromSample, Sample, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use ringbuf::{
    HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};

mod convert;
// The audio path's shape, which the devices convert to and from.
pub use crate::audio::{FRAME, SAMPLE_RATE};
pub use convert::{FromFrames, ToFrames};

/// How often the conversion threads wake.
const POLL: Duration = Duration::from_millis(5);

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("no {0} device")]
    NoDevice(&'static str),
    #[error("no device with id {0:?}")]
    UnknownDevice(String),
    #[error("{0}")]
    Cpal(String),
    #[error("unsupported sample format {0:?}")]
    UnsupportedFormat(cpal::SampleFormat),
    #[error("resampler: {0}")]
    Resampler(String),
}

impl From<cpal::Error> for DeviceError {
    fn from(e: cpal::Error) -> Self {
        DeviceError::Cpal(e.to_string())
    }
}

pub type Result<T, E = DeviceError> = std::result::Result<T, E>;

/// A device as a settings screen lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable across runs where the platform allows; what settings store.
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Direction {
    Input,
    Output,
}

fn list(direction: Direction) -> Result<Vec<DeviceInfo>> {
    let host = cpal::default_host();
    let default = match direction {
        Direction::Input => host.default_input_device(),
        Direction::Output => host.default_output_device(),
    }
    .and_then(|d| d.id().ok());
    let devices: Vec<cpal::Device> = match direction {
        Direction::Input => host.input_devices()?.collect(),
        Direction::Output => host.output_devices()?.collect(),
    };
    Ok(devices
        .into_iter()
        .filter_map(|d| {
            let id = d.id().ok()?;
            let name = d.description().map(|desc| desc.name().to_owned()).unwrap_or_else(|_| id.to_string());
            Some(DeviceInfo { is_default: Some(&id) == default.as_ref(), id: id.to_string(), name })
        })
        .collect())
}

/// Microphones, the default one marked.
pub fn inputs() -> Result<Vec<DeviceInfo>> {
    list(Direction::Input)
}

/// Loudspeakers and headphones, the default one marked.
pub fn outputs() -> Result<Vec<DeviceInfo>> {
    list(Direction::Output)
}

fn find(direction: Direction, id: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    let kind = if direction == Direction::Input { "input" } else { "output" };
    match id {
        None => match direction {
            Direction::Input => host.default_input_device(),
            Direction::Output => host.default_output_device(),
        }
        .ok_or(DeviceError::NoDevice(kind)),
        Some(id) => {
            let mut devices: Box<dyn Iterator<Item = cpal::Device>> = match direction {
                Direction::Input => Box::new(host.input_devices()?),
                Direction::Output => Box::new(host.output_devices()?),
            };
            devices.find(|d| d.id().is_ok_and(|d| d.to_string() == id)).ok_or_else(|| DeviceError::UnknownDevice(id.into()))
        }
    }
}

/// Counters a call's stats screen can show.
#[derive(Debug, Default)]
pub struct DeviceStats {
    /// Frames delivered (capture) or requested (playback).
    pub frames: AtomicU64,
    /// Samples lost because the ring was full (capture) or empty (playback).
    pub glitches: AtomicU64,
}

/// A running microphone. Dropping it stops the device.
pub struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub stats: Arc<DeviceStats>,
    /// The device's own rate, before conversion.
    pub device_rate: u32,
}

impl Capture {
    /// Open a microphone (`None`: the default) and call `on_frame` with every
    /// 20 ms of it, on a thread of its own.
    pub fn start(device_id: Option<&str>, on_frame: impl FnMut(&[f32; FRAME]) + Send + 'static) -> Result<Capture> {
        let device_id = device_id.map(str::to_owned);
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(DeviceStats::default());
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = {
            let (stop, stats) = (stop.clone(), stats.clone());
            std::thread::Builder::new()
                .name("horo-capture".into())
                .spawn(move || capture_thread(device_id, on_frame, stop, stats, ready_tx))
                .map_err(|e| DeviceError::Cpal(e.to_string()))?
        };
        let device_rate = ready_rx.recv().map_err(|_| DeviceError::Cpal("capture thread died".into()))??;
        Ok(Capture { stop, thread: Some(thread), stats, device_rate })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// How long to wait before reopening a device whose stream was lost, and
/// between tries while it won't open.
const REOPEN_AFTER: Duration = Duration::from_millis(300);

/// Set by a stream's error callback when cpal has given the stream up: the
/// device went away, or its sample rate changed under us (Bluetooth
/// headphones do, between their music and headset modes, whenever any app
/// opens or closes their microphone). cpal pauses such a stream for good, so
/// the thread that owns it has to open the device again.
fn on_stream_error(which: &'static str, lost: Arc<AtomicBool>) -> impl FnMut(cpal::Error) + Send + 'static {
    move |e| match e.kind() {
        // An overrun or a followed route change: the stream carries on.
        cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged => tracing::debug!("{which}: {e}"),
        _ => {
            tracing::warn!("{which} stopped ({e}); reopening it");
            lost.store(true, Ordering::Relaxed);
        }
    }
}

/// Reopen a lost device with `open`: the one asked for, else the system
/// default, trying again until it opens or `stop` is set.
fn reopen<T>(which: &str, device_id: Option<&str>, stop: &AtomicBool, mut open: impl FnMut(Option<&str>) -> Result<T>) -> Option<T> {
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(REOPEN_AFTER);
        match open(device_id) {
            Ok(opened) => return Some(opened),
            Err(e) if device_id.is_some() => match open(None) {
                Ok(opened) => {
                    tracing::warn!("{which}: {e}; using the system default until it's back");
                    return Some(opened);
                }
                Err(e) => tracing::warn!("{which}: still can't reopen: {e}"),
            },
            Err(e) => tracing::warn!("{which}: still can't reopen: {e}"),
        }
    }
    None
}

fn capture_thread(
    device_id: Option<String>,
    mut on_frame: impl FnMut(&[f32; FRAME]),
    stop: Arc<AtomicBool>,
    stats: Arc<DeviceStats>,
    ready: mpsc::Sender<Result<u32>>,
) {
    let lost = Arc::new(AtomicBool::new(false));
    let open = |device_id: Option<&str>| {
        lost.store(false, Ordering::Relaxed);
        let device = find(Direction::Input, device_id)?;
        let config = device.default_input_config()?;
        let rate = config.sample_rate();
        let channels = config.channels() as usize;
        // Half a second of headroom between the callback and this thread.
        let (producer, consumer) = HeapRb::<f32>::new(rate as usize / 2).split();
        let (stats, lost) = (stats.clone(), lost.clone());
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => input_stream::<f32>(&device, &config.into(), channels, producer, stats, lost)?,
            cpal::SampleFormat::I16 => input_stream::<i16>(&device, &config.into(), channels, producer, stats, lost)?,
            other => return Err(DeviceError::UnsupportedFormat(other)),
        };
        stream.play().map_err(|e| DeviceError::Cpal(e.to_string()))?;
        let frames = ToFrames::new(rate).map_err(DeviceError::Resampler)?;
        Ok((stream, consumer, frames, rate))
    };
    let (mut stream, mut consumer, mut frames, rate) = match open(device_id.as_deref()) {
        Ok(ok) => ok,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(rate));

    let mut scratch = vec![0f32; rate as usize / 10];
    while !stop.load(Ordering::Relaxed) {
        if lost.load(Ordering::Relaxed) {
            drop(stream);
            let Some(opened) = reopen("microphone", device_id.as_deref(), &stop, &open) else { return };
            (stream, consumer, frames, _) = opened;
            continue;
        }
        loop {
            let n = consumer.pop_slice(&mut scratch);
            if n == 0 {
                break;
            }
            frames.push(&scratch[..n], |frame| {
                stats.frames.fetch_add(1, Ordering::Relaxed);
                on_frame(frame);
            });
        }
        std::thread::sleep(POLL);
    }
}

fn input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    mut producer: impl Producer<Item = f32> + Send + 'static,
    stats: Arc<DeviceStats>,
    lost: Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let stream = device.build_input_stream::<T, _, _>(
        *config,
        move |data: &[T], _| {
            // Real-time: downmix and push, nothing else.
            for frame in data.chunks(channels) {
                let mono = frame.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / channels as f32;
                if producer.try_push(mono).is_err() {
                    stats.glitches.fetch_add(1, Ordering::Relaxed);
                }
            }
        },
        on_stream_error("microphone", lost),
        None,
    )?;
    Ok(stream)
}

/// A running loudspeaker. Dropping it stops the device.
pub struct Playback {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub stats: Arc<DeviceStats>,
    pub device_rate: u32,
}

impl Playback {
    /// Open a loudspeaker (`None`: the default) and fill it by calling
    /// `next_frame` for every 20 ms, on a thread of its own, paced by the
    /// device. Whatever `next_frame` does (a jitter buffer and a mixer)
    /// runs there, not in the real-time callback.
    pub fn start(device_id: Option<&str>, next_frame: impl FnMut(&mut [f32; FRAME]) + Send + 'static) -> Result<Playback> {
        let device_id = device_id.map(str::to_owned);
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(DeviceStats::default());
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = {
            let (stop, stats) = (stop.clone(), stats.clone());
            std::thread::Builder::new()
                .name("horo-playback".into())
                .spawn(move || playback_thread(device_id, next_frame, stop, stats, ready_tx))
                .map_err(|e| DeviceError::Cpal(e.to_string()))?
        };
        let device_rate = ready_rx.recv().map_err(|_| DeviceError::Cpal("playback thread died".into()))??;
        Ok(Playback { stop, thread: Some(thread), stats, device_rate })
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn playback_thread(
    device_id: Option<String>,
    mut next_frame: impl FnMut(&mut [f32; FRAME]),
    stop: Arc<AtomicBool>,
    stats: Arc<DeviceStats>,
    ready: mpsc::Sender<Result<u32>>,
) {
    let lost = Arc::new(AtomicBool::new(false));
    let open = |device_id: Option<&str>| {
        lost.store(false, Ordering::Relaxed);
        let device = find(Direction::Output, device_id)?;
        let config = device.default_output_config()?;
        let rate = config.sample_rate();
        let channels = config.channels() as usize;
        let (producer, consumer) = HeapRb::<f32>::new(rate as usize / 2).split();
        let (stats, lost) = (stats.clone(), lost.clone());
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => output_stream::<f32>(&device, &config.into(), channels, consumer, stats, lost)?,
            cpal::SampleFormat::I16 => output_stream::<i16>(&device, &config.into(), channels, consumer, stats, lost)?,
            other => return Err(DeviceError::UnsupportedFormat(other)),
        };
        let frames = FromFrames::new(rate).map_err(DeviceError::Resampler)?;
        Ok((stream, producer, frames, rate))
    };
    let (mut stream, mut producer, mut frames, rate) = match open(device_id.as_deref()) {
        Ok(ok) => ok,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };

    // Keep about 40 ms queued ahead of the device: enough to ride out this
    // thread being scheduled late, little enough not to add much delay.
    let target_for = |rate: u32| (rate as usize / 1000) * 40;
    let mut target = target_for(rate);
    let mut frame = [0f32; FRAME];
    let mut fill = |producer: &mut ringbuf::HeapProd<f32>, frames: &mut FromFrames, target: usize| {
        while producer.occupied_len() < target {
            next_frame(&mut frame);
            stats.frames.fetch_add(1, Ordering::Relaxed);
            frames.push(&frame, |samples| {
                producer.push_slice(samples);
            });
        }
    };
    // Prime before starting, so the device's first callback isn't empty.
    fill(&mut producer, &mut frames, target);
    if let Err(e) = stream.play() {
        let _ = ready.send(Err(DeviceError::Cpal(e.to_string())));
        return;
    }
    let _ = ready.send(Ok(rate));
    while !stop.load(Ordering::Relaxed) {
        if lost.load(Ordering::Relaxed) {
            drop(stream);
            // Primed and started like the first time; a reopened device may
            // run at another rate (headset mode is 16 or 24 kHz).
            let restart = |device_id: Option<&str>| {
                let (stream, mut producer, mut frames, rate) = open(device_id)?;
                fill(&mut producer, &mut frames, target_for(rate));
                stream.play().map_err(|e| DeviceError::Cpal(e.to_string()))?;
                Ok((stream, producer, frames, rate))
            };
            let Some(opened) = reopen("loudspeaker", device_id.as_deref(), &stop, restart) else { return };
            let rate;
            (stream, producer, frames, rate) = opened;
            target = target_for(rate);
            continue;
        }
        fill(&mut producer, &mut frames, target);
        std::thread::sleep(POLL);
    }
}

fn output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    mut consumer: impl Consumer<Item = f32> + Send + 'static,
    stats: Arc<DeviceStats>,
    lost: Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let stream = device.build_output_stream::<T, _, _>(
        *config,
        move |data: &mut [T], _| {
            // Real-time: pop and duplicate across channels, nothing else.
            for frame in data.chunks_mut(channels) {
                let sample = consumer.try_pop().unwrap_or_else(|| {
                    stats.glitches.fetch_add(1, Ordering::Relaxed);
                    0.0
                });
                for out in frame {
                    *out = T::from_sample(sample);
                }
            }
        },
        on_stream_error("loudspeaker", lost),
        None,
    )?;
    Ok(stream)
}
