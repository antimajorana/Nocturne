//! iPad Studio UX v2, phase 1: the keyboard keeps its spaces, dialogs take the keyboard once,
//! the squeezed status box is gone, a held finger's menu stays open, and a resting palm does
//! not draw.
//!
//! Each test drives the real `NocturneApp` with the events the iPad sends. Nothing is mocked.

use egui::os::OperatingSystem;
use egui::{Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use nocturne_engine::{InkActiveMode, Session};
use nocturne_model::NodeKind;
use nocturne_store::Settings;
use nocturne_store::testing::TempDir;
use nocturne_ui_egui::touch_ux::TouchLongPress;
use nocturne_ui_egui::{NocturneApp, PlatformEditAction, PlatformTextField, ribbon};
use std::path::{Path, PathBuf};

const SCREEN: egui::Vec2 = vec2(1180.0, 820.0);

fn frame_at(ctx: &egui::Context, app: &mut NocturneApp, time: Option<f64>, mut events: Vec<egui::Event>) {
    events.extend(app.take_events());
    let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)), time, events, ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| app.show(ui));
    out.textures_delta.clear();
}

fn frame(ctx: &egui::Context, app: &mut NocturneApp, events: Vec<egui::Event>) {
    frame_at(ctx, app, None, events);
}

fn frames(ctx: &egui::Context, app: &mut NocturneApp, count: usize) {
    for _ in 0..count {
        frame(ctx, app, Vec::new());
    }
}

fn ipad_session(tmp: &TempDir) -> (Session, PathBuf) {
    let settings = Settings::at(tmp.path().join("cfg/settings.toml"));
    settings.set("privacy_welcome_seen", "true").expect("welcome seen");
    settings.set_default_format(nocturne_model::PageFormat::Cmgfm).expect("cmgfm default");
    let mut session = Session::new(&tmp.path().join("notebooks"), settings).expect("session");
    session.run("notebook.create", &[("name".into(), "Field Notes".into())].into_iter().collect()).expect("create notebook");
    let page = session.page_to_show("field-notes").expect("first page");
    (session, page)
}

fn ipad_app(name: &str) -> (TempDir, egui::Context, NocturneApp, PathBuf) {
    let tmp = TempDir::new(name);
    let (session, page) = ipad_session(&tmp);
    let mut app = NocturneApp::new(session);
    app.open_path(&page.to_string_lossy());
    let ctx = egui::Context::default();
    ctx.set_os(OperatingSystem::IOS);
    frames(&ctx, &mut app, 3);
    assert!(app.is_tablet_studio(&ctx), "an iPad opens in Tablet Studio");
    (tmp, ctx, app, page)
}

fn focus_title(ctx: &egui::Context, app: &mut NocturneApp) {
    ctx.memory_mut(|m| m.request_focus(egui::Id::unique("page-title-box")));
    frames(ctx, app, 2);
    assert!(app.title_is_focused(), "the title has the keyboard");
}

fn touch(id: u64, phase: egui::TouchPhase, pos: Pos2, force: Option<f32>) -> egui::Event {
    egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(id), phase, pos, force }
}

fn button(button: egui::PointerButton, pressed: bool, pos: Pos2) -> egui::Event {
    egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::NONE }
}

fn primary(pressed: bool, pos: Pos2) -> egui::Event {
    button(egui::PointerButton::Primary, pressed, pos)
}

fn area_rect(ctx: &egui::Context, id: egui::Id) -> Option<Rect> {
    ctx.memory(|m| m.area_rect(id))
}

// ---------------------------------------------------------------------------------------------
// 1A. The keyboard: spaces, the field the keyboard sees, and focus taken once.
// ---------------------------------------------------------------------------------------------

/// Screenshot 2: "Blue Lagoon" typed on the iPad keyboard arrived as "BlueLagoon", because a
/// space typed on its own was trimmed to nothing.
#[test]
fn page_title_keeps_spaces_typed_on_the_platform_keyboard() {
    let (_tmp, ctx, mut app, _page) = ipad_app("p1-title-spaces");
    focus_title(&ctx, &mut app);
    app.apply_platform_edit_action(PlatformEditAction::SelectAll);
    frames(&ctx, &mut app, 2);

    for piece in ["Blue", " ", "Lagoon", " ", " ", "x"] {
        app.insert_platform_text(piece);
        frames(&ctx, &mut app, 1);
    }
    assert_eq!(app.title_draft(), Some("Blue Lagoon  x"), "every space typed stays where it was typed");

    // A line break typed or pasted into the one-line title is a space, and the spaces round it stay.
    app.insert_platform_text(" a\nb");
    frames(&ctx, &mut app, 1);
    assert_eq!(app.title_draft(), Some("Blue Lagoon  x a b"));
}

