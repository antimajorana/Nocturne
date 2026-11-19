//! Sync baseline manifest (`kdrive-sync-state.json`), SHA-256 helpers, and exclusion rules.

use crate::sync::SyncError;
use nocturne_store::Settings;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const MANIFEST_FILE: &str = "kdrive-sync-state.json";

#[derive(Clone, Debug)]
pub(crate) struct LocalEntry {
    pub sha256: String,
    pub size: u64,
    pub mtime_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ManifestEntry {
    pub local_sha256: String,
    pub local_size: u64,
    pub remote_id: u64,
    pub remote_fingerprint: String,
    pub synced_unix_secs: u64,
}

/// Returns `true` when `rel_path` (slash-separated) must NEVER be synced to or from kDrive.
pub fn is_excluded_rel_path(rel_path: &str) -> bool {
    if rel_path.is_empty() || rel_path.contains('\\') || rel_path.contains(':') || rel_path.chars().any(char::is_control) {
        return true;
    }
    let mut segments = rel_path.split('/');
    let Some(first) = segments.next() else { return true };
    if first.eq_ignore_ascii_case("welcome-to-nocturne") || first.eq_ignore_ascii_case("Inbox") {
        return true;
    }
    for seg in std::iter::once(first).chain(segments) {
        if seg.is_empty() || seg == "." || seg == ".." || seg.starts_with('.') {
            return true;
        }
        let lower = seg.to_ascii_lowercase();
        if lower == "thumbs.db"
            || lower == "desktop.ini"
            || lower == "nocturne-diagnostics.log"
            || lower.ends_with(".tmp-write")
            || lower.ends_with(".tmp")
            || lower.ends_with(".tmp-secret")
        {
            return true;
        }
    }
    false
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> Result<(String, u64), SyncError> {
    let mut file = std::fs::File::open(path).map_err(|e| SyncError::Io(format!("open {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf).map_err(|e| SyncError::Io(format!("read {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    let hex = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok((hex, total))
}

pub(crate) fn unix_now_secs() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn file_mtime_secs(path: &Path) -> u64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or_else(unix_now_secs)
}

pub(crate) fn scan_local_tree(root: &Path, cancel: Option<&AtomicBool>) -> Result<BTreeMap<String, LocalEntry>, SyncError> {
    let mut out = BTreeMap::new();
    if !root.exists() {
        return Ok(out);
    }
    let mut stack = vec![(String::new(), root.to_path_buf())];
    while let Some((prefix, dir)) = stack.pop() {
        if cancel.is_some_and(|f| f.load(Ordering::Relaxed)) {
            return Err(SyncError::Cancelled);
        }
        let rd = std::fs::read_dir(&dir).map_err(|e| SyncError::Io(format!("read_dir {}: {e}", dir.display())))?;
        for entry in rd {
            let entry = entry.map_err(|e| SyncError::Io(format!("dir entry in {}: {e}", dir.display())))?;
            let ft = entry.file_type().map_err(|e| SyncError::Io(format!("file_type in {}: {e}", dir.display())))?;
            if ft.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            if is_excluded_rel_path(&rel) {
                continue;
            }
            let full = entry.path();
            if ft.is_dir() {
                stack.push((rel, full));
            } else if ft.is_file() {
                let (sha256, size) = sha256_file(&full)?;
                let mtime_secs = file_mtime_secs(&full);
                out.insert(rel, LocalEntry { sha256, size, mtime_secs });
            }
        }
    }
    Ok(out)
}

/// Scans `<root>/<notebook>/.nocturne/recycle-bin/*/recycled.toml` and returns slash-separated
/// relative path prefixes (e.g. `"quantum-notes/new-section-1/old-page"`) that were explicitly
/// recycled in Nocturne, so sync trashes the matching remote copy instead of re-downloading it.
pub(crate) fn scan_recycled_prefixes(root: &Path) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let Ok(notebooks) = std::fs::read_dir(root) else { return out };
    for nb_entry in notebooks.flatten() {
        let nb_path = nb_entry.path();
        if !nb_path.is_dir() {
            continue;
        }
        let nb_name = nb_entry.file_name().to_string_lossy().into_owned();
        if nb_name.starts_with('.') || nb_name.eq_ignore_ascii_case("welcome-to-nocturne") || nb_name.eq_ignore_ascii_case("Inbox") {
            continue;
        }
        let bin_dir = nb_path.join(".nocturne").join("recycle-bin");
        let Ok(recycled_entries) = std::fs::read_dir(&bin_dir) else { continue };
        for rec_entry in recycled_entries.flatten() {
            let meta = rec_entry.path().join("recycled.toml");
            let Ok(text) = std::fs::read_to_string(&meta) else { continue };
            let map = nocturne_model::kv::parse(&text);
            if let Some(orig) = map.get("original").map(|s| s.trim().trim_matches('/')).filter(|s| !s.is_empty()) {
                out.insert(format!("{nb_name}/{orig}"));
            }
        }
    }
    out
}

pub(crate) fn manifest_path_for(settings: &Settings) -> PathBuf {
    settings.path().parent().unwrap_or_else(|| Path::new(".")).join(MANIFEST_FILE)
}

pub(crate) fn make_target_key(local_root: &Path, endpoint: &str, drive_id: u64, remote_path: &str) -> String {
    let canon = std::fs::canonicalize(local_root).unwrap_or_else(|_| local_root.to_path_buf());
    let root_str = nocturne_store::tree::display_path(&canon).replace('\\', "/");
    format!("{}|{}|{}|{}", root_str, endpoint.trim().trim_end_matches('/'), drive_id, remote_path)
}

pub(crate) fn load_manifest(path: &Path, expected_key: &str) -> BTreeMap<String, ManifestEntry> {
    let Ok(bytes) = std::fs::read(path) else { return BTreeMap::new() };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return BTreeMap::new() };
    if json.get("target_key").and_then(|v| v.as_str()) != Some(expected_key) {
        return BTreeMap::new();
    }
    let Some(files) = json.get("files").and_then(|v| v.as_object()) else { return BTreeMap::new() };
    let mut out = BTreeMap::new();
    for (rel, val) in files {
        let Some(local_sha256) = val.get("local_sha256").and_then(|v| v.as_str()) else { continue };
        let Some(local_size) = val.get("local_size").and_then(|v| v.as_u64()) else { continue };
        let Some(remote_id) = val.get("remote_id").and_then(|v| v.as_u64()) else { continue };
        let Some(remote_fingerprint) = val.get("remote_fingerprint").and_then(|v| v.as_str()) else { continue };
        let synced_unix_secs = val.get("synced_unix_secs").and_then(|v| v.as_u64()).unwrap_or(0);
        out.insert(
            rel.clone(),
            ManifestEntry {
                local_sha256: local_sha256.to_string(),
                local_size,
                remote_id,
                remote_fingerprint: remote_fingerprint.to_string(),
                synced_unix_secs,
            },
        );
    }
    out
}

pub(crate) fn save_manifest(path: &Path, target_key: &str, entries: &BTreeMap<String, ManifestEntry>) -> Result<(), SyncError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SyncError::Io(format!("create {}: {e}", parent.display())))?;
    }
    let mut files = serde_json::Map::new();
    for (rel, entry) in entries {
        files.insert(
            rel.clone(),
            serde_json::json!({
                "local_sha256": entry.local_sha256,
                "local_size": entry.local_size,
                "remote_id": entry.remote_id,
                "remote_fingerprint": entry.remote_fingerprint,
                "synced_unix_secs": entry.synced_unix_secs,
            }),
        );
    }
    let doc = serde_json::json!({
        "version": 1,
        "target_key": target_key,
        "files": files,
    });
    let text = serde_json::to_string_pretty(&doc).map_err(|e| SyncError::Io(e.to_string()))?;
    let tmp = path.with_extension("tmp-write");
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| SyncError::Io(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        SyncError::Io(format!("rename {}: {e}", path.display()))
    })
}
