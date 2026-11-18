use egui::{Pos2, Rect, Vec2};
use nocturne_engine::{InkActiveMode, Session};
use nocturne_ios::edit_menu::{
    EditMenuAction, EditMenuBridgeState, EditMenuCapabilities, build_menu_items, compute_menu_target_rect, is_long_press_near_caret,
    should_present_native_edit_menu,
};
use nocturne_ios::gestures::{
    GestureBridgeState, GestureRect, OFFSCREEN_RELEASE_POS, PureInputEvent, PureTouchPhase, compute_incremental_pan, compute_incremental_pinch,
    egui_overlay_blocks_native_gesture, filter_pure_events, filter_pure_events_with_stylus_ids, should_begin_native_pan, should_begin_native_pinch,
};
use nocturne_ios::keyboard::compute_soft_keyboard_state;
use nocturne_ios::pencil::{PencilBridgeState, PencilCommand, PencilPreferredAction, PencilSqueezePhase};
use nocturne_ios::text_input::{
    TextDocumentModel, TextEditCommand, caret_rect_or_fallback, dispatch_pending_text_commands, dispatch_pending_text_commands_with_context,
    indirect_scribble_target_rect, lock_arrow_focus_if_ready, should_route_ime_to_egui, split_insert_text_commands,
};
use nocturne_model::NodeKind;
use nocturne_store::{Node, Settings, testing::TempDir};
use nocturne_ui_egui::clipboard::{Board, Picture};
use nocturne_ui_egui::stylus::Stylus;
use nocturne_ui_egui::touch_ux::{MAX_ZOOM_PERCENT, MIN_ZOOM_PERCENT, TouchLongPress};
use nocturne_ui_egui::{NocturneApp, PlatformEditAction, pagemenu};
use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::rc::Rc;

#[derive(Clone, Default)]
struct SharedClipboard(Rc<RefCell<Option<String>>>);

impl Board for SharedClipboard {
    fn text(&mut self) -> Option<String> {
        self.0.borrow().clone()
    }

    fn html(&mut self) -> Option<String> {
        None
    }

    fn picture(&mut self) -> Option<Picture> {
        None
    }

    fn has_picture(&mut self) -> bool {
        false
    }

    fn set_text(&mut self, text: &str) -> Result<(), String> {
        *self.0.borrow_mut() = Some(text.to_owned());
        Ok(())
    }

    fn set_html(&mut self, _html: &str, text: &str) -> Result<(), String> {
        *self.0.borrow_mut() = Some(text.to_owned());
        Ok(())
    }
}

fn first_section_path(nodes: &[Node]) -> Option<std::path::PathBuf> {
    for node in nodes {
        if node.info.kind == NodeKind::Section {
            return Some(node.path.clone());
        }
        if let Some(found) = first_section_path(&node.children) {
            return Some(found);
        }
    }
    None
}

