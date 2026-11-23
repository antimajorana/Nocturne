//! iPad Studio UX v2, phase 4A: lasso handwritten mathematics to LaTeX, single-line editable
//! RaTeX preview, positioned `Block::MathBlock` insertion with one-step Undo/Redo, Tool Settings
//! toggle and key-safe service setup, and CMGFM/HTML parity against a real loopback HTTP server.

#[allow(dead_code)]
mod support;

use egui::accesskit::Role;
use egui::os::OperatingSystem;
use egui::{Key, Modifiers, MouseWheelSource, MouseWheelUnit, Pos2, Rect, TouchPhase, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use nocturne_engine::{Session, ocr::OcrProvider};
use nocturne_formats::{boxes, markdown::Block};
use nocturne_ink::{Dab, Rgb, Stroke, Tool};
use nocturne_model::{Container, PageFormat};
use nocturne_store::{Settings, page::load_page, testing::TempDir};
use nocturne_ui_egui::inkpad::{Pad, held_area};
use nocturne_ui_egui::math_ocr::{TEX_FIELD, typesets};
use nocturne_ui_egui::theme::{Theme, contrast};
use nocturne_ui_egui::{NocturneApp, ribbon};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::{thread, time::Duration};

const SCREEN: egui::Vec2 = vec2(1180.0, 820.0);

struct Server {
    endpoint: String,
    seen: Arc<Mutex<usize>>,
    replies: Arc<Mutex<VecDeque<(u16, String)>>>,
    gate: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        listener.set_nonblocking(true).expect("nonblocking");
        let endpoint = format!("http://127.0.0.1:{}", listener.local_addr().expect("addr").port());
        let (seen, replies) = (Arc::new(Mutex::new(0usize)), Arc::new(Mutex::new(VecDeque::new())));
        let (gate, stop) = (Arc::new(AtomicBool::new(true)), Arc::new(AtomicBool::new(false)));
        let (seen_bg, replies_bg, gate_bg, stop_bg) = (Arc::clone(&seen), Arc::clone(&replies), Arc::clone(&gate), Arc::clone(&stop));
        let handle = thread::spawn(move || {
            while !stop_bg.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).expect("blocking stream");
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                let (mut line, mut length) = (String::new(), 0usize);
                let _ = reader.read_line(&mut line);
                while {
                    line.clear();
                    reader.read_line(&mut line).unwrap_or(0) > 0 && !line.trim_end().is_empty()
                } {
                    if let Some((k, v)) = line.trim_end().split_once(':')
                        && k.eq_ignore_ascii_case("content-length")
                    {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let _ = reader.read_exact(&mut vec![0u8; length]);
                *seen_bg.lock().expect("seen") += 1;
                while !gate_bg.load(Ordering::Acquire) && !stop_bg.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(5));
                }
                let (status, text) = replies_bg.lock().expect("replies").pop_front().unwrap_or((500, "{}".to_string()));
                let msg = format!(
                    "HTTP/1.1 {status} Reply\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
                let _ = stream.write_all(msg.as_bytes()).and_then(|()| stream.flush());
            }
        });
        Self { endpoint, seen, replies, gate, stop, handle: Some(handle) }
    }

    fn reply(&self, status: u16, body: Value) {
        self.replies.lock().expect("replies").push_back((status, body.to_string()));
    }

    fn reply_openai(&self, status: u16, content: &str) {
        self.reply(status, json!({ "choices": [{ "finish_reason": "stop", "message": { "content": content, "refusal": null } }] }));
    }

    fn set_gate(&self, open: bool) {
        self.gate.store(open, Ordering::Release);
    }

    fn count(&self) -> usize {
        *self.seen.lock().expect("seen")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.gate.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn studio_page(name: &str, format: PageFormat) -> (TempDir, Harness<'static, NocturneApp>, PathBuf) {
    let tmp = TempDir::new(name);
    let settings = Settings::at(tmp.path().join("cfg/settings.toml"));
    settings.set("privacy_welcome_seen", "true").and_then(|()| settings.set_default_format(format)).expect("settings");
    let mut session = Session::new(&tmp.path().join("notebooks"), settings).expect("session");
    session.run("notebook.create", &[("name".into(), "Math Notes".into())].into_iter().collect()).expect("create notebook");
    let page = session.page_to_show("math-notes").expect("first page");
    session.run("page.save", &[("path".into(), page.display().to_string()), ("body".into(), String::new())].into_iter().collect()).expect("blank");
    let mut app = NocturneApp::new(session);
    app.open_path(&page.to_string_lossy());
    let base = Harness::builder().with_size(SCREEN).with_accessibility_check(false);
    let mut h = if std::env::var_os("NOCTURNE_UI_VISUAL_DIR").is_some() {
        base.renderer(support::gpu::renderer()).build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app)
    } else {
        base.build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app)
    };
    h.ctx.set_os(OperatingSystem::IOS);
    steps(&mut h, 4);
    assert!(h.state().is_tablet_studio(&h.ctx));
    (tmp, h, page)
}

