//! Pictures from `mxc://` URIs: avatars, image attachments, thumbnails.
//!
//! [`Media::image`] answers at once with what it has. On a miss it asks
//! horo-core for the bytes (from the SDK's cache when it can, decrypted for
//! encrypted rooms) and redraws the windows when they arrive, so views just
//! ask again on their next render. Failures are remembered for a minute,
//! the way horo-core remembers them, so a broken avatar isn't refetched on
//! every frame.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{App, Global, Image, ImageFormat};
use horo_core::matrix::media;

use crate::backend::Backend;

/// How many pictures stay decoded-ready in memory.
const KEEP: usize = 400;
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
    entries: HashMap<Key, Entry>,
    order: VecDeque<Key>,
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
        match media.entries.get(&key) {
            Some(Entry::Ready(image)) => return Some(image.clone()),
            Some(Entry::Loading) => return None,
            Some(Entry::Failed(at)) if at.elapsed() < RETRY_AFTER => return None,
            _ => {}
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
        if self.entries.insert(key.clone(), entry).is_none() {
            self.order.push_back(key);
            while self.order.len() > KEEP {
                if let Some(old) = self.order.pop_front() {
                    self.entries.remove(&old);
                }
            }
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
    fn pictures_shrink_to_fit_and_keep_their_shape() {
        assert_eq!(fit(Some(1600), Some(1200), (400., 300.), 48.), (400., 300.));
        assert_eq!(fit(Some(1200), Some(1600), (400., 300.), 48.), (225., 300.));
        assert_eq!(fit(Some(100), Some(50), (400., 300.), 48.), (100., 50.));
        assert_eq!(fit(Some(20), Some(10), (400., 300.), 48.), (96., 48.));
        assert_eq!(fit(None, Some(10), (400., 300.), 48.), (320., 240.));
    }
}