fn make_test_app(label: &str) -> (TempDir, NocturneApp, SharedClipboard) {
    let dir = TempDir::new(label);
    let notebooks = dir.path().join("notebooks");
    let settings_path = dir.path().join("settings.toml");
    fs::create_dir_all(&notebooks).expect("create notebooks");
    let settings = Settings::at(&settings_path);
    settings.set("privacy_welcome_seen", "true").expect("set privacy_welcome_seen");
    let mut session = Session::new(&notebooks, settings).expect("create session");
    session.run("notebook.create", &[("name".to_owned(), "Notes".to_owned())].into_iter().collect()).expect("create notebook");
    let section_dir = first_section_path(session.tree()).expect("default section created by notebook.create");
    let page_out = session
        .run(
            "node.create",
            &[
                ("parent".to_owned(), section_dir.to_string_lossy().into_owned()),
                ("kind".to_owned(), "page".to_owned()),
                ("title".to_owned(), "Scratch".to_owned()),
                ("format".to_owned(), "cmgfm".to_owned()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("create cmgfm page");
    let nocturne_engine::Output::Path(page_dir) = page_out else {
        panic!("expected Output::Path from node.create, got {page_out:?}");
    };
    session
        .run(
            "page.save",
            &[("path".to_owned(), page_dir.to_string_lossy().into_owned()), ("body".to_owned(), "Start\n".to_owned())].into_iter().collect(),
        )
        .expect("seed page body");
    let clip = SharedClipboard::default();
    let mut app = NocturneApp::new(session).with_clipboard(Box::new(clip.clone()));
    app.open_path(&page_dir.to_string_lossy());
    (dir, app, clip)
}

#[test]
fn pencil_bridge_drains_double_tap_and_squeeze_into_nocturne_eraser_toggle() {
    let (_dir, mut app, _clip) = make_test_app("ios-bridge-pencil");
    assert_ne!(app.session().ink_tool_state().active_mode, InkActiveMode::Erase);

    let mut pencil = PencilBridgeState::new();
    assert_eq!(pencil.record_tap(PencilPreferredAction::Ignore), PencilCommand::Ignore);
    assert_eq!(pencil.record_squeeze(PencilPreferredAction::SwitchEraser, PencilSqueezePhase::Began), PencilCommand::Ignore);
    assert_eq!(pencil.record_squeeze(PencilPreferredAction::SwitchEraser, PencilSqueezePhase::Changed), PencilCommand::Ignore);
    assert_eq!(pencil.record_tap(PencilPreferredAction::SwitchEraser), PencilCommand::ToggleEraser);
    assert_eq!(pencil.record_squeeze(PencilPreferredAction::SwitchPrevious, PencilSqueezePhase::Ended), PencilCommand::ToggleEraser);

    let count = pencil.take_toggle_eraser_count();
    assert_eq!(count, 2);
    assert_eq!(pencil.take_toggle_eraser_count(), 0);

    // First toggle activates eraser; second restores previous writing tool.
    app.toggle_eraser_tool();
    assert_eq!(app.session().ink_tool_state().active_mode, InkActiveMode::Erase);
    app.toggle_eraser_tool();
    assert_eq!(app.session().ink_tool_state().active_mode, InkActiveMode::Draw);
}

#[test]
fn gesture_bridge_and_finger_drawing_coordinate_pan_pinch_and_pencil_passthrough() {
    let (_dir, mut app, _clip) = make_test_app("ios-bridge-gestures");
    assert!(!app.is_finger_drawing());

    // Default (`!finger_drawing`): 1-finger pan begins inside canvas and preserves Apple Pencil.
    let mut gstate = GestureBridgeState {
        finger_drawing: app.is_finger_drawing(),
        canvas_rect: Some(GestureRect::from_bounds([100.0, 100.0, 900.0, 700.0])),
        egui_using_pointer: false,
        ..Default::default()
    };
    assert!(should_begin_native_pan(1, gstate.finger_drawing, gstate.point_in_canvas(300.0, 300.0), gstate.egui_using_pointer));
    assert!(!should_begin_native_pan(1, gstate.finger_drawing, gstate.point_in_canvas(20.0, 300.0), gstate.egui_using_pointer));
    assert!(should_begin_native_pinch(2, gstate.point_in_canvas(300.0, 300.0), gstate.egui_using_pointer));

    gstate.pan_active = true;
    gstate.pinch_active = true;
    gstate.needs_pointer_release = true;

    let [dx, dy] = compute_incremental_pan(12.0, -40.0);
    gstate.accumulate_pan(dx, dy);
    let (scale, focal) = compute_incremental_pinch(1.5, Some([400.0, 350.0])).expect("valid pinch");
    gstate.accumulate_pinch(scale, focal);

    if let Some([pdx, pdy]) = gstate.drain_pan_delta() {
        app.apply_external_pan(Vec2::new(pdx, pdy));
    }
    if let Some((pscale, pfocal)) = gstate.drain_pinch() {
        app.apply_external_pinch_zoom(pscale, pfocal.map(|[x, y]| Pos2::new(x, y)));
    }
    assert_eq!(app.zoom_percent(), 150);

    // Clamp zoom bounds across extreme pinch gestures.
    app.apply_external_pinch_zoom(10.0, None);
    assert_eq!(app.zoom_percent(), MAX_ZOOM_PERCENT);
    app.apply_external_pinch_zoom(0.05, None);
    assert_eq!(app.zoom_percent(), MIN_ZOOM_PERCENT);

    // Active pan/pinch suppresses finger events & Zoom while passing Apple Pencil through.
    let pencil_ev = PureInputEvent::Touch { device_id: 1, id: 77, phase: PureTouchPhase::Move, pos: [450.0, 400.0], force: Some(0.6) };
    let mut events = vec![
        PureInputEvent::Touch { device_id: 0, id: 1, phase: PureTouchPhase::Move, pos: [200.0, 220.0], force: None },
        PureInputEvent::PointerMoved([200.0, 220.0]),
        PureInputEvent::Zoom(1.2),
        pencil_ev,
    ];
    filter_pure_events(&mut events, &mut gstate, true, false);
    assert_eq!(events, vec![pencil_ev]);

    // When Finger drawing is enabled, 1-finger pan yields to drawing while 2-finger pan remains active.
    app.set_finger_drawing(true);
    assert!(app.is_finger_drawing());
    assert!(!should_begin_native_pan(1, app.is_finger_drawing(), true, false));
    assert!(should_begin_native_pan(2, app.is_finger_drawing(), true, false));
    let _ = OFFSCREEN_RELEASE_POS;
}

#[test]
fn text_input_edit_menu_and_keyboard_visibility_integrate_cleanly() {
    let (_dir, mut app, clip) = make_test_app("ios-bridge-text");
    let mut doc = TextDocumentModel::new();
    let mut menu = EditMenuBridgeState::new();

    // Docked vs floating keyboard coordination drives formatting-bar visibility state.
    let (vis_docked, inset_docked) = compute_soft_keyboard_state(1024.0, Some((0.0, 710.0, 768.0, 314.0)), 20.0);
    assert!(vis_docked);
    assert!((inset_docked - 314.0).abs() < 1e-4);
    app.set_soft_keyboard_visible(vis_docked, inset_docked);

    let (vis_float, inset_float) = compute_soft_keyboard_state(1024.0, Some((200.0, 360.0, 320.0, 280.0)), 20.0);
    assert!(vis_float);
    assert!((inset_float - 20.0).abs() < 1e-4);

    // Split newlines into InsertText + PressEnter before dispatching to NocturneApp.
    let cmds = doc.insert_text("Alpha\nBeta");
    assert_eq!(
        cmds,
        vec![TextEditCommand::InsertText("Alpha".to_owned()), TextEditCommand::PressEnter, TextEditCommand::InsertText("Beta".to_owned()),]
    );
    assert_eq!(
        split_insert_text_commands("x\r\ny"),
        vec![TextEditCommand::InsertText("x".to_owned()), TextEditCommand::PressEnter, TextEditCommand::InsertText("y".to_owned()),]
    );

    app.insert_platform_text("Alpha");
    app.insert_platform_text("!");
    app.delete_backward_platform();
    assert!(
        app.body().is_some_and(|b| b.contains("Alpha")),
        "expected Alpha in body, got body={:?}, caret={:?}",
        app.body(),
        app.keyboard_caret_rect()
    );

    // Edit menu capability & presentation gating with TouchLongPress.
    let mut lp = TouchLongPress::default();
    let press = egui::Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: egui::TouchId(1),
        phase: egui::TouchPhase::Start,
        pos: Pos2::new(240.0, 300.0),
        force: None,
    };
    assert_eq!(lp.update(0.0, &[press]), None);
    let fired = lp.update(0.55, &[]).expect("long press fires");
    assert!(should_present_native_edit_menu(true, true));
    let target = compute_menu_target_rect(Some([240.0, 300.0, 2.0, 18.0]), Some([fired.x, fired.y]));
    menu.set_presentation(target, EditMenuCapabilities { has_selection: true, can_paste: true, is_writable: true });
    assert_eq!(build_menu_items(true, true, true).len(), 5);

    // SelectAll + Copy -> Paste roundtrip through NocturneApp platform edit hooks.
    app.apply_platform_edit_action(PlatformEditAction::SelectAll);
    assert_eq!(app.selected_text_for_platform(), "StartAlpha");
    app.apply_platform_edit_action(PlatformEditAction::Copy);
    assert_eq!(clip.0.borrow().as_deref(), Some("StartAlpha"));

    // Caret sanitization and Scribble gating respect pointer_is_write.
    assert_eq!(caret_rect_or_fallback(None, [0.0, 0.0, 1024.0, 768.0]), [512.0, 192.0, 2.0, 18.0]);
    assert!(indirect_scribble_target_rect(Some([240.0, 300.0, 2.0, 18.0]), true).is_some());
    assert!(indirect_scribble_target_rect(Some([240.0, 300.0, 2.0, 18.0]), false).is_none());
    let _ = Rect::from_min_size(Pos2::ZERO, Vec2::new(10.0, 10.0));
    let _ = EditMenuAction::Paste;
}

fn step_app_frame(ctx: &egui::Context, app: &mut NocturneApp, mut raw_input: egui::RawInput) {
    raw_input.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 768.0)));
    raw_input.events.extend(app.take_events());
    let mut out = ctx.run_ui(raw_input, |ui| {
        app.show(ui);
    });
    out.textures_delta.clear();
}

