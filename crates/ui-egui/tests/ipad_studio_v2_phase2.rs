//! iPad Studio UX v2, phase 2: an eraser that takes a whole stroke or only what is under it,
//! with a size of its own, and a pen that straightens its stroke when it is held still.
//!
//! Each test drives the real `NocturneApp`. Nothing is mocked.

use egui::os::OperatingSystem;
use egui::{Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use nocturne_engine::{EraserKind, Session};
use nocturne_store::Settings;
use nocturne_store::testing::TempDir;
use nocturne_ui_egui::{NocturneApp, ribbon};
use std::path::PathBuf;

const SCREEN: egui::Vec2 = vec2(1180.0, 820.0);

fn frame_at(ctx: &egui::Context, app: &mut NocturneApp, time: f64, mut events: Vec<egui::Event>) {
    events.extend(app.take_events());
    let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)), time: Some(time), events, ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| app.show(ui));
    out.textures_delta.clear();
}

fn session_with_page(tmp: &TempDir) -> (Session, PathBuf) {
    let settings = Settings::at(tmp.path().join("cfg/settings.toml"));
    settings.set("privacy_welcome_seen", "true").expect("welcome seen");
    settings.set_default_format(nocturne_model::PageFormat::Cmgfm).expect("cmgfm default");
    let mut session = Session::new(&tmp.path().join("notebooks"), settings).expect("session");
    session.run("notebook.create", &[("name".into(), "Field Notes".into())].into_iter().collect()).expect("create notebook");
    let page = session.page_to_show("field-notes").expect("first page");
    (session, page)
}

/// A window on an iPad with a page open, and a clock the test moves.
struct Desk {
    _tmp: TempDir,
    ctx: egui::Context,
    app: NocturneApp,
    now: f64,
}

impl Desk {
    fn new(name: &str) -> Self {
        let tmp = TempDir::new(name);
        let (session, page) = session_with_page(&tmp);
        let mut app = NocturneApp::new(session);
        app.open_path(&page.to_string_lossy());
        let ctx = egui::Context::default();
        ctx.set_os(OperatingSystem::IOS);
        let mut desk = Self { _tmp: tmp, ctx, app, now: 1.0 };
        desk.idle(3);
        desk
    }

    fn step(&mut self, dt: f64, events: Vec<egui::Event>) {
        self.now += dt;
        frame_at(&self.ctx, &mut self.app, self.now, events);
    }

    fn idle(&mut self, frames: usize) {
        for _ in 0..frames {
            self.step(0.02, Vec::new());
        }
    }

    fn run(&mut self, command: &str, params: &[(&str, &str)]) {
        let params = params.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
        self.app.session_mut().run(command, &params).unwrap_or_else(|e| panic!("{command}: {e}"));
    }

    fn pick(&mut self, act: ribbon::Act) {
        self.app.ribbon_act_for_test(act);
        self.idle(2);
    }

    fn press(&mut self, at: Pos2) {
        self.step(0.02, vec![egui::Event::PointerMoved(at), button(true, at)]);
    }

    fn move_to(&mut self, dt: f64, at: Pos2) {
        self.step(dt, vec![egui::Event::PointerMoved(at)]);
    }

    fn lift(&mut self, at: Pos2) {
        self.step(0.02, vec![button(false, at)]);
        self.idle(3);
    }

    /// Press at the first point, pass through the rest, lift at the last.
    fn drag(&mut self, path: &[Pos2]) {
        self.press(path[0]);
        for at in &path[1..] {
            self.move_to(0.02, *at);
        }
        self.lift(path[path.len() - 1]);
    }

    /// A place on open paper, given from the page's top left corner.
    fn paper(&self, dx: f32, dy: f32) -> Pos2 {
        let page = self.app.page_rect().expect("page");
        pos2(page.min.x + dx, page.min.y + dy)
    }
}

fn button(pressed: bool, pos: Pos2) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
}

fn line(from: Pos2, to: Pos2, steps: usize) -> Vec<Pos2> {
    (0..=steps).map(|i| from.lerp(to, i as f32 / steps as f32)).collect()
}

