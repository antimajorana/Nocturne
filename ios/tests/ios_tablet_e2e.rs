use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use nocturne_engine::{Output, Params, Session};
use nocturne_ios::diagnostics::{
    CONTROL_PORT_ENV, DIAGNOSTICS_FILENAME, MAX_EXPORT_BYTES, export_diagnostics_log, parse_control_port, requested_control_port_from_env,
    trim_to_tail_utf8_line,
};
use nocturne_ios::inbox::{IngestReport, ingest_inbox};
use nocturne_ios::keyboard::compute_keyboard_bottom_inset;
use nocturne_ios::pasteboard_codec::{decode_png_picture, decode_utf8_html};
use nocturne_ios::storage::{INBOX_FOLDER, NOTEBOOKS_FOLDER, layout};
use nocturne_model::{NodeKind, PageFormat};
use nocturne_store::{Settings, testing::TempDir};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

fn params(pairs: &[(&str, &str)]) -> Params {
    pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
}

fn assert_close(actual: f32, expected: f32, label: &str) {
    assert!((actual - expected).abs() <= 1e-4, "{label}: expected {expected}, got {actual}");
}

fn encode_rgba_png(width: u32, height: u32) -> Vec<u8> {
    let mut img = RgbaImage::from_pixel(width, height, Rgba([12, 34, 56, 255]));
    if width > 1 && height > 1 {
        img.put_pixel(1, 1, Rgba([220, 120, 40, 200]));
    }
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(img).write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png).expect("encode png");
    bytes
}

fn encode_rgb_jpeg(width: u32, height: u32) -> Vec<u8> {
    let img = RgbImage::from_pixel(width, height, Rgb([200, 40, 90]));
    let mut bytes = Vec::new();
    DynamicImage::ImageRgb8(img).write_to(&mut Cursor::new(&mut bytes), ImageFormat::Jpeg).expect("encode jpeg");
    bytes
}

fn assert_no_tmp_files(dir: &Path) {
    if !dir.exists() {
        return;
    }
    let tmp_files: Vec<PathBuf> = fs::read_dir(dir)
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().ends_with(".tmp")))
        .collect();
    assert!(tmp_files.is_empty(), "unexpected .tmp files in {}: {tmp_files:?}", dir.display());
}

fn load_page_via_engine(session: &mut Session, page_path: &Path) -> (PageFormat, String, String) {
    let path_str = page_path.to_str().expect("utf8 page path");
    let out = session.run("page.load", &params(&[("path", path_str)])).expect("page.load");
    match out {
        Output::Page(page) => (page.format, page.title, page.body),
        other => panic!("expected Output::Page, got {other:?}"),
    }
}

// ============================================================================
// Tier 1: Core Happy-Path Behavior (>= 5 tests)
// ============================================================================

#[test]
fn tier1_storage_layout_maps_ios_sandbox_without_creating_inbox() {
    let temp = TempDir::new("ios-e2e-t1-layout");
    let sandbox_home = temp.path().join("sandbox");
    let bundle_res = temp.path().join("BundleResources");
    let welcome = bundle_res.join("WelcomeNotebook");
    fs::create_dir_all(welcome.join("html")).expect("welcome html");
    fs::create_dir_all(welcome.join("cmgfm")).expect("welcome cmgfm");

    assert_eq!(layout(None, None), None);
    assert_eq!(layout(None, Some(&bundle_res)), None);

    let resolved = layout(Some(&sandbox_home), Some(&bundle_res)).expect("valid layout");
    assert_eq!(resolved.notebooks, sandbox_home.join("Documents").join(NOTEBOOKS_FOLDER));
    assert_eq!(resolved.inbox, sandbox_home.join("Documents").join(INBOX_FOLDER));
    assert_eq!(resolved.app_data, sandbox_home.join("Library").join("Application Support").join(NOTEBOOKS_FOLDER));
    assert_eq!(resolved.settings, resolved.app_data.join("settings.toml"));
    assert_eq!(resolved.welcome_payload, Some(welcome));

    // Layout calculation must be pure and never create sandbox directories (especially Documents/Inbox).
    assert!(!sandbox_home.exists(), "layout must not create sandbox_home on disk");
    assert!(!resolved.inbox.exists(), "layout must never create Documents/Inbox");
    assert!(!resolved.notebooks.exists(), "layout must not pre-create Documents/Nocturne");
}

#[test]
fn tier1_ingest_missing_inbox_returns_empty_and_never_creates_inbox() {
    let temp = TempDir::new("ios-e2e-t1-missing-inbox");
    let sandbox_home = temp.path().join("sandbox");
    let resolved = layout(Some(&sandbox_home), None).expect("layout");
    fs::create_dir_all(&resolved.app_data).expect("app_data");
    fs::create_dir_all(&resolved.notebooks).expect("notebooks");

    let mut session = Session::new(&resolved.notebooks, Settings::at(&resolved.settings)).expect("session");
    assert!(!resolved.inbox.exists());

    let report = ingest_inbox(&resolved.inbox, &mut session);
    assert_eq!(report, IngestReport::default());
    assert!(!resolved.inbox.exists(), "ingest_inbox must never create Documents/Inbox");
    assert!(session.tree().is_empty(), "missing inbox must not create notebooks or sections");
}

#[test]
fn tier1_ingest_valid_markdown_and_html_into_quick_notes_imported() {
    let temp = TempDir::new("ios-e2e-t1-ingest-valid");
    let sandbox_home = temp.path().join("sandbox");
    let resolved = layout(Some(&sandbox_home), None).expect("layout");
    fs::create_dir_all(&resolved.app_data).expect("app_data");
    fs::create_dir_all(&resolved.notebooks).expect("notebooks");
    fs::create_dir_all(&resolved.inbox).expect("external share creates Inbox");

    let md_path = resolved.inbox.join("Lecture Summary.md");
    fs::write(&md_path, "\u{feff}---\ntitle: Ignored\nauthor: External\n---\n\n# Key Equations\n\n- Action item\n").expect("write md");

    let html_path = resolved.inbox.join("Article Clip.html");
    fs::write(&html_path, "<!doctype html><html><head><title>Clip</title></head><body><h1>Headline</h1><p>Clipped body</p></body></html>")
        .expect("write html");

    let mut session = Session::new(&resolved.notebooks, Settings::at(&resolved.settings)).expect("session");
    let report = ingest_inbox(&resolved.inbox, &mut session);

    assert_eq!(report.notices.len(), 2, "expected 2 import notices, got {:?}", report.notices);
    assert!(!md_path.exists(), "imported .md source should be removed from Inbox");
    assert!(!html_path.exists(), "imported .html source should be removed from Inbox");
    assert!(!resolved.inbox.parent().expect("Documents").join("Inbox-Failed").exists(), "valid files must not trigger Inbox-Failed");

    let (md_page_path, html_page_path) = {
        let quick_notes = session.tree().iter().find(|n| n.info.kind == NodeKind::Notebook && n.info.title == "Quick Notes").expect("Quick Notes");
        let imported = quick_notes.children.iter().find(|n| n.info.kind == NodeKind::Section && n.info.title == "Imported").expect("Imported");
        assert_eq!(imported.children.len(), 2);

        let md_path = imported.children.iter().find(|n| n.info.title == "Lecture Summary").expect("Lecture Summary page").path.clone();
        let html_path = imported.children.iter().find(|n| n.info.title == "Article Clip").expect("Article Clip page").path.clone();
        (md_path, html_path)
    };

    let (md_fmt, _, md_body) = load_page_via_engine(&mut session, &md_page_path);
    assert_eq!(md_fmt, PageFormat::Cmgfm);
    assert!(md_body.contains("# Key Equations"));
    assert!(md_body.contains("- Action item"));
    assert!(!md_body.contains("author: External"), "YAML frontmatter should be stripped: {md_body}");

    let (html_fmt, _, html_body) = load_page_via_engine(&mut session, &html_page_path);
    assert_eq!(html_fmt, PageFormat::Html);
    assert_eq!(html_body, "<h1>Headline</h1><p>Clipped body</p>");

    let opened = report.last_opened.expect("last_opened should point to an imported page");
    assert!(opened == md_page_path || opened == html_page_path);
}

