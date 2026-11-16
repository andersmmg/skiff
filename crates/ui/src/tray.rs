//! System tray icon (StatusNotifierItem) on its own thread.
//!
//! ksni owns a dedicated D-Bus connection; clicks and menu items cross into
//! the GPUI main thread over a channel drained by a bridge task — the same
//! shape as the engine-event bridge in `state`. Hiding the window closes it;
//! app state survives in the `Skiff` global, and Show/Hide reopens a fresh
//! window on demand.

use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::time::Duration;

use gpui_kit::{App, QuitMode, WindowHandle};
use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, Tray, TrayMethods as _};
use state::Skiff;

use crate::Root;
use crate::window::open_main_window;

/// What the user asked for from the tray.
#[derive(Clone, Copy)]
enum Action {
    Toggle,
    PauseAll,
    ResumeAll,
    Quit,
}

struct SkiffTray {
    actions: Sender<Action>,
}

impl SkiffTray {
    fn send(&self, action: Action) {
        // A dropped receiver only means the app is quitting; nothing to do.
        self.actions.send(action).ok();
    }
}

impl Tray for SkiffTray {
    fn id(&self) -> String {
        "skiff".into()
    }

    fn title(&self) -> String {
        "Skiff".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![icon()]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Action::Toggle);
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        self.send(Action::Toggle);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let item = |label: &str, action: Action| {
            MenuItem::Standard(StandardItem {
                label: label.into(),
                activate: Box::new(move |tray: &mut SkiffTray| tray.send(action)),
                ..Default::default()
            })
        };
        vec![
            item("Show/Hide", Action::Toggle),
            MenuItem::Separator,
            item("Pause all", Action::PauseAll),
            item("Resume all", Action::ResumeAll),
            MenuItem::Separator,
            item("Quit", Action::Quit),
        ]
    }
}

/// A 32×32 accent disc with a white download arrow, drawn in code so the
/// tray needs no image assets. ARGB32, network byte order per the spec.
fn icon() -> Icon {
    const SIZE: i32 = 32;
    let mut data = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            let inside = (dx * dx + dy * dy).sqrt() <= 15.0;
            let shaft = (14..=17).contains(&x) && (6..=17).contains(&y);
            let head = (18..=25).contains(&y) && (x - 16).abs() <= y - 17;
            let pixel = if !inside {
                [0, 0, 0, 0]
            } else if shaft || head {
                [255, 255, 255, 255]
            } else {
                [255, 0x4f, 0x8c, 0xff] // app accent
            };
            data.extend_from_slice(&pixel);
        }
    }
    Icon {
        width: SIZE,
        height: SIZE,
        data,
    }
}

/// Poll interval for the tray's action channel. Small enough that clicks
/// feel instant, coarse enough to cost nothing while idle.
const BRIDGE_INTERVAL: Duration = Duration::from_millis(50);

/// Registers the tray icon and bridges its actions onto the GPUI thread.
///
/// `main_window` is the window opened at startup: seeding the bridge with it
/// makes the first Show/Hide toggle *that* window instead of opening a
/// second one beside it.
///
/// Returns whether the tray is actually running: without it the app keeps
/// the normal quit-on-last-window-close behavior instead of hiding to tray.
pub fn init(cx: &mut App, main_window: Option<WindowHandle<Root>>) -> bool {
    let (tx, rx) = mpsc::channel::<Action>();
    if !register(tx) {
        return false;
    }
    // Closing the window now hides it; the tray's Show/Hide reopens it.
    cx.set_quit_mode(QuitMode::Explicit);
    bridge(rx, main_window, cx);
    true
}

/// GPUI tasks take an owned, `'static` context rather than borrowing `&mut
/// App`, so the bridge clones one out with `to_async` and drives a plain
/// future on the foreground executor.
fn bridge(
    actions: Receiver<Action>,
    main_window: Option<WindowHandle<Root>>,
    cx: &mut App,
) {
    let cx = cx.to_async();
    let executor = cx.foreground_executor().clone();
    executor
        .spawn(async move {
            // The open window: the startup window at first, then whatever the
            // bridge reopened. A stale handle (the user closed the window
            // with its own button) fails to update, which reads as "hidden"
            // and triggers a reopen.
            let mut window: Option<WindowHandle<Root>> = main_window;
            loop {
                let mut toggle = false;
                let mut quit = false;
                while let Ok(action) = actions.try_recv() {
                    match action {
                        Action::Toggle => toggle = true,
                        Action::Quit => quit = true,
                        Action::PauseAll => cx.update(|cx| pause_all(cx, true)),
                        Action::ResumeAll => cx.update(|cx| pause_all(cx, false)),
                    }
                }
                if toggle || quit {
                    let mut reopened = None;
                    cx.update(|cx: &mut App| {
                        if quit {
                            cx.quit();
                            return;
                        }
                        let mut still_open = false;
                        if let Some(handle) = window.take() {
                            still_open = handle
                                .update(cx, |_, window, _| window.remove_window())
                                .is_ok();
                        }
                        if !still_open {
                            match open_main_window(cx) {
                                Ok(handle) => reopened = Some(handle),
                                Err(error) => {
                                    log::warn!(target: "tray", "cannot open window: {error}")
                                }
                            }
                        }
                    });
                    window = reopened;
                }
                cx.background_executor().timer(BRIDGE_INTERVAL).await;
            }
        })
        .detach();
}

fn pause_all(cx: &mut App, pause: bool) {
    let client = Skiff::global(cx).client.clone();
    let run = client.clone();
    client.spawn(async move {
        let result = if pause {
            run.pause_all().await
        } else {
            run.unpause_all().await
        };
        if let Err(error) = result {
            log::warn!(target: "tray", "pause/resume all failed: {error}");
        }
    });
}

fn register(actions: Sender<Action>) -> bool {
    let (ready_tx, ready_rx): (SyncSender<bool>, _) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tray runtime");
        runtime.block_on(async move {
            let tray = SkiffTray { actions };
            match tray.spawn().await {
                Ok(_handle) => {
                    ready_tx.send(true).ok();
                    // Keep the service (and its handle) alive for the app's life.
                    std::future::pending::<()>().await;
                }
                Err(error) => {
                    log::warn!(target: "tray", "cannot register tray icon: {error}");
                    ready_tx.send(false).ok();
                }
            }
        });
    });
    // Registration is a local D-Bus round trip; give it a moment.
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_spec_size_argb32() {
        let icon = icon();
        assert_eq!(icon.data.len(), (icon.width * icon.height * 4) as usize);
        // Corner is transparent, the disc is accent blue, the arrow is white.
        assert_eq!(&icon.data[0..4], &[0, 0, 0, 0]);
        let disc = ((4 * 32 + 16) * 4) as usize;
        assert_eq!(&icon.data[disc..disc + 4], &[255, 0x4f, 0x8c, 0xff]);
        let shaft = ((10 * 32 + 16) * 4) as usize;
        assert_eq!(&icon.data[shaft..shaft + 4], &[255, 255, 255, 255]);
    }
}
