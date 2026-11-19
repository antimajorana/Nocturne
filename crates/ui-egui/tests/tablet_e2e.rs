//! Opaque-box E2E tests for Nocturne iPadOS/tablet input, tool toggles,
//! docked mobile formatting bar, and Windows desktop parity (Tiers 1-4).
//!
//! Covers F1-F6 from `TEST_INFRA.md` using only public APIs (`NocturneApp`,
//! `ControlRequest`, `egui_kittest::Harness`, `Stylus`, `touch_ux`,
//! `nocturne_engine::Session`, and `support::clipboard::MemoryBoard`).

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use nocturne_engine::Session;
use nocturne_engine::sample::{Sample, create_sample};
use nocturne_store::Settings;
use nocturne_store::testing::TempDir;
use nocturne_ui_egui::control::ControlRequest;
use nocturne_ui_egui::stylus::Stylus;
use nocturne_ui_egui::theme::{Accent, Mode, Theme};
use nocturne_ui_egui::touch_ux::{TABLET_MIN_INTERACT_HEIGHT, compute_multitouch_canvas_step};
use nocturne_ui_egui::{LOGO_PNG, NocturneApp};
use serde_json::{Value, json};
use std::time::Duration;

mod support;
use support::clipboard::MemoryBoard;

const LIGHT: Theme = Theme { mode: Mode::Light, accent: Accent::Default };
const KEYBOARD_INSET_BOTTOM: f32 = 320.0;

fn session(t: &TempDir) -> Session {
    let settings = Settings::at(t.path().join("settings.toml"));
    settings.set("privacy_welcome_seen", "true").expect("test settings");
    settings.set("split_panes", "false").expect("test navigation setting");
    settings.set("pressure_unavailable_never", "true").expect("test pressure setting");
    Session::new(&t.path().join("notebooks"), settings).expect("session")
}

fn sample_app(label: &str) -> (TempDir, NocturneApp, Sample) {
    let t = TempDir::new(label);
    let mut s = session(&t);
    let sample = create_sample(&mut s, Some(LOGO_PNG)).expect("sample");
    let mut app = NocturneApp::new(s);
    app.set_redact_paths(true);
    (t, app, sample)
}

fn sample_app_with_board(label: &str, board: &MemoryBoard) -> (TempDir, NocturneApp, Sample) {
    let (t, app, sample) = sample_app(label);
    let app = app.with_clipboard(Box::new(board.clone()));
    (t, app, sample)
}

fn sized_harness(mut app: NocturneApp, theme: Theme, size: egui::Vec2) -> Harness<'static, NocturneApp> {
    app.set_theme(theme);
    Harness::builder()
        .with_size(size)
        .with_accessibility_check(false)
        .renderer(support::gpu::renderer())
        .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app)
}

fn desktop_harness(app: NocturneApp) -> Harness<'static, NocturneApp> {
    sized_harness(app, LIGHT, egui::vec2(1500.0, 900.0))
}

fn tablet_harness(app: NocturneApp) -> Harness<'static, NocturneApp> {
    sized_harness(app, LIGHT, egui::vec2(1024.0, 768.0))
}

fn ipad_portrait_harness(app: NocturneApp) -> Harness<'static, NocturneApp> {
    sized_harness(app, LIGHT, egui::vec2(820.0, 1180.0))
}

fn pump(h: &mut Harness<'_, NocturneApp>) {
    let events = h.state_mut().take_events();
    h.input_mut().events.extend(events);
    h.step();
}

fn settle(h: &mut Harness<'_, NocturneApp>) {
    const BACKGROUND: Duration = Duration::from_millis(200);
    let deadline = std::time::Instant::now() + BACKGROUND;
    for _ in 0..16 {
        pump(h);
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if !left.is_zero() {
            std::thread::sleep(left.min(Duration::from_millis(12)));
        }
    }
    pump(h);
}

fn ask(h: &mut Harness<'_, NocturneApp>, method: &str, p: Value) -> Result<Value, String> {
    let (req, rx) = ControlRequest::new(method, p);
    let ctx = h.ctx.clone();
    h.state_mut().handle_control(&ctx, req);
    pump(h);
    rx.try_recv().unwrap_or_else(|_| Err("no answer".into()))
}

fn inspect(h: &mut Harness<'_, NocturneApp>) -> Value {
    ask(h, "ui.inspect", json!({})).expect("ui.inspect")
}

fn page_origin(h: &mut Harness<'_, NocturneApp>) -> egui::Pos2 {
    let v = inspect(h);
    let origin = v.pointer("/page/origin").and_then(Value::as_array).cloned().unwrap_or_default();
    let at = |i: usize| origin.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    egui::pos2(at(0), at(1))
}

fn page_of(h: &mut Harness<'_, NocturneApp>, s: &Sample, title: &str, body: &str) -> String {
    let section = s.md_page.parent().expect("section").to_string_lossy().into_owned();
    let made =
        ask(h, "engine.execute", json!({"command": "node.create", "params": {"parent": section, "kind": "page", "title": title, "format": "cmgfm"}}))
            .expect("create page");
    let page = made.as_str().expect("page path").to_string();
    ask(h, "engine.execute", json!({"command": "page.save", "params": {"path": &page, "body": body}})).expect("save page");
    ask(h, "ui.open", json!({"path": &page})).expect("open page");
    settle(h);
    page
}

fn readonly_html_page(h: &mut Harness<'_, NocturneApp>, s: &Sample, title: &str) -> String {
    let section = s.html_page.parent().expect("section").to_string_lossy().into_owned();
    let made =
        ask(h, "engine.execute", json!({"command": "node.create", "params": {"parent": section, "kind": "page", "title": title, "format": "html"}}))
            .expect("create html page");
    let page = made.as_str().expect("html page path").to_string();
    ask(h, "engine.execute", json!({"command": "page.save", "params": {"path": &page, "body": "<p>Keep me</p><aside data-x=\"1\">side</aside>"}}))
        .expect("save readonly html");
    ask(h, "ui.open", json!({"path": &page})).expect("open readonly html");
    settle(h);
    page
}

fn ink_pointer(v: &Value) -> String {
    v.pointer("/ink/pointer").and_then(Value::as_str).unwrap_or_default().to_string()
}

fn ink_nib(v: &Value) -> Option<u64> {
    v.pointer("/ink/nib").and_then(Value::as_u64)
}

fn ink_tool(v: &Value) -> String {
    v.pointer("/ink/tool").and_then(Value::as_str).unwrap_or_default().to_string()
}

fn ink_strokes(v: &Value) -> u64 {
    v.pointer("/ink/strokes").and_then(Value::as_u64).unwrap_or(0)
}

fn canvas_offset(v: &Value) -> (f64, f64) {
    let arr = v.pointer("/canvas/viewOffset").and_then(Value::as_array).expect("canvas viewOffset");
    (arr[0].as_f64().unwrap_or(0.0), arr[1].as_f64().unwrap_or(0.0))
}

fn page_origin_vec(v: &Value) -> (f64, f64) {
    let arr = v.pointer("/page/origin").and_then(Value::as_array).expect("page origin");
    (arr[0].as_f64().unwrap_or(0.0), arr[1].as_f64().unwrap_or(0.0))
}

fn require_finger_drawing(v: &Value) -> bool {
    for ptr in ["/ink/fingerDrawing", "/ink/finger_drawing", "/fingerDrawing", "/finger_drawing"] {
        if let Some(b) = v.pointer(ptr).and_then(Value::as_bool) {
            return b;
        }
    }
    panic!("ui.inspect missing finger_drawing field (expected /ink/fingerDrawing or /finger_drawing): {v}");
}

fn require_eraser_active(v: &Value) -> bool {
    for ptr in ["/ink/eraserActive", "/ink/eraser_active", "/eraserActive", "/eraser_active"] {
        if let Some(b) = v.pointer(ptr).and_then(Value::as_bool) {
            return b;
        }
    }
    panic!("ui.inspect missing eraser_active field (expected /ink/eraserActive or /eraser_active): {v}");
}

fn require_formatting_bar_visible(v: &Value) -> bool {
    for ptr in [
        "/formattingBarVisible",
        "/formatting_bar_visible",
        "/page/formattingBarVisible",
        "/page/formatting_bar_visible",
        "/mobileFormattingBarVisible",
        "/mobile_formatting_bar_visible",
    ] {
        if let Some(b) = v.pointer(ptr).and_then(Value::as_bool) {
            return b;
        }
    }
    panic!("ui.inspect missing formatting_bar_visible field: {v}");
}

fn json_zoom_percent(v: &Value) -> Option<u32> {
    for ptr in ["/canvas/zoomPercent", "/canvas/zoom_percent", "/zoomPercent", "/zoom_percent", "/page/zoomPercent", "/page/zoom_percent"] {
        if let Some(z) = v.pointer(ptr).and_then(Value::as_u64) {
            return Some(z as u32);
        }
    }
    None
}

fn set_keyboard_inset(h: &mut Harness<'_, NocturneApp>, bottom: f32) {
    h.input_mut().safe_area_insets = Some(egui::SafeAreaInsets(egui::epaint::MarginF32 { left: 0.0, right: 0.0, top: 0.0, bottom }));
    settle(h);
}

fn find_label_case_insensitive(h: &Harness<'_, NocturneApp>, candidates: &[&str]) -> Option<(String, egui::Rect)> {
    let ppp = h.ctx.pixels_per_point();
    for node in h.query_all(egui_kittest::kittest::by()) {
        let Some(label) = node.accesskit_node().label() else { continue };
        let lower = label.trim().to_ascii_lowercase();
        let matched = candidates.iter().any(|c| {
            let c_low = c.to_ascii_lowercase();
            lower == c_low || lower.starts_with(&format!("{c_low} ("))
        });
        if !matched {
            continue;
        }
        let Some(b) = node.accesskit_node().bounding_box() else { continue };
        let rect = egui::Rect::from_min_max(egui::pos2(b.x0 as f32 / ppp, b.y0 as f32 / ppp), egui::pos2(b.x1 as f32 / ppp, b.y1 as f32 / ppp));
        if rect.is_positive() {
            return Some((label.to_string(), rect));
        }
    }
    None
}

fn click_finger_drawing_toggle(h: &mut Harness<'_, NocturneApp>) {
    let (_, rect) = find_label_case_insensitive(h, &["Finger drawing", "Finger Drawing"]).expect("Finger drawing toggle button must exist in UI");
    let c = rect.center();
    ask(h, "ui.click", json!({"x": c.x, "y": c.y})).expect("click Finger drawing toggle");
    settle(h);
}

fn find_docked_bar_button(h: &Harness<'_, NocturneApp>, candidates: &[&str], keyboard_bottom: f32) -> egui::Rect {
    let ppp = h.ctx.pixels_per_point();
    let viewport_h = h.ctx.viewport_rect().height();
    let keyboard_top = viewport_h - keyboard_bottom;
    let min_bar_top = viewport_h * 0.35;
    let mut found = Vec::new();
    for node in h.query_all(egui_kittest::kittest::by()) {
        let Some(label) = node.accesskit_node().label() else { continue };
        let lower = label.trim().to_ascii_lowercase();
        let matched = candidates.iter().any(|c| {
            let c_low = c.to_ascii_lowercase();
            lower == c_low || lower.starts_with(&format!("{c_low} ("))
        });
        if !matched {
            continue;
        }
        let Some(b) = node.accesskit_node().bounding_box() else { continue };
        let rect = egui::Rect::from_min_max(egui::pos2(b.x0 as f32 / ppp, b.y0 as f32 / ppp), egui::pos2(b.x1 as f32 / ppp, b.y1 as f32 / ppp));
        if rect.is_positive() && rect.center().y >= min_bar_top && rect.max.y <= keyboard_top + 2.0 {
            found.push(rect);
        }
    }
    assert!(!found.is_empty(), "docked formatting bar button matching {candidates:?} not found in region y=[{min_bar_top}, {keyboard_top}]");
    let rect = found.into_iter().max_by(|a, b| a.center().y.total_cmp(&b.center().y)).expect("at least one rect");
    assert!(
        rect.height() >= TABLET_MIN_INTERACT_HEIGHT - 0.5,
        "formatting bar target height must be >= {TABLET_MIN_INTERACT_HEIGHT}pt, got {rect:?}"
    );
    rect
}

fn click_docked_bar_button(h: &mut Harness<'_, NocturneApp>, candidates: &[&str], keyboard_bottom: f32) -> egui::Rect {
    let rect = find_docked_bar_button(h, candidates, keyboard_bottom);
    let c = rect.center();
    ask(h, "ui.click", json!({"x": c.x, "y": c.y})).expect("click docked formatting bar button");
    settle(h);
    rect
}

fn inject_primary_touch_drag(h: &mut Harness<'_, NocturneApp>, id: u64, from: egui::Pos2, to: egui::Pos2, steps: usize, force: Option<f32>) {
    let contact = |phase, pos| egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(id), phase, pos, force };
    h.input_mut().events.push(contact(egui::TouchPhase::Start, from));
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    let total = steps.max(2);
    for i in 1..=total {
        let t = i as f32 / total as f32;
        let pos = from.lerp(to, t);
        h.input_mut().events.push(contact(egui::TouchPhase::Move, pos));
        h.input_mut().events.push(egui::Event::PointerMoved(pos));
        h.step();
    }
    h.input_mut().events.push(contact(egui::TouchPhase::End, to));
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    settle(h);
}

