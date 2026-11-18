//! Imports files staged in `Documents/Inbox` on iPadOS / iOS.
//!
//! Supported formats (`.md`/`.markdown`, `.html`/`.htm`, `.one`, `.onepkg`) are imported
//! into the active [`Session`] and removed from `Inbox`. Anything unsupported or unreadable
//! is moved to `<Documents>/Inbox-Failed/` with a unique filename so user files are never discarded.

use nocturne_engine::{Output, Params, Session};
use nocturne_formats::{frontmatter, html};
use nocturne_import::{Existing, Options};
use nocturne_model::{NodeKind, PageFormat};
use nocturne_store::Node;
use nocturne_store::ops;
use nocturne_store::page::MAX_PAGE_BYTES;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

const QUICK_NOTES_NOTEBOOK: &str = "Quick Notes";
const IMPORTED_SECTION: &str = "Imported";
const FAILED_FOLDER: &str = "Inbox-Failed";
const DEFAULT_NOTE_TITLE: &str = "Imported note";

/// Outcome of a pass over the iOS `Documents/Inbox` folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IngestReport {
    /// Path of the last imported page or notebook node to open in the UI, if any.
    pub last_opened: Option<PathBuf>,
    /// Human-readable notices/errors to surface to the user.
    pub notices: Vec<String>,
}

/// Imports every supported regular file currently waiting in `inbox`.
///
/// Missing `inbox` directories are treated as empty. Directories (such as `Inbox-Failed`)
/// and hidden dot-files are skipped; failed or unsupported files are quarantined in
/// `<Documents>/Inbox-Failed/` without data loss.
pub fn ingest_inbox(inbox: &Path, session: &mut Session) -> IngestReport {
    let mut report = IngestReport::default();
    if !inbox.is_dir() {
        return report;
    }

    let files = match collect_inbox_files(inbox) {
        Ok(files) => files,
        Err(err) => {
            report.notices.push(format!("Could not inspect Inbox {}: {err}.", inbox.display()));
            return report;
        }
    };

    for file_path in files {
        let ext = file_path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
        match ext.as_deref() {
            Some("md" | "markdown") => import_text_note(inbox, &file_path, session, PageFormat::Cmgfm, &mut report),
            Some("html" | "htm") => import_text_note(inbox, &file_path, session, PageFormat::Html, &mut report),
            Some("one" | "onepkg") => import_onenote(inbox, &file_path, session, &mut report),
            _ => quarantine_with_notice(inbox, &file_path, "unsupported file type", &mut report),
        }
    }

    report
}

