//! Two-phase 3-way folder reconciler for local-first kDrive sync.

use crate::sync::kdrive::{KDriveClient, RemoteEntry};
use crate::sync::manifest::{
    LocalEntry, ManifestEntry, is_excluded_rel_path, load_manifest, make_target_key, manifest_path_for, save_manifest, scan_local_tree, sha256_bytes,
    sha256_file, unix_now_secs,
};
use crate::sync::transport::{SecretStore, SyncHttpClient};
use crate::sync::{KDRIVE_TOKEN_KEY, SyncConfig, SyncError, SyncReport};
use nocturne_store::Settings;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const STAGE_DIR_NAME: &str = ".nocturne-sync-stage";
const TRASH_DIR_NAME: &str = ".sync-trash";
const MAX_SAFE_DELETE_COUNT: usize = 10;

#[derive(Clone, Debug)]
struct StagedDownload {
    rel_path: String,
    staged_file: PathBuf,
    expected_local_sha256: Option<String>,
    downloaded_sha256: String,
    downloaded_size: u64,
    remote_id: u64,
    remote_fingerprint: String,
    force_conflict_copy: bool,
}

#[derive(Clone, Debug)]
struct StagedLocalTrash {
    rel_path: String,
    expected_local_sha256: String,
}

/// Output of [`stage_sync`], ready to be applied atomically by [`apply_staged_sync`].
#[derive(Debug)]
pub struct StagedSync {
    local_root: PathBuf,
    manifest_path: PathBuf,
    target_key: String,
    stage_dir: PathBuf,
    manifest_entries: BTreeMap<String, ManifestEntry>,
    downloads: Vec<StagedDownload>,
    local_trashes: Vec<StagedLocalTrash>,
    pub report: SyncReport,
    pub discovered_drive_id: Option<u64>,
}

impl StagedSync {
    /// Relative paths of local files that will be written or trashed when [`apply_staged_sync`] runs.
    pub fn touched_local_paths(&self) -> Vec<&str> {
        self.downloads.iter().map(|d| d.rel_path.as_str()).chain(self.local_trashes.iter().map(|t| t.rel_path.as_str())).collect()
    }
}

