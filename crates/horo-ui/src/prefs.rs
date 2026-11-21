//! The app's own settings (as opposed to the account's): a JSON file in the
//! data directory. Fields missing from an older file take their defaults,
//! and an unreadable file is the defaults, so a bad file never stops the app.
//! The WebView build's equivalents live in src/lib/settings.ts.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Prefs {
    /// Banners and a sound for messages your push rules say are worth it.
    pub notify_messages: bool,
    /// Banners and a sound when a call starts in one of your rooms.
    pub notify_calls: bool,
    /// Notify for the room you're already reading, too.
    pub notify_when_focused: bool,
    /// A [`crate::sounds::MESSAGE_SOUNDS`] id, or `none`.
    pub message_sound: String,
    /// A [`crate::sounds::CALL_SOUNDS`] id, or `none`.
    pub call_sound: String,
    /// 0 to 1.
    pub notification_volume: f32,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            notify_messages: true,
            notify_calls: true,
            notify_when_focused: false,
            message_sound: "blip".into(),
            call_sound: "ring".into(),
            notification_volume: 0.7,
        }
    }
}

fn file(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

impl Prefs {
    pub fn load(data_dir: &Path) -> Prefs {
        match std::fs::read(file(data_dir)) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!("settings.json unreadable, using defaults: {e}");
                Prefs::default()
            }),
            Err(_) => Prefs::default(),
        }
    }

    /// Written whole, through a temporary file, so a crash mid-write can't
    /// leave half a file behind.
    pub fn save(&self, data_dir: &Path) {
        let result = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(data_dir)?;
            let tmp = data_dir.join("settings.json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
            std::fs::rename(tmp, file(data_dir))
        })();
        if let Err(e) = result {
            tracing::warn!("couldn't save settings: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults_and_a_round_trip_keeps_the_rest() {
        let dir = std::env::temp_dir().join(format!("horo-prefs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(file(&dir), br#"{"messageSound":"pop","unknownThing":1}"#).unwrap();
        let prefs = Prefs::load(&dir);
        assert_eq!(prefs.message_sound, "pop");
        assert!(prefs.notify_messages);

        let changed = Prefs { notification_volume: 0.25, ..prefs };
        changed.save(&dir);
        assert_eq!(Prefs::load(&dir), changed);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_broken_file_is_the_defaults() {
        let dir = std::env::temp_dir().join(format!("horo-prefs-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(file(&dir), b"{not json").unwrap();
        assert_eq!(Prefs::load(&dir), Prefs::default());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