fn collect_inbox_files(inbox: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(inbox)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        // Skip directories (including `Failed`) and non-regular entries such as symlinks.
        if entry.file_type().is_ok_and(|kind| kind.is_file()) {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

fn import_text_note(inbox: &Path, file_path: &Path, session: &mut Session, format: PageFormat, report: &mut IngestReport) {
    let meta = match fs::metadata(file_path) {
        Ok(meta) => meta,
        Err(err) => {
            quarantine_with_notice(inbox, file_path, &format!("could not read file metadata: {err}"), report);
            return;
        }
    };
    if meta.len() > MAX_PAGE_BYTES as u64 {
        quarantine_with_notice(inbox, file_path, "file exceeds maximum page size", report);
        return;
    }

    let text = match fs::read_to_string(file_path) {
        Ok(text) => text,
        Err(err) => {
            quarantine_with_notice(inbox, file_path, &format!("not valid UTF-8 text: {err}"), report);
            return;
        }
    };

    let body = match format {
        PageFormat::Cmgfm => frontmatter::split_front(&text).1.trim_start_matches(['\r', '\n']).to_string(),
        PageFormat::Html => html::inner(&text),
    };
    if body.len() > MAX_PAGE_BYTES {
        quarantine_with_notice(inbox, file_path, "page body exceeds maximum page size", report);
        return;
    }

    let raw_stem = file_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let title = nocturne_import::clean_title(&raw_stem, DEFAULT_NOTE_TITLE);

    match create_and_save_page(session, &title, format, &body) {
        Ok(page_path) => {
            report.last_opened = Some(page_path);
            let file_name = display_name(file_path);
            if let Err(err) = fs::remove_file(file_path) {
                report
                    .notices
                    .push(format!("Imported \"{title}\" into {QUICK_NOTES_NOTEBOOK} / {IMPORTED_SECTION}, but could not remove {file_name}: {err}."));
            } else {
                report.notices.push(format!("Imported \"{title}\" into {QUICK_NOTES_NOTEBOOK} / {IMPORTED_SECTION}."));
            }
        }
        Err(reason) => quarantine_with_notice(inbox, file_path, &reason, report),
    }
}

fn import_onenote(inbox: &Path, file_path: &Path, session: &mut Session, report: &mut IngestReport) {
    let mut opts = Options::new(file_path, session.root());
    opts.existing = Existing::Keep;
    opts.format = session.settings.global_format().0;

    let before_pages = collect_node_paths(session.tree(), Some(NodeKind::Page));
    let before_all = collect_node_paths(session.tree(), None);

    match nocturne_import::run(&opts, &mut |_| {}) {
        Ok(import_report) if import_report.sections_imported > 0 || import_report.pages > 0 || import_report.skipped.is_empty() => {
            session.refresh();
            if let Some(opened) = find_first_new_node(session.tree(), &before_pages, Some(NodeKind::Page))
                .or_else(|| find_first_new_node(session.tree(), &before_all, None))
            {
                report.last_opened = Some(opened);
            }
            let file_name = display_name(file_path);
            if let Err(err) = fs::remove_file(file_path) {
                report.notices.push(format!(
                    "Imported {file_name} ({} section(s), {} page(s)), but could not remove it from Inbox: {err}.",
                    import_report.sections_imported, import_report.pages
                ));
            } else {
                let mut notice = format!("Imported {file_name}: {} section(s), {} page(s).", import_report.sections_imported, import_report.pages);
                if !import_report.skipped.is_empty() {
                    notice.push_str(&format!(" ({} skipped)", import_report.skipped.len()));
                }
                report.notices.push(notice);
            }
        }
        Ok(import_report) => {
            let reason = import_report
                .skipped
                .first()
                .map(|skipped| skipped.reason.clone())
                .unwrap_or_else(|| "no sections or pages could be imported".to_string());
            quarantine_with_notice(inbox, file_path, &reason, report);
        }
        Err(err) => quarantine_with_notice(inbox, file_path, &err, report),
    }
}

fn ensure_imported_section(session: &mut Session) -> Result<PathBuf, String> {
    let notebook_path = match session.tree().iter().find(|node| node.info.kind == NodeKind::Notebook && node.info.title == QUICK_NOTES_NOTEBOOK) {
        Some(notebook) => notebook.path.clone(),
        None => {
            let out = session.run("notebook.create", &params(&[("name", QUICK_NOTES_NOTEBOOK), ("empty", "true")])).map_err(|e| e.to_string())?;
            output_path(out, "notebook.create")?
        }
    };

    if let Some(section) = session
        .tree()
        .iter()
        .find(|node| node.path == notebook_path)
        .and_then(|notebook| notebook.children.iter().find(|child| child.info.kind == NodeKind::Section && child.info.title == IMPORTED_SECTION))
    {
        return Ok(section.path.clone());
    }

    let nb_str = path_utf8(&notebook_path)?;
    let out =
        session.run("node.create", &params(&[("parent", nb_str), ("kind", "section"), ("title", IMPORTED_SECTION)])).map_err(|e| e.to_string())?;
    output_path(out, "node.create")
}

fn create_and_save_page(session: &mut Session, title: &str, format: PageFormat, body: &str) -> Result<PathBuf, String> {
    let section_path = ensure_imported_section(session)?;
    let section_str = path_utf8(&section_path)?;
    let out = session
        .run("node.create", &params(&[("parent", section_str), ("kind", "page"), ("title", title), ("format", format.id())]))
        .map_err(|e| e.to_string())?;
    let page_path = output_path(out, "node.create")?;
    let page_str = path_utf8(&page_path)?;

    if let Err(err) = session.run("page.save", &params(&[("path", page_str), ("body", body)])) {
        let _ = ops::delete_node(session.root(), &page_path);
        session.refresh();
        return Err(err.to_string());
    }

    Ok(page_path)
}

fn quarantine_dir(inbox: &Path) -> PathBuf {
    inbox.parent().unwrap_or(inbox).join(FAILED_FOLDER)
}

fn quarantine_with_notice(inbox: &Path, file_path: &Path, reason: &str, report: &mut IngestReport) {
    let file_name = display_name(file_path);
    match move_to_failed(inbox, file_path) {
        Ok(dest) => {
            let dest_name = display_name(&dest);
            report.notices.push(format!("Could not import {file_name} ({reason}); moved to {FAILED_FOLDER}/{dest_name}."));
        }
        Err(err) => {
            report.notices.push(format!("Could not import {file_name} ({reason}) and failed to quarantine it: {err}."));
        }
    }
}

fn move_to_failed(inbox: &Path, file_path: &Path) -> Result<PathBuf, String> {
    let failed_dir = quarantine_dir(inbox);
    fs::create_dir_all(&failed_dir).map_err(|e| format!("cannot create {}: {e}", failed_dir.display()))?;
    let name = file_path.file_name().unwrap_or_else(|| OsStr::new("file"));
    let dest = unique_failed_path(&failed_dir, name);
    if fs::rename(file_path, &dest).is_err() {
        fs::copy(file_path, &dest).map_err(|e| format!("cannot copy to {}: {e}", dest.display()))?;
        if let Err(err) = fs::remove_file(file_path) {
            let _ = fs::remove_file(&dest);
            return Err(format!("cannot remove original {}: {err}", file_path.display()));
        }
    }
    Ok(dest)
}

fn unique_failed_path(failed_dir: &Path, file_name: &OsStr) -> PathBuf {
    let candidate = failed_dir.join(file_name);
    if !candidate.exists() && fs::symlink_metadata(&candidate).is_err() {
        return candidate;
    }
    let path = Path::new(file_name);
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| "file".to_string());
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).filter(|e| !e.is_empty());
    let mut index = 2usize;
    loop {
        let numbered = match &ext {
            Some(ext) => format!("{stem}-{index}.{ext}"),
            None => format!("{stem}-{index}"),
        };
        let next = failed_dir.join(numbered);
        if !next.exists() && fs::symlink_metadata(&next).is_err() {
            return next;
        }
        index += 1;
    }
}