fn inject_primary_touch_tap(h: &mut Harness<'_, NocturneApp>, id: u64, at: egui::Pos2, force: Option<f32>) {
    let contact = |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(id), phase, pos: at, force };
    h.input_mut().events.push(contact(egui::TouchPhase::Start));
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    h.input_mut().events.push(contact(egui::TouchPhase::End));
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    settle(h);
}

fn inject_mouse_drag(h: &mut Harness<'_, NocturneApp>, from: egui::Pos2, to: egui::Pos2, button: &str) {
    ask(h, "ui.press", json!({"x": from.x, "y": from.y, "button": button})).expect("mouse press");
    pump(h);
    let mid = from.lerp(to, 0.5);
    ask(h, "ui.move", json!({"x": mid.x, "y": mid.y})).expect("mouse move mid");
    pump(h);
    ask(h, "ui.move", json!({"x": to.x, "y": to.y})).expect("mouse move end");
    pump(h);
    ask(h, "ui.release", json!({"x": to.x, "y": to.y, "button": button})).expect("mouse release");
    settle(h);
}

fn forward_copies(h: &mut Harness<'_, NocturneApp>, board: &MemoryBoard) {
    for command in &h.output().platform_output.commands {
        if let egui::OutputCommand::CopyText(text) = command {
            board.put_text(text);
        }
    }
}

fn press_copy(h: &mut Harness<'_, NocturneApp>, board: &MemoryBoard, key: &str) {
    h.input_mut().events.push(egui::Event::Copy);
    ask(h, "ui.key", json!({"key": key, "ctrl": true})).expect("ctrl copy key");
    forward_copies(h, board);
    for _ in 0..8 {
        pump(h);
        forward_copies(h, board);
    }
}

fn scrollable_body() -> String {
    (1..=45).map(|i| format!("Paragraph line number {i:02} for vertical touch scroll verification.")).collect::<Vec<_>>().join("\n\n")
}

// ============================================================================
// F1: UI Pen/Eraser Toggle (Tier 1 >= 5, Tier 2 >= 5)
// ============================================================================

#[test]
fn f1_t1_engine_toggle_eraser_syncs_ui_pointer_to_erase_and_back() {
    let (_t, mut app, s) = sample_app("f1-t1-sync");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);

    h.get_by_label("Draw").click();
    settle(&mut h);
    h.get_by_label("Pen red").click();
    settle(&mut h);
    let red_nib = ink_nib(&inspect(&mut h));

    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("toggle to eraser");
    settle(&mut h);
    let erased = inspect(&mut h);
    assert_eq!(ink_pointer(&erased), "erase", "ink.toggle_eraser must put NocturneApp into erase mode: {erased}");
    assert!(require_eraser_active(&erased), "eraser_active must be true");

    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("toggle back to pen");
    settle(&mut h);
    let restored = inspect(&mut h);
    assert_eq!(ink_pointer(&restored), "draw", "second toggle must restore draw mode: {restored}");
    assert!(!require_eraser_active(&restored), "eraser_active must be false");
    assert_eq!(ink_nib(&restored), red_nib, "previous red pen nib must be restored");
}

#[test]
fn f1_t1_toggle_eraser_preserves_pen_highlighter_and_pencil_nibs() {
    let (_t, mut app, s) = sample_app("f1-t1-nibs");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);
    h.get_by_label("Draw").click();
    settle(&mut h);

    for (label, expected_tool) in [("Pen blue", "pen"), ("Highlighter yellow", "highlighter"), ("Pencil grey", "pencil")] {
        h.get_by_label(label).click();
        settle(&mut h);
        let before = inspect(&mut h);
        let nib = ink_nib(&before);
        assert_eq!(ink_tool(&before), expected_tool);

        ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("to eraser");
        settle(&mut h);
        assert_eq!(ink_pointer(&inspect(&mut h)), "erase");

        ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("restore tool");
        settle(&mut h);
        let after = inspect(&mut h);
        assert_eq!(ink_pointer(&after), "draw");
        assert_eq!(ink_nib(&after), nib, "{label} nib must be restored");
        assert_eq!(ink_tool(&after), expected_tool, "{label} tool kind must be restored");
    }
}

#[test]
fn f1_t1_ui_eraser_button_and_command_toggle_round_trip() {
    let (_t, mut app, s) = sample_app("f1-t1-ui-btn");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);

    h.get_by_label("Draw").click();
    settle(&mut h);
    h.get_by_label("Highlighter mint").click();
    settle(&mut h);
    let mint_nib = ink_nib(&inspect(&mut h));

    h.get_by_label("Eraser").click();
    settle(&mut h);
    let erased = inspect(&mut h);
    assert_eq!(ink_pointer(&erased), "erase");
    assert!(require_eraser_active(&erased));

    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("toggle back from UI eraser");
    settle(&mut h);
    let restored = inspect(&mut h);
    assert_eq!(ink_pointer(&restored), "draw", "toggling after clicking UI Eraser must restore Highlighter mint: {restored}");
    assert_eq!(ink_nib(&restored), mint_nib);
}

#[test]
fn f1_t1_eraser_erases_drawn_ink_and_toggle_back_draws_again() {
    let (_t, mut app, s) = sample_app("f1-t1-erase-stroke");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);
    h.get_by_label("Draw").click();
    settle(&mut h);
    h.get_by_label("Pen black").click();
    settle(&mut h);

    let origin = page_origin(&mut h);
    let p1 = origin + egui::vec2(460.0, 220.0);
    let p2 = origin + egui::vec2(580.0, 220.0);
    inject_primary_touch_drag(&mut h, 1, p1, p2, 6, Some(0.6));
    assert_eq!(ink_strokes(&inspect(&mut h)), 1, "stylus draws initial stroke");

    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("enter eraser");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "erase");

    inject_primary_touch_drag(&mut h, 2, origin + egui::vec2(520.0, 200.0), origin + egui::vec2(520.0, 240.0), 6, Some(0.6));
    assert_eq!(ink_strokes(&inspect(&mut h)), 0, "eraser stroke removes the drawn stroke");

    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("restore pen");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "draw");
    inject_primary_touch_drag(&mut h, 3, p1 + egui::vec2(0.0, 40.0), p2 + egui::vec2(0.0, 40.0), 6, Some(0.6));
    assert_eq!(ink_strokes(&inspect(&mut h)), 1, "restored pen draws again");
}

#[test]
fn f1_t1_explicit_mode_eraser_and_pen_via_engine_execute() {
    let (_t, mut app, s) = sample_app("f1-t1-explicit-mode");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);
    h.get_by_label("Draw").click();
    settle(&mut h);
    h.get_by_label("Pen green").click();
    settle(&mut h);
    let green_nib = ink_nib(&inspect(&mut h));

    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser", "params": {"mode": "eraser"}})).expect("mode=eraser");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "erase");

    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser", "params": {"mode": "eraser"}})).expect("idempotent mode=eraser");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "erase");

    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser", "params": {"mode": "pen"}})).expect("mode=pen");
    settle(&mut h);
    let back = inspect(&mut h);
    assert_eq!(ink_pointer(&back), "draw");
    assert_eq!(ink_nib(&back), green_nib);
}

#[test]
fn f1_t2_toggle_eraser_from_write_lasso_pick_and_pan_modes() {
    let (_t, mut app, s) = sample_app("f1-t2-other-modes");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);
    h.get_by_label("Draw").click();
    settle(&mut h);
    h.get_by_label("Pen red").click();
    settle(&mut h);
    let red_nib = ink_nib(&inspect(&mut h));

    for (label, expected) in [("Type", "write"), ("Select", "select"), ("Lasso Select", "lasso"), ("Panning Hand", "pan")] {
        h.get_by_label(label).click();
        settle(&mut h);
        assert_eq!(ink_pointer(&inspect(&mut h)), expected);

        ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("enter erase");
        settle(&mut h);
        assert_eq!(ink_pointer(&inspect(&mut h)), "erase", "must enter erase from {label}");

        ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("restore pen");
        settle(&mut h);
        let back = inspect(&mut h);
        assert_eq!(ink_pointer(&back), "draw", "toggling back from erase must restore draw");
        assert_eq!(ink_nib(&back), red_nib, "previous red nib preserved across {label}");
    }
}

#[test]
fn f1_t2_invalid_mode_param_returns_error_and_leaves_ui_pointer_unchanged() {
    let (_t, mut app, s) = sample_app("f1-t2-bad-param");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);
    h.get_by_label("Draw").click();
    settle(&mut h);
    h.get_by_label("Pen blue").click();
    settle(&mut h);
    let blue_nib = ink_nib(&inspect(&mut h));

    for bad in ["bogus", "write", "123", "true"] {
        let res = ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser", "params": {"mode": bad}}));
        assert!(res.is_err(), "expected error for invalid mode {bad:?}: {res:?}");
        let after = inspect(&mut h);
        assert_eq!(ink_pointer(&after), "draw");
        assert_eq!(ink_nib(&after), blue_nib);
    }
}

#[test]
fn f1_t2_toggle_eraser_with_no_page_open_is_safe_and_persists_when_page_opens() {
    let (_t, app, s) = sample_app("f1-t2-no-page");
    let mut h = desktop_harness(app);
    settle(&mut h);

    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("toggle on welcome");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "erase");

    ask(&mut h, "ui.open", json!({"path": s.md_page.to_string_lossy()})).expect("open page");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "erase");

    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("toggle back");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "draw");
}

#[test]
fn f1_t2_rapid_repeated_eraser_toggles_preserve_nib_and_existing_strokes() {
    let (_t, mut app, s) = sample_app("f1-t2-rapid");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);
    h.get_by_label("Draw").click();
    settle(&mut h);
    h.get_by_label("Highlighter yellow").click();
    settle(&mut h);
    let yellow_nib = ink_nib(&inspect(&mut h));

    let origin = page_origin(&mut h);
    inject_primary_touch_drag(&mut h, 1, origin + egui::vec2(460.0, 220.0), origin + egui::vec2(540.0, 220.0), 4, Some(0.5));
    assert_eq!(ink_strokes(&inspect(&mut h)), 1);

    for step in 0..10 {
        ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("rapid toggle");
        pump(&mut h);
        let cur = inspect(&mut h);
        let expect_erase = step % 2 == 0;
        assert_eq!(ink_pointer(&cur), if expect_erase { "erase" } else { "draw" });
        assert_eq!(ink_strokes(&cur), 1, "toggling alone must not alter stroke count");
    }
    assert_eq!(ink_nib(&inspect(&mut h)), yellow_nib);
}

#[test]
fn f1_t2_toggle_eraser_on_readonly_page_does_not_panic_or_corrupt_page() {
    let (_t, app, s) = sample_app("f1-t2-readonly");
    let mut h = desktop_harness(app);
    settle(&mut h);
    readonly_html_page(&mut h, &s, "ReadOnlyF1");
    let before = h.state().body().unwrap_or_default().to_string();

    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("erase on readonly");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "erase");
    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("pen on readonly");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "draw");
    assert_eq!(h.state().body().unwrap_or_default(), before);
}

// ============================================================================
// F2: Finger-Drawing Toggle (Tier 1 >= 5, Tier 2 >= 5)
// ============================================================================

#[test]
fn f2_t1_finger_drawing_defaults_to_false_in_ui_inspect() {
    let (_t, mut app, s) = sample_app("f2-t1-default");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);

    let state = inspect(&mut h);
    assert!(!require_finger_drawing(&state), "finger_drawing must default to false: {state}");
}

#[test]
fn f2_t1_engine_execute_finger_drawing_toggles_and_syncs_ui() {
    let (_t, mut app, s) = sample_app("f2-t1-engine-sync");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);

    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing"})).expect("toggle on");
    settle(&mut h);
    assert!(require_finger_drawing(&inspect(&mut h)));

    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "false"}})).expect("explicit false");
    settle(&mut h);
    assert!(!require_finger_drawing(&inspect(&mut h)));

    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "true"}})).expect("explicit true");
    settle(&mut h);
    assert!(require_finger_drawing(&inspect(&mut h)));
}

#[test]
fn f2_t1_ui_finger_drawing_toolbar_toggle_switches_state() {
    let (_t, mut app, s) = sample_app("f2-t1-ui-toggle");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);

    h.get_by_label("Draw").click();
    settle(&mut h);
    assert!(!require_finger_drawing(&inspect(&mut h)));

    click_finger_drawing_toggle(&mut h);
    assert!(require_finger_drawing(&inspect(&mut h)), "UI toggle must enable finger_drawing");

    click_finger_drawing_toggle(&mut h);
    assert!(!require_finger_drawing(&inspect(&mut h)), "second UI toggle click must disable finger_drawing");
}

#[test]
fn f2_t1_enabling_finger_drawing_switches_1finger_touch_from_pan_to_draw() {
    let (_t, app, s) = sample_app("f2-t1-pan-vs-draw");
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "FingerDrawSwitch", &scrollable_body());

    let origin = page_origin(&mut h);
    let start = origin + egui::vec2(500.0, 260.0);
    let end = origin + egui::vec2(500.0, 140.0);

    // Default: finger_drawing == false -> 1-finger drag scrolls/pans, 0 strokes.
    assert!(!require_finger_drawing(&inspect(&mut h)));
    let before = inspect(&mut h);
    inject_primary_touch_drag(&mut h, 1, start, end, 6, None);
    let after_pan = inspect(&mut h);
    assert_eq!(ink_strokes(&after_pan), 0, "1-finger touch must not draw when finger_drawing is false");
    assert_ne!(
        (page_origin_vec(&before), canvas_offset(&before)),
        (page_origin_vec(&after_pan), canvas_offset(&after_pan)),
        "1-finger touch must scroll/pan when finger_drawing is false"
    );

    // Enable finger_drawing -> 1-finger drag draws ink.
    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "true"}})).expect("enable finger drawing");
    settle(&mut h);
    assert!(require_finger_drawing(&inspect(&mut h)));
    inject_primary_touch_drag(&mut h, 2, origin + egui::vec2(480.0, 200.0), origin + egui::vec2(580.0, 240.0), 6, None);
    assert_eq!(ink_strokes(&inspect(&mut h)), 1, "1-finger touch must draw when finger_drawing is true");
}

