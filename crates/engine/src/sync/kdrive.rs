//! Infomaniak kDrive API v2/v3 protocol client over `SyncHttpClient`.

use crate::sync::transport::{SyncHttpClient, SyncHttpMethod, SyncHttpRequest, SyncHttpResponse};
use crate::sync::{SyncError, is_excluded_rel_path, normalize_remote_path};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

const ROOT_DIR_ID: u64 = 1;
const MAX_RETRIES: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteEntry {
    pub id: u64,
    pub parent_id: u64,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub last_modified_at: u64,
    pub revised_at: u64,
    pub hash: Option<String>,
    pub etag: Option<String>,
    pub visibility: Option<String>,
}

impl RemoteEntry {
    /// Deterministic remote change fingerprint combining content hash (when kDrive supplies one)
    /// or `etag` + `last_modified_at` + `revised_at` + byte size + file ID when `hash` is null.
    pub fn fingerprint(&self) -> String {
        match self.hash.as_deref().filter(|h| !h.is_empty()) {
            Some(h) => format!("h:{h}:s:{}", self.size),
            None => match self.etag.as_deref().filter(|e| !e.is_empty()) {
                Some(etag) => format!("e:{etag}:m:{}:r:{}:s:{}:id:{}", self.last_modified_at, self.revised_at, self.size, self.id),
                None => format!("m:{}:r:{}:s:{}:id:{}", self.last_modified_at, self.revised_at, self.size, self.id),
            },
        }
    }
}

pub struct KDriveClient<'a> {
    http: &'a dyn SyncHttpClient,
    endpoint: String,
    token: String,
    cancel: Option<&'a AtomicBool>,
}

fn is_loopback_http(url: &str) -> bool {
    let authority = url.split("://").nth(1).and_then(|s| s.split(['/', '?', '#']).next()).unwrap_or("");
    if authority.contains('@') {
        return false;
    }
    for prefix in ["http://127.0.0.1", "http://localhost"] {
        if let Some(rest) = url.strip_prefix(prefix)
            && (rest.is_empty() || rest.starts_with(':') || rest.starts_with('/'))
        {
            return true;
        }
    }
    false
}