#[test]
fn canvas_click_and_drag_widget_does_not_block_native_pan_or_pinch() {
    let ctx = egui::Context::default();
    let press_pos = Pos2::new(400.0, 300.0);

    // Frame 0: Layout canvas widgets (`open-paper` Sense::click + `ink-layer` Sense::click_and_drag)
    // so egui records their hit-test rects for the next frame's press.
    let raw0 = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 768.0))), ..Default::default() };
    let mut out0 = ctx.run_ui(raw0, |ui| {
        let rect = ui.available_rect_before_wrap();
        ui.interact(rect, ui.scope_id().with("open-paper"), egui::Sense::click());
        ui.interact(rect, ui.scope_id().with("ink-layer"), egui::Sense::click_and_drag());
    });
    out0.textures_delta.clear();

    // Frame 1: Touch-down (`PointerButton::Primary` pressed) over the canvas widgets.
    let mut raw1 = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 768.0))), ..Default::default() };
    raw1.events.push(egui::Event::PointerMoved(press_pos));
    raw1.events.push(egui::Event::PointerButton {
        pos: press_pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    let mut out1 = ctx.run_ui(raw1, |ui| {
        let rect = ui.available_rect_before_wrap();
        ui.interact(rect, ui.scope_id().with("open-paper"), egui::Sense::click());
        ui.interact(rect, ui.scope_id().with("ink-layer"), egui::Sense::click_and_drag());
    });
    out1.textures_delta.clear();

    // `ctx.egui_is_using_pointer()` IS true because touch-down set potential_click_id/potential_drag_id,
    // which previously killed native UIKit pan/pinch!
    assert!(ctx.egui_is_using_pointer());

    let blocked = egui_overlay_blocks_native_gesture(&ctx, false, Some(press_pos));
    assert!(!blocked, "canvas Sense::click_and_drag must NOT block native pan/pinch");
    assert!(should_begin_native_pan(1, false, true, blocked));
    assert!(should_begin_native_pinch(2, true, blocked));

    // When show_source is active, native gestures yield.
    assert!(egui_overlay_blocks_native_gesture(&ctx, true, Some(press_pos)));

    // When a Foreground overlay covers the pointer position, native gestures yield.
    for _ in 0..2 {
        let mut raw2 = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 768.0))), ..Default::default() };
        raw2.events.push(egui::Event::PointerMoved(press_pos));
        let mut out2 = ctx.run_ui(raw2, |ui| {
            egui::Area::new(ui.scope_id().with("overlay-menu")).order(egui::Order::Foreground).fixed_pos(Pos2::new(350.0, 250.0)).show(
                ui.ctx(),
                |ui| {
                    ui.allocate_exact_size(Vec2::new(140.0, 120.0), egui::Sense::click());
                },
            );
        });
        out2.textures_delta.clear();
    }
    let blocked_overlay = egui_overlay_blocks_native_gesture(&ctx, false, Some(press_pos));
    assert!(blocked_overlay);
    assert!(!should_begin_native_pan(1, false, true, blocked_overlay));
    assert!(!should_begin_native_pinch(2, true, blocked_overlay));
}

#[test]
fn touch_phase_end_during_active_pan_unblocks_long_press_and_clears_stylus() {
    // 1. TouchPhase::End while pan_active == true must pass through to TouchLongPress.
    let mut gstate = GestureBridgeState { pan_active: true, ..Default::default() };
    let mut lp = TouchLongPress::default();
    let f1_start = PureInputEvent::Touch { device_id: 0, id: 1, phase: PureTouchPhase::Start, pos: [200.0, 200.0], force: None };
    let f1_end = PureInputEvent::Touch { device_id: 0, id: 1, phase: PureTouchPhase::End, pos: [200.0, 200.0], force: None };
    assert_eq!(
        lp.update(
            0.0,
            &[egui::Event::Touch {
                device_id: egui::TouchDeviceId(0),
                id: egui::TouchId(1),
                phase: egui::TouchPhase::Start,
                pos: Pos2::new(200.0, 200.0),
                force: None
            }]
        ),
        None
    );

    let mut frame_end = vec![f1_end];
    filter_pure_events(&mut frame_end, &mut gstate, false, false);
    assert_eq!(frame_end, vec![f1_end]);
    assert_eq!(
        lp.update(
            0.1,
            &[egui::Event::Touch {
                device_id: egui::TouchDeviceId(0),
                id: egui::TouchId(1),
                phase: egui::TouchPhase::End,
                pos: Pos2::new(200.0, 200.0),
                force: None
            }]
        ),
        None
    );

    // Subsequent single-finger long-press must still fire because contact #1 was not leaked.
    let _ = f1_start;
    assert_eq!(
        lp.update(
            1.0,
            &[egui::Event::Touch {
                device_id: egui::TouchDeviceId(0),
                id: egui::TouchId(2),
                phase: egui::TouchPhase::Start,
                pos: Pos2::new(220.0, 220.0),
                force: None
            }]
        ),
        None
    );
    assert_eq!(lp.update(1.55, &[]), Some(Pos2::new(220.0, 220.0)));

    // 2. Stylus TouchId Move/End with force: None during pan_active == true clears Stylus::has_active_stylus().
    let mut stylus = Stylus::default();
    let mut stylus_gstate = GestureBridgeState { pan_active: true, needs_pointer_release: true, ..Default::default() };
    let mut active_ids = BTreeSet::from([99u64]);

    let p_start = egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(99),
        phase: egui::TouchPhase::Start,
        pos: Pos2::new(300.0, 300.0),
        force: Some(0.4),
    };
    stylus.update(&[p_start]);
    assert!(stylus.has_active_stylus());

    let mut pure_move = vec![PureInputEvent::Touch { device_id: 1, id: 99, phase: PureTouchPhase::Move, pos: [310.0, 310.0], force: None }];
    filter_pure_events_with_stylus_ids(&mut pure_move, &mut stylus_gstate, true, &active_ids);
    assert_eq!(pure_move.len(), 1);
    stylus.update(&[egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(99),
        phase: egui::TouchPhase::Move,
        pos: Pos2::new(310.0, 310.0),
        force: None,
    }]);
    assert!(stylus.has_active_stylus());

    let mut pure_end = vec![PureInputEvent::Touch { device_id: 1, id: 99, phase: PureTouchPhase::End, pos: [315.0, 315.0], force: None }];
    filter_pure_events_with_stylus_ids(&mut pure_end, &mut stylus_gstate, true, &active_ids);
    assert_eq!(pure_end.len(), 1);
    active_ids.clear();
    stylus.update(&[egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(99),
        phase: egui::TouchPhase::End,
        pos: Pos2::new(315.0, 315.0),
        force: None,
    }]);
    assert!(!stylus.has_active_stylus(), "Stylus must not get stuck active after force: None TouchPhase::End");
}