#[test]
fn f2_t1_disabling_finger_drawing_restores_1finger_scroll_without_losing_strokes() {
    let (_t, app, s) = sample_app("f2-t1-disable-keep");
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "KeepStrokes", &scrollable_body());

    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "true"}})).expect("on");
    settle(&mut h);
    let origin = page_origin(&mut h);
    inject_primary_touch_drag(&mut h, 1, origin + egui::vec2(460.0, 200.0), origin + egui::vec2(560.0, 220.0), 6, None);
    assert_eq!(ink_strokes(&inspect(&mut h)), 1);

    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "false"}})).expect("off");
    settle(&mut h);
    let before = inspect(&mut h);
    inject_primary_touch_drag(&mut h, 2, origin + egui::vec2(500.0, 280.0), origin + egui::vec2(500.0, 150.0), 6, None);
    let after = inspect(&mut h);
    assert_eq!(ink_strokes(&after), 1, "existing stroke preserved and no second stroke drawn");
    assert_ne!(
        (page_origin_vec(&before), canvas_offset(&before)),
        (page_origin_vec(&after), canvas_offset(&after)),
        "1-finger drag scrolls/pans again"
    );
}

#[test]
fn f2_t2_invalid_finger_drawing_param_returns_error_without_mutating_state() {
    let (_t, mut app, s) = sample_app("f2-t2-bad-param");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);

    for bad in ["yes", "no", "1", "0", "maybe"] {
        let r = ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": bad}}));
        assert!(r.is_err(), "expected error for {bad:?}");
        assert!(!require_finger_drawing(&inspect(&mut h)));
    }
}

#[test]
fn f2_t2_finger_drawing_setting_survives_page_switch_and_ribbon_tab_change() {
    let (_t, mut app, s) = sample_app("f2-t2-persist-session");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);

    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "true"}})).expect("enable");
    settle(&mut h);

    h.get_by_label("View").click();
    settle(&mut h);
    assert!(require_finger_drawing(&inspect(&mut h)));

    ask(&mut h, "ui.open", json!({"path": s.html_page.to_string_lossy()})).expect("switch page");
    settle(&mut h);
    assert!(require_finger_drawing(&inspect(&mut h)), "finger_drawing must stay enabled across page switches");
}

#[test]
fn f2_t2_finger_drawing_works_on_ipad_portrait_viewport() {
    let (_t, app, s) = sample_app("f2-t2-portrait");
    let mut h = ipad_portrait_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "PortraitDraw", "Empty note line");

    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "true"}})).expect("on");
    settle(&mut h);
    let origin = page_origin(&mut h);
    inject_primary_touch_drag(&mut h, 1, origin + egui::vec2(220.0, 200.0), origin + egui::vec2(320.0, 260.0), 6, None);
    assert_eq!(ink_strokes(&inspect(&mut h)), 1);
}

#[test]
fn f2_t2_finger_drawing_with_eraser_active_erases_not_draws() {
    let (_t, mut app, s) = sample_app("f2-t2-with-eraser");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);

    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "true"}})).expect("on");
    settle(&mut h);
    let origin = page_origin(&mut h);
    let p1 = origin + egui::vec2(460.0, 220.0);
    let p2 = origin + egui::vec2(560.0, 220.0);
    inject_primary_touch_drag(&mut h, 1, p1, p2, 6, None);
    assert_eq!(ink_strokes(&inspect(&mut h)), 1);

    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser", "params": {"mode": "eraser"}})).expect("eraser");
    settle(&mut h);
    inject_primary_touch_drag(&mut h, 2, origin + egui::vec2(510.0, 200.0), origin + egui::vec2(510.0, 240.0), 6, None);
    assert_eq!(ink_strokes(&inspect(&mut h)), 0, "finger drag with eraser active must erase the stroke");
}

#[test]
fn f2_t2_finger_drawing_on_readonly_page_does_not_panic_or_mutate_body() {
    let (_t, app, s) = sample_app("f2-t2-readonly");
    let mut h = tablet_harness(app);
    settle(&mut h);
    readonly_html_page(&mut h, &s, "ReadOnlyF2");
    let before = h.state().body().unwrap_or_default().to_string();

    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "true"}})).expect("on");
    settle(&mut h);
    let origin = page_origin(&mut h);
    inject_primary_touch_drag(&mut h, 1, origin + egui::vec2(180.0, 180.0), origin + egui::vec2(260.0, 220.0), 5, None);
    assert_eq!(h.state().body().unwrap_or_default(), before);
}

// ============================================================================
// F3: Scroll vs. Stylus Draw & Per-TouchId Separation (Tier 1 >= 5, Tier 2 >= 5)
// ============================================================================

#[test]
fn f3_t1_finger_touch_drag_scrolls_vertically_when_finger_drawing_false() {
    let (_t, app, s) = sample_app("f3-t1-finger-scroll");
    let mut h = tablet_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "ScrollPage", &scrollable_body());

    assert!(!require_finger_drawing(&inspect(&mut h)));
    let before = inspect(&mut h);
    let y0 = page_origin_vec(&before).1;
    let pan0 = canvas_offset(&before).1;

    let origin = page_origin(&mut h);
    inject_primary_touch_drag(&mut h, 10, origin + egui::vec2(200.0, 320.0), origin + egui::vec2(200.0, 120.0), 8, None);

    let after = inspect(&mut h);
    assert_eq!(ink_strokes(&after), 0, "finger drag must not create ink");
    let y1 = page_origin_vec(&after).1;
    let pan1 = canvas_offset(&after).1;
    assert!(y1 < y0 || (pan1 - pan0).abs() > 10.0, "finger drag upward must scroll/pan page: ({y0}, {pan0}) -> ({y1}, {pan1})");
}

#[test]
fn f3_t1_stylus_touch_draws_immediately_even_when_finger_drawing_false() {
    let (_t, mut app, s) = sample_app("f3-t1-stylus-draws");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);

    assert!(!require_finger_drawing(&inspect(&mut h)));
    assert_eq!(ink_pointer(&inspect(&mut h)), "write");

    let origin = page_origin(&mut h);
    inject_primary_touch_drag(&mut h, 20, origin + egui::vec2(350.0, 220.0), origin + egui::vec2(450.0, 260.0), 6, Some(0.6));

    let after = inspect(&mut h);
    assert_eq!(ink_strokes(&after), 1, "stylus touch with force must draw immediately");
    assert_eq!(ink_pointer(&after), "draw");
    assert_eq!(after.pointer("/pen/seen").and_then(Value::as_bool), Some(true));
}

#[test]
fn f3_t1_stylus_touch_records_varied_pressure_levels() {
    let (_t, mut app, s) = sample_app("f3-t1-pressure");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);

    let origin = page_origin(&mut h);
    let at = origin + egui::vec2(320.0, 200.0);
    for (idx, p) in [0.15_f32, 0.55, 0.95].into_iter().enumerate() {
        let phase = if idx == 0 { egui::TouchPhase::Start } else { egui::TouchPhase::Move };
        let pos = at + egui::vec2(idx as f32 * 25.0, 0.0);
        h.input_mut().events.push(egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(5), phase, pos, force: Some(p) });
        h.input_mut().events.push(egui::Event::PointerMoved(pos));
        if idx == 0 {
            h.input_mut().events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            });
        }
        h.step();
    }
    let end = at + egui::vec2(75.0, 0.0);
    h.input_mut().events.push(egui::Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: egui::TouchId(5),
        phase: egui::TouchPhase::End,
        pos: end,
        force: Some(0.4),
    });
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: end,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    settle(&mut h);

    let pen = inspect(&mut h)["pen"].clone();
    assert_eq!(pen["seen"], json!(true));
    assert!(pen["hardest"].as_f64().unwrap_or(0.0) >= 0.94, "hardest pressure must be ~0.95: {pen}");
    assert!(pen["readings"].as_u64().unwrap_or(0) >= 3);
    assert_eq!(ink_strokes(&inspect(&mut h)), 1);
}

#[test]
fn f3_t1_concurrent_finger_and_stylus_touch_ids_do_not_clobber_each_other() {
    // Verify Stylus per-contact isolation directly and end-to-end inside NocturneApp.
    let mut tracker = Stylus::default();
    let finger_id = egui::TouchId(100);
    let finger2_id = egui::TouchId(101);
    let stylus_id = egui::TouchId(200);
    tracker.update(&[
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: finger_id,
            phase: egui::TouchPhase::Start,
            pos: egui::pos2(80.0, 80.0),
            force: None,
        },
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(1),
            id: stylus_id,
            phase: egui::TouchPhase::Start,
            pos: egui::pos2(200.0, 200.0),
            force: Some(0.65),
        },
    ]);
    assert!(tracker.sample().pen && tracker.sample().down);
    assert!((tracker.sample().pressure - 0.65).abs() < 0.01);
    assert_eq!(tracker.landed(), Some(egui::pos2(200.0, 200.0)));

    // Same frame: stylus Move followed by finger Move (force: None) must NOT zero stylus pressure.
    tracker.update(&[
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(1),
            id: stylus_id,
            phase: egui::TouchPhase::Move,
            pos: egui::pos2(220.0, 205.0),
            force: Some(0.70),
        },
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: finger_id,
            phase: egui::TouchPhase::Move,
            pos: egui::pos2(81.0, 81.0),
            force: None,
        },
    ]);
    assert!(
        tracker.sample().down && tracker.sample().pressure > 0.0,
        "finger Move after stylus Move must not clobber stylus: {:?}",
        tracker.sample()
    );
    assert!((tracker.sample().pressure - 0.70).abs() < 0.01);

    // Same frame: stylus Move followed by finger End must NOT mark stylus as lifted!
    tracker.update(&[
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(1),
            id: stylus_id,
            phase: egui::TouchPhase::Move,
            pos: egui::pos2(240.0, 210.0),
            force: Some(0.75),
        },
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: finger_id,
            phase: egui::TouchPhase::End,
            pos: egui::pos2(82.0, 82.0),
            force: None,
        },
    ]);
    assert!(
        tracker.sample().down && tracker.sample().pressure > 0.0,
        "finger End after stylus Move must not clear stylus state: {:?}",
        tracker.sample()
    );
    assert!((tracker.sample().pressure - 0.75).abs() < 0.01, "finger lift must not zero stylus pressure: {:?}", tracker.sample());

    // Separate frames: a second finger starts and then ends ALONE while stylus stays down.
    tracker.update(&[egui::Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: finger2_id,
        phase: egui::TouchPhase::Start,
        pos: egui::pos2(95.0, 95.0),
        force: None,
    }]);
    assert!(tracker.sample().down && tracker.sample().pressure > 0.0, "separate finger Start must not clobber active stylus: {:?}", tracker.sample());
    tracker.update(&[egui::Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: finger2_id,
        phase: egui::TouchPhase::End,
        pos: egui::pos2(95.0, 95.0),
        force: None,
    }]);
    assert!(
        tracker.sample().down && tracker.sample().pressure > 0.0,
        "separate finger End alone must not lift active stylus: {:?}",
        tracker.sample()
    );
    assert!((tracker.sample().pressure - 0.75).abs() < 0.01);

    tracker.update(&[egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: stylus_id,
        phase: egui::TouchPhase::End,
        pos: egui::pos2(245.0, 212.0),
        force: Some(0.2),
    }]);
    assert!(!tracker.sample().down, "stylus End must lift stylus");

    // Now verify inside NocturneApp with mid-stroke palm/finger arrival and lift during stylus drawing.
    let (_t, mut app, s) = sample_app("f3-t1-concurrent");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);
    let origin = page_origin(&mut h);
    let palm = origin + egui::vec2(120.0, 300.0);
    let pen_start = origin + egui::vec2(340.0, 220.0);
    let pen_mid1 = origin + egui::vec2(380.0, 228.0);
    let pen_mid2 = origin + egui::vec2(410.0, 234.0);
    let pen_end = origin + egui::vec2(450.0, 240.0);

    // Palm rests (TouchId 100, force None), while stylus starts (TouchId 200, force Some(0.65)).
    h.input_mut().events.push(egui::Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: finger_id,
        phase: egui::TouchPhase::Start,
        pos: palm,
        force: None,
    });
    h.step();

    h.input_mut().events.extend([
        egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: stylus_id, phase: egui::TouchPhase::Start, pos: pen_start, force: Some(0.65) },
        egui::Event::PointerMoved(pen_start),
        egui::Event::PointerButton { pos: pen_start, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
    ]);
    h.step();

    // Mid-stroke: stylus Move followed by finger Move (force: None).
    h.input_mut().events.extend([
        egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: stylus_id, phase: egui::TouchPhase::Move, pos: pen_mid1, force: Some(0.70) },
        egui::Event::PointerMoved(pen_mid1),
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: finger_id,
            phase: egui::TouchPhase::Move,
            pos: palm + egui::vec2(4.0, 2.0),
            force: None,
        },
    ]);
    h.step();

    // Mid-stroke: stylus Move followed by palm End in same frame.
    h.input_mut().events.extend([
        egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: stylus_id, phase: egui::TouchPhase::Move, pos: pen_mid2, force: Some(0.75) },
        egui::Event::PointerMoved(pen_mid2),
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: finger_id,
            phase: egui::TouchPhase::End,
            pos: palm + egui::vec2(4.0, 2.0),
            force: None,
        },
    ]);
    h.step();

    // Separate frame while stylus remains down: second finger starts and lifts alone.
    h.input_mut().events.push(egui::Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: finger2_id,
        phase: egui::TouchPhase::Start,
        pos: palm + egui::vec2(20.0, 0.0),
        force: None,
    });
    h.step();
    h.input_mut().events.push(egui::Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: finger2_id,
        phase: egui::TouchPhase::End,
        pos: palm + egui::vec2(20.0, 0.0),
        force: None,
    });
    h.step();

    // Stylus continues with higher pressure and finishes stroke.
    h.input_mut().events.extend([
        egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: stylus_id, phase: egui::TouchPhase::Move, pos: pen_end, force: Some(0.85) },
        egui::Event::PointerMoved(pen_end),
    ]);
    h.step();
    h.input_mut().events.extend([
        egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: stylus_id, phase: egui::TouchPhase::End, pos: pen_end, force: Some(0.4) },
        egui::Event::PointerButton { pos: pen_end, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE },
    ]);
    h.step();
    settle(&mut h);

    let after = inspect(&mut h);
    assert_eq!(ink_strokes(&after), 1, "stylus stroke must complete cleanly as 1 stroke despite mid-stroke palm/finger lifts: {after}");
    assert!(after["pen"]["hardest"].as_f64().unwrap_or(0.0) >= 0.84, "post-lift stylus Move must still record 0.85 pressure: {after}");
}