#[test]
fn tier1_keyboard_docked_portrait_and_landscape_expand_bottom_inset() {
    // Portrait iPad Air (1180 x 820 or 1024 x 768): docked 314pt keyboard
    let portrait = compute_keyboard_bottom_inset(1024.0, 710.0, 314.0, 20.0);
    assert_close(portrait, 314.0, "portrait docked keyboard");

    // Landscape iPad: docked 353pt keyboard on 768pt screen
    let landscape = compute_keyboard_bottom_inset(768.0, 415.0, 353.0, 20.0);
    assert_close(landscape, 353.0, "landscape docked keyboard");

    // Hidden / offscreen keyboard preserves hardware safe_bottom
    assert_close(compute_keyboard_bottom_inset(1024.0, 1024.0, 314.0, 20.0), 20.0, "keyboard at bottom edge");
    assert_close(compute_keyboard_bottom_inset(1024.0, 1200.0, 314.0, 24.0), 24.0, "keyboard below screen");
}

#[test]
fn tier1_diagnostics_export_writes_nocturne_diagnostics_log() {
    let temp = TempDir::new("ios-e2e-t1-diag");
    let active_log = temp.path().join("Library").join("Application Support").join("Nocturne").join("nocturne.log");
    let notebooks_dir = temp.path().join("Documents").join("Nocturne");

    // Missing active log still produces the target diagnostics file with an init notice.
    let exported_missing = export_diagnostics_log(&active_log, &notebooks_dir).expect("export missing log");
    assert_eq!(exported_missing, notebooks_dir.join(DIAGNOSTICS_FILENAME));
    let placeholder = fs::read_to_string(&exported_missing).expect("read placeholder");
    assert!(placeholder.contains("Nocturne iOS diagnostics log initialized"));
    assert_no_tmp_files(&notebooks_dir);

    // Populated active log is exported verbatim.
    fs::create_dir_all(active_log.parent().expect("log parent")).expect("create log dir");
    let content = "2026-11-17 12:00:00 INFO boot\n2026-11-17 12:00:01 INFO window ready\n";
    fs::write(&active_log, content).expect("write active log");
    let exported = export_diagnostics_log(&active_log, &notebooks_dir).expect("export populated log");
    assert_eq!(exported, notebooks_dir.join(DIAGNOSTICS_FILENAME));
    assert_eq!(fs::read_to_string(&exported).expect("read exported"), content);
    assert_no_tmp_files(&notebooks_dir);

    assert_eq!(parse_control_port(Some("0")), Some(0));
    assert_eq!(parse_control_port(Some(" 49152 ")), Some(49152));
    let _ = (CONTROL_PORT_ENV, requested_control_port_from_env());
}

#[test]
fn tier1_pasteboard_codec_decodes_valid_png_and_utf8_html() {
    let png_bytes = encode_rgba_png(4, 3);
    let pic_png = decode_png_picture(&png_bytes).expect("valid 4x3 PNG");
    assert_eq!((pic_png.width, pic_png.height), (4, 3));
    assert_eq!(pic_png.png, png_bytes, "PNG input should be preserved verbatim");

    let jpeg_bytes = encode_rgb_jpeg(6, 5);
    let pic_jpeg = decode_png_picture(&jpeg_bytes).expect("valid 6x5 JPEG re-encoded to PNG");
    assert_eq!((pic_jpeg.width, pic_jpeg.height), (6, 5));
    assert_eq!(image::guess_format(&pic_jpeg.png).expect("png output"), ImageFormat::Png);

    assert_eq!(decode_utf8_html(b"<p>Hello <b>iPad</b></p>").as_deref(), Some("<p>Hello <b>iPad</b></p>"));
    assert_eq!(decode_utf8_html(b"<div>Trim NUL</div>\0\0").as_deref(), Some("<div>Trim NUL</div>"));
}

// ============================================================================
// Tier 2: Edge Cases, Boundaries & Hostile Inputs (>= 5 tests)
// ============================================================================

#[test]
fn tier2_keyboard_floating_mid_screen_and_safe_bottom_boundaries() {
    // Floating iPad keyboard hovering mid-screen (bottom = 660 < 1024 - 8, height = 280 < 1024 * 0.55)
    assert_close(compute_keyboard_bottom_inset(1024.0, 380.0, 280.0, 20.0), 20.0, "floating mid-screen portrait");
    assert_close(compute_keyboard_bottom_inset(768.0, 200.0, 260.0, 20.0), 20.0, "floating mid-screen landscape");

    // Exact 8.0pt floating bottom-gap boundary (`kb_y + kb_h < screen_h - 8.0`):
    // 716.0 + 300.0 = 1016.0 == 1024.0 - 8.0 -> NOT floating (docked inset 1024 - 716 = 308.0).
    assert_close(compute_keyboard_bottom_inset(1024.0, 716.0, 300.0, 20.0), 308.0, "exact 8.0pt gap is docked");
    // 715.9 + 300.0 = 1015.9 < 1016.0 -> floating (preserves safe_bottom 20.0).
    assert_close(compute_keyboard_bottom_inset(1024.0, 715.9, 300.0, 20.0), 20.0, "715.9 just inside floating 8.0pt gap");

    // Exact 0.55 height-ratio boundary (`kb_h < screen_h * 0.55`):
    // 550.0 == 1000.0 * 0.55 -> NOT floating (inset 1000 - 200 = 800, clamped to 75% = 750.0).
    assert_close(compute_keyboard_bottom_inset(1000.0, 200.0, 550.0, 20.0), 750.0, "exact 0.55 height ratio is not floating");
    // 549.9 < 550.0 (and 200 + 549.9 = 749.9 < 992) -> floating (preserves safe_bottom 20.0).
    assert_close(compute_keyboard_bottom_inset(1000.0, 200.0, 549.9, 20.0), 20.0, "549.9 just below 0.55 ratio is floating");

    // Floating just above the 8pt gap threshold (380 + 635 = 1015 < 1016, height 635 >= 563.2 -> NOT floating!)
    assert_close(compute_keyboard_bottom_inset(1024.0, 380.0, 635.0, 20.0), 644.0, "tall keyboard exceeding 0.55 height ratio is not floating");

    // Negative `kb_origin_y` extending above top of screen:
    // Tall/docked negative origin clamps to 75% of screen height; short floating negative origin stays at safe_bottom.
    assert_close(compute_keyboard_bottom_inset(1000.0, -50.0, 1050.0, 20.0), 750.0, "negative kb_origin_y docked clamps to 75%");
    assert_close(compute_keyboard_bottom_inset(1000.0, -50.0, 200.0, 20.0), 20.0, "negative kb_origin_y short floating preserves safe_bottom");

    // Hardware safe-bottom larger than docked overlap (and even > 0.75 * screen_h) is preserved
    assert_close(compute_keyboard_bottom_inset(1024.0, 1012.0, 12.0, 24.0), 24.0, "hardware safe_bottom > overlap");
    assert_close(compute_keyboard_bottom_inset(1000.0, 200.0, 800.0, 800.0), 800.0, "hardware safe_bottom > 0.75 * screen_h preserved");

    // Oversized docked keyboard is clamped to 75% of screen height
    assert_close(compute_keyboard_bottom_inset(1000.0, 100.0, 900.0, 20.0), 750.0, "docked inset clamped to 75%");
}

