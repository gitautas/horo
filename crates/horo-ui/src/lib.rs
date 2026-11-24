//! horo's interface in gpui: the views, the theme, and the bridge between
//! horo-core (async, on tokio) and gpui's own executor.
//!
//! An app calls [`init`] with a [`Backend`], then [`open_window`].

use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

pub mod alerts;
pub mod backend;
#[cfg(target_os = "macos")]
pub mod camera;
pub mod call;
pub mod emoji;
pub mod media;
pub mod mentions;
pub mod packs;
pub mod prefs;
pub mod registration;
pub mod presence;
pub mod rich;
pub mod rooms;
#[cfg(target_os = "macos")]
pub mod screen;
pub mod sounds;
pub mod store;
pub mod theme;
pub mod verify;
#[cfg(target_os = "macos")]
pub mod video;
pub mod views;

pub use backend::Backend;

gpui::actions!(
    horo,
    [
        /// Open settings (⌘, on macOS, Ctrl+, elsewhere).
        OpenSettings,
        Quit,
    ]
);

/// What the app embeds for gpui to load: gpui-component's icon set, and
/// the few Lucide icons horo uses beyond it (assets/icons, ISC licence).
pub fn assets() -> Assets {
    Assets
}

pub struct Assets;

/// horo's own icons, by the path an `Icon` asks for.
macro_rules! own_icons {
    ($($name:literal),* $(,)?) => {
        &[$((concat!("icons/", $name, ".svg"), include_bytes!(concat!("../assets/icons/", $name, ".svg")))),*]
    };
}
const OWN_ICONS: &[(&str, &[u8])] = own_icons![
    "phone", "phone-off", "phone-incoming", "phone-call", "mic-off", "headphones", "compass", "shield-check", "bell-off",
    "video", "lock", "funnel", "circle-plus", "smile", "send", "shield-alert", "badge-check", "badge-question-mark", "lock-open",
    "hand", "cat", "hamburger", "plane", "trophy", "lightbulb", "flag", "search", "orbit", "users", "sticker", "messages-square", "pencil", "trash", "music", "video-off", "mail", "sparkles", "ticket", "screen-share", "screen-share-off",
];

/// The horo wordmark, drawn in the rail.
pub const LOGO: &[u8] = include_bytes!("../assets/logo.svg");

impl gpui::AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match OWN_ICONS.iter().find(|(p, _)| *p == path) {
            Some((_, bytes)) => Ok(Some(std::borrow::Cow::Borrowed(bytes))),
            None => gpui_kit_assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<gpui::SharedString>> {
        let mut all = gpui_kit_assets::Assets.list(path)?;
        all.extend(OWN_ICONS.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| gpui::SharedString::from(*p)));
        Ok(all)
    }
}

/// gpui-component, the theme and the core's backend, once per app.
pub fn init(backend: Backend, cx: &mut App) {
    gpui_component::init(cx);
    theme::install(cx);
    alerts::init();
    cx.set_global(backend);
    // ⌘ on macOS, Ctrl elsewhere. (This gpui has no `secondary-`: it
    // panics on it at startup.)
    #[cfg(target_os = "macos")]
    let (settings, quit) = ("cmd-,", "cmd-q");
    #[cfg(not(target_os = "macos"))]
    let (settings, quit) = ("ctrl-,", "ctrl-q");
    cx.bind_keys([gpui::KeyBinding::new(settings, OpenSettings, None), gpui::KeyBinding::new(quit, Quit, None)]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    // The app menu, where a Mac keeps Settings… and its shortcut.
    cx.set_menus(vec![gpui::Menu {
        name: "horo".into(),
        items: vec![gpui::MenuItem::action("Settings…", OpenSettings), gpui::MenuItem::separator(), gpui::MenuItem::action("Quit horo", Quit)],
        disabled: false,
    }]);
}

/// The main window.
pub fn open_window(cx: &mut App) -> anyhow::Result<()> {
    let bounds = Bounds::centered(None, size(px(1180.), px(780.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // No title bar: the traffic lights sit over the spaces rail, as the
        // Tauri build's overlay title bar has them, and the columns' tops
        // drag the window.
        titlebar: Some(TitlebarOptions {
            title: Some("horo".into()),
            appears_transparent: true,
            traffic_light_position: Some(gpui::point(px(14.), px(14.))),
        }),
        window_min_size: Some(size(px(720.), px(480.))),
        ..Default::default()
    };
    cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| views::app::AppView::new(window, cx));
        cx.new(|cx| gpui_component::Root::new(view, window, cx))
    })?;
    cx.activate(true);
    Ok(())
}


#[cfg(test)]
mod tests {
    use gpui::AssetSource;

    /// Every `icons/….svg` the source names must load: the icon kit
    /// leaves some of Lucide out, and a missing icon draws as nothing (a
    /// button you can't see).
    #[test]
    fn every_icon_used_is_there() {
        fn scan(dir: &std::path::Path, found: &mut std::collections::BTreeSet<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    scan(&path, found);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let mut rest = text.as_str();
                    while let Some(at) = rest.find("\"icons/") {
                        rest = &rest[at + 1..];
                        if let Some(end) = rest.find('"') {
                            let name = &rest[..end];
                            if name.ends_with(".svg") && !name.contains('{') {
                                found.insert(name.to_owned());
                            }
                        }
                    }
                }
            }
        }
        let mut used = std::collections::BTreeSet::new();
        scan(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut used);
        assert!(used.len() > 20, "found only {used:?}");
        let missing: Vec<&String> = used.iter().filter(|p| super::Assets.load(p).ok().flatten().is_none()).collect();
        assert!(missing.is_empty(), "icons that won't load: {missing:?}");
    }
}
