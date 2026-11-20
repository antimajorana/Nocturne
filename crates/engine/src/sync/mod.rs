//! Built-in Infomaniak kDrive REST API sync (layer L4).
//!
//! Infomaniak restricts WebDAV (`connect.kdrive.infomaniak.com`) to paid plans, while the official
//! REST API (`https://api.infomaniak.com`, `/2/drive` and `/3/drive`) works on free kDrive plans
//! with a Personal API Token (`drive` scope).
//!
//! Sync is local-first and two-phase so an open page in the UI and a background sync never
//! clobber each other:
//! 1. [`stage_sync`] runs off the UI thread: uploads local-only edits and stages remote downloads
//!    into `<local_root>/.nocturne-sync-stage/<run_id>/`, recording the `expected_local_sha256`
//!    seen when staging started.
//! 2. [`apply_staged_sync`] runs on the caller's thread after flushing any in-memory page edits:
//!    it re-hashes each local file right before renaming, preserving concurrent edits beside the
//!    file as `<stem>.conflict-<ts>.<ext>` so neither side ever loses work.

mod kdrive;
pub(crate) use kdrive::is_loopback_http;
mod manifest;
mod reconcile;
mod transport;

pub use kdrive::{KDriveClient, RemoteEntry};
pub use manifest::{is_excluded_rel_path, sha256_bytes, sha256_file};
pub use reconcile::{StagedSync, apply_staged_sync, run_sync_blocking, stage_sync};
pub use transport::{
    FileSecretStore, SecretStore, SyncHttpClient, SyncHttpMethod, SyncHttpRequest, SyncHttpResponse, UnsupportedSecretStore, UnsupportedSyncClient,
    default_http_client, default_secret_store,
};

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
pub use transport::UreqSyncClient;

use nocturne_store::Settings;
use std::fmt;

pub const DEFAULT_KDRIVE_ENDPOINT: &str = "https://api.infomaniak.com";
pub const DEFAULT_KDRIVE_REMOTE_PATH: &str = "The Oracle/Nocturne";
pub const KDRIVE_TOKEN_KEY: &str = "kdrive_personal_token";

const KEY_ENDPOINT: &str = "sync_endpoint";
const KEY_DRIVE_ID: &str = "sync_drive_id";
const KEY_REMOTE_PATH: &str = "sync_remote_path";
const KEY_AUTO_SYNC: &str = "sync_auto";
const KEY_MARKDOWN_DEFAULT: &str = "sync_markdown_default";
const KEY_PROPAGATE_DELETES: &str = "sync_propagate_deletes";
const KEY_LAST_SYNC_SUMMARY: &str = "sync_last_summary";
const KEY_LAST_SYNC_UNIX: &str = "sync_last_unix";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncError {
    NotConfigured(String),
    Unsupported(String),
    Auth(String),
    RateLimited { retry_after_secs: u64, message: String },
    Cancelled,
    Transport(String),
    Protocol(String),
    SafetyRefused(String),
    Io(String),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncError::NotConfigured(msg)
            | SyncError::Unsupported(msg)
            | SyncError::Auth(msg)
            | SyncError::Transport(msg)
            | SyncError::Protocol(msg)
            | SyncError::SafetyRefused(msg)
            | SyncError::Io(msg) => f.write_str(msg),
            SyncError::RateLimited { retry_after_secs, message } => {
                write!(f, "kDrive rate limit reached (retry after {retry_after_secs}s): {message}")
            }
            SyncError::Cancelled => f.write_str("sync cancelled"),
        }
    }
}

impl std::error::Error for SyncError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncConfig {
    pub endpoint: String,
    pub drive_id: Option<u64>,
    pub remote_path: String,
    pub auto_sync: bool,
    pub markdown_default: bool,
    pub propagate_deletes: bool,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_KDRIVE_ENDPOINT.to_string(),
            drive_id: None,
            remote_path: DEFAULT_KDRIVE_REMOTE_PATH.to_string(),
            auto_sync: false,
            markdown_default: true,
            propagate_deletes: false,
        }
    }
}