#[test]
fn tier2_keyboard_hostile_nan_inf_negative_and_zero_inputs_never_panic() {
    let cases = [
        (0.0, 500.0, 300.0, 20.0, 20.0),
        (-1024.0, 500.0, 300.0, 20.0, 20.0),
        (1024.0, 500.0, 0.0, 20.0, 20.0),
        (1024.0, 500.0, -300.0, 20.0, 20.0),
        (f32::NAN, 500.0, 300.0, 20.0, 20.0),
        (1024.0, f32::NAN, 300.0, 20.0, 20.0),
        (1024.0, 500.0, f32::NAN, 20.0, 20.0),
        (f32::INFINITY, 500.0, 300.0, 20.0, 20.0),
        (1024.0, f32::NEG_INFINITY, 300.0, 20.0, 20.0),
        (1024.0, 500.0, f32::INFINITY, 20.0, 20.0),
        (1024.0, 710.0, 314.0, -15.0, 314.0),
        (1024.0, 1024.0, 314.0, f32::NAN, 0.0),
        (1024.0, 1024.0, 314.0, f32::INFINITY, 0.0),
        (1024.0, 1024.0, 314.0, -5.0, 0.0),
    ];

    for (screen_h, kb_y, kb_h, safe_b, expected) in cases {
        let inset = compute_keyboard_bottom_inset(screen_h, kb_y, kb_h, safe_b);
        assert!(inset.is_finite() && inset >= 0.0, "non-finite or negative inset for ({screen_h}, {kb_y}, {kb_h}, {safe_b}): {inset}");
        assert_close(inset, expected, "hostile keyboard input");
    }
}

#[test]
fn tier2_inbox_quarantines_unsupported_and_corrupt_to_inbox_failed_with_collisions() {
    let temp = TempDir::new("ios-e2e-t2-quarantine");
    let sandbox_home = temp.path().join("sandbox");
    let resolved = layout(Some(&sandbox_home), None).expect("layout");
    fs::create_dir_all(&resolved.app_data).expect("app_data");
    fs::create_dir_all(&resolved.notebooks).expect("notebooks");
    fs::create_dir_all(&resolved.inbox).expect("inbox");

    let documents = resolved.inbox.parent().expect("Documents");
    let failed_dir = documents.join("Inbox-Failed");
    fs::create_dir_all(&failed_dir).expect("pre-create Inbox-Failed");
    fs::write(failed_dir.join("clip.bin"), b"existing-1").expect("existing failed 1");
    fs::write(failed_dir.join("clip-2.bin"), b"existing-2").expect("existing failed 2");

    // Stage unsupported, corrupt, dotfile, and subdirectory inside Documents/Inbox.
    fs::write(resolved.inbox.join("clip.bin"), b"new-binary-payload").expect("write clip.bin");
    fs::write(resolved.inbox.join("broken.md"), [0xFF, 0xFE, 0x80, 0x00]).expect("write invalid utf8 md");
    fs::write(resolved.inbox.join("corrupt.one"), b"not-a-onenote-file").expect("write corrupt one");
    fs::write(resolved.inbox.join(".DS_Store"), b"apple-meta").expect("write dotfile");
    fs::create_dir_all(resolved.inbox.join("NestedFolder")).expect("write nested dir");

    let mut session = Session::new(&resolved.notebooks, Settings::at(&resolved.settings)).expect("session");
    let report = ingest_inbox(&resolved.inbox, &mut session);

    assert_eq!(report.last_opened, None);
    assert_eq!(report.notices.len(), 3, "expected 3 quarantine notices, got {:?}", report.notices);

    // Quarantined files must live under <Documents>/Inbox-Failed/, NOT inside Documents/Inbox/.
    assert!(!resolved.inbox.join("Failed").exists(), "must never quarantine inside Documents/Inbox/Failed");
    assert!(!resolved.inbox.join("Inbox-Failed").exists(), "must never quarantine inside Documents/Inbox/Inbox-Failed");
    assert!(!resolved.inbox.join("clip.bin").exists());
    assert!(!resolved.inbox.join("broken.md").exists());
    assert!(!resolved.inbox.join("corrupt.one").exists());

    // Dotfile and directory remain untouched.
    assert!(resolved.inbox.join(".DS_Store").is_file());
    assert!(resolved.inbox.join("NestedFolder").is_dir());

    // Collision suffixing preserved previous files and wrote -3.
    assert_eq!(fs::read(failed_dir.join("clip.bin")).expect("read clip"), b"existing-1");
    assert_eq!(fs::read(failed_dir.join("clip-2.bin")).expect("read clip-2"), b"existing-2");
    assert_eq!(fs::read(failed_dir.join("clip-3.bin")).expect("read clip-3"), b"new-binary-payload");
    assert_eq!(fs::read(failed_dir.join("broken.md")).expect("read broken.md"), [0xFF, 0xFE, 0x80, 0x00]);
    assert_eq!(fs::read(failed_dir.join("corrupt.one")).expect("read corrupt.one"), b"not-a-onenote-file");

    // Post-quarantine re-run is idempotent: zero notices and Inbox-Failed untouched.
    let rerun = ingest_inbox(&resolved.inbox, &mut session);
    assert_eq!(rerun, IngestReport::default(), "post-quarantine re-run must be idempotent");
    assert_eq!(fs::read(failed_dir.join("clip-3.bin")).expect("clip-3 preserved"), b"new-binary-payload");
}

#[test]
fn tier2_inbox_extensionless_quarantine_collisions_and_rerun_idempotence() {
    let temp = TempDir::new("ios-e2e-t2-extensionless");
    let sandbox_home = temp.path().join("sandbox");
    let resolved = layout(Some(&sandbox_home), None).expect("layout");
    fs::create_dir_all(&resolved.app_data).expect("app_data");
    fs::create_dir_all(&resolved.notebooks).expect("notebooks");
    fs::create_dir_all(&resolved.inbox).expect("inbox");

    let failed_dir = resolved.inbox.parent().expect("Documents").join("Inbox-Failed");
    let mut session = Session::new(&resolved.notebooks, Settings::at(&resolved.settings)).expect("session");

    // Pass 1: extensionless `README` quarantines to `Inbox-Failed/README`.
    fs::write(resolved.inbox.join("README"), b"readme-1").expect("write README 1");
    let r1 = ingest_inbox(&resolved.inbox, &mut session);
    assert_eq!(r1.notices.len(), 1, "{:?}", r1.notices);
    assert_eq!(fs::read(failed_dir.join("README")).expect("README"), b"readme-1");

    // Pass 2: colliding extensionless `README` quarantines to `Inbox-Failed/README-2`.
    fs::write(resolved.inbox.join("README"), b"readme-2").expect("write README 2");
    let r2 = ingest_inbox(&resolved.inbox, &mut session);
    assert_eq!(r2.notices.len(), 1, "{:?}", r2.notices);
    assert_eq!(fs::read(failed_dir.join("README-2")).expect("README-2"), b"readme-2");

    // Pass 3: third colliding extensionless `README` quarantines to `Inbox-Failed/README-3`.
    fs::write(resolved.inbox.join("README"), b"readme-3").expect("write README 3");
    let r3 = ingest_inbox(&resolved.inbox, &mut session);
    assert_eq!(r3.notices.len(), 1, "{:?}", r3.notices);
    assert_eq!(fs::read(failed_dir.join("README-3")).expect("README-3"), b"readme-3");
    assert_eq!(fs::read_dir(&resolved.inbox).expect("read inbox").count(), 0);

    // Post-quarantine re-run is idempotent and leaves all 3 files intact.
    assert_eq!(ingest_inbox(&resolved.inbox, &mut session), IngestReport::default());
    assert_eq!(fs::read(failed_dir.join("README")).expect("README intact"), b"readme-1");
    assert_eq!(fs::read(failed_dir.join("README-2")).expect("README-2 intact"), b"readme-2");
    assert_eq!(fs::read(failed_dir.join("README-3")).expect("README-3 intact"), b"readme-3");
}

