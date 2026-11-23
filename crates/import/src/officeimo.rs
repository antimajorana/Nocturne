//! Optional OfficeIMO bridge discovery and invocation.
//!
//! The bridge is deliberately an external process. This keeps the Nocturne core Rust-only,
//! lets releases carry a platform-specific OfficeIMO build, and gives us a stable replacement
//! point if the importer implementation changes later.

use std::ffi::OsString;
use std::fs;
use std::io::Read;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

/// The page serialization requested from the bridge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Html,
    Cmgfm,
}

/// Progress reported while a folder import is being materialized.
#[derive(Clone, Debug)]
pub enum Progress {
    Found { total: usize },
    Section { index: usize, total: usize, label: String },
}

impl Format {
    /// Parses the public page format names used by the CLI and bridge contract.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "html" => Ok(Self::Html),
            "cmgfm" => Ok(Self::Cmgfm),
            other => Err(format!("unknown OfficeIMO format '{other}'; use html or cmgfm")),
        }
    }
}

impl Format {
    fn argument(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Cmgfm => "cmgfm",
        }
    }
}

/// Locates an explicitly installed or bundled OfficeIMO bridge.
pub fn locate() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("NOCTURNE_OFFICEIMO_BRIDGE") {
        let path = PathBuf::from(path);
        return executable_or_error(path, "NOCTURNE_OFFICEIMO_BRIDGE");
    }

    let relative = Path::new("importers").join("officeimo").join(target_name()).join(executable_name());
    let mut candidates = Vec::new();
    if let Some(root) = std::env::var_os("NOCTURNE_DATA_DIR").map(PathBuf::from).or_else(data_dir) {
        candidates.push(root.join(&relative));
    }
    // Windows MSI installs the bridge beside the application under Program Files, while
    // portable/dev installs may use the data directory above. Check both layouts.
    if let Ok(executable) = std::env::current_exe()
        && let Some(app_dir) = executable.parent()
    {
        candidates.push(app_dir.join(&relative));
    }
    if let Some(path) = candidates.iter().find(|path| path.is_file()) {
        return Ok(path.clone());
    }
    let searched = candidates.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join("; ");
    Err(format!("the installed OfficeIMO bridge was not found; searched: {searched}"))
}

/// Invokes the bridge using the stable command-line contract.
pub fn run(source: &Path, output: &Path, format: Format) -> Result<String, String> {
    let cancelled = AtomicBool::new(false);
    run_with_progress(source, output, format, &cancelled, &mut |_| {})
}

/// Runs the bridge while reporting folder progress and allowing the caller to cancel.
pub fn run_with_progress(
    source: &Path,
    output: &Path,
    format: Format,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(Progress),
) -> Result<String, String> {
    if !source.exists() {
        return Err(format!("the OfficeIMO source {} does not exist", source.display()));
    }
    if source.is_dir() {
        return run_folder(source, output, format, cancelled, progress);
    }
    // A OneNote package (issue #17): unpacked into a folder named after it, which is then
    // imported as a notebook folder. The unpacked copy is removed when the import ends.
    if crate::package::is_package(source) {
        let scratch = crate::package::Scratch::new()?;
        let name = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let folder = scratch.path.join(crate::package::safe_name(&safe_name(&name)).unwrap_or_else(|| PathBuf::from("OneNote package")));
        crate::package::unpack(source, &folder)?;
        return run_folder(&folder, output, format, cancelled, progress);
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("OfficeIMO import cancelled".to_string());
    }
    run_one(source, output, format, cancelled)
}

fn run_one(source: &Path, output: &Path, format: Format, cancelled: &AtomicBool) -> Result<String, String> {
    let bridge = locate()?;
    run_bridge(&bridge, source, output, format, cancelled)
}

fn drain_pipe(pipe: Option<impl Read + Send + 'static>) -> Option<std::thread::JoinHandle<Result<Vec<u8>, std::io::Error>>> {
    pipe.map(|mut reader| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            reader.read_to_end(&mut buf)?;
            Ok(buf)
        })
    })
}

