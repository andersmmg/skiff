//! aria2 JSON-RPC data transfer objects.
//!
//! Every numeric value in the aria2 JSON-RPC protocol is a string; the raw
//! types mirror that so deserialization never has to guess. Parsing helpers
//! live next to the raw values.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 request envelope.
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: String,
    pub method: String,
    pub params: Vec<Value>,
}

/// JSON-RPC 2.0 response envelope.
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse<T> {
    pub result: Option<T>,
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

/// A single file of a task, as reported by aria2.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskFile {
    pub index: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub length: String,
    #[serde(default)]
    pub completed_length: String,
    #[serde(default)]
    pub selected: String,
    #[serde(default)]
    pub uris: Vec<TaskFileUri>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TaskFileUri {
    pub uri: String,
    #[serde(default)]
    pub status: String,
}

/// BitTorrent metadata, only what the UI needs.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BtInfo {
    #[serde(default)]
    pub info: Option<BtName>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BtName {
    #[serde(default)]
    pub name: String,
}

/// A download task returned by `tellStatus`, `tellActive`, `tellWaiting` or
/// `tellStopped`. Unknown fields (media, ed2k, tracker data …) are ignored.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Aria2Task {
    pub gid: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub total_length: String,
    #[serde(default)]
    pub completed_length: String,
    #[serde(default)]
    pub upload_length: String,
    #[serde(default)]
    pub download_speed: String,
    #[serde(default)]
    pub upload_speed: String,
    #[serde(default)]
    pub connections: String,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub files: Vec<TaskFile>,
    #[serde(default)]
    pub bittorrent: Option<BtInfo>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub followed_by: Vec<String>,
}

impl Aria2Task {
    /// `aria2` emits numbers as strings everywhere; bad values count as zero
    /// rather than poisoning a whole snapshot.
    pub fn number(field: &str) -> u64 {
        field.parse().unwrap_or(0)
    }

    pub fn total_length(&self) -> u64 {
        Self::number(&self.total_length)
    }

    pub fn completed_length(&self) -> u64 {
        Self::number(&self.completed_length)
    }

    pub fn upload_length(&self) -> u64 {
        Self::number(&self.upload_length)
    }

    pub fn download_speed(&self) -> u64 {
        Self::number(&self.download_speed)
    }

    pub fn upload_speed(&self) -> u64 {
        Self::number(&self.upload_speed)
    }

    pub fn connections(&self) -> u64 {
        Self::number(&self.connections)
    }

    /// Best human-readable name: torrent name, then file name, then URL tail,
    /// then the gid.
    pub fn name(&self) -> String {
        if let Some(name) = self
            .bittorrent
            .as_ref()
            .and_then(|bt| bt.info.as_ref())
            .map(|info| info.name.trim())
            .filter(|name| !name.is_empty())
        {
            return name.to_owned();
        }
        if let Some(file) = self.files.first() {
            if let Some(name) = file_basename(&file.path).filter(|n| !n.is_empty()) {
                return name;
            }
            if let Some(name) = self.primary_uri().as_deref().and_then(url_tail) {
                return name;
            }
        }
        if let Some(name) = self.primary_uri().as_deref().and_then(url_tail) {
            return name;
        }
        self.gid.clone()
    }

    /// First known URI of the first file.
    pub fn primary_uri(&self) -> Option<String> {
        self.files
            .first()
            .and_then(|file| file.uris.first().map(|entry| entry.uri.clone()))
    }
}

fn file_basename(path: &str) -> Option<String> {
    let path = path.trim_end_matches(['/', '\\']);
    let last = path.rsplit(['/', '\\']).next()?;
    if last.is_empty() { None } else { Some(last.to_owned()) }
}

fn url_tail(url: &str) -> Option<String> {
    let tail = url.split(['?', '#']).next().unwrap_or(url);
    let tail = tail.trim_end_matches('/');
    file_basename(tail)
}

/// Raw `aria2.getGlobalStat` result.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalStat {
    #[serde(default)]
    pub download_speed: String,
    #[serde(default)]
    pub upload_speed: String,
    #[serde(default)]
    pub num_active: String,
    #[serde(default)]
    pub num_waiting: String,
    #[serde(default)]
    pub num_stopped: String,
    #[serde(default)]
    pub num_stopped_total: String,
}

impl GlobalStat {
    pub fn download_speed(&self) -> u64 {
        Aria2Task::number(&self.download_speed)
    }

    pub fn upload_speed(&self) -> u64 {
        Aria2Task::number(&self.upload_speed)
    }

    pub fn num_active(&self) -> u64 {
        Aria2Task::number(&self.num_active)
    }

    pub fn num_waiting(&self) -> u64 {
        Aria2Task::number(&self.num_waiting)
    }

    pub fn num_stopped(&self) -> u64 {
        Aria2Task::number(&self.num_stopped)
    }
}

/// One poll tick's worth of engine state, gathered in a single
/// `system.multicall` round trip.
#[derive(Debug, Clone, Default)]
pub struct PollSnapshot {
    pub stats: GlobalStat,
    pub active: Vec<Aria2Task>,
    pub waiting: Vec<Aria2Task>,
    pub stopped: Vec<Aria2Task>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_a_minimal_task_with_string_numbers() {
        let json = serde_json::json!({
            "gid": "abc123",
            "status": "active",
            "totalLength": "1024",
            "completedLength": "512",
            "uploadLength": "0",
            "downloadSpeed": "100",
            "uploadSpeed": "0",
            "connections": "5",
            "dir": "/tmp"
        });
        let task: Aria2Task = serde_json::from_value(json).expect("deserialize");
        assert_eq!(task.gid, "abc123");
        assert_eq!(task.total_length(), 1024);
        assert_eq!(task.completed_length(), 512);
        assert_eq!(task.connections(), 5);
    }

    #[test]
    fn names_prefer_torrent_name_then_file_name_then_url() {
        let mut task = Aria2Task {
            gid: "gid1".into(),
            ..Default::default()
        };
        task.files.push(TaskFile {
            path: "/downloads/some file.bin".into(),
            uris: vec![TaskFileUri {
                uri: "https://example.com/some%20file.bin".into(),
                status: "used".into(),
            }],
            ..Default::default()
        });
        assert_eq!(task.name(), "some file.bin");

        task.bittorrent = Some(BtInfo {
            info: Some(BtName {
                name: "ubuntu.iso".into(),
            }),
        });
        assert_eq!(task.name(), "ubuntu.iso");

        let bare = Aria2Task {
            gid: "gid2".into(),
            ..Default::default()
        };
        assert_eq!(bare.name(), "gid2");
    }
}