#[test]
fn tier2_diagnostics_oversized_multibyte_utf8_and_non_utf8_and_tmp_cleanup() {
    let temp = TempDir::new("ios-e2e-t2-diag");
    let active_log = temp.path().join("nocturne.log");
    let notebooks_dir = temp.path().join("Notebooks");

    // 1. Oversized (> 512 KiB) log with multibyte UTF-8 trims cleanly at a whole line boundary.
    let line = "2026-11-17 INFO utf8-line 😀 é مرحبا 日本語\n";
    let final_marker = "2026-11-17 INFO final-tail-marker\n";
    let big_log = format!("{}{final_marker}", line.repeat((MAX_EXPORT_BYTES / line.len()) + 12));
    assert!(big_log.len() > MAX_EXPORT_BYTES);
    fs::write(&active_log, &big_log).expect("write oversized log");

    let exported = export_diagnostics_log(&active_log, &notebooks_dir).expect("export oversized log");
    let exported_text = fs::read_to_string(&exported).expect("exported diagnostics must be valid UTF-8");
    assert!(exported_text.len() <= MAX_EXPORT_BYTES);
    assert!(exported_text.starts_with("2026-11-17 INFO "));
    assert!(exported_text.ends_with(final_marker));
    assert!(!exported_text.contains('\u{FFFD}'));
    assert_no_tmp_files(&notebooks_dir);

    // 2. Non-UTF-8 bytes never panic and respect the byte cap.
    fs::write(&active_log, b"\xff\xfe\x80header\n2026-11-17 INFO valid-tail \xff\x90 end\n").expect("write non-utf8 log");
    let lossy_exported = export_diagnostics_log(&active_log, &notebooks_dir).expect("export lossy log");
    let lossy_text = fs::read_to_string(&lossy_exported).expect("lossy output is valid UTF-8");
    assert!(lossy_text.len() <= MAX_EXPORT_BYTES && lossy_text.contains("valid-tail"));
    assert!(trim_to_tail_utf8_line(&[0xFF; 64], 12).len() <= 12);
    assert_eq!(trim_to_tail_utf8_line(b"abc\n", 0), "");

    // 3. Rename failure (target path blocked by a directory) cleans up .tmp files.
    let blocked_notebooks = temp.path().join("BlockedNotebooks");
    fs::create_dir_all(blocked_notebooks.join(DIAGNOSTICS_FILENAME)).expect("create blocking dir");
    let err = export_diagnostics_log(&active_log, &blocked_notebooks).expect_err("rename over dir must fail");
    assert!(err.contains("cannot replace"), "unexpected error: {err}");
    assert_no_tmp_files(&blocked_notebooks);

    // 4. Invalid control port values are rejected.
    for raw in [None, Some(""), Some("   "), Some("-1"), Some("65536"), Some("not-a-port")] {
        assert_eq!(parse_control_port(raw), None, "raw={raw:?}");
    }
}

#[test]
fn tier2_pasteboard_codec_rejects_oversized_corrupt_png_and_whitespace_html() {
    assert!(decode_png_picture(&[]).is_none());
    assert!(decode_png_picture(b"definitely-not-a-png").is_none());

    let valid_png = encode_rgba_png(2, 2);
    let truncated = &valid_png[..valid_png.len().min(16)];
    assert!(decode_png_picture(truncated).is_none());

    let too_wide = encode_rgba_png(20_001, 1);
    assert!(decode_png_picture(&too_wide).is_none(), "width > 20_000 must be rejected");

    let too_tall = encode_rgba_png(1, 20_001);
    assert!(decode_png_picture(&too_tall).is_none(), "height > 20_000 must be rejected");

    let boundary_wide = encode_rgba_png(20_000, 1);
    let ok = decode_png_picture(&boundary_wide).expect("exact 20_000 width should be accepted");
    assert_eq!((ok.width, ok.height), (20_000, 1));

    for invalid_html in [b"".as_slice(), b"\0\0\0".as_slice(), b"   \n\t\r\0".as_slice(), b"\xff\xfe<p>bad</p>\0".as_slice()] {
        assert_eq!(decode_utf8_html(invalid_html), None, "expected None for {invalid_html:?}");
    }
}

// ============================================================================
// Tier 3: Cross-Feature & Static Invariants (>= 2 tests)
// ============================================================================

#[test]
fn tier3_sandbox_layout_inbox_session_persistence_and_diagnostics_roundtrip() {
    let temp = TempDir::new("ios-e2e-t3-roundtrip");
    let sandbox_home = temp.path().join("AppSandbox");
    let bundle_res = temp.path().join("BundleResources");
    fs::create_dir_all(bundle_res.join("WelcomeNotebook").join("html")).expect("html dir");
    fs::create_dir_all(bundle_res.join("WelcomeNotebook").join("cmgfm")).expect("cmgfm dir");

    let resolved = layout(Some(&sandbox_home), Some(&bundle_res)).expect("layout");
    assert!(!resolved.inbox.exists());

    fs::create_dir_all(&resolved.app_data).expect("app_data");
    fs::create_dir_all(&resolved.notebooks).expect("notebooks");
    let active_log = resolved.app_data.join("nocturne.log");
    fs::write(&active_log, "2026-11-17 12:00:00 INFO startup\n").expect("write startup log");

    let md_page_path;
    let html_page_path;
    {
        let mut session = Session::new(&resolved.notebooks, Settings::at(&resolved.settings)).expect("session 1");
        assert_eq!(ingest_inbox(&resolved.inbox, &mut session), IngestReport::default());
        assert!(!resolved.inbox.exists(), "missing Inbox stays absent");

        // External iOS Share sheet drops mixed payload into Documents/Inbox.
        fs::create_dir_all(&resolved.inbox).expect("share creates Inbox");
        fs::write(resolved.inbox.join("Field Notes.markdown"), "---\ntitle: X\n---\n\nObservation 1\n").expect("write md");
        fs::write(resolved.inbox.join("Ref.HTM"), "<html><body><p>Ref body</p></body></html>").expect("write htm");
        fs::write(resolved.inbox.join("archive.bin"), b"\x00\x01\x02\x03").expect("write bin");
        fs::write(resolved.inbox.join(".hidden"), b"ignore").expect("write dotfile");

        let report = ingest_inbox(&resolved.inbox, &mut session);
        assert_eq!(report.notices.len(), 3, "{:?}", report.notices);
        assert!(report.last_opened.is_some());

        let section = &session.tree()[0].children[0];
        md_page_path = section.children.iter().find(|n| n.info.title == "Field Notes").expect("Field Notes").path.clone();
        html_page_path = section.children.iter().find(|n| n.info.title == "Ref").expect("Ref").path.clone();
    }

    // Reopen a fresh Session at the same sandbox paths to verify disk persistence.
    let mut reopened = Session::new(&resolved.notebooks, Settings::at(&resolved.settings)).expect("session 2");
    let (fmt_md, _, body_md) = load_page_via_engine(&mut reopened, &md_page_path);
    assert_eq!(fmt_md, PageFormat::Cmgfm);
    assert_eq!(body_md.trim(), "Observation 1");

    let (fmt_html, _, body_html) = load_page_via_engine(&mut reopened, &html_page_path);
    assert_eq!(fmt_html, PageFormat::Html);
    assert_eq!(body_html, "<p>Ref body</p>");

    // Unsupported file was quarantined to <Documents>/Inbox-Failed/ and dotfile stayed in Inbox.
    let failed = resolved.inbox.parent().expect("Documents").join("Inbox-Failed");
    assert_eq!(fs::read(failed.join("archive.bin")).expect("quarantined bin"), b"\x00\x01\x02\x03");
    assert!(resolved.inbox.join(".hidden").is_file());

    // Diagnostics export writes alongside notebooks with zero temp files.
    let exported = export_diagnostics_log(&active_log, &resolved.notebooks).expect("export diagnostics");
    assert!(exported.is_file());
    assert!(fs::read_to_string(&exported).expect("read exported").contains("INFO startup"));
    assert_no_tmp_files(&resolved.notebooks);
}

