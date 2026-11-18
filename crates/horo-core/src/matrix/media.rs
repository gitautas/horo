//! Serving `mxc://` content to the WebView.
//!
//! Avatars and image attachments are far too numerous to shuttle through IPC as
//! base64 — a busy room would push megabytes per render. Instead the app
//! registers a `horo://` URI scheme, so the frontend can write
//! `<img src="horo://media/<url-encoded mxc>?w=64&h=64">` and let the WebView
//! handle caching and lazy loading like it would for any other image.
//!
//! Requests are served from the SDK's media cache when possible, so scrolling a
//! timeline twice doesn't hit the homeserver twice.

use std::{
    collections::{HashMap, VecDeque},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use matrix_sdk::{
    Client,
    media::{MediaFormat, MediaRequestParameters, MediaThumbnailSettings},
    ruma::{
        MxcUri, UInt, api::client::media::get_content_thumbnail::v3::Method,
        events::room::MediaSource,
    },
};

use crate::error::{Error, Result};

/// Known media sources, keyed by `mxc://` URI.
///
/// In an encrypted room an attachment's bytes are ciphertext, and the keys to
/// open them live in the *event*, not in the URI. A `horo://` request carries
/// only the URI, so rebuilding a `MediaSource::Plain` from it downloads the
/// ciphertext and hands the WebView noise — which is exactly what a video that
/// refuses to play looks like.
///
/// Timeline conversion sees the real `MediaSource`, so it records it here and
/// the protocol handler looks it up. Registration always happens before the
/// WebView can request the URL, because the URL is built from the same
/// conversion.
static SOURCES: LazyLock<Mutex<SourceRegistry>> =
    LazyLock::new(|| Mutex::new(SourceRegistry::default()));

/// Enough for any plausible scrollback; old entries fall off the back. A
/// forgotten source degrades to an unencrypted fetch, the same as before.
const MAX_REMEMBERED_SOURCES: usize = 4096;

#[derive(Default)]
struct SourceRegistry {
    by_uri: HashMap<String, MediaSource>,
    order: VecDeque<String>,
}

/// Record how to fetch (and decrypt) this media, keyed by its URI.
pub fn remember_source(source: &MediaSource) {
    let uri = match source {
        MediaSource::Plain(uri) => uri.to_string(),
        MediaSource::Encrypted(file) => file.url.to_string(),
    };

    let Ok(mut registry) = SOURCES.lock() else { return };
    if registry.by_uri.insert(uri.clone(), source.clone()).is_none() {
        registry.order.push_back(uri);
        while registry.order.len() > MAX_REMEMBERED_SOURCES {
            if let Some(oldest) = registry.order.pop_front() {
                registry.by_uri.remove(&oldest);
            }
        }
    }
}

fn remembered_source(mxc: &str) -> Option<MediaSource> {
    SOURCES.lock().ok()?.by_uri.get(mxc).cloned()
}

/// Ceiling on requested thumbnail dimensions. Anything larger is served as the
/// full file instead — a client asking for a 20000px thumbnail is either
/// confused or hostile, and homeservers charge real CPU for the resize.
const MAX_THUMBNAIL_DIMENSION: u32 = 2048;

pub struct FetchedMedia {
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// Requests the server has recently refused, and when it refused them.
///
/// Remote media that the homeserver can't federate fails *slowly* — a ten
/// second timeout, then `M_UNKNOWN`. Without this, every re-render of a room
/// with a few such avatars queues another ten seconds of doomed requests, and
/// they compete with the fetches that would have worked. A short memory turns
/// the second attempt into an instant failure and lets the good media through.
///
/// Keyed by URI **and size**, not by URI. A failure is not always a property of
/// the media: a size the server won't produce says nothing about a size it
/// already has cached, and keying on the URI alone means one bad request blanks
/// an avatar that was rendering perfectly well somewhere else.
static FAILURES: LazyLock<Mutex<HashMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Long enough to stop a render loop from re-asking, short enough that a
/// server which has since fetched the file gets another chance quickly.
const FAILURE_MEMORY: Duration = Duration::from_secs(60);

/// Bound on the failure map, so a long session can't grow it without limit.
const MAX_REMEMBERED_FAILURES: usize = 1024;

fn failure_key(mxc: &str, size: Option<(u32, u32)>) -> String {
    match size {
        Some((w, h)) => format!("{mxc}|{w}x{h}"),
        None => format!("{mxc}|full"),
    }
}

fn recently_failed(key: &str) -> bool {
    let Ok(failures) = FAILURES.lock() else { return false };
    failures.get(key).is_some_and(|at| at.elapsed() < FAILURE_MEMORY)
}

fn remember_failure(key: &str) {
    let Ok(mut failures) = FAILURES.lock() else { return };
    failures.retain(|_, at| at.elapsed() < FAILURE_MEMORY);
    if failures.len() < MAX_REMEMBERED_FAILURES {
        failures.insert(key.to_owned(), Instant::now());
    }
}

fn forget_failure(key: &str) {
    if let Ok(mut failures) = FAILURES.lock() {
        failures.remove(key);
    }
}

/// Fetch media by `mxc://` URI, optionally as a thumbnail of the given size.
pub async fn fetch(
    client: &Client,
    mxc: &str,
    size: Option<(u32, u32)>,
    mime_hint: Option<&str>,
) -> Result<FetchedMedia> {
    let uri = <&MxcUri>::from(mxc);
    // Reject anything that isn't a well-formed mxc URI before it reaches the
    // network layer: this input comes from the WebView, so it is untrusted.
    if uri.parts().is_err() {
        return Err(Error::Other(format!("not a valid mxc uri: {mxc}")));
    }

    // Prefer the source the timeline recorded — it carries the decryption keys
    // for attachments in encrypted rooms.
    let source =
        remembered_source(mxc).unwrap_or_else(|| MediaSource::Plain(uri.to_owned()));
    let encrypted = matches!(source, MediaSource::Encrypted(_));

    let failure_key = failure_key(mxc, size);
    if recently_failed(&failure_key) {
        return Err(Error::Other(format!(
            "the server couldn't produce {mxc} at this size a moment ago; \
             not asking again yet"
        )));
    }

    let format = match size {
        // Server-side thumbnails are impossible for encrypted media: the server
        // only has ciphertext to resize. Always take the whole file and let the
        // WebView scale it.
        Some((w, h))
            if !encrypted && w <= MAX_THUMBNAIL_DIMENSION && h <= MAX_THUMBNAIL_DIMENSION =>
        {
            MediaFormat::Thumbnail(MediaThumbnailSettings {
                method: Method::Scale,
                width: UInt::from(w),
                height: UInt::from(h),
                animated: true,
            })
        }
        _ => MediaFormat::File,
    };

    let wanted_thumbnail = matches!(format, MediaFormat::Thumbnail(_));
    let request = MediaRequestParameters { source: source.clone(), format };

    let bytes = match client.media().get_media_content(&request, true).await {
        Ok(bytes) => bytes,

        // A thumbnail can fail where the original succeeds: the server resizes
        // on demand, and for media it hasn't federated yet that resize is what
        // times out. It often still has — or can still fetch — the whole file,
        // so ask for that before giving up. The WebView scales it for us.
        Err(thumbnail_error) if wanted_thumbnail => {
            tracing::debug!("thumbnail failed for {mxc}, trying the original: {thumbnail_error}");

            let whole = MediaRequestParameters { source, format: MediaFormat::File };
            match client.media().get_media_content(&whole, true).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    remember_failure(&failure_key);
                    return Err(Error::Other(format!("couldn't load media: {error}")));
                }
            }
        }

        Err(error) => {
            remember_failure(&failure_key);
            return Err(Error::Other(format!("couldn't load media: {error}")));
        }
    };

    forget_failure(&failure_key);

    // Homeservers don't hand back a content type through this API, so sniff the
    // magic bytes and fall back to the caller's hint. Serving the wrong type
    // would either break rendering or, worse, let a file be interpreted as
    // something executable by the WebView.
    let mime = sniff_mime(&bytes)
        .map(str::to_owned)
        .or_else(|| mime_hint.map(str::to_owned))
        .unwrap_or_else(|| "application/octet-stream".to_owned());

    Ok(FetchedMedia { bytes, mime })
}

