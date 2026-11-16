//! JSON-RPC transport for the local aria2-next engine.
//!
//! Ordinary calls answer within `CALL_TIMEOUT`; anything slower means the
//! engine is stuck. Native aria2 errors travel inside HTTP 400/404/500
//! responses, so the error envelope is decoded before the HTTP status.

use std::fmt;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::types::{GlobalStat, JsonRpcRequest, JsonRpcResponse, PollSnapshot, Aria2Task};

/// Ordinary calls answer at once; anything slower means the engine is stuck.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum RpcError {
    /// The engine did not answer (not running, restarting, network failed).
    Transport(String),
    /// The engine answered with a JSON-RPC error.
    Protocol {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    /// The answer was not a valid JSON-RPC response.
    Malformed(String),
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RpcError::Transport(message) => write!(f, "engine unreachable: {message}"),
            RpcError::Protocol { code, message, .. } => {
                write!(f, "engine error {code}: {message}")
            }
            RpcError::Malformed(message) => write!(f, "malformed engine response: {message}"),
        }
    }
}

impl std::error::Error for RpcError {}

impl From<reqwest::Error> for RpcError {
    fn from(error: reqwest::Error) -> Self {
        RpcError::Transport(error.to_string())
    }
}

pub struct RpcClient {
    http: reqwest::Client,
    /// Current engine endpoint. Swapped in place when settings are saved so
    /// the one client the whole UI shares follows the engine across a
    /// sidecar respawn instead of going stale.
    endpoint: RwLock<(u16, String)>,
    next_id: AtomicU64,
    /// The engine tokio runtime, once bound. UI thread callers (GPUI's event
    /// loop, which is *not* a tokio runtime) spawn calls through this.
    runtime: Option<tokio::runtime::Handle>,
}

impl RpcClient {
    pub fn new(port: u16, secret: impl Into<String>) -> Self {
        Self {
            // Local loopback only; never reuse a proxy from the environment.
            http: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("local RPC client"),
            endpoint: RwLock::new((port, secret.into())),
            next_id: AtomicU64::new(1),
            runtime: None,
        }
    }

    /// Point the client at a new engine endpoint. The supervisor calls this
    /// when settings are saved, right after killing the old sidecar: the
    /// next call, and every one after it, uses the new port and secret.
    pub fn set_endpoint(&self, port: u16, secret: String) {
        *self
            .endpoint
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = (port, secret);
    }

    /// Read the current endpoint (clone; calls hold it for one request).
    fn endpoint(&self) -> (u16, String) {
        let guard = self
            .endpoint
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (guard.0, guard.1.clone())
    }

    /// Bind the client to the engine's runtime. reqwest needs tokio's
    /// reactor, so calls must never run on GPUI's own executor.
    pub fn attach_runtime(&mut self, handle: tokio::runtime::Handle) {
        self.runtime = Some(handle);
    }

    /// Run `future` where tokio's reactor lives: the bound engine runtime,
    /// or a throwaway one when the client was never bound (unit tests).
    pub fn spawn(&self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        match &self.runtime {
            Some(handle) => {
                handle.spawn(future);
            }
            None => {
                std::thread::spawn(move || {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("rpc fallback runtime")
                        .block_on(future);
                });
            }
        }
    }

