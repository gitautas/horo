//! Sharing the screen, on Linux: the desktop portal's ScreenCast. Its own
//! picker asks what to share (a monitor or a window, as other apps' sharing
//! does), then hands over a PipeWire remote with that one stream on it,
//! which `pw_video` reads, scaled to fit 1080p, with the cursor drawn in.
//!
//! Which picker shows, and whether the choice is remembered, is up to the
//! desktop's portal backend (GNOME, KDE, wlroots' or the compositor's own).

use ashpd::desktop::{
    PersistMode, ResponseError, Session,
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
};

use crate::pw_video;

/// The biggest picture sent: a screen is scaled down to fit.
const MAX_SIZE: (u32, u32) = (1920, 1080);

/// Called with each picture, on the capture's thread.
pub type OnFrame = pw_video::OnFrame;

/// What's being shared. Dropping it stops, and ends the portal's session
/// (which is what takes the "sharing" indicator away, where there is one).
pub struct ScreenShare {
    capture: Option<pw_video::Capture>,
    session: Option<Session<Screencast>>,
}

impl ScreenShare {
    /// Ask the portal what to share and start sending it, `on_frame`
    /// hearing each picture. `Ok(None)` if the picker was cancelled. Waits
    /// for the user: run it off the UI thread.
    pub async fn start(on_frame: OnFrame) -> Result<Option<ScreenShare>, String> {
        let failed = |e: ashpd::Error| format!("couldn't share the screen: {e}");
        let proxy = Screencast::new().await.map_err(|e| format!("the desktop can't share screens (no ScreenCast portal): {e}"))?;
        let session = proxy.create_session(Default::default()).await.map_err(failed)?;
        // An embedded cursor where the portal can, so it's part of the
        // picture; otherwise none rather than one sent apart and lost.
        let cursor = match proxy.available_cursor_modes().await {
            Ok(modes) if modes.contains(CursorMode::Embedded) => CursorMode::Embedded,
            _ => CursorMode::Hidden,
        };
        let options = SelectSourcesOptions::default()
            .set_cursor_mode(cursor)
            .set_sources(SourceType::Monitor | SourceType::Window)
            .set_multiple(false)
            .set_persist_mode(PersistMode::DoNot);
        let picked = async {
            proxy.select_sources(&session, options).await?.response()?;
            proxy.start(&session, None, Default::default()).await?.response()
        };
        let streams = match picked.await {
            Ok(streams) => streams,
            Err(ashpd::Error::Response(ResponseError::Cancelled)) => {
                let _ = session.close().await;
                return Ok(None);
            }
            Err(e) => {
                let _ = session.close().await;
                return Err(failed(e));
            }
        };
        let Some(stream) = streams.streams().first() else {
            let _ = session.close().await;
            return Ok(None);
        };
        let node = stream.pipe_wire_node_id();
        tracing::info!("sharing the screen: node {node}, {:?}", stream.size());
        let remote = match proxy.open_pipe_wire_remote(&session, Default::default()).await {
            Ok(remote) => remote,
            Err(e) => {
                let _ = session.close().await;
                return Err(failed(e));
            }
        };
        match pw_video::start(pw_video::Source::Screen { remote, node }, MAX_SIZE, on_frame) {
            Ok(capture) => Ok(Some(ScreenShare { capture: Some(capture), session: Some(session) })),
            Err(e) => {
                let _ = session.close().await;
                Err(format!("couldn't share the screen: {e}"))
            }
        }
    }
}

impl Drop for ScreenShare {
    fn drop(&mut self) {
        drop(self.capture.take());
        if let Some(session) = self.session.take() {
            // Closing is a D-Bus call; nothing waits on it.
            let _ = std::thread::Builder::new().name("horo screen close".into()).spawn(move || {
                if let Err(e) = futures_lite::future::block_on(session.close()) {
                    tracing::debug!("closing the screen cast session: {e}");
                }
            });
        }
    }
}
