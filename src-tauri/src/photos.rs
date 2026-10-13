//! The photo library, on the platforms that have one.
//!
//! A WebView can't enumerate photos — `<input type="file">` runs the system
//! picker out of process and returns only the chosen file. That's the right
//! privacy default, and it's why showing a grid of recent photos inside the app
//! needs native code. The iOS half lives in `gen/apple/Sources/horo/main.mm`;
//! this is the Rust side of that bridge.
//!
//! Exports land in the temp directory as ordinary files, so the picked photo
//! rejoins the *existing* upload path — `send_attachment` with a path — rather
//! than needing a second one that carries bytes.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{Error, Result};

/// One item in the library, with a thumbnail small enough to hand to the
/// WebView inline.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Photo {
    pub id: String,
    /// Videos get a duration badge and a longer export.
    pub video: bool,
    pub seconds: i64,
    /// A `data:image/jpeg;base64,…` URI. Small enough to inline, and it avoids
    /// a second round trip per tile just to fetch the image.
    pub thumb: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RecentPhotos {
    /// False where there is no library to read — every desktop platform.
    pub supported: bool,
    /// True when the user granted access to *some* photos. Not an error: the
    /// list is what they chose to share, and the UI offers a way to widen it.
    pub limited: bool,
    /// True when access was refused outright, so the UI can say so rather than
    /// showing an empty grid that looks broken.
    pub denied: bool,
    pub photos: Vec<Photo>,
}

impl RecentPhotos {
    fn unsupported() -> Self {
        Self { supported: false, limited: false, denied: false, photos: Vec::new() }
    }
}

#[cfg(target_os = "ios")]
mod ios {
    use std::ffi::{CStr, CString, c_char, c_int};
    use std::sync::OnceLock;

    use super::{Error, Photo, RecentPhotos, Result};

    /*
     * The app calls *us*, not the other way round.
     *
     * The obvious shape — `extern "C"` declarations here, implementations in
     * `main.mm` — does not link. Cargo builds this crate as a dylib and links
     * it on its own, long before the app's own object files exist, so any
     * symbol defined over in `main.mm` is simply undefined at that point. The
     * dependency runs one way: the app links the Rust library, which is how
     * `ffi::start_app` is reachable from `main`.
     *
     * So the bridge is registered rather than imported. `main` hands us three
     * function pointers before starting the app, and we keep them here. It also
     * means a build with no photo support isn't a link error — it's an empty
     * `OnceLock`, which the commands report as "unsupported".
     */
    type RecentFn = unsafe extern "C" fn(c_int, c_int) -> *mut c_char;
    type ExportFn = unsafe extern "C" fn(*const c_char) -> *mut c_char;
    type FreeFn = unsafe extern "C" fn(*mut c_char);

    struct Bridge {
        recent: RecentFn,
        export: ExportFn,
        free: FreeFn,
    }

    static BRIDGE: OnceLock<Bridge> = OnceLock::new();

    /// Called once from `main.mm`, before the app starts.
    ///
    /// # Safety
    /// The three pointers must be the PhotoKit entry points from `main.mm`, and
    /// strings from `recent`/`export` must be freeable by `free`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn horo_register_photo_bridge(
        recent: RecentFn,
        export: ExportFn,
        free: FreeFn,
    ) {
        let _ = BRIDGE.set(Bridge { recent, export, free });
    }

    fn bridge() -> Result<&'static Bridge> {
        BRIDGE.get().ok_or_else(|| Error::Other("the photo library isn't available".into()))
    }

    /// Take ownership of a string from the bridge, freeing the original.
    ///
    /// It was allocated by `strdup` on the other side, so it goes back through
    /// the bridge's own `free` rather than Rust's allocator.
    fn take(bridge: &Bridge, pointer: *mut c_char) -> Result<String> {
        if pointer.is_null() {
            return Err(Error::Other("the photo library returned nothing".into()));
        }
        // SAFETY: the bridge only returns NUL-terminated `strdup`ed buffers,
        // and this is the only place that frees them.
        let owned = unsafe { CStr::from_ptr(pointer) }.to_string_lossy().into_owned();
        unsafe { (bridge.free)(pointer) };
        Ok(owned)
    }

    #[derive(serde::Deserialize)]
    struct RecentPayload {
        status: String,
        #[serde(default)]
        assets: Vec<Photo>,
    }

    #[derive(serde::Deserialize)]
    struct ExportPayload {
        status: String,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        message: Option<String>,
    }

    pub fn recent(limit: u32, thumb_px: u32) -> Result<RecentPhotos> {
        let bridge = bridge()?;
        let json = take(bridge, unsafe { (bridge.recent)(limit as c_int, thumb_px as c_int) })?;
        let payload: RecentPayload = serde_json::from_str(&json)?;

        Ok(RecentPhotos {
            supported: true,
            limited: payload.status == "limited",
            denied: payload.status == "denied",
            photos: payload.assets,
        })
    }

    pub fn export(id: &str) -> Result<String> {
        let bridge = bridge()?;
        let c_id = CString::new(id).map_err(|_| Error::Other("bad photo id".into()))?;
        let json = take(bridge, unsafe { (bridge.export)(c_id.as_ptr()) })?;
        let payload: ExportPayload = serde_json::from_str(&json)?;

        match (payload.status.as_str(), payload.path) {
            ("ok", Some(path)) => Ok(path),
            _ => Err(Error::Other(
                payload.message.unwrap_or_else(|| "couldn't open that photo".into()),
            )),
        }
    }
}

/// The newest items in the library, with thumbnails.
///
/// Blocking: the bridge waits on PhotoKit callbacks, so it must not run on the
/// main thread. `spawn_blocking` also keeps a slow first call — authorisation,
/// or a library with a lot in it — off the async runtime's worker threads.
#[tauri::command]
pub async fn photos_recent(limit: u32) -> Result<RecentPhotos> {
    #[cfg(target_os = "ios")]
    {
        tauri::async_runtime::spawn_blocking(move || ios::recent(limit, 200))
            .await
            .map_err(|e| Error::Other(e.to_string()))?
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = limit;
        Ok(RecentPhotos::unsupported())
    }
}

/// Copy one asset out of the library and return the file's path.
#[tauri::command]
pub async fn photos_export(id: String) -> Result<String> {
    #[cfg(target_os = "ios")]
    {
        tauri::async_runtime::spawn_blocking(move || ios::export(&id))
            .await
            .map_err(|e| Error::Other(e.to_string()))?
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = id;
        Err(Error::Other("there's no photo library on this platform".into()))
    }
}