/// Draws one long level stroke with the first pen and returns its two ends on the screen.
fn draw_a_long_stroke(desk: &mut Desk) -> (Pos2, Pos2) {
    desk.pick(ribbon::Act::InkPen(0));
    let (a, b) = (desk.paper(160.0, 300.0), desk.paper(460.0, 300.0));
    desk.drag(&line(a, b, 30));
    assert_eq!(desk.app.ink_stroke_count(), 1, "one stroke is on the page");
    (a, b)
}

// ---------------------------------------------------------------------------------------------
// 2A. Two erasers, and a size of the eraser's own.
// ---------------------------------------------------------------------------------------------

#[test]
fn the_object_eraser_takes_the_whole_stroke_it_crosses() {
    let mut desk = Desk::new("p2-object-eraser");
    let (a, b) = draw_a_long_stroke(&mut desk);
    assert_eq!(desk.app.session().ink_options().eraser_kind, EraserKind::Object, "the eraser takes whole strokes unless told otherwise");
    desk.pick(ribbon::Act::InkPointer(ribbon::Pointer::Erase));
    let mid = a.lerp(b, 0.5);
    desk.drag(&line(mid - vec2(0.0, 40.0), mid + vec2(0.0, 40.0), 10));
    assert_eq!(desk.app.ink_stroke_count(), 0, "crossing a stroke rubs all of it out");
}

#[test]
fn the_pixel_eraser_cuts_the_stroke_and_one_undo_puts_it_back() {
    let mut desk = Desk::new("p2-pixel-eraser");
    let (a, b) = draw_a_long_stroke(&mut desk);
    let whole = desk.app.ink_bounds().expect("ink");
    desk.run("ink.eraser_mode", &[("mode", "pixel")]);
    desk.pick(ribbon::Act::InkPointer(ribbon::Pointer::Erase));

    // One drag that crosses the stroke twice, a third and two thirds of the way along.
    let (first, second) = (a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0));
    let mut path = line(first - vec2(0.0, 40.0), first + vec2(0.0, 40.0), 10);
    path.extend(line(first + vec2(0.0, 40.0), second + vec2(0.0, 40.0), 10));
    path.extend(line(second + vec2(0.0, 40.0), second - vec2(0.0, 40.0), 10));
    desk.drag(&path);

    assert_eq!(desk.app.ink_stroke_count(), 3, "two cuts leave three pieces");
    let pieces = desk.app.ink_bounds().expect("ink is left");
    assert!((pieces[0] - whole[0]).abs() < 0.5 && (pieces[2] - whole[2]).abs() < 0.5, "both ends of the stroke are still there");
    let points: usize = desk.app.ink_dab_counts().iter().sum();
    assert!(points < 120, "the pieces are not swollen with points: {points}");

    desk.pick(ribbon::Act::Undo);
    assert_eq!(desk.app.ink_stroke_count(), 1, "the whole drag is one step to undo");
    assert_eq!(desk.app.ink_bounds().expect("ink"), whole);
}

#[test]
fn the_eraser_has_a_size_of_its_own() {
    let mut desk = Desk::new("p2-eraser-size");
    let (a, b) = draw_a_long_stroke(&mut desk);
    desk.pick(ribbon::Act::InkPointer(ribbon::Pointer::Erase));
    let beside = a.lerp(b, 0.5) + vec2(0.0, 14.0);

    // The small eraser, 6 points across, misses a stroke 14 points away.
    desk.drag(&[beside, beside + vec2(1.0, 0.0)]);
    assert_eq!(desk.app.ink_stroke_count(), 1, "the small eraser does not reach it");

    // The largest, 40 points across, reaches it from the same place. The pen is still a thin pen.
    desk.run("ink.eraser_width", &[("width", "xl")]);
    desk.drag(&[beside, beside + vec2(1.0, 0.0)]);
    assert_eq!(desk.app.ink_stroke_count(), 0, "the large eraser does");
}

// ---------------------------------------------------------------------------------------------
// 2B. Hold the pen still for two seconds to make the stroke straight.
// ---------------------------------------------------------------------------------------------

