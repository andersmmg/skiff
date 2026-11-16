//! The task store: engine snapshots flattened into render-friendly downloads.

use engine::{Aria2Task, GlobalStat, PollSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStatus {
    Active,
    Waiting,
    Paused,
    Complete,
    Failed,
    Removed,
}

impl DownloadStatus {
    fn from_aria2(status: &str) -> Self {
        match status {
            "active" => Self::Active,
            "waiting" => Self::Waiting,
            "paused" => Self::Paused,
            "complete" => Self::Complete,
            "error" => Self::Failed,
            "removed" => Self::Removed,
            _ => Self::Waiting,
        }
    }
}

/// One download, ready to render: numbers already parsed.
#[derive(Debug, Clone)]
pub struct Download {
    pub gid: String,
    pub name: String,
    pub uri: Option<String>,
    pub status: DownloadStatus,
    pub total: u64,
    pub completed: u64,
    pub uploaded: u64,
    pub download_speed: u64,
    pub upload_speed: u64,
    pub connections: u64,
    /// True when aria2 reports BitTorrent metadata: only torrents upload,
    /// so other tasks never show upload figures.
    pub torrent: bool,
    pub error: Option<String>,
}

impl Download {
    fn from_aria2(task: &Aria2Task) -> Self {
        Self {
            gid: task.gid.clone(),
            name: task.name(),
            uri: task.primary_uri(),
            status: DownloadStatus::from_aria2(&task.status),
            total: task.total_length(),
            completed: task.completed_length(),
            uploaded: task.upload_length(),
            download_speed: task.download_speed(),
            upload_speed: task.upload_speed(),
            connections: task.connections(),
            torrent: task.bittorrent.is_some(),
            error: task.error_message.clone().filter(|message| !message.is_empty()),
        }
    }

    /// 0.0..=1.0; unknown size counts as zero progress, shown as a spinner
    /// state by the UI instead.
    pub fn progress(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.completed as f32 / self.total as f32).clamp(0.0, 1.0)
        }
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.status, DownloadStatus::Complete | DownloadStatus::Failed | DownloadStatus::Removed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Active,
    Waiting,
    Paused,
    Complete,
    Failed,
}

impl Filter {
    pub const ALL: [Filter; 6] = [
        Filter::All,
        Filter::Active,
        Filter::Waiting,
        Filter::Paused,
        Filter::Complete,
        Filter::Failed,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Active => "Active",
            Filter::Waiting => "Waiting",
            Filter::Paused => "Paused",
            Filter::Complete => "Complete",
            Filter::Failed => "Failed",
        }
    }

    fn matches(&self, download: &Download) -> bool {
        match self {
            Filter::All => true,
            Filter::Active => download.status == DownloadStatus::Active,
            Filter::Waiting => download.status == DownloadStatus::Waiting,
            Filter::Paused => download.status == DownloadStatus::Paused,
            Filter::Complete => download.status == DownloadStatus::Complete,
            Filter::Failed => download.status == DownloadStatus::Failed,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Counts {
    pub all: usize,
    pub active: usize,
    pub waiting: usize,
    pub paused: usize,
    pub complete: usize,
    pub failed: usize,
}

impl Counts {
    pub fn of(&self, filter: Filter) -> usize {
        match filter {
            Filter::All => self.all,
            Filter::Active => self.active,
            Filter::Waiting => self.waiting,
            Filter::Paused => self.paused,
            Filter::Complete => self.complete,
            Filter::Failed => self.failed,
        }
    }
}

/// The whole task store, updated atomically per snapshot.
#[derive(Default)]
pub struct TaskList {
    pub downloads: Vec<Download>,
    pub stats: GlobalStat,
}

impl TaskList {
    /// Active tasks first, then waiting/paused, then finished — the engine's
    /// own ordering inside each bucket.
    pub fn apply(&mut self, snapshot: PollSnapshot) {
        self.stats = snapshot.stats;
        self.downloads = snapshot
            .active
            .iter()
            .chain(snapshot.waiting.iter())
            .chain(snapshot.stopped.iter())
            .map(Download::from_aria2)
            .collect();
    }

    pub fn counts(&self) -> Counts {
        let mut counts = Counts {
            all: self.downloads.len(),
            ..Default::default()
        };
        for download in &self.downloads {
            match download.status {
                DownloadStatus::Active => counts.active += 1,
                DownloadStatus::Waiting => counts.waiting += 1,
                DownloadStatus::Paused => counts.paused += 1,
                DownloadStatus::Complete => counts.complete += 1,
                DownloadStatus::Failed => counts.failed += 1,
                DownloadStatus::Removed => {}
            }
        }
        counts
    }

    pub fn visible<'a>(&'a self, filter: Filter) -> impl Iterator<Item = &'a Download> {
        self.downloads.iter().filter(move |download| filter.matches(download))
    }
}