/// Identify the handful of formats a chat client actually renders inline.
/// Anything unrecognised stays `application/octet-stream`, which the WebView
/// will download rather than execute.
fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        [0x00, 0x00, 0x00, _, b'f', b't', b'y', b'p', ..] => Some("video/mp4"),
        [b'O', b'g', b'g', b'S', ..] => Some("audio/ogg"),
        [b'I', b'D', b'3', ..] | [0xFF, 0xFB, ..] => Some("audio/mpeg"),
        _ if bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml") => {
            // SVG can carry script, so never serve it as an image; let it
            // download as an opaque blob instead.
            None
        }
        _ => None,
    }
}

/// Parse a `horo://media/<encoded-mxc>?w=..&h=..&mime=..` request path.
pub struct MediaRequest {
    pub mxc: String,
    pub size: Option<(u32, u32)>,
    pub mime_hint: Option<String>,
}

pub fn parse_request(uri: &str) -> Option<MediaRequest> {
    let parsed = url::Url::parse(uri).ok()?;
    let path = parsed.path();

    // Tauri hands us `horo://media/<mxc>` on macOS and Linux, but
    // `http://horo.localhost/media/<mxc>` on Windows and Android. In the first
    // form `media` parses as the *host* and the mxc is the whole path; in the
    // second the host is the app and `media` is a path segment. Handle both
    // rather than assuming a platform.
    let encoded = match parsed.host_str() {
        Some("media") => path.trim_start_matches('/'),
        _ => path.strip_prefix("/media/")?,
    };

    let mxc = percent_decode(encoded)?;
    if !mxc.starts_with("mxc://") {
        return None;
    }

    let mut width = None;
    let mut height = None;
    let mut mime_hint = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "w" => width = value.parse::<u32>().ok(),
            "h" => height = value.parse::<u32>().ok(),
            "mime" => mime_hint = Some(value.into_owned()),
            _ => {}
        }
    }

    let size = match (width, height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
        _ => None,
    };

    Some(MediaRequest { mxc, size, mime_hint })
}