#[test]
fn long_press_away_from_caret_opens_pagemenu_while_near_caret_presents_edit_menu() {
    let (_dir, mut app, _clip) = make_test_app("ios-bridge-long-press");
    let ctx = egui::Context::default();

    // Initial layout frame to populate page_rect.
    step_app_frame(&ctx, &mut app, egui::RawInput::default());
    let page = app.page_rect().expect("page_rect populated");

    // Click inside block 0 to focus the text caret.
    let tap_pos = Pos2::new(page.min.x + 24.0, page.min.y + 64.0);
    let mut click_raw = egui::RawInput::default();
    click_raw.events.push(egui::Event::PointerMoved(tap_pos));
    click_raw.events.push(egui::Event::PointerButton {
        pos: tap_pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    click_raw.events.push(egui::Event::PointerButton {
        pos: tap_pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    step_app_frame(&ctx, &mut app, click_raw);
    step_app_frame(&ctx, &mut app, egui::RawInput::default());

    let caret = app.keyboard_caret_rect().expect("caret focused after clicking block 0");
    let caret_arr = [caret.min.x, caret.min.y, caret.width(), caret.height()];
    let page_arr = [page.min.x, page.min.y, page.width(), page.height()];

    // Long-press right next to caret -> native EditMenu allowed.
    let near_pos = [caret.min.x + 12.0, caret.min.y + 6.0];
    assert!(is_long_press_near_caret(near_pos, Some(caret_arr), Some(page_arr)));
    assert!(should_present_native_edit_menu(true, true));

    // Long-press on empty canvas 280pt below caret -> NOT near caret -> Secondary click opens pagemenu.
    let canvas_pos = Pos2::new(page.center().x, (page.max.y - 40.0).max(caret.max.y + 200.0));
    assert!(!is_long_press_near_caret([canvas_pos.x, canvas_pos.y], Some(caret_arr), Some(page_arr)));

    let mut secondary_raw = egui::RawInput::default();
    secondary_raw.events.push(egui::Event::PointerMoved(canvas_pos));
    secondary_raw.events.push(egui::Event::PointerButton {
        pos: canvas_pos,
        button: egui::PointerButton::Secondary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    secondary_raw.events.push(egui::Event::PointerButton {
        pos: canvas_pos,
        button: egui::PointerButton::Secondary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    step_app_frame(&ctx, &mut app, secondary_raw);
    assert!(pagemenu::is_open(&ctx), "pagemenu must open when long-press falls outside caret proximity");
}

#[test]
fn multiline_insert_text_and_marked_composition_preserve_blocks_in_nocturne_app() {
    let (_dir, mut app, _clip) = make_test_app("ios-bridge-multiline-ime");
    let ctx = egui::Context::default();

    step_app_frame(&ctx, &mut app, egui::RawInput::default());
    let page = app.page_rect().expect("page_rect populated");
    let tap_pos = Pos2::new(page.min.x + 24.0, page.min.y + 64.0);
    let mut click_raw = egui::RawInput::default();
    click_raw.events.push(egui::Event::PointerMoved(tap_pos));
    click_raw.events.push(egui::Event::PointerButton {
        pos: tap_pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    click_raw.events.push(egui::Event::PointerButton {
        pos: tap_pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    step_app_frame(&ctx, &mut app, click_raw);
    step_app_frame(&ctx, &mut app, egui::RawInput::default());
    assert!(app.keyboard_caret_rect().is_some());

    // SelectAll + Delete to start with an empty block 0, then dispatch "Alpha\nBeta".
    let mut doc = TextDocumentModel::new();
    let mut queue = VecDeque::new();
    let mut preedit = false;
    app.apply_platform_edit_action(PlatformEditAction::SelectAll);
    app.delete_backward_platform();
    queue.extend(doc.insert_text("Alpha\nBeta"));

    // Frame A: inserts "Alpha", pushes Key::Enter, pauses queue before "Beta".
    let mut frame_a = egui::RawInput::default();
    let needs_repaint = dispatch_pending_text_commands(&mut queue, false, &mut preedit, &mut app, &mut frame_a, || None);
    assert!(needs_repaint);
    assert_eq!(queue, VecDeque::from(vec![TextEditCommand::InsertText("Beta".to_owned())]));
    step_app_frame(&ctx, &mut app, frame_a);

    // Frame B: after Key::Enter created block 1, drains "Beta" into block 1.
    let mut frame_b = egui::RawInput::default();
    let more = dispatch_pending_text_commands(&mut queue, false, &mut preedit, &mut app, &mut frame_b, || None);
    assert!(!more);
    assert!(queue.is_empty());
    step_app_frame(&ctx, &mut app, frame_b);

    let blocks = app.editor_blocks();
    assert!(blocks.len() >= 2, "expected 2 blocks after Alpha\\nBeta, got {blocks:?}");
    assert!(app.body().is_some_and(|b| b.contains("Alpha\n\nBeta") || (b.contains("Alpha") && b.contains("Beta"))));
    assert!(!app.body().is_some_and(|b| b.contains("AlphaBeta")), "Alpha and Beta must NOT be concatenated in one block: {:?}", app.body());

    // Marked composition: setMarkedText("nihao") + deleteBackward cancels without corrupting "Beta",
    // then setMarkedText("nihao") + unmarkText commits "nihao".
    let cmd_mark1 = doc.set_marked_text(Some("nihao"), (5, 5));
    queue.push_back(cmd_mark1);
    queue.extend(doc.delete_backward());
    let mut frame_c = egui::RawInput::default();
    dispatch_pending_text_commands(&mut queue, false, &mut preedit, &mut app, &mut frame_c, || None);
    step_app_frame(&ctx, &mut app, frame_c);
    assert!(app.body().is_some_and(|b| b.contains("Beta")), "deleteBackward on uncommitted marked text must not delete Beta: {:?}", app.body());

    let cmd_mark2 = doc.set_marked_text(Some(" nihao"), (6, 6));
    queue.push_back(cmd_mark2);
    if let Some(unmark_cmd) = doc.unmark_text() {
        queue.push_back(unmark_cmd);
    }
    let mut frame_d = egui::RawInput::default();
    dispatch_pending_text_commands(&mut queue, false, &mut preedit, &mut app, &mut frame_d, || None);
    step_app_frame(&ctx, &mut app, frame_d);
    assert!(app.body().is_some_and(|b| b.contains("Beta nihao")), "unmarkText must commit marked composition into NocturneApp, got {:?}", app.body());
}

fn focus_block_zero(ctx: &egui::Context, app: &mut NocturneApp) {
    step_app_frame(ctx, app, egui::RawInput::default());
    let page = app.page_rect().expect("page_rect populated");
    let tap_pos = Pos2::new(page.min.x + 24.0, page.min.y + 64.0);
    let mut click_raw = egui::RawInput::default();
    click_raw.events.push(egui::Event::PointerMoved(tap_pos));
    click_raw.events.push(egui::Event::PointerButton {
        pos: tap_pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    click_raw.events.push(egui::Event::PointerButton {
        pos: tap_pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    step_app_frame(ctx, app, click_raw);
    step_app_frame(ctx, app, egui::RawInput::default());
    assert!(app.keyboard_caret_rect().is_some(), "expected focused caret in block 0");
}

fn drain_queue_with_focus_lock(ctx: &egui::Context, app: &mut NocturneApp, queue: &mut VecDeque<TextEditCommand>) {
    let mut focus_wait = 0u8;
    let mut preedit = false;
    for _ in 0..64 {
        let mut raw = egui::RawInput::default();
        if app.keyboard_caret_rect().is_some() {
            let _ = lock_arrow_focus_if_ready(ctx);
        }
        let repaint = dispatch_pending_text_commands_with_context(Some(ctx), &mut focus_wait, queue, false, &mut preedit, app, &mut raw, || None);
        step_app_frame(ctx, app, raw);
        if queue.is_empty() && !repaint {
            break;
        }
    }
    assert!(queue.is_empty(), "text command queue did not drain cleanly: {queue:?}");
}

#[test]
fn order_middle_dialog_blocks_native_gesture_while_ink_overlay_does_not() {
    let (_dir, mut app, _clip) = make_test_app("ios-bridge-order-middle");
    let ctx = egui::Context::default();

    step_app_frame(&ctx, &mut app, egui::RawInput::default());
    step_app_frame(&ctx, &mut app, egui::RawInput::default());
    let page = app.page_rect().expect("page_rect populated");

    // Open an Order::Middle egui::Window dialog (`Dialog::Link`) and settle 2 frames.
    app.open_link_dialog();
    step_app_frame(&ctx, &mut app, egui::RawInput::default());
    step_app_frame(&ctx, &mut app, egui::RawInput::default());

    let dialog_center = Pos2::new(512.0, 384.0);
    assert_eq!(ctx.layer_id_at(dialog_center).map(|layer| layer.order), Some(egui::Order::Middle), "Link dialog must register an Order::Middle Area");
    let blocked_in_dialog = egui_overlay_blocks_native_gesture(&ctx, false, Some(dialog_center));
    assert!(blocked_in_dialog, "Order::Middle dialog must block native pan/pinch");
    assert!(!should_begin_native_pan(1, false, true, blocked_in_dialog));
    assert!(!should_begin_native_pinch(2, true, blocked_in_dialog));

    // Close dialog via Escape, enter Draw mode (`ink-overlay` active), and verify canvas does not block.
    let mut esc_raw = egui::RawInput::default();
    esc_raw.events.push(egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    esc_raw.events.push(egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    step_app_frame(&ctx, &mut app, esc_raw);
    app.toggle_eraser_tool();
    app.toggle_eraser_tool();
    assert_eq!(app.session().ink_tool_state().active_mode, InkActiveMode::Draw);
    step_app_frame(&ctx, &mut app, egui::RawInput::default());
    step_app_frame(&ctx, &mut app, egui::RawInput::default());

    let canvas_pos = Pos2::new(page.center().x, (page.min.y + 180.0).min(page.max.y - 40.0));
    let blocked_on_ink_canvas = egui_overlay_blocks_native_gesture(&ctx, false, Some(canvas_pos));
    assert!(!blocked_on_ink_canvas, "Draw-mode ink-overlay canvas must NOT block native gestures");
    assert!(should_begin_native_pan(1, false, true, blocked_on_ink_canvas));
    assert!(should_begin_native_pinch(2, true, blocked_on_ink_canvas));
}

fn first_block_text(app: &NocturneApp) -> Option<String> {
    match app.editor_blocks().first()? {
        nocturne_formats::markdown::Block::Paragraph(spans)
        | nocturne_formats::markdown::Block::Heading { spans, .. }
        | nocturne_formats::markdown::Block::Quote(spans)
        | nocturne_formats::markdown::Block::Item { spans, .. } => Some(spans.iter().map(|s| s.text.as_str()).collect()),
        nocturne_formats::markdown::Block::Code { text, .. } | nocturne_formats::markdown::Block::MathBlock(text) => Some(text.clone()),
        _ => None,
    }
}

#[test]
fn immediate_replace_range_and_move_cursor_keep_block_focus_in_nocturne_app() {
    let (_dir, mut app, _clip) = make_test_app("ios-bridge-move-cursor-focus");
    let ctx = egui::Context::default();
    focus_block_zero(&ctx, &mut app);

    // 1. Insert "Hello" and immediately queue `replace_range(2, 3, "X")` on the same frame.
    app.apply_platform_edit_action(PlatformEditAction::SelectAll);
    app.delete_backward_platform();
    let mut doc = TextDocumentModel::new();
    let mut queue = VecDeque::new();
    queue.extend(doc.insert_text("Hello"));
    queue.extend(doc.replace_range(2, 3, "X"));
    drain_queue_with_focus_lock(&ctx, &mut app, &mut queue);

    assert_eq!(first_block_text(&app).as_deref(), Some("HeXlo"));
    assert!(app.keyboard_caret_rect().is_some(), "block 0 must retain focus across MoveCursor");

    // 1b. Verify distinct characters so an off-by-one step cannot hide ("01234" -> "01X34").
    app.apply_platform_edit_action(PlatformEditAction::SelectAll);
    app.delete_backward_platform();
    let mut doc_distinct = TextDocumentModel::new();
    queue.extend(doc_distinct.insert_text("01234"));
    queue.extend(doc_distinct.replace_range(2, 3, "X"));
    drain_queue_with_focus_lock(&ctx, &mut app, &mut queue);

    assert_eq!(first_block_text(&app).as_deref(), Some("01X34"));
    assert!(app.keyboard_caret_rect().is_some(), "block 0 must retain focus across MoveCursor");

    // 2. Replace with "ABCDE", move cursor -3, and insert "*" -> "AB*CDE".
    app.apply_platform_edit_action(PlatformEditAction::SelectAll);
    app.delete_backward_platform();
    let mut doc2 = TextDocumentModel::new();
    queue.extend(doc2.insert_text("ABCDE"));
    queue.push_back(TextEditCommand::MoveCursor(-3));
    queue.push_back(TextEditCommand::InsertText("*".to_owned()));
    drain_queue_with_focus_lock(&ctx, &mut app, &mut queue);

    assert_eq!(first_block_text(&app).as_deref(), Some("AB*CDE"));
    assert!(app.keyboard_caret_rect().is_some(), "block 0 must retain focus after MoveCursor(-3) + InsertText");
}

#[test]
fn non_empty_selection_replace_range_end_to_end_in_nocturne_app() {
    let cases: &[(&str, usize, usize, usize, usize, &str, &str)] = &[
        // adjacent before: sel=(3,5), rep=(1,3)
        ("01234567", 3, 5, 1, 3, "XY", "0XY34567"),
        // gapped before: sel=(5,7), rep=(1,3)
        ("01234567", 5, 7, 1, 3, "XY", "0XY34567"),
        // adjacent after: sel=(1,3), rep=(3,5)
        ("01234567", 1, 3, 3, 5, "XY", "012XY567"),
        // gapped after: sel=(1,3), rep=(5,7)
        ("01234567", 1, 3, 5, 7, "XY", "01234XY7"),
        // left overlap: sel=(2,6), rep=(1,4)
        ("01234567", 2, 6, 1, 4, "XY", "0XY4567"),
        // right overlap: sel=(2,6), rep=(4,8)
        ("01234567", 2, 6, 4, 8, "XY", "0123XY"),
        // strictly interior: sel=(2,6), rep=(3,5)
        ("01234567", 2, 6, 3, 5, "XY", "012XY567"),
        // prefix interior: sel=(0,11), rep=(0,5)
        ("Hello world", 0, 11, 0, 5, "XY", "XY world"),
    ];

    for &(initial, sel_lo, sel_hi, rep_lo, rep_hi, replacement, expected) in cases {
        let (_dir, mut app, _clip) = make_test_app("ios-bridge-sel-replace");
        let ctx = egui::Context::default();
        focus_block_zero(&ctx, &mut app);

        app.apply_platform_edit_action(PlatformEditAction::SelectAll);
        app.delete_backward_platform();
        app.insert_platform_text(initial);
        step_app_frame(&ctx, &mut app, egui::RawInput::default());
        step_app_frame(&ctx, &mut app, egui::RawInput::default());

        let total_chars = initial.chars().count();
        let mut setup_queue = VecDeque::new();
        let back_steps = total_chars - sel_lo;
        if let Ok(steps) = isize::try_from(back_steps)
            && steps > 0
        {
            setup_queue.push_back(TextEditCommand::MoveCursor(-steps));
        }
        drain_queue_with_focus_lock(&ctx, &mut app, &mut setup_queue);

        for _ in sel_lo..sel_hi {
            let _ = lock_arrow_focus_if_ready(&ctx);
            let mut raw = egui::RawInput::default();
            raw.events.push(egui::Event::Key {
                key: egui::Key::ArrowRight,
                physical_key: Some(egui::Key::ArrowRight),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            });
            raw.events.push(egui::Event::Key {
                key: egui::Key::ArrowRight,
                physical_key: Some(egui::Key::ArrowRight),
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            });
            step_app_frame(&ctx, &mut app, raw);
        }

        let selected = app.selected_text_for_platform();
        let expected_selected: String = initial.chars().skip(sel_lo).take(sel_hi - sel_lo).collect();
        assert_eq!(selected, expected_selected, "failed to establish selection ({sel_lo}, {sel_hi}) on {initial:?}");

        let mut doc = TextDocumentModel::new();
        doc.insert_text(initial);
        doc.set_selection_utf16(sel_lo, sel_hi);
        let mut queue = VecDeque::from(doc.replace_range(rep_lo, rep_hi, replacement));
        drain_queue_with_focus_lock(&ctx, &mut app, &mut queue);

        assert_eq!(first_block_text(&app).as_deref(), Some(expected), "NocturneApp mismatch for sel=({sel_lo},{sel_hi}), rep=({rep_lo},{rep_hi})");
        assert_eq!(doc.text(), expected);
    }
}

fn step_app_frame_with_shell_ime(ctx: &egui::Context, app: &mut NocturneApp, mut raw_input: egui::RawInput) -> (Option<Rect>, bool, Vec<String>) {
    raw_input.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 768.0)));
    raw_input.events.extend(app.take_events());
    let mut ime_cursor_rect = None;
    let mut egui_wants_ime = false;
    let mut copied_texts = Vec::new();
    let mut out = ctx.run_ui(raw_input, |ui| {
        app.show(ui);
        let inner_ctx = ui.ctx().clone();
        ime_cursor_rect = inner_ctx.output_mut(|o| o.ime.take().map(|ime| ime.cursor_rect));
        egui_wants_ime = should_route_ime_to_egui(ime_cursor_rect, app.keyboard_caret_rect(), app.page_rect(), &inner_ctx);
        if egui_wants_ime {
            copied_texts = inner_ctx.output(|o| {
                o.commands
                    .iter()
                    .filter_map(|cmd| match cmd {
                        egui::OutputCommand::CopyText(text) => Some(text.clone()),
                        _ => None,
                    })
                    .collect()
            });
        }
    });
    out.textures_delta.clear();
    (ime_cursor_rect, egui_wants_ime, copied_texts)
}

#[test]
fn shell_ui_ime_routing_preserves_edit_menu_selection_copy_and_routes_external_widgets() {
    let (_dir, mut app, clip) = make_test_app("ios-bridge-shell-ime-routing");
    let ctx = egui::Context::default();

    // 1. Steady-state block 0 keeps egui_wants_ime == false even though pageedit emits o.ime = Some(...).
    focus_block_zero(&ctx, &mut app);
    let (ime_block, wants_ime_block, _) = step_app_frame_with_shell_ime(&ctx, &mut app, egui::RawInput::default());
    assert!(ime_block.is_some(), "expected pageedit TextEdit to populate PlatformOutput.ime on steady-state frame");
    assert!(!wants_ime_block, "steady-state block 0 must keep egui_wants_ime == false");

    let caret = app.keyboard_caret_rect().expect("block 0 caret");
    let page = app.page_rect().expect("page_rect");
    let near_caret = is_long_press_near_caret(
        [caret.min.x + 10.0, caret.min.y + 6.0],
        Some([caret.min.x, caret.min.y, caret.width(), caret.height()]),
        Some([page.min.x, page.min.y, page.width(), page.height()]),
    );
    assert!(near_caret);
    assert!(should_present_native_edit_menu(!wants_ime_block && near_caret, true));

    // Mirror Shell::ui: !egui_wants_ime routes SelectAll + Copy through NocturneApp and SharedClipboard.
    app.apply_platform_edit_action(PlatformEditAction::SelectAll);
    let selected_block = if wants_ime_block { String::new() } else { app.selected_text_for_platform() };
    assert_eq!(selected_block, "Start");
    app.apply_platform_edit_action(PlatformEditAction::Copy);
    assert_eq!(clip.0.borrow().as_deref(), Some("Start"));

    // 2. Steady-state Page Title (frame 1 fallback cr == page_rect & frame 2 localized) keeps egui_wants_ime == false.
    let title_pos = Pos2::new(page.min.x + 24.0, page.min.y + 18.0);
    let mut title_click = egui::RawInput::default();
    title_click.events.push(egui::Event::PointerMoved(title_pos));
    title_click.events.push(egui::Event::PointerButton {
        pos: title_pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    title_click.events.push(egui::Event::PointerButton {
        pos: title_pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    let (_ime_title_f1, wants_ime_title_f1, _) = step_app_frame_with_shell_ime(&ctx, &mut app, title_click);
    assert!(!wants_ime_title_f1, "page title frame 1 (cr == page_rect fallback) must keep egui_wants_ime == false");
    let (ime_title, wants_ime_title, _) = step_app_frame_with_shell_ime(&ctx, &mut app, egui::RawInput::default());
    assert!(ime_title.is_some(), "expected page title TextEdit to populate PlatformOutput.ime");
    assert!(!wants_ime_title, "steady-state page title must keep egui_wants_ime == false");
    app.apply_platform_edit_action(PlatformEditAction::SelectAll);
    let selected_title = if wants_ime_title { String::new() } else { app.selected_text_for_platform() };
    assert_eq!(selected_title, "Scratch");
    app.apply_platform_edit_action(PlatformEditAction::Copy);
    assert_eq!(clip.0.borrow().as_deref(), Some("Scratch"));

    // 3. Re-focus block 0, then trigger top-bar Search (`Cmd+F`) -> ed.caret stays parked, Search sits outside page_rect -> egui_wants_ime == true.
    focus_block_zero(&ctx, &mut app);
    let mut find_raw = egui::RawInput::default();
    find_raw.events.push(egui::Event::Key {
        key: egui::Key::F,
        physical_key: Some(egui::Key::F),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    });
    find_raw.events.push(egui::Event::Key {
        key: egui::Key::F,
        physical_key: Some(egui::Key::F),
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    });
    let _ = step_app_frame_with_shell_ime(&ctx, &mut app, find_raw);
    let (ime_search, wants_ime_search, _) = step_app_frame_with_shell_ime(&ctx, &mut app, egui::RawInput::default());
    assert!(ime_search.is_some(), "expected Search TextEdit to populate PlatformOutput.ime after Cmd+F");
    assert!(wants_ime_search, "top-bar Search TextEdit outside page_rect must set egui_wants_ime == true");

    // 4. Open Link dialog -> Order::Middle dialog sets egui_wants_ime == true.
    focus_block_zero(&ctx, &mut app);
    app.open_link_dialog();
    let _ = step_app_frame_with_shell_ime(&ctx, &mut app, egui::RawInput::default());
    let (ime_link, wants_ime_link, _) = step_app_frame_with_shell_ime(&ctx, &mut app, egui::RawInput::default());
    assert!(ime_link.is_some(), "expected Link dialog TextEdit to populate PlatformOutput.ime");
    assert!(wants_ime_link, "open_link_dialog() must set egui_wants_ime == true");
}

#[test]
fn scrolled_long_page_routes_cmd_f_search_to_egui_and_returns_cleanly_to_block() {
    let (_dir, mut app, _clip) = make_test_app("ios-bridge-scrolled-search-ime");
    let ctx = egui::Context::default();

    let long_body: String = (0..120).map(|i| format!("Paragraph {i}\n\n")).collect();
    app.set_body(&long_body);
    focus_block_zero(&ctx, &mut app);

    let (ime_before, wants_before, _) = step_app_frame_with_shell_ime(&ctx, &mut app, egui::RawInput::default());
    assert!(ime_before.is_some());
    assert!(!wants_before, "focused block 0 on unscrolled page must keep egui_wants_ime == false");
    assert_eq!(first_block_text(&app).as_deref(), Some("Paragraph 0"));

    // Scroll down via external pan until page_rect translates upward behind the top bar (min.y < 20.0).
    for _ in 0..20 {
        app.apply_external_pan(Vec2::new(0.0, -50.0));
        let _ = step_app_frame_with_shell_ime(&ctx, &mut app, egui::RawInput::default());
        if app.page_rect().is_some_and(|r| r.min.y < 20.0) {
            break;
        }
    }
    let scrolled_page = app.page_rect().expect("scrolled page_rect");
    assert!(scrolled_page.min.y < 20.0, "expected scrolled page_rect.min.y < 20.0, got {:?}", scrolled_page);

    // Trigger Cmd+F Search while block 0 caret is still parked; Search sits in top bar (y < 30).
    let mut find_raw = egui::RawInput::default();
    find_raw.events.push(egui::Event::Key {
        key: egui::Key::F,
        physical_key: Some(egui::Key::F),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    });
    find_raw.events.push(egui::Event::Key {
        key: egui::Key::F,
        physical_key: Some(egui::Key::F),
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    });
    let _ = step_app_frame_with_shell_ime(&ctx, &mut app, find_raw);
    let (ime_search, wants_search, _) = step_app_frame_with_shell_ime(&ctx, &mut app, egui::RawInput::default());
    let ime_search_rect = ime_search.expect("Search TextEdit must emit PlatformOutput.ime");
    assert!(
        scrolled_page.expand(4.0).contains(ime_search_rect.center()),
        "test precondition: scrolled page_rect ({scrolled_page:?}) overlaps Search ({ime_search_rect:?})"
    );
    assert!(wants_search, "Cmd+F Search on scrolled page must set egui_wants_ime == true");

    // Typing + SelectAll + Copy while egui_wants_ime == true goes to Search (CopyText) and never mutates block 0.
    let mut type_raw = egui::RawInput::default();
    type_raw.events.push(egui::Event::Text("query".to_owned()));
    let (_, wants_after_type, _) = step_app_frame_with_shell_ime(&ctx, &mut app, type_raw);
    assert!(wants_after_type, "Search typing must keep egui_wants_ime == true");
    assert_eq!(first_block_text(&app).as_deref(), Some("Paragraph 0"), "Search typing must not mutate block 0");

    let mut copy_raw = egui::RawInput::default();
    nocturne_ios::text_input::push_edit_action_to_egui(&mut copy_raw.events, PlatformEditAction::SelectAll, None);
    nocturne_ios::text_input::push_edit_action_to_egui(&mut copy_raw.events, PlatformEditAction::Copy, None);
    let (_, wants_after_copy, copied) = step_app_frame_with_shell_ime(&ctx, &mut app, copy_raw);
    assert!(wants_after_copy);
    assert_eq!(copied, vec!["query".to_owned()]);
    assert_eq!(first_block_text(&app).as_deref(), Some("Paragraph 0"));

    // While still scrolled (page_rect.min.y < 20.0), tapping the Font Size ribbon TextEdit also
    // routes IME to egui instead of falling back to scrolled page_rect containment.
    let font_size_pos = Pos2::new(415.4, 71.0);
    let mut ribbon_click = egui::RawInput::default();
    ribbon_click.events.push(egui::Event::PointerMoved(font_size_pos));
    ribbon_click.events.push(egui::Event::PointerButton {
        pos: font_size_pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    ribbon_click.events.push(egui::Event::PointerButton {
        pos: font_size_pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    let _ = step_app_frame_with_shell_ime(&ctx, &mut app, ribbon_click);
    let (ime_ribbon, wants_ribbon, _) = step_app_frame_with_shell_ime(&ctx, &mut app, egui::RawInput::default());
    let ime_ribbon_rect = ime_ribbon.expect("Font Size ribbon TextEdit must emit PlatformOutput.ime");
    assert!(
        ime_ribbon_rect.center().y > scrolled_page.min.y,
        "test precondition: scrolled page_rect ({scrolled_page:?}) moved above ribbon Font Size ({ime_ribbon_rect:?})"
    );
    assert!(wants_ribbon, "ribbon Font Size TextEdit on scrolled page must set egui_wants_ime == true");

    // Scroll back to top and return focus to block 0 -> egui_wants_ime returns to false.
    for _ in 0..20 {
        app.apply_external_pan(Vec2::new(0.0, 50.0));
        let _ = step_app_frame_with_shell_ime(&ctx, &mut app, egui::RawInput::default());
    }
    focus_block_zero(&ctx, &mut app);
    let (ime_returned, wants_returned, _) = step_app_frame_with_shell_ime(&ctx, &mut app, egui::RawInput::default());
    assert!(ime_returned.is_some());
    assert!(!wants_returned, "returning from scrolled Search to block 0 must restore egui_wants_ime == false");
}

#[test]
fn replace_range_while_marked_active_end_to_end_in_nocturne_and_egui() {
    // Part 1: NocturneApp (!egui_wants_ime) Case A & Case B
    for (label, rep_lo, rep_hi, repl, expected) in [("case-a", 5, 7, "你", "hello你"), ("case-b", 0, 2, "HE", "HElloni")] {
        let (_dir, mut app, _clip) = make_test_app(&format!("ios-bridge-marked-app-{label}"));
        let ctx = egui::Context::default();
        focus_block_zero(&ctx, &mut app);
        app.apply_platform_edit_action(PlatformEditAction::SelectAll);
        app.delete_backward_platform();

        let mut doc = TextDocumentModel::new();
        let mut queue = VecDeque::new();
        queue.extend(doc.insert_text("hello"));
        queue.push_back(doc.set_marked_text(Some("ni"), (2, 2)));
        drain_queue_with_focus_lock(&ctx, &mut app, &mut queue);

        queue.extend(doc.replace_range(rep_lo, rep_hi, repl));
        drain_queue_with_focus_lock(&ctx, &mut app, &mut queue);

        assert_eq!(first_block_text(&app).as_deref(), Some(expected), "NocturneApp mismatch for {label}");
        assert_eq!(doc.text(), expected, "doc mismatch for {label}");
        assert_eq!(doc.marked_range_utf16(), None);
    }

    // Part 2: Live egui::TextEdit (egui_wants_ime == true) Case A & Case B
    for (label, rep_lo, rep_hi, repl, expected) in [("case-a", 5, 7, "你", "hello你"), ("case-b", 0, 2, "HE", "HElloni")] {
        let (_dir, mut app, _clip) = make_test_app(&format!("ios-bridge-marked-egui-{label}"));
        let ctx = egui::Context::default();
        let mut buf = String::new();
        let mut preedit = false;
        let mut focus_wait = 0u8;
        let edit_id = egui::Id::unique(format!("marked-egui-{label}"));

        let mut step_egui = |queue: &mut VecDeque<TextEditCommand>, buf: &mut String, preedit: &mut bool, focus_wait: &mut u8| {
            for _ in 0..32 {
                let mut raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0))), ..Default::default() };
                let repaint = dispatch_pending_text_commands_with_context(Some(&ctx), focus_wait, queue, true, preedit, &mut app, &mut raw, || None);
                let mut out = ctx.run_ui(raw, |ui| {
                    let resp = ui.add(egui::TextEdit::multiline(buf).id(edit_id));
                    if !ui.memory(|m| m.has_focus(edit_id)) {
                        resp.request_focus();
                    }
                });
                out.textures_delta.clear();
                if queue.is_empty() && !repaint {
                    break;
                }
            }
            assert!(queue.is_empty(), "egui TextEdit queue did not drain for {label}: {queue:?}");
        };

        let mut doc = TextDocumentModel::new();
        let mut queue = VecDeque::new();
        step_egui(&mut queue, &mut buf, &mut preedit, &mut focus_wait);

        queue.extend(doc.insert_text("hello"));
        queue.push_back(doc.set_marked_text(Some("ni"), (2, 2)));
        step_egui(&mut queue, &mut buf, &mut preedit, &mut focus_wait);
        assert!(preedit, "expected active preedit before replace_range for {label}");

        queue.extend(doc.replace_range(rep_lo, rep_hi, repl));
        step_egui(&mut queue, &mut buf, &mut preedit, &mut focus_wait);

        assert_eq!(buf, expected, "egui::TextEdit mismatch for {label}");
        assert_eq!(doc.text(), expected, "doc mismatch for {label}");
        assert!(!preedit, "egui_ime_preedit must be cleared after replace_range for {label}");
        assert_eq!(doc.marked_range_utf16(), None);
    }
}