fn steps(h: &mut Harness<'_, NocturneApp>, count: usize) {
    for _ in 0..count {
        let events = h.state_mut().take_events();
        h.input_mut().events.extend(events);
        h.step();
    }
}

fn wait_until(h: &mut Harness<'_, NocturneApp>, what: &str, mut ready: impl FnMut(&Harness<'_, NocturneApp>) -> bool) {
    for _ in 0..120 {
        steps(h, 1);
        if ready(h) {
            steps(h, 2);
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what} (status: {:?})", h.state().status());
}

fn run(h: &mut Harness<'_, NocturneApp>, command: &str, params: &[(&str, &str)]) {
    let params = params.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
    h.state_mut().session_mut().run(command, &params).unwrap_or_else(|e| panic!("{command}: {e}"));
    steps(h, 3);
}

fn enable_openai_ocr(h: &mut Harness<'_, NocturneApp>, server: &Server, key: &str) {
    run(h, "ink.math_ocr_toggle", &[("enabled", "true")]);
    run(h, "ink.math_ocr_configure", &[("provider", "openai"), ("endpoint", &server.endpoint), ("key", key)]);
}

fn node_rect(node: &egui_kittest::Node<'_>, ppp: f32) -> Option<Rect> {
    let b = node.accesskit_node().bounding_box()?;
    let rect = Rect::from_min_max(pos2(b.x0 as f32 / ppp, b.y0 as f32 / ppp), pos2(b.x1 as f32 / ppp, b.y1 as f32 / ppp));
    rect.is_positive().then_some(rect)
}

fn named(h: &Harness<'_, NocturneApp>, name: &str) -> Option<Rect> {
    let ppp = h.ctx.pixels_per_point();
    h.query_all(egui_kittest::kittest::by()).find_map(|node| {
        let ak = node.accesskit_node();
        let text = ak.label().or_else(|| (ak.role() == Role::Label).then(|| ak.value()).flatten())?;
        text.replace('\n', " ").trim().eq_ignore_ascii_case(name).then(|| node_rect(&node, ppp))?
    })
}

fn area(h: &Harness<'_, NocturneApp>, id: &str) -> Option<Rect> {
    h.ctx.memory(|m| m.areas().visible_layer_ids().iter().any(|l| l.id == egui::Id::unique(id)).then(|| m.area_rect(egui::Id::unique(id))).flatten())
}

fn button(pressed: bool, pos: Pos2) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

fn click(h: &mut Harness<'_, NocturneApp>, at: Pos2) {
    h.input_mut().events.extend([egui::Event::PointerMoved(at), button(true, at)]);
    steps(h, 1);
    h.input_mut().events.push(button(false, at));
    steps(h, 3);
}

fn click_named(h: &mut Harness<'_, NocturneApp>, label: &str) {
    click(h, named(h, label).unwrap_or_else(|| panic!("missing button {label:?}")).center());
}

fn type_replace(h: &mut Harness<'_, NocturneApp>, at: Pos2, text: &str) {
    let k = |key, pressed, modifiers| egui::Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers };
    click(h, at);
    h.input_mut().events.extend([
        k(Key::A, true, Modifiers::COMMAND),
        k(Key::A, false, Modifiers::COMMAND),
        k(Key::Backspace, true, Modifiers::NONE),
        k(Key::Backspace, false, Modifiers::NONE),
    ]);
    steps(h, 1);
    h.input_mut().events.push(egui::Event::Text(text.to_string()));
    steps(h, 3);
}