fn finish_pipe(handle: Option<std::thread::JoinHandle<Result<Vec<u8>, std::io::Error>>>) -> Result<Vec<u8>, String> {
    match handle {
        Some(handle) => match handle.join() {
            Ok(Ok(bytes)) => Ok(bytes),
            Ok(Err(e)) => Err(format!("cannot read OfficeIMO bridge output: {e}")),
            Err(_) => Err("OfficeIMO bridge reader thread ended unexpectedly".to_string()),
        },
        None => Ok(Vec::new()),
    }
}

fn run_bridge(bridge: &Path, source: &Path, output: &Path, format: Format, cancelled: &AtomicBool) -> Result<String, String> {
    let mut command = Command::new(bridge);
    command
        .arg("--source")
        .arg(source)
        .arg("--out")
        .arg(output)
        .arg("--format")
        .arg(format.argument())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    suppress_console(&mut command);
    let mut child = command.spawn().map_err(|e| format!("cannot start OfficeIMO bridge {}: {e}", bridge.display()))?;
    let stdout_reader = drain_pipe(child.stdout.take());
    let stderr_reader = drain_pipe(child.stderr.take());
    let status = loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = finish_pipe(stdout_reader);
            let _ = finish_pipe(stderr_reader);
            return Err("OfficeIMO import cancelled".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(40)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = finish_pipe(stdout_reader);
                let _ = finish_pipe(stderr_reader);
                return Err(format!("cannot wait for OfficeIMO bridge: {e}"));
            }
        }
    };
    let stdout_bytes = finish_pipe(stdout_reader)?;
    let stderr_bytes = finish_pipe(stderr_reader)?;
    let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr_bytes);
        return Err(format!("OfficeIMO bridge failed with {}: {}", status, stderr.trim()));
    }
    Ok(stdout)
}

fn run_folder(
    source: &Path,
    notebooks: &Path,
    format: Format,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(Progress),
) -> Result<String, String> {
    let notebook_name = safe_name(source.file_name().and_then(|v| v.to_str()).unwrap_or("Imported notebook"));
    let (notebook_name, notebook) = unique_notebook_path(notebooks, &notebook_name);
    fs::create_dir_all(&notebook).map_err(|e| format!("cannot create {}: {e}", notebook.display()))?;
    write_node(&notebook, "notebook", &notebook_name, format)?;

    let mut files = Vec::new();
    collect_sections(source, source, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err(format!("no .one section files were found under {}", source.display()));
    }
    progress(Progress::Found { total: files.len() });

    let staging = notebook.join(format!(".officeimo-staging-{}", std::process::id()));
    let result = (|| {
        let mut failures = Vec::new();
        for (index, file) in files.iter().enumerate() {
            if cancelled.load(Ordering::Relaxed) {
                return Err("OfficeIMO import cancelled".to_string());
            }
            let label = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "section".to_string());
            progress(Progress::Section { index: index + 1, total: files.len(), label });
            let relative_parent = file.parent().and_then(|p| p.strip_prefix(source).ok()).unwrap_or(Path::new(""));
            let mut destination = notebook.clone();
            for component in relative_parent.components() {
                let group_name = safe_name(&component.as_os_str().to_string_lossy());
                destination.push(&group_name);
                if !destination.join("_node.toml").is_file() {
                    fs::create_dir_all(&destination).map_err(|e| format!("cannot create {}: {e}", destination.display()))?;
                    write_node(&destination, "group", &group_name, format)?;
                }
            }
            let section_name = section_name(file);
            destination.push(section_name.clone());
            let stage = staging.join(format!("{index}"));
            fs::create_dir_all(&stage).map_err(|e| format!("cannot create {}: {e}", stage.display()))?;
            if let Err(error) = run_one(file, &stage, format, cancelled) {
                failures.push(format!("{}: {}", file.display(), error));
                continue;
            }
            let generated = stage.join("Imported section");
            if !generated.is_dir() {
                failures.push(format!("{}: OfficeIMO produced no section", file.display()));
                continue;
            }
            copy_tree(&generated, &destination)?;
            write_node(&destination, "section", &section_name, format)?;
        }
        normalize_ids(&notebook)?;
        if !failures.is_empty() {
            let report = format!(
                "OfficeIMO imported {} of {} sections into {}\n{}",
                files.len() - failures.len(),
                files.len(),
                notebook.display(),
                failures.join("\n")
            );
            fs::write(notebook.join("import-errors.txt"), &report).map_err(|e| format!("cannot write import error report: {e}"))?;
            if failures.len() == files.len() {
                return Err(report);
            }
            Ok(report)
        } else {
            Ok(format!("OfficeIMO imported {} sections into {}", files.len(), notebook.display()))
        }
    })();
    let _ = fs::remove_dir_all(&staging);
    result
}