fn collect_node_paths(nodes: &[Node], kind_filter: Option<NodeKind>) -> HashSet<PathBuf> {
    let mut paths = HashSet::new();
    let mut stack: Vec<&Node> = nodes.iter().collect();
    while let Some(node) = stack.pop() {
        if kind_filter.is_none_or(|kind| node.info.kind == kind) {
            paths.insert(node.path.clone());
        }
        for child in &node.children {
            stack.push(child);
        }
    }
    paths
}

fn find_first_new_node(nodes: &[Node], before: &HashSet<PathBuf>, kind_filter: Option<NodeKind>) -> Option<PathBuf> {
    for node in nodes {
        if kind_filter.is_none_or(|kind| node.info.kind == kind) && !before.contains(&node.path) {
            return Some(node.path.clone());
        }
        if let Some(found) = find_first_new_node(&node.children, before, kind_filter) {
            return Some(found);
        }
    }
    None
}

fn params(pairs: &[(&str, &str)]) -> Params {
    pairs.iter().map(|(key, value)| ((*key).to_string(), (*value).to_string())).collect()
}

fn output_path(output: Output, command: &str) -> Result<PathBuf, String> {
    match output {
        Output::Path(path) => Ok(path),
        _ => Err(format!("{command} returned an unexpected result")),
    }
}

fn path_utf8(path: &Path) -> Result<&str, String> {
    path.to_str().ok_or_else(|| format!("path {} is not valid UTF-8", path.display()))
}

