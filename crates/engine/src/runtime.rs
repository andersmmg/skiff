//! Engine lifecycle: spawn-or-attach supervision and the poll loop.
//!
//! `Engine::start` returns immediately; a supervisor task on a private tokio
//! runtime tries the RPC port first (an instance may already be running),
//! spawns the sidecar when needed, and then keeps a poll loop alive. The UI
//! consumes the resulting event stream; nothing here knows about GPUI.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::client::{RpcClient, RpcError};
use crate::sidecar::Sidecar;
use crate::types::PollSnapshot;

/// How long the attach probe waits for an answer before giving up on an
/// already-running instance.
const ATTACH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// Gap between readiness probes while the sidecar starts or restarts.
const READY_INTERVAL: Duration = Duration::from_millis(300);

/// Set by SIGTERM/SIGINT so the supervisor can kill the sidecar before the
/// process goes away, instead of leaving an orphaned engine behind.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// Ask the supervisor to kill the sidecar and stop the process. Called when
/// the app quits; the child's PDEATHSIG covers exits that never get here.
pub fn request_shutdown() {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// RPC port on 127.0.0.1. Chosen fresh at startup so several Skiff
    /// instances never collide.
    pub port: u16,
    /// RPC secret; random per launch, never leaves loopback.
    pub secret: String,
    /// Where new downloads land.
    pub download_dir: PathBuf,
    /// Where the engine keeps resume state; never next to payloads.
    pub state_dir: PathBuf,
    pub max_concurrent: u32,
    pub connection_limit: u32,
    pub split: u32,
    /// Explicit engine binary; otherwise discovered.
    pub binary: Option<PathBuf>,
    pub poll_interval: Duration,
}

impl EngineConfig {
    /// Platform-standard directories, a free loopback port, a fresh secret.
    pub fn standard() -> Self {
        let download_dir = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(std::env::temp_dir);
        let state_dir = dirs::state_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(std::env::temp_dir)
            .join("skiff")
            .join("aria2");
        let _ = std::fs::create_dir_all(&download_dir);
        let _ = std::fs::create_dir_all(&state_dir);
        Self {
            port: free_port(),
            secret: (0..4).map(|_| fastrand::alphanumeric()).collect(),
            download_dir,
            state_dir,
            max_concurrent: 4,
            connection_limit: 8,
            split: 8,
            binary: None,
            // ~6 snapshots/s: loopback RPC is cheap, and the UI reads much
            // smoother than the old 500ms cadence.
            poll_interval: Duration::from_millis(150),
        }
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .unwrap_or(6801)
}

#[derive(Debug)]
pub enum EngineEvent {
    /// The engine answered and this is its state right now.
    Snapshot(PollSnapshot),
    /// The engine went quiet; supervision is retrying.
    Unavailable(String),
    /// The engine (re)appeared; carries `aria2.getVersion`.
    Ready(Value),
}

pub struct Engine {
    // Owns the private runtime; dropping it tears down the supervisor and,
    // through `Sidecar::drop`, kills the engine process.
    _runtime: tokio::runtime::Runtime,
    client: ArcRpcClient,
    events: Receiver<EngineEvent>,
    /// Settings-save channel into the supervisor (see [`Reconfigurator`]).
    reconfigure: Sender<EngineConfig>,
}

/// Cheap handle shared between the supervisor task and the UI.
pub type ArcRpcClient = std::sync::Arc<RpcClient>;

/// Cloneable ask: respawn the sidecar with a new [`EngineConfig`]. This is
/// how a settings save applies while the app keeps running — the shared
/// client's endpoint moves with it, and unfinished tasks come back from the
/// engine session.
#[derive(Clone)]
pub struct Reconfigurator(Sender<EngineConfig>);

impl Reconfigurator {
    /// Queue a configuration; the supervisor drains on its next loop tick
    /// and the last write wins.
    pub fn send(&self, config: EngineConfig) {
        self.0.send(config).ok();
    }
}

impl Engine {
    pub fn start(config: EngineConfig) -> Result<Self> {
        let _ = ctrlc::set_handler(|| SHUTDOWN.store(true, Ordering::SeqCst));
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let mut client = RpcClient::new(config.port, config.secret.clone());
        client.attach_runtime(runtime.handle().clone());
        let client = std::sync::Arc::new(client);
        let (sender, events) = std::sync::mpsc::channel::<EngineEvent>();
        let (reconfigure, pending) = std::sync::mpsc::channel::<EngineConfig>();
        runtime.spawn(supervise(config, client.clone(), sender, pending));
        Ok(Self {
            _runtime: runtime,
            client,
            events,
            reconfigure,
        })
    }

    pub fn client(&self) -> ArcRpcClient {
        self.client.clone()
    }