/// A hand-drawn line: nearly straight, with a wobble.
fn wobbly(from: Pos2, to: Pos2) -> Vec<Pos2> {
    line(from, to, 24).into_iter().enumerate().map(|(i, p)| p + vec2(0.0, if i % 2 == 0 { 1.5 } else { -1.5 })).collect()
}

fn draw_and_hold(desk: &mut Desk, path: &[Pos2], hold_secs: f64) {
    desk.press(path[0]);
    for at in &path[1..] {
        desk.move_to(0.02, *at);
    }
    let mut held = 0.0;
    while held < hold_secs {
        desk.step(0.1, Vec::new());
        held += 0.1;
    }
}

#[test]
fn a_pen_held_still_for_two_seconds_straightens_a_nearly_straight_stroke() {
    let mut desk = Desk::new("p2-straighten");
    desk.pick(ribbon::Act::InkPen(0));
    let (a, b) = (desk.paper(160.0, 300.0), desk.paper(360.0, 300.0));
    let path = wobbly(a, b);
    let end = path[path.len() - 1];

    draw_and_hold(&mut desk, &path, 1.5);
    desk.lift(end);
    assert!(desk.app.ink_dab_counts()[0] > 10, "lifted before two seconds, it stays as it was drawn");

    desk.pick(ribbon::Act::Undo);
    assert_eq!(desk.app.ink_stroke_count(), 0);
    draw_and_hold(&mut desk, &path, 2.2);
    // Still down: the end of the straight line follows the pen until it lifts.
    let new_end = end + vec2(30.0, 60.0);
    desk.move_to(0.05, end + vec2(15.0, 30.0));
    desk.move_to(0.05, new_end);
    desk.lift(new_end);
    assert_eq!(desk.app.ink_stroke_count(), 1);
    assert_eq!(desk.app.ink_dab_counts(), vec![2], "a straight stroke is its two ends");
    let [x0, y0, x1, y1] = desk.app.ink_bounds().expect("ink");
    assert!((y1 - y0) > (x1 - x0) * 0.2, "the line runs to where the pen was lifted, not where it paused: {x0},{y0} to {x1},{y1}");
}

#[test]
fn a_pause_after_a_loop_does_not_flatten_it_and_the_setting_turns_straightening_off() {
    let mut desk = Desk::new("p2-straighten-guard");
    desk.pick(ribbon::Act::InkPen(0));
    // Most of a circle, 80 points across, ending 30 points from where it began.
    let centre = desk.paper(300.0, 320.0);
    let around: Vec<Pos2> = (0..=40)
        .map(|i| {
            let angle = 0.4 + (i as f32 / 40.0) * (std::f32::consts::TAU - 0.8);
            centre + vec2(angle.cos(), angle.sin()) * 40.0
        })
        .collect();
    let end = around[around.len() - 1];
    draw_and_hold(&mut desk, &around, 2.4);
    desk.lift(end);
    assert!(desk.app.ink_dab_counts()[0] > 10, "a letter or a loop is not a line, however long the pen rests");

    desk.pick(ribbon::Act::Undo);
    desk.run("ink.hold_to_straighten", &[("enabled", "false")]);
    let (a, b) = (desk.paper(160.0, 500.0), desk.paper(360.0, 500.0));
    let path = wobbly(a, b);
    draw_and_hold(&mut desk, &path, 2.4);
    desk.lift(path[path.len() - 1]);
    assert!(desk.app.ink_dab_counts()[0] > 10, "with the setting off the stroke is left as drawn");
}

// ---------------------------------------------------------------------------------------------
// The eraser's choices on the Studio tool pill.
// ---------------------------------------------------------------------------------------------

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
    h.input_mut().events.extend([egui::Event::PointerMoved(at), button(true, at)]);
    h.step();
    h.input_mut().events.extend([button(false, at)]);
    h.step();
    h.step();
}