/// The same through egui's own text events, which is how the title is typed once the keyboard
/// is routed to the title's text box. The saved title has its outer spaces trimmed.
#[test]
fn page_title_typed_through_egui_keeps_inner_spaces_and_trims_on_commit() {
    let (_tmp, ctx, mut app, _page) = ipad_app("p1-title-egui");
    focus_title(&ctx, &mut app);
    frame(
        &ctx,
        &mut app,
        vec![egui::Event::Key {
            key: egui::Key::A,
            physical_key: Some(egui::Key::A),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }],
    );
    for piece in ["  Lemon", " ", "tare "] {
        frame(&ctx, &mut app, vec![egui::Event::Text(piece.to_string())]);
    }
    assert_eq!(app.title_draft(), Some("  Lemon tare "), "nothing is trimmed while typing");

    let enter = |pressed| egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: Some(egui::Key::Enter),
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&ctx, &mut app, vec![enter(true), enter(false)]);
    frames(&ctx, &mut app, 3);
    assert_eq!(app.title(), Some("Lemon tare"), "the saved title is trimmed at its ends only");
}

/// The keyboard must be told the whole field and the real selection. With an empty field it
/// thinks there is nothing to delete, and it cannot find word boundaries for its suggestions.
#[test]
fn the_focused_field_is_reported_with_its_text_and_selection() {
    let (_tmp, ctx, mut app, page) = ipad_app("p1-focused-field");
    assert_eq!(app.focused_text_field(&ctx), None, "nothing has the keyboard yet");

    app.open_rename_dialog(page.clone(), "Lemon tare".to_string());
    frames(&ctx, &mut app, 3);
    assert_eq!(
        app.focused_text_field(&ctx),
        Some(PlatformTextField { text: "Lemon tare".to_string(), selection: (10, 10) }),
        "the name being renamed is there before anything is typed, with the caret at its end"
    );
    frame(&ctx, &mut app, vec![egui::Event::Text(" x".to_string())]);
    frames(&ctx, &mut app, 1);
    assert_eq!(app.focused_text_field(&ctx), Some(PlatformTextField { text: "Lemon tare x".to_string(), selection: (12, 12) }));

    let (_tmp2, ctx2, mut app2, _page2) = ipad_app("p1-focused-title");
    focus_title(&ctx2, &mut app2);
    let field = app2.focused_text_field(&ctx2).expect("the title is a field");
    assert_eq!(Some(field.text.as_str()), app2.title_draft());
}

fn assert_takes_focus_once(ctx: &egui::Context, app: &mut NocturneApp, what: &str) {
    frames(ctx, app, 3);
    let field = ctx.memory(|m| m.focused()).unwrap_or_else(|| panic!("{what}: the text box has the keyboard when the dialog opens"));
    // The user puts the keyboard away. Asking for focus again on every frame brought it straight
    // back, which on an iPad is the keyboard dropping and rising again.
    ctx.memory_mut(|m| m.surrender_focus(field));
    frames(ctx, app, 3);
    assert_ne!(ctx.memory(|m| m.focused()), Some(field), "{what}: focus is asked for once, when the dialog opens");
}