/// Percent-decode a value the frontend encoded with `encodeURIComponent`.
pub fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// An inclusive byte range, resolved against a known content length.
#[derive(Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    pub fn len(&self) -> u64 {
        self.end - self.start + 1
    }
}

/// Parse a `Range` header.
///
/// WKWebView's media loader will not play a video unless the source supports
/// byte ranges: it probes with `Range: bytes=0-1` and gives up if the response
/// isn't a 206. Serving the whole file with a 200 makes `<video>` silently fail
/// to start, which is exactly what it looked like.
///
/// Only single ranges are handled — multipart ranges are legal but no media
/// element asks for them, and a partial response is always a valid answer.
pub fn parse_range(header: &str, total: u64) -> Option<ByteRange> {
    if total == 0 {
        return None;
    }

    let spec = header.trim().strip_prefix("bytes=")?.trim();
    // A comma means multiple ranges; serve the first and let the client re-ask.
    let spec = spec.split(',').next()?.trim();
    let (start_raw, end_raw) = spec.split_once('-')?;

    let (start, end) = if start_raw.is_empty() {
        // `bytes=-N` — the final N bytes.
        let suffix: u64 = end_raw.trim().parse().ok()?;
        if suffix == 0 {
            return None;
        }
        (total.saturating_sub(suffix), total - 1)
    } else {
        let start: u64 = start_raw.trim().parse().ok()?;
        let end = if end_raw.trim().is_empty() {
            total - 1
        } else {
            end_raw.trim().parse::<u64>().ok()?.min(total - 1)
        };
        (start, end)
    };

    // A start past the end of the content is unsatisfiable.
    if start > end || start >= total {
        return None;
    }

    Some(ByteRange { start, end })
}

