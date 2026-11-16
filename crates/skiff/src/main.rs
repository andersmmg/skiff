//! Skiff: native download manager.
//!
//! Startup order: logging, settings, engine runtime, then the GPUI Kit
//! application. `gpui_kit::init` boots the component library (themes, key
//! bindings) before the first window opens.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::App;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // The engine RPC endpoint is fixed across launches (port + secret in the
    // settings file) so browser extensions can be pointed at it once.
    let settings = state::Settings::load();
    let config = settings.engine_config();
    log::info!(
        target: "skiff",
        "engine RPC: http://127.0.0.1:{}/jsonrpc (secret in the skiff settings file)",
        settings.rpc_port
    );

    // The engine owns a private tokio runtime and returns immediately; the
    // supervisor attaches or spawns aria2-next in the background.
    let engine = match engine::Engine::start(config) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("skiff: cannot start engine runtime: {error:#}");
            return;
        }
    };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            state::init(cx, engine);

            let main_window = match ui::open_main_window(cx) {
                Ok(handle) => Some(handle),
                Err(error) => {
                    log::error!("cannot open window: {error}");
                    None
                }
            };
            // The tray can hide the window instead of quitting, so only arm
            // that behavior once the tray is actually registered. Handing it
            // the startup window keeps Show/Hide toggling that same window.
            if ui::tray::init(cx, main_window) {
                log::info!(target: "skiff", "tray icon ready");
            }
            cx.activate(true);
        });
}