#[test]
fn tier3_static_ios_src_audit_inbox_non_creation_and_unsafe_scoping() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut rs_files = Vec::new();
    collect_rs_files_recursive(&src_dir, &mut rs_files);
    assert!(!rs_files.is_empty(), "expected Rust source files in {}", src_dir.display());

    for path in rs_files {
        let rel = path.strip_prefix(&src_dir).expect("path under src_dir");
        let rel_display = rel.to_string_lossy().replace('\\', "/");
        let raw = fs::read_to_string(&path).expect("read source file");
        let file_is_ios_gated = is_module_path_ios_gated(&src_dir, rel);
        audit_ios_source_file(&rel_display, &raw, file_is_ios_gated).unwrap_or_else(|err| panic!("{err}"));
    }
}

#[test]
fn tier3_static_audit_helpers_detect_inbox_unsafe_and_cfg_edges() {
    // 1. `#[cfg(test)] mod tests;` must stop at `;` and NOT swallow following production code.
    let cfg_test_semicolon = r#"
#[cfg(test)]
mod tests;

fn prod_bad(documents: &std::path::Path) {
    let target = documents.join("Inbox");
    let _ = std::fs::create_dir_all(&target);
}
"#;
    let err = audit_ios_source_file("sample.rs", cfg_test_semicolon, false).expect_err("must catch create_dir_all after #[cfg(test)] mod tests;");
    assert!(err.contains("Documents/Inbox"), "unexpected error: {err}");

    // 2. `DirBuilder::new().create(documents.join("Inbox"))` and `DirBuilder` variable + aliased path must be caught.
    let dir_builder_src = r#"
use std::fs::DirBuilder;
fn prod_dir_builder(documents: &std::path::Path) {
    let _ = DirBuilder::new().recursive(true).create(documents.join("Inbox"));
}
"#;
    let err = audit_ios_source_file("dirbuilder.rs", dir_builder_src, false).expect_err("must catch DirBuilder Inbox creation");
    assert!(err.contains("Documents/Inbox"), "unexpected error: {err}");

    // 3. Trailing `//` comment on `#[cfg(target_os = "ios")]` item must NOT leak iOS cfg scope.
    let trailing_comment_leak = r#"
#[cfg(target_os = "ios")]
const X: i32 = 1; // ios-only

#[allow(unsafe_code)]
// SAFETY: bogus outside ios cfg
fn non_ios_unsafe() {}
"#;
    let err = audit_ios_source_file("leak.rs", trailing_comment_leak, false).expect_err("must catch unsafe_code after trailing-comment ios item");
    assert!(err.contains("outside #[cfg(target_os = \"ios\")]"), "unexpected error: {err}");

    // 4. Single-line `#[cfg(target_os = "ios")] pub mod foo;` must be recognized as iOS-gated.
    assert_eq!(parse_ios_gated_modules("#[cfg(target_os = \"ios\")] pub mod foo; // iPadOS\npub mod bar;\n"), vec!["foo".to_string()]);

    // 5. Inner `#![allow(unsafe_code)]` and `#[expect(unsafe_code, unused)]` must be audited,
    //    and `MainThreadMarker` in comments or `#[cfg(test)]` must NOT satisfy production check.
    let inner_allow = r#"
#![allow(unsafe_code)]
// SAFETY: claimed inner allow
// MainThreadMarker only in comment
#[cfg(test)]
mod tests { use objc2::MainThreadMarker; }
"#;
    let err = audit_ios_source_file("inner.rs", inner_allow, true).expect_err("must reject MainThreadMarker only in comments/tests");
    assert!(err.contains("MainThreadMarker"), "unexpected error: {err}");

    let expect_outside_ios = r#"
#[expect(unsafe_code, unused)]
// SAFETY: outside ios
fn bad() {}
"#;
    let err = audit_ios_source_file("expect.rs", expect_outside_ios, false).expect_err("must catch #[expect(unsafe_code, unused)]");
    assert!(err.contains("outside #[cfg(target_os = \"ios\")]"), "unexpected error: {err}");

    // 6. Valid production iOS-gated module with `MainThreadMarker` and `// SAFETY:` passes cleanly.
    let valid_ios = r#"
use objc2::MainThreadMarker;
#[cfg(target_os = "ios")]
pub fn ok() {
    let _mtm = MainThreadMarker::new();
    #[allow(unsafe_code)]
    // SAFETY: guarded by MainThreadMarker
    let _ = 1;
}
"#;
    audit_ios_source_file("valid.rs", valid_ios, false).expect("valid iOS-gated production code should pass");
}

// ============================================================================
// Tier 4: Real-World Application Scenario S5
// ============================================================================

#[test]
fn tier4_scenario_s5_ipad_lifecycle_share_inbox_keyboard_and_diagnostics() {
    let temp = TempDir::new("ios-e2e-t4-s5");
    let sandbox_home = temp.path().join("iPadContainer");
    let resolved = layout(Some(&sandbox_home), None).expect("sandbox layout");

    // 1. First launch: initialize app_data and notebooks, never create Documents/Inbox.
    fs::create_dir_all(&resolved.app_data).expect("app_data");
    fs::create_dir_all(&resolved.notebooks).expect("notebooks");
    let active_log = resolved.app_data.join("nocturne.log");

    let mut session = Session::new(&resolved.notebooks, Settings::at(&resolved.settings)).expect("session");
    assert_eq!(ingest_inbox(&resolved.inbox, &mut session), IngestReport::default());
    assert!(!resolved.inbox.exists());

    // Initial startup diagnostics export (active log absent yet -> placeholder).
    let diag_path = export_diagnostics_log(&active_log, &resolved.notebooks).expect("initial diagnostics");
    assert!(fs::read_to_string(&diag_path).expect("read initial diag").contains("initialized"));

    // 2. App backgrounds; user invokes iOS Share sheet from Safari/Mail to drop files into Documents/Inbox.
    fs::create_dir_all(&resolved.inbox).expect("iOS Share sheet creates Inbox");
    fs::write(resolved.inbox.join("1-Research.md"), "---\ntitle: Ignored\n---\n\n# Abstract\n\nInitial findings.\n").expect("share md");
    fs::write(resolved.inbox.join("2-Clipping.html"), "<html><body><p>Saved reference</p></body></html>").expect("share html");
    fs::write(resolved.inbox.join("3-Attachment.sketch"), b"binary-sketch-archive").expect("share unsupported");

    // 3. App returns to foreground and ingests Inbox.
    let ingest_report = ingest_inbox(&resolved.inbox, &mut session);
    assert_eq!(ingest_report.notices.len(), 3, "{:?}", ingest_report.notices);
    let opened = ingest_report.last_opened.expect("opened imported page");
    assert_eq!(fs::read_dir(&resolved.inbox).expect("read inbox").count(), 0, "Inbox must be emptied after ingest");

    let (opened_fmt, opened_title, opened_body) = load_page_via_engine(&mut session, &opened);
    assert_eq!(opened_fmt, PageFormat::Html);
    assert_eq!(opened_title, "2-Clipping");
    assert_eq!(opened_body, "<p>Saved reference</p>");

    let failed_dir = resolved.inbox.parent().expect("Documents").join("Inbox-Failed");
    assert_eq!(fs::read(failed_dir.join("3-Attachment.sketch")).expect("quarantined sketch"), b"binary-sketch-archive");

    // 4. Keyboard lifecycle while editing the newly opened note on M1 iPad Air:
    //    docked portrait -> floating mid-screen -> docked landscape -> hardware keyboard / hidden.
    let safe_bottom = 20.0;
    let docked_portrait = compute_keyboard_bottom_inset(1180.0, 866.0, 314.0, safe_bottom);
    assert_close(docked_portrait, 314.0, "S5 docked portrait");

    let floating_mid = compute_keyboard_bottom_inset(1180.0, 420.0, 280.0, safe_bottom);
    assert_close(floating_mid, safe_bottom, "S5 floating mid-screen");

    let docked_landscape = compute_keyboard_bottom_inset(820.0, 467.0, 353.0, safe_bottom);
    assert_close(docked_landscape, 353.0, "S5 docked landscape");

    let hidden = compute_keyboard_bottom_inset(820.0, 820.0, 353.0, safe_bottom);
    assert_close(hidden, safe_bottom, "S5 hidden keyboard");

    // 5. Runtime logs accumulate and overwrite the diagnostics export cleanly on background/exit.
    fs::write(
        &active_log,
        "2026-11-17 12:10:00 INFO window ready\n2026-11-17 12:10:05 INFO inbox: Imported 2-Clipping\n2026-11-17 12:10:10 INFO saved page\n",
    )
    .expect("write runtime log");
    let final_diag = export_diagnostics_log(&active_log, &resolved.notebooks).expect("overwrite diagnostics");
    assert_eq!(final_diag, diag_path);
    let final_text = fs::read_to_string(&final_diag).expect("read final diag");
    assert!(!final_text.contains("initialized"), "stale placeholder must be overwritten");
    assert!(final_text.contains("INFO saved page"));
    assert_no_tmp_files(&resolved.notebooks);
}