fn page_pos(h: &Harness<'_, NocturneApp>, px: f32, py: f32) -> Pos2 {
    h.state().editor_origin().expect("editor origin") + vec2(px, py)
}

fn drag_screen(h: &mut Harness<'_, NocturneApp>, path: &[Pos2]) {
    h.input_mut().events.extend([egui::Event::PointerMoved(path[0]), button(true, path[0])]);
    steps(h, 1);
    for &pt in &path[1..] {
        h.input_mut().events.push(egui::Event::PointerMoved(pt));
        steps(h, 1);
    }
    h.input_mut().events.push(button(false, path[path.len() - 1]));
    steps(h, 3);
}

fn draw_page_line(h: &mut Harness<'_, NocturneApp>, x0: f32, y0: f32, x1: f32, y1: f32) {
    h.state_mut().ribbon_act_for_test(ribbon::Act::InkPen(0));
    steps(h, 2);
    let (a, b) = (page_pos(h, x0, y0), page_pos(h, x1, y1));
    drag_screen(h, &(0..=14).map(|i| a.lerp(b, i as f32 / 14.0)).collect::<Vec<_>>());
}

fn draw_plus(h: &mut Harness<'_, NocturneApp>, cx: f32, cy: f32) {
    draw_page_line(h, cx - 24.0, cy, cx + 24.0, cy);
    draw_page_line(h, cx, cy - 24.0, cx, cy + 24.0);
}

fn lasso_page_rect(h: &mut Harness<'_, NocturneApp>, x0: f32, y0: f32, x1: f32, y1: f32) {
    h.state_mut().ribbon_act_for_test(ribbon::Act::InkPointer(ribbon::Pointer::Lasso));
    steps(h, 2);
    let c = [page_pos(h, x0, y0), page_pos(h, x1, y0), page_pos(h, x1, y1), page_pos(h, x0, y1), page_pos(h, x0, y0)];
    let mut pts: Vec<Pos2> = c.windows(2).flat_map(|s| (0..4).map(move |i| s[0].lerp(s[1], i as f32 / 4.0))).collect();
    pts.push(c[4]);
    drag_screen(h, &pts);
}

fn convert_and_insert(h: &mut Harness<'_, NocturneApp>, x0: f32, y0: f32, x1: f32, y1: f32) {
    lasso_page_rect(h, x0, y0, x1, y1);
    assert_eq!(h.state().held_ink_count(), 2);
    click_named(h, "Convert to LaTeX");
    wait_until(h, "preview modal", |h| named(h, "Insert equation").is_some());
    click_named(h, "Insert equation");
    h.state_mut().save_open_page();
}

fn click_in_tool_settings(h: &mut Harness<'_, NocturneApp>, name: &str) {
    if area(h, "studio-tool-settings").is_none() {
        click_named(h, "Tool settings");
    }
    for _ in 0..12 {
        let screen = area(h, "studio-tool-settings").expect("Tool Settings is open");
        let at = named(h, name).unwrap_or_else(|| panic!("{name} is in Tool Settings"));
        if screen.shrink(6.0).contains_rect(at) {
            click(h, at.center());
            return;
        }
        let delta = vec2(0.0, if at.center().y > screen.center().y { -90.0 } else { 90.0 });
        let wheel = egui::Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta,
            phase: TouchPhase::Move,
            source: MouseWheelSource::Wheel,
            modifiers: Modifiers::NONE,
        };
        h.input_mut().events.extend([egui::Event::PointerMoved(screen.center()), wheel]);
        steps(h, 3);
    }
    panic!("{name} could not be scrolled into Tool Settings");
}

