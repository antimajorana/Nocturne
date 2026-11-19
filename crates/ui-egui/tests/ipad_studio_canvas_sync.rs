//! Integration tests for iPadOS Tablet Studio mode, 15% zoom + Fit All Content, collapsible
//! minimap, eraser auto-return on stylus lift, and non-blocking kDrive background sync in `NocturneApp`.

use egui::os::OperatingSystem;
use egui::{Pos2, Rect, pos2, vec2};
use nocturne_engine::{InkActiveMode, InkPenKind, Session};
use nocturne_store::Settings;
use nocturne_store::testing::TempDir;
use nocturne_ui_egui::NocturneApp;
use nocturne_ui_egui::ribbon;
use nocturne_ui_egui::tablet_chrome::UiLayoutMode;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

fn step_frame(ctx: &egui::Context, app: &mut NocturneApp, mut raw: egui::RawInput) {
    if raw.screen_rect.is_none() {
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, vec2(1180.0, 820.0)));
    }
    raw.events.extend(app.take_events());
    let mut out = ctx.run_ui(raw, |ui| {
        app.show(ui);
    });
    out.textures_delta.clear();
}

fn make_ipad_app(name: &str) -> (TempDir, egui::Context, NocturneApp, std::path::PathBuf) {
    let tmp = TempDir::new(name);
    let root = tmp.path().join("notebooks");
    let settings = Settings::at(tmp.path().join("cfg/settings.toml"));
    settings.set("privacy_welcome_seen", "true").expect("welcome seen");
    settings.set_default_format(nocturne_model::PageFormat::Cmgfm).expect("cmgfm default");
    let mut session = Session::new(&root, settings).expect("session");
    session.run("notebook.create", &[("name".into(), "Field Notes".into())].into_iter().collect()).expect("create nb");
    let page_dir = session.page_to_show("field-notes").expect("first page");
    let mut app = NocturneApp::new(session);
    app.open_path(&page_dir.to_string_lossy());
    let ctx = egui::Context::default();
    ctx.set_os(OperatingSystem::IOS);
    (tmp, ctx, app, page_dir)
}

#[test]
fn ipad_defaults_to_tablet_studio_and_switches_cleanly_with_classic_ribbon() {
    let (_tmp, ctx, mut app, page_dir) = make_ipad_app("studio-default");
    assert!(page_dir.join("index.md").is_file(), "iPad default format must create index.md");

    step_frame(&ctx, &mut app, egui::RawInput::default());
    step_frame(&ctx, &mut app, egui::RawInput::default());
    assert!(app.is_tablet_studio(&ctx), "iPadOS >= 600pt must default to Tablet Studio mode");
    assert!(app.page_rect().is_some(), "canvas page_rect must be active in Tablet Studio");

    // Toggle to Classic Ribbon and back.
    app.set_ui_layout_mode(UiLayoutMode::ClassicRibbon);
    step_frame(&ctx, &mut app, egui::RawInput::default());
    assert!(!app.is_tablet_studio(&ctx));

    app.set_ui_layout_mode(UiLayoutMode::TabletStudio);
    step_frame(&ctx, &mut app, egui::RawInput::default());
    assert!(app.is_tablet_studio(&ctx));
}