#[test]
fn name_dialogs_take_the_keyboard_once_and_sit_on_a_modal_backdrop() {
    let (_tmp, ctx, mut app, page) = ipad_app("p1-dialog-focus");
    let section = page.parent().expect("section").to_path_buf();

    app.open_rename_dialog(page.clone(), "Lemon tare".to_string());
    assert_takes_focus_once(&ctx, &mut app, "Rename");
    assert!(area_rect(&ctx, egui::Id::unique("rename-modal-backdrop")).is_some(), "Rename dims the window behind it");
    frame(
        &ctx,
        &mut app,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: Some(egui::Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );

    let (_tmp, ctx, mut app, _page) = ipad_app("p1-dialog-focus-add");
    app.open_add_dialog(section.clone(), NodeKind::Section, NodeKind::Page);
    assert_takes_focus_once(&ctx, &mut app, "Add");
    assert!(area_rect(&ctx, egui::Id::unique("add-modal-backdrop")).is_some(), "Add dims the window behind it");

    let (_tmp, ctx, mut app, _page) = ipad_app("p1-dialog-focus-notebook");
    app.open_new_notebook_dialog();
    assert_takes_focus_once(&ctx, &mut app, "New notebook");
}

/// Typing through the platform keyboard still saves and still undoes as one run of writing.
#[test]
fn platform_typing_in_the_page_marks_it_to_save_and_undoes_as_a_run() {
    let (_tmp, ctx, mut app, _page) = ipad_app("p1-typing");
    let page = app.page_rect().expect("page");
    app.ribbon_act_for_test(ribbon::Act::InkPointer(ribbon::Pointer::Write));
    frames(&ctx, &mut app, 1);
    let at = pos2(page.center().x, page.min.y + 260.0);
    frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at), primary(true, at)]);
    frame(&ctx, &mut app, vec![primary(false, at)]);
    frames(&ctx, &mut app, 2);
    assert!(app.keyboard_caret_rect().is_some(), "a click on the page puts the caret there");

    let before = app.body().unwrap_or_default().to_string();
    for piece in ["Blue", " ", "lagoons"] {
        app.insert_platform_text(piece);
        frames(&ctx, &mut app, 1);
    }
    let typed = app.body().unwrap_or_default().to_string();
    assert!(typed.contains("Blue lagoons"), "the words are in the page: {typed:?}");
    app.delete_backward_platform();
    frames(&ctx, &mut app, 1);
    assert!(app.body().unwrap_or_default().contains("Blue lagoon"), "delete takes one letter");
    assert!(!app.body().unwrap_or_default().contains("Blue lagoons"));

    assert_ne!(app.body().unwrap_or_default(), before);

    app.ribbon_act_for_test(ribbon::Act::Undo);
    frames(&ctx, &mut app, 2);
    assert!(!app.body().unwrap_or_default().contains("Blue"), "one undo takes back the whole run of typing: {:?}", app.body());
}

// ---------------------------------------------------------------------------------------------
// 1B. The squeezed status box.
// ---------------------------------------------------------------------------------------------

#[test]
fn studio_shows_no_floating_box_for_routine_messages() {
    let (_tmp, ctx, mut app, _page) = ipad_app("p1-no-toast");
    app.say("Drawing saved, 2 strokes");
    frames(&ctx, &mut app, 3);
    assert_eq!(area_rect(&ctx, egui::Id::unique("studio-transient-notice")), None, "the old toast is gone");
    assert_eq!(area_rect(&ctx, egui::Id::unique("studio-error-banner")), None, "a routine message is not an error");
}

#[test]
fn studio_shows_a_real_failure_on_one_line_until_it_is_dismissed() {
    let (_tmp, ctx, mut app, _page) = ipad_app("p1-error-banner");
    app.copy_link_to(Path::new("C:/not/inside/any/notebook"));
    frames(&ctx, &mut app, 3);
    let banner = area_rect(&ctx, egui::Id::unique("studio-error-banner")).expect("a failure is shown");
    assert!(banner.width() >= 240.0, "wide enough to read: {banner:?}");
    assert!(banner.height() <= 48.0, "one line, not a column of letters: {banner:?}");
    assert!(Rect::from_min_size(Pos2::ZERO, SCREEN).contains_rect(banner), "on the screen: {banner:?}");
    assert!(app.status_banner_showing());

    let at = banner.center();
    frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at), primary(true, at)]);
    frame(&ctx, &mut app, vec![primary(false, at), egui::Event::PointerGone]);
    frames(&ctx, &mut app, 2);
    assert!(!app.status_banner_showing(), "a tap dismisses it");
    // A routine message after it does not bring a banner back.
    app.say("Renamed");
    frames(&ctx, &mut app, 2);
    assert!(!app.status_banner_showing());
}

// ---------------------------------------------------------------------------------------------
// 1C. A held finger's menu.
// ---------------------------------------------------------------------------------------------

/// One frame as the iPad shell runs it: what is left of a used-up touch is dropped, then a long
/// press that comes due becomes a context click.
fn shell_frame(ctx: &egui::Context, app: &mut NocturneApp, lp: &mut TouchLongPress, time: f64, mut events: Vec<egui::Event>) {
    lp.filter_consumed(&mut events);
    if let Some(at) = lp.update(time, &events) {
        lp.consume_as_context_click(&mut events, at);
    }
    frame_at(ctx, app, Some(time), events);
}

