//! horo's calls, from the Matrix room down to the speaker.
//!
//! - [`matrixrtc`]: the MatrixRTC side, as Element Call does it: the call
//!   membership, LiveKit credentials from the homeserver's OpenID token, and
//!   ringing. Its functions are re-exported here.
//! - [`livekit`]: LiveKit signalling, the WebSocket half of joining a room.
//! - [`peer`]: one WebRTC peer connection over str0m, with host UDP, ICE-TCP
//!   and TURN candidates gathered together.
//! - [`turn`]: the sans-I/O TURN client behind the relayed candidates.
//! - [`audio`]: Opus, echo cancellation, the jitter buffer, the mixer and the
//!   noise gate.
//!
//! Everything but [`matrixrtc`] sits behind the `media` feature (on by
//! default). The Tauri shell turns it off: its WebView does the media.

pub mod matrixrtc;

#[cfg(feature = "media")]
pub mod audio;
#[cfg(feature = "media")]
pub mod livekit;
#[cfg(feature = "media")]
pub mod peer;
#[cfg(feature = "media")]
pub mod turn;

pub use matrixrtc::{CallCredentials, CallParticipants, active_calls, join, leave, participants, refresh};

#[cfg(feature = "media")]
pub use peer::{IceConnectionState, Impairment, Peer, PeerConfig, PeerError, PeerEvent, TurnServer};
#[cfg(feature = "media")]
pub use str0m::media::{MediaKind, Mid};