fn setup_inputs(h: &Harness<'_, NocturneApp>) -> Vec<(Rect, String)> {
    let (within, ppp) = (area(h, "math-ocr-setup").expect("math-ocr-setup open").expand(2.0), h.ctx.pixels_per_point());
    let mut out: Vec<(Rect, String)> = h
        .query_all(egui_kittest::kittest::by())
        .filter_map(|node| {
            let ak = node.accesskit_node();
            let r = matches!(ak.role(), egui::accesskit::Role::TextInput | egui::accesskit::Role::PasswordInput).then(|| node_rect(&node, ppp))??;
            within.contains_rect(r).then(|| (r, ak.value().unwrap_or_default().to_string()))
        })
        .collect();
    out.sort_by(|a, b| a.0.center().y.total_cmp(&b.0.center().y));
    out
}

fn assert_not_leaked(h: &Harness<'_, NocturneApp>, secret: &str) {
    assert!(!h.state().status().contains(secret), "status leaked {secret}");
    for node in h.query_all(egui_kittest::kittest::by()) {
        let ak = node.accesskit_node();
        assert!(!ak.label().unwrap_or_default().contains(secret) && !ak.value().unwrap_or_default().contains(secret), "UI leaked {secret}");
    }
}

fn read_arranged_page(page_dir: &Path) -> (Vec<Block>, Vec<Container>, String) {
    let loaded = load_page(page_dir).expect("load_page");
    let (blocks, bx) = boxes::read(loaded.format, &loaded.body);
    (blocks, bx.expect("arranged containers"), loaded.body)
}

fn make_stroke(x0: f32, x1: f32, y: f32) -> Stroke {
    let mut s = Stroke::new(Tool::Pen, Rgb::BLACK, 3.0);
    s.add(Dab::new(x0, y, 0.7));
    s.add(Dab::new(x1, y, 0.7));
    s
}

fn maybe_save_visual(h: &mut Harness<'_, NocturneApp>, stem: &str) {
    if let Some(dir) = std::env::var_os("NOCTURNE_UI_VISUAL_DIR").map(PathBuf::from) {
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(img) = support::gpu::render(h) {
            let _ = img.save(dir.join(format!("{stem}.png")));
        }
    }
}

#[test]
fn pad_held_strokes_bounds_and_exact_snapshot_removal() {
    let tmp = TempDir::new("p4-pad-snapshot");
    let mut pad = Pad::of_page(&tmp.path().join("page"));
    assert!(pad.held_strokes().is_empty() && pad.held_bounds().is_none() && !pad.remove_exact_strokes(&[]));
    for s in [make_stroke(40.0, 100.0, 50.0), make_stroke(40.0, 100.0, 50.0), make_stroke(240.0, 300.0, 180.0)] {
        pad.drawing.add(s);
    }
    let all = pad.drawing.strokes.clone();
    pad.hold_touching((30.0, 40.0, 110.0, 60.0));
    assert_eq!((pad.holding(), pad.held_strokes()), (2, vec![all[0].clone(), all[1].clone()]));
    let hb = pad.held_bounds().expect("held bounds");
    assert!((hb[0] - 38.5).abs() < 0.2 && (hb[1] - 48.5).abs() < 0.2 && (hb[2] - 101.5).abs() < 0.2 && (hb[3] - 51.5).abs() < 0.2, "{hb:?}");

    assert!(!pad.remove_exact_strokes(&[all[0].clone(), all[0].clone(), all[0].clone()]) && pad.strokes() == 3);
    assert!(pad.remove_exact_strokes(std::slice::from_ref(&all[0])));
    assert_eq!((pad.holding(), pad.drawing.strokes.as_slice()), (0, [all[1].clone(), all[2].clone()].as_slice()));

    let snap = pad.drawing.strokes.clone();
    pad.hold_touching((30.0, 40.0, 110.0, 60.0));
    pad.move_held(8.0, 0.0);
    let moved = pad.drawing.strokes.clone();
    assert!(!pad.remove_exact_strokes(&snap) && pad.drawing.strokes == moved);
    pad.drawing.strokes = snap.clone();
    pad.drawing.strokes[1].width += 1.5;
    let altered = pad.drawing.strokes.clone();
    assert!(!pad.remove_exact_strokes(&snap) && pad.drawing.strokes == altered);
    pad.drawing.strokes = vec![snap[1].clone()];
    assert!(!pad.remove_exact_strokes(&snap) && pad.drawing.strokes == vec![snap[1].clone()]);
}

