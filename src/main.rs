#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

use gpui::{px, AppContext as _, Application};
use gpui_component::Root;

use crate::app::AppView;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let app = Application::new().with_assets(gpui_component_assets::Assets);

    app.run(move |cx| {
        gpui_component::init(cx);

        let _window = cx.open_window(
            gpui::WindowOptions {
                titlebar: None,
                window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
                    None,
                    gpui::size(px(1280.), px(820.)),
                    cx,
                ))),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| AppView::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        );
    });
}
