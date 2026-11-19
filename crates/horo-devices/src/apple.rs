//! Apple's VoiceProcessingIO: the microphone and the loudspeaker as one
//! unit, with echo cancellation, noise suppression and gain control between
//! them. It's what FaceTime uses, on macOS and iOS alike.
//!
//! Echo cancellation needs to see what's played to subtract it from what's
//! heard, so capture and playback can't be two independent streams the way
//! [`Capture`](crate::Capture) and [`Playback`](crate::Playback) are: one
//! [`Duplex`] owns both. Its frames are the same 48 kHz, 20 ms ones; the
//! unit converts to and from whatever the hardware runs at.
//!
//! As with the cpal backend, the real-time callbacks only move samples
//! through lock-free rings; a thread of the duplex's own cuts them into
//! frames and calls the caller's closures.
//!
//! On iOS the app's audio session has to be `PlayAndRecord` in `VoiceChat`
//! mode first ([`activate_voice_session`]).

use std::{
    ffi::c_void,
    ptr::NonNull,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};

use objc2_audio_toolbox::{
    AURenderCallbackStruct, AudioComponentDescription, AudioComponentFindNext, AudioComponentInstanceDispose,
    AudioComponentInstanceNew, AudioOutputUnitStart, AudioOutputUnitStop, AudioUnit, AudioUnitInitialize,
    AudioUnitRender, AudioUnitRenderActionFlags, AudioUnitSetProperty, kAUVoiceIOProperty_BypassVoiceProcessing,
    kAUVoiceIOProperty_MuteOutput, kAudioOutputUnitProperty_EnableIO, kAudioOutputUnitProperty_SetInputCallback,
    kAudioUnitManufacturer_Apple, kAudioUnitProperty_SetRenderCallback, kAudioUnitProperty_StreamFormat,
    kAudioUnitScope_Global, kAudioUnitScope_Input, kAudioUnitScope_Output, kAudioUnitSubType_VoiceProcessingIO,
    kAudioUnitType_Output,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp, kAudioFormatFlagIsFloat,
    kAudioFormatFlagIsNonInterleaved, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM,
};
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};

use crate::{DeviceError, DeviceStats, FRAME, POLL, Result, SAMPLE_RATE};

/// The microphone and loudspeaker elements of an I/O unit.
const INPUT_BUS: u32 = 1;
const OUTPUT_BUS: u32 = 0;
/// More than a callback ever asks for (the unit's default maximum is 4096).
const MAX_CALLBACK: usize = 8192;
/// Queued ahead of the loudspeaker; see `playback_thread` in lib.rs.
const PLAYBACK_AHEAD: usize = SAMPLE_RATE as usize / 1000 * 40;

fn status(what: &str, code: i32) -> Result<()> {
    if code == 0 { Ok(()) } else { Err(DeviceError::Cpal(format!("{what}: OSStatus {code}"))) }
}

/// An `AudioUnit` handle. The unit's property calls are thread-safe.
#[derive(Clone, Copy)]
struct Unit(AudioUnit);
unsafe impl Send for Unit {}
unsafe impl Sync for Unit {}

impl Unit {
    unsafe fn set<T>(self, id: u32, scope: u32, element: u32, value: &T) -> i32 {
        unsafe {
            AudioUnitSetProperty(self.0, id, scope, element, value as *const T as *const c_void, size_of::<T>() as u32)
        }
    }
}

/// What the input callback owns.
struct CaptureSide {
    unit: Unit,
    scratch: Vec<f32>,
    ring: HeapProd<f32>,
    stats: Arc<DeviceStats>,
}

/// What the render callback owns.
struct PlaybackSide {
    ring: HeapCons<f32>,
    stats: Arc<DeviceStats>,
}

unsafe extern "C-unwind" fn on_input(
    refcon: NonNull<c_void>,
    flags: NonNull<AudioUnitRenderActionFlags>,
    timestamp: NonNull<AudioTimeStamp>,
    _bus: u32,
    frames: u32,
    _data: *mut AudioBufferList,
) -> i32 {
    // Real-time: render the processed microphone into scratch, push it.
    let side = unsafe { &mut *(refcon.as_ptr() as *mut CaptureSide) };
    let frames = (frames as usize).min(side.scratch.len());
    let mut list = AudioBufferList {
        mNumberBuffers: 1,
        mBuffers: [AudioBuffer {
            mNumberChannels: 1,
            mDataByteSize: (frames * 4) as u32,
            mData: side.scratch.as_mut_ptr() as *mut c_void,
        }],
    };
    let code = unsafe {
        AudioUnitRender(side.unit.0, flags.as_ptr(), timestamp, INPUT_BUS, frames as u32, NonNull::from(&mut list))
    };
    if code != 0 {
        return code;
    }
    let pushed = side.ring.push_slice(&side.scratch[..frames]);
    if pushed < frames {
        side.stats.glitches.fetch_add((frames - pushed) as u64, Ordering::Relaxed);
    }
    0
}

