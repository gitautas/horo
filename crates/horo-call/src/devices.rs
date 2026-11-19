//! A call's [`Microphone`] and [`Speaker`] on real hardware.

use horo_devices::{Capture, FRAME, Playback};

use crate::{Microphone, Result, Speaker};

/// Which hardware a call uses.
#[derive(Debug, Clone, Default)]
pub enum AudioDevices {
    /// The default microphone and loudspeaker with the platform's voice
    /// processing (echo cancellation) where there is one: Apple's
    /// VoiceProcessingIO. Elsewhere, the plain defaults, so use headphones.
    #[default]
    Voice,
    /// Particular devices (`None`: the default), without echo cancellation.
    Plain { input: Option<String>, output: Option<String> },
}

/// Running audio for a call. Dropping it stops the devices.
pub enum RunningAudio {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    Voice(horo_devices::Duplex),
    Plain(Capture, Playback),
}

impl RunningAudio {
    /// Whether the echo canceller is in the path.
    pub fn cancels_echo(&self) -> bool {
        !matches!(self, RunningAudio::Plain(..))
    }
}

pub fn start(mut microphone: Microphone, mut speaker: Speaker, devices: AudioDevices) -> Result<RunningAudio> {
    match devices {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        AudioDevices::Voice => {
            #[cfg(target_os = "ios")]
            horo_devices::activate_voice_session()?;
            let duplex = horo_devices::Duplex::start(
                move |frame: &[f32; FRAME]| microphone.push(frame),
                move |frame: &mut [f32; FRAME]| speaker.pull(frame),
            )?;
            Ok(RunningAudio::Voice(duplex))
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        AudioDevices::Voice => start(microphone, speaker, AudioDevices::Plain { input: None, output: None }),
        AudioDevices::Plain { input, output } => {
            let capture = Capture::start(input.as_deref(), move |frame: &[f32; FRAME]| microphone.push(frame))?;
            let playback = Playback::start(output.as_deref(), move |frame: &mut [f32; FRAME]| speaker.pull(frame))?;
            Ok(RunningAudio::Plain(capture, playback))
        }
    }
}