fn finger_tap(ctx: &egui::Context, app: &mut NocturneApp, lp: &mut TouchLongPress, id: u64, time: f64, at: Pos2) {
    shell_frame(ctx, app, lp, time, vec![touch(id, egui::TouchPhase::Start, at, None), egui::Event::PointerMoved(at), primary(true, at)]);
    shell_frame(ctx, app, lp, time + 0.08, vec![touch(id, egui::TouchPhase::End, at, None), primary(false, at), egui::Event::PointerGone]);
    shell_frame(ctx, app, lp, time + 0.12, Vec::new());
}

fn finger_hold_and_lift(ctx: &egui::Context, app: &mut NocturneApp, lp: &mut TouchLongPress, id: u64, start: f64, at: Pos2) {
    shell_frame(ctx, app, lp, start, vec![touch(id, egui::TouchPhase::Start, at, None), egui::Event::PointerMoved(at), primary(true, at)]);
    shell_frame(ctx, app, lp, start + 0.30, Vec::new());
    shell_frame(ctx, app, lp, start + 0.55, Vec::new());
    shell_frame(ctx, app, lp, start + 0.60, Vec::new());
    shell_frame(ctx, app, lp, start + 0.90, vec![touch(id, egui::TouchPhase::End, at, None), primary(false, at), egui::Event::PointerGone]);
    for i in 1..=3 {
        shell_frame(ctx, app, lp, start + 0.90 + 0.05 * f64::from(i), Vec::new());
    }
}

/// Screenshot 1: the menu a held finger opens closed again as the finger lifted.
#[test]
fn a_held_fingers_page_menu_stays_open_after_the_lift_and_takes_a_tap() {
    let (_tmp, ctx, mut app, _page) = ipad_app("p1-long-press");
    app.ribbon_act_for_test(ribbon::Act::InkPointer(ribbon::Pointer::Write));
    frames(&ctx, &mut app, 2);
    let page = app.page_rect().expect("page");
    let at = pos2(page.min.x + 220.0, page.min.y + 300.0);
    let mut lp = TouchLongPress::default();

    finger_hold_and_lift(&ctx, &mut app, &mut lp, 1, 1.0, at);
    assert!(nocturne_ui_egui::pagemenu::is_open(&ctx), "the menu is still open after the finger lifts");
    assert!(app.keyboard_caret_rect().is_none(), "the lift is not a tap on the page under the menu");

    // A tap outside closes it.
    let outside = pos2(page.max.x - 60.0, page.min.y + 120.0);
    finger_tap(&ctx, &mut app, &mut lp, 2, 3.0, outside);
    assert!(!nocturne_ui_egui::pagemenu::is_open(&ctx), "a new tap away from the menu closes it");

    // Open it again; a tap on one of its rows picks the row.
    finger_hold_and_lift(&ctx, &mut app, &mut lp, 3, 5.0, at);
    assert!(nocturne_ui_egui::pagemenu::is_open(&ctx));
    let menu = area_rect(&ctx, egui::Id::unique("page-menu").with("area")).expect("the menu has a place");
    let row = pos2(menu.center().x, menu.min.y + 0.42 * menu.height());
    let start = egui::TouchPhase::Start;
    shell_frame(&ctx, &mut app, &mut lp, 7.0, vec![touch(4, start, row, None), egui::Event::PointerMoved(row), primary(true, row)]);
    assert!(nocturne_ui_egui::pagemenu::is_open(&ctx), "a press on the menu is not a press outside it");
    shell_frame(&ctx, &mut app, &mut lp, 7.08, vec![touch(4, egui::TouchPhase::End, row, None), primary(false, row), egui::Event::PointerGone]);
    shell_frame(&ctx, &mut app, &mut lp, 7.12, Vec::new());
    assert!(!nocturne_ui_egui::pagemenu::is_open(&ctx), "the tap picked the row, and picking a row closes the menu");
}

