//! The main window: opened at startup and again whenever the tray's Show/Hide
//! asks for it. Hiding closes the window; nothing else is lost, because task
//! state, settings, and the engine client live in the `Skiff` global rather
//! than in the view tree.

use anyhow::Result;
use gpui_kit::{
    App, AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowHandle, WindowOptions, px,
    size,
};

use crate::Root;

pub fn open_main_window(cx: &mut App) -> Result<WindowHandle<Root>> {
    let bounds = Bounds::centered(None, size(px(1024.), px(700.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Skiff".into()),
            ..Default::default()
        }),
        app_id: Some("skiff".into()),
        window_min_size: Some(size(px(720.), px(480.))),
        ..Default::default()
    };
    cx.open_window(options, |window, cx: &mut App| {
        cx.new(|cx| Root::new(window, cx))
    })
}
