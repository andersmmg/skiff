//! Application state: task store, settings, and the engine-event bridge.
//!
//! Like sonora, subsystems are entities held in a single global. Views never
//! touch the engine directly for reads — they observe entities, and a bridge
//! task applies engine snapshots on the main thread.

mod settings;
mod tasks;

pub use settings::Settings;
pub use tasks::{Counts, Download, DownloadStatus, Filter, TaskList};

use std::sync::Arc;
use std::time::Duration;

use engine::{Engine, RpcClient};
use gpui_kit::{App, AppContext, AsyncApp, Entity, Global};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// The engine process/port has not answered yet.
    Connecting,
    Connected,
    /// The engine went quiet; supervision is retrying.
    Lost(String),
}

pub struct Connection {
    pub status: ConnectionStatus,
}

/// The app-wide state hub, registered as a GPUI global.
pub struct Skiff {
    pub tasks: Entity<TaskList>,
    pub connection: Entity<Connection>,
    pub settings: Entity<Settings>,
    pub client: Arc<RpcClient>,
    /// Ask the engine supervisor to respawn the sidecar with new settings.
    pub reconfigure: engine::Reconfigurator,
}

impl Global for Skiff {}

impl Skiff {
    pub fn global(cx: &App) -> &Skiff {
        cx.global::<Skiff>()
    }
}

/// Bridge poll cadence; engine snapshots arrive at the engine's own poll
/// interval, this only bounds how stale the UI may be — keep it well under
/// the engine's poll gap so rows and counts appear right away.
const BRIDGE_INTERVAL: Duration = Duration::from_millis(50);

pub fn init(cx: &mut App, engine: Engine) {
    let client = engine.client();
    let reconfigure = engine.reconfigurator();
    let tasks = cx.new(|_| TaskList::default());
    let connection = cx.new(|_| Connection {
        status: ConnectionStatus::Connecting,
    });
    let settings = cx.new(|_| Settings::load());
    cx.set_global(Skiff {
        tasks: tasks.clone(),
        connection: connection.clone(),
        settings,
        client,
        reconfigure,
    });

    // The supervisor normally learns about shutdown from a signal; a normal
    // app quit delivers none, so ask it directly to kill the sidecar. (The
    // child's PDEATHSIG is the backstop for exits that skip even this.)
    cx.on_app_quit(|_cx| async {
        engine::request_shutdown();
    })
    .detach();

    let executor = cx.background_executor();
    let timer_source = executor.clone();
    cx.spawn(async move |cx: &mut AsyncApp| loop {
        while let Ok(event) = engine.events().try_recv() {
            let update = match event {
                engine::EngineEvent::Ready(_) => Some((None, ConnectionStatus::Connected)),
                engine::EngineEvent::Unavailable(reason) => {
                    Some((None, ConnectionStatus::Lost(reason)))
                }
                engine::EngineEvent::Snapshot(snapshot) => {
                    Some((Some(snapshot), ConnectionStatus::Connected))
                }
            };
            let (snapshot, status) = match update {
                Some(update) => update,
                None => continue,
            };
            let _ = cx.update(|cx: &mut App| {
                connection.update(cx, |connection, cx| {
                    if connection.status != status {
                        connection.status = status;
                        cx.notify();
                    }
                });
                if let Some(snapshot) = snapshot {
                    // `Entity::update` does not fire observers on its own;
                    // without this notify the window is never invalidated and
                    // the UI would only repaint when input events force it.
                    tasks.update(cx, |list, cx| {
                        list.apply(snapshot);
                        cx.notify();
                    });
                }
            });
        }
        timer_source.timer(BRIDGE_INTERVAL).await;
    })
    .detach();
}