impl<'a> KDriveClient<'a> {
    pub fn new(http: &'a dyn SyncHttpClient, endpoint: &str, token: &str, cancel: Option<&'a AtomicBool>) -> Result<Self, SyncError> {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(SyncError::NotConfigured("enter a kDrive Personal API Token before syncing".into()));
        }
        let clean = endpoint.trim().trim_end_matches('/').to_string();
        if !clean.starts_with("https://") && !is_loopback_http(&clean) {
            return Err(SyncError::NotConfigured("sync endpoint must use https:// (or loopback http://127.0.0.1 in local tests)".into()));
        }
        Ok(Self { http, endpoint: clean, token, cancel })
    }

    fn check_cancel(&self) -> Result<(), SyncError> {
        if self.cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(SyncError::Cancelled);
        }
        Ok(())
    }

    fn send_with_retry(
        &self,
        method: SyncHttpMethod,
        path_and_query: &str,
        extra_headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Result<SyncHttpResponse, SyncError> {
        let url = format!("{}{}", self.endpoint, path_and_query);
        let mut attempt = 0usize;
        loop {
            self.check_cancel()?;
            let mut headers =
                vec![("Authorization".to_string(), format!("Bearer {}", self.token)), ("Accept".to_string(), "application/json".to_string())];
            for (k, v) in extra_headers {
                headers.push(((*k).to_string(), (*v).to_string()));
            }
            let resp = self.http.request(SyncHttpRequest { method, url: url.clone(), headers, body: body.clone(), timeout_secs: 60 })?;
            match resp.status {
                200..=299 => return Ok(resp),
                401 | 403 => {
                    let detail = extract_api_error(&resp.body).unwrap_or_else(|| format!("HTTP {}", resp.status));
                    return Err(SyncError::Auth(format!(
                        "kDrive rejected the token ({detail}). Check that the Personal API Token is valid and has the 'drive' scope"
                    )));
                }
                429 | 503 => {
                    let retry_secs = resp.header("Retry-After").and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(1 + attempt as u64).clamp(1, 65);
                    let detail = extract_api_error(&resp.body).unwrap_or_else(|| format!("HTTP {}", resp.status));
                    if attempt >= MAX_RETRIES {
                        return Err(SyncError::RateLimited { retry_after_secs: retry_secs, message: detail });
                    }
                    attempt += 1;
                    sleep_interruptible(retry_secs, self.cancel)?;
                }
                status => {
                    let detail = extract_api_error(&resp.body).unwrap_or_else(|| format!("HTTP {status}"));
                    return Err(SyncError::Protocol(format!("kDrive API error on {path_and_query}: {detail}")));
                }
            }
        }
    }

    fn parse_envelope_data(resp: &SyncHttpResponse, context: &str) -> Result<serde_json::Value, SyncError> {
        let json: serde_json::Value =
            serde_json::from_slice(&resp.body).map_err(|e| SyncError::Protocol(format!("{context}: invalid JSON response: {e}")))?;
        if json.get("result").and_then(|v| v.as_str()) == Some("error") {
            let err_msg = json
                .get("error")
                .and_then(|e| e.get("description").or_else(|| e.get("code")))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown kDrive error");
            return Err(SyncError::Protocol(format!("{context}: {err_msg}")));
        }
        json.get("data").cloned().ok_or_else(|| SyncError::Protocol(format!("{context}: response missing 'data' field")))
    }

    /// Probes an optional JSON discovery endpoint:
    /// - propagates `401` (`SyncError::Auth`), `429`/`503` (`RateLimited`), `Cancelled`, and `Transport`;
    /// - returns `Ok(None)` on `403` (missing account/user scope on a `drive`-only token), `404`, or `422`
    ///   (missing required query parameter such as `account_id`) so fallback probes can run cleanly.
    fn probe_discovery_data(&self, path_and_query: &str) -> Result<Option<serde_json::Value>, SyncError> {
        let url = format!("{}{}", self.endpoint, path_and_query);
        let mut attempt = 0usize;
        loop {
            self.check_cancel()?;
            let headers =
                vec![("Authorization".to_string(), format!("Bearer {}", self.token)), ("Accept".to_string(), "application/json".to_string())];
            let resp =
                self.http.request(SyncHttpRequest { method: SyncHttpMethod::Get, url: url.clone(), headers, body: Vec::new(), timeout_secs: 30 })?;
            match resp.status {
                200..=299 => return Ok(Self::parse_envelope_data(&resp, path_and_query).ok()),
                401 => {
                    let detail = extract_api_error(&resp.body).unwrap_or_else(|| "HTTP 401".into());
                    return Err(SyncError::Auth(format!(
                        "kDrive rejected the token ({detail}). Check that the Personal API Token is valid and has the 'drive' scope"
                    )));
                }
                429 | 503 => {
                    let retry_secs = resp.header("Retry-After").and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(1 + attempt as u64).clamp(1, 65);
                    let detail = extract_api_error(&resp.body).unwrap_or_else(|| format!("HTTP {}", resp.status));
                    if attempt >= MAX_RETRIES {
                        return Err(SyncError::RateLimited { retry_after_secs: retry_secs, message: detail });
                    }
                    attempt += 1;
                    sleep_interruptible(retry_secs, self.cancel)?;
                }
                _ => return Ok(None),
            }
        }
    }

    /// Discovers the user's primary kDrive ID when none was explicitly configured.
    ///
    /// Probes `/2/drive/init`, `/1/my_ksuite/current?with=drive`, `/1/products?service_name=drive`
    /// + `/2/drive?account_id=...`, `/1/accounts` + `/2/drive?account_id=...`, and `/2/drive`.
    pub fn discover_drive_id(&self) -> Result<u64, SyncError> {
        if let Some(data) = self.probe_discovery_data("/2/drive/init?no_avatar_default=1&with=drives")?
            && let Some(id) = data.get("drives").and_then(|d| d.as_array()).and_then(|arr| pick_best_drive_id(arr))
        {
            return Ok(id);
        }
        if let Some(data) = self.probe_discovery_data("/1/my_ksuite/current?with=drive")?
            && let Some(id) = data.get("drive").and_then(|d| d.get("id")).and_then(|v| v.as_u64()).filter(|id| *id > 0)
        {
            return Ok(id);
        }
        let mut account_ids = Vec::new();
        if let Some(data) = self.probe_discovery_data("/1/products?service_name=drive")?
            && let Some(arr) = data.as_array()
        {
            for item in arr {
                if let Some(aid) = item.get("account_id").and_then(|v| v.as_u64()).filter(|id| *id > 0)
                    && !account_ids.contains(&aid)
                {
                    account_ids.push(aid);
                }
            }
        }
        if account_ids.is_empty()
            && let Some(data) = self.probe_discovery_data("/1/accounts")?
            && let Some(arr) = data.as_array()
        {
            for item in arr {
                if let Some(aid) = item.get("id").and_then(|v| v.as_u64()).filter(|id| *id > 0)
                    && !account_ids.contains(&aid)
                {
                    account_ids.push(aid);
                }
            }
        }
        for account_id in account_ids {
            if let Some(data) = self.probe_discovery_data(&format!("/2/drive?account_id={account_id}"))?
                && let Some(arr) = data.as_array()
                && let Some(id) = pick_best_drive_id(arr)
            {
                return Ok(id);
            }
        }
        if let Some(data) = self.probe_discovery_data("/2/drive")?
            && let Some(arr) = data.as_array()
            && let Some(id) = pick_best_drive_id(arr)
        {
            return Ok(id);
        }
        Err(SyncError::NotConfigured(
            "could not auto-discover your kDrive ID with a 'drive'-only token; enter your numeric Drive ID (or paste your https://ksuite.infomaniak.com/.../drive/<ID> URL) in Cloud Sync settings".into(),
        ))
    }

    /// Lists all direct children of `dir_id` using cursor pagination.
    pub fn list_directory(&self, drive_id: u64, dir_id: u64) -> Result<Vec<RemoteEntry>, SyncError> {
        let mut entries = Vec::new();
        let mut cursor: Option<String> = None;
        for _page in 0..200 {
            let query = match cursor.as_deref() {
                Some(c) => format!("/3/drive/{drive_id}/files/{dir_id}/files?limit=1000&with=hash&cursor={}", url_encode(c)),
                None => format!("/3/drive/{drive_id}/files/{dir_id}/files?limit=1000&with=hash"),
            };
            let resp = self.send_with_retry(SyncHttpMethod::Get, &query, &[], Vec::new())?;
            let json: serde_json::Value =
                serde_json::from_slice(&resp.body).map_err(|e| SyncError::Protocol(format!("list directory {dir_id}: invalid JSON: {e}")))?;
            let data = json
                .get("data")
                .and_then(|d| d.as_array())
                .ok_or_else(|| SyncError::Protocol(format!("list directory {dir_id}: expected data array")))?;
            for raw in data {
                if let Some(item) = parse_remote_entry(raw, dir_id) {
                    entries.push(item);
                }
            }
            let has_more = json.get("has_more").and_then(|v| v.as_bool()).unwrap_or(false);
            let next_cursor = json.get("cursor").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
            if has_more {
                if next_cursor.is_none() || next_cursor == cursor {
                    return Err(SyncError::Protocol(format!("list directory {dir_id}: has_more=true without advancing cursor")));
                }
                if _page + 1 == 200 {
                    return Err(SyncError::Protocol(format!("list directory {dir_id}: exceeded 200 paginated pages")));
                }
                cursor = next_cursor;
            } else {
                break;
            }
        }
        Ok(entries)
    }

    /// Creates a child directory inside `parent_id`.
    pub fn create_directory(&self, drive_id: u64, parent_id: u64, name: &str) -> Result<RemoteEntry, SyncError> {
        let path = format!("/3/drive/{drive_id}/files/{parent_id}/directory");
        let payload = serde_json::json!({ "name": name }).to_string();
        let resp = self.send_with_retry(SyncHttpMethod::Post, &path, &[("Content-Type", "application/json")], payload.into_bytes())?;
        let data = Self::parse_envelope_data(&resp, "create directory")?;
        parse_remote_entry(&data, parent_id).ok_or_else(|| SyncError::Protocol(format!("create directory {name}: missing id in response")))
    }

    /// Resolves (and creates if missing) the configured `remote_path` under kDrive's `Private`
    /// user space (`ROOT_ID = 1`), matching the folder synced by the desktop kDrive client.
    pub fn ensure_sync_root(&self, drive_id: u64, remote_path: &str) -> Result<(u64, String), SyncError> {
        let clean = normalize_remote_path(remote_path);
        let segments: Vec<&str> = clean.split('/').filter(|s| !s.is_empty()).collect();
        let top_children = self.list_directory(drive_id, ROOT_DIR_ID)?;

        let (mut current_id, start_idx) = if let Some(first) = segments.first()
            && let Some(matched) = top_children.iter().find(|e| e.is_dir && e.name.eq_ignore_ascii_case(first))
        {
            (matched.id, 1usize)
        } else if let Some(priv_dir) =
            top_children.iter().find(|e| e.is_dir && (e.visibility.as_deref() == Some("is_private_space") || e.name.eq_ignore_ascii_case("Private")))
        {
            (priv_dir.id, 0usize)
        } else {
            (ROOT_DIR_ID, 0usize)
        };

        for seg in segments.iter().skip(start_idx) {
            let children = self.list_directory(drive_id, current_id)?;
            if let Some(existing) = children.iter().find(|e| e.is_dir && e.name == *seg) {
                current_id = existing.id;
            } else {
                let created = self.create_directory(drive_id, current_id, seg)?;
                current_id = created.id;
            }
        }
        Ok((current_id, clean))
    }

    /// Walks the remote sync subtree directory-by-directory, returning `(files_by_rel_path, dirs_by_rel_path)`.
    pub fn walk_subtree(&self, drive_id: u64, root_dir_id: u64) -> Result<(BTreeMap<String, RemoteEntry>, BTreeMap<String, u64>), SyncError> {
        let mut files = BTreeMap::new();
        let mut dirs = BTreeMap::new();
        dirs.insert(String::new(), root_dir_id);
        let mut stack = vec![(String::new(), root_dir_id)];
        while let Some((prefix, dir_id)) = stack.pop() {
            self.check_cancel()?;
            let children = self.list_directory(drive_id, dir_id)?;
            for child in children {
                let rel = if prefix.is_empty() { child.name.clone() } else { format!("{prefix}/{}", child.name) };
                if is_excluded_rel_path(&rel) {
                    continue;
                }
                if child.is_dir {
                    dirs.insert(rel.clone(), child.id);
                    stack.push((rel, child.id));
                } else {
                    files.insert(rel, child);
                }
            }
        }
        Ok((files, dirs))
    }

    /// Ensures every directory segment in `rel_dir` exists under `root_dir_id`, caching IDs in `dirs`.
    pub fn ensure_rel_dir(&self, drive_id: u64, root_dir_id: u64, rel_dir: &str, dirs: &mut BTreeMap<String, u64>) -> Result<u64, SyncError> {
        if rel_dir.is_empty() {
            return Ok(root_dir_id);
        }
        if let Some(id) = dirs.get(rel_dir) {
            return Ok(*id);
        }
        let mut current_id = root_dir_id;
        let mut built = String::new();
        for seg in rel_dir.split('/').filter(|s| !s.is_empty()) {
            built = if built.is_empty() { seg.to_string() } else { format!("{built}/{seg}") };
            if let Some(id) = dirs.get(&built) {
                current_id = *id;
                continue;
            }
            let children = self.list_directory(drive_id, current_id)?;
            if let Some(existing) = children.iter().find(|e| e.is_dir && e.name == seg) {
                current_id = existing.id;
                dirs.insert(built.clone(), current_id);
            } else {
                let created = self.create_directory(drive_id, current_id, seg)?;
                current_id = created.id;
                dirs.insert(built.clone(), current_id);
            }
        }
        Ok(current_id)
    }

    /// Uploads raw file bytes into `parent_dir_id` (`conflict=version` replaces existing files cleanly).
    pub fn upload_file(
        &self,
        drive_id: u64,
        parent_dir_id: u64,
        file_name: &str,
        bytes: &[u8],
        last_modified_at: u64,
    ) -> Result<RemoteEntry, SyncError> {
        let query = format!(
            "/3/drive/{drive_id}/upload?directory_id={parent_dir_id}&total_size={}&file_name={}&conflict=version&last_modified_at={last_modified_at}&with=hash",
            bytes.len(),
            url_encode(file_name)
        );
        let resp = self.send_with_retry(SyncHttpMethod::Post, &query, &[("Content-Type", "application/octet-stream")], bytes.to_vec())?;
        let data = Self::parse_envelope_data(&resp, "upload file")?;
        parse_remote_entry(&data, parent_dir_id).ok_or_else(|| SyncError::Protocol(format!("upload {file_name}: missing file metadata")))
    }

    /// Downloads raw file bytes for `file_id`.
    pub fn download_file(&self, drive_id: u64, file_id: u64) -> Result<Vec<u8>, SyncError> {
        let path = format!("/2/drive/{drive_id}/files/{file_id}/download");
        let resp = self.send_with_retry(SyncHttpMethod::Get, &path, &[("Accept", "*/*")], Vec::new())?;
        Ok(resp.body)
    }

    /// Moves `file_id` to kDrive's trash (`DELETE /2/drive/{drive_id}/files/{file_id}`).
    /// Treats HTTP 404 as already deleted so re-running after a partial trash pass is idempotent.
    pub fn trash_file(&self, drive_id: u64, file_id: u64) -> Result<(), SyncError> {
        let path = format!("/2/drive/{drive_id}/files/{file_id}");
        let url = format!("{}{}", self.endpoint, path);
        let headers = vec![("Authorization".to_string(), format!("Bearer {}", self.token)), ("Accept".to_string(), "application/json".to_string())];
        let resp = self.http.request(SyncHttpRequest { method: SyncHttpMethod::Delete, url, headers, body: Vec::new(), timeout_secs: 60 })?;
        if resp.status == 404 || (200..=299).contains(&resp.status) {
            return Ok(());
        }
        let _ = self.send_with_retry(SyncHttpMethod::Delete, &path, &[], Vec::new())?;
        Ok(())
    }
}

