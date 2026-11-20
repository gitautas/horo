//! A call's [`Microphone`] and [`Speaker`] on real hardware.

use std::time::Duration;

use horo_devices::{Capture, FRAME, Playback};
use horo_media::audio::echo::{EchoCanceller, EchoStats, Processing};

use crate::{Microphone, Result, Speaker};

/// Where a frame handed to the loudspeaker comes back from the microphone,
/// as a starting point: the playback queue (40 ms) plus both devices'
/// buffers and resamplers. AEC3 finds the exact delay itself; on a MacBook
/// Air's own speakers and microphone it measured 104–112 ms.
const ECHO_DELAY_HINT: Duration = Duration::from_millis(100);

/// Which hardware a call uses.
#[derive(Debug, Clone)]
pub struct AudioDevices {
    /// A device id from `horo_devices::inputs()`; `None`, the default.
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

/// Running audio for a call. Dropping it stops the devices.
pub struct RunningAudio {
    _capture: Capture,
    _playback: Playback,
    echo: Option<EchoCanceller>,
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
}

pub fn start(mut microphone: Microphone, mut speaker: Speaker, devices: AudioDevices) -> Result<RunningAudio> {
    let echo = devices.cancel_echo.then(|| EchoCanceller::new(Processing::default(), ECHO_DELAY_HINT));
    microphone.set_echo_canceller(echo.clone());
    speaker.set_echo_canceller(echo.clone());
    let capture = Capture::start(devices.input.as_deref(), move |frame: &[f32; FRAME]| microphone.push(frame))?;
    let playback = Playback::start(devices.output.as_deref(), move |frame: &mut [f32; FRAME]| speaker.pull(frame))?;
    Ok(RunningAudio { _capture: capture, _playback: playback, echo })
}
