//! A single error type for everything crossing the Tauri IPC boundary.
//!
//! The frontend only ever sees `{ kind, message }`, so it can branch on `kind`
//! without parsing prose.

use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not signed in")]
    NotSignedIn,

    #[error("{0}")]
    Auth(String),

    #[error("unknown room {0}")]
    UnknownRoom(String),

    #[error("no open timeline for room {0}")]
    NoTimeline(String),

    #[error("{0}")]
    Matrix(#[from] matrix_sdk::Error),

    #[error("{0}")]
    MatrixHttp(#[from] matrix_sdk::HttpError),

    #[error("{0}")]
    Id(#[from] matrix_sdk::IdParseError),

    #[error("{0}")]
    ClientBuild(#[from] matrix_sdk::ClientBuildError),

    #[error("{0}")]
    Timeline(#[from] matrix_sdk_ui::timeline::Error),

    #[error("{0}")]
    RoomList(#[from] matrix_sdk_ui::room_list_service::Error),

    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Http(#[from] reqwest::Error),

    #[error("{0}")]
    Other(String),
}

/// A stable, machine-readable discriminant for the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotSignedIn,
    Auth,
    UnknownRoom,
    NoTimeline,
    Matrix,
    BadId,
    ClientBuild,
    Timeline,
    RoomList,
    Io,
    Json,
    Http,
    Other,
}

/// What an `Error` looks like once it reaches the frontend.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct UwuError {
    kind: ErrorKind,
    message: String,
}

impl Error {
    fn kind(&self) -> ErrorKind {
        match self {
            Self::NotSignedIn => ErrorKind::NotSignedIn,
            Self::Auth(_) => ErrorKind::Auth,
            Self::UnknownRoom(_) => ErrorKind::UnknownRoom,
            Self::NoTimeline(_) => ErrorKind::NoTimeline,
            Self::Matrix(_) | Self::MatrixHttp(_) => ErrorKind::Matrix,
            Self::Id(_) => ErrorKind::BadId,
            Self::ClientBuild(_) => ErrorKind::ClientBuild,
            Self::Timeline(_) => ErrorKind::Timeline,
            Self::RoomList(_) => ErrorKind::RoomList,
            Self::Io(_) => ErrorKind::Io,
            Self::Json(_) => ErrorKind::Json,
            Self::Http(_) => ErrorKind::Http,
            Self::Other(_) => ErrorKind::Other,
        }
    }
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Self::Other(e.to_string())
    }
}

impl From<url::ParseError> for Error {
    fn from(e: url::ParseError) -> Self {
        Self::Other(e.to_string())
    }
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        UwuError { kind: self.kind(), message: message(self) }.serialize(s)
    }
}

/// The error's own message, plus whatever caused it that it doesn't already say.
///
/// Transport errors keep the useful part in their sources: reqwest's top line
/// is "error sending request for url (…)", and only further down does it say
/// "operation timed out" or "connection refused".
fn message(error: &Error) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        let text = cause.to_string();
        // `#[error("{0}")]` wrappers repeat their inner error word for word.
        if !text.is_empty() && !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        source = cause.source();
    }
    message
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
