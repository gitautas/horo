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

enum Entry {
    Loading,
    Ready(Arc<Image>),
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
                Entry::Ready(image) => return Some(image.clone()),
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
                media::fetch(&core.client, &mxc, size, None).await
            }
        });
        cx.spawn(async move |cx| {
            let entry = match task.await {
                Ok(Ok(fetched)) => match format(&fetched.mime) {
                    Some(format) => Entry::Ready(Arc::new(Image::from_bytes(format, fetched.bytes))),
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
}