#[test]
fn eraser_auto_return_waits_for_stylus_lift_after_erasing_and_preserves_previous_pen() {
    let (_tmp, ctx, mut app, _page_dir) = make_ipad_app("eraser-auto-return");
    step_frame(&ctx, &mut app, egui::RawInput::default());
    step_frame(&ctx, &mut app, egui::RawInput::default());
    let page = app.page_rect().expect("page_rect");

    // Select pen slot 1 (`FountainPen` or second nib), draw a stroke across the canvas, then lift.
    app.session_mut().select_writing_tool(InkPenKind::FountainPen, 1);
    app.ribbon_act_for_test(ribbon::Act::InkPen(1));
    app.ribbon_act_for_test(ribbon::Act::ToggleEraserAutoReturn);
    step_frame(&ctx, &mut app, egui::RawInput::default());

    let p0 = pos2(page.center().x - 40.0, page.min.y + 180.0);
    let p1 = pos2(page.center().x + 40.0, page.min.y + 180.0);
    let stylus_ev = |phase: egui::TouchPhase, pos: Pos2, force: Option<f32>| egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(11),
        phase,
        pos,
        force,
    };
    let ptr_btn = |pressed: bool, pos: Pos2| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };

    // Draw 1 stroke.
    step_frame(
        &ctx,
        &mut app,
        egui::RawInput {
            events: vec![stylus_ev(egui::TouchPhase::Start, p0, Some(0.5)), egui::Event::PointerMoved(p0), ptr_btn(true, p0)],
            ..Default::default()
        },
    );
    step_frame(
        &ctx,
        &mut app,
        egui::RawInput { events: vec![stylus_ev(egui::TouchPhase::Move, p1, Some(0.6)), egui::Event::PointerMoved(p1)], ..Default::default() },
    );
    step_frame(
        &ctx,
        &mut app,
        egui::RawInput {
            events: vec![stylus_ev(egui::TouchPhase::End, p1, Some(0.0)), ptr_btn(false, p1), egui::Event::PointerGone],
            ..Default::default()
        },
    );
    assert_eq!(app.ink_stroke_count(), 1, "expected 1 drawn stroke");

    // Double-tap Apple Pencil -> toggle_eraser_tool() switches to Erase.
    app.toggle_eraser_tool();
    assert_eq!(app.session().ink_tool_state().active_mode, InkActiveMode::Erase);

    // Touching empty paper with the eraser and lifting without hitting ink MUST stay in Erase!
    let empty_pos = pos2(page.center().x, page.min.y + 320.0);
    step_frame(
        &ctx,
        &mut app,
        egui::RawInput {
            events: vec![stylus_ev(egui::TouchPhase::Start, empty_pos, Some(0.4)), egui::Event::PointerMoved(empty_pos), ptr_btn(true, empty_pos)],
            ..Default::default()
        },
    );
    step_frame(
        &ctx,
        &mut app,
        egui::RawInput {
            events: vec![stylus_ev(egui::TouchPhase::End, empty_pos, Some(0.0)), ptr_btn(false, empty_pos), egui::Event::PointerGone],
            ..Default::default()
        },
    );
    assert_eq!(app.session().ink_tool_state().active_mode, InkActiveMode::Erase, "lifting eraser without rubbing ink out must keep Eraser active");

    // Now rub across the stroke at `p0..p1`: while stylus is STILL DOWN, Eraser stays active;
    // on stylus LIFT, `eraser_auto_return` switches back to Draw (`rack_index == 1`)!
    let mid = pos2(page.center().x, page.min.y + 180.0);
    step_frame(
        &ctx,
        &mut app,
        egui::RawInput {
            events: vec![stylus_ev(egui::TouchPhase::Start, mid, Some(0.5)), egui::Event::PointerMoved(mid), ptr_btn(true, mid)],
            ..Default::default()
        },
    );
    assert_eq!(app.ink_stroke_count(), 0, "stroke must be erased on contact");
    assert_eq!(app.session().ink_tool_state().active_mode, InkActiveMode::Erase, "must remain in Erase while stylus is still down");

    step_frame(
        &ctx,
        &mut app,
        egui::RawInput {
            events: vec![stylus_ev(egui::TouchPhase::End, mid, Some(0.0)), ptr_btn(false, mid), egui::Event::PointerGone],
            ..Default::default()
        },
    );
    assert_eq!(app.session().ink_tool_state().active_mode, InkActiveMode::Draw, "lifting stylus after erasing must auto-return to Draw");
    assert_eq!(app.session().ink_tool_state().rack_index, 1, "must restore previous rack index");
}