#[cfg(windows)]
fn suppress_console(command: &mut Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn suppress_console(_command: &mut Command) {}

fn unique_notebook_path(parent: &Path, name: &str) -> (String, PathBuf) {
    let candidate = parent.join(name);
    if !candidate.exists() {
        return (name.to_string(), candidate);
    }
    for number in 1..=u32::MAX {
        let candidate_name = format!("{name} ({number})");
        let candidate = parent.join(&candidate_name);
        if !candidate.exists() {
            return (candidate_name, candidate);
        }
    }
    let fallback = format!("{name} ({})", u64::from(u32::MAX) + 1);
    (fallback.clone(), parent.join(fallback))
}

fn collect_sections(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read an entry under {}: {e}", dir.display()))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| format!("cannot inspect {}: {e}", path.display()))?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if entry.file_name().to_string_lossy().eq_ignore_ascii_case("OneNote_RecycleBin") {
                continue;
            }
            collect_sections(root, &path, files)?;
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("one")) && path.strip_prefix(root).is_ok() {
            files.push(path);
        }
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|e| format!("cannot create {}: {e}", destination.display()))?;
    for entry in fs::read_dir(source).map_err(|e| format!("cannot read {}: {e}", source.display()))? {
        let entry = entry.map_err(|e| format!("cannot read an entry under {}: {e}", source.display()))?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if entry.file_type().map_err(|e| format!("cannot inspect {}: {e}", from.display()))?.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| format!("cannot copy {} to {}: {e}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

fn write_node(path: &Path, kind: &str, title: &str, format: Format) -> Result<(), String> {
    let title = title.replace(['\\', '"'], "_");
    let content = if kind == "notebook" {
        format!("kind = \"notebook\"\ntitle = \"{title}\"\nid = \"00000000000000000001\"\nformat = \"{}\"\n", format.argument())
    } else {
        format!("kind = \"{kind}\"\ntitle = \"{title}\"\nid = \"00000000000000000002\"\n")
    };
    fs::write(path.join("_node.toml"), content).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn normalize_ids(root: &Path) -> Result<(), String> {
    let mut nodes = Vec::new();
    collect_nodes(root, &mut nodes)?;
    nodes.sort();
    for (index, node) in nodes.iter().enumerate() {
        let text = fs::read_to_string(node).map_err(|e| format!("cannot read {}: {e}", node.display()))?;
        let updated = text
            .lines()
            .map(|line| if line.starts_with("id = ") { format!("id = \"{:020}\"", index + 1) } else { line.to_string() })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(node, updated).map_err(|e| format!("cannot rewrite {}: {e}", node.display()))?;
    }
    let mut pages = Vec::new();
    collect_pages(root, &mut pages)?;
    pages.sort();
    for (index, page) in pages.iter().enumerate() {
        let text = fs::read_to_string(page).map_err(|e| format!("cannot read {}: {e}", page.display()))?;
        let id = format!("{:020}", nodes.len() + index + 1);
        let updated = normalize_page_id(page, &text, &id);
        fs::write(page, updated).map_err(|e| format!("cannot rewrite {}: {e}", page.display()))?;
    }
    Ok(())
}

fn normalize_page_id(page: &Path, text: &str, id: &str) -> String {
    match page.file_name().and_then(|name| name.to_str()) {
        Some("index.md") => {
            let (front, body) = nocturne_formats::frontmatter::split_front(text);
            if front.is_empty() {
                text.to_string()
            } else {
                let updated_front = nocturne_formats::frontmatter::set_value(&front, "id", id);
                format!("{updated_front}{body}")
            }
        }
        Some("index.html") => {
            let Some(close) = text.to_ascii_lowercase().find("</head>") else {
                return text.to_string();
            };
            let split = close + "</head>".len();
            let head = text.get(..split).unwrap_or("");
            let tail = text.get(split..).unwrap_or("");
            let updated_head = nocturne_formats::html::set_meta(head, "nocturne:id", id);
            format!("{updated_head}{tail}")
        }
        _ => text.to_string(),
    }
}

fn collect_nodes(dir: &Path, nodes: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("cannot read an entry under {}: {e}", dir.display()))?;
        let path = entry.path();
        if entry.file_type().map_err(|e| format!("cannot inspect {}: {e}", path.display()))?.is_dir() {
            collect_nodes(&path, nodes)?;
        } else if path.file_name().is_some_and(|name| name == "_node.toml") {
            nodes.push(path);
        }
    }
    Ok(())
}

fn collect_pages(dir: &Path, pages: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("cannot read an entry under {}: {e}", dir.display()))?;
        let path = entry.path();
        if entry.file_type().map_err(|e| format!("cannot inspect {}: {e}", path.display()))?.is_dir() {
            collect_pages(&path, pages)?;
        } else if path.file_name().is_some_and(|name| name == "index.html" || name == "index.md") {
            pages.push(path);
        }
    }
    Ok(())
}

