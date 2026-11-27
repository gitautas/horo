//! horo-core inside a gpui app.
//!
//! The core is async on tokio; gpui runs its own executor. [`Backend`] owns a
//! tokio runtime, and [`Backend::run`] hands a future to it and gives back
//! something gpui can await: views start core calls from their own tasks and
//! update themselves with the result. horo-core's [`Event`]s cross the other
//! way through a channel that [`crate::store::Store`] drains on gpui's side.

use std::{future::Future, path::PathBuf, sync::Arc};

use gpui::{App, Global};
use horo_core::{AppState, Event, EventSink, Host};
use tokio::sync::mpsc;

/// The core's side of the bridge: events into a channel, URLs to the OS.
struct ChannelHost {
    events: mpsc::UnboundedSender<Event>,
}

impl EventSink for ChannelHost {
    fn emit(&self, event: Event) {
        // The window being gone is no reason to fail a sync task.
        let _ = self.events.send(event);
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        open_url(url)
    }
}

/// Open a URL in the system browser (SSO sends people there).
pub fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");
    command.arg(url).spawn().map(|_| ()).map_err(|e| format!("couldn't open {url}: {e}"))
}

pub struct Backend {
    runtime: Arc<tokio::runtime::Runtime>,
    pub state: Arc<AppState>,
    pub data_dir: PathBuf,
    pub host: Host,
    events: std::sync::Mutex<Option<mpsc::UnboundedReceiver<Event>>>,
}

impl Global for Backend {}

impl Backend {
    /// A runtime of its own, and the core's state rooted at `data_dir`.
    pub fn new(data_dir: PathBuf) -> std::io::Result<Backend> {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("horo-core").build()?;
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        Ok(Backend {
            runtime: Arc::new(runtime),
            state: Arc::new(AppState::default()),
            data_dir,
            host: Arc::new(ChannelHost { events: events_tx }),
            events: std::sync::Mutex::new(Some(events_rx)),
        })
    }

    /// Where the app keeps its session and stores, per platform: the
    /// platform's app-data directory under the bundle identifier, where
    /// the Tauri app kept them, so an install updated from it carries on
    /// signed in with its keys. `HORO_DATA` points elsewhere (dev builds do,
    /// so they never share the installed app's store).
    pub fn default_data_dir() -> PathBuf {
        const IDENTIFIER: &str = "lt.uwu.uwum";
        let base = std::env::var_os("HORO_DATA").map(PathBuf::from);
        base.unwrap_or_else(|| {
            let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
            #[cfg(target_os = "macos")]
            let dir = home.join("Library/Application Support");
            #[cfg(target_os = "windows")]
            let dir = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or(home);
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let dir = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
            dir.join(IDENTIFIER)
        })
    }

    /// Run `future` on the core's runtime; await the result from gpui.
    pub fn run<F, T>(&self, future: F) -> impl Future<Output = anyhow::Result<T>> + use<F, T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let handle = self.runtime.spawn(future);
        async move { handle.await.map_err(|e| anyhow::anyhow!("core task failed: {e}")) }
    }

    /// Run `future` on the core's runtime for as long as it takes, or until
    /// the returned handle aborts it (a call's membership refresh).
    pub fn spawn<F>(&self, future: F) -> tokio::task::AbortHandle
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.runtime.spawn(future).abort_handle()
    }

    /// The event stream, once: the store takes it.
    pub fn take_events(&self) -> Option<mpsc::UnboundedReceiver<Event>> {
        self.events.lock().ok()?.take()
    }

    pub fn global(cx: &App) -> &Backend {
        cx.global::<Backend>()
    }
}