// ============================================================================
// Static Audit Helpers
// ============================================================================

fn collect_rs_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())).filter_map(Result::ok).map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_rs_files_recursive(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn is_module_path_ios_gated(src_dir: &Path, rel_path: &Path) -> bool {
    let mut parts: Vec<String> = rel_path.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let Some(file) = parts.pop() else {
        return false;
    };
    let stem = Path::new(&file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if parts.is_empty() && (stem == "lib" || stem == "main") {
        return false;
    }
    if stem != "mod" {
        parts.push(stem);
    }

    let mut parent_file = src_dir.join("lib.rs");
    for (depth, mod_name) in parts.iter().enumerate() {
        let Ok(parent_src) = fs::read_to_string(&parent_file) else {
            return false;
        };
        if parse_ios_gated_modules(&parent_src).iter().any(|m| m == mod_name) {
            return true;
        }
        if depth + 1 < parts.len() {
            let sub_dir: PathBuf = parts[..=depth].iter().collect();
            let mod_rs = src_dir.join(&sub_dir).join("mod.rs");
            let flat_rs = src_dir.join(format!("{}.rs", sub_dir.display()));
            parent_file = if mod_rs.is_file() { mod_rs } else { flat_rs };
        }
    }
    false
}

fn audit_ios_source_file(rel_path: &str, raw: &str, mut file_is_ios_gated: bool) -> Result<(), String> {
    let raw_lines: Vec<&str> = raw.lines().collect();
    let clean_with_strings = strip_comments(raw, false);
    let clean_no_strings = strip_comments(raw, true);

    let prod_with_strings = strip_cfg_test_items(&clean_with_strings, &clean_no_strings);
    let prod_no_strings = strip_cfg_test_items(&clean_no_strings, &clean_no_strings);

    if prod_with_strings.iter().zip(prod_no_strings.iter()).any(|(w, n)| is_inner_ios_cfg(w, n)) {
        file_is_ios_gated = true;
    }

    // (i) Zero create_dir / create_dir_all / DirBuilder calls targeting Inbox or layout.inbox in production code.
    let inbox_aliases = collect_inbox_aliases(&prod_with_strings);
    for (stmt_with_str, stmt_no_str) in collect_dir_creation_chunks(&prod_with_strings, &prod_no_strings) {
        if chunk_targets_inbox(&stmt_with_str, &stmt_no_str, &inbox_aliases) {
            return Err(format!("{rel_path} must never create Documents/Inbox: `{}`", stmt_with_str.trim()));
        }
    }

    // (ii) Every allow/expect(unsafe_code) sits in a #[cfg(target_os = "ios")] context
    // with a `// SAFETY:` comment nearby and `MainThreadMarker` in actual non-comment,
    // non-test production lines of the enclosing `#[cfg(target_os = "ios")]` module/file.
    let ios_cfg_scopes = compute_ios_cfg_scope_mask(&clean_with_strings, &clean_no_strings);
    let has_prod_mtm_file = prod_no_strings.iter().any(|l| contains_ident(l, "MainThreadMarker"));

    for (idx, line_no_str) in clean_no_strings.iter().enumerate() {
        if !line_allows_unsafe_code(line_no_str) {
            continue;
        }
        let scope = ios_cfg_scopes[idx];
        if !file_is_ios_gated && scope.is_none() {
            return Err(format!("{rel_path}:{} allows/expects unsafe_code outside #[cfg(target_os = \"ios\")]", idx + 1));
        }

        let window_start = idx.saturating_sub(3);
        let window_end = (idx + 6).min(raw_lines.len());
        let has_safety_comment = raw_lines[window_start..window_end].iter().any(|l| l.contains("// SAFETY:"));
        if !has_safety_comment {
            return Err(format!("{rel_path}:{} allows/expects unsafe_code without a nearby `// SAFETY:` comment", idx + 1));
        }

        let has_mtm_in_scope = if file_is_ios_gated {
            has_prod_mtm_file
        } else if let Some(scope_id) = scope {
            prod_no_strings.iter().enumerate().any(|(j, l)| ios_cfg_scopes[j] == Some(scope_id) && contains_ident(l, "MainThreadMarker"))
        } else {
            false
        };
        if !has_mtm_in_scope {
            return Err(format!(
                "{rel_path}:{} allows/expects unsafe_code without `MainThreadMarker` in production lines of the enclosing #[cfg(target_os = \"ios\")] module/file",
                idx + 1
            ));
        }
    }

    Ok(())
}

fn strip_comments(source: &str, strip_string_bodies: bool) -> Vec<String> {
    let expected_len = source.lines().count();
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut i = 0usize;
    let mut block_depth = 0usize;

    while i < chars.len() {
        let ch = chars[i];
        if ch == '\r' {
            i += 1;
            continue;
        }
        if block_depth > 0 {
            if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
                block_depth += 1;
                i += 2;
            } else if ch == '*' && i + 1 < chars.len() && chars[i + 1] == '/' {
                block_depth -= 1;
                i += 2;
            } else {
                if ch == '\n' {
                    out.push('\n');
                }
                i += 1;
            }
            continue;
        }

        if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            block_depth = 1;
            i += 2;
            continue;
        }

        // Raw string literal: r#"..."#, br#"..."#, cr#"..."#
        let prev_is_ident = i > 0 && (chars[i - 1].is_ascii_alphanumeric() || chars[i - 1] == '_');
        if !prev_is_ident && let Some((prefix_len, hashes)) = parse_raw_string_start(&chars[i..]) {
            for &pc in &chars[i..i + prefix_len] {
                out.push(pc);
            }
            i += prefix_len;
            while i < chars.len() {
                let c = chars[i];
                if c == '\r' {
                    i += 1;
                    continue;
                }
                if c == '"' && i + hashes < chars.len() && chars[i + 1..=i + hashes].iter().all(|&h| h == '#') {
                    out.push('"');
                    for _ in 0..hashes {
                        out.push('#');
                    }
                    i += 1 + hashes;
                    break;
                }
                if c == '\n' || !strip_string_bodies {
                    out.push(c);
                }
                i += 1;
            }
            continue;
        }

        // Normal string literal "..."
        if ch == '"' {
            out.push('"');
            i += 1;
            while i < chars.len() {
                let c = chars[i];
                if c == '\r' {
                    i += 1;
                    continue;
                }
                if c == '\\' && i + 1 < chars.len() {
                    if !strip_string_bodies {
                        out.push('\\');
                        out.push(chars[i + 1]);
                    }
                    i += 2;
                    continue;
                }
                if c == '"' {
                    out.push('"');
                    i += 1;
                    break;
                }
                if c == '\n' || !strip_string_bodies {
                    out.push(c);
                }
                i += 1;
            }
            continue;
        }

        // Char literal 'x' / '\x' (do not treat lifetimes like 'a or 'static as char literals)
        if ch == '\''
            && let Some(lit_len) = parse_char_literal_len(&chars[i..])
        {
            if strip_string_bodies {
                out.push_str("' '");
            } else {
                for &lc in &chars[i..i + lit_len] {
                    out.push(lc);
                }
            }
            i += lit_len;
            continue;
        }

        out.push(ch);
        i += 1;
    }

    let mut lines: Vec<String> = out.split('\n').map(str::to_string).collect();
    lines.resize(expected_len, String::new());
    lines
}