#[test]
fn f3_t1_multitouch_canvas_step_pans_and_zooms_without_creating_strokes() {
    let (_t, app, s) = sample_app("f3-t1-multitouch");
    let mut h = tablet_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "PinchPage", &scrollable_body());

    // Verify helper behavior and app zoom range.
    let step = compute_multitouch_canvas_step(egui::vec2(15.0, -30.0), 1.25, 100);
    assert_eq!(step.pan_x, 15.0);
    assert_eq!(step.scroll_y, -30.0);
    assert_eq!(step.new_zoom, Some(125));

    let before = inspect(&mut h);
    let pan_before = canvas_offset(&before);
    let origin_before = page_origin_vec(&before);

    // Simulate two active finger contacts (no force) spreading and panning across frames,
    // emitting PointerMoved at the gesture center so egui's TouchState activates multi_touch().
    let origin = page_origin(&mut h);
    let f1 = origin + egui::vec2(200.0, 120.0);
    let f2 = origin + egui::vec2(300.0, 120.0);
    for (phase, p1, p2) in [
        (egui::TouchPhase::Start, f1, f2),
        (egui::TouchPhase::Move, f1 + egui::vec2(-20.0, -15.0), f2 + egui::vec2(40.0, -15.0)),
        (egui::TouchPhase::Move, f1 + egui::vec2(-30.0, -30.0), f2 + egui::vec2(70.0, -30.0)),
        (egui::TouchPhase::End, f1 + egui::vec2(-30.0, -30.0), f2 + egui::vec2(70.0, -30.0)),
    ] {
        h.input_mut().events.push(egui::Event::PointerMoved(p1.lerp(p2, 0.5)));
        h.input_mut().events.push(egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(1), phase, pos: p1, force: None });
        h.input_mut().events.push(egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(2), phase, pos: p2, force: None });
        h.step();
    }
    settle(&mut h);
    let after = inspect(&mut h);
    assert_eq!(ink_strokes(&after), 0, "two-finger pinch/scroll must never draw ink");
    let zoom = h.state().zoom_percent();
    assert!(zoom > 100 && (50..=250).contains(&zoom), "two-finger pinch spread must zoom in within canonical 50..=250 bounds: {zoom}");
    let pan_after = canvas_offset(&after);
    let origin_after = page_origin_vec(&after);
    assert!(
        (pan_after.0 - pan_before.0).abs() > 1.0 || (origin_after.1 - origin_before.1).abs() > 1.0,
        "two-finger pan/scroll must move canvas_pan or page_origin: pan {pan_before:?}->{pan_after:?}, origin {origin_before:?}->{origin_after:?}"
    );
}

#[test]
fn f3_t2_stylus_zero_and_nan_force_boundary_handling() {
    let mut tracker = Stylus::default();
    tracker.update(&[egui::Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: egui::TouchId(1),
        phase: egui::TouchPhase::Start,
        pos: egui::pos2(10.0, 10.0),
        force: Some(f32::NAN),
    }]);
    assert!(!tracker.sample().pen, "NaN force must not activate pen mode");
    assert!(tracker.sample().pressure.is_finite());

    tracker.update(&[egui::Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: egui::TouchId(2),
        phase: egui::TouchPhase::Start,
        pos: egui::pos2(20.0, 20.0),
        force: Some(0.0),
    }]);
    assert!(tracker.sample().pen, "finite 0.0 force identifies a stylus contact");
    assert_eq!(tracker.sample().pressure, 0.0);

    let (_t, mut app, s) = sample_app("f3-t2-nan-app");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);
    let origin = page_origin(&mut h);
    inject_primary_touch_drag(&mut h, 1, origin + egui::vec2(260.0, 220.0), origin + egui::vec2(320.0, 240.0), 4, Some(f32::NAN));
    let pen = inspect(&mut h)["pen"].clone();
    assert!(pen["pressure"].as_f64().is_some_and(f64::is_finite), "app pressure stays finite on NaN input: {pen}");
}

#[test]
fn f3_t2_stylus_landed_on_ribbon_does_not_draw_on_page() {
    let (_t, mut app, s) = sample_app("f3-t2-ribbon-touch");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);

    let draw_rect = h.get_by_label("Draw").rect();
    let from = draw_rect.center() + egui::vec2(40.0, 30.0);
    let to = from + egui::vec2(60.0, 0.0);
    inject_primary_touch_drag(&mut h, 9, from, to, 4, Some(0.6));

    assert_eq!(ink_strokes(&inspect(&mut h)), 0, "stylus stroke on ribbon must not create ink on the page");
}

#[test]
fn f3_t2_touch_cancel_phase_cleans_up_without_stuck_drag() {
    let (_t, mut app, s) = sample_app("f3-t2-cancel");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);
    let origin = page_origin(&mut h);
    let p0 = origin + egui::vec2(300.0, 220.0);
    let p1 = p0 + egui::vec2(40.0, 10.0);

    h.input_mut().events.push(egui::Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: egui::TouchId(7),
        phase: egui::TouchPhase::Start,
        pos: p0,
        force: Some(0.6),
    });
    h.input_mut().events.push(egui::Event::PointerMoved(p0));
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: p0,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();

    h.input_mut().events.push(egui::Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: egui::TouchId(7),
        phase: egui::TouchPhase::Cancel,
        pos: p1,
        force: Some(0.6),
    });
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: p1,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    settle(&mut h);

    let state = inspect(&mut h);
    assert_eq!(state.pointer("/ink/holding").and_then(Value::as_u64), Some(0), "TouchPhase::Cancel must release held stroke: {state}");
}

#[test]
fn f3_t2_zoom_percent_clamping_consistency_at_min_and_max_bounds() {
    let (_t, mut app, s) = sample_app("f3-t2-zoom-clamp");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);

    h.get_by_label("View").click();
    settle(&mut h);
    for _ in 0..20 {
        h.get_by_label("Zoom Out").click();
        pump(&mut h);
    }
    settle(&mut h);
    let min_zoom = h.state().zoom_percent();
    assert_eq!(min_zoom, 15, "minimum canonical zoom_percent must clamp at 15: {min_zoom}");
    if let Some(json_z) = json_zoom_percent(&inspect(&mut h)) {
        assert_eq!(json_z, min_zoom);
    }

    for _ in 0..30 {
        h.get_by_label("Zoom In").click();
        pump(&mut h);
    }
    settle(&mut h);
    let max_zoom = h.state().zoom_percent();
    assert_eq!(max_zoom, 250, "maximum canonical zoom_percent must clamp at 250: {max_zoom}");
    if let Some(json_z) = json_zoom_percent(&inspect(&mut h)) {
        assert_eq!(json_z, max_zoom);
    }

    // Verify touch_ux step stays consistent with canonical 15..=250 range when integrated.
    let step_low = compute_multitouch_canvas_step(egui::Vec2::ZERO, 0.2, 15);
    let step_high = compute_multitouch_canvas_step(egui::Vec2::ZERO, 4.0, 250);
    assert!(step_low.new_zoom.is_none_or(|z| z >= 15), "zoom_percent clamping consistency at lower bound: {step_low:?}");
    assert!(step_high.new_zoom.is_none_or(|z| z <= 250), "zoom_percent clamping consistency at upper bound: {step_high:?}");
}

#[test]
fn f3_t2_interleaved_three_touch_ids_finger_palm_stylus_no_panic() {
    let (_t, mut app, s) = sample_app("f3-t2-three-contacts");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);
    let origin = page_origin(&mut h);

    // Frame 1: finger 0 + palm 1 + stylus 2 start.
    h.input_mut().events.extend([
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(0),
            phase: egui::TouchPhase::Start,
            pos: origin + egui::vec2(60.0, 320.0),
            force: None,
        },
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(1),
            phase: egui::TouchPhase::Start,
            pos: origin + egui::vec2(90.0, 360.0),
            force: None,
        },
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(1),
            id: egui::TouchId(2),
            phase: egui::TouchPhase::Start,
            pos: origin + egui::vec2(260.0, 200.0),
            force: Some(0.5),
        },
        egui::Event::PointerMoved(origin + egui::vec2(260.0, 200.0)),
        egui::Event::PointerButton {
            pos: origin + egui::vec2(260.0, 200.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    h.step();

    // Frame 2: stylus Move followed by palm/finger Move (force: None).
    let mid1 = origin + egui::vec2(290.0, 208.0);
    let mid2 = origin + egui::vec2(315.0, 214.0);
    let end = origin + egui::vec2(340.0, 220.0);
    h.input_mut().events.extend([
        egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(2), phase: egui::TouchPhase::Move, pos: mid1, force: Some(0.65) },
        egui::Event::PointerMoved(mid1),
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(1),
            phase: egui::TouchPhase::Move,
            pos: origin + egui::vec2(92.0, 362.0),
            force: None,
        },
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(0),
            phase: egui::TouchPhase::Move,
            pos: origin + egui::vec2(62.0, 322.0),
            force: None,
        },
    ]);
    h.step();

    // Frame 3: stylus Move followed by palm Cancel and finger End in same frame while stylus stays down.
    h.input_mut().events.extend([
        egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(2), phase: egui::TouchPhase::Move, pos: mid2, force: Some(0.7) },
        egui::Event::PointerMoved(mid2),
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(1),
            phase: egui::TouchPhase::Cancel,
            pos: origin + egui::vec2(92.0, 362.0),
            force: None,
        },
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(0),
            phase: egui::TouchPhase::End,
            pos: origin + egui::vec2(62.0, 322.0),
            force: None,
        },
    ]);
    h.step();

    // Frame 4: a fresh finger contact starts and ends alone while stylus remains down.
    h.input_mut().events.push(egui::Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: egui::TouchId(3),
        phase: egui::TouchPhase::Start,
        pos: origin + egui::vec2(75.0, 330.0),
        force: None,
    });
    h.step();
    h.input_mut().events.push(egui::Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: egui::TouchId(3),
        phase: egui::TouchPhase::End,
        pos: origin + egui::vec2(75.0, 330.0),
        force: None,
    });
    h.step();

    // Frame 5-6: stylus continues with higher pressure and lifts cleanly.
    h.input_mut().events.extend([
        egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(2), phase: egui::TouchPhase::Move, pos: end, force: Some(0.85) },
        egui::Event::PointerMoved(end),
    ]);
    h.step();
    h.input_mut().events.extend([
        egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(2), phase: egui::TouchPhase::End, pos: end, force: Some(0.85) },
        egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE },
    ]);
    h.step();
    settle(&mut h);

    let after = inspect(&mut h);
    assert_eq!(ink_strokes(&after), 1);
    assert!(after["pen"]["hardest"].as_f64().unwrap_or(0.0) >= 0.84, "stylus pressure must survive interleaved palm/finger lifts: {after}");
}

// ============================================================================
// F4: Tap-to-Edit Text & Caret Geometry (Tier 1 >= 5, Tier 2 >= 5)
// ============================================================================

#[test]
fn f4_t1_tap_into_text_block_from_draw_mode_enters_write_and_focuses_caret() {
    const LINE: &str = "Tap this paragraph from draw mode";
    let (_t, app, s) = sample_app("f4-t1-tap-draw");
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "TapDraw", LINE);

    h.get_by_label("Draw").click();
    settle(&mut h);
    h.get_by_label("Pen black").click();
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "draw");

    let rect = h.get_by_label(LINE).rect();
    inject_primary_touch_tap(&mut h, 1, rect.center(), None);

    let after = inspect(&mut h);
    assert_eq!(ink_pointer(&after), "write", "tapping text from draw mode must enter write mode: {after}");
    assert!(after.pointer("/page/caret").is_some_and(|c| !c.is_null()), "caret must be focused after tap: {after}");
    assert_eq!(ink_strokes(&after), 0, "tapping text with finger must not create an ink stroke");
}