#[test]
fn disabled_by_default_shows_no_convert_button_and_sends_zero_requests() {
    let server = Server::start();
    let (_tmp, mut h, _page_dir) = studio_page("p4-disabled-default", PageFormat::Cmgfm);
    assert!(!h.state().session().ocr_config().enabled);
    run(&mut h, "ink.math_ocr_configure", &[("provider", "openai"), ("endpoint", &server.endpoint), ("key", "sk-disabled-key")]);
    assert!(!h.state().session().ocr_config().enabled);

    draw_plus(&mut h, 200.0, 220.0);
    assert_eq!(h.state().ink_stroke_count(), 2);
    let t0 = h.ctx.input(|inp| inp.time);
    for i in 1..=8 {
        h.input_mut().time = Some(t0 + f64::from(i) * 0.5);
        steps(&mut h, 1);
    }
    assert!(area(&h, "math-ocr-convert").is_none() && named(&h, "Convert to LaTeX").is_none(), "no auto-popup pause-suggest chip");

    lasso_page_rect(&mut h, 160.0, 180.0, 240.0, 260.0);
    assert!(h.state().held_ink_count() > 0 && held_area(&h.ctx).is_some());
    steps(&mut h, 6);
    assert!(area(&h, "math-ocr-convert").is_none() && named(&h, "Convert to LaTeX").is_none() && server.count() == 0);
}

#[test]
fn tool_settings_toggles_math_ocr_and_saves_service_without_leaking_keys() {
    const SECRET: &str = "sk-phase4-secret-never-echoed";
    let server = Server::start();
    let (_tmp, mut h, _page_dir) = studio_page("p4-tool-settings", PageFormat::Cmgfm);
    click_in_tool_settings(&mut h, "Recognise handwritten equations");
    assert!(h.state().session().ocr_config().enabled);

    click_in_tool_settings(&mut h, "Recognition service...");
    assert!(area(&h, "math-ocr-setup").is_some());
    click_named(&mut h, "OpenAI");
    assert!(named(&h, "No key is kept for this service.").is_some());

    let inputs = setup_inputs(&h);
    assert!(inputs.len() >= 2);
    type_replace(&mut h, inputs[0].0.center(), &server.endpoint);
    let inputs = setup_inputs(&h);
    type_replace(&mut h, inputs[inputs.len() - 1].0.center(), SECRET);
    click_named(&mut h, "Save");

    assert!(h.state().session().math_ocr_ready() && h.state().session().ocr_config().provider == OcrProvider::OpenAi);
    assert!(!std::fs::read_to_string(h.state().session().settings.path()).unwrap_or_default().contains(SECRET));
    let job = h.state().session().math_ocr_job(&[make_stroke(10.0, 30.0, 10.0)]).expect("ocr job");
    assert!(!format!("{job:?}").contains(SECRET), "OcrJob Debug must never echo secret");
    assert_not_leaked(&h, SECRET);

    click_in_tool_settings(&mut h, "Recognition service...");
    assert!(named(&h, "A key is kept for this service.").is_some());
    assert!(setup_inputs(&h).last().expect("password input").1.is_empty(), "password buffer must reopen empty");
    assert_not_leaked(&h, SECRET);

    click_named(&mut h, "Forget key");
    assert!(!h.state().session().math_ocr_ready() && named(&h, "No key is kept for this service.").is_some());
}

