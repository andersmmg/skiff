//! Supervised `aria2-next` child process.
//!
//! The engine runs as a sidecar bound to loopback only: a random high port
//! chosen at startup and a random RPC secret. Killing this process on drop
//! keeps the supervisor restart path simple.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::{env, io};

use anyhow::{Context, Result};

use crate::runtime::EngineConfig;

pub struct Sidecar {
    child: Child,
    port: u16,
}

impl Sidecar {
    pub fn spawn(config: &EngineConfig) -> Result<Self> {
        let binary = find_binary(config).context(
            "aria2-next binary not found (set SKIFF_ARIA2_NEXT or install aria2-next on PATH)",
        )?;
        let mut command = Command::new(&binary);
        command
            .arg("--enable-rpc")
            .arg(format!("--rpc-listen-port={}", config.port))
            .arg("--rpc-listen-all=false")
            .arg(format!("--rpc-secret={}", config.secret))
            .arg(format!("--dir={}", config.download_dir.display()))
            .arg(format!("--state-dir={}", config.state_dir.display()))
            .arg(format!(
                "--max-concurrent-downloads={}",
                config.max_concurrent
            ))
            .arg(format!(
                "--max-connection-per-server={}",
                config.connection_limit
            ))
            .arg(format!("--split={}", config.split))
            .arg("--min-split-size=1M")
            .arg("--continue=true")
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        // Unfinished tasks persist across every respawn (settings save,
        // engine loss, next launch): the periodic write is what survives
        // the SIGKILL on shutdown — verified against aria2-next 3.0.5.
        // `-i` restores them on start; aria2 refuses a missing input file,
        // so pass it only once the session actually exists.
        let session = config.state_dir.join("session");
        command
            .arg(format!("--save-session={}", session.display()))
            .arg("--save-session-interval=1");
        if session.is_file() {
            command.arg(format!("--input-file={}", session.display()));
        }
        // The sidecar must die with the app. Task-drop kills it on graceful
        // shutdown, but a crashing or hard-killed Skiff never runs the
        // supervisor's destructors — that is how aria2-next instances used
        // to outlive the app. PDEATHSIG closes every remaining exit path.
        #[cfg(unix)]
        unsafe {
            use std::os::unix::process::CommandExt;
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Parent already gone by now: don't linger as an orphan.
                if libc::getppid() == 1 {
                    libc::raise(libc::SIGKILL);
                }
                Ok(())
            });
        }
        log::debug!(target: "engine", "spawning sidecar {:?} on port {}", binary, config.port);
        let mut child = command.spawn().with_context(|| {
            format!("failed to spawn sidecar {}", binary.display())
        })?;

        // Surface engine stderr through the app log without blocking spawn.
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                use std::io::BufRead;
                let reader = io::BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    log::info!(target: "aria2", "{line}");
                }
            });
        }

        Ok(Self {
            child,
            port: config.port,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// `Some(exit)` when the engine died on its own.
    pub fn poll_exit(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Resolution order: explicit config, `SKIFF_ARIA2_NEXT`, `PATH`, then the
/// directory the `skiff` binary itself lives in (bundled sidecar layouts).
fn find_binary(config: &EngineConfig) -> Option<PathBuf> {
    if let Some(binary) = &config.binary {
        return Some(binary.clone());
    }
    if let Some(binary) = env::var_os("SKIFF_ARIA2_NEXT") {
        let binary = PathBuf::from(binary);
        if binary.is_file() {
            return Some(binary);
        }
    }
    let executable = if cfg!(windows) { "aria2-next.exe" } else { "aria2-next" };
    if let Some(paths) = env::var_os("PATH") {
        for directory in env::split_paths(&paths) {
            let candidate = directory.join(executable);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let beside = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(executable)))
        .filter(|candidate| candidate.is_file());
    beside
}