    pub fn port(&self) -> u16 {
        self.endpoint().0
    }

    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        mut params: Vec<Value>,
    ) -> Result<T, RpcError> {
        let (port, secret) = self.endpoint();
        if !secret.is_empty() {
            params.insert(0, Value::String(format!("token:{secret}")));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: id.to_string(),
            method: method.to_owned(),
            params,
        };
        let response = self
            .http
            .post(format!("http://127.0.0.1:{port}/jsonrpc"))
            .timeout(CALL_TIMEOUT)
            .json(&request)
            .send()
            .await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        let body: JsonRpcResponse<T> = serde_json::from_slice(&bytes).map_err(|error| {
            RpcError::Malformed(format!("invalid JSON-RPC response (HTTP {status}): {error}"))
        })?;
        if let Some(error) = body.error {
            return Err(RpcError::Protocol {
                code: error.code,
                message: error.message,
                data: error.data,
            });
        }
        body.result
            .ok_or_else(|| RpcError::Malformed("missing result".into()))
    }

    /// One round trip for everything the UI shows per tick: global numbers
    /// plus the three task buckets.
    pub async fn poll(&self) -> Result<PollSnapshot, RpcError> {
        let calls = json!([
            { "methodName": "aria2.getGlobalStat", "params": [] },
            { "methodName": "aria2.tellActive", "params": [] },
            { "methodName": "aria2.tellWaiting", "params": [0, 1000] },
            { "methodName": "aria2.tellStopped", "params": [0, 500] },
        ]);
        // system.multicall authenticates per nested call, so the token is
        // injected into each `params` rather than the outer call.
        let (port, secret) = self.endpoint();
        let mut params = vec![calls];
        if !secret.is_empty() {
            let token = Value::String(format!("token:{secret}"));
            if let Some(calls) = params.first_mut().and_then(Value::as_array_mut) {
                for call in calls {
                    if let Some(inner) = call.get_mut("params").and_then(Value::as_array_mut) {
                        inner.insert(0, token.clone());
                    }
                }
            }
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: id.to_string(),
            method: "system.multicall".into(),
            params,
        };
        let response = self
            .http
            .post(format!("http://127.0.0.1:{port}/jsonrpc"))
            .timeout(CALL_TIMEOUT)
            .json(&request)
            .send()
            .await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        let body: JsonRpcResponse<Vec<Value>> =
            serde_json::from_slice(&bytes).map_err(|error| {
                RpcError::Malformed(format!("invalid multicall response (HTTP {status}): {error}"))
            })?;
        if let Some(error) = body.error {
            return Err(RpcError::Protocol {
                code: error.code,
                message: error.message,
                data: error.data,
            });
        }
        let results = body
            .result
            .ok_or_else(|| RpcError::Malformed("missing multicall result".into()))?;
        // Each nested call answers as a single-element array wrapping its
        // result, in request order.
        let at = |index: usize, what: &str| -> Result<Value, RpcError> {
            results
                .get(index)
                .and_then(|entry| entry.as_array())
                .and_then(|wrapped| wrapped.first().cloned())
                .ok_or_else(|| RpcError::Malformed(format!("{what}: short multicall result")))
        };
        let stats: GlobalStat = serde_json::from_value(at(0, "getGlobalStat")?)
            .map_err(|error| RpcError::Malformed(format!("getGlobalStat: {error}")))?;
        let active: Vec<Aria2Task> = serde_json::from_value(at(1, "tellActive")?)
            .map_err(|error| RpcError::Malformed(format!("tellActive: {error}")))?;
        let waiting: Vec<Aria2Task> = serde_json::from_value(at(2, "tellWaiting")?)
            .map_err(|error| RpcError::Malformed(format!("tellWaiting: {error}")))?;
        let stopped: Vec<Aria2Task> = serde_json::from_value(at(3, "tellStopped")?)
            .map_err(|error| RpcError::Malformed(format!("tellStopped: {error}")))?;
        Ok(PollSnapshot {
            stats,
            active,
            waiting,
            stopped,
        })
    }

    pub async fn version(&self) -> Result<Value, RpcError> {
        self.call("aria2.getVersion", vec![]).await
    }

    pub async fn add_uri(&self, uris: &[String], options: Value) -> Result<String, RpcError> {
        self.call(
            "aria2.addUri",
            vec![json!(uris), options],
        )
        .await
    }

    pub async fn pause(&self, gid: &str) -> Result<String, RpcError> {
        self.call("aria2.pause", vec![json!(gid)]).await
    }

    pub async fn unpause(&self, gid: &str) -> Result<String, RpcError> {
        self.call("aria2.unpause", vec![json!(gid)]).await
    }    pub async fn remove(&self, gid: &str) -> Result<String, RpcError> {
        self.call("aria2.remove", vec![json!(gid)])
            .await
    }

    /// Queue-wide variants for the tray menu: pause/resume everything the
    /// engine currently knows about.
    pub async fn pause_all(&self) -> Result<String, RpcError> {
        self.call("aria2.pauseAll", vec![]).await
    }

    pub async fn unpause_all(&self) -> Result<String, RpcError> {
        self.call("aria2.unpauseAll", vec![]).await
    }

    pub async fn force_remove(&self, gid: &str) -> Result<String, RpcError> {
        self.call("aria2.forceRemove", vec![json!(gid)]).await
    }

    pub async fn purge_download_result(&self) -> Result<String, RpcError> {
        self.call("aria2.purgeDownloadResult", vec![]).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal loopback server that answers one JSON-RPC request with the
    /// four wrapped multicall results in request order, after checking that
    /// the auth token arrived inside each nested call.
    fn stub_engine(secret: &str) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let secret = secret.to_owned();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = stream.read(&mut buffer).expect("read");
                request.extend_from_slice(&buffer[..read]);
                if let Some(header_end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&request[..header_end]).to_string();
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().expect("length"))
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + 4 + length {
                        break;
                    }
                }
            }
            let body_start = request
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .expect("head")
                + 4;
            let body: serde_json::Value =
                serde_json::from_slice(&request[body_start..]).expect("json");
            assert_eq!(body["method"], "system.multicall");
            for call in body["params"][0].as_array().expect("calls") {
                assert_eq!(call["params"][0], format!("token:{secret}"));
            }
            let wrapped = |value: serde_json::Value| serde_json::json!([value]);
            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": body["id"],
                "result": [
                    wrapped(serde_json::json!({
                        "downloadSpeed": "2048",
                        "uploadSpeed": "128",
                        "numActive": "1",
                        "numWaiting": "1",
                        "numStopped": "1",
                        "numStoppedTotal": "1",
                    })),
                    wrapped(serde_json::json!([{
                        "gid": "g1",
                        "status": "active",
                        "totalLength": "4096",
                        "completedLength": "1024",
                        "uploadLength": "0",
                        "downloadSpeed": "2048",
                        "uploadSpeed": "0",
                        "connections": "4",
                        "dir": "/tmp",
                        "files": [],
                    }])),
                    wrapped(serde_json::json!([{
                        "gid": "g2",
                        "status": "waiting",
                        "totalLength": "0",
                        "completedLength": "0",
                        "uploadLength": "0",
                        "downloadSpeed": "0",
                        "uploadSpeed": "0",
                        "connections": "0",
                        "dir": "/tmp",
                        "files": [],
                    }])),
                    wrapped(serde_json::json!([{
                        "gid": "g3",
                        "status": "complete",
                        "totalLength": "10",
                        "completedLength": "10",
                        "uploadLength": "10",
                        "downloadSpeed": "0",
                        "uploadSpeed": "0",
                        "connections": "0",
                        "dir": "/tmp",
                        "files": [],
                    }])),
                ],
            });
            let payload = serde_json::to_string(&response).expect("serialize");
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            stream.write_all(reply.as_bytes()).expect("write");
        });
        port
    }

    #[tokio::test]
    async fn poll_decodes_multicall_snapshot_and_authenticates_each_call() {
        let secret = "hunter2";
        let port = stub_engine(secret);
        let client = RpcClient::new(port, secret);
        let snapshot = client.poll().await.expect("poll");
        assert_eq!(snapshot.stats.download_speed(), 2048);
        assert_eq!(snapshot.stats.num_active(), 1);
        assert_eq!(snapshot.active.len(), 1);
        assert_eq!(snapshot.active[0].gid, "g1");
        assert_eq!(snapshot.active[0].completed_length(), 1024);
        assert_eq!(snapshot.waiting.len(), 1);
        assert_eq!(snapshot.stopped.len(), 1);
        assert_eq!(snapshot.stopped[0].status, "complete");
    }

    /// The UI's event loop is not a tokio runtime; spawning through
    /// `RpcClient::spawn` must still give the future a reactor. Without one,
    /// the very first timer in a request panics ("no reactor running").
    #[test]
    fn spawn_runs_future_on_the_attached_runtime_from_a_plain_thread() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("runtime");
        let mut client = RpcClient::new(1, "unused");
        client.attach_runtime(runtime.handle().clone());

        let (done, received) = std::sync::mpsc::channel();
        client.spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            done.send(()).ok();
        });
        received.recv_timeout(Duration::from_secs(5)).expect("future ran");
    }

    /// Same, with no runtime attached: the fallback must also provide a
    /// reactor rather than panicking on the calling thread.
    #[test]
    fn spawn_falls_back_to_a_fresh_runtime_when_unbound() {
        let client = RpcClient::new(1, "unused");
        let (done, received) = std::sync::mpsc::channel();
        client.spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            done.send(()).ok();
        });
        received.recv_timeout(Duration::from_secs(5)).expect("future ran");
    }

    /// Settings saves move the live endpoint (port + secret) without
    /// rebuilding the one client the whole UI shares.
    #[tokio::test]
    async fn set_endpoint_switches_port_and_secret() {
        let old_port = stub_engine("old");
        let new_port = stub_engine("new");
        let client = RpcClient::new(old_port, "old");
        client.poll().await.expect("old endpoint answers");
        client.set_endpoint(new_port, "new".into());
        client.poll()
            .await
            .expect("new endpoint answers with the new token");
        assert_eq!(client.port(), new_port);
    }
}