    /// A cloneable handle for asking the supervisor to respawn the sidecar
    /// with new settings, without restarting the app.
    pub fn reconfigurator(&self) -> Reconfigurator {
        Reconfigurator(self.reconfigure.clone())
    }

    pub fn events(&self) -> &Receiver<EngineEvent> {
        &self.events
    }
}

/// Drain pending settings updates: kill our sidecar (its drop is a
/// synchronous kill+wait), adopt the new config, and move the shared client
/// to the new endpoint. Returns whether anything was applied; the caller
/// restarts the attach/spawn cycle so the sidecar comes back with the new
/// arguments.
fn take_reconfigure(
    reconfigure: &Receiver<EngineConfig>,
    config: &mut EngineConfig,
    client: &ArcRpcClient,
    sidecar: &mut Option<Sidecar>,
    backoff: &mut Duration,
) -> bool {
    let mut applied = false;
    while let Ok(fresh) = reconfigure.try_recv() {
        *sidecar = None;
        *config = fresh;
        client.set_endpoint(config.port, config.secret.clone());
        *backoff = Duration::from_millis(500);
        applied = true;
    }
    applied
}

async fn supervise(
    mut config: EngineConfig,
    client: ArcRpcClient,
    events: Sender<EngineEvent>,
    reconfigure: Receiver<EngineConfig>,
) {
    let mut sidecar: Option<Sidecar> = None;
    let mut backoff = Duration::from_millis(500);
    'supervise: loop {
        // A settings save applies live here: with the old child already
        // dead, the attach probe cannot mistake it for a fresh endpoint.
        take_reconfigure(&reconfigure, &mut config, &client, &mut sidecar, &mut backoff);
        if shutdown_requested() {
            sidecar.take();
            std::process::exit(0);
        }
        // Attach first: a user-started `aria2-next --enable-rpc` wins.
        if client
            .call_within_ok("aria2.getVersion", ATTACH_PROBE_TIMEOUT)
            .await
            .is_ok()
        {
            sidecar.take();
        } else {
            match Sidecar::spawn(&config) {
                Ok(spawned) => {
                    log::info!(target: "engine", "sidecar up on 127.0.0.1:{}", config.port);
                    sidecar = Some(spawned);
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    log::warn!(target: "engine", "cannot run engine: {message}");
                    events.send(EngineEvent::Unavailable(message)).ok();
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(5));
                    continue;
                }
            }
            // Wait until the RPC port answers.
            loop {
                // A reconfigure while waiting (spawn failing, slow start)
                // must not starve: restart the cycle with the new config.
                if take_reconfigure(&reconfigure, &mut config, &client, &mut sidecar, &mut backoff) {
                    continue 'supervise;
                }
                match client
                    .call_within_ok("aria2.getVersion", READY_INTERVAL)
                    .await
                {
                    Ok(_) => break,
                    Err(_) => {
                        if let Some(spawned) = sidecar.as_mut()
                            && let Some(exit) = spawned.poll_exit()
                        {
                            log::error!(target: "engine", "sidecar exited early: {exit}");
                            sidecar = None;
                            break;
                        }
                        tokio::time::sleep(READY_INTERVAL).await;
                    }
                }
            }
            if sidecar.is_none() {
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(5));
                continue;
            }
        }

        match client.version().await {
            Ok(version) => {
                events.send(EngineEvent::Ready(version)).ok();
            }
            Err(error) => {
                events
                    .send(EngineEvent::Unavailable(error.to_string()))
                    .ok();
            }
        }

        // Poll until the engine stops answering.
        let mut failures = 0_u32;
        loop {
            // A settings save mid-poll restarts the cycle with the new config.
            if take_reconfigure(&reconfigure, &mut config, &client, &mut sidecar, &mut backoff) {
                continue 'supervise;
            }
            match client.poll().await {
                Ok(snapshot) => {
                    failures = 0;
                    backoff = Duration::from_millis(500);
                    events.send(EngineEvent::Snapshot(snapshot)).ok();
                }
                Err(error) => {
                    failures += 1;
                    if failures >= 3 {
                        log::warn!(target: "engine", "engine lost: {error}");
                        events
                            .send(EngineEvent::Unavailable(error.to_string()))
                            .ok();
                        break;
                    }
                }
            }
            if shutdown_requested() {
                sidecar.take();
                std::process::exit(0);
            }
            tokio::time::sleep(config.poll_interval).await;
        }

        // Engine gone: kill any sidecar we own (drop) and re-supervise.
        sidecar.take();
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(5));
    }
}