#[test]
fn f4_t1_tap_into_text_block_from_pan_mode_enters_write_mode() {
    const LINE: &str = "Tap this paragraph from pan mode";
    let (_t, app, s) = sample_app("f4-t1-tap-pan");
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "TapPan", LINE);

    h.get_by_label("Draw").click();
    settle(&mut h);
    h.get_by_label("Panning Hand").click();
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "pan");

    let rect = h.get_by_label(LINE).rect();
    inject_primary_touch_tap(&mut h, 2, rect.center(), None);

    let after = inspect(&mut h);
    assert_eq!(ink_pointer(&after), "write", "tapping text from pan mode must enter write mode: {after}");
    assert!(after.pointer("/page/caret").is_some_and(|c| !c.is_null()), "caret must focus on tapped block: {after}");

    ask(&mut h, "ui.text", json!({"text": "!"})).expect("type after tap");
    settle(&mut h);
    assert!(h.state().body().unwrap_or_default().contains('!'), "typing after tap reaches the paragraph");
}

#[test]
fn f4_t1_keyboard_caret_rect_returns_tighter_caret_rect_than_page_rect() {
    let (_t, mut app, s) = sample_app("f4-t1-caret-rect");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);

    ask(&mut h, "ui.caret", json!({"block": 1})).expect("caret");
    settle(&mut h);

    let page_rect = h.state().page_rect().expect("page_rect");
    let caret_rect = h.state().keyboard_caret_rect().expect("keyboard_caret_rect");
    assert!(
        caret_rect.height() < page_rect.height() * 0.25,
        "keyboard_caret_rect must return a tighter line/caret rect, not full page_rect: caret={caret_rect:?} page={page_rect:?}"
    );
    assert!(page_rect.expand(4.0).contains(caret_rect.center()), "caret_rect {caret_rect:?} must lie within page_rect {page_rect:?}");
}

#[test]
fn f4_t1_keyboard_caret_rect_tracks_different_blocks_vertically() {
    let body = "First block near the top\n\nSecond block\n\nThird block\n\nFourth block\n\nFifth block lower down";
    let (_t, app, s) = sample_app("f4-t1-caret-track");
    let mut h = tablet_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "TrackCaret", body);

    ask(&mut h, "ui.caret", json!({"block": 0})).expect("top caret");
    settle(&mut h);
    let top_rect = h.state().keyboard_caret_rect().expect("top caret rect");

    ask(&mut h, "ui.caret", json!({"block": 4})).expect("lower caret");
    settle(&mut h);
    let lower_rect = h.state().keyboard_caret_rect().expect("lower caret rect");

    assert!(lower_rect.min.y > top_rect.min.y + 20.0, "caret rect in block 4 must sit below block 0: top={top_rect:?} lower={lower_rect:?}");
}

#[test]
fn f4_t1_tap_empty_canvas_starts_new_box_and_focuses_caret() {
    let (_t, app, s) = sample_app("f4-t1-open-paper");
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "OpenPaper", "Initial box content");

    let before_boxes = inspect(&mut h).pointer("/page/containers").and_then(Value::as_array).map_or(0, Vec::len);
    let origin = page_origin(&mut h);
    inject_primary_touch_tap(&mut h, 3, origin + egui::vec2(520.0, 140.0), None);

    let after = inspect(&mut h);
    assert!(after.pointer("/page/caret").is_some_and(|c| !c.is_null()), "tapping empty paper creates caret: {after}");
    let after_boxes = after.pointer("/page/containers").and_then(Value::as_array).map_or(0, Vec::len);
    assert!(after_boxes > before_boxes, "new box created at tap location: {before_boxes} -> {after_boxes}");
}

#[test]
fn f4_t2_keyboard_caret_rect_is_none_when_no_caret_is_focused() {
    let (_t, mut app, s) = sample_app("f4-t2-no-caret");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);

    assert_eq!(h.state().keyboard_caret_rect(), None);
}

#[test]
fn f4_t2_keyboard_caret_rect_is_none_in_source_view() {
    let (_t, mut app, s) = sample_app("f4-t2-source-view");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);

    ask(&mut h, "ui.caret", json!({"block": 1})).expect("caret");
    settle(&mut h);
    assert!(h.state().keyboard_caret_rect().is_some());

    h.state_mut().set_show_source(true);
    settle(&mut h);
    assert_eq!(h.state().keyboard_caret_rect(), None, "source view must suppress keyboard_caret_rect");
}

#[test]
fn f4_t2_keyboard_caret_rect_is_none_when_modal_dialog_is_open() {
    let (_t, mut app, s) = sample_app("f4-t2-dialog");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);

    ask(&mut h, "ui.caret", json!({"block": 1})).expect("caret");
    settle(&mut h);
    assert!(h.state().keyboard_caret_rect().is_some());

    h.get_by_label("New notebook").click();
    settle(&mut h);
    assert_eq!(inspect(&mut h)["dialogOpen"], json!(true));
    assert_eq!(h.state().keyboard_caret_rect(), None, "modal dialog must suppress keyboard_caret_rect");
}

#[test]
fn f4_t2_keyboard_caret_rect_is_none_in_compact_list_view() {
    let (_t, mut app, s) = sample_app("f4-t2-compact-list");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = sized_harness(app, LIGHT, egui::vec2(390.0, 800.0));
    settle(&mut h);

    ask(&mut h, "ui.caret", json!({"block": 1})).expect("caret");
    settle(&mut h);
    assert!(h.state().keyboard_caret_rect().is_some());

    assert!(h.state_mut().system_back());
    settle(&mut h);
    assert_eq!(inspect(&mut h)["panes"]["list"], json!(true));
    assert_eq!(h.state().keyboard_caret_rect(), None, "compact list must suppress keyboard_caret_rect");
}

#[test]
fn f4_t2_tap_on_readonly_page_does_not_focus_writable_caret() {
    let (_t, app, s) = sample_app("f4-t2-readonly");
    let mut h = tablet_harness(app);
    settle(&mut h);
    readonly_html_page(&mut h, &s, "ReadOnlyF4");
    let before = h.state().body().unwrap_or_default().to_string();

    let para = h.get_by_label("Keep me").rect();
    inject_primary_touch_tap(&mut h, 4, para.center(), None);

    let after = inspect(&mut h);
    assert!(after.pointer("/page/caret").is_none_or(Value::is_null), "readonly page must not acquire writable caret");
    assert_eq!(h.state().keyboard_caret_rect(), None);
    ask(&mut h, "ui.text", json!({"text": "bad"})).expect("text ignored");
    settle(&mut h);
    assert_eq!(h.state().body().unwrap_or_default(), before);
}

// ============================================================================
// F5: Docked Above-Keyboard Formatting Bar (Tier 1 >= 5, Tier 2 >= 5)
// ============================================================================

#[test]
fn f5_t1_formatting_bar_appears_above_keyboard_inset_with_touch_targets() {
    let (_t, mut app, s) = sample_app("f5-t1-visible");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);

    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);
    ask(&mut h, "ui.caret", json!({"block": 1})).expect("caret");
    settle(&mut h);

    let state = inspect(&mut h);
    assert!(require_formatting_bar_visible(&state), "formatting bar must appear when keyboard inset > 0 and caret is focused: {state}");

    for group in [
        &["Indent", "Increase indentation"][..],
        &["Outdent", "Decrease indentation"][..],
        &["Undo"][..],
        &["Redo"][..],
        &["Bold"][..],
        &["Checkbox", "To Do", "To-do", "To Do Tag"][..],
    ] {
        let rect = find_docked_bar_button(&h, group, KEYBOARD_INSET_BOTTOM);
        assert!(rect.max.y <= 768.0 - KEYBOARD_INSET_BOTTOM + 2.0, "{group:?} must sit above keyboard inset: {rect:?}");
    }
    let img = support::gpu::render(&mut h).expect("offscreen GPU render of docked formatting bar");
    assert!(img.width() > 0 && img.height() > 0);
}

#[test]
fn f5_t1_formatting_bar_hides_when_keyboard_inset_zero_or_no_caret() {
    let (_t, mut app, s) = sample_app("f5-t1-hides");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);

    // Keyboard inset active but no caret focused -> hidden.
    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);
    assert!(!require_formatting_bar_visible(&inspect(&mut h)), "bar must hide without caret");

    // Caret focused with keyboard inset -> visible.
    ask(&mut h, "ui.caret", json!({"block": 1})).expect("caret");
    settle(&mut h);
    assert!(require_formatting_bar_visible(&inspect(&mut h)), "bar must show with caret and keyboard inset");

    // Keyboard dismissed (bottom == 0) -> hidden.
    set_keyboard_inset(&mut h, 0.0);
    assert!(!require_formatting_bar_visible(&inspect(&mut h)), "bar must hide when keyboard inset returns to 0");
}

#[test]
fn f5_t1_formatting_bar_bold_and_checkbox_mutate_page_body() {
    let (_t, app, s) = sample_app("f5-t1-bold-todo");
    let mut h = tablet_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "BarBoldTodo", "Alpha line");

    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);
    ask(&mut h, "ui.caret", json!({"block": 0})).expect("caret");
    settle(&mut h);
    ask(&mut h, "ui.key", json!({"key": "Home"})).expect("Home");
    ask(&mut h, "ui.key", json!({"key": "End", "shift": true})).expect("Shift+End");
    settle(&mut h);

    click_docked_bar_button(&mut h, &["Bold"], KEYBOARD_INSET_BOTTOM);
    let body_bold = h.state().body().unwrap_or_default().to_string();
    assert!(body_bold.contains("**Alpha line**"), "docked Bold must wrap selection: {body_bold}");

    click_docked_bar_button(&mut h, &["Checkbox", "To Do", "To-do", "To Do Tag"], KEYBOARD_INSET_BOTTOM);
    let body_todo = h.state().body().unwrap_or_default().to_string();
    assert!(body_todo.contains("- [ ]"), "docked Checkbox/ToDo must make a task line: {body_todo}");
}

#[test]
fn f5_t1_formatting_bar_indent_and_outdent_update_list_nesting() {
    let (_t, app, s) = sample_app("f5-t1-indent");
    let mut h = tablet_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "BarIndent", "- Parent item\n- Child item");

    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);
    ask(&mut h, "ui.caret", json!({"block": 1})).expect("caret on second list item");
    settle(&mut h);

    click_docked_bar_button(&mut h, &["Indent", "Increase indentation"], KEYBOARD_INSET_BOTTOM);
    let indented = h.state().body().unwrap_or_default().to_string();
    assert!(
        indented.lines().any(|l| l.starts_with("  - Child item") || l.starts_with("    - Child item") || l.starts_with("\t- Child item")),
        "docked Indent must indent list item:\n{indented}"
    );

    click_docked_bar_button(&mut h, &["Outdent", "Decrease indentation"], KEYBOARD_INSET_BOTTOM);
    let outdented = h.state().body().unwrap_or_default().to_string();
    assert!(outdented.lines().any(|l| l.starts_with("- Child item")), "docked Outdent must un-indent list item:\n{outdented}");
}

#[test]
fn f5_t1_formatting_bar_undo_and_redo_revert_and_reapply_edits() {
    let (_t, app, s) = sample_app("f5-t1-undo-redo");
    let mut h = tablet_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "BarUndoRedo", "Start");

    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);
    ask(&mut h, "ui.caret", json!({"block": 0})).expect("caret");
    settle(&mut h);
    ask(&mut h, "ui.text", json!({"text": " Added"})).expect("type");
    settle(&mut h);
    assert!(h.state().body().unwrap_or_default().contains("Added"));

    click_docked_bar_button(&mut h, &["Undo"], KEYBOARD_INSET_BOTTOM);
    assert!(!h.state().body().unwrap_or_default().contains("Added"), "docked Undo must revert typed text");

    click_docked_bar_button(&mut h, &["Redo"], KEYBOARD_INSET_BOTTOM);
    assert!(h.state().body().unwrap_or_default().contains("Added"), "docked Redo must reapply typed text");
}

#[test]
fn f5_t2_formatting_bar_does_not_cover_active_caret_rect() {
    let body = (1..=12).map(|i| format!("Line {i:02}")).collect::<Vec<_>>().join("\n\n");
    let (_t, app, s) = sample_app("f5-t2-no-overlap");
    let mut h = tablet_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "NoOverlap", &body);

    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);
    ask(&mut h, "ui.caret", json!({"block": 8})).expect("caret");
    settle(&mut h);

    let caret_rect = h.state().keyboard_caret_rect().expect("caret rect");
    let bold_rect = find_docked_bar_button(&h, &["Bold"], KEYBOARD_INSET_BOTTOM);
    assert!(!bold_rect.intersects(caret_rect), "docked formatting bar {bold_rect:?} must not overlap active caret {caret_rect:?}");
}

#[test]
fn f5_t2_formatting_bar_hidden_or_blocked_when_modal_dialog_is_open() {
    let (_t, mut app, s) = sample_app("f5-t2-modal-zorder");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);

    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);
    ask(&mut h, "ui.caret", json!({"block": 1})).expect("caret");
    settle(&mut h);
    assert!(require_formatting_bar_visible(&inspect(&mut h)));
    let bar_rect = find_docked_bar_button(&h, &["Bold"], KEYBOARD_INSET_BOTTOM);
    let before = h.state().body().unwrap_or_default().to_string();

    h.get_by_label("New notebook").click();
    settle(&mut h);
    assert_eq!(inspect(&mut h)["dialogOpen"], json!(true));
    assert!(!require_formatting_bar_visible(&inspect(&mut h)), "formatting bar must hide behind modal dialog");

    // Clicking where the bar button sat must not format the page behind the modal.
    let c = bar_rect.center();
    ask(&mut h, "ui.click", json!({"x": c.x, "y": c.y})).expect("click behind modal");
    settle(&mut h);
    assert_eq!(h.state().body().unwrap_or_default(), before);
}

