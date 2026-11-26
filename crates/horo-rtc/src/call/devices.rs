//! A call's [`Microphone`] and [`Speaker`] on real hardware.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::devices::{Capture, FRAME, Playback};
use crate::audio::echo::{EchoCanceller, EchoStats, Processing};

use super::{Microphone, Result, Speaker};

/// Where a frame handed to the loudspeaker comes back from the microphone,
/// as a starting point: the playback queue (40 ms) plus both devices'
/// buffers and resamplers. AEC3 finds the exact delay itself; on a MacBook
/// Air's own speakers and microphone it measured 104–112 ms.
const ECHO_DELAY_HINT: Duration = Duration::from_millis(100);

/// Which hardware a call uses.
#[derive(Debug, Clone)]
pub struct AudioDevices {
    /// A device id from `crate::devices::inputs()`; `None`, the default.
    pub input: Option<String>,
    pub output: Option<String>,
    /// Remove the loudspeaker's sound from the microphone (horo's software
    /// canceller: WebRTC's AEC3 in Rust, with noise suppression). Off only
    /// makes sense with headphones.
    pub cancel_echo: bool,
}

impl Default for AudioDevices {
    fn default() -> Self {
        AudioDevices { input: None, output: None, cancel_echo: true }
    }
}

/// Running audio for a call. Dropping it stops the devices. Either
/// device can be swapped while it runs ([`RunningAudio::set_input`],
/// [`RunningAudio::set_output`]): the call carries on through the new one.
pub struct RunningAudio {
    capture: Option<Capture>,
    playback: Option<Playback>,
    microphone: Arc<Mutex<Microphone>>,
    speaker: Arc<Mutex<Speaker>>,
    devices: AudioDevices,
    echo: Option<EchoCanceller>,
    /// The chosen microphone or speaker wouldn't open (unplugged, say),
    /// so the system default is in use instead.
    fallen_back: Vec<FellBack>,
}

/// A device that wouldn't open at the start, and why.
#[derive(Debug, Clone)]
pub struct FellBack {
    /// "microphone" or "speaker".
    pub which: &'static str,
    pub reason: String,
}

impl RunningAudio {
    /// Whether the echo canceller is in the path.
    pub fn cancels_echo(&self) -> bool {
        self.echo.is_some()
    }

    /// How the echo canceller is doing.
    pub fn echo_stats(&self) -> Option<EchoStats> {
        self.echo.as_ref().map(EchoCanceller::stats)
    }

    /// Devices that wouldn't open at the start, the default used instead.
    pub fn fallen_back(&self) -> &[FellBack] {
        &self.fallen_back
    }

    /// Which devices are in use.
    pub fn devices(&self) -> &AudioDevices {
        &self.devices
    }

    /// Hear the call through another microphone (`None`, the default).
    /// If it won't open, the old one is opened again and the error
    /// returned.
    pub fn set_input(&mut self, device: Option<String>) -> Result<()> {
        if device == self.devices.input {
            return Ok(());
        }
        // The old one stops first: two feeding the call at once would send
        // every frame twice.
        self.capture = None;
        match capture(&self.microphone, device.as_deref()) {
            Ok(new) => {
                self.capture = Some(new);
                self.devices.input = device;
                Ok(())
            }
            Err(e) => {
                self.capture = capture(&self.microphone, self.devices.input.as_deref()).ok();
                Err(e)
            }
        }
    }

    /// Play the call through another output (`None`, the default). If it
    /// won't open, the old one is opened again and the error returned.
    pub fn set_output(&mut self, device: Option<String>) -> Result<()> {
        if device == self.devices.output {
            return Ok(());
        }
        self.playback = None;
        match playback(&self.speaker, device.as_deref()) {
            Ok(new) => {
                self.playback = Some(new);
                self.devices.output = device;
                Ok(())
            }
            Err(e) => {
                self.playback = playback(&self.speaker, self.devices.output.as_deref()).ok();
                Err(e)
            }
        }
    }
}

fn capture(microphone: &Arc<Mutex<Microphone>>, device: Option<&str>) -> Result<Capture> {
    let microphone = microphone.clone();
    Ok(Capture::start(device, move |frame: &[f32; FRAME]| {
        if let Ok(mut microphone) = microphone.lock() {
            microphone.push(frame);
        }
    })?)
}

fn playback(speaker: &Arc<Mutex<Speaker>>, device: Option<&str>) -> Result<Playback> {
    let speaker = speaker.clone();
    Ok(Playback::start(device, move |frame: &mut [f32; FRAME]| match speaker.lock() {
        Ok(mut speaker) => speaker.pull(frame),
        Err(_) => frame.fill(0.0),
    })?)
}

pub fn start(mut microphone: Microphone, mut speaker: Speaker, devices: AudioDevices) -> Result<RunningAudio> {
    let echo = devices.cancel_echo.then(|| EchoCanceller::new(Processing::default(), ECHO_DELAY_HINT));
    microphone.set_echo_canceller(echo.clone());
    speaker.set_echo_canceller(echo.clone());
    let (microphone, speaker) = (Arc::new(Mutex::new(microphone)), Arc::new(Mutex::new(speaker)));
    let mut devices = devices;
    let mut fallen_back = Vec::new();
    // A chosen device that's gone (headphones off) shouldn't stop the
    // call: the system default stands in, and the caller is told.
    let capture = match capture(&microphone, devices.input.as_deref()) {
        Err(e) if devices.input.is_some() => {
            fallen_back.push(FellBack { which: "microphone", reason: e.to_string() });
            devices.input = None;
            capture(&microphone, None)?
        }
        opened => opened?,
    };
    let playback = match playback(&speaker, devices.output.as_deref()) {
        Err(e) if devices.output.is_some() => {
            fallen_back.push(FellBack { which: "speaker", reason: e.to_string() });
            devices.output = None;
            playback(&speaker, None)?
        }
        opened => opened?,
    };
    Ok(RunningAudio { capture: Some(capture), playback: Some(playback), microphone, speaker, devices, echo, fallen_back })
}