#[test]
fn convert_edit_and_insert_replaces_all_ink_at_ink_origin_with_one_step_undo_redo() {
    let server = Server::start();
    server.reply_openai(200, "$$\\frac{a}{b}$$");
    let (_tmp, mut h, page_dir) = studio_page("p4-convert-all-ink", PageFormat::Cmgfm);
    enable_openai_ocr(&mut h, &server, "sk-phase4-convert");

    draw_plus(&mut h, 210.0, 220.0);
    assert_eq!(h.state().ink_stroke_count(), 2);
    let mut disk_pad = Pad::of_page(&page_dir);
    disk_pad.hold_touching((170.0, 180.0, 250.0, 260.0));
    let [ink_x0, ink_y0, _, _] = disk_pad.held_bounds().expect("held_bounds");

    lasso_page_rect(&mut h, 170.0, 180.0, 250.0, 260.0);
    assert!(h.state().held_ink_count() == 2 && area(&h, "math-ocr-convert").is_some());
    maybe_save_visual(&mut h, "d4_lasso_convert_chip");

    click_named(&mut h, "Convert to LaTeX");
    wait_until(&mut h, "preview modal", |h| named(h, "Insert equation").is_some());
    assert_eq!(server.count(), 1);
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(egui::Id::unique(TEX_FIELD)));
    assert_eq!(h.state().focused_text_field(&h.ctx).map(|f| f.text), Some("\\frac{a}{b}".to_string()));
    assert!(!h.query_all_by_role(Role::MultilineTextInput).any(|n| n.value().as_deref() == Some("\\frac{a}{b}")), "single-line");
    let tex_rect = h
        .query_all_by_role(Role::TextInput)
        .find_map(|n| (n.value().as_deref() == Some("\\frac{a}{b}")).then(|| node_rect(&n, h.ctx.pixels_per_point()))?)
        .expect("single-line TEX_FIELD");
    type_replace(&mut h, tex_rect.center(), "\\frac{a}{b} + 1");

    let (check_ctx, mut ratex_ok) = (egui::Context::default(), false);
    let mut out = check_ctx.run_ui(egui::RawInput::default(), |ui| ratex_ok = typesets(ui, "\\frac{a}{b} + 1"));
    out.textures_delta.clear();
    assert!(ratex_ok, "edited TeX typesets through RaTeX");
    maybe_save_visual(&mut h, "d4_ratex_preview_modal");

    click_named(&mut h, "Insert equation");
    h.state_mut().save_open_page();
    assert!(h.state().ink_stroke_count() == 0 && !page_dir.join("ink.svg").exists());
    let (blocks, boxes, _) = read_arranged_page(&page_dir);
    assert_eq!(blocks, vec![Block::MathBlock("\\frac{a}{b} + 1".to_string())]);
    assert_eq!(boxes.len(), 1, "zero 0-block ghost containers: {boxes:?}");
    assert_eq!((boxes[0].x, boxes[0].y, boxes[0].blocks), ((ink_x0 - 6.0).max(0.0).round(), (ink_y0 - 14.0).max(0.0).round(), 1));

    let has_ink = |b: &[Block]| b.iter().any(|x| matches!(x, Block::Image { src, .. } if src == "ink.svg"));
    let has_math = |b: &[Block]| b.iter().any(|x| matches!(x, Block::MathBlock(_)));
    click_named(&mut h, "Undo");
    h.state_mut().save_open_page();
    assert!(h.state().ink_stroke_count() == 2 && page_dir.join("ink.svg").is_file());
    assert!(has_ink(&h.state().editor_blocks()) && !has_math(&h.state().editor_blocks()));

    click_named(&mut h, "Redo");
    h.state_mut().save_open_page();
    let (re_blocks, re_boxes, _) = read_arranged_page(&page_dir);
    assert!(h.state().ink_stroke_count() == 0 && !page_dir.join("ink.svg").exists());
    assert_eq!((re_blocks, re_boxes), (vec![Block::MathBlock("\\frac{a}{b} + 1".to_string())], boxes));
}

