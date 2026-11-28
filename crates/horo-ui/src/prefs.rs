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
    /// The microphone for calls (a `horo_rtc::devices` id); `None` follows
    /// the system default.
    pub audio_input: Option<String>,
    /// The noise gate's threshold in dBFS: the microphone is silenced
    /// while it's quieter. At `GATE_OFF_DB` (the default) nothing is cut,
    /// as a threshold set wrong cuts people off mid-word.
    pub input_sensitivity: f32,
    /// The speaker for calls; `None` follows the system default.
    pub audio_output: Option<String>,
    /// The camera's unique ID; `None` is the system's preferred one.
    pub video_input: Option<String>,
    /// The room info panel beside the timeline.
    pub show_info_panel: bool,
    /// The room list's width, and the info panel's, as dragged.
    pub room_list_width: f32,
    pub info_panel_width: f32,
    /// The whole window's zoom, as a factor (1 is the design's size).
    pub zoom: f32,
    /// The look: theme, accent, shade and contrast; a font to read in
    /// (empty for the theme's); the message text size in px.
    pub theme: crate::theme::ThemeName,
    pub accent: crate::theme::Accent,
    pub shade: crate::theme::Shade,
    pub contrast: crate::theme::Contrast,
    pub font_family: String,
    pub message_font_size: f32,
    /// The release whose notice was waved away (a newer one speaks up).
    pub dismissed_update: Option<String>,
    /// 0 (default yellow) to 5, for emoji that take one.
    pub skin_tone: u8,
    /// The reactions you reach for, newest first: the hover bar's quick
    /// row and the picker's "recent".
    pub recent_reactions: Vec<String>,
    /// Spaces whose muted rooms are left out of the list, by id.
    pub hide_muted_in_spaces: Vec<String>,
    /// Custom left-rail order of space IDs (Discord-style server ordering).
    pub space_order: Vec<String>,
    /// Show OpenGraph link preview cards under pasted URLs.
    pub link_previews: bool,
    /// Private per-user notes (Discord-style), keyed by Matrix user ID.
    pub user_notes: std::collections::HashMap<String, String>,
    /// Cached custom status text (synced with MSC4133 `chat.commet.profile_status`).
    pub custom_status: String,
}

/// How many recent reactions are kept.
/// The columns' widths to start with, and how far they can be dragged.
pub const ROOM_LIST_WIDTH: f32 = 298.;
pub const INFO_PANEL_WIDTH: f32 = 296.;
pub const PANE_MIN: f32 = 240.;
pub const PANE_MAX: f32 = 420.;

impl Prefs {
    /// The appearance settings, as the theme takes them.
    pub fn appearance(&self) -> crate::theme::Appearance {
        crate::theme::Appearance {
            theme: self.theme,
            accent: self.accent,
            shade: self.shade,
            contrast: self.contrast,
            font_family: self.font_family.clone(),
            message_font_size: self.message_font_size,
        }
    }
}

pub const MAX_RECENT_REACTIONS: usize = 6;

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            notify_messages: true,
            notify_calls: true,
            notify_when_focused: false,
            message_sound: "blip".into(),
            call_sound: "ring".into(),
            notification_volume: 0.7,
            room_list_width: ROOM_LIST_WIDTH,
            info_panel_width: INFO_PANEL_WIDTH,
            dismissed_update: None,
            zoom: 1.,
            theme: Default::default(),
            accent: Default::default(),
            shade: Default::default(),
            contrast: Default::default(),
            font_family: String::new(),
            message_font_size: crate::theme::MESSAGE_SIZE_DEFAULT,
            audio_input: None,
            input_sensitivity: horo_rtc::audio::gate::GATE_OFF_DB,
            video_input: None,
            audio_output: None,
            show_info_panel: true,
            skin_tone: 0,
            recent_reactions: ["💜", "😹", "🥺", "✨", "👀", "🔥"].map(String::from).to_vec(),
            hide_muted_in_spaces: Vec::new(),
            space_order: Vec::new(),
            link_previews: true,
            user_notes: std::collections::HashMap::new(),
            custom_status: String::new(),
        }
    }
}

fn file(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

impl Prefs {
    /// `key` was just reacted with: to the front of the recent ones.
    pub fn note_reaction(&mut self, key: &str) {
        self.recent_reactions.retain(|k| k != key);
        self.recent_reactions.insert(0, key.to_owned());
        self.recent_reactions.truncate(MAX_RECENT_REACTIONS);
    }

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
    fn a_reaction_moves_to_the_front_and_the_list_stays_short() {
        let mut prefs = Prefs::default();
        prefs.note_reaction("🔥");
        assert_eq!(prefs.recent_reactions[0], "🔥");
        assert_eq!(prefs.recent_reactions.len(), MAX_RECENT_REACTIONS);
        prefs.note_reaction("🐸");
        assert_eq!(prefs.recent_reactions[..2], ["🐸".to_owned(), "🔥".to_owned()]);
        assert_eq!(prefs.recent_reactions.len(), MAX_RECENT_REACTIONS);
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