fn display_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_store::Settings;
    use nocturne_store::page::load_page;
    use nocturne_store::testing::TempDir;

    fn test_session(temp: &TempDir) -> (PathBuf, Session) {
        let inbox = temp.path().join("Inbox");
        let notebooks = temp.path().join("Notebooks");
        let settings = Settings::at(temp.path().join("settings.toml"));
        let session = Session::new(&notebooks, settings).expect("session");
        (inbox, session)
    }

    #[test]
    fn missing_or_empty_inbox_returns_default_report() {
        let temp = TempDir::new("ios-inbox-empty");
        let (inbox, mut session) = test_session(&temp);

        assert_eq!(ingest_inbox(&inbox, &mut session), IngestReport::default());
        assert!(!inbox.exists(), "missing inbox is not created");

        fs::create_dir_all(&inbox).expect("inbox dir");
        fs::create_dir_all(quarantine_dir(&inbox)).expect("failed dir");
        fs::write(inbox.join(".DS_Store"), b"ignored").expect("hidden file");
        assert_eq!(ingest_inbox(&inbox, &mut session), IngestReport::default());
        assert!(inbox.join(".DS_Store").is_file(), "hidden dot-files are left untouched");
        assert!(session.tree().is_empty());
    }

    #[test]
    fn markdown_files_strip_frontmatter_and_delete_source_files() {
        let temp = TempDir::new("ios-inbox-md");
        let (inbox, mut session) = test_session(&temp);
        fs::create_dir_all(&inbox).expect("inbox dir");

        let md_path = inbox.join("Meeting Notes.md");
        fs::write(&md_path, "\u{feff}---\ntitle: External Title\ntags: [work]\n---\n\n# Agenda\n\n- Review roadmap\n").expect("write md");
        let upper_path = inbox.join("Follow Up.MARKDOWN");
        fs::write(&upper_path, "Plain markdown body\n").expect("write markdown");

        let report = ingest_inbox(&inbox, &mut session);
        assert_eq!(report.notices.len(), 2, "{:?}", report.notices);
        assert!(!md_path.exists(), "source .md file is deleted after import");
        assert!(!upper_path.exists(), "source .MARKDOWN file is deleted after import");

        let notebook = session.tree().iter().find(|node| node.info.title == QUICK_NOTES_NOTEBOOK).expect("Quick Notes");
        let section = notebook.children.iter().find(|node| node.info.title == IMPORTED_SECTION).expect("Imported");
        assert_eq!(section.children.len(), 2);

        let meeting_node = section.children.iter().find(|node| node.info.title == "Meeting Notes").expect("Meeting Notes page");
        assert_eq!(meeting_node.info.format, Some(PageFormat::Cmgfm));
        let meeting_page = load_page(&meeting_node.path).expect("load meeting page");
        assert!(meeting_page.body.contains("# Agenda"));
        assert!(meeting_page.body.contains("Review roadmap"));
        assert!(!meeting_page.body.contains("External Title"), "frontmatter was stripped: {}", meeting_page.body);

        let followup_node = section.children.iter().find(|node| node.info.title == "Follow Up").expect("Follow Up page");
        assert_eq!(followup_node.info.format, Some(PageFormat::Cmgfm));
        let followup_page = load_page(&followup_node.path).expect("load followup page");
        assert!(followup_page.body.contains("Plain markdown body"));

        assert_eq!(report.last_opened, Some(meeting_node.path.clone()));

        // A second ingest pass reuses the existing Quick Notes / Imported section.
        fs::write(inbox.join("Third.md"), "More notes\n").expect("write third md");
        let second_report = ingest_inbox(&inbox, &mut session);
        assert_eq!(session.tree().len(), 1);
        let section = &session.tree()[0].children[0];
        assert_eq!(section.children.len(), 3);
        assert!(second_report.last_opened.is_some());
    }

    #[test]
    fn html_files_extract_inner_body_and_preserve_content() {
        let temp = TempDir::new("ios-inbox-html");
        let (inbox, mut session) = test_session(&temp);
        fs::create_dir_all(&inbox).expect("inbox dir");

        let html_path = inbox.join("Web Clip.HTML");
        fs::write(
            &html_path,
            "<!doctype html>\n<html><head><title>Web</title></head><body class=\"reader\">\n<h1>Headline</h1>\n<p>Saved paragraph</p>\n</body></html>\n",
        )
        .expect("write html");
        let htm_path = inbox.join("Snippet.htm");
        fs::write(&htm_path, "<p>Bare fragment</p>").expect("write htm");

        let report = ingest_inbox(&inbox, &mut session);
        assert_eq!(report.notices.len(), 2, "{:?}", report.notices);
        assert!(!html_path.exists());
        assert!(!htm_path.exists());

        let section = &session.tree()[0].children[0];
        let clip_node = section.children.iter().find(|node| node.info.title == "Web Clip").expect("Web Clip page");
        assert_eq!(clip_node.info.format, Some(PageFormat::Html));
        let clip_page = load_page(&clip_node.path).expect("load html page");
        assert_eq!(clip_page.body, "<h1>Headline</h1>\n<p>Saved paragraph</p>");

        let snippet_node = section.children.iter().find(|node| node.info.title == "Snippet").expect("Snippet page");
        let snippet_page = load_page(&snippet_node.path).expect("load htm page");
        assert_eq!(snippet_page.body, "<p>Bare fragment</p>");
        assert_eq!(report.last_opened, Some(clip_node.path.clone()));
    }

    #[test]
    fn unsupported_and_corrupt_files_are_quarantined_without_loss() {
        let temp = TempDir::new("ios-inbox-quarantine");
        let (inbox, mut session) = test_session(&temp);
        fs::create_dir_all(&inbox).expect("inbox dir");
        let failed = quarantine_dir(&inbox);
        fs::create_dir_all(&failed).expect("failed dir");
        fs::write(failed.join("notes.txt"), b"existing failed note").expect("existing failed");

        fs::write(inbox.join("notes.txt"), b"new unsupported note").expect("write txt");
        fs::write(inbox.join("bad.md"), [0xFF, 0xFE, 0x00, 0x80]).expect("write invalid utf8 md");
        fs::write(inbox.join("damaged.one"), b"not a real section").expect("write bad one");
        fs::write(inbox.join("damaged.onepkg"), b"not a real cabinet").expect("write bad onepkg");

        let report = ingest_inbox(&inbox, &mut session);
        assert_eq!(report.last_opened, None);
        assert_eq!(report.notices.len(), 4, "{:?}", report.notices);

        assert!(!inbox.join("notes.txt").exists());
        assert!(!inbox.join("bad.md").exists());
        assert!(!inbox.join("damaged.one").exists());
        assert!(!inbox.join("damaged.onepkg").exists());

        assert_eq!(fs::read(failed.join("notes.txt")).expect("existing kept"), b"existing failed note");
        assert_eq!(fs::read(failed.join("notes-2.txt")).expect("unique quarantined txt"), b"new unsupported note");
        assert_eq!(fs::read(failed.join("bad.md")).expect("quarantined md"), [0xFF, 0xFE, 0x00, 0x80]);
        assert_eq!(fs::read(failed.join("damaged.one")).expect("quarantined one"), b"not a real section");
        assert_eq!(fs::read(failed.join("damaged.onepkg")).expect("quarantined onepkg"), b"not a real cabinet");

        assert_eq!(fs::read_dir(&inbox).expect("read inbox").count(), 0, "quarantined files are removed from Inbox");

        // Re-running does not touch Inbox-Failed/ contents.
        assert_eq!(ingest_inbox(&inbox, &mut session), IngestReport::default());
    }

    #[test]
    fn valid_onenote_section_is_imported_and_removed_from_inbox() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/import/tests/fixtures/synthetic/formatted-text.one");
        let temp = TempDir::new("ios-inbox-one");
        let (inbox, mut session) = test_session(&temp);
        fs::create_dir_all(&inbox).expect("inbox dir");

        let staged = inbox.join("Formatted.one");
        fs::copy(&fixture, &staged).expect("copy fixture");

        let report = ingest_inbox(&inbox, &mut session);
        assert!(!staged.exists(), "imported .one file is removed from Inbox");
        assert!(!quarantine_dir(&inbox).exists(), "valid .one file was not quarantined");
        assert_eq!(report.notices.len(), 1, "{:?}", report.notices);
        let opened = report.last_opened.expect("imported page selected");
        assert!(opened.is_dir(), "last_opened points to imported page folder: {}", opened.display());
        assert!(!session.tree().is_empty());
    }
}
