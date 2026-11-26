//! Pictures from `mxc://` URIs: avatars, image attachments, thumbnails.
//!
//! [`Media::image`] answers at once with what it has. On a miss it asks
//! horo-core for the bytes (from the SDK's cache when it can, decrypted for
//! encrypted rooms) and redraws the windows when they arrive, so views just
//! ask again on their next render. Failures are remembered for a minute,
//! the way horo-core remembers them, so a broken avatar isn't refetched on
//! every frame.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{App, Global, Image, ImageFormat};
use horo_core::matrix::media;

use crate::backend::Backend;

/// How many pictures stay decoded-ready in memory. Past this, the least
/// recently drawn go first; one still loading is never dropped, or a
/// screenful bigger than this would refetch itself forever.
const KEEP: usize = 800;
const RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    mxc: String,
    size: Option<(u32, u32)>,
}

/// The share of see-through pixels that makes an image a cut-out (sticker-like
/// transparent PNG/WebP/GIF drawn frameless, matching `src/lib/cutout.ts`).
const CUTOUT_SHARE: f32 = 0.04;
/// Below this alpha a pixel counts as see-through.
const SEE_THROUGH: u8 = 128;

enum Entry {
    Loading,
    Ready { image: Arc<Image>, cutout: bool },
    Failed(Instant),
}

#[derive(Default)]
pub struct Media {
    /// Each with when it was last asked for, on [`Media::clock`].
    entries: HashMap<Key, (Entry, u64)>,
    clock: u64,
}

impl Global for Media {}

impl Media {
    /// The picture at `mxc` (a server thumbnail at `size`, when given), if
    /// it's here yet. Starts fetching it otherwise.
    pub fn image(mxc: &str, size: Option<(u32, u32)>, cx: &mut App) -> Option<Arc<Image>> {
        Self::image_with_cutout(mxc, size, cx).map(|(image, _)| image)
    }

    /// As [`Media::image`], and whether the picture is a transparent cut-out
    /// (drawn frameless, like a sticker).
    pub fn image_with_cutout(mxc: &str, size: Option<(u32, u32)>, cx: &mut App) -> Option<(Arc<Image>, bool)> {
        if !mxc.starts_with("mxc://") {
            return None;
        }
        let key = Key { mxc: mxc.to_owned(), size };
        let media = cx.default_global::<Media>();
        media.clock += 1;
        let now = media.clock;
        if let Some((entry, used)) = media.entries.get_mut(&key) {
            *used = now;
            match entry {
                Entry::Ready { image, cutout } => return Some((image.clone(), *cutout)),
                Entry::Loading => return None,
                Entry::Failed(at) if at.elapsed() < RETRY_AFTER => return None,
                Entry::Failed(_) => {}
            }
        }
        media.insert(key.clone(), Entry::Loading);

        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, mxc) = (backend.state.clone(), key.mxc.clone());
            async move {
                let core = state.core().await?;
                let fetched = media::fetch(&core.client, &mxc, size, None).await?;
                // Check alpha off the UI thread so transparent cut-outs render
                // frameless without stalling painting.
                let cutout = is_cutout_bytes(&fetched.mime, &fetched.bytes);
                Ok::<_, horo_core::Error>((fetched, cutout))
            }
        });
        cx.spawn(async move |cx| {
            let entry = match task.await {
                Ok(Ok((fetched, cutout))) => match format(&fetched.mime) {
                    Some(format) => Entry::Ready { image: Arc::new(Image::from_bytes(format, fetched.bytes)), cutout },
                    None => {
                        tracing::debug!("{} is {}, not a picture", key.mxc, fetched.mime);
                        Entry::Failed(Instant::now())
                    }
                },
                Ok(Err(e)) => {
                    tracing::debug!("couldn't load {}: {e}", key.mxc);
                    Entry::Failed(Instant::now())
                }
                Err(e) => {
                    tracing::debug!("couldn't load {}: {e}", key.mxc);
                    Entry::Failed(Instant::now())
                }
            };
            cx.update(|cx| {
                cx.default_global::<Media>().insert(key, entry);
                cx.refresh_windows();
            });
        })
        .detach();
        None
    }

    fn insert(&mut self, key: Key, entry: Entry) {
        let used = self.entries.get(&key).map_or(self.clock, |(_, used)| *used);
        let fresh = self.entries.insert(key, (entry, used)).is_none();
        if fresh && self.entries.len() > KEEP {
            self.evict();
        }
    }

    /// Down to nine tenths of [`KEEP`], least recently asked for first, so
    /// this runs once per many inserts rather than on every one.
    fn evict(&mut self) {
        let mut idle: Vec<(u64, Key)> =
            self.entries.iter().filter(|(_, (entry, _))| !matches!(entry, Entry::Loading)).map(|(key, (_, used))| (*used, key.clone())).collect();
        idle.sort_unstable_by_key(|(used, _)| *used);
        let excess = self.entries.len().saturating_sub(KEEP * 9 / 10);
        for (_, key) in idle.into_iter().take(excess) {
            self.entries.remove(&key);
        }
    }
}