unsafe extern "C-unwind" fn on_render(
    refcon: NonNull<c_void>,
    _flags: NonNull<AudioUnitRenderActionFlags>,
    _timestamp: NonNull<AudioTimeStamp>,
    _bus: u32,
    frames: u32,
    data: *mut AudioBufferList,
) -> i32 {
    // Real-time: pop what the duplex thread queued, pad with silence.
    let side = unsafe { &mut *(refcon.as_ptr() as *mut PlaybackSide) };
    let Some(list) = (unsafe { data.as_mut() }) else { return 0 };
    let buffer = &mut list.mBuffers[0];
    let out = unsafe { std::slice::from_raw_parts_mut(buffer.mData as *mut f32, frames as usize) };
    let popped = side.ring.pop_slice(out);
    if popped < out.len() {
        out[popped..].fill(0.0);
        side.stats.glitches.fetch_add((out.len() - popped) as u64, Ordering::Relaxed);
    }
    0
}

/// A running VoiceProcessingIO unit: microphone and loudspeaker together.
/// Dropping it stops both.
pub struct Duplex {
    unit: Unit,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    capture_side: *mut CaptureSide,
    playback_side: *mut PlaybackSide,
    /// Frames delivered from the microphone; samples dropped.
    pub capture: Arc<DeviceStats>,
    /// Frames asked for by the loudspeaker; samples padded with silence.
    pub playback: Arc<DeviceStats>,
}

// The raw sides are only touched by the unit's callbacks, and freed after
// the unit is gone.
unsafe impl Send for Duplex {}

impl Duplex {
    /// Open the default microphone and loudspeaker as one voice-processing
    /// unit. `on_capture` gets every 20 ms of processed microphone;
    /// `next_playback` is asked for every 20 ms to play. Both run on the
    /// duplex's thread, never in the real-time callbacks.
    pub fn start(
        mut on_capture: impl FnMut(&[f32; FRAME]) + Send + 'static,
        mut next_playback: impl FnMut(&mut [f32; FRAME]) + Send + 'static,
    ) -> Result<Duplex> {
        let capture = Arc::new(DeviceStats::default());
        let playback = Arc::new(DeviceStats::default());
        let (mic_in, mut mic_out) = HeapRb::<f32>::new(SAMPLE_RATE as usize / 2).split();
        let (mut speaker_in, speaker_out) = HeapRb::<f32>::new(SAMPLE_RATE as usize / 2).split();

        // Prime the loudspeaker so the first callback isn't empty.
        let mut frame = [0f32; FRAME];
        while speaker_in.occupied_len() < PLAYBACK_AHEAD {
            next_playback(&mut frame);
            playback.frames.fetch_add(1, Ordering::Relaxed);
            speaker_in.push_slice(&frame);
        }

        let unit = unsafe { new_unit()? };
        let capture_side = Box::into_raw(Box::new(CaptureSide {
            unit,
            scratch: vec![0.0; MAX_CALLBACK],
            ring: mic_in,
            stats: capture.clone(),
        }));
        let playback_side = Box::into_raw(Box::new(PlaybackSide { ring: speaker_out, stats: playback.clone() }));
        let mut duplex = Duplex {
            unit,
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            capture_side,
            playback_side,
            capture: capture.clone(),
            playback: playback.clone(),
        };
        // On error, dropping `duplex` disposes the unit and frees the sides.
        unsafe { configure(unit, capture_side, playback_side)? };

        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        let stop = duplex.stop.clone();
        duplex.thread = Some(
            std::thread::Builder::new()
                .name("horo-duplex".into())
                .spawn(move || {
                    let _ = ready_tx.send(());
                    let mut mic = [0f32; FRAME];
                    while !stop.load(Ordering::Relaxed) {
                        // The unit runs at 48 kHz on our side: no resampling.
                        while mic_out.occupied_len() >= FRAME {
                            mic_out.pop_slice(&mut mic);
                            capture.frames.fetch_add(1, Ordering::Relaxed);
                            on_capture(&mic);
                        }
                        while speaker_in.occupied_len() < PLAYBACK_AHEAD {
                            next_playback(&mut frame);
                            playback.frames.fetch_add(1, Ordering::Relaxed);
                            speaker_in.push_slice(&frame);
                        }
                        std::thread::sleep(POLL);
                    }
                })
                .map_err(|e| DeviceError::Cpal(e.to_string()))?,
        );
        let _ = ready_rx.recv();
        unsafe { status("AudioOutputUnitStart", AudioOutputUnitStart(unit.0))? };
        Ok(duplex)
    }

