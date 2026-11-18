//! horo's Matrix layer, independent of any UI.
//!
//! A host (today the Tauri shell) implements [`events::EventSink`] and hands it
//! in when a session starts; from then on the core reports changes through it
//! as typed [`events::Event`]s, and answers calls through the functions in
//! [`matrix`] and [`verification`].

pub mod dto;
pub mod error;
pub mod events;
pub mod matrix;
pub mod verification;

pub use error::{Error, Result};
pub use events::{Event, EventSink, Host};
pub use matrix::{MatrixCore, core::AppState};