fn section_name(path: &Path) -> String {
    let mut name = path.file_stem().and_then(|v| v.to_str()).unwrap_or("Untitled section").to_string();
    if let Some((prefix, _)) = name.rsplit_once(" (On ") {
        name = prefix.to_string();
    }
    safe_name(&name)
}

fn safe_name(value: &str) -> String {
    let name: String = value.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let name = name.trim().trim_end_matches('.');
    if name.is_empty() { "Imported notebook".to_string() } else { name.to_string() }
}

fn executable_or_error(path: PathBuf, source: &str) -> Result<PathBuf, String> {
    if path.is_file() { Ok(path) } else { Err(format!("{source} was not found at {}", path.display())) }
}

fn data_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).map(|p| p.join("Nocturne"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local").join("share")))
            .map(|p| p.join("Nocturne"))
    }
}

fn target_name() -> &'static str {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "win-x64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "osx-x64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "osx-arm64"
    } else {
        "unsupported"
    }
}

fn executable_name() -> OsString {
    if cfg!(windows) { OsString::from("officeimo-nocturne.exe") } else { OsString::from("officeimo-nocturne") }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue #17: a package is unpacked and imported as a folder of sections named after it.
    /// No bridge is installed for the test, so the run stops at the first section; what it has
    /// found by then is the package's section.
    #[test]
    fn a_package_is_unpacked_and_its_sections_imported_as_a_folder() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/3p/officeimo/makecab-lzx-notebook.onepkg");
        let out = nocturne_store::testing::TempDir::new("officeimo-onepkg");
        let mut seen = Vec::new();
        let _ = run_with_progress(&source, out.path(), Format::Html, &AtomicBool::new(false), &mut |p| seen.push(format!("{p:?}")));
        assert!(seen.first().is_some_and(|p| p.contains("Found { total: 1 }")), "the package's one section was found: {seen:?}");
        assert!(seen.iter().any(|p| p.contains("Compressed section.one")), "{seen:?}");
        assert!(out.path().join("makecab-lzx-notebook").is_dir(), "the notebook is named after the package");
    }

    #[test]
    fn folder_names_and_backup_dates_become_safe_container_names() {
        assert_eq!(safe_name("2026"), "2026");
        assert_eq!(section_name(Path::new("Work (On 9-29-2026).one")), "Work");
        assert_eq!(safe_name("A:B"), "A_B");
    }

    #[test]
    fn section_discovery_is_recursive_and_skips_recycle_bin() {
        let root = std::env::temp_dir().join(format!("nocturne-officeimo-{}", std::process::id()));
        let nested = root.join("2026").join("MBA");
        let recycle = root.join("2026").join("OneNote_RecycleBin");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&nested).expect("test directory");
        fs::create_dir_all(&recycle).expect("test directory");
        fs::write(nested.join("Work.one"), []).expect("test file");
        fs::write(recycle.join("Deleted.one"), []).expect("test file");
        let mut files = Vec::new();
        collect_sections(&root, &root, &mut files).expect("scan");
        assert_eq!(files, vec![nested.join("Work.one")]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repeated_folder_imports_get_windows_style_names() {
        let root = std::env::temp_dir().join(format!("nocturne-officeimo-names-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("2026")).expect("test directory");
        fs::create_dir_all(root.join("2026 (1)")).expect("test directory");
        assert_eq!(unique_notebook_path(&root, "2026"), ("2026 (2)".to_string(), root.join("2026 (2)")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn target_names_are_release_rids() {
        assert!(matches!(target_name(), "win-x64" | "linux-x64" | "osx-x64" | "osx-arm64" | "unsupported"));
    }

    #[test]
    fn normalize_ids_rewrites_only_metadata_and_preserves_page_bodies() {
        let dir = nocturne_store::testing::TempDir::new("officeimo-normalize-ids");
        let root = dir.path();
        fs::write(root.join("_node.toml"), "kind = \"notebook\"\ntitle = \"N\"\nid = \"old\"\n").expect("node");

        let md_dir = root.join("01-md");
        fs::create_dir_all(&md_dir).expect("md dir");
        let md_body = "\n# Notes\n\n```yaml\nid: keep-me\n  id: indented-keep-me\n```\n\n<meta name=\"nocturne:id\" content=\"literal-body\">\n";
        let md_page = format!("---\ntitle: MD\nid: old-md\n---\n{md_body}");
        fs::write(md_dir.join("index.md"), &md_page).expect("write md");

        let html_dir = root.join("02-html");
        fs::create_dir_all(&html_dir).expect("html dir");
        let html_tail = "\n<body>\n<pre>id: keep-me\n<meta name=\"nocturne:id\" content=\"example\"></pre>\n</body>\n</html>\n";
        let html_page = format!("<!doctype html>\n<html>\n<head>\n<meta name=\"nocturne:id\" content=\"old-html\">\n</head>{html_tail}");
        fs::write(html_dir.join("index.html"), &html_page).expect("write html");

        let html_missing_dir = root.join("03-html-missing");
        fs::create_dir_all(&html_missing_dir).expect("html missing dir");
        let html_missing = format!("<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n</head>{html_tail}");
        fs::write(html_missing_dir.join("index.html"), &html_missing).expect("write html missing");

        normalize_ids(root).expect("normalize_ids");

        let md_after = fs::read_to_string(md_dir.join("index.md")).expect("read md");
        let (front, body_after) = nocturne_formats::frontmatter::split_front(&md_after);
        assert_eq!(nocturne_formats::frontmatter::value(&front, "id").as_deref(), Some("00000000000000000002"));
        assert_eq!(body_after, md_body, "markdown body must remain untouched");

        let html_after = fs::read_to_string(html_dir.join("index.html")).expect("read html");
        assert_eq!(nocturne_formats::html::meta(&html_after, "nocturne:id").as_deref(), Some("00000000000000000003"));
        assert!(html_after.ends_with(html_tail), "html body must remain untouched:\n{html_after}");

        let html_missing_after = fs::read_to_string(html_missing_dir.join("index.html")).expect("read html missing");
        assert_eq!(nocturne_formats::html::meta(&html_missing_after, "nocturne:id").as_deref(), Some("00000000000000000004"));
        assert!(html_missing_after.ends_with(html_tail), "body meta mention must not be rewritten:\n{html_missing_after}");
    }

    #[cfg(unix)]
    #[test]
    fn run_bridge_drains_large_stdout_and_stderr_without_deadlocking() {
        use std::os::unix::fs::PermissionsExt;

        let dir = nocturne_store::testing::TempDir::new("officeimo-pipe-drain");
        let script = dir.path().join("chatty-bridge.sh");
        // Write >128 KiB to both stdout and stderr so unread OS pipes would deadlock.
        let body = "#!/bin/sh\nline='0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'\ni=0\nwhile [ \"$i\" -lt 2200 ]; do\n  printf '%s\\n' \"$line\"\n  printf '%s\\n' \"$line\" >&2\n  i=$((i + 1))\ndone\n";
        fs::write(&script, body).expect("write script");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");

        let cancelled = AtomicBool::new(false);
        let stdout = run_bridge(&script, dir.path(), dir.path(), Format::Html, &cancelled).expect("bridge succeeds");
        assert!(stdout.len() >= 2200 * 65, "all stdout drained: {} bytes", stdout.len());
    }
}