#[test]
fn zoom_15_pinch_focal_fit_all_negative_ink_and_foreground_pill_guard() {
    let (_tmp, ctx, mut app, page_dir) = make_ipad_app("zoom-15-fit-all");
    // Write an SVG with strokes spanning negative coordinates (-500, -300) to far positive (3200, 2600)!
    let mut drawing = nocturne_ink::Drawing::new(820.0, 1100.0);
    let mut s1 = nocturne_ink::Stroke::new(nocturne_ink::Tool::Pen, nocturne_ink::Rgb(20, 40, 200), 3.0);
    s1.add(nocturne_ink::Dab::new(-500.0, -300.0, 0.5));
    s1.add(nocturne_ink::Dab::new(-420.0, -240.0, 0.5));
    let mut s2 = nocturne_ink::Stroke::new(nocturne_ink::Tool::Pen, nocturne_ink::Rgb(200, 40, 20), 3.0);
    s2.add(nocturne_ink::Dab::new(3100.0, 2500.0, 0.5));
    s2.add(nocturne_ink::Dab::new(3200.0, 2600.0, 0.5));
    drawing.add(s1);
    drawing.add(s2);
    std::fs::create_dir_all(page_dir.join("assets")).expect("assets");
    std::fs::write(page_dir.join("ink.svg"), nocturne_ink::svg::write(&drawing, nocturne_ink::svg::Flavour::Editable)).expect("write ink");
    app.open_path(&page_dir.to_string_lossy());

    step_frame(&ctx, &mut app, egui::RawInput::default());
    step_frame(&ctx, &mut app, egui::RawInput::default());
    let page_rect = app.page_rect().expect("page_rect");

    // 1. Pinch zoom from 100% down to 15% around a focal point on the canvas: verify focal point stays put!
    let focal = pos2(page_rect.center().x + 60.0, page_rect.center().y);
    let origin_before = app.editor_origin().expect("origin");
    let page_x_under_focal = focal.x - origin_before.x;
    app.apply_external_pinch_zoom(0.15, Some(focal));
    assert_eq!(app.zoom_percent(), 15);
    step_frame(&ctx, &mut app, egui::RawInput::default());
    step_frame(&ctx, &mut app, egui::RawInput::default());
    let origin_after = app.editor_origin().expect("origin after 15%");
    let screen_x_after = origin_after.x + page_x_under_focal * 0.15;
    assert!((screen_x_after - focal.x).abs() <= 2.5, "pinch focal X drifted: before={}, after={screen_x_after}", focal.x);

    // 2. Fit All Content in Write mode and Draw mode with negative + far-positive ink bounds,
    // starting from a deeply scrolled/panned position (`M7` regression check)!
    for mode in [ribbon::Pointer::Write, ribbon::Pointer::Draw] {
        app.ribbon_act_for_test(ribbon::Act::InkPointer(mode));
        app.ribbon_act_for_test(ribbon::Act::ZoomTo(100));
        app.apply_external_pan(vec2(-600.0, -2100.0));
        step_frame(&ctx, &mut app, egui::RawInput::default());
        step_frame(&ctx, &mut app, egui::RawInput::default());
        app.ribbon_act_for_test(ribbon::Act::FitAllContent);
        // Frame 1 applies zoom; Frame 2 reads post-zoom origin and centers pan + scroll; Frame 3 settles!
        step_frame(&ctx, &mut app, egui::RawInput::default());
        step_frame(&ctx, &mut app, egui::RawInput::default());
        step_frame(&ctx, &mut app, egui::RawInput::default());
        assert!((15..=30).contains(&app.zoom_percent()), "expected overview zoom for 3700x2900 span, got {}", app.zoom_percent());
        let z = app.zoom_percent() as f32 / 100.0;
        let orig = app.editor_origin().expect("origin");
        let pr = app.page_view_rect().expect("page_view_rect");
        // Expected center of (-500..3200, -300..2600) is ~(1350, 1150) (with container [0..780] included inside it!).
        let center_screen = orig + vec2(1350.0 * z, 1150.0 * z);
        assert!((center_screen.x - pr.center().x).abs() < 30.0, "Fit All X off-center in {mode:?}: {center_screen:?} vs {:?}", pr.center());
        assert!((center_screen.y - pr.center().y).abs() < 30.0, "Fit All Y off-center in {mode:?}: {center_screen:?} vs {:?}", pr.center());
    }

    // 3. Pressing with Pen on the Order::Foreground Pencil Tool Pill must NOT add a stroke to the page!
    app.ribbon_act_for_test(ribbon::Act::InkPen(0));
    step_frame(&ctx, &mut app, egui::RawInput::default());
    let strokes_before = app.ink_stroke_count();
    let pr = app.page_view_rect().expect("page_view_rect");
    let pill_pos = pos2(pr.center().x, pr.max.y - 24.0);
    assert_eq!(ctx.layer_id_at(pill_pos).map(|l| l.order), Some(egui::Order::Foreground), "Pencil tool pill must sit at Order::Foreground");
    step_frame(
        &ctx,
        &mut app,
        egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(pill_pos),
                egui::Event::PointerButton { pos: pill_pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
                egui::Event::PointerButton { pos: pill_pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE },
            ],
            ..Default::default()
        },
    );
    assert_eq!(app.ink_stroke_count(), strokes_before, "tapping the Foreground Pencil pill must never start an ink stroke underneath");
}