impl RpcClient {
    /// Readiness/attach probe with a caller-chosen timeout.
    async fn call_within_ok(&self, method: &str, timeout: Duration) -> Result<Value, RpcError> {
        tokio::time::timeout(timeout, self.version())
            .await
            .unwrap_or_else(|_| {
                Err(RpcError::Transport(format!(
                    "{method} timed out after {timeout:?}"
                )))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: aria2-next used to outlive Skiff — closing the app left
    /// the engine running (user found six orphans). On graceful shutdown,
    /// dropping the engine must kill the sidecar. Hard kills and crashes are
    /// covered by the child's PDEATHSIG (see sidecar.rs), which cannot be
    /// asserted from a test process that has to stay alive.
    #[cfg(target_os = "linux")]
    #[test]
    fn dropping_engine_kills_the_sidecar() {
        let Some(binary) = test_binary() else {
            eprintln!("skipping: no aria2-next binary found");
            return;
        };
        let mut config = EngineConfig::standard();
        config.binary = Some(binary);
        config.poll_interval = Duration::from_millis(100);
        let port = config.port;
        let engine = Engine::start(config).expect("engine starts");

        let pid = wait_for_process(&format!("--rpc-listen-port={port}"))
            .unwrap_or_else(|| panic!("sidecar never came up on port {port}"));
        assert!(process_alive(pid), "sidecar died during startup");

        drop(engine);
        for _ in 0..50 {
            if !process_alive(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("sidecar (pid {pid}) survived engine shutdown");
    }

    /// The machine's aria2-next, if any; tests skip without it.
    #[cfg(target_os = "linux")]
    fn test_binary() -> Option<PathBuf> {
        std::env::var_os("SKIFF_ARIA2_NEXT")
            .map(PathBuf::from)
            .filter(|path| path.is_file())
            .or_else(|| {
                dirs::home_dir()
                    .map(|home| home.join("Downloads/aria2-next"))
                    .filter(|path| path.is_file())
            })
    }

    /// Poll `/proc` for a cmdline containing `needle` (10s budget).
    #[cfg(target_os = "linux")]
    fn wait_for_process(needle: &str) -> Option<i32> {
        (0..100).find_map(|_| {
            let found = find_process(needle);
            if found.is_none() {
                std::thread::sleep(Duration::from_millis(100));
            }
            found
        })
    }

    /// A settings save must reach the running engine: the supervisor kills
    /// the sidecar, respawns it with the new secret, and the shared client
    /// follows — all without an app restart (the user's complaint).
    #[cfg(target_os = "linux")]
    #[test]
    fn reconfigure_respawns_sidecar_with_the_new_secret() {
        let Some(binary) = test_binary() else {
            eprintln!("skipping: no aria2-next binary found");
            return;
        };
        let mut config = EngineConfig::standard();
        config.binary = Some(binary);
        config.poll_interval = Duration::from_millis(100);
        let port = config.port;
        let engine = Engine::start(config.clone()).expect("engine starts");
        let first_pid = wait_for_process(&format!("--rpc-listen-port={port}"))
            .unwrap_or_else(|| panic!("sidecar never came up on port {port}"));

        // The settings-save path: same port, brand-new secret.
        let mut fresh = config;
        fresh.secret = (0..24).map(|_| fastrand::alphanumeric()).collect();
        engine.reconfigurator().send(fresh.clone());

        let secret_needle = format!("--rpc-secret={}", fresh.secret);
        let second_pid = wait_for_process(&secret_needle)
            .unwrap_or_else(|| panic!("sidecar never respawned with the new secret"));
        assert_ne!(second_pid, first_pid, "sidecar was not respawned");
        for _ in 0..50 {
            if !process_alive(first_pid) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!process_alive(first_pid), "old sidecar survived reconfigure");

        // The one client the UI shares must authenticate with the new secret.
        let client = engine.client();
        let mut answered = false;
        for _ in 0..50 {
            let (done, got) = std::sync::mpsc::channel();
            let call = client.clone();
            client.spawn(async move {
                let ok = call.version().await.is_ok();
                done.send(ok).ok();
            });
            if matches!(got.recv_timeout(Duration::from_secs(2)), Ok(true)) {
                answered = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(answered, "client never authenticated with the new secret");

        drop(engine);
        for _ in 0..50 {
            if !process_alive(second_pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("respawned sidecar survived engine shutdown");
    }

    #[cfg(target_os = "linux")]
    fn find_process(needle: &str) -> Option<i32> {
        for entry in std::fs::read_dir("/proc").ok()? {
            // Never `?` out of the loop: `/proc` also holds `self`,
            // `thread-self` and transient entries that must be skipped.
            let Ok(entry) = entry else { continue };
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(pid) = name.parse::<i32>() else { continue };
            let Ok(cmdline) = std::fs::read_to_string(entry.path().join("cmdline")) else {
                continue;
            };
            if cmdline.contains(needle) {
                return Some(pid);
            }
        }
        None
    }

    #[cfg(target_os = "linux")]
    fn process_alive(pid: i32) -> bool {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
}