/// Phase 1: runs network discovery, uploads local-only changes, and stages remote downloads into
/// `<local_root>/.nocturne-sync-stage/<run_id>/` without mutating any live local notebook file.
pub fn stage_sync(
    local_root: &Path,
    settings: &Settings,
    config: &SyncConfig,
    http: &dyn SyncHttpClient,
    secrets: &dyn SecretStore,
    cancel: Option<&AtomicBool>,
) -> Result<StagedSync, SyncError> {
    if !http.is_supported() {
        return Err(SyncError::Unsupported("kDrive sync is not available in this build".into()));
    }
    let token = secrets
        .get_secret(KDRIVE_TOKEN_KEY)?
        .ok_or_else(|| SyncError::NotConfigured("enter a kDrive Personal API Token in Cloud Sync settings".into()))?;
    let client = KDriveClient::new(http, &config.endpoint, &token, cancel)?;

    let (drive_id, discovered_drive_id) = match config.drive_id {
        Some(id) => (id, None),
        None => {
            let id = client.discover_drive_id()?;
            (id, Some(id))
        }
    };
    let (sync_root_id, normalized_remote) = client.ensure_sync_root(drive_id, &config.remote_path)?;
    let local_files = scan_local_tree(local_root, cancel)?;
    let (remote_files, mut remote_dirs) = client.walk_subtree(drive_id, sync_root_id)?;

    let manifest_path = manifest_path_for(settings);
    let target_key = make_target_key(local_root, &config.endpoint, drive_id, &normalized_remote);
    let baseline = load_manifest(&manifest_path, &target_key);

    let now = unix_now_secs();
    let stage_parent = local_root.join(STAGE_DIR_NAME);
    let pid_suffix = format!("-{}", std::process::id());
    if let Ok(entries) = std::fs::read_dir(&stage_parent) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && name.starts_with("run-")
                && !name.ends_with(&pid_suffix)
            {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let stage_dir = stage_parent.join(format!("run-{now}{pid_suffix}"));
    let _ = std::fs::remove_dir_all(&stage_dir);
    std::fs::create_dir_all(&stage_dir).map_err(|e| SyncError::Io(format!("create stage dir {}: {e}", stage_dir.display())))?;

    let mut all_paths = BTreeSet::new();
    all_paths.extend(local_files.keys().cloned());
    all_paths.extend(remote_files.keys().cloned());
    all_paths.extend(baseline.keys().cloned());

    // Pre-scan deletion circuit breakers BEFORE performing any remote upload or delete!
    let mut planned_remote_deletes: Vec<(String, u64)> = Vec::new();
    let mut planned_local_deletes: Vec<StagedLocalTrash> = Vec::new();
    if config.propagate_deletes {
        if !baseline.is_empty() && (local_files.is_empty() || remote_files.is_empty()) {
            let _ = std::fs::remove_dir_all(&stage_dir);
            return Err(SyncError::SafetyRefused("refusing to propagate deletions because one side of the sync folder is completely empty".into()));
        }
        for rel in &all_paths {
            if is_excluded_rel_path(rel) {
                continue;
            }
            match (local_files.get(rel), remote_files.get(rel), baseline.get(rel)) {
                (Some(l), None, Some(b)) if l.sha256 == b.local_sha256 => {
                    planned_local_deletes.push(StagedLocalTrash { rel_path: rel.clone(), expected_local_sha256: b.local_sha256.clone() });
                }
                (None, Some(r), Some(b)) if r.fingerprint() == b.remote_fingerprint => {
                    planned_remote_deletes.push((rel.clone(), r.id));
                }
                _ => {}
            }
        }
        let total_deletes = planned_remote_deletes.len() + planned_local_deletes.len();
        if total_deletes > MAX_SAFE_DELETE_COUNT || (!baseline.is_empty() && total_deletes * 2 > baseline.len()) {
            let _ = std::fs::remove_dir_all(&stage_dir);
            return Err(SyncError::SafetyRefused(format!(
                "refusing to delete {total_deletes} files in one sync pass (limit {MAX_SAFE_DELETE_COUNT}); turn off delete propagation or delete manually"
            )));
        }
    }

    let mut next_manifest = baseline.clone();
    let mut downloads = Vec::new();
    let mut report = SyncReport { drive_id, remote_path: normalized_remote, finished_unix_secs: now, ..SyncReport::default() };
    let mut stage_counter = 0usize;

    for rel in all_paths {
        if cancel.is_some_and(|f| f.load(Ordering::Relaxed)) {
            let _ = std::fs::remove_dir_all(&stage_dir);
            return Err(SyncError::Cancelled);
        }
        if is_excluded_rel_path(&rel) {
            continue;
        }
        let loc = local_files.get(&rel);
        let rem = remote_files.get(&rel);
        let base = baseline.get(&rel);

        match (loc, rem, base) {
            (Some(l), Some(r), Some(b)) => {
                let local_changed = l.sha256 != b.local_sha256;
                let remote_changed = r.fingerprint() != b.remote_fingerprint;
                match (local_changed, remote_changed) {
                    (false, false) => {
                        report.unchanged += 1;
                    }
                    (true, false) => {
                        let (uploaded, sent_sha, sent_size) =
                            upload_local_rel(&client, drive_id, sync_root_id, local_root, &rel, l, &mut remote_dirs)?;
                        next_manifest.insert(
                            rel.clone(),
                            ManifestEntry {
                                local_sha256: sent_sha,
                                local_size: sent_size,
                                remote_id: uploaded.id,
                                remote_fingerprint: uploaded.fingerprint(),
                                synced_unix_secs: now,
                            },
                        );
                        report.uploaded.push(rel);
                    }
                    (false, true) => {
                        stage_counter += 1;
                        let staged = stage_remote_download(&client, drive_id, r, &stage_dir, stage_counter, &rel, Some(l.sha256.clone()), false)?;
                        downloads.push(staged);
                    }
                    (true, true) => {
                        // True 3-way edit conflict: download remote bytes FIRST and persist the conflict copy
                        // directly to disk BEFORE uploading local over remote so no mid-pass abort can lose it!
                        stage_counter += 1;
                        let staged = stage_remote_download(&client, drive_id, r, &stage_dir, stage_counter, &rel, Some(l.sha256.clone()), true)?;
                        if staged.downloaded_sha256 == l.sha256 {
                            let _ = std::fs::remove_file(&staged.staged_file);
                            next_manifest.insert(
                                rel,
                                ManifestEntry {
                                    local_sha256: l.sha256.clone(),
                                    local_size: l.size,
                                    remote_id: r.id,
                                    remote_fingerprint: r.fingerprint(),
                                    synced_unix_secs: now,
                                },
                            );
                            report.unchanged += 1;
                        } else {
                            let conflict_rel = conflict_rel_path(&rel, now);
                            let conflict_dest = local_path_for(local_root, &conflict_rel)?;
                            write_staged_to_dest(&staged.staged_file, &conflict_dest)?;
                            report.conflicts.push(conflict_rel);
                            let (uploaded, sent_sha, sent_size) =
                                upload_local_rel(&client, drive_id, sync_root_id, local_root, &rel, l, &mut remote_dirs)?;
                            next_manifest.insert(
                                rel.clone(),
                                ManifestEntry {
                                    local_sha256: sent_sha,
                                    local_size: sent_size,
                                    remote_id: uploaded.id,
                                    remote_fingerprint: uploaded.fingerprint(),
                                    synced_unix_secs: now,
                                },
                            );
                        }
                    }
                }
            }
            (Some(l), Some(r), None) => {
                // First sync with file present on both sides and no baseline: compare SHA-256!
                stage_counter += 1;
                let staged = stage_remote_download(&client, drive_id, r, &stage_dir, stage_counter, &rel, Some(l.sha256.clone()), true)?;
                if staged.downloaded_sha256 == l.sha256 {
                    let _ = std::fs::remove_file(&staged.staged_file);
                    next_manifest.insert(
                        rel,
                        ManifestEntry {
                            local_sha256: l.sha256.clone(),
                            local_size: l.size,
                            remote_id: r.id,
                            remote_fingerprint: r.fingerprint(),
                            synced_unix_secs: now,
                        },
                    );
                    report.unchanged += 1;
                } else {
                    let conflict_rel = conflict_rel_path(&rel, now);
                    let conflict_dest = local_path_for(local_root, &conflict_rel)?;
                    write_staged_to_dest(&staged.staged_file, &conflict_dest)?;
                    report.conflicts.push(conflict_rel);
                    let (uploaded, sent_sha, sent_size) = upload_local_rel(&client, drive_id, sync_root_id, local_root, &rel, l, &mut remote_dirs)?;
                    next_manifest.insert(
                        rel.clone(),
                        ManifestEntry {
                            local_sha256: sent_sha,
                            local_size: sent_size,
                            remote_id: uploaded.id,
                            remote_fingerprint: uploaded.fingerprint(),
                            synced_unix_secs: now,
                        },
                    );
                }
            }
            (Some(l), None, None) => {
                let (uploaded, sent_sha, sent_size) = upload_local_rel(&client, drive_id, sync_root_id, local_root, &rel, l, &mut remote_dirs)?;
                next_manifest.insert(
                    rel.clone(),
                    ManifestEntry {
                        local_sha256: sent_sha,
                        local_size: sent_size,
                        remote_id: uploaded.id,
                        remote_fingerprint: uploaded.fingerprint(),
                        synced_unix_secs: now,
                    },
                );
                report.uploaded.push(rel);
            }
            (None, Some(r), None) => {
                stage_counter += 1;
                let staged = stage_remote_download(&client, drive_id, r, &stage_dir, stage_counter, &rel, None, false)?;
                downloads.push(staged);
            }
            (Some(l), None, Some(b)) => {
                // Remote was deleted since baseline. Modify ALWAYS beats delete; otherwise honour propagate_deletes.
                if l.sha256 != b.local_sha256 || !config.propagate_deletes {
                    let (uploaded, sent_sha, sent_size) = upload_local_rel(&client, drive_id, sync_root_id, local_root, &rel, l, &mut remote_dirs)?;
                    next_manifest.insert(
                        rel.clone(),
                        ManifestEntry {
                            local_sha256: sent_sha,
                            local_size: sent_size,
                            remote_id: uploaded.id,
                            remote_fingerprint: uploaded.fingerprint(),
                            synced_unix_secs: now,
                        },
                    );
                    report.uploaded.push(rel);
                }
            }
            (None, Some(r), Some(b)) => {
                // Local was deleted since baseline. Remote modify ALWAYS beats delete; otherwise honour propagate_deletes.
                if r.fingerprint() != b.remote_fingerprint || !config.propagate_deletes {
                    stage_counter += 1;
                    let staged = stage_remote_download(&client, drive_id, r, &stage_dir, stage_counter, &rel, None, false)?;
                    downloads.push(staged);
                }
            }
            (None, None, Some(_)) => {
                next_manifest.remove(&rel);
            }
            (None, None, None) => {}
        }
    }

    for (rel, file_id) in planned_remote_deletes {
        client.trash_file(drive_id, file_id)?;
        next_manifest.remove(&rel);
        report.trashed_remote.push(rel);
    }

    Ok(StagedSync {
        local_root: local_root.to_path_buf(),
        manifest_path,
        target_key,
        stage_dir,
        manifest_entries: next_manifest,
        downloads,
        local_trashes: planned_local_deletes,
        report,
        discovered_drive_id,
    })
}

fn upload_local_rel(
    client: &KDriveClient<'_>,
    drive_id: u64,
    sync_root_id: u64,
    local_root: &Path,
    rel_path: &str,
    local: &LocalEntry,
    remote_dirs: &mut BTreeMap<String, u64>,
) -> Result<(RemoteEntry, String, u64), SyncError> {
    let (parent_rel, file_name) = match rel_path.rsplit_once('/') {
        Some((dir, name)) => (dir, name),
        None => ("", rel_path),
    };
    let parent_id = client.ensure_rel_dir(drive_id, sync_root_id, parent_rel, remote_dirs)?;
    let full = local_path_for(local_root, rel_path)?;
    let bytes = std::fs::read(&full).map_err(|e| SyncError::Io(format!("read {}: {e}", full.display())))?;
    let sent_sha = sha256_bytes(&bytes);
    let sent_size = bytes.len() as u64;
    let uploaded = client.upload_file(drive_id, parent_id, file_name, &bytes, local.mtime_secs)?;
    Ok((uploaded, sent_sha, sent_size))
}

fn stage_remote_download(
    client: &KDriveClient<'_>,
    drive_id: u64,
    remote: &RemoteEntry,
    stage_dir: &Path,
    idx: usize,
    rel_path: &str,
    expected_local_sha256: Option<String>,
    force_conflict_copy: bool,
) -> Result<StagedDownload, SyncError> {
    let bytes = client.download_file(drive_id, remote.id)?;
    let downloaded_sha256 = sha256_bytes(&bytes);
    let downloaded_size = bytes.len() as u64;
    let staged_file = stage_dir.join(format!("item-{idx}.part"));
    std::fs::write(&staged_file, &bytes).map_err(|e| SyncError::Io(format!("write stage {}: {e}", staged_file.display())))?;
    Ok(StagedDownload {
        rel_path: rel_path.to_string(),
        staged_file,
        expected_local_sha256,
        downloaded_sha256,
        downloaded_size,
        remote_id: remote.id,
        remote_fingerprint: remote.fingerprint(),
        force_conflict_copy,
    })
}

/// Phase 2: applies staged downloads and local trash moves atomically, re-checking SHA-256 on each
/// target so concurrent local edits made during the sync are never overwritten.
pub fn apply_staged_sync(mut staged: StagedSync, settings: &Settings) -> Result<SyncReport, SyncError> {
    let now = unix_now_secs();
    for dl in staged.downloads {
        let dest = local_path_for(&staged.local_root, &dl.rel_path)?;
        let current_sha = if dest.is_file() { Some(sha256_file(&dest)?.0) } else { None };
        let concurrent_change = current_sha != dl.expected_local_sha256;

        if dl.force_conflict_copy || concurrent_change {
            if current_sha.as_deref() == Some(dl.downloaded_sha256.as_str()) {
                let _ = std::fs::remove_file(&dl.staged_file);
                continue;
            }
            let conflict_rel = conflict_rel_path(&dl.rel_path, now);
            let conflict_dest = local_path_for(&staged.local_root, &conflict_rel)?;
            write_staged_to_dest(&dl.staged_file, &conflict_dest)?;
            staged.report.conflicts.push(conflict_rel);
            continue;
        }

        write_staged_to_dest(&dl.staged_file, &dest)?;
        staged.manifest_entries.insert(
            dl.rel_path.clone(),
            ManifestEntry {
                local_sha256: dl.downloaded_sha256,
                local_size: dl.downloaded_size,
                remote_id: dl.remote_id,
                remote_fingerprint: dl.remote_fingerprint,
                synced_unix_secs: now,
            },
        );
        staged.report.downloaded.push(dl.rel_path);
    }

    for tr in staged.local_trashes {
        let dest = local_path_for(&staged.local_root, &tr.rel_path)?;
        if dest.is_file()
            && let Ok((cur_sha, _)) = sha256_file(&dest)
            && cur_sha == tr.expected_local_sha256
        {
            let trash_dest = staged.local_root.join(TRASH_DIR_NAME).join(now.to_string()).join(&tr.rel_path);
            if let Some(parent) = trash_dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| SyncError::Io(format!("create sync-trash dir {}: {e}", parent.display())))?;
            }
            if std::fs::rename(&dest, &trash_dest).is_err() {
                std::fs::copy(&dest, &trash_dest).map_err(|e| SyncError::Io(format!("backup {} to sync-trash: {e}", dest.display())))?;
                std::fs::remove_file(&dest).map_err(|e| SyncError::Io(format!("remove {} after sync-trash backup: {e}", dest.display())))?;
            }
            staged.manifest_entries.remove(&tr.rel_path);
            staged.report.trashed_local.push(tr.rel_path);
        }
    }

    let _ = std::fs::remove_dir_all(&staged.stage_dir);
    staged.report.finished_unix_secs = unix_now_secs();
    save_manifest(&staged.manifest_path, &staged.target_key, &staged.manifest_entries)?;
    SyncConfig::record_last_sync(settings, &staged.report);
    Ok(staged.report)
}