/// Download media to a caller-chosen path. Used by "save attachment".
pub async fn save_to(client: &Client, mxc: &str, destination: &str) -> Result<()> {
    let media = fetch(client, mxc, None, None).await?;
    tokio::fs::write(destination, &media.bytes).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MXC: &str = "mxc://uwu.gg/AbCdEf123";

    fn encoded() -> String {
        // What `encodeURIComponent` produces on the JS side.
        MXC.replace(':', "%3A").replace('/', "%2F")
    }

    #[test]
    fn parses_the_macos_and_linux_form() {
        // Here `media` is the URL *host*, and the mxc is the whole path.
        let request = parse_request(&format!("horo://media/{}", encoded())).unwrap();
        assert_eq!(request.mxc, MXC);
        assert_eq!(request.size, None);
    }

    #[test]
    fn parses_the_windows_and_android_form() {
        // Here the host is the app and `media` is a path segment.
        let request =
            parse_request(&format!("http://horo.localhost/media/{}", encoded())).unwrap();
        assert_eq!(request.mxc, MXC);
    }

    #[test]
    fn reads_thumbnail_dimensions_and_mime_hint() {
        let request =
            parse_request(&format!("horo://media/{}?w=64&h=48&mime=image/png", encoded()))
                .unwrap();
        assert_eq!(request.size, Some((64, 48)));
        assert_eq!(request.mime_hint.as_deref(), Some("image/png"));
    }

    #[test]
    fn ignores_degenerate_dimensions() {
        // A zero or half-specified size means "give me the original", not a
        // request for a 0-pixel thumbnail.
        assert_eq!(parse_request(&format!("horo://media/{}?w=0&h=0", encoded())).unwrap().size, None);
        assert_eq!(parse_request(&format!("horo://media/{}?w=64", encoded())).unwrap().size, None);
    }

    #[test]
    fn rejects_anything_that_is_not_an_mxc_uri() {
        // The WebView is untrusted input; this must not become an SSRF vector.
        assert!(parse_request("horo://media/https%3A%2F%2Fevil.example%2Fx").is_none());
        assert!(parse_request("horo://media/%2Fetc%2Fpasswd").is_none());
        assert!(parse_request("horo://other/whatever").is_none());
    }

    #[test]
    fn parses_the_probe_wkwebview_opens_video_with() {
        // The very first thing WKWebView asks for.
        assert_eq!(parse_range("bytes=0-1", 1000), Some(ByteRange { start: 0, end: 1 }));
    }

    #[test]
    fn parses_open_ended_and_suffix_ranges() {
        assert_eq!(parse_range("bytes=500-", 1000), Some(ByteRange { start: 500, end: 999 }));
        assert_eq!(parse_range("bytes=-100", 1000), Some(ByteRange { start: 900, end: 999 }));
    }

    #[test]
    fn clamps_an_end_past_the_content() {
        assert_eq!(parse_range("bytes=0-99999", 1000), Some(ByteRange { start: 0, end: 999 }));
    }

    #[test]
    fn serves_the_first_of_a_multipart_range() {
        // Legal, but no media element asks for it; one range is a valid answer.
        assert_eq!(parse_range("bytes=0-9,20-29", 1000), Some(ByteRange { start: 0, end: 9 }));
    }

    #[test]
    fn rejects_unsatisfiable_and_malformed_ranges() {
        // A start past the end would slice out of bounds — this must never
        // reach the indexing in the protocol handler.
        assert_eq!(parse_range("bytes=1000-1001", 1000), None);
        assert_eq!(parse_range("bytes=900-100", 1000), None);
        assert_eq!(parse_range("bytes=abc-def", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
        assert_eq!(parse_range("bytes=", 1000), None);
        assert_eq!(parse_range("", 1000), None);
        assert_eq!(parse_range("bytes=-0", 1000), None);
        // Nothing to serve from an empty body.
        assert_eq!(parse_range("bytes=0-1", 0), None);
    }

    #[test]
    fn a_full_range_stays_in_bounds() {
        let range = parse_range("bytes=0-", 10).unwrap();
        assert_eq!(range.len(), 10);
        assert_eq!(range.end, 9);
    }

    #[test]
    fn sniffs_only_formats_we_render_inline() {
        assert_eq!(sniff_mime(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]), Some("image/png"));
        assert_eq!(sniff_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        // SVG can carry script, so it must never be served as a renderable image.
        assert_eq!(sniff_mime(b"<svg xmlns=\"http://www.w3.org/2000/svg\">"), None);
        assert_eq!(sniff_mime(b"whatever this is"), None);
    }
}