#[test]
fn tapping_the_eraser_again_offers_object_or_pixel_and_a_size() {
    let tmp = TempDir::new("p2-eraser-popover");
    let (session, page) = session_with_page(&tmp);
    let mut app = NocturneApp::new(session);
    app.open_path(&page.to_string_lossy());
    let mut h = Harness::builder().with_size(SCREEN).with_accessibility_check(false).build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
    h.ctx.set_os(OperatingSystem::IOS);
    for _ in 0..4 {
        h.step();
    }
    let eraser = labelled(&h, |l| l.starts_with("Switch between the eraser")).expect("the eraser on the tool pill");
    click(&mut h, eraser.center());
    assert_eq!(labelled(&h, |l| l == "Pixel eraser"), None, "the first tap only picks the eraser up");
    click(&mut h, eraser.center());
    let pixel = labelled(&h, |l| l == "Pixel eraser").expect("a second tap offers the pixel eraser");
    click(&mut h, pixel.center());
    assert_eq!(h.state().session().ink_options().eraser_kind, EraserKind::Pixel);
    let large = labelled(&h, |l| l == "Eraser size L").expect("and its sizes");
    click(&mut h, large.center());
    assert_eq!(h.state().session().ink_options().eraser_width, 24.0);
    let object = labelled(&h, |l| l == "Object eraser").expect("and the object eraser");
    click(&mut h, object.center());
    assert_eq!(h.state().session().ink_options().eraser_kind, EraserKind::Object);
}

/// Small handwriting is short and low, and still not a line: a zigzag sixteen points wide and
/// twelve tall must come through a long rest as it was written.
#[test]
fn a_rest_after_small_handwriting_leaves_it_as_written() {
    let mut desk = Desk::new("p2-straighten-small");
    desk.pick(ribbon::Act::InkPen(0));
    let start = desk.paper(200.0, 300.0);
    let zigzag: Vec<Pos2> = (0..=8).map(|i| start + vec2(2.0 * i as f32, if i % 2 == 0 { 0.0 } else { 12.0 })).collect();
    draw_and_hold(&mut desk, &zigzag, 2.4);
    desk.lift(zigzag[zigzag.len() - 1]);
    assert!(desk.app.ink_dab_counts()[0] > 4, "a zigzag is not flattened: {:?}", desk.app.ink_dab_counts());
}

/// A real pen is never quite still. While it rests it goes on reporting small shifts and
/// changes of pressure; they are not part of the line, and must not stop it being made straight
/// nor decide how heavy it is.
#[test]
fn a_pen_that_trembles_while_it_rests_still_straightens_its_line() {
    let mut desk = Desk::new("p2-straighten-tremble");
    desk.pick(ribbon::Act::InkPen(0));
    let (a, b) = (desk.paper(160.0, 300.0), desk.paper(360.0, 300.0));
    let pen = |phase, pos, force| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(9), phase, pos, force: Some(force) };
    desk.step(0.02, vec![pen(egui::TouchPhase::Start, a, 0.8), egui::Event::PointerMoved(a), button(true, a)]);
    for at in line(a, b, 20).into_iter().skip(1) {
        desk.step(0.02, vec![pen(egui::TouchPhase::Move, at, 0.8), egui::Event::PointerMoved(at)]);
    }
    // 2.4 seconds of rest: the tip wanders 0.4 of a point and the pressure flutters low.
    for i in 0..24 {
        let at = b + vec2(if i % 2 == 0 { 0.4 } else { -0.4 }, if i % 3 == 0 { 0.4 } else { 0.0 });
        let force = if i % 2 == 0 { 0.2 } else { 0.3 };
        desk.step(0.1, vec![pen(egui::TouchPhase::Move, at, force), egui::Event::PointerMoved(at)]);
    }
    desk.step(0.02, vec![pen(egui::TouchPhase::End, b, 0.0), button(false, b), egui::Event::PointerGone]);
    desk.idle(3);
    assert_eq!(desk.app.ink_dab_counts(), vec![2], "the line is made straight for all the trembling");
    assert!(
        (desk.app.ink_pressures()[0] - 0.8).abs() < 0.05,
        "at the pressure it was drawn with, not the resting one: {:?}",
        desk.app.ink_pressures()
    );
}
