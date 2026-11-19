//! horo's media engine.
//!
//! - [`peer`]: one WebRTC peer connection over str0m, with host UDP,
//!   ICE-TCP and TURN candidates gathered together.
//! - [`turn`]: the sans-I/O TURN client behind the relayed candidates.

pub mod peer;
pub mod turn;

pub use peer::{IceConnectionState, Peer, PeerConfig, PeerError, PeerEvent, TurnServer};
pub use str0m::media::{MediaKind, Mid};