#[test]
fn f5_t2_formatting_bar_hidden_on_readonly_page_and_source_view() {
    let (_t, mut app, s) = sample_app("f5-t2-readonly-src");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = tablet_harness(app);
    settle(&mut h);

    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);
    ask(&mut h, "ui.caret", json!({"block": 1})).expect("caret");
    settle(&mut h);
    assert!(require_formatting_bar_visible(&inspect(&mut h)));

    h.state_mut().set_show_source(true);
    settle(&mut h);
    assert!(!require_formatting_bar_visible(&inspect(&mut h)), "bar must hide in source view");

    h.state_mut().set_show_source(false);
    readonly_html_page(&mut h, &s, "ReadOnlyF5");
    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);
    assert!(!require_formatting_bar_visible(&inspect(&mut h)), "bar must hide on read-only page");
}

#[test]
fn f5_t2_formatting_bar_handles_cross_block_selection_safely() {
    const FIRST: &str = "Cross block alpha";
    const SECOND: &str = "Cross block beta";
    let (_t, app, s) = sample_app("f5-t2-across");
    let mut h = tablet_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "AcrossBar", &format!("{FIRST}\n\n{SECOND}"));

    let r1 = h.get_by_label(FIRST).rect();
    let r2 = h.get_by_label(SECOND).rect();
    inject_mouse_drag(&mut h, r1.left_center() + egui::vec2(10.0, 0.0), r2.center(), "left");
    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);

    let state = inspect(&mut h);
    assert!(state.pointer("/page/across").is_some_and(|a| !a.is_null()), "cross-block selection exists: {state}");
    // Must not panic regardless of whether bar stays hidden or shows on cross-block selection.
    let _visible = require_formatting_bar_visible(&state);
}

#[test]
fn f5_t2_formatting_bar_adapts_to_ipad_portrait_and_landscape_resizes() {
    let (_t, mut app, s) = sample_app("f5-t2-resize");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = ipad_portrait_harness(app);
    settle(&mut h);

    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);
    ask(&mut h, "ui.caret", json!({"block": 1})).expect("caret");
    settle(&mut h);
    assert!(require_formatting_bar_visible(&inspect(&mut h)));
    let r_portrait = find_docked_bar_button(&h, &["Bold"], KEYBOARD_INSET_BOTTOM);
    assert!(r_portrait.max.x <= 820.0 && r_portrait.max.y <= 1180.0 - KEYBOARD_INSET_BOTTOM + 2.0);

    h.set_size(egui::vec2(1180.0, 820.0));
    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);
    assert!(require_formatting_bar_visible(&inspect(&mut h)));
    let r_land = find_docked_bar_button(&h, &["Bold"], KEYBOARD_INSET_BOTTOM);
    assert!(r_land.max.x <= 1180.0 && r_land.max.y <= 820.0 - KEYBOARD_INSET_BOTTOM + 2.0);
}

// ============================================================================
// F6: Windows Desktop Mouse/Keyboard Non-Regression & Parity (Tier 1 >= 5, Tier 2 >= 5)
// ============================================================================

#[test]
fn f6_t1_mouse_left_drag_in_write_mode_selects_text_and_does_not_pan() {
    const LINE: &str = "Desktop mouse left drag selection stays crisp";
    let (_t, app, s) = sample_app("f6-t1-mouse-select");
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "MouseSelect", LINE);

    let before = inspect(&mut h);
    assert_eq!(ink_pointer(&before), "write");
    let pan0 = canvas_offset(&before);

    let rect = h.get_by_label(LINE).rect();
    inject_mouse_drag(&mut h, rect.left_center() + egui::vec2(10.0, 0.0), rect.right_center() - egui::vec2(10.0, 0.0), "left");

    let after = inspect(&mut h);
    assert_eq!(canvas_offset(&after), pan0, "mouse left-drag in write mode must not pan canvas");
    assert_eq!(ink_strokes(&after), 0);
    let at = after.pointer("/page/caret/at").and_then(Value::as_u64);
    let anchor = after.pointer("/page/caret/anchor").and_then(Value::as_u64);
    let has_caret_sel = at.is_some() && at != anchor;
    let has_across = after.pointer("/page/across").is_some_and(|a| !a.is_null());
    assert!(has_caret_sel || has_across, "mouse drag must select text: {after}");
}

#[test]
fn f6_t1_ctrl_b_ctrl_z_ctrl_y_keyboard_shortcuts_work() {
    let (_t, app, s) = sample_app("f6-t1-shortcuts");
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "Shortcuts", "alpha hello");

    ask(&mut h, "ui.caret", json!({"block": 0})).expect("caret");
    settle(&mut h);
    ask(&mut h, "ui.key", json!({"key": "Home"})).expect("Home");
    settle(&mut h);
    for _ in 0..6 {
        ask(&mut h, "ui.key", json!({"key": "ArrowRight"})).expect("ArrowRight");
        pump(&mut h);
    }
    for _ in 0..5 {
        ask(&mut h, "ui.key", json!({"key": "ArrowRight", "shift": true})).expect("Select word");
        pump(&mut h);
    }
    settle(&mut h);

    ask(&mut h, "ui.key", json!({"key": "B", "ctrl": true})).expect("Ctrl+B");
    settle(&mut h);
    assert!(h.state().body().unwrap_or_default().contains("**hello**"), "Ctrl+B bolds selection: {:?}", h.state().body());

    ask(&mut h, "ui.key", json!({"key": "Z", "ctrl": true})).expect("Ctrl+Z");
    settle(&mut h);
    assert!(!h.state().body().unwrap_or_default().contains("**hello**"), "Ctrl+Z undoes bold: {:?}", h.state().body());

    ask(&mut h, "ui.key", json!({"key": "Y", "ctrl": true})).expect("Ctrl+Y");
    settle(&mut h);
    assert!(h.state().body().unwrap_or_default().contains("**hello**"), "Ctrl+Y redoes bold: {:?}", h.state().body());
}

#[test]
fn f6_t1_alt_digit_express_keys_pick_pen_and_alt_0_returns_to_write() {
    let (_t, mut app, s) = sample_app("f6-t1-alt-keys");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);

    assert_eq!(ink_pointer(&inspect(&mut h)), "write");
    ask(&mut h, "ui.key", json!({"key": "2", "alt": true})).expect("Alt+2");
    settle(&mut h);
    let s2 = inspect(&mut h);
    assert_eq!((ink_pointer(&s2).as_str(), ink_nib(&s2)), ("draw", Some(1)));

    ask(&mut h, "ui.key", json!({"key": "0", "alt": true})).expect("Alt+0");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "write");

    ask(&mut h, "ui.key", json!({"key": "9", "alt": true})).expect("Alt+9");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "write");
}

#[test]
fn f6_t1_memory_board_copy_and_paste_preserve_markdown_content() {
    let board = MemoryBoard::new();
    let (_t, app, s) = sample_app_with_board("f6-t1-clipboard", &board);
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "CopyPaste", "CopiedPhrase\n\nTargetLine");

    ask(&mut h, "ui.caret", json!({"block": 0})).expect("caret 0");
    settle(&mut h);
    ask(&mut h, "ui.key", json!({"key": "Home"})).expect("Home");
    settle(&mut h);
    ask(&mut h, "ui.key", json!({"key": "End", "shift": true})).expect("Shift+End");
    settle(&mut h);
    press_copy(&mut h, &board, "C");
    assert!(board.read_text().contains("CopiedPhrase"), "MemoryBoard received copy: {:?}", board.read_text());

    ask(&mut h, "ui.caret", json!({"block": 1})).expect("caret 1");
    settle(&mut h);
    ask(&mut h, "ui.key", json!({"key": "End"})).expect("End");
    settle(&mut h);
    ask(&mut h, "ui.paste", json!({"text": format!(" {}", board.read_text())})).expect("paste");
    settle(&mut h);

    let body = h.state().body().unwrap_or_default().to_string();
    assert_eq!(body.matches("CopiedPhrase").count(), 2, "pasted phrase appears twice:\n{body}");
}

#[test]
fn f6_t1_mouse_wheel_scroll_and_ctrl_wheel_zoom_work_on_desktop() {
    let (_t, app, s) = sample_app("f6-t1-wheel");
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "WheelScroll", &scrollable_body());

    let origin = page_origin(&mut h);
    let hover = origin + egui::vec2(200.0, 200.0);
    ask(&mut h, "ui.move", json!({"x": hover.x, "y": hover.y})).expect("hover page");
    settle(&mut h);

    let y0 = page_origin_vec(&inspect(&mut h)).1;
    ask(&mut h, "ui.scroll", json!({"y": -200.0})).expect("wheel scroll");
    settle(&mut h);
    let y1 = page_origin_vec(&inspect(&mut h)).1;
    assert!(y1 < y0, "mouse wheel scroll moves page origin vertically: {y0} -> {y1}");

    let zoom0 = h.state().zoom_percent();
    h.input_mut().events.push(egui::Event::PointerMoved(hover));
    h.input_mut().events.push(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, 60.0),
        modifiers: egui::Modifiers { ctrl: true, command: true, ..Default::default() },
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
    });
    h.step();
    settle(&mut h);
    let zoom1 = h.state().zoom_percent();
    assert!(zoom1 >= zoom0 && (50..=250).contains(&zoom1), "ctrl+wheel zoom stays valid: {zoom0} -> {zoom1}");
}

#[test]
fn f6_t2_middle_mouse_drag_still_pans_canvas_when_finger_drawing_false() {
    let (_t, mut app, s) = sample_app("f6-t2-mid-pan");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);

    assert!(!require_finger_drawing(&inspect(&mut h)));
    let pan0 = canvas_offset(&inspect(&mut h));
    let origin = page_origin(&mut h);
    inject_mouse_drag(&mut h, origin + egui::vec2(300.0, 260.0), origin + egui::vec2(380.0, 320.0), "middle");

    let after = inspect(&mut h);
    assert_ne!(canvas_offset(&after), pan0, "middle mouse drag still pans canvas");
    assert_eq!(ink_strokes(&after), 0);
}

#[test]
fn f6_t2_windows_stylus_pen_draws_and_typing_returns_to_write() {
    let (_t, app, s) = sample_app("f6-t2-win-pen");
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "WinPen", "Initial text");

    // Pure mouse click/move in Write mode does not draw.
    let origin = page_origin(&mut h);
    inject_mouse_drag(&mut h, origin + egui::vec2(360.0, 220.0), origin + egui::vec2(440.0, 240.0), "left");
    assert_eq!(ink_strokes(&inspect(&mut h)), 0);

    // Caret focused, then Windows PT_PEN touch (force: Some) draws.
    ask(&mut h, "ui.caret", json!({"block": 0})).expect("caret");
    settle(&mut h);
    inject_primary_touch_drag(&mut h, 1, origin + egui::vec2(360.0, 220.0), origin + egui::vec2(440.0, 240.0), 5, Some(0.55));
    assert_eq!(ink_pointer(&inspect(&mut h)), "draw");
    assert_eq!(ink_strokes(&inspect(&mut h)), 1);

    // Typing resumes Write mode and appends text.
    h.input_mut().events.push(egui::Event::Text("X".to_string()));
    h.step();
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "write");
    assert!(h.state().body().unwrap_or_default().contains('X'));
}

#[test]
fn f6_t2_windows_touchscreen_finger_drawing_parity_with_mouse_selection() {
    const LINE: &str = "Hybrid desktop selection line";
    let (_t, app, s) = sample_app("f6-t2-hybrid-parity");
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "HybridWin", LINE);

    // Enable finger drawing (Windows touchscreen parity), draw with finger touch.
    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "true"}})).expect("on");
    settle(&mut h);
    let origin = page_origin(&mut h);
    inject_primary_touch_drag(&mut h, 5, origin + egui::vec2(450.0, 200.0), origin + egui::vec2(530.0, 220.0), 5, None);
    assert_eq!(ink_strokes(&inspect(&mut h)), 1);

    // Switch back to Write and verify mouse left-drag still selects text cleanly.
    h.get_by_label("Draw").click();
    settle(&mut h);
    h.get_by_label("Type").click();
    settle(&mut h);
    let rect = h.get_by_label(LINE).rect();
    inject_mouse_drag(&mut h, rect.left_center() + egui::vec2(10.0, 0.0), rect.right_center() - egui::vec2(10.0, 0.0), "left");
    let after = inspect(&mut h);
    assert_eq!(ink_strokes(&after), 1, "mouse drag in Write must select text, not add strokes");
    let at = after.pointer("/page/caret/at").and_then(Value::as_u64);
    let anchor = after.pointer("/page/caret/anchor").and_then(Value::as_u64);
    assert!(at.is_some() && at != anchor);
}

#[test]
fn f6_t2_desktop_panning_hand_button_still_pans_and_toggles_back_to_write() {
    let (_t, mut app, s) = sample_app("f6-t2-panning-hand");
    app.open_path(&s.html_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);

    h.get_by_label("Draw").click();
    settle(&mut h);
    h.get_by_label("Panning Hand").click();
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "pan");

    let pan0 = canvas_offset(&inspect(&mut h));
    let origin = page_origin(&mut h);
    inject_mouse_drag(&mut h, origin + egui::vec2(160.0, 200.0), origin + egui::vec2(100.0, 140.0), "left");
    let pan1 = canvas_offset(&inspect(&mut h));
    assert!((pan1.0 - pan0.0 + 60.0).abs() < 4.0 && (pan1.1 - pan0.1 + 60.0).abs() < 4.0, "Panning Hand pans: {pan0:?} -> {pan1:?}");

    h.get_by_label("Panning Hand").click();
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "write");
}