#[test]
fn partial_lasso_keeps_remaining_ink_reference_and_undoes_in_one_step() {
    let server = Server::start();
    server.reply_openai(200, "x^2 + y^2");
    let (_tmp, mut h, page_dir) = studio_page("p4-partial-lasso", PageFormat::Cmgfm);
    enable_openai_ocr(&mut h, &server, "sk-phase4-partial");
    draw_plus(&mut h, 190.0, 220.0);
    draw_page_line(&mut h, 520.0, 380.0, 600.0, 380.0);
    assert_eq!(h.state().ink_stroke_count(), 3);
    convert_and_insert(&mut h, 150.0, 180.0, 230.0, 260.0);

    let has_ink = |b: &[Block]| b.iter().any(|x| matches!(x, Block::Image { src, .. } if src == "ink.svg"));
    let (blocks, boxes, _) = read_arranged_page(&page_dir);
    assert!(h.state().ink_stroke_count() == 1 && page_dir.join("ink.svg").is_file());
    assert!(has_ink(&blocks) && blocks.contains(&Block::MathBlock("x^2 + y^2".to_string())));
    assert_eq!(boxes.iter().map(|c| c.blocks).sum::<usize>(), blocks.len());

    click_named(&mut h, "Undo");
    h.state_mut().save_open_page();
    assert!(h.state().ink_stroke_count() == 3 && page_dir.join("ink.svg").is_file());
    assert!(!h.state().editor_blocks().iter().any(|b| matches!(b, Block::MathBlock(_))));

    click_named(&mut h, "Redo");
    h.state_mut().save_open_page();
    let after = h.state().editor_blocks();
    assert!(h.state().ink_stroke_count() == 1 && page_dir.join("ink.svg").is_file());
    assert!(has_ink(&after) && after.contains(&Block::MathBlock("x^2 + y^2".to_string())));
}

#[test]
fn ink_changed_mid_request_replaces_nothing() {
    let server = Server::start();
    server.set_gate(false);
    server.reply_openai(200, "a + b");
    let (_tmp, mut h, page_dir) = studio_page("p4-mid-request", PageFormat::Cmgfm);
    enable_openai_ocr(&mut h, &server, "sk-phase4-gate");
    draw_page_line(&mut h, 90.0, 130.0, 170.0, 130.0);
    draw_page_line(&mut h, 90.0, 190.0, 170.0, 190.0);
    lasso_page_rect(&mut h, 70.0, 110.0, 190.0, 210.0);
    assert_eq!(h.state().held_ink_count(), 2);
    click_named(&mut h, "Convert to LaTeX");
    wait_until(&mut h, "in-flight request", |_| server.count() == 1);
    assert!(named(&h, "Reading the handwriting...").is_some());

    h.state_mut().ribbon_act_for_test(ribbon::Act::InkPointer(ribbon::Pointer::Erase));
    steps(&mut h, 2);
    let (ea, eb) = (page_pos(&h, 130.0, 112.0), page_pos(&h, 130.0, 148.0));
    drag_screen(&mut h, &(0..=8).map(|i| ea.lerp(eb, i as f32 / 8.0)).collect::<Vec<_>>());
    assert_eq!(h.state().ink_stroke_count(), 1);

    server.set_gate(true);
    wait_until(&mut h, "preview after release", |h| named(h, "Insert equation").is_some());
    click_named(&mut h, "Insert equation");
    h.state_mut().save_open_page();
    assert!(h.state().ink_stroke_count() == 1 && page_dir.join("ink.svg").is_file());
    assert!(!h.state().editor_blocks().iter().any(|b| matches!(b, Block::MathBlock(_))));
    assert!(h.state().status().to_ascii_lowercase().contains("ink changed"), "status: {:?}", h.state().status());
}