pub fn run_sync_blocking(
    local_root: &Path,
    settings: &Settings,
    config: &mut SyncConfig,
    http: &dyn SyncHttpClient,
    secrets: &dyn SecretStore,
    cancel: Option<&AtomicBool>,
) -> Result<SyncReport, SyncError> {
    let staged = stage_sync(local_root, settings, config, http, secrets, cancel)?;
    if let Some(id) = staged.discovered_drive_id {
        config.drive_id = Some(id);
        let _ = config.save(settings);
    }
    apply_staged_sync(staged, settings)
}

fn local_path_for(local_root: &Path, rel_path: &str) -> Result<PathBuf, SyncError> {
    if is_excluded_rel_path(rel_path) {
        return Err(SyncError::SafetyRefused(format!("refused unsafe relative sync path: {rel_path}")));
    }
    let mut out = local_root.to_path_buf();
    for seg in rel_path.split('/') {
        out.push(seg);
    }
    Ok(out)
}

fn conflict_rel_path(rel_path: &str, ts: u64) -> String {
    let (dir, name) = match rel_path.rsplit_once('/') {
        Some((d, n)) => (Some(d), n),
        None => (None, rel_path),
    };
    let conflict_name = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}.conflict-{ts}.{ext}"),
        _ => format!("{name}.conflict-{ts}"),
    };
    match dir {
        Some(d) => format!("{d}/{conflict_name}"),
        None => conflict_name,
    }
}

fn write_staged_to_dest(staged_file: &Path, dest: &Path) -> Result<(), SyncError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SyncError::Io(format!("create {}: {e}", parent.display())))?;
    }
    // Use store's write_atomic when replacing a page's main index.html / index.md so .nocturne/versions snapshots it!
    if dest.is_file()
        && let Some(file_name) = dest.file_name().and_then(|n| n.to_str())
        && (file_name == "index.md" || file_name == "index.html")
        && let Ok(text) = std::fs::read_to_string(staged_file)
    {
        nocturne_store::page::write_atomic(dest, &text).map_err(|e| SyncError::Io(e.to_string()))?;
        let _ = std::fs::remove_file(staged_file);
        return Ok(());
    }
    let tmp = dest.with_extension("tmp-write");
    std::fs::copy(staged_file, &tmp).map_err(|e| SyncError::Io(format!("copy to {}: {e}", tmp.display())))?;
    let _ = std::fs::remove_file(staged_file);
    std::fs::rename(&tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        SyncError::Io(format!("replace {}: {e}", dest.display()))
    })
}