/// With a pen in hand a held finger must not put the pen away: the lift used to count as a tap,
/// and a tap on the page switches to writing.
#[test]
fn a_held_finger_with_a_pen_selected_keeps_the_pen() {
    let (_tmp, ctx, mut app, _page) = ipad_app("p1-long-press-pen");
    app.ribbon_act_for_test(ribbon::Act::InkPen(0));
    frames(&ctx, &mut app, 2);
    assert_eq!(app.session().ink_tool_state().active_mode, InkActiveMode::Draw);
    let page = app.page_rect().expect("page");
    let at = pos2(page.min.x + 220.0, page.min.y + 300.0);
    let mut lp = TouchLongPress::default();
    finger_hold_and_lift(&ctx, &mut app, &mut lp, 1, 1.0, at);
    assert_eq!(app.session().ink_tool_state().active_mode, InkActiveMode::Draw, "the pen is still in hand");
    assert_eq!(app.ink_stroke_count(), 0, "and the finger drew nothing");
}

// ---------------------------------------------------------------------------------------------
// 1D. Palm and pen, and where the finger drawing switch lives.
// ---------------------------------------------------------------------------------------------

/// Screenshot 3. The window layer follows only the first contact with its pointer. A palm that
/// lands before the pen is that contact, so the pointer stays with the palm while the pen
/// writes. The stroke must be where the pen went.
#[test]
fn a_resting_palm_does_not_draw_and_the_pen_draws_where_the_pen_is() {
    let (_tmp, ctx, mut app, _page) = ipad_app("p1-palm");
    assert!(!app.is_finger_drawing());
    app.ribbon_act_for_test(ribbon::Act::InkPen(0));
    frames(&ctx, &mut app, 2);
    let page = app.page_rect().expect("page");
    let palm = pos2(page.center().x + 180.0, page.min.y + 520.0);
    let pen: Vec<Pos2> = (0..=8).map(|i| pos2(page.center().x - 120.0 + 12.0 * i as f32, page.min.y + 220.0)).collect();

    // The palm lands: touch 1, and the only pointer the window will make.
    frame(&ctx, &mut app, vec![touch(1, egui::TouchPhase::Start, palm, None), egui::Event::PointerMoved(palm), primary(true, palm)]);
    frame(&ctx, &mut app, Vec::new());
    assert_eq!(app.ink_stroke_count(), 0);

    // The pen lands and writes: touch 2, with a force and no pointer events. The palm rolls a little.
    frame(&ctx, &mut app, vec![touch(2, egui::TouchPhase::Start, pen[0], Some(0.5))]);
    for (i, p) in pen.iter().enumerate().skip(1) {
        let roll = palm + vec2(0.4 * i as f32, 0.3 * i as f32);
        frame(
            &ctx,
            &mut app,
            vec![touch(1, egui::TouchPhase::Move, roll, None), egui::Event::PointerMoved(roll), touch(2, egui::TouchPhase::Move, *p, Some(0.6))],
        );
    }
    let last = pen[pen.len() - 1];
    frame(&ctx, &mut app, vec![touch(2, egui::TouchPhase::End, last, Some(0.0))]);
    frames(&ctx, &mut app, 2);
    frame(&ctx, &mut app, vec![touch(1, egui::TouchPhase::End, palm, None), primary(false, palm), egui::Event::PointerGone]);
    frames(&ctx, &mut app, 2);

    assert_eq!(app.ink_stroke_count(), 1, "one stroke, the pen's");
    let [x0, y0, x1, y1] = app.ink_bounds().expect("ink bounds");
    let origin = app.editor_origin().expect("page origin");
    let zoom = (x1 - x0).max(1.0) / 96.0;
    assert!((0.5..=2.0).contains(&zoom), "the stroke is as long as the pen's path (96 points on screen): {x0}..{x1}");
    let (sy0, sy1) = (origin.y + y0 * zoom, origin.y + y1 * zoom);
    assert!((sy0 - pen[0].y).abs() < 20.0 && (sy1 - pen[0].y).abs() < 20.0, "it lies on the pen's line, not at the palm: y {sy0}..{sy1}");
    let sx0 = origin.x + x0 * zoom;
    assert!((sx0 - pen[0].x).abs() < 20.0, "it starts where the pen came down: x {sx0} vs {}", pen[0].x);
}

fn studio_harness(name: &str) -> (TempDir, Harness<'static, NocturneApp>) {
    let tmp = TempDir::new(name);
    let (session, page) = ipad_session(&tmp);
    let mut app = NocturneApp::new(session);
    app.open_path(&page.to_string_lossy());
    let mut h = Harness::builder().with_size(SCREEN).with_accessibility_check(false).build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
    h.ctx.set_os(OperatingSystem::IOS);
    for _ in 0..4 {
        h.step();
    }
    (tmp, h)
}