/// Whether encoded image bytes are a transparent cut-out (at least
/// [`CUTOUT_SHARE`] of pixels see-through). Only alpha-capable formats are
/// decoded; failures return `false` so photos keep their frame.
pub fn is_cutout_bytes(mime: &str, bytes: &[u8]) -> bool {
    let fmt = match mime {
        "image/png" | "image/apng" => image::ImageFormat::Png,
        "image/webp" => image::ImageFormat::WebP,
        "image/gif" => image::ImageFormat::Gif,
        _ => return false,
    };
    let Ok(decoded) = image::load_from_memory_with_format(bytes, fmt) else {
        return false;
    };
    if !decoded.color().has_alpha() {
        return false;
    }
    let rgba = decoded.into_rgba8();
    is_cutout_rgba(rgba.as_raw(), rgba.width(), rgba.height())
}

/// Whether RGBA pixels (`w`×`h`) have enough see-through pixels to count as a
/// cut-out. Samples a grid of up to 48×48 points so large images stay fast.
pub fn is_cutout_rgba(rgba: &[u8], width: u32, height: u32) -> bool {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return false;
    }
    let step_x = (w / 48).max(1);
    let step_y = (h / 48).max(1);
    let mut total = 0usize;
    let mut clear = 0usize;
    let mut y = 0;
    while y < h {
        let row = y * w * 4;
        let mut x = 0;
        while x < w {
            if rgba[row + x * 4 + 3] < SEE_THROUGH {
                clear += 1;
            }
            total += 1;
            x += step_x;
        }
        y += step_y;
    }
    total > 0 && (clear as f32 / total as f32) >= CUTOUT_SHARE
}

/// The formats gpui draws, by the type horo-core sniffed.
fn format(mime: &str) -> Option<ImageFormat> {
    Some(match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        _ => return None,
    })
}

/// `width`×`height` scaled down to fit `max`, never up, never below `min`
/// on its short side.
pub fn fit(width: Option<u64>, height: Option<u64>, max: (f32, f32), min: f32) -> (f32, f32) {
    let (w, h) = match (width, height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as f32, h as f32),
        _ => return (max.0.min(320.), max.1.min(240.)),
    };
    let scale = (max.0 / w).min(max.1 / h).min(1.0);
    let (w, h) = (w * scale, h * scale);
    let grow = (min / w.min(h)).max(1.0);
    ((w * grow).min(max.0), (h * grow).min(max.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_drops_the_least_recently_used_and_keeps_what_is_loading() {
        let mut media = Media::default();
        let key = |i: usize| Key { mxc: format!("mxc://x/{i}"), size: None };
        media.insert(key(0), Entry::Loading);
        for i in 1..=KEEP {
            media.clock += 1;
            media.insert(key(i), Entry::Failed(Instant::now()));
        }
        assert!(media.entries.len() <= KEEP);
        assert!(media.entries.contains_key(&key(0)), "a picture still loading stays");
        assert!(media.entries.contains_key(&key(KEEP)), "the newest stays");
        assert!(!media.entries.contains_key(&key(1)), "the oldest goes");
    }

    #[test]
    fn pictures_shrink_to_fit_and_keep_their_shape() {
        assert_eq!(fit(Some(1600), Some(1200), (400., 300.), 48.), (400., 300.));
        assert_eq!(fit(Some(1200), Some(1600), (400., 300.), 48.), (225., 300.));
        assert_eq!(fit(Some(100), Some(50), (400., 300.), 48.), (100., 50.));
        assert_eq!(fit(Some(20), Some(10), (400., 300.), 48.), (96., 48.));
        assert_eq!(fit(None, Some(10), (400., 300.), 48.), (320., 240.));
    }

    #[test]
    fn transparent_images_are_detected_as_cutouts() {
        // Opaque 10x10 image: not a cut-out.
        let opaque = vec![255u8; 10 * 10 * 4];
        assert!(!is_cutout_rgba(&opaque, 10, 10));

        // 3% see-through (3 of 100 pixels): still a photo (e.g. rounded screenshot).
        let mut slight = opaque.clone();
        for p in 0..3 {
            slight[p * 4 + 3] = 0;
        }
        assert!(!is_cutout_rgba(&slight, 10, 10));

        // 4% see-through (4 of 100 pixels): cut-out.
        slight[3 * 4 + 3] = 0;
        assert!(is_cutout_rgba(&slight, 10, 10));

        // Real encoded PNG with transparency vs JPEG (never decoded).
        let mut buf = Vec::new();
        let img = image::RgbaImage::from_fn(8, 8, |x, _| if x == 0 { image::Rgba([0, 0, 0, 0]) } else { image::Rgba([255, 0, 0, 255]) });
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).unwrap();
        assert!(is_cutout_bytes("image/png", &buf));
        assert!(!is_cutout_bytes("image/jpeg", &buf));
    }
}