fn pick_best_drive_id(drives: &[serde_json::Value]) -> Option<u64> {
    drives
        .iter()
        .filter_map(|item| {
            let obj = item.get("drive").unwrap_or(item);
            obj.get("id").and_then(|v| v.as_u64()).filter(|id| *id > 0)
        })
        .next()
}

fn parse_remote_entry(raw: &serde_json::Value, fallback_parent: u64) -> Option<RemoteEntry> {
    let id = raw.get("id")?.as_u64()?;
    let name = raw.get("name")?.as_str()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let parent_id = raw.get("parent_id").and_then(|v| v.as_u64()).unwrap_or(fallback_parent);
    let kind = raw.get("type").and_then(|v| v.as_str()).unwrap_or("file");
    let is_dir = kind.eq_ignore_ascii_case("dir");
    let size = raw.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
    let last_modified_at = raw.get("last_modified_at").and_then(|v| v.as_u64()).unwrap_or(0);
    let revised_at = raw.get("revised_at").and_then(|v| v.as_u64()).unwrap_or(0);
    let hash = raw.get("hash").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let etag = raw.get("etag").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let visibility = raw.get("visibility").and_then(|v| v.as_str()).map(str::to_string);
    Some(RemoteEntry { id, parent_id, name, is_dir, size, last_modified_at, revised_at, hash, etag, visibility })
}

fn extract_api_error(body: &[u8]) -> Option<String> {
    let json: serde_json::Value = serde_json::from_slice(body).ok()?;
    json.get("error").and_then(|e| e.get("description").or_else(|| e.get("code"))).and_then(|v| v.as_str()).map(str::to_string)
}

pub fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(char::from(byte)),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn sleep_interruptible(secs: u64, cancel: Option<&AtomicBool>) -> Result<(), SyncError> {
    let steps = (secs.saturating_mul(10)).max(1);
    for _ in 0..steps {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(SyncError::Cancelled);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}