fn labelled(h: &Harness<'_, NocturneApp>, matches: impl Fn(&str) -> bool) -> Option<Rect> {
    let ppp = h.ctx.pixels_per_point();
    h.query_all(egui_kittest::kittest::by()).find_map(|node| {
        let label = node.accesskit_node().label()?;
        if !matches(label.trim()) {
            return None;
        }
        let b = node.accesskit_node().bounding_box()?;
        let rect = Rect::from_min_max(pos2(b.x0 as f32 / ppp, b.y0 as f32 / ppp), pos2(b.x1 as f32 / ppp, b.y1 as f32 / ppp));
        rect.is_positive().then_some(rect)
    })
}

fn click(h: &mut Harness<'_, NocturneApp>, at: Pos2) {
    h.input_mut().events.extend([egui::Event::PointerMoved(at), primary(true, at)]);
    h.step();
    h.input_mut().events.extend([primary(false, at)]);
    h.step();
    h.step();
}

/// The hand on the tool pill meant "pan" on the desktop and "draw with a finger" here, and said
/// neither. It is gone from the pill; the choice is a labelled switch.
#[test]
fn studio_has_a_labelled_draw_with_finger_switch_and_no_hand_on_the_tool_pill() {
    let (_tmp, mut h) = studio_harness("p1-finger-switch");
    assert!(h.state().is_tablet_studio(&h.ctx));
    assert!(!h.state().is_finger_drawing(), "off by default");
    assert_eq!(labelled(&h, |l| l == "Draw with one finger instead of scrolling"), None, "the unlabelled hand is off the tool pill");

    // Its home is the Tool Settings screen, behind the three dots on the tool bar.
    let dots = labelled(&h, |l| l == "Tool settings").expect("the way to Tool Settings");
    click(&mut h, dots.center());
    h.step();
    h.step();
    let switch = labelled(&h, |l| l == "Draw with finger").expect("a labelled Draw with finger switch");
    click(&mut h, switch.center());
    assert!(h.state().is_finger_drawing(), "the switch turns finger drawing on");
}

#[test]
fn finger_drawing_is_remembered_by_the_next_window() {
    let tmp = TempDir::new("p1-finger-persist");
    let (mut session, _page) = ipad_session(&tmp);
    assert!(!session.finger_drawing());
    session.run("ink.finger_drawing", &[("enabled".into(), "true".into())].into_iter().collect()).expect("turn on");
    drop(session);
    let again = Session::new(&tmp.path().join("notebooks"), Settings::at(tmp.path().join("cfg/settings.toml"))).expect("second session");
    assert!(again.finger_drawing(), "the choice is in settings.toml");
}

/// Asking for the keyboard once must not cost the desktop its habits: a click on another part of
/// the dialog takes the keyboard from the name box, as any click elsewhere does, and the box
/// gets it back, so typing and Enter go on working.
#[test]
fn a_click_elsewhere_in_a_name_dialog_gives_the_keyboard_back_to_the_name() {
    let tmp = TempDir::new("p1-dialog-refocus");
    let (session, _page) = ipad_session(&tmp);
    let mut app = NocturneApp::new(session);
    let ctx = egui::Context::default();
    frames(&ctx, &mut app, 3);
    app.open_new_notebook_dialog();
    frames(&ctx, &mut app, 3);
    let field = ctx.memory(|m| m.focused()).expect("the name box has the keyboard");
    frame(&ctx, &mut app, vec![egui::Event::Text("Zeta".to_string())]);

    // A click on the dialog's own label, just above the box.
    let label = ctx.read_response(field).expect("the name box is drawn").rect.left_top() + vec2(12.0, -10.0);
    frame(&ctx, &mut app, vec![egui::Event::PointerMoved(label), primary(true, label)]);
    frame(&ctx, &mut app, vec![primary(false, label)]);
    frames(&ctx, &mut app, 2);
    assert_eq!(ctx.memory(|m| m.focused()), Some(field), "the name box has the keyboard again");

    let enter = |pressed| egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: Some(egui::Key::Enter),
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&ctx, &mut app, vec![egui::Event::Text(" Two".to_string())]);
    frame(&ctx, &mut app, vec![enter(true), enter(false)]);
    frames(&ctx, &mut app, 3);
    assert!(app.session().tree().iter().any(|n| n.info.title == "Zeta Two"), "Enter created the notebook that was named");
}