#[test]
fn f6_t2_ctrl_b_in_draw_mode_keeps_pen_and_alt_shortcut_does_not_type() {
    let (_t, app, s) = sample_app("f6-t2-shortcut-isolation");
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "ShortcutGuard", "Keep clean");

    ask(&mut h, "ui.caret", json!({"block": 0})).expect("caret");
    settle(&mut h);
    let before = h.state().body().unwrap_or_default().to_string();

    h.get_by_label("Draw").click();
    settle(&mut h);
    h.get_by_label("Pen black").click();
    settle(&mut h);
    ask(&mut h, "ui.key", json!({"key": "b", "ctrl": true})).expect("Ctrl+B");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "draw", "Ctrl+B must not drop pen");

    let alt = egui::Modifiers { alt: true, ..Default::default() };
    h.input_mut().events.push(egui::Event::Key { key: egui::Key::Num1, physical_key: None, pressed: true, repeat: false, modifiers: alt });
    h.input_mut().events.push(egui::Event::Text("\u{a1}".to_string()));
    h.step();
    settle(&mut h);
    assert_eq!(h.state().body().unwrap_or_default(), before, "Alt+digit must not type stray text");
}

// ============================================================================
// Tier 3: Cross-Feature Pairwise Combinations (>= 7 tests)
// ============================================================================

#[test]
fn t3_f1_f3_eraser_toggle_with_concurrent_finger_pan_and_stylus() {
    let (_t, app, s) = sample_app("t3-f1-f3");
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "PairF1F3", &scrollable_body());

    let origin = page_origin(&mut h);
    // Draw 1 stroke with stylus (finger_drawing == false).
    let s_from = origin + egui::vec2(480.0, 220.0);
    let s_to = origin + egui::vec2(580.0, 220.0);
    inject_primary_touch_drag(&mut h, 1, s_from, s_to, 6, Some(0.6));
    assert_eq!(ink_strokes(&inspect(&mut h)), 1);

    // Toggle to eraser (F1); 1-finger touch still pans (F3) without erasing.
    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("eraser");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "erase");
    inject_primary_touch_drag(&mut h, 2, origin + egui::vec2(520.0, 260.0), origin + egui::vec2(520.0, 200.0), 5, None);
    assert_eq!(ink_strokes(&inspect(&mut h)), 1, "finger scroll in erase mode when finger_drawing==false must not erase stroke");

    // Stylus erases the stroke, then toggle back restores Pen.
    let origin2 = page_origin(&mut h);
    inject_primary_touch_drag(&mut h, 3, origin2 + egui::vec2(520.0, 140.0), origin2 + egui::vec2(520.0, 280.0), 8, Some(0.6));
    assert_eq!(ink_strokes(&inspect(&mut h)), 0, "stylus in erase mode erases the stroke");
    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("restore pen");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "draw");
}

#[test]
fn t3_f2_f3_toggle_finger_drawing_mid_session_with_stylus_and_finger() {
    let (_t, app, s) = sample_app("t3-f2-f3");
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "PairF2F3", &scrollable_body());

    let origin = page_origin(&mut h);
    // Stylus draws with finger_drawing == false.
    inject_primary_touch_drag(&mut h, 1, origin + egui::vec2(460.0, 180.0), origin + egui::vec2(560.0, 180.0), 5, Some(0.6));
    assert_eq!(ink_strokes(&inspect(&mut h)), 1);

    // Finger scrolls.
    let y0 = page_origin_vec(&inspect(&mut h)).1;
    inject_primary_touch_drag(&mut h, 2, origin + egui::vec2(300.0, 320.0), origin + egui::vec2(300.0, 180.0), 5, None);
    assert_eq!(ink_strokes(&inspect(&mut h)), 1);
    assert!(page_origin_vec(&inspect(&mut h)).1 < y0 || canvas_offset(&inspect(&mut h)).1.abs() > 1.0);

    // Enable finger_drawing -> finger draws 2nd stroke.
    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "true"}})).expect("on");
    settle(&mut h);
    inject_primary_touch_drag(&mut h, 3, origin + egui::vec2(460.0, 250.0), origin + egui::vec2(560.0, 250.0), 5, None);
    assert_eq!(ink_strokes(&inspect(&mut h)), 2);

    // Disable finger_drawing -> finger scrolls again.
    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "false"}})).expect("off");
    settle(&mut h);
    inject_primary_touch_drag(&mut h, 4, origin + egui::vec2(300.0, 180.0), origin + egui::vec2(300.0, 320.0), 5, None);
    assert_eq!(ink_strokes(&inspect(&mut h)), 2);
}

#[test]
fn t3_f1_f4_tap_text_from_eraser_switches_to_write_and_toggle_eraser_recovers_nib() {
    const LINE: &str = "Tap me while erasing";
    let (_t, app, s) = sample_app("t3-f1-f4");
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "PairF1F4", LINE);

    h.get_by_label("Draw").click();
    settle(&mut h);
    h.get_by_label("Highlighter yellow").click();
    settle(&mut h);
    let yellow_nib = ink_nib(&inspect(&mut h));

    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("enter eraser");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "erase");

    let rect = h.get_by_label(LINE).rect();
    inject_primary_touch_tap(&mut h, 1, rect.center(), None);
    let tapped = inspect(&mut h);
    assert_eq!(ink_pointer(&tapped), "write", "tapping text from erase mode enters write mode");
    assert!(tapped.pointer("/page/caret").is_some_and(|c| !c.is_null()));

    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("eraser again");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "erase");
    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("restore highlighter");
    settle(&mut h);
    let restored = inspect(&mut h);
    assert_eq!(ink_pointer(&restored), "draw");
    assert_eq!(ink_nib(&restored), yellow_nib, "Highlighter yellow nib preserved across tap-to-edit");
}

#[test]
fn t3_f4_f5_tap_to_edit_from_draw_shows_docked_formatting_bar_and_formats() {
    const LINE: &str = "Format via tap and bar";
    let (_t, app, s) = sample_app("t3-f4-f5");
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "PairF4F5", LINE);

    h.get_by_label("Draw").click();
    settle(&mut h);
    h.get_by_label("Pen black").click();
    settle(&mut h);
    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);

    let rect = h.get_by_label(LINE).rect();
    inject_primary_touch_tap(&mut h, 1, rect.center(), None);
    let state = inspect(&mut h);
    assert_eq!(ink_pointer(&state), "write");
    assert!(h.state().keyboard_caret_rect().is_some());
    assert!(require_formatting_bar_visible(&state));

    ask(&mut h, "ui.key", json!({"key": "Home"})).expect("Home");
    ask(&mut h, "ui.key", json!({"key": "End", "shift": true})).expect("Shift+End");
    settle(&mut h);
    click_docked_bar_button(&mut h, &["Bold"], KEYBOARD_INSET_BOTTOM);
    assert!(h.state().body().unwrap_or_default().contains("**Format via tap and bar**"));
}

#[test]
fn t3_f3_f5_scroll_with_finger_while_keyboard_open_keeps_formatting_bar_docked() {
    let (_t, app, s) = sample_app("t3-f3-f5");
    let mut h = tablet_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "PairF3F5", &scrollable_body());

    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);
    ask(&mut h, "ui.caret", json!({"block": 0})).expect("caret");
    settle(&mut h);
    let bar_before = find_docked_bar_button(&h, &["Bold"], KEYBOARD_INSET_BOTTOM);

    let origin_before = page_origin_vec(&inspect(&mut h));
    let page_rect = h.state().page_rect().expect("page_rect");
    let drag_x = page_rect.center().x;
    let from = egui::pos2(drag_x, (page_rect.max.y - 24.0).max(page_rect.min.y + 40.0));
    let to = egui::pos2(drag_x, page_rect.min.y + 24.0);
    assert!(page_rect.contains(from) && page_rect.contains(to), "scroll drag must stay inside page_rect {page_rect:?}: {from:?} -> {to:?}");
    inject_primary_touch_drag(&mut h, 1, from, to, 6, None);
    let after = inspect(&mut h);
    let origin_after = page_origin_vec(&after);
    assert!(
        origin_after.1 < origin_before.1,
        "finger scroll inside page_rect must move page upward: before={origin_before:?} after={origin_after:?}"
    );
    assert!(require_formatting_bar_visible(&after));
    let bar_after = find_docked_bar_button(&h, &["Bold"], KEYBOARD_INSET_BOTTOM);
    assert!((bar_after.min.y - bar_before.min.y).abs() < 2.0, "bar stays docked above keyboard during scroll");
}

#[test]
fn t3_f3_f6_finger_scroll_keeps_keyboard_focus_in_caret_block() {
    // A finger scroll shows the caret's block as finished words until the finger lifts.
    // The keyboard is still writing in that block afterwards: on an iPad with a keyboard
    // the next key goes on from where the caret was, with no tap to bring it back.
    let (_t, app, s) = sample_app("t3-f3-f6-focus");
    let mut h = tablet_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "ScrollFocus", &scrollable_body());

    ask(&mut h, "ui.caret", json!({"block": 0})).expect("caret");
    settle(&mut h);
    let focused_before = h.ctx.memory(|m| m.focused());
    assert!(focused_before.is_some(), "the caret's text box has keyboard focus before the scroll");

    let origin_before = page_origin_vec(&inspect(&mut h));
    let page_rect = h.state().page_rect().expect("page_rect");
    let from = egui::pos2(page_rect.center().x, page_rect.center().y + 75.0);
    let to = egui::pos2(page_rect.center().x, page_rect.center().y - 75.0);
    inject_primary_touch_drag(&mut h, 7, from, to, 6, None);
    let origin_after = page_origin_vec(&inspect(&mut h));
    assert!(origin_after.1 < origin_before.1, "the drag scrolled the page, not tapped it: before={origin_before:?} after={origin_after:?}");
    assert_eq!(h.ctx.memory(|m| m.focused()), focused_before, "a finger scroll leaves keyboard focus in the caret's text box");

    ask(&mut h, "ui.text", json!({"text": " KeptFocus"})).expect("type after scroll");
    settle(&mut h);
    assert!(h.state().body().unwrap_or_default().contains("verification. KeptFocus"), "typing after a finger scroll reaches the caret's block");
}

#[test]
fn t3_f5_f6_desktop_ctrl_z_after_formatting_bar_bold_and_indent() {
    let (_t, app, s) = sample_app("t3-f5-f6");
    let mut h = tablet_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "PairF5F6", "- First\n- Second");

    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);
    ask(&mut h, "ui.caret", json!({"block": 1})).expect("caret");
    settle(&mut h);
    click_docked_bar_button(&mut h, &["Indent", "Increase indentation"], KEYBOARD_INSET_BOTTOM);
    assert!(h.state().body().unwrap_or_default().lines().any(|l| l.starts_with("  - Second") || l.starts_with("\t- Second")));

    ask(&mut h, "ui.key", json!({"key": "Z", "ctrl": true})).expect("Ctrl+Z");
    settle(&mut h);
    assert!(h.state().body().unwrap_or_default().lines().any(|l| l.starts_with("- Second")), "Ctrl+Z undoes docked bar Indent");

    ask(&mut h, "ui.key", json!({"key": "Y", "ctrl": true})).expect("Ctrl+Y");
    settle(&mut h);
    assert!(
        h.state().body().unwrap_or_default().lines().any(|l| l.starts_with("  - Second") || l.starts_with("\t- Second")),
        "Ctrl+Y redoes docked bar Indent"
    );
}

#[test]
fn t3_f1_f2_f6_engine_commands_alt_shortcut_and_clipboard_parity() {
    let board = MemoryBoard::new();
    let (_t, app, s) = sample_app_with_board("t3-f1-f2-f6", &board);
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "PairF1F2F6", "ClipWord");

    ask(&mut h, "ui.caret", json!({"block": 0})).expect("caret");
    settle(&mut h);
    ask(&mut h, "ui.key", json!({"key": "Home"})).expect("Home");
    settle(&mut h);
    ask(&mut h, "ui.key", json!({"key": "End", "shift": true})).expect("Shift+End");
    settle(&mut h);
    press_copy(&mut h, &board, "C");
    assert!(board.read_text().contains("ClipWord"));

    ask(&mut h, "ui.key", json!({"key": "2", "alt": true})).expect("Alt+2");
    settle(&mut h);
    assert_eq!((ink_pointer(&inspect(&mut h)).as_str(), ink_nib(&inspect(&mut h))), ("draw", Some(1)));

    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "true"}})).expect("finger drawing");
    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("eraser");
    settle(&mut h);
    let mid = inspect(&mut h);
    assert!(require_finger_drawing(&mid));
    assert_eq!(ink_pointer(&mid), "erase");

    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("restore");
    settle(&mut h);
    assert_eq!((ink_pointer(&inspect(&mut h)).as_str(), ink_nib(&inspect(&mut h))), ("draw", Some(1)));
}

// ============================================================================
// Tier 4: Real-World Application Scenarios (S1, S2, S3, S4, S6 from TEST_INFRA.md)
// ============================================================================