impl SyncConfig {
    pub fn load(settings: &Settings) -> Self {
        let endpoint = settings
            .get(KEY_ENDPOINT)
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_KDRIVE_ENDPOINT.to_string());
        let drive_id = settings.get(KEY_DRIVE_ID).and_then(|s| parse_drive_id_input(&s).ok().flatten());
        let remote_path = normalize_remote_path(&settings.get(KEY_REMOTE_PATH).unwrap_or_else(|| DEFAULT_KDRIVE_REMOTE_PATH.to_string()));
        let auto_sync = settings.get(KEY_AUTO_SYNC).is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
        let markdown_default = settings.get(KEY_MARKDOWN_DEFAULT).map(|v| !v.trim().eq_ignore_ascii_case("false")).unwrap_or(true);
        let propagate_deletes = settings.get(KEY_PROPAGATE_DELETES).is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
        Self { endpoint, drive_id, remote_path, auto_sync, markdown_default, propagate_deletes }
    }

    pub fn save(&self, settings: &Settings) -> Result<(), SyncError> {
        let clean_endpoint = self.endpoint.trim().trim_end_matches('/');
        let endpoint = if clean_endpoint.is_empty() { DEFAULT_KDRIVE_ENDPOINT } else { clean_endpoint };
        settings.set(KEY_ENDPOINT, endpoint).map_err(|e| SyncError::Io(e.to_string()))?;
        let drive_str = self.drive_id.map(|id| id.to_string()).unwrap_or_default();
        settings.set(KEY_DRIVE_ID, &drive_str).map_err(|e| SyncError::Io(e.to_string()))?;
        let remote = normalize_remote_path(&self.remote_path);
        settings.set(KEY_REMOTE_PATH, &remote).map_err(|e| SyncError::Io(e.to_string()))?;
        settings.set(KEY_AUTO_SYNC, if self.auto_sync { "true" } else { "false" }).map_err(|e| SyncError::Io(e.to_string()))?;
        settings.set(KEY_MARKDOWN_DEFAULT, if self.markdown_default { "true" } else { "false" }).map_err(|e| SyncError::Io(e.to_string()))?;
        settings.set(KEY_PROPAGATE_DELETES, if self.propagate_deletes { "true" } else { "false" }).map_err(|e| SyncError::Io(e.to_string()))?;
        Ok(())
    }

    pub fn record_last_sync(settings: &Settings, report: &SyncReport) {
        let summary = format!("up {}, down {}, conflicts {}", report.uploaded.len(), report.downloaded.len(), report.conflicts.len());
        let _ = settings.set(KEY_LAST_SYNC_SUMMARY, &summary);
        let _ = settings.set(KEY_LAST_SYNC_UNIX, &report.finished_unix_secs.to_string());
    }

    pub fn last_sync_info(settings: &Settings) -> (Option<u64>, Option<String>) {
        let ts = settings.get(KEY_LAST_SYNC_UNIX).and_then(|v| v.trim().parse::<u64>().ok());
        let summary = settings.get(KEY_LAST_SYNC_SUMMARY).filter(|s| !s.trim().is_empty());
        (ts, summary)
    }
}

/// Parses a user-supplied kDrive identifier (`""`, `"auto"`, numeric `"123456"`, or a kDrive web
/// URL such as `"https://ksuite.infomaniak.com/all/kdrive/app/drive/123456/files/1"`).
pub fn parse_drive_id_input(raw: &str) -> Result<Option<u64>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    if let Ok(id) = trimmed.parse::<u64>()
        && id > 0
    {
        return Ok(Some(id));
    }
    let clean = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    let segs: Vec<&str> = clean.split('/').collect();
    for pair in segs.windows(2) {
        if pair[0].eq_ignore_ascii_case("drive")
            && let Ok(id) = pair[1].parse::<u64>()
            && id > 0
        {
            return Ok(Some(id));
        }
    }
    Err("drive_id must be a positive number, a kDrive URL (.../drive/<ID>), or auto".into())
}

pub fn normalize_remote_path(raw: &str) -> String {
    let joined =
        raw.replace('\\', "/").split('/').map(str::trim).filter(|seg| !seg.is_empty() && *seg != "." && *seg != "..").collect::<Vec<_>>().join("/");
    if joined.is_empty() { DEFAULT_KDRIVE_REMOTE_PATH.to_string() } else { joined }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub drive_id: u64,
    pub remote_path: String,
    pub uploaded: Vec<String>,
    pub downloaded: Vec<String>,
    pub trashed_remote: Vec<String>,
    pub trashed_local: Vec<String>,
    pub conflicts: Vec<String>,
    pub unchanged: usize,
    pub finished_unix_secs: u64,
}

impl SyncReport {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "drive_id": self.drive_id,
            "remote_path": self.remote_path,
            "uploaded": self.uploaded,
            "downloaded": self.downloaded,
            "trashed_remote": self.trashed_remote,
            "trashed_local": self.trashed_local,
            "conflicts": self.conflicts,
            "unchanged": self.unchanged,
            "finished_unix_secs": self.finished_unix_secs,
        })
    }
}
