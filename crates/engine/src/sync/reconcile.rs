//! Two-phase 3-way folder reconciler for local-first kDrive sync.

use crate::sync::kdrive::{KDriveClient, RemoteEntry};
use crate::sync::manifest::{
    LocalEntry, ManifestEntry, is_excluded_rel_path, load_manifest, make_target_key, manifest_path_for, save_manifest, scan_local_tree,
    scan_recycled_prefixes, sha256_bytes, sha256_file, unix_now_secs,
};
use crate::sync::transport::{SecretStore, SyncHttpClient};
use crate::sync::{KDRIVE_TOKEN_KEY, SyncConfig, SyncError, SyncReport};
use nocturne_store::Settings;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const STAGE_DIR_NAME: &str = ".nocturne-sync-stage";
const TRASH_DIR_NAME: &str = ".sync-trash";
const MAX_SAFE_DELETE_COUNT: usize = 10;

static STAGE_RUN_SEQ: AtomicU64 = AtomicU64::new(1);
static WRITE_TMP_SEQ: AtomicU64 = AtomicU64::new(1);

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

impl Drop for StagedSync {
    fn drop(&mut self) {
        if self.stage_dir.exists() {
            let _ = std::fs::remove_dir_all(&self.stage_dir);
        }
    }
}

impl StagedSync {
    /// Relative paths of local files that were written or will be written/trashed by this sync pass.
    pub fn touched_local_paths(&self) -> Vec<&str> {
        self.downloads
            .iter()
            .map(|d| d.rel_path.as_str())
            .chain(self.local_trashes.iter().map(|t| t.rel_path.as_str()))
            .chain(self.report.conflicts.iter().map(String::as_str))
            .collect()
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
    let recycled_prefixes = scan_recycled_prefixes(local_root);
    let (remote_files, remote_dirs) = client.walk_subtree(drive_id, sync_root_id)?;

    let manifest_path = manifest_path_for(settings);
    let target_key = make_target_key(local_root, &config.endpoint, drive_id, &normalized_remote);
    let baseline = load_manifest(&manifest_path, &target_key);

    let now = unix_now_secs();
    let stage_parent = local_root.join(STAGE_DIR_NAME);
    let pid_tag = format!("-{}-", std::process::id());
    let pid_suffix = format!("-{}", std::process::id());
    if let Ok(entries) = std::fs::read_dir(&stage_parent) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && name.starts_with("run-")
                && !name.contains(&pid_tag)
                && !name.ends_with(&pid_suffix)
            {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let seq = STAGE_RUN_SEQ.fetch_add(1, Ordering::Relaxed);
    let stage_dir = stage_parent.join(format!("run-{now}{pid_tag}{seq}"));
    let _ = std::fs::remove_dir_all(&stage_dir);
    std::fs::create_dir_all(&stage_dir).map_err(|e| SyncError::Io(format!("create stage dir {}: {e}", stage_dir.display())))?;

    let stage_res = stage_sync_into_dir(
        &client,
        drive_id,
        discovered_drive_id,
        sync_root_id,
        normalized_remote,
        local_root,
        &local_files,
        &recycled_prefixes,
        &remote_files,
        remote_dirs,
        manifest_path,
        target_key,
        baseline,
        &stage_dir,
        now,
        config,
        cancel,
    );
    if stage_res.is_err() {
        let _ = std::fs::remove_dir_all(&stage_dir);
    }
    stage_res
}

fn is_recycled_rel_path(rel: &str, recycled_prefixes: &BTreeSet<String>) -> bool {
    recycled_prefixes.iter().any(|pfx| rel == pfx || rel.starts_with(&format!("{pfx}/")))
}

#[allow(clippy::too_many_arguments)]
fn stage_sync_into_dir(
    client: &KDriveClient<'_>,
    drive_id: u64,
    discovered_drive_id: Option<u64>,
    sync_root_id: u64,
    normalized_remote: String,
    local_root: &Path,
    local_files: &BTreeMap<String, LocalEntry>,
    recycled_prefixes: &BTreeSet<String>,
    remote_files: &BTreeMap<String, RemoteEntry>,
    mut remote_dirs: BTreeMap<String, u64>,
    manifest_path: PathBuf,
    target_key: String,
    baseline: BTreeMap<String, ManifestEntry>,
    stage_dir: &Path,
    now: u64,
    config: &SyncConfig,
    cancel: Option<&AtomicBool>,
) -> Result<StagedSync, SyncError> {
    let mut all_paths = BTreeSet::new();
    all_paths.extend(local_files.keys().cloned());
    all_paths.extend(remote_files.keys().cloned());
    all_paths.extend(baseline.keys().cloned());

    // Pre-scan deletion circuit breakers BEFORE performing any remote upload or delete!
    // Explicitly recycled items in `<notebook>/.nocturne/recycle-bin` always trash their remote
    // counterpart (up to 50 files per pass) even when `propagate_deletes == false` so deleted
    // pages/sections do not reappear on the next sync.
    let mut planned_remote_deletes: Vec<(String, u64)> = Vec::new();
    let mut deferred_recycled: BTreeSet<String> = BTreeSet::new();
    let mut planned_local_deletes: Vec<StagedLocalTrash> = Vec::new();
    if config.propagate_deletes && !baseline.is_empty() && (local_files.is_empty() || remote_files.is_empty()) {
        return Err(SyncError::SafetyRefused("refusing to propagate deletions because one side of the sync folder is completely empty".into()));
    }
    let mut unrecycled_deletes = 0usize;
    for rel in &all_paths {
        if is_excluded_rel_path(rel) {
            continue;
        }
        match (local_files.get(rel), remote_files.get(rel), baseline.get(rel)) {
            (Some(l), None, Some(b)) if config.propagate_deletes && l.sha256 == b.local_sha256 => {
                unrecycled_deletes += 1;
                planned_local_deletes.push(StagedLocalTrash { rel_path: rel.clone(), expected_local_sha256: b.local_sha256.clone() });
            }
            (None, Some(r), Some(b)) if r.fingerprint() == b.remote_fingerprint => {
                let explicitly_recycled = is_recycled_rel_path(rel, recycled_prefixes);
                if explicitly_recycled {
                    if planned_remote_deletes.len() < 50 {
                        planned_remote_deletes.push((rel.clone(), r.id));
                    } else {
                        deferred_recycled.insert(rel.clone());
                    }
                } else if config.propagate_deletes {
                    unrecycled_deletes += 1;
                    planned_remote_deletes.push((rel.clone(), r.id));
                }
            }
            _ => {}
        }
    }
    if config.propagate_deletes && (unrecycled_deletes > MAX_SAFE_DELETE_COUNT || (!baseline.is_empty() && unrecycled_deletes * 2 > baseline.len())) {
        return Err(SyncError::SafetyRefused(format!(
            "refusing to delete {unrecycled_deletes} files in one sync pass (limit {MAX_SAFE_DELETE_COUNT}); turn off delete propagation or delete manually"
        )));
    }

    let mut next_manifest = baseline.clone();
    let mut downloads = Vec::new();
    let mut report = SyncReport { drive_id, remote_path: normalized_remote, finished_unix_secs: now, ..SyncReport::default() };
    let mut stage_counter = 0usize;

    for rel in all_paths {
        if cancel.is_some_and(|f| f.load(Ordering::Relaxed)) {
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
                            upload_local_rel(client, drive_id, sync_root_id, local_root, &rel, l, &mut remote_dirs)?;
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
                        let staged = stage_remote_download(client, drive_id, r, stage_dir, stage_counter, &rel, Some(l.sha256.clone()), false)?;
                        // If kDrive populated `hash` asynchronously without content changing (`downloaded_sha256 == l.sha256`),
                        // refresh the manifest fingerprint directly without rewriting the local file or adding a version snapshot.
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
                            downloads.push(staged);
                        }
                    }
                    (true, true) => {
                        // Download remote bytes FIRST:
                        // - If `downloaded_sha256 == l.sha256`, both sides made identical edits -> unchanged.
                        // - If `downloaded_sha256 == b.local_sha256`, remote content never changed since baseline
                        //   (e.g. kDrive's async `hash` arrived after upload) -> upload local edit without false conflict!
                        // - Otherwise persist remote conflict copy BEFORE uploading local over remote!
                        stage_counter += 1;
                        let staged = stage_remote_download(client, drive_id, r, stage_dir, stage_counter, &rel, Some(l.sha256.clone()), true)?;
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
                        } else if staged.downloaded_sha256 == b.local_sha256 {
                            let _ = std::fs::remove_file(&staged.staged_file);
                            let (uploaded, sent_sha, sent_size) =
                                upload_local_rel(client, drive_id, sync_root_id, local_root, &rel, l, &mut remote_dirs)?;
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
                        } else {
                            let (conflict_rel, conflict_dest) = unique_conflict_dest(local_root, &rel, now)?;
                            write_staged_to_dest(&staged.staged_file, &conflict_dest)?;
                            report.conflicts.push(conflict_rel);
                            let (uploaded, sent_sha, sent_size) =
                                upload_local_rel(client, drive_id, sync_root_id, local_root, &rel, l, &mut remote_dirs)?;
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
                let staged = stage_remote_download(client, drive_id, r, stage_dir, stage_counter, &rel, Some(l.sha256.clone()), true)?;
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
                    let (conflict_rel, conflict_dest) = unique_conflict_dest(local_root, &rel, now)?;
                    write_staged_to_dest(&staged.staged_file, &conflict_dest)?;
                    report.conflicts.push(conflict_rel);
                    let (uploaded, sent_sha, sent_size) = upload_local_rel(client, drive_id, sync_root_id, local_root, &rel, l, &mut remote_dirs)?;
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
                let (uploaded, sent_sha, sent_size) = upload_local_rel(client, drive_id, sync_root_id, local_root, &rel, l, &mut remote_dirs)?;
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
                let staged = stage_remote_download(client, drive_id, r, stage_dir, stage_counter, &rel, None, false)?;
                downloads.push(staged);
            }
            (Some(l), None, Some(b)) => {
                // Remote was deleted since baseline. Modify ALWAYS beats delete; otherwise honour propagate_deletes.
                if l.sha256 != b.local_sha256 || !config.propagate_deletes {
                    let (uploaded, sent_sha, sent_size) = upload_local_rel(client, drive_id, sync_root_id, local_root, &rel, l, &mut remote_dirs)?;
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
                // Local was deleted since baseline. Remote modify ALWAYS beats delete; otherwise
                // honour explicit local recycle-bin deletion (including items deferred past the
                // 50-item batch cap) or `propagate_deletes`.
                let planned_trash = planned_remote_deletes.iter().any(|(p, _)| p == &rel);
                if r.fingerprint() != b.remote_fingerprint {
                    stage_counter += 1;
                    let staged = stage_remote_download(client, drive_id, r, stage_dir, stage_counter, &rel, None, false)?;
                    downloads.push(staged);
                } else if deferred_recycled.contains(&rel) {
                    // Keep the baseline entry in `next_manifest` without downloading so the next
                    // sync pass trashes the next batch instead of resurrecting deleted files locally.
                } else if !planned_trash {
                    stage_counter += 1;
                    let staged = stage_remote_download(client, drive_id, r, stage_dir, stage_counter, &rel, None, false)?;
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
        if cancel.is_some_and(|f| f.load(Ordering::Relaxed)) {
            return Err(SyncError::Cancelled);
        }
        client.trash_file(drive_id, file_id)?;
        next_manifest.remove(&rel);
        report.trashed_remote.push(rel);
    }

    Ok(StagedSync {
        local_root: local_root.to_path_buf(),
        manifest_path,
        target_key,
        stage_dir: stage_dir.to_path_buf(),
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
    let downloads = std::mem::take(&mut staged.downloads);
    for dl in downloads {
        let dest = local_path_for(&staged.local_root, &dl.rel_path)?;
        let current_sha = if dest.is_file() { Some(sha256_file(&dest)?.0) } else { None };
        let concurrent_change = current_sha != dl.expected_local_sha256;

        if dl.force_conflict_copy || concurrent_change {
            // If the file was deleted locally mid-sync while remote modified it, modify beats delete:
            // write the remote revision directly to `dest` instead of stranding a `.conflict-*` orphan.
            if current_sha.is_none() && !dl.force_conflict_copy {
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
                continue;
            }
            if current_sha.as_deref() == Some(dl.downloaded_sha256.as_str()) {
                let _ = std::fs::remove_file(&dl.staged_file);
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
                continue;
            }
            let (conflict_rel, conflict_dest) = unique_conflict_dest(&staged.local_root, &dl.rel_path, now)?;
            write_staged_to_dest(&dl.staged_file, &conflict_dest)?;
            staged.report.conflicts.push(conflict_rel);
            // Record the remote fingerprint we just materialized into `.conflict-<ts>` while keeping
            // `local_sha256` at `expected_local_sha256`, so the next sync pass uploads the new local
            // edit cleanly instead of creating a duplicate conflict copy for this same remote version.
            let prev_local_sha = dl
                .expected_local_sha256
                .clone()
                .or_else(|| staged.manifest_entries.get(&dl.rel_path).map(|m| m.local_sha256.clone()))
                .unwrap_or_default();
            let prev_local_size = staged.manifest_entries.get(&dl.rel_path).map_or(0, |m| m.local_size);
            staged.manifest_entries.insert(
                dl.rel_path.clone(),
                ManifestEntry {
                    local_sha256: prev_local_sha,
                    local_size: prev_local_size,
                    remote_id: dl.remote_id,
                    remote_fingerprint: dl.remote_fingerprint,
                    synced_unix_secs: now,
                },
            );
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

    let local_trashes = std::mem::take(&mut staged.local_trashes);
    for tr in local_trashes {
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
    Ok(std::mem::take(&mut staged.report))
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

fn conflict_rel_path(rel_path: &str, ts: u64, suffix: Option<usize>) -> String {
    let (dir, name) = match rel_path.rsplit_once('/') {
        Some((d, n)) => (Some(d), n),
        None => (None, rel_path),
    };
    let tag = match suffix {
        Some(n) => format!("conflict-{ts}-{n}"),
        None => format!("conflict-{ts}"),
    };
    let conflict_name = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}.{tag}.{ext}"),
        _ => format!("{name}.{tag}"),
    };
    match dir {
        Some(d) => format!("{d}/{conflict_name}"),
        None => conflict_name,
    }
}

/// Allocates a conflict copy path under `local_root` that does not collide with any existing file,
/// guaranteeing an earlier conflict copy is never overwritten within the same timestamp second.
fn unique_conflict_dest(local_root: &Path, rel_path: &str, ts: u64) -> Result<(String, PathBuf), SyncError> {
    for attempt in 0usize..10_000 {
        let suffix = if attempt == 0 { None } else { Some(attempt + 1) };
        let rel = conflict_rel_path(rel_path, ts, suffix);
        let dest = local_path_for(local_root, &rel)?;
        if !dest.exists() {
            return Ok((rel, dest));
        }
    }
    Err(SyncError::Io(format!("could not allocate unique conflict filename for {rel_path}")))
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
    let seq = WRITE_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let base = dest.file_name().and_then(|n| n.to_str()).unwrap_or("sync");
    let tmp_name = format!(".{base}.{}.{seq}.tmp-write", std::process::id());
    let tmp = dest.parent().map_or_else(|| PathBuf::from(&tmp_name), |p| p.join(&tmp_name));
    std::fs::copy(staged_file, &tmp).map_err(|e| SyncError::Io(format!("copy to {}: {e}", tmp.display())))?;
    let _ = std::fs::remove_file(staged_file);
    std::fs::rename(&tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        SyncError::Io(format!("replace {}: {e}", dest.display()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    #[test]
    fn more_than_50_recycled_files_trash_in_batches_without_resurrection_or_safety_refusal() {
        use crate::sync::UreqSyncClient;
        use nocturne_store::testing::TempDir;
        use std::io::{BufRead, BufReader, Write};
        use std::net::{TcpListener, TcpStream};
        use std::sync::{Arc, Mutex};
        use std::thread;

        for propagate_deletes in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
            let addr = listener.local_addr().expect("local addr");
            let stop = Arc::new(AtomicBool::new(false));
            let stop_srv = Arc::clone(&stop);
            let trashed_ids = Arc::new(Mutex::new(Vec::<u64>::new()));
            let trashed_srv = Arc::clone(&trashed_ids);
            let unexpected_calls = Arc::new(Mutex::new(Vec::<String>::new()));
            let unexpected_srv = Arc::clone(&unexpected_calls);

            let server = thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop_srv.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(mut stream) = stream else { break };
                    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                    let mut req_line = String::new();
                    if reader.read_line(&mut req_line).unwrap_or(0) == 0 {
                        continue;
                    }
                    loop {
                        let mut hdr = String::new();
                        if reader.read_line(&mut hdr).unwrap_or(0) == 0 || hdr == "\r\n" || hdr == "\n" {
                            break;
                        }
                    }
                    let mut parts = req_line.split_whitespace();
                    let method = parts.next().unwrap_or("");
                    let path = parts.next().unwrap_or("");
                    if method == "DELETE"
                        && let Some(id) = path.rsplit('/').next().and_then(|seg| seg.parse::<u64>().ok())
                    {
                        trashed_srv.lock().expect("lock").push(id);
                        let _ = stream
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
                    } else {
                        unexpected_srv.lock().expect("lock").push(req_line.trim().to_string());
                        let _ = stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                    let _ = stream.flush();
                }
            });

            let tmp = TempDir::new(&format!("reconcile-recycled-batch-{propagate_deletes}"));
            let endpoint = format!("http://{addr}");
            let http = UreqSyncClient::new();
            let client = KDriveClient::new(&http, &endpoint, "test-token", None).expect("client");

            let kept_rel = "nb/_node.toml".to_string();
            let kept_sha = sha256_bytes(b"kind = \"notebook\"\n");
            let mut local_files = BTreeMap::new();
            local_files.insert(kept_rel.clone(), LocalEntry { sha256: kept_sha.clone(), size: 18, mtime_secs: 1_700_000_000 });

            let mut remote_files = BTreeMap::new();
            let mut baseline = BTreeMap::new();
            let kept_remote = RemoteEntry {
                id: 1,
                parent_id: 10,
                name: "_node.toml".into(),
                is_dir: false,
                size: 18,
                last_modified_at: 1_700_000_000,
                revised_at: 1_700_000_000,
                hash: Some("h-kept".into()),
                etag: None,
                visibility: None,
            };
            baseline.insert(
                kept_rel.clone(),
                ManifestEntry {
                    local_sha256: kept_sha,
                    local_size: 18,
                    remote_id: kept_remote.id,
                    remote_fingerprint: kept_remote.fingerprint(),
                    synced_unix_secs: 1_700_000_000,
                },
            );
            remote_files.insert(kept_rel.clone(), kept_remote);

            let total_recycled = 62usize;
            for i in 0..total_recycled {
                let rel = format!("nb/old-section/page-{i:02}/index.md");
                let id = 100 + i as u64;
                let remote = RemoteEntry {
                    id,
                    parent_id: 20,
                    name: "index.md".into(),
                    is_dir: false,
                    size: 12,
                    last_modified_at: 1_700_000_000,
                    revised_at: 1_700_000_000,
                    hash: Some(format!("h-{id}")),
                    etag: None,
                    visibility: None,
                };
                baseline.insert(
                    rel.clone(),
                    ManifestEntry {
                        local_sha256: format!("sha-{id}"),
                        local_size: 12,
                        remote_id: id,
                        remote_fingerprint: remote.fingerprint(),
                        synced_unix_secs: 1_700_000_000,
                    },
                );
                remote_files.insert(rel, remote);
            }

            let mut recycled_prefixes = BTreeSet::new();
            recycled_prefixes.insert("nb/old-section".to_string());
            let config = SyncConfig { propagate_deletes, ..SyncConfig::default() };

            // Pass 1: trashes the first 50, defers the remaining 12 without downloading or SafetyRefused.
            let stage_dir_1 = tmp.path().join("stage-1");
            std::fs::create_dir_all(&stage_dir_1).expect("stage 1");
            let staged_1 = stage_sync_into_dir(
                &client,
                7,
                None,
                10,
                "Nocturne".into(),
                tmp.path(),
                &local_files,
                &recycled_prefixes,
                &remote_files,
                BTreeMap::new(),
                tmp.path().join("manifest.json"),
                "key".into(),
                baseline.clone(),
                &stage_dir_1,
                1_700_000_100,
                &config,
                None,
            )
            .expect("pass 1 must succeed without SafetyRefused");

            assert_eq!(staged_1.report.trashed_remote.len(), 50, "propagate_deletes={propagate_deletes}");
            assert!(staged_1.downloads.is_empty(), "deferred recycled files must not be re-downloaded");
            assert!(staged_1.report.downloaded.is_empty());
            assert_eq!(staged_1.manifest_entries.len(), 1 + (total_recycled - 50));
            for rel in &staged_1.report.trashed_remote {
                assert!(!staged_1.manifest_entries.contains_key(rel));
                remote_files.remove(rel);
            }
            for (rel, entry) in &baseline {
                if !staged_1.report.trashed_remote.contains(rel) {
                    assert_eq!(staged_1.manifest_entries.get(rel), Some(entry), "deferred entry must keep its baseline");
                }
            }

            // Pass 2: trashes the remaining 12 deferred files cleanly.
            let stage_dir_2 = tmp.path().join("stage-2");
            std::fs::create_dir_all(&stage_dir_2).expect("stage 2");
            let staged_2 = stage_sync_into_dir(
                &client,
                7,
                None,
                10,
                "Nocturne".into(),
                tmp.path(),
                &local_files,
                &recycled_prefixes,
                &remote_files,
                BTreeMap::new(),
                tmp.path().join("manifest.json"),
                "key".into(),
                staged_1.manifest_entries.clone(),
                &stage_dir_2,
                1_700_000_200,
                &config,
                None,
            )
            .expect("pass 2 must succeed");

            assert_eq!(staged_2.report.trashed_remote.len(), total_recycled - 50);
            assert!(staged_2.downloads.is_empty());
            assert_eq!(staged_2.manifest_entries.len(), 1);
            assert!(staged_2.manifest_entries.contains_key(&kept_rel));

            stop.store(true, Ordering::Relaxed);
            let _ = TcpStream::connect(addr);
            let _ = server.join();

            assert_eq!(trashed_ids.lock().expect("lock").len(), total_recycled);
            assert!(unexpected_calls.lock().expect("lock").is_empty(), "no downloads should have been requested");
        }
    }
}
