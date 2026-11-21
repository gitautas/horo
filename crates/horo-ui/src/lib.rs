//! horo's interface in gpui: the views, the theme, and the bridge between
//! horo-core (async, on tokio) and gpui's own executor.
//!
//! An app calls [`init`] with a [`Backend`], then [`open_window`].

use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

pub mod alerts;
pub mod backend;
pub mod call;
pub mod media;
pub mod prefs;
pub mod presence;
pub mod rich;
pub mod rooms;
pub mod sounds;
pub mod store;
pub mod theme;
pub mod verify;
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
const OWN_ICONS: &[(&str, &[u8])] = &[
    ("icons/phone.svg", include_bytes!("../assets/icons/phone.svg")),
    ("icons/phone-off.svg", include_bytes!("../assets/icons/phone-off.svg")),
    ("icons/phone-incoming.svg", include_bytes!("../assets/icons/phone-incoming.svg")),
    ("icons/mic-off.svg", include_bytes!("../assets/icons/mic-off.svg")),
    ("icons/headphones.svg", include_bytes!("../assets/icons/headphones.svg")),
];

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
        titlebar: Some(TitlebarOptions { title: Some("horo".into()), ..Default::default() }),
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