fn parse_raw_string_start(slice: &[char]) -> Option<(usize, usize)> {
    let offset = if slice.starts_with(&['b', 'r']) || slice.starts_with(&['c', 'r']) {
        2
    } else if slice.starts_with(&['r']) {
        1
    } else {
        return None;
    };
    let mut hashes = 0usize;
    while offset + hashes < slice.len() && slice[offset + hashes] == '#' {
        hashes += 1;
    }
    if offset + hashes < slice.len() && slice[offset + hashes] == '"' { Some((offset + hashes + 1, hashes)) } else { None }
}

fn parse_char_literal_len(slice: &[char]) -> Option<usize> {
    if slice.len() < 3 || slice[0] != '\'' {
        return None;
    }
    if slice[1] == '\\' {
        let mut idx = 2usize;
        while idx < slice.len() && slice[idx] != '\n' && idx <= 12 {
            if slice[idx] == '\'' {
                return Some(idx + 1);
            }
            idx += 1;
        }
        None
    } else if slice[1] != '\'' && slice[1] != '\n' && slice[2] == '\'' {
        Some(3)
    } else {
        None
    }
}

fn is_outer_ios_cfg(with_str: &str, no_str: &str) -> bool {
    no_str.contains("#[cfg(target_os =") && with_str.contains("#[cfg(target_os = \"ios\")]")
}

fn is_inner_ios_cfg(with_str: &str, no_str: &str) -> bool {
    no_str.contains("#![cfg(target_os =") && with_str.contains("#![cfg(target_os = \"ios\")]")
}