#[test]
fn background_sync_flushes_dirty_open_page_and_uploads_cleanly() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let addr = listener.local_addr().expect("addr");
    let uploaded_body: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let stop = Arc::new(AtomicBool::new(false));

    let up_bg = Arc::clone(&uploaded_body);
    let stop_bg = Arc::clone(&stop);
    let srv_thread = thread::spawn(move || {
        while !stop_bg.load(Ordering::Relaxed) {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.set_nonblocking(false);
                let Ok(cloned) = stream.try_clone() else { continue };
                let mut reader = BufReader::new(cloned);
                let mut req_line = String::new();
                let _ = reader.read_line(&mut req_line);
                let mut clen = 0usize;
                loop {
                    let mut l = String::new();
                    if reader.read_line(&mut l).is_err() || l == "\r\n" || l.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = l.split_once(':')
                        && k.trim().eq_ignore_ascii_case("content-length")
                    {
                        clen = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; clen];
                if clen > 0 {
                    let _ = reader.read_exact(&mut body);
                }
                let parts: Vec<&str> = req_line.split_whitespace().collect();
                if parts.len() < 2 {
                    continue;
                }
                let (method, target) = (parts[0], parts[1]);
                let (path, query) = target.split_once('?').unwrap_or((target, ""));
                let reply_json = |s: &mut std::net::TcpStream, val: serde_json::Value| {
                    let txt = val.to_string();
                    let head =
                        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", txt.len());
                    let _ = s.write_all(head.as_bytes());
                    let _ = s.write_all(txt.as_bytes());
                };
                if method == "POST" && path.ends_with("/upload") {
                    if query.contains("index.md") {
                        *up_bg.lock().expect("lock") = Some(String::from_utf8_lossy(&body).into_owned());
                    }
                    reply_json(
                        &mut stream,
                        serde_json::json!({ "result": "success", "data": { "id": 501, "name": "index.md", "type": "file", "size": body.len(), "hash": "h1" } }),
                    );
                } else if method == "POST" && path.ends_with("/directory") {
                    reply_json(&mut stream, serde_json::json!({ "result": "success", "data": { "id": 100, "name": "dir", "type": "dir" } }));
                } else if method == "GET" && path == "/3/drive/88/files/1/files" {
                    reply_json(
                        &mut stream,
                        serde_json::json!({ "result": "success", "data": [{ "id": 2, "name": "Private", "type": "dir", "visibility": "is_private_space" }], "has_more": false }),
                    );
                } else if method == "GET" && path.ends_with("/files") {
                    reply_json(&mut stream, serde_json::json!({ "result": "success", "data": [], "has_more": false }));
                } else {
                    reply_json(&mut stream, serde_json::json!({ "result": "success", "data": [] }));
                }
            } else {
                thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    });

    let (_tmp, ctx, mut app, _page_dir) = make_ipad_app("bg-sync-dirty-flush");
    let endpoint = format!("http://{addr}");
    app.session_mut()
        .run(
            "sync.configure",
            &[
                ("endpoint".into(), endpoint),
                ("drive_id".into(), "88".into()),
                ("remote_path".into(), "The Oracle/Nocturne".into()),
                ("token".into(), "tok".into()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("cfg");

    // Dirty the open page in memory WITHOUT manually saving, then start background sync!
    app.set_body("Fresh iPad Pencil + Markdown note");
    step_frame(&ctx, &mut app, egui::RawInput::default());
    app.start_background_sync(&ctx);

    for _ in 0..80 {
        step_frame(&ctx, &mut app, egui::RawInput::default());
        if uploaded_body.lock().expect("lock").is_some() {
            break;
        }
        thread::sleep(std::time::Duration::from_millis(15));
    }
    stop.store(true, Ordering::Relaxed);
    let _ = srv_thread.join();

    let uploaded = uploaded_body.lock().expect("lock").clone().expect("dirty open page must be flushed and uploaded by start_background_sync");
    assert!(uploaded.contains("Fresh iPad Pencil + Markdown note"), "uploaded markdown missing in-memory edit: {uploaded}");
}
