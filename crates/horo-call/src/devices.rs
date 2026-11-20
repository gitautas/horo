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

/// Who removes the loudspeaker's sound from the microphone.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum EchoCancellation {
    /// horo's own (WebRTC's AEC3 in Rust, with noise suppression): every
    /// platform, any devices.
    #[default]
    Software,
    /// Apple's VoiceProcessingIO: macOS and iOS, default devices only.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    Apple,
    /// None: headphones only.
    Off,
}

/// Which hardware a call uses, and how echo is handled.
#[derive(Debug, Clone, Default)]
pub struct AudioDevices {
    /// A device id from `horo_devices::inputs()`; `None`, the default.
    pub input: Option<String>,
    pub output: Option<String>,
    pub echo: EchoCancellation,
}

/// Running audio for a call. Dropping it stops the devices.
pub enum RunningAudio {
    Devices { capture: Capture, playback: Playback, echo: Option<EchoCanceller> },
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    Apple(horo_devices::Duplex),
}

impl RunningAudio {
    /// Whether an echo canceller is in the path.
    pub fn cancels_echo(&self) -> bool {
        match self {
            RunningAudio::Devices { echo, .. } => echo.is_some(),
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            RunningAudio::Apple(_) => true,
        }
    }

    /// The software canceller's numbers, when it's the one running.
    pub fn echo_stats(&self) -> Option<EchoStats> {
        match self {
            RunningAudio::Devices { echo: Some(echo), .. } => Some(echo.stats()),
            _ => None,
        }
    }
}

pub fn start(mut microphone: Microphone, mut speaker: Speaker, devices: AudioDevices) -> Result<RunningAudio> {
    match devices.echo {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        EchoCancellation::Apple => {
            if devices.input.is_some() || devices.output.is_some() {
                tracing::warn!("Apple's voice processing uses the default devices; ignoring the chosen ones");
            }
            #[cfg(target_os = "ios")]
            horo_devices::activate_voice_session()?;
            let duplex = horo_devices::Duplex::start(
                move |frame: &[f32; FRAME]| microphone.push(frame),
                move |frame: &mut [f32; FRAME]| speaker.pull(frame),
            )?;
            Ok(RunningAudio::Apple(duplex))
        }
        EchoCancellation::Software | EchoCancellation::Off => {
            let echo = (devices.echo == EchoCancellation::Software).then(|| EchoCanceller::new(Processing::default(), ECHO_DELAY_HINT));
            microphone.set_echo_canceller(echo.clone());
            speaker.set_echo_canceller(echo.clone());
            let capture = Capture::start(devices.input.as_deref(), move |frame: &[f32; FRAME]| microphone.push(frame))?;
            let playback = Playback::start(devices.output.as_deref(), move |frame: &mut [f32; FRAME]| speaker.pull(frame))?;
            Ok(RunningAudio::Devices { capture, playback, echo })
        }
    }
}
