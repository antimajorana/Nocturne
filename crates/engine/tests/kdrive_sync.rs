//! End-to-end integration tests for kDrive free-tier REST sync against a real loopback TCP HTTP server.

use nocturne_engine::sync::{SyncConfig, apply_staged_sync, is_excluded_rel_path, sha256_bytes, stage_sync};
use nocturne_engine::{Output, Params, Session};
use nocturne_store::Settings;
use nocturne_store::testing::TempDir;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Clone, Debug)]
struct ServerNode {
    id: u64,
    parent_id: u64,
    name: String,
    is_dir: bool,
    bytes: Vec<u8>,
    last_modified_at: u64,
    revised_at: u64,
    omit_hash: bool,
    visibility: Option<String>,
    trashed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiscoveryMode {
    DriveInit,
    OpenApiAccountProducts,
    DriveScopeOnlyNoAccount,
}

struct LoopbackState {
    expected_token: String,
    drive_id: u64,
    next_id: u64,
    nodes: BTreeMap<u64, ServerNode>,
    page_size: usize,
    rate_limit_next: usize,
    discovery_mode: DiscoveryMode,
    redirect_downloads: bool,
}

impl LoopbackState {
    fn new(token: &str, drive_id: u64) -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            1,
            ServerNode {
                id: 1,
                parent_id: 0,
                name: "Root".into(),
                is_dir: true,
                bytes: Vec::new(),
                last_modified_at: 1_700_000_000,
                revised_at: 1_700_000_000,
                omit_hash: false,
                visibility: Some("is_root".into()),
                trashed: false,
            },
        );
        // kDrive's virtual root 1 contains "Private" (is_private_space), which maps to Desktop kDrive!
        nodes.insert(
            2,
            ServerNode {
                id: 2,
                parent_id: 1,
                name: "Private".into(),
                is_dir: true,
                bytes: Vec::new(),
                last_modified_at: 1_700_000_000,
                revised_at: 1_700_000_000,
                omit_hash: false,
                visibility: Some("is_private_space".into()),
                trashed: false,
            },
        );
        Self {
            expected_token: token.into(),
            drive_id,
            next_id: 10,
            nodes,
            page_size: 2,
            rate_limit_next: 0,
            discovery_mode: DiscoveryMode::DriveInit,
            redirect_downloads: false,
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn ensure_dir_chain_under_private(&mut self, rel: &str) -> u64 {
        let mut cur = 2u64;
        for seg in rel.split('/').filter(|s| !s.is_empty()) {
            if let Some(found) = self.nodes.values().find(|n| !n.trashed && n.is_dir && n.parent_id == cur && n.name == seg).map(|n| n.id) {
                cur = found;
            } else {
                let id = self.alloc_id();
                self.nodes.insert(
                    id,
                    ServerNode {
                        id,
                        parent_id: cur,
                        name: seg.into(),
                        is_dir: true,
                        bytes: Vec::new(),
                        last_modified_at: 1_700_000_100,
                        revised_at: 1_700_000_100,
                        omit_hash: false,
                        visibility: None,
                        trashed: false,
                    },
                );
                cur = id;
            }
        }
        cur
    }

    fn put_file(&mut self, parent_id: u64, name: &str, bytes: &[u8], mtime: u64) -> ServerNode {
        if let Some(existing_id) = self.nodes.values().find(|n| !n.trashed && !n.is_dir && n.parent_id == parent_id && n.name == name).map(|n| n.id) {
            let node = self.nodes.get_mut(&existing_id).expect("existing");
            node.bytes = bytes.to_vec();
            node.last_modified_at = mtime;
            node.revised_at = node.revised_at.saturating_add(1).max(mtime);
            return node.clone();
        }
        let id = self.alloc_id();
        let node = ServerNode {
            id,
            parent_id,
            name: name.into(),
            is_dir: false,
            bytes: bytes.to_vec(),
            last_modified_at: mtime,
            revised_at: mtime,
            omit_hash: false,
            visibility: None,
            trashed: false,
        };
        self.nodes.insert(id, node.clone());
        node
    }

    fn find_under_private(&self, rel: &str) -> Option<&ServerNode> {
        let mut cur = 2u64;
        let parts: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        for (i, seg) in parts.iter().enumerate() {
            let last = i + 1 == parts.len();
            let node = self.nodes.values().find(|n| !n.trashed && n.parent_id == cur && n.name == *seg && n.is_dir != last)?;
            if last {
                return Some(node);
            }
            cur = node.id;
        }
        None
    }
}

struct LoopbackServer {
    endpoint: String,
    state: Arc<Mutex<LoopbackState>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl LoopbackServer {
    fn start(token: &str, drive_id: u64) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        listener.set_nonblocking(true).expect("nonblocking");
        let addr = listener.local_addr().expect("addr");
        let state = Arc::new(Mutex::new(LoopbackState::new(token, drive_id)));
        let stop = Arc::new(AtomicBool::new(false));
        let state_bg = Arc::clone(&state);
        let stop_bg = Arc::clone(&stop);

        let handle = thread::spawn(move || {
            while !stop_bg.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        handle_conn(stream, &state_bg);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        Self { endpoint: format!("http://{addr}"), state, stop, handle: Some(handle) }
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn node_json(n: &ServerNode) -> serde_json::Value {
    let hash = if n.is_dir || n.omit_hash { serde_json::Value::Null } else { serde_json::Value::String(sha256_bytes(&n.bytes)) };
    serde_json::json!({
        "id": n.id,
        "parent_id": n.parent_id,
        "name": n.name,
        "type": if n.is_dir { "dir" } else { "file" },
        "size": n.bytes.len(),
        "last_modified_at": n.last_modified_at,
        "revised_at": n.revised_at,
        "hash": hash,
        "visibility": n.visibility,
    })
}

fn url_decode(s: &str) -> String {
    let mut bytes = Vec::new();
    let raw = s.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && i + 2 < raw.len()
            && let Ok(hex) = std::str::from_utf8(&raw[i + 1..i + 3])
            && let Ok(val) = u8::from_str_radix(hex, 16)
        {
            bytes.push(val);
            i += 3;
            continue;
        }
        bytes.push(raw[i]);
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn parse_query(q: &str) -> BTreeMap<String, String> {
    q.split('&').filter(|p| !p.is_empty()).filter_map(|p| p.split_once('=')).map(|(k, v)| (k.to_string(), url_decode(v))).collect()
}

fn write_http(mut stream: std::net::TcpStream, status: u16, extra: &[(&str, &str)], body: &[u8]) {
    let reason = match status {
        200 => "OK",
        302 => "Found",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        _ => "Bad Request",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn handle_conn(stream: std::net::TcpStream, state: &Arc<Mutex<LoopbackState>>) {
    let Ok(cloned) = stream.try_clone() else { return };
    let mut reader = BufReader::new(cloned);
    let mut req_line = String::new();
    if reader.read_line(&mut req_line).is_err() || req_line.is_empty() {
        return;
    }
    let parts: Vec<&str> = req_line.split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }
    let (method, full_target) = (parts[0], parts[1]);
    let (path, query_str) = full_target.split_once('?').unwrap_or((full_target, ""));
    let query = parse_query(query_str);

    let mut host_hdr = String::new();
    let mut auth = String::new();
    let mut content_len = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("authorization") {
                auth = v.trim().to_string();
            } else if k.trim().eq_ignore_ascii_case("content-length") {
                content_len = v.trim().parse::<usize>().unwrap_or(0);
            } else if k.trim().eq_ignore_ascii_case("host") {
                host_hdr = v.trim().to_string();
            }
        }
    }
    let mut body = vec![0u8; content_len];
    if content_len > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }

    let mut st = state.lock().expect("lock");
    // Unauthenticated pre-signed storage download target used when `redirect_downloads == true`.
    if method == "GET"
        && let Some(id_str) = path.strip_prefix("/storage/blob/")
    {
        let file_id = id_str.parse::<u64>().unwrap_or(0);
        if let Some(node) = st.nodes.get(&file_id).filter(|n| !n.trashed && !n.is_dir) {
            write_http(stream, 200, &[("Content-Type", "application/octet-stream")], &node.bytes);
        } else {
            write_http(stream, 404, &[], b"not found");
        }
        return;
    }

    if auth != format!("Bearer {}", st.expected_token) {
        let err = serde_json::json!({ "result": "error", "error": { "code": "not_authorized", "description": "invalid bearer token" } });
        write_http(stream, 401, &[("Content-Type", "application/json")], err.to_string().as_bytes());
        return;
    }
    if st.rate_limit_next > 0 {
        st.rate_limit_next -= 1;
        let err = serde_json::json!({ "result": "error", "error": { "code": "rate_limit", "description": "60 requests per minute" } });
        write_http(stream, 429, &[("Content-Type", "application/json"), ("Retry-After", "1")], err.to_string().as_bytes());
        return;
    }

    if method == "GET" && path == "/2/drive/init" {
        match st.discovery_mode {
            DiscoveryMode::DriveInit => {
                let payload = serde_json::json!({ "result": "success", "data": { "drives": [{ "id": st.drive_id, "name": "Free kDrive" }] } });
                write_http(stream, 200, &[("Content-Type", "application/json")], payload.to_string().as_bytes());
            }
            DiscoveryMode::OpenApiAccountProducts | DiscoveryMode::DriveScopeOnlyNoAccount => {
                let err = serde_json::json!({ "result": "error", "error": { "code": "object_not_found", "description": "route not found" } });
                write_http(stream, 404, &[("Content-Type", "application/json")], err.to_string().as_bytes());
            }
        }
        return;
    }
    if method == "GET" && (path == "/1/my_ksuite/current" || path == "/1/products" || path == "/1/accounts") {
        match st.discovery_mode {
            DiscoveryMode::OpenApiAccountProducts if path == "/1/products" => {
                let payload = serde_json::json!({ "result": "success", "data": [{ "id": 91, "account_id": 555, "service_name": "drive" }] });
                write_http(stream, 200, &[("Content-Type", "application/json")], payload.to_string().as_bytes());
            }
            DiscoveryMode::DriveScopeOnlyNoAccount => {
                let err = serde_json::json!({ "result": "error", "error": { "code": "forbidden_error", "description": "missing account scope" } });
                write_http(stream, 403, &[("Content-Type", "application/json")], err.to_string().as_bytes());
            }
            _ => {
                let err = serde_json::json!({ "result": "error", "error": { "code": "object_not_found", "description": "not applicable" } });
                write_http(stream, 404, &[("Content-Type", "application/json")], err.to_string().as_bytes());
            }
        }
        return;
    }
    if method == "GET" && path == "/2/drive" {
        if query.get("account_id").map(String::as_str) == Some("555") {
            let payload = serde_json::json!({ "result": "success", "data": [{ "id": st.drive_id, "name": "Free kDrive" }] });
            write_http(stream, 200, &[("Content-Type", "application/json")], payload.to_string().as_bytes());
        } else {
            let err = serde_json::json!({ "result": "error", "error": { "code": "validation_failed", "description": "The account id field is required." } });
            write_http(stream, 422, &[("Content-Type", "application/json")], err.to_string().as_bytes());
        }
        return;
    }

    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    // POST /3/drive/{drive_id}/files/{parent_id}/directory
    if method == "POST" && segs.len() == 6 && segs[0] == "3" && segs[1] == "drive" && segs[3] == "files" && segs[5] == "directory" {
        let parent_id = segs[4].parse::<u64>().unwrap_or(0);
        // Forbid creating user folders directly inside virtual root 1 (real kDrive constraint!).
        if parent_id == 1 {
            let err = serde_json::json!({ "result": "error", "error": { "description": "cannot create directory in virtual root 1" } });
            write_http(stream, 400, &[("Content-Type", "application/json")], err.to_string().as_bytes());
            return;
        }
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let name = json.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let id = st.alloc_id();
        let node = ServerNode {
            id,
            parent_id,
            name,
            is_dir: true,
            bytes: Vec::new(),
            last_modified_at: 1_700_000_200,
            revised_at: 1_700_000_200,
            omit_hash: false,
            visibility: None,
            trashed: false,
        };
        st.nodes.insert(id, node.clone());
        let resp = serde_json::json!({ "result": "success", "data": node_json(&node) });
        write_http(stream, 200, &[("Content-Type", "application/json")], resp.to_string().as_bytes());
        return;
    }

    // GET /3/drive/{drive_id}/files/{dir_id}/files (with cursor pagination)
    if method == "GET" && segs.len() == 6 && segs[0] == "3" && segs[1] == "drive" && segs[3] == "files" && segs[5] == "files" {
        let dir_id = segs[4].parse::<u64>().unwrap_or(0);
        let all: Vec<serde_json::Value> = st.nodes.values().filter(|n| !n.trashed && n.parent_id == dir_id).map(node_json).collect();
        let offset = query.get("cursor").and_then(|c| c.parse::<usize>().ok()).unwrap_or(0);
        let end = (offset + st.page_size).min(all.len());
        let slice = &all[offset.min(all.len())..end];
        let has_more = end < all.len();
        let next_cursor = if has_more { serde_json::Value::String(end.to_string()) } else { serde_json::Value::Null };
        let resp = serde_json::json!({ "result": "success", "data": slice, "has_more": has_more, "cursor": next_cursor });
        write_http(stream, 200, &[("Content-Type", "application/json")], resp.to_string().as_bytes());
        return;
    }

    // POST /3/drive/{drive_id}/upload
    if method == "POST" && segs.len() == 4 && segs[0] == "3" && segs[1] == "drive" && segs[3] == "upload" {
        let parent_id = query.get("directory_id").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let name = query.get("file_name").cloned().unwrap_or_default();
        if name == "fail-upload.md" {
            let err = serde_json::json!({ "result": "error", "error": { "description": "simulated mid-pass upload failure" } });
            write_http(stream, 500, &[("Content-Type", "application/json")], err.to_string().as_bytes());
            return;
        }
        let mtime = query.get("last_modified_at").and_then(|v| v.parse::<u64>().ok()).unwrap_or(1_700_000_500);
        let saved = st.put_file(parent_id, &name, &body, mtime);
        let resp = serde_json::json!({ "result": "success", "data": node_json(&saved) });
        write_http(stream, 200, &[("Content-Type", "application/json")], resp.to_string().as_bytes());
        return;
    }

    // GET /2/drive/{drive_id}/files/{file_id}/download
    if method == "GET" && segs.len() == 6 && segs[0] == "2" && segs[1] == "drive" && segs[3] == "files" && segs[5] == "download" {
        let file_id = segs[4].parse::<u64>().unwrap_or(0);
        if let Some(node) = st.nodes.get(&file_id).filter(|n| !n.trashed && !n.is_dir) {
            if st.redirect_downloads && !host_hdr.is_empty() {
                let loc = format!("http://{host_hdr}/storage/blob/{file_id}");
                write_http(stream, 302, &[("Location", &loc)], b"");
            } else {
                write_http(stream, 200, &[("Content-Type", "application/octet-stream")], &node.bytes);
            }
        } else {
            write_http(stream, 404, &[], b"not found");
        }
        return;
    }

    // DELETE /2/drive/{drive_id}/files/{file_id}
    if method == "DELETE" && segs.len() == 5 && segs[0] == "2" && segs[1] == "drive" && segs[3] == "files" {
        let file_id = segs[4].parse::<u64>().unwrap_or(0);
        if let Some(node) = st.nodes.get_mut(&file_id) {
            node.trashed = true;
        }
        let resp = serde_json::json!({ "result": "success", "data": true });
        write_http(stream, 200, &[("Content-Type", "application/json")], resp.to_string().as_bytes());
        return;
    }

    write_http(stream, 404, &[], b"unknown route");
}

fn params(pairs: &[(&str, &str)]) -> Params {
    pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
}

fn json_of(out: Output) -> serde_json::Value {
    match out {
        Output::Json(v) => v,
        other => panic!("expected Output::Json, got {other:?}"),
    }
}

#[test]
fn exclusion_rules_reject_dot_dirs_temp_files_inbox_welcome_and_traversal() {
    assert!(is_excluded_rel_path("welcome-to-nocturne/sec/page/index.md"));
    assert!(is_excluded_rel_path("Inbox/shared.md"));
    assert!(is_excluded_rel_path("nb/.nocturne/versions/123.md"));
    assert!(is_excluded_rel_path("nb/sec/page/index.tmp-write"));
    assert!(is_excluded_rel_path("nb/sec/page/ink.tmp"));
    assert!(is_excluded_rel_path("../escape.md"));
    assert!(is_excluded_rel_path("nb/../../escape.md"));
    assert!(is_excluded_rel_path("nocturne-diagnostics.log"));
    assert!(!is_excluded_rel_path("physics-notes/_node.toml"));
    assert!(!is_excluded_rel_path("physics-notes/week-1/lecture-1/index.md"));
    assert!(!is_excluded_rel_path("physics-notes/week-1/lecture-1/assets/ink.svg"));
}

#[test]
fn kdrive_sync_roundtrip_private_space_pagination_exclusions_and_429_retry() {
    let srv = LoopbackServer::start("secret-drive-token", 424242);
    let ipad_tmp = TempDir::new("sync-ipad");
    let pc_tmp = TempDir::new("sync-pc");

    let ipad_settings = Settings::at(ipad_tmp.path().join("cfg/settings.toml"));
    let mut ipad = Session::new(&ipad_tmp.path().join("notebooks"), ipad_settings).expect("ipad session");

    // Configure sync with auto drive discovery, default "The Oracle/Nocturne", and Markdown default.
    let status = json_of(
        ipad.run(
            "sync.configure",
            &params(&[
                ("endpoint", &srv.endpoint),
                ("drive_id", "auto"),
                ("remote_path", "The Oracle/Nocturne"),
                ("token", "secret-drive-token"),
                ("markdown_default", "true"),
            ]),
        )
        .expect("configure"),
    );
    assert_eq!(status["has_token"], true);
    assert_eq!(status["markdown_default"], true);
    // Token must NEVER appear in settings.toml!
    let raw_settings = std::fs::read_to_string(ipad.settings.path()).unwrap_or_default();
    assert!(!raw_settings.contains("secret-drive-token"));

    // Create a Markdown notebook + page + ink.svg, plus excluded files (.nocturne, .tmp-write, welcome-to-nocturne).
    let nb_out = ipad.run("notebook.create", &params(&[("name", "Quantum Notes")])).expect("create nb");
    let Output::Path(nb_dir) = nb_out else { panic!("expected path") };
    let page_dir = ipad.page_to_show("quantum-notes").expect("first page");
    assert!(page_dir.join("index.md").is_file(), "default_format=cmgfm creates index.md");
    std::fs::write(page_dir.join("assets/ink.svg"), b"<svg><path d='M0 0 L10 10'/></svg>").expect("write ink");
    std::fs::create_dir_all(page_dir.join(".nocturne/versions")).expect("versions dir");
    std::fs::write(page_dir.join(".nocturne/versions/snap.md"), b"old").expect("version snap");
    std::fs::write(page_dir.join("index.tmp-write"), b"partial").expect("tmp write");
    std::fs::create_dir_all(ipad.root().join("welcome-to-nocturne")).expect("welcome dir");
    std::fs::write(ipad.root().join("welcome-to-nocturne/_node.toml"), b"kind = \"notebook\"\n").expect("welcome node");

    // Trigger one HTTP 429 to verify automatic Retry-After backoff recovery.
    srv.state.lock().expect("lock").rate_limit_next = 1;

    let r1 = json_of(ipad.run("sync.now", &params(&[])).expect("first sync"));
    assert_eq!(r1["drive_id"], 424242);
    assert_eq!(r1["uploaded"].as_array().expect("arr").len(), 4, "nb _node.toml + sec _node.toml + index.md + assets/ink.svg");

    // Verify files landed inside Private/The Oracle/Nocturne on kDrive (NOT virtual root 1), and exclusions were skipped!
    {
        let st = srv.state.lock().expect("lock");
        assert!(st.find_under_private("The Oracle/Nocturne/quantum-notes/_node.toml").is_some());
        assert!(st.find_under_private("The Oracle/Nocturne/quantum-notes/new-section-1/untitled-page/index.md").is_some());
        assert!(st.find_under_private("The Oracle/Nocturne/quantum-notes/new-section-1/untitled-page/assets/ink.svg").is_some());
        assert!(st.find_under_private("The Oracle/Nocturne/welcome-to-nocturne/_node.toml").is_none());
    }

    // Immediate second sync is a pure no-op (0 uploads, 0 downloads).
    let r2 = json_of(ipad.run("sync.now", &params(&[])).expect("second sync"));
    assert_eq!(r2["uploaded"].as_array().expect("arr").len(), 0);
    assert_eq!(r2["downloaded"].as_array().expect("arr").len(), 0);
    assert_eq!(r2["unchanged"], 4);

    // Sync down onto a second client (PC / Kandinsky workspace) and verify index.md + ink.svg arrive intact!
    let pc_settings = Settings::at(pc_tmp.path().join("cfg/settings.toml"));
    let mut pc = Session::new(&pc_tmp.path().join("notebooks"), pc_settings).expect("pc session");
    pc.run(
        "sync.configure",
        &params(&[("endpoint", &srv.endpoint), ("drive_id", "424242"), ("remote_path", "The Oracle/Nocturne"), ("token", "secret-drive-token")]),
    )
    .expect("pc configure");
    let pc_sync = json_of(pc.run("sync.now", &params(&[])).expect("pc sync"));
    assert_eq!(pc_sync["downloaded"].as_array().expect("arr").len(), 4);
    assert!(pc.root().join("quantum-notes/new-section-1/untitled-page/index.md").is_file());
    assert!(pc.root().join("quantum-notes/new-section-1/untitled-page/assets/ink.svg").is_file());

    // Verify node.format command converts an HTML notebook's _node.toml to cmgfm.
    let _ = nb_dir;
    ipad.run("node.format", &params(&[("path", "quantum-notes"), ("format", "html")])).expect("set html");
    ipad.run("node.format", &params(&[("path", "quantum-notes"), ("format", "cmgfm")])).expect("set cmgfm");
}

#[test]
fn concurrent_edit_and_true_3way_conflict_preserve_both_copies_without_data_loss() {
    let srv = LoopbackServer::start("tok-conflict", 777);
    let a_tmp = TempDir::new("sync-conflict-a");
    let b_tmp = TempDir::new("sync-conflict-b");

    let mut a = Session::new(&a_tmp.path().join("nb"), Settings::at(a_tmp.path().join("cfg/s.toml"))).expect("a");
    let mut b = Session::new(&b_tmp.path().join("nb"), Settings::at(b_tmp.path().join("cfg/s.toml"))).expect("b");
    for s in [&mut a, &mut b] {
        s.run(
            "sync.configure",
            &params(&[("endpoint", &srv.endpoint), ("drive_id", "777"), ("remote_path", "The Oracle/Nocturne"), ("token", "tok-conflict")]),
        )
        .expect("cfg");
    }

    // Create initial file on A, sync A -> kDrive -> B.
    std::fs::create_dir_all(a.root().join("notes/sec/p1")).expect("dir");
    std::fs::write(a.root().join("notes/sec/p1/index.md"), b"---\ntitle: P1\n---\nv1\n").expect("v1");
    a.run("sync.now", &params(&[])).expect("a sync 1");
    b.run("sync.now", &params(&[])).expect("b sync 1");

    // First-sync / identical content check on a third client C that ALREADY has the exact same bytes locally:
    let c_tmp = TempDir::new("sync-identical-c");
    let mut c = Session::new(&c_tmp.path().join("nb"), Settings::at(c_tmp.path().join("cfg/s.toml"))).expect("c");
    std::fs::create_dir_all(c.root().join("notes/sec/p1")).expect("c dir");
    std::fs::write(c.root().join("notes/sec/p1/index.md"), b"---\ntitle: P1\n---\nv1\n").expect("c v1");
    c.run(
        "sync.configure",
        &params(&[("endpoint", &srv.endpoint), ("drive_id", "777"), ("remote_path", "The Oracle/Nocturne"), ("token", "tok-conflict")]),
    )
    .expect("c cfg");
    let rc = json_of(c.run("sync.now", &params(&[])).expect("c sync"));
    assert_eq!(rc["conflicts"].as_array().expect("arr").len(), 0, "identical pre-existing files must not conflict");

    // True 3-way conflict: A writes "from iPad", B writes "from PC", B syncs first, then A syncs.
    std::fs::write(b.root().join("notes/sec/p1/index.md"), b"---\ntitle: P1\n---\nfrom PC\n").expect("pc edit");
    b.run("sync.now", &params(&[])).expect("b sync 2");

    std::fs::write(a.root().join("notes/sec/p1/index.md"), b"---\ntitle: P1\n---\nfrom iPad\n").expect("ipad edit");
    let ra = json_of(a.run("sync.now", &params(&[])).expect("a conflict sync"));
    let conflicts = ra["conflicts"].as_array().expect("conflicts");
    assert_eq!(conflicts.len(), 1);
    let conflict_rel = conflicts[0].as_str().expect("str");
    assert_eq!(std::fs::read_to_string(a.root().join("notes/sec/p1/index.md")).expect("canon"), "---\ntitle: P1\n---\nfrom iPad\n");
    assert_eq!(std::fs::read_to_string(a.root().join(conflict_rel)).expect("conflict copy"), "---\ntitle: P1\n---\nfrom PC\n");

    // Simulate typing DURING an in-flight background sync on B:
    // Stage sync on B (sees remote updated to "from iPad"), then mutate B's local file BEFORE apply_staged_sync!
    let cfg_b = SyncConfig::load(&b.settings);
    let staged = stage_sync(b.root(), &b.settings, &cfg_b, b.sync_http().as_ref(), b.sync_secrets().as_ref(), None).expect("stage on b");
    std::fs::write(b.root().join("notes/sec/p1/index.md"), b"---\ntitle: P1\n---\ntyped mid-sync!\n").expect("mid-sync edit");
    let applied = apply_staged_sync(staged, &b.settings).expect("apply on b");
    assert_eq!(applied.conflicts.len(), 1, "mid-sync local edit must not be clobbered");
    assert_eq!(
        std::fs::read_to_string(b.root().join("notes/sec/p1/index.md")).expect("kept mid-sync edit"),
        "---\ntitle: P1\n---\ntyped mid-sync!\n"
    );

    // Subsequent sync pass on B MUST upload `typed mid-sync!` cleanly with ZERO duplicate conflict copies!
    let rb2 = json_of(b.run("sync.now", &params(&[])).expect("b post-conflict sync"));
    assert_eq!(rb2["conflicts"].as_array().expect("arr").len(), 0, "must not duplicate conflict copy on next sync pass");
    assert!(
        rb2["uploaded"].as_array().expect("arr").iter().any(|v| v.as_str() == Some("notes/sec/p1/index.md")),
        "must upload canonical index.md cleanly: {rb2}"
    );
}

#[test]
fn invalid_token_and_local_root_change_and_delete_vs_modify_behave_safely() {
    let srv = LoopbackServer::start("good-token", 999);
    let tmp = TempDir::new("sync-safety");
    let mut s = Session::new(&tmp.path().join("root1"), Settings::at(tmp.path().join("cfg/s.toml"))).expect("s");

    // Wrong token fails with clear Auth error.
    s.run("sync.configure", &params(&[("endpoint", &srv.endpoint), ("token", "wrong-token")])).expect("cfg bad");
    let bad = s.run("sync.now", &params(&[])).expect_err("must reject 401");
    assert!(bad.to_string().contains("rejected the token"), "got: {bad}");

    // Valid token + propagate_deletes=true: create 2 files and sync.
    s.run("sync.configure", &params(&[("endpoint", &srv.endpoint), ("drive_id", "999"), ("token", "good-token"), ("propagate_deletes", "true")]))
        .expect("cfg good");
    std::fs::create_dir_all(s.root().join("nb")).expect("nb");
    std::fs::write(s.root().join("nb/a.md"), b"alpha").expect("a");
    std::fs::write(s.root().join("nb/b.md"), b"beta").expect("b");
    std::fs::write(s.root().join("nb/c.md"), b"gamma").expect("c");
    s.run("sync.now", &params(&[])).expect("sync 1");

    // Delete vs modify: delete a.md locally WHILE remote a.md is modified -> remote modify wins (re-downloaded, NOT deleted)!
    std::fs::remove_file(s.root().join("nb/a.md")).expect("rm local a");
    {
        let mut st = srv.state.lock().expect("lock");
        let nb_id = st.ensure_dir_chain_under_private("The Oracle/Nocturne/nb");
        st.put_file(nb_id, "a.md", b"alpha-remote-updated", 1_700_999_999);
    }
    let r_mod = json_of(s.run("sync.now", &params(&[])).expect("sync delete-vs-modify"));
    assert_eq!(r_mod["trashed_remote"].as_array().expect("arr").len(), 0);
    assert_eq!(std::fs::read_to_string(s.root().join("nb/a.md")).expect("restored modified a"), "alpha-remote-updated");

    // Switching to a brand-new empty local root (`root2`) sharing the same settings.toml must NOT treat root1's files as deleted!
    let mut s2 = Session::new(&tmp.path().join("root2"), Settings::at(tmp.path().join("cfg/s.toml"))).expect("s2");
    let r_root2 = json_of(s2.run("sync.now", &params(&[])).expect("sync new root2"));
    assert_eq!(r_root2["trashed_remote"].as_array().expect("arr").len(), 0, "changing local_root must never trash remote");
    assert_eq!(r_root2["downloaded"].as_array().expect("arr").len(), 3, "new local_root downloads remote files");

    // H1 regression: if `a.md` conflicts on both sides AND a later file (`fail-upload.md`) aborts the pass,
    // the remote conflict copy MUST already be persisted locally before the upload ran!
    std::fs::write(s.root().join("nb/a.md"), b"alpha-local-h1").expect("write local h1");
    std::fs::write(s.root().join("nb/fail-upload.md"), b"trigger-500").expect("write fail-upload");
    {
        let mut st = srv.state.lock().expect("lock");
        let nb_id = st.ensure_dir_chain_under_private("The Oracle/Nocturne/nb");
        st.put_file(nb_id, "a.md", b"alpha-remote-h1", 1_701_111_111);
    }
    assert!(s.run("sync.now", &params(&[])).is_err(), "fail-upload.md must abort the sync pass");
    let preserved_conflict = std::fs::read_dir(s.root().join("nb"))
        .expect("read nb")
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with("a.conflict-"))
        .expect("remote conflict copy must survive mid-pass failure");
    assert_eq!(std::fs::read_to_string(preserved_conflict.path()).expect("read conflict"), "alpha-remote-h1");
    let stage_parent = s.root().join(".nocturne-sync-stage");
    let leftover_runs = std::fs::read_dir(&stage_parent)
        .map(|rd| rd.filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().starts_with("run-")).count())
        .unwrap_or(0);
    assert_eq!(leftover_runs, 0, "failed stage_sync must clean up its run-* staging directory");

    assert!(s.run("sync.configure", &params(&[("endpoint", "http://localhost.evil.example"), ("token", "good-token")])).is_ok());
    assert!(s.run("sync.now", &params(&[])).is_err(), "http://localhost.evil.example must be rejected");
}

#[test]
fn openapi_account_discovery_url_drive_id_302_download_revised_at_and_child_format_conversion() {
    let srv = LoopbackServer::start("tok-openapi", 654321);
    {
        let mut st = srv.state.lock().expect("lock");
        st.discovery_mode = DiscoveryMode::OpenApiAccountProducts;
        st.redirect_downloads = true;
    }

    let tmp = TempDir::new("sync-openapi");
    let mut s = Session::new(&tmp.path().join("nb"), Settings::at(tmp.path().join("cfg/s.toml"))).expect("s");
    s.run("sync.configure", &params(&[("endpoint", &srv.endpoint), ("drive_id", "auto"), ("token", "tok-openapi")])).expect("cfg");

    // 1. `/2/drive/init` is 404 and `/2/drive` without `account_id` is 422 -> discovers `account_id=555` via `/1/products` -> `/2/drive?account_id=555`!
    std::fs::create_dir_all(s.root().join("vault")).expect("dir");
    std::fs::write(s.root().join("vault/todo.md"), b"todo: 0").expect("write");
    let r1 = json_of(s.run("sync.now", &params(&[])).expect("sync via account_id discovery"));
    assert_eq!(r1["drive_id"], 654321);

    // 2. When `hash` is null and `last_modified_at` + byte size stay unchanged, advancing `revised_at` MUST trigger download (via 302 redirect)!
    {
        let mut st = srv.state.lock().expect("lock");
        let dir_id = st.ensure_dir_chain_under_private("The Oracle/Nocturne/vault");
        let file_id = st.nodes.values().find(|n| !n.trashed && n.parent_id == dir_id && n.name == "todo.md").expect("todo.md").id;
        let node = st.nodes.get_mut(&file_id).expect("mut");
        node.omit_hash = true;
        node.bytes = b"todo: 1".to_vec(); // exact same 7-byte size!
        node.revised_at += 10; // only revised_at changes!
    }
    let r2 = json_of(s.run("sync.now", &params(&[])).expect("sync null-hash revised_at + 302 redirect"));
    assert_eq!(r2["downloaded"].as_array().expect("arr").len(), 1);
    assert_eq!(std::fs::read_to_string(s.root().join("vault/todo.md")).expect("updated via 302"), "todo: 1");

    // 2b. When kDrive later populates `hash` asynchronously (changing `r.fingerprint()` from `m:...` to `h:...` while
    // remote bytes still match baseline `todo: 1`) AND the user edits locally (`todo: 2`), sync must upload cleanly with ZERO false conflicts!
    {
        let mut st = srv.state.lock().expect("lock");
        let dir_id = st.ensure_dir_chain_under_private("The Oracle/Nocturne/vault");
        let file_id = st.nodes.values().find(|n| !n.trashed && n.parent_id == dir_id && n.name == "todo.md").expect("todo.md").id;
        st.nodes.get_mut(&file_id).expect("mut").omit_hash = false;
    }
    std::fs::write(s.root().join("vault/todo.md"), b"todo: 2").expect("local edit after async hash");
    let r2b = json_of(s.run("sync.now", &params(&[])).expect("sync async hash + local edit"));
    assert_eq!(r2b["conflicts"].as_array().expect("arr").len(), 0, "async hash population must not trigger false conflict");
    assert_eq!(r2b["uploaded"].as_array().expect("arr").len(), 1);

    // 3. Strict `drive`-only token (`/1/*` -> 403, `/2/drive` -> 422) yields actionable `NotConfigured` error, and accepts pasted kDrive URL!
    srv.state.lock().expect("lock").discovery_mode = DiscoveryMode::DriveScopeOnlyNoAccount;
    s.run("sync.configure", &params(&[("drive_id", "auto")])).expect("reset auto");
    let err = s.run("sync.now", &params(&[])).expect_err("must ask for Drive ID");
    assert!(err.to_string().contains("Drive ID"), "expected actionable Drive ID prompt, got: {err}");

    let st_url = json_of(
        s.run("sync.configure", &params(&[("drive_id", "https://ksuite.infomaniak.com/all/kdrive/app/drive/654321/files/1")]))
            .expect("parse kDrive web URL"),
    );
    assert_eq!(st_url["drive_id"], 654321);

    // 4. `convert_notebooks_to_markdown=true` clears child section `format = "html"` overrides so new pages use `index.md`!
    let nb = match s.run("notebook.create", &params(&[("name", "Legacy HTML"), ("format", "html")])).expect("html nb") {
        Output::Path(p) => p,
        other => panic!("expected Path, got {other:?}"),
    };
    let sec = match s
        .run("node.create", &params(&[("parent", &nb.to_string_lossy()), ("kind", "section"), ("title", "Explicit HTML Sec"), ("format", "html")]))
        .expect("html sec")
    {
        Output::Path(p) => p,
        other => panic!("expected Path, got {other:?}"),
    };
    s.run("sync.configure", &params(&[("convert_notebooks_to_markdown", "true")])).expect("convert md");
    let new_page =
        match s.run("node.create", &params(&[("parent", &sec.to_string_lossy()), ("kind", "page"), ("title", "Fresh Note")])).expect("page") {
            Output::Path(p) => p,
            other => panic!("expected Path, got {other:?}"),
        };
    assert!(new_page.join("index.md").is_file(), "child section must inherit converted notebook's cmgfm format");

    // 5. Deleting (`node.delete` -> recycle bin) a synced page with default `propagate_deletes = false` trashes it on kDrive
    // instead of resurrecting the deleted page on the next sync!
    s.run("sync.now", &params(&[])).expect("upload new_page");
    s.run("node.delete", &params(&[("path", &new_page.to_string_lossy())])).expect("recycle new_page");
    assert!(!new_page.exists());
    let r_del = json_of(s.run("sync.now", &params(&[])).expect("sync after recycle"));
    assert!(!new_page.exists(), "recycled page must not be re-downloaded when propagate_deletes is false");
    assert!(!r_del["trashed_remote"].as_array().expect("arr").is_empty(), "recycled page files must be trashed on kDrive");
}