#[test]
fn service_failure_shows_redacted_error_and_leaves_ink_intact() {
    const SECRET: &str = "sk-phase4-auth-secret-999";
    const LEAK: &str = "UPSTREAM_BODY_DETAIL_LEAK";
    let server = Server::start();
    let (_tmp, mut h, page_dir) = studio_page("p4-service-failure", PageFormat::Cmgfm);
    enable_openai_ocr(&mut h, &server, SECRET);

    draw_plus(&mut h, 200.0, 220.0);
    let svg_before = std::fs::read_to_string(page_dir.join("ink.svg")).expect("ink.svg");

    for (req_no, code, expected) in
        [(1, 401u16, "The recognition service did not accept the key."), (2, 500u16, "The recognition service answered with status 500.")]
    {
        server.reply(code, json!({ "error": { "message": format!("err {SECRET} {LEAK}") } }));
        lasso_page_rect(&mut h, 145.0, 165.0, 255.0, 275.0);
        click_named(&mut h, "Convert to LaTeX");
        wait_until(&mut h, "failure handled", |h| server.count() == req_no && area(h, "math-ocr-window").is_none());
        assert_eq!((h.state().status(), h.state().ink_stroke_count()), (expected, 2));
        assert_not_leaked(&h, SECRET);
        assert_not_leaked(&h, LEAK);
        assert_eq!(std::fs::read_to_string(page_dir.join("ink.svg")).expect("ink.svg"), svg_before);
        if let Some(banner) = area(&h, "studio-error-banner") {
            click(&mut h, banner.center());
        }
    }
}

#[test]
fn converted_math_block_round_trips_identically_in_cmgfm_and_html_and_passes_14_theme_contrasts() {
    let convert_in = |name: &str, format: PageFormat| {
        let server = Server::start();
        server.reply_openai(200, "\\sum_{k=1}^{n} k^2");
        let (tmp, mut h, page_dir) = studio_page(name, format);
        enable_openai_ocr(&mut h, &server, "sk-phase4-parity");
        draw_plus(&mut h, 200.0, 220.0);
        convert_and_insert(&mut h, 160.0, 180.0, 240.0, 260.0);
        let (blocks, bx, body) = read_arranged_page(&page_dir);
        let mut fresh = NocturneApp::new(Session::new(&tmp.path().join("notebooks"), Settings::at(tmp.path().join("cfg/settings.toml"))).expect("s"));
        fresh.open_path(&page_dir.to_string_lossy());
        (blocks, bx, fresh.editor_blocks(), body)
    };

    let (md_blocks, md_boxes, md_reloaded, md_body) = convert_in("p4-parity-cmgfm", PageFormat::Cmgfm);
    let (html_blocks, html_boxes, html_reloaded, html_body) = convert_in("p4-parity-html", PageFormat::Html);
    assert!(md_body.contains("$$\n\\sum_{k=1}^{n} k^2\n$$") && html_body.contains("<p class=\"math\">\\sum_{k=1}^{n} k^2</p>"));
    let want = vec![Block::MathBlock("\\sum_{k=1}^{n} k^2".to_string())];
    assert_eq!((&md_blocks, &html_blocks, &md_reloaded, &html_reloaded), (&want, &want, &want, &want));
    assert!(md_boxes.len() == 1 && md_boxes[0].blocks == 1 && md_boxes == html_boxes);

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    for rel in ["src/math_ocr.rs", "tests/ipad_studio_v2_phase4.rs"] {
        assert!((1..600).contains(&std::fs::read_to_string(manifest.join(rel)).expect(rel).lines().count()), "{rel} >= 600 lines");
    }

    let all_themes = Theme::all();
    assert_eq!(all_themes.len(), 14);
    for theme in all_themes {
        let (t, label) = (theme.tokens(), theme.label());
        for (fg, bg, min) in
            [(t.text, t.panel, 7.0), (t.muted, t.panel, 4.5), (t.strong, t.code_bg, 7.0), (t.warn, t.panel, 4.5), (t.band_text, t.band, 4.5)]
        {
            assert!(contrast(fg, bg) >= min, "{label}: contrast {:.2} < {min}", contrast(fg, bg));
        }
    }
}
