//! horo's interface in gpui: the views, the theme, and the bridge between
//! horo-core (async, on tokio) and gpui's own executor.
//!
//! An app calls [`init`] with a [`Backend`], then [`open_window`].

use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

pub mod backend;
pub mod rooms;
pub mod store;
pub mod theme;
pub mod views;

pub use backend::Backend;

/// gpui-component, the theme and the core's backend, once per app.
pub fn init(backend: Backend, cx: &mut App) {
    gpui_component::init(cx);
    theme::install(cx);
    cx.set_global(backend);
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