#[test]
fn t4_s1_lecture_note_taking_tap_type_stylus_eraser_undo_save_reload() {
    let t = TempDir::new("t4-s1-lecture");
    let mut s1 = session(&t);
    let sample = create_sample(&mut s1, Some(LOGO_PNG)).expect("sample");
    let mut app1 = NocturneApp::new(s1);
    app1.set_redact_paths(true);

    let mut h = desktop_harness(app1);
    settle(&mut h);
    let page_path = page_of(&mut h, &sample, "Lecture1", "Intro topic");

    // 1. Tap body to type with keyboard & formatting bar.
    let rect = h.get_by_label("Intro topic").rect();
    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);
    inject_primary_touch_tap(&mut h, 1, rect.center(), None);
    ask(&mut h, "ui.key", json!({"key": "End"})).expect("End");
    ask(&mut h, "ui.text", json!({"text": " notes"})).expect("type notes");
    settle(&mut h);
    click_docked_bar_button(&mut h, &["Checkbox", "To Do", "To-do", "To Do Tag"], KEYBOARD_INSET_BOTTOM);

    // 2. Dismiss keyboard, switch to stylus draw (2 strokes).
    set_keyboard_inset(&mut h, 0.0);
    let origin = page_origin(&mut h);
    inject_primary_touch_drag(&mut h, 2, origin + egui::vec2(460.0, 180.0), origin + egui::vec2(560.0, 180.0), 5, Some(0.6));
    inject_primary_touch_drag(&mut h, 3, origin + egui::vec2(460.0, 260.0), origin + egui::vec2(560.0, 260.0), 5, Some(0.6));
    assert_eq!(ink_strokes(&inspect(&mut h)), 2);

    // 3. Double-tap eraser toggle to erase first stroke and toggle back.
    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("erase");
    settle(&mut h);
    inject_primary_touch_drag(&mut h, 4, origin + egui::vec2(510.0, 160.0), origin + egui::vec2(510.0, 200.0), 5, Some(0.6));
    assert_eq!(ink_strokes(&inspect(&mut h)), 1);
    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("pen");
    settle(&mut h);

    // 4. Undo/redo stroke via ribbon, save, and reload in a fresh app instance.
    h.get_by_label("Draw").click();
    settle(&mut h);
    h.get_by_label("Undo stroke").click();
    settle(&mut h);
    assert_eq!(ink_strokes(&inspect(&mut h)), 2);
    h.get_by_label("Redo stroke").click();
    settle(&mut h);
    assert_eq!(ink_strokes(&inspect(&mut h)), 1);

    h.state_mut().save_page_now();
    settle(&mut h);
    drop(h);

    let s2 = Session::new(&t.path().join("notebooks"), Settings::at(t.path().join("settings.toml"))).expect("reload");
    let mut app2 = NocturneApp::new(s2);
    app2.set_redact_paths(true);
    app2.open_path(&page_path);
    let mut h2 = desktop_harness(app2);
    settle(&mut h2);
    let reloaded = inspect(&mut h2);
    assert_eq!(ink_strokes(&reloaded), 1, "ink stroke persisted across reload");
    let body = h2.state().body().unwrap_or_default().to_string();
    assert!(body.contains("- [ ]") && body.contains("Intro topic notes"), "notes persisted across reload:\n{body}");
}

#[test]
fn t4_s2_touch_only_ipad_scroll_finger_draw_and_tap_to_edit() {
    let (_t, app, s) = sample_app("t4-s2-touch-only");
    let mut h = ipad_portrait_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "TouchOnly", &format!("EditTargetLine\n\n{}", scrollable_body()));

    // 1. Scroll down with finger (no Pencil).
    assert!(!require_finger_drawing(&inspect(&mut h)));
    let page_rect = h.state().page_rect().expect("page_rect");
    let origin0 = page_origin(&mut h);
    let scroll_x = page_rect.center().x;
    let down_from = egui::pos2(scroll_x, (page_rect.min.y + 540.0).min(page_rect.max.y - 40.0));
    let down_to = egui::pos2(scroll_x, page_rect.min.y + 240.0);
    assert!(page_rect.contains(down_from) && page_rect.contains(down_to), "step 1 scroll must stay inside page_rect {page_rect:?}");
    inject_primary_touch_drag(&mut h, 1, down_from, down_to, 8, None);
    let origin1 = page_origin(&mut h);
    assert!(origin1.y < origin0.y, "step 1 finger scroll must move page upward: {origin0:?} -> {origin1:?}");
    assert_eq!(ink_strokes(&inspect(&mut h)), 0);

    // 2. Enable Finger drawing, draw 2 finger strokes inside page_rect.
    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "true"}})).expect("on");
    settle(&mut h);
    let stroke_x0 = page_rect.min.x + 140.0;
    let stroke_x1 = stroke_x0 + 120.0;
    inject_primary_touch_drag(&mut h, 2, egui::pos2(stroke_x0, page_rect.min.y + 120.0), egui::pos2(stroke_x1, page_rect.min.y + 140.0), 6, None);
    inject_primary_touch_drag(&mut h, 3, egui::pos2(stroke_x0, page_rect.min.y + 180.0), egui::pos2(stroke_x1, page_rect.min.y + 200.0), 6, None);
    assert_eq!(ink_strokes(&inspect(&mut h)), 2);

    // 3. Disable Finger drawing, scroll back up inside page_rect so EditTargetLine is back in view.
    ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "false"}})).expect("off");
    settle(&mut h);
    let up_from = egui::pos2(scroll_x, page_rect.min.y + 120.0);
    let up_to = egui::pos2(scroll_x, (page_rect.min.y + 540.0).min(page_rect.max.y - 40.0));
    assert!(page_rect.contains(up_from) && page_rect.contains(up_to), "step 3 scroll must stay inside page_rect {page_rect:?}");
    inject_primary_touch_drag(&mut h, 4, up_from, up_to, 8, None);
    let origin2 = page_origin(&mut h);
    assert!(origin2.y > origin1.y, "step 3 finger scroll must bring EditTargetLine back into view: {origin1:?} -> {origin2:?}");
    assert_eq!(ink_strokes(&inspect(&mut h)), 2);

    // 4. Tap text to enter Write and type.
    let target = h.get_by_label("EditTargetLine").rect();
    inject_primary_touch_tap(&mut h, 5, target.center(), None);
    assert_eq!(ink_pointer(&inspect(&mut h)), "write");
    ask(&mut h, "ui.key", json!({"key": "End"})).expect("End");
    ask(&mut h, "ui.text", json!({"text": " TouchEdited"})).expect("type");
    settle(&mut h);
    assert!(h.state().body().unwrap_or_default().contains("EditTargetLine TouchEdited"));
}

#[test]
fn t4_s3_outliner_and_task_workflow_with_docked_formatting_bar() {
    let t = TempDir::new("t4-s3-outliner");
    let mut s1 = session(&t);
    let sample = create_sample(&mut s1, Some(LOGO_PNG)).expect("sample");
    let mut app = NocturneApp::new(s1);
    app.set_redact_paths(true);

    let mut h = tablet_harness(app);
    settle(&mut h);
    let page_path = page_of(&mut h, &sample, "Outliner", "- Top goal\n- Subtask");

    set_keyboard_inset(&mut h, KEYBOARD_INSET_BOTTOM);
    ask(&mut h, "ui.caret", json!({"block": 1})).expect("caret block 1");
    settle(&mut h);

    // Select Subtask, Bold it, Indent it, turn into Checkbox, Undo/Redo check, then Outdent + Indent.
    ask(&mut h, "ui.key", json!({"key": "Home"})).expect("Home");
    ask(&mut h, "ui.key", json!({"key": "End", "shift": true})).expect("Shift+End");
    settle(&mut h);

    click_docked_bar_button(&mut h, &["Bold"], KEYBOARD_INSET_BOTTOM);
    click_docked_bar_button(&mut h, &["Indent", "Increase indentation"], KEYBOARD_INSET_BOTTOM);
    click_docked_bar_button(&mut h, &["Checkbox", "To Do", "To-do", "To Do Tag"], KEYBOARD_INSET_BOTTOM);
    click_docked_bar_button(&mut h, &["Outdent", "Decrease indentation"], KEYBOARD_INSET_BOTTOM);
    click_docked_bar_button(&mut h, &["Indent", "Increase indentation"], KEYBOARD_INSET_BOTTOM);
    let before_undo = h.state().body().unwrap_or_default().to_string();
    click_docked_bar_button(&mut h, &["Undo"], KEYBOARD_INSET_BOTTOM);
    let after_undo = h.state().body().unwrap_or_default().to_string();
    assert_ne!(after_undo, before_undo, "docked Undo must revert the prior edit before Redo:\n{after_undo}");
    click_docked_bar_button(&mut h, &["Redo"], KEYBOARD_INSET_BOTTOM);
    let after_redo = h.state().body().unwrap_or_default().to_string();
    assert_eq!(after_redo, before_undo, "docked Redo must restore the undone edit:\n{after_redo}");

    h.state_mut().save_page_now();
    settle(&mut h);
    let md = std::fs::read_to_string(std::path::Path::new(&page_path).join("index.md")).expect("read index.md");
    assert!(md.contains("**Subtask**"), "persisted markdown must keep bold:\n{md}");
    assert!(md.contains("[ ]"), "persisted markdown must keep checkbox:\n{md}");
    assert!(
        md.lines().any(|l| (l.starts_with("  -") || l.starts_with("    -") || l.starts_with("\t-")) && l.contains("Subtask")),
        "persisted markdown must keep indented task:\n{md}"
    );
}

#[test]
fn t4_s4_windows_2in1_hybrid_mouse_stylus_zoom_and_shortcuts() {
    let board = MemoryBoard::new();
    let (_t, app, s) = sample_app_with_board("t4-s4-hybrid", &board);
    let mut h = desktop_harness(app);
    settle(&mut h);
    page_of(&mut h, &s, "WinHybrid", "HybridLineAlpha\n\nHybridLineBeta");

    // 1. Mouse drag-select across lines & copy, then focus block 0 & bold with Ctrl+B.
    let r = h.get_by_label("HybridLineAlpha").rect();
    let r2 = h.get_by_label("HybridLineBeta").rect();
    inject_mouse_drag(&mut h, r.left_center() + egui::vec2(5.0, 0.0), r2.center(), "left");
    press_copy(&mut h, &board, "C");
    assert!(!board.read_text().is_empty(), "MemoryBoard received copy after cross-block drag");
    ask(&mut h, "ui.caret", json!({"block": 0})).expect("caret 0");
    settle(&mut h);
    ask(&mut h, "ui.key", json!({"key": "Home"})).expect("Home");
    settle(&mut h);
    ask(&mut h, "ui.key", json!({"key": "End", "shift": true})).expect("Shift+End");
    settle(&mut h);
    ask(&mut h, "ui.key", json!({"key": "B", "ctrl": true})).expect("Ctrl+B");
    settle(&mut h);
    assert!(h.state().body().unwrap_or_default().contains("**"));

    // 2. Stylus annotate ink + eraser toggle.
    let origin = page_origin(&mut h);
    inject_primary_touch_drag(&mut h, 1, origin + egui::vec2(450.0, 220.0), origin + egui::vec2(560.0, 240.0), 6, Some(0.65));
    assert_eq!(ink_strokes(&inspect(&mut h)), 1);
    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("eraser");
    settle(&mut h);
    assert_eq!(ink_pointer(&inspect(&mut h)), "erase");
    ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("pen");
    settle(&mut h);

    // 3. View zoom + Ctrl+Z / Ctrl+Y.
    h.get_by_label("View").click();
    settle(&mut h);
    h.get_by_label("Zoom In").click();
    settle(&mut h);
    assert_eq!(h.state().zoom_percent(), 110);

    ask(&mut h, "ui.key", json!({"key": "z", "ctrl": true})).expect("Ctrl+Z");
    settle(&mut h);
    assert_eq!(ink_strokes(&inspect(&mut h)), 0, "Ctrl+Z must undo the stylus stroke before Ctrl+Y redoes it");
    ask(&mut h, "ui.key", json!({"key": "y", "ctrl": true})).expect("Ctrl+Y");
    settle(&mut h);
    assert_eq!(ink_strokes(&inspect(&mut h)), 1);
}

#[test]
fn t4_s6_multi_tool_palette_round_trip_pen_highlighter_pencil_eraser() {
    let (_t, mut app, s) = sample_app("t4-s6-palette");
    app.open_path(&s.md_page.to_string_lossy());
    let mut h = desktop_harness(app);
    settle(&mut h);
    h.get_by_label("Draw").click();
    settle(&mut h);

    let origin = page_origin(&mut h);
    let tools = [("Pen black", "pen"), ("Pen red", "pen"), ("Highlighter yellow", "highlighter"), ("Pencil grey", "pencil")];

    for (idx, (label, kind)) in tools.into_iter().enumerate() {
        h.get_by_label(label).click();
        settle(&mut h);
        let nib = ink_nib(&inspect(&mut h));
        assert_eq!(ink_tool(&inspect(&mut h)), kind);

        ask(&mut h, "engine.execute", json!({"command": "ink.finger_drawing", "params": {"enabled": "toggle"}})).expect("toggle finger");
        ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("to eraser");
        settle(&mut h);
        assert_eq!(ink_pointer(&inspect(&mut h)), "erase");

        ask(&mut h, "engine.execute", json!({"command": "ink.toggle_eraser"})).expect("back to tool");
        settle(&mut h);
        let restored = inspect(&mut h);
        assert_eq!(ink_pointer(&restored), "draw");
        assert_eq!(ink_nib(&restored), nib, "{label} nib must match after eraser round-trip");
        assert_eq!(ink_tool(&restored), kind);

        let y = 180.0 + idx as f32 * 35.0;
        inject_primary_touch_drag(&mut h, (idx + 1) as u64, origin + egui::vec2(460.0, y), origin + egui::vec2(550.0, y), 4, Some(0.6));
        assert_eq!(ink_strokes(&inspect(&mut h)), (idx + 1) as u64);
    }
}