fn strip_leading_outer_attrs(mut code: &str) -> &str {
    loop {
        let trimmed = code.trim_start();
        let Some(rest) = trimmed.strip_prefix("#[") else {
            return trimmed;
        };
        let mut depth = 1usize;
        let mut end = None;
        for (idx, ch) in rest.char_indices() {
            match ch {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(idx);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close_idx) = end else {
            return trimmed;
        };
        code = &rest[close_idx + 1..];
    }
}

fn parse_ios_gated_modules(lib_src: &str) -> Vec<String> {
    let clean_with_strings = strip_comments(lib_src, false);
    let clean_no_strings = strip_comments(lib_src, true);
    let mut modules = Vec::new();
    let mut pending_ios_cfg = false;

    for (raw_with, raw_no) in clean_with_strings.iter().zip(clean_no_strings.iter()) {
        let with_str = raw_with.trim();
        let no_str = raw_no.trim();
        if no_str.is_empty() {
            continue;
        }

        let mut candidate = no_str;
        if is_outer_ios_cfg(with_str, no_str) {
            pending_ios_cfg = true;
            if let Some((_, tail)) = no_str.split_once("#[cfg(target_os =")
                && let Some((_, after_attr)) = tail.split_once(")]")
            {
                candidate = after_attr.trim();
            } else {
                continue;
            }
        }

        if !pending_ios_cfg {
            continue;
        }
        let item = strip_leading_outer_attrs(candidate);
        if item.is_empty() {
            continue;
        }
        let mod_tail = item
            .strip_prefix("pub(crate) mod ")
            .or_else(|| item.strip_prefix("pub(super) mod "))
            .or_else(|| item.strip_prefix("pub mod "))
            .or_else(|| item.strip_prefix("mod "));
        if let Some(tail) = mod_tail
            && let Some(name) = tail.split([';', ' ', '{']).next()
            && !name.is_empty()
        {
            modules.push(name.to_string());
        }
        pending_ios_cfg = false;
    }
    modules
}

fn strip_cfg_test_items(lines: &[String], no_str_lines: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0usize;
    while i < lines.len() {
        let trimmed = no_str_lines[i].trim();
        if trimmed.contains("#[cfg(test)]") || trimmed.starts_with("#[cfg(all(test,") {
            let mut brace_depth = 0isize;
            let mut bracket_depth = 0isize;
            let mut paren_depth = 0isize;
            let mut saw_brace = false;
            while i < lines.len() {
                out.push(String::new());
                let mut done = false;
                for ch in no_str_lines[i].chars() {
                    match ch {
                        '{' => {
                            brace_depth += 1;
                            saw_brace = true;
                        }
                        '}' => brace_depth -= 1,
                        '[' => bracket_depth += 1,
                        ']' => bracket_depth -= 1,
                        '(' => paren_depth += 1,
                        ')' => paren_depth -= 1,
                        ';' if !saw_brace && brace_depth == 0 && bracket_depth == 0 && paren_depth == 0 => {
                            done = true;
                        }
                        _ => {}
                    }
                }
                i += 1;
                if done || (saw_brace && brace_depth <= 0) {
                    break;
                }
            }
            continue;
        }
        out.push(lines[i].clone());
        i += 1;
    }
    out
}

fn split_code_chunks(with_str_lines: &[String]) -> Vec<(String, String)> {
    let joined = with_str_lines.join("\n");
    let chars: Vec<char> = joined.chars().collect();
    let mut chunks = Vec::new();
    let mut cur_w = String::new();
    let mut cur_n = String::new();
    let mut i = 0usize;

    while i < chars.len() {
        let ch = chars[i];
        let prev_is_ident = i > 0 && (chars[i - 1].is_ascii_alphanumeric() || chars[i - 1] == '_');
        if !prev_is_ident && let Some((prefix_len, hashes)) = parse_raw_string_start(&chars[i..]) {
            for &pc in &chars[i..i + prefix_len] {
                cur_w.push(pc);
                cur_n.push(pc);
            }
            i += prefix_len;
            while i < chars.len() {
                let c = chars[i];
                if c == '"' && i + hashes < chars.len() && chars[i + 1..=i + hashes].iter().all(|&h| h == '#') {
                    cur_w.push('"');
                    cur_n.push('"');
                    for _ in 0..hashes {
                        cur_w.push('#');
                        cur_n.push('#');
                    }
                    i += 1 + hashes;
                    break;
                }
                cur_w.push(if c == '\n' { ' ' } else { c });
                i += 1;
            }
            continue;
        }

        if ch == '"' {
            cur_w.push('"');
            cur_n.push('"');
            i += 1;
            while i < chars.len() {
                let c = chars[i];
                if c == '\\' && i + 1 < chars.len() {
                    cur_w.push('\\');
                    cur_w.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                if c == '"' {
                    cur_w.push('"');
                    cur_n.push('"');
                    i += 1;
                    break;
                }
                cur_w.push(if c == '\n' { ' ' } else { c });
                i += 1;
            }
            continue;
        }

        if ch == '\''
            && let Some(lit_len) = parse_char_literal_len(&chars[i..])
        {
            for &lc in &chars[i..i + lit_len] {
                cur_w.push(lc);
            }
            cur_n.push_str("' '");
            i += lit_len;
            continue;
        }

        if matches!(ch, ';' | '{' | '}') {
            if !cur_n.trim().is_empty() {
                chunks.push((std::mem::take(&mut cur_w), std::mem::take(&mut cur_n)));
            } else {
                cur_w.clear();
                cur_n.clear();
            }
        } else {
            let pushed = if ch == '\n' { ' ' } else { ch };
            cur_w.push(pushed);
            cur_n.push(pushed);
        }
        i += 1;
    }

    if !cur_n.trim().is_empty() {
        chunks.push((cur_w, cur_n));
    }
    chunks
}

fn collect_inbox_aliases(with_str_lines: &[String]) -> Vec<String> {
    let mut aliases: Vec<String> = Vec::new();
    let chunks = split_code_chunks(with_str_lines);
    for _pass in 0..2 {
        for (chunk_w, chunk_n) in &chunks {
            let trimmed_n = chunk_n.trim();
            let Some(after_let) = trimmed_n.strip_prefix("let ") else {
                continue;
            };
            let after_mut = after_let.trim_start().strip_prefix("mut ").unwrap_or(after_let).trim_start();
            let Some(var_name) = after_mut.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).next() else {
                continue;
            };
            if var_name.is_empty() || aliases.iter().any(|a| a == var_name) {
                continue;
            }
            let Some((_, rhs_w)) = chunk_w.split_once('=') else {
                continue;
            };
            let Some((_, rhs_n)) = trimmed_n.split_once('=') else {
                continue;
            };
            let rhs_n_trim = rhs_n.trim();
            let neutralized = rhs_n_trim.contains(".parent()") || rhs_n_trim.contains("quarantine_dir") || rhs_n_trim.contains("FAILED_FOLDER");
            if neutralized {
                continue;
            }
            let targets_inbox = rhs_w.contains("\"Inbox\"")
                || contains_ident(rhs_n_trim, "INBOX_FOLDER")
                || contains_field_inbox(rhs_n_trim)
                || rhs_n_trim == "inbox"
                || rhs_n_trim == "&inbox"
                || aliases.iter().any(|a| contains_ident(rhs_n_trim, a));
            if targets_inbox {
                aliases.push(var_name.to_string());
            }
        }
    }
    aliases
}

fn collect_dir_creation_chunks(with_str_lines: &[String], no_str_lines: &[String]) -> Vec<(String, String)> {
    let uses_dir_builder = no_str_lines.iter().any(|l| contains_ident(l, "DirBuilder"));
    split_code_chunks(with_str_lines)
        .into_iter()
        .filter(|(_, chunk_n)| {
            chunk_n.contains("create_dir(")
                || chunk_n.contains("create_dir_all(")
                || (uses_dir_builder && (chunk_n.contains(".create(") || contains_ident(chunk_n, "DirBuilder")))
        })
        .collect()
}

fn chunk_targets_inbox(chunk_w: &str, chunk_n: &str, inbox_aliases: &[String]) -> bool {
    contains_field_inbox(chunk_n)
        || chunk_n.contains("(&inbox")
        || chunk_n.contains("(inbox")
        || contains_ident(chunk_n, "INBOX_FOLDER")
        || chunk_w.contains("\"Inbox\"")
        || inbox_aliases.iter().any(|alias| contains_ident(chunk_n, alias))
}

fn contains_field_inbox(text: &str) -> bool {
    let bytes = text.as_bytes();
    let is_ident_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = 0usize;
    while let Some(pos) = text[start..].find(".inbox") {
        let after = start + pos + ".inbox".len();
        if after == bytes.len() || !is_ident_byte(bytes[after]) {
            return true;
        }
        start += pos + 1;
    }
    false
}

fn contains_ident(text: &str, ident: &str) -> bool {
    let bytes = text.as_bytes();
    let is_ident_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = 0usize;
    while let Some(pos) = text[start..].find(ident) {
        let abs = start + pos;
        let before_ok = abs == 0 || !is_ident_byte(bytes[abs - 1]);
        let after = abs + ident.len();
        let after_ok = after == bytes.len() || !is_ident_byte(bytes[after]);
        if before_ok && after_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

fn line_allows_unsafe_code(no_str_line: &str) -> bool {
    let trimmed = no_str_line.trim();
    if !(trimmed.contains("#[") || trimmed.contains("#![")) {
        return false;
    }
    for kw in ["allow(", "expect("] {
        let mut start = 0usize;
        while let Some(pos) = trimmed[start..].find(kw) {
            let body_start = start + pos + kw.len();
            let mut depth = 1usize;
            for (rel_idx, ch) in trimmed[body_start..].char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            if contains_ident(&trimmed[body_start..body_start + rel_idx], "unsafe_code") {
                                return true;
                            }
                            break;
                        }
                    }
                    _ => {}
                }
            }
            start = body_start;
        }
    }
    false
}

fn compute_ios_cfg_scope_mask(with_str_lines: &[String], no_str_lines: &[String]) -> Vec<Option<usize>> {
    let mut mask = vec![None; no_str_lines.len()];
    let mut next_scope_id = 1usize;
    let mut pending_scope: Option<usize> = None;
    let mut active_scope: Option<(usize, isize)> = None;
    let mut brace_depth = 0isize;

    for (idx, (raw_with, raw_no)) in with_str_lines.iter().zip(no_str_lines.iter()).enumerate() {
        let with_str = raw_with.trim();
        let no_str = raw_no.trim();
        if no_str.is_empty() {
            mask[idx] = active_scope.map(|(id, _)| id).or(pending_scope);
            continue;
        }

        let mut scan_str = no_str;
        if is_outer_ios_cfg(with_str, no_str) {
            let id = next_scope_id;
            next_scope_id += 1;
            pending_scope = Some(id);
            if let Some((_, tail)) = no_str.split_once("#[cfg(target_os =")
                && let Some((_, after_attr)) = tail.split_once(")]")
            {
                scan_str = after_attr.trim();
            } else {
                scan_str = "";
            }
        }

        mask[idx] = active_scope.map(|(id, _)| id).or(pending_scope);

        let mut line_opened_scope = false;
        for ch in scan_str.chars() {
            if ch == '{' {
                if let Some(id) = pending_scope
                    && active_scope.is_none()
                {
                    active_scope = Some((id, brace_depth));
                    pending_scope = None;
                    line_opened_scope = true;
                }
                brace_depth += 1;
            } else if ch == '}' {
                brace_depth -= 1;
                if let Some((_, target_depth)) = active_scope
                    && brace_depth <= target_depth
                {
                    active_scope = None;
                }
            }
        }

        let item_tail = strip_leading_outer_attrs(scan_str);
        if pending_scope.is_some() && !line_opened_scope && !item_tail.is_empty() && item_tail.ends_with(';') {
            pending_scope = None;
        }
    }
    mask
}