    /// Turn the voice processing off (`true`) or back on, while running: an
    /// A/B switch for measuring what it does.
    pub fn set_bypass(&self, bypass: bool) -> Result<()> {
        let value = bypass as u32;
        unsafe {
            status(
                "bypass voice processing",
                self.unit.set(kAUVoiceIOProperty_BypassVoiceProcessing, kAudioUnitScope_Global, INPUT_BUS, &value),
            )
        }
    }

    /// Silence the microphone inside the unit. The canceller keeps adapting,
    /// so unmuting doesn't bring back a burst of echo.
    pub fn set_muted(&self, muted: bool) -> Result<()> {
        let value = muted as u32;
        unsafe {
            status("mute microphone", self.unit.set(kAUVoiceIOProperty_MuteOutput, kAudioUnitScope_Global, INPUT_BUS, &value))
        }
    }
}

impl Drop for Duplex {
    fn drop(&mut self) {
        unsafe {
            AudioOutputUnitStop(self.unit.0);
            AudioComponentInstanceDispose(self.unit.0);
            // The unit is gone; no callback can touch the sides now.
            drop(Box::from_raw(self.capture_side));
            drop(Box::from_raw(self.playback_side));
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

unsafe fn new_unit() -> Result<Unit> {
    unsafe {
        let mut desc = AudioComponentDescription {
            componentType: kAudioUnitType_Output,
            componentSubType: kAudioUnitSubType_VoiceProcessingIO,
            componentManufacturer: kAudioUnitManufacturer_Apple,
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        let component = AudioComponentFindNext(std::ptr::null_mut(), NonNull::from(&mut desc));
        if component.is_null() {
            return Err(DeviceError::NoDevice("voice-processing"));
        }
        let mut unit: AudioUnit = std::ptr::null_mut();
        status("AudioComponentInstanceNew", AudioComponentInstanceNew(component, NonNull::from(&mut unit)))?;
        Ok(Unit(unit))
    }
}

unsafe fn configure(unit: Unit, capture: *mut CaptureSide, playback: *mut PlaybackSide) -> Result<()> {
    unsafe {
        let on: u32 = 1;
        status("enable microphone", unit.set(kAudioOutputUnitProperty_EnableIO, kAudioUnitScope_Input, INPUT_BUS, &on))?;
        status("enable loudspeaker", unit.set(kAudioOutputUnitProperty_EnableIO, kAudioUnitScope_Output, OUTPUT_BUS, &on))?;
        let format = AudioStreamBasicDescription {
            mSampleRate: SAMPLE_RATE as f64,
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagIsNonInterleaved,
            mBytesPerPacket: 4,
            mFramesPerPacket: 1,
            mBytesPerFrame: 4,
            mChannelsPerFrame: 1,
            mBitsPerChannel: 32,
            mReserved: 0,
        };
        // What we hand the loudspeaker, and what we take from the microphone.
        status("loudspeaker format", unit.set(kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, OUTPUT_BUS, &format))?;
        status("microphone format", unit.set(kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output, INPUT_BUS, &format))?;
        let render = AURenderCallbackStruct { inputProc: Some(on_render), inputProcRefCon: playback as *mut c_void };
        status("render callback", unit.set(kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, OUTPUT_BUS, &render))?;
        let input = AURenderCallbackStruct { inputProc: Some(on_input), inputProcRefCon: capture as *mut c_void };
        status("input callback", unit.set(kAudioOutputUnitProperty_SetInputCallback, kAudioUnitScope_Global, INPUT_BUS, &input))?;
        status("AudioUnitInitialize", AudioUnitInitialize(unit.0))
    }
}

/// iOS: put the app's audio session in voice-chat mode, loudspeaker by
/// default, before starting a [`Duplex`].
#[cfg(target_os = "ios")]
pub fn activate_voice_session() -> Result<()> {
    use objc2_avf_audio::{
        AVAudioSession, AVAudioSessionCategoryOptions, AVAudioSessionCategoryPlayAndRecord, AVAudioSessionModeVoiceChat,
    };
    unsafe {
        let session = AVAudioSession::sharedInstance();
        session
            .setCategory_mode_options_error(
                AVAudioSessionCategoryPlayAndRecord.unwrap(),
                AVAudioSessionModeVoiceChat.unwrap(),
                AVAudioSessionCategoryOptions::DefaultToSpeaker | AVAudioSessionCategoryOptions::AllowBluetoothHFP,
            )
            .map_err(|e| DeviceError::Cpal(format!("audio session category: {e:?}")))?;
        session.setActive_error(true).map_err(|e| DeviceError::Cpal(format!("audio session: {e:?}")))?;
    }
    Ok(())
}
