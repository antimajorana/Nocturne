//! iPad Studio UX v2, phase 3: a tool bar that sits on any edge and carries the tools chosen
//! for it, a Tool Settings screen, and a page with no bar across its top.
//!
//! Each test drives the real `NocturneApp`. Nothing is mocked.

use egui::os::OperatingSystem;
use egui::{Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use nocturne_engine::{Session, ToolbarDock};
use nocturne_store::Settings;
use nocturne_store::testing::TempDir;
use nocturne_ui_egui::NocturneApp;
use nocturne_ui_egui::tablet_chrome::nearest_dock;

const SCREEN: egui::Vec2 = vec2(1180.0, 820.0);

fn studio(name: &str) -> (TempDir, Harness<'static, NocturneApp>) {
    let tmp = TempDir::new(name);
    let settings = Settings::at(tmp.path().join("cfg/settings.toml"));
    settings.set("privacy_welcome_seen", "true").expect("welcome seen");
    settings.set_default_format(nocturne_model::PageFormat::Cmgfm).expect("cmgfm default");
    let mut session = Session::new(&tmp.path().join("notebooks"), settings).expect("session");
    session.run("notebook.create", &[("name".into(), "Field Notes".into())].into_iter().collect()).expect("create notebook");
    let page = session.page_to_show("field-notes").expect("first page");
    let mut app = NocturneApp::new(session);
    app.open_path(&page.to_string_lossy());
    let mut h = Harness::builder().with_size(SCREEN).with_accessibility_check(false).build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
    h.ctx.set_os(OperatingSystem::IOS);
    steps(&mut h, 4);
    assert!(h.state().is_tablet_studio(&h.ctx));
    (tmp, h)
}

fn steps(h: &mut Harness<'_, NocturneApp>, count: usize) {
    for _ in 0..count {
        h.step();
    }
}

fn run(h: &mut Harness<'_, NocturneApp>, command: &str, params: &[(&str, &str)]) {
    let params = params.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
    h.state_mut().session_mut().run(command, &params).unwrap_or_else(|e| panic!("{command}: {e}"));
    steps(h, 3);
}

/// The place of the first thing on the screen whose name matches. A name's line breaks count
/// as spaces.
fn labelled(h: &Harness<'_, NocturneApp>, matches: impl Fn(&str) -> bool) -> Option<Rect> {
    let ppp = h.ctx.pixels_per_point();
    h.query_all(egui_kittest::kittest::by()).find_map(|node| {
        let label = node.accesskit_node().label()?.replace('\n', " ");
        if !matches(label.trim()) {
            return None;
        }
        let b = node.accesskit_node().bounding_box()?;
        let rect = Rect::from_min_max(pos2(b.x0 as f32 / ppp, b.y0 as f32 / ppp), pos2(b.x1 as f32 / ppp, b.y1 as f32 / ppp));
        rect.is_positive().then_some(rect)
    })
}

fn named(h: &Harness<'_, NocturneApp>, name: &str) -> Option<Rect> {
    labelled(h, |l| l.eq_ignore_ascii_case(name))
}

fn button(pressed: bool, pos: Pos2) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
}

fn click(h: &mut Harness<'_, NocturneApp>, at: Pos2) {
    h.input_mut().events.extend([egui::Event::PointerMoved(at), button(true, at)]);
    h.step();
    h.input_mut().events.extend([button(false, at)]);
    steps(h, 3);
}

fn area(h: &Harness<'_, NocturneApp>, id: &str) -> Option<Rect> {
    h.ctx.memory(|m| m.area_rect(egui::Id::unique(id)))
}

fn tool_bar(h: &Harness<'_, NocturneApp>) -> Rect {
    area(h, "studio-apple-pencil-pill").expect("the tool bar is on the page")
}

// ---------------------------------------------------------------------------------------------
// 3A. The tool bar on any edge.
// ---------------------------------------------------------------------------------------------

#[test]
fn the_tool_bar_sits_on_the_edge_it_is_given_and_runs_along_it() {
    let (_tmp, mut h) = studio("p3-dock-edges");
    let view = h.state().page_view_rect().expect("the page view");
    for dock in ToolbarDock::ALL {
        run(&mut h, "ui.toolbar_dock", &[("edge", dock.id())]);
        let bar = tool_bar(&h);
        assert!(Rect::from_min_size(Pos2::ZERO, SCREEN).contains_rect(bar), "{dock:?}: the bar is on the screen: {bar:?}");
        let gap = match dock {
            ToolbarDock::Bottom => view.max.y - bar.max.y,
            ToolbarDock::Top => bar.min.y - view.min.y,
            ToolbarDock::Left => bar.min.x - view.min.x,
            ToolbarDock::Right => view.max.x - bar.max.x,
        };
        assert!((0.0..=80.0).contains(&gap), "{dock:?}: the bar is against its edge, {gap} points from it ({bar:?} in {view:?})");
        if dock.is_vertical() {
            assert!(bar.height() > bar.width() * 2.0, "{dock:?}: the bar runs down the page: {bar:?}");
        } else {
            assert!(bar.width() > bar.height() * 2.0, "{dock:?}: the bar runs across the page: {bar:?}");
        }
        assert!(named(&h, "Undo").is_some_and(|undo| bar.expand(1.0).contains_rect(undo)), "{dock:?}: its buttons are on it");
    }
}

#[test]
fn the_minimap_is_top_right_and_keeps_clear_of_the_tool_bar() {
    let (_tmp, mut h) = studio("p3-minimap");
    let view = h.state().page_view_rect().expect("the page view");
    for dock in ToolbarDock::ALL {
        run(&mut h, "ui.toolbar_dock", &[("edge", dock.id())]);
        let (map, bar) = (area(&h, "infinite-canvas-minimap").expect("the minimap"), tool_bar(&h));
        assert!(map.center().x > view.center().x && map.center().y < view.center().y, "{dock:?}: the minimap is top right: {map:?} in {view:?}");
        assert!(!map.intersects(bar), "{dock:?}: the minimap {map:?} does not lie under the tool bar {bar:?}");
    }
}

#[test]
fn a_bar_let_go_near_an_edge_lands_on_that_edge() {
    let view = Rect::from_min_max(pos2(100.0, 50.0), pos2(1100.0, 800.0));
    assert_eq!(nearest_dock(view, pos2(600.0, 780.0)), ToolbarDock::Bottom);
    assert_eq!(nearest_dock(view, pos2(600.0, 70.0)), ToolbarDock::Top);
    assert_eq!(nearest_dock(view, pos2(130.0, 400.0)), ToolbarDock::Left);
    assert_eq!(nearest_dock(view, pos2(1080.0, 400.0)), ToolbarDock::Right);
    assert_eq!(nearest_dock(view, pos2(-500.0, 400.0)), ToolbarDock::Left, "past the edge is still that edge");

    let (_tmp, mut h) = studio("p3-dock-drag");
    let view = h.state().page_view_rect().expect("the page view");
    let grip = named(&h, "Move the tool bar").expect("the bar has a grip").center();
    let target = pos2(view.min.x + 30.0, view.center().y);
    h.input_mut().events.extend([egui::Event::PointerMoved(grip), button(true, grip)]);
    h.step();
    for i in 1..=8 {
        h.input_mut().events.push(egui::Event::PointerMoved(grip.lerp(target, i as f32 / 8.0)));
        h.step();
    }
    assert_eq!(h.state().session().studio_options().toolbar_dock, ToolbarDock::Bottom, "nothing moves until the bar is let go");
    h.input_mut().events.push(button(false, target));
    steps(&mut h, 3);
    assert_eq!(h.state().session().studio_options().toolbar_dock, ToolbarDock::Left, "dragged to the left edge, it stays there");
    assert!(tool_bar(&h).height() > tool_bar(&h).width() * 2.0);
}

#[test]
fn the_bar_carries_the_tools_chosen_for_it_in_the_order_chosen() {
    let (_tmp, mut h) = studio("p3-roster");
    let x_of = |h: &Harness<'_, NocturneApp>, name: &str| named(h, name).map(|r| r.center().x);
    let eraser_x = |h: &Harness<'_, NocturneApp>| labelled(h, |l| l.starts_with("Switch between the eraser")).map(|r| r.center().x);
    let (lasso, shapes) = (x_of(&h, "Lasso select").expect("lasso"), x_of(&h, "Shapes").expect("shapes"));
    assert!(lasso < shapes, "the lasso comes before the shapes on a fresh bar");

    run(&mut h, "ink.configure_tools", &[("tools", "shapes,-lasso,-history")]);
    assert_eq!(x_of(&h, "Lasso select"), None, "a hidden tool is off the bar");
    assert_eq!(x_of(&h, "Undo"), None);
    let (shapes, eraser) = (x_of(&h, "Shapes").expect("shapes"), eraser_x(&h).expect("eraser"));
    assert!(shapes < eraser, "the shapes were put first");
    assert!(named(&h, "Tool settings").is_some(), "the way to Tool Settings is always there");
    let _ = click;
}

// ---------------------------------------------------------------------------------------------
// 3B. Tool Settings.
// ---------------------------------------------------------------------------------------------

fn open_tool_settings(h: &mut Harness<'_, NocturneApp>) {
    let dots = named(h, "Tool settings").expect("the three dots on the tool bar").center();
    click(h, dots);
    assert!(area(h, "studio-tool-settings").is_some(), "the three dots open Tool Settings");
}

#[test]
fn tool_settings_holds_the_switches_the_eraser_and_the_bars_edge() {
    let (_tmp, mut h) = studio("p3-settings-switches");
    assert_eq!(named(&h, "Draw with finger"), None, "the switch is not out on the page");
    open_tool_settings(&mut h);

    let finger = named(&h, "Draw with finger").expect("Draw with finger").center();
    click(&mut h, finger);
    assert!(h.state().is_finger_drawing());

    assert!(h.state().session().ink_options().hold_to_straighten);
    let straighten = named(&h, "Hold 2 seconds to straighten a line").expect("hold to straighten").center();
    click(&mut h, straighten);
    assert!(!h.state().session().ink_options().hold_to_straighten);

    let pixel = named(&h, "Pixel eraser").expect("the eraser's kinds").center();
    click(&mut h, pixel);
    assert_eq!(h.state().session().ink_options().eraser_kind, nocturne_engine::EraserKind::Pixel);

    let left = named(&h, "Left").expect("the bar's edges").center();
    click(&mut h, left);
    assert_eq!(h.state().session().studio_options().toolbar_dock, ToolbarDock::Left);
    assert!(area(&h, "studio-tool-settings").is_some(), "the screen stays open while its choices are made");

    let close = named(&h, "Close").expect("a way out").center();
    click(&mut h, close);
    steps(&mut h, 2);
    assert_eq!(named(&h, "Draw with finger"), None, "Close puts the screen away");
}

#[test]
fn tool_settings_hides_and_reorders_the_tools_on_the_bar() {
    let (_tmp, mut h) = studio("p3-settings-roster");
    open_tool_settings(&mut h);
    let lasso = reveal(&mut h, "Lasso");
    click(&mut h, lasso);
    assert_eq!(h.state().session().studio_options().roster.write(), "history,write,pens,eraser,-lasso,select,shapes");
    assert_eq!(named(&h, "Lasso select"), None, "and it is off the bar");

    let earlier = reveal(&mut h, "Move Shapes earlier");
    click(&mut h, earlier);
    assert_eq!(h.state().session().studio_options().roster.write(), "history,write,pens,eraser,-lasso,shapes,select");
}

#[test]
fn each_kind_of_pen_keeps_its_own_colours_up_to_twelve() {
    let (_tmp, mut h) = studio("p3-settings-pens");
    open_tool_settings(&mut h);
    let pens = |h: &Harness<'_, NocturneApp>, tool: nocturne_ink::Tool| h.state().pen_rack().iter().filter(|n| n.tool == tool).count();
    let before = pens(&h, nocturne_ink::Tool::Pencil);
    let total = h.state().pen_rack().len();
    let add = reveal(&mut h, "Add a colour to Pencil");
    click(&mut h, add);
    assert_eq!(pens(&h, nocturne_ink::Tool::Pencil), before + 1, "the pencil has one more colour");
    assert_eq!(h.state().pen_rack().len(), total + 1, "and no other pen lost one");

    // The new colour is the pen in hand: a colour picked now is its colour.
    let slot = h.state().session().ink_tool_state().rack_index;
    let swatch = reveal(&mut h, "Pen colour");
    click(&mut h, swatch);
    assert_eq!(h.state().pen_rack()[slot].tool, nocturne_ink::Tool::Pencil);

    let remove = reveal(&mut h, "Remove a colour from Pencil");
    click(&mut h, remove);
    assert_eq!(pens(&h, nocturne_ink::Tool::Pencil), before);

    // Twelve is the most: the button to add is switched off there.
    for _ in 0..nocturne_ui_egui::tool_settings::MAX_COLOURS_PER_TOOL + 2 {
        {
            let add = reveal(&mut h, "Add a colour to Highlighter");
            click(&mut h, add);
        }
    }
    assert!(pens(&h, nocturne_ink::Tool::Highlighter) <= nocturne_ui_egui::tool_settings::MAX_COLOURS_PER_TOOL);
    assert!(h.state().pen_rack().len() <= nocturne_ink::rack::MAX_NIBS, "the rack as a whole keeps its limit");
}

/// Scrolls the Tool Settings list until the control with this name is inside the screen, as a
/// finger would, and returns where it is.
fn reveal(h: &mut Harness<'_, NocturneApp>, name: &str) -> Pos2 {
    for _ in 0..12 {
        let screen = area(h, "studio-tool-settings").expect("Tool Settings is open");
        let at = named(h, name).unwrap_or_else(|| panic!("{name} is in Tool Settings"));
        if screen.shrink(6.0).contains_rect(at) {
            return at.center();
        }
        let over = screen.center();
        let down = if at.center().y > screen.center().y { -90.0 } else { 90.0 };
        h.input_mut().events.extend([
            egui::Event::PointerMoved(over),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, down),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        steps(h, 3);
    }
    panic!("{name} could not be scrolled into Tool Settings");
}

// ---------------------------------------------------------------------------------------------
// 3C. A page with no bar across its top.
// ---------------------------------------------------------------------------------------------

#[test]
fn an_open_page_reaches_the_top_of_the_screen_under_a_small_floating_header() {
    let (_tmp, mut h) = studio("p3-header");
    steps(&mut h, 3);
    let view = h.state().page_view_rect().expect("the page view");
    assert!(view.min.y < 24.0, "no bar across the window: the page view starts at the top, at {}", view.min.y);
    let header = area(&h, "studio-header-pill").expect("the floating header");
    assert!(header.min.x - view.min.x < 30.0 && header.min.y - view.min.y < 30.0, "it is at the top left of the page: {header:?} in {view:?}");
    assert!(header.width() < view.width() * 0.6, "it is a pill, not a bar: {header:?}");
    assert!(labelled(&h, |l| l.eq_ignore_ascii_case("Add page")).is_some(), "a new page is one tap away");
    assert!(any_in(&h, "Add page", header), "and it is on the header");

    // Everything else is behind the three dots.
    let more = named(&h, "More...").expect("more").center();
    click(&mut h, more);
    for entry in ["Back", "Forward", "Search all pages", "File", "Fit all content", "Switch to classic ribbon"] {
        assert!(labelled(&h, |l| l.eq_ignore_ascii_case(entry)).is_some(), "{entry} is in the header's menu");
    }
    let search = named(&h, "Search all pages").expect("search").center();
    click(&mut h, search);
    steps(&mut h, 2);
    assert!(h.state().focused_text_field(&h.ctx).is_some(), "choosing search opens the search box in the header, with the keyboard");

    // A tool bar docked along the top sits under the header, not across it.
    run(&mut h, "ui.toolbar_dock", &[("edge", "top")]);
    let (header, bar) = (area(&h, "studio-header-pill").expect("header"), tool_bar(&h));
    assert!(!header.intersects(bar), "the header {header:?} and the tool bar {bar:?} do not overlap");
}

/// Whether something with this name lies inside `within`. The same name can be on the screen
/// more than once (the drawer has an Add page of its own).
fn any_in(h: &Harness<'_, NocturneApp>, name: &str, within: Rect) -> bool {
    let ppp = h.ctx.pixels_per_point();
    h.query_all(egui_kittest::kittest::by()).any(|node| {
        let named = node.accesskit_node().label().is_some_and(|l| l.replace('\n', " ").trim().eq_ignore_ascii_case(name));
        let inside = node.accesskit_node().bounding_box().is_some_and(|b| {
            within
                .expand(1.0)
                .contains_rect(Rect::from_min_max(pos2(b.x0 as f32 / ppp, b.y0 as f32 / ppp), pos2(b.x1 as f32 / ppp, b.y1 as f32 / ppp)))
        });
        named && inside
    })
}

// ---------------------------------------------------------------------------------------------
// Found in review.
// ---------------------------------------------------------------------------------------------

/// A dialog over the page must not bring the bar across the window back: the page would jump
/// down by the bar's height and up again when the dialog closed.
#[test]
fn a_dialog_over_the_page_does_not_move_the_page() {
    let (_tmp, mut h) = studio("p3-dialog-no-jump");
    steps(&mut h, 3);
    let before = h.state().page_view_rect().expect("the page view");
    h.state_mut().open_new_notebook_dialog();
    steps(&mut h, 4);
    let during = h.state().page_view_rect().expect("the page view under the dialog");
    assert_eq!(during.min.y, before.min.y, "the page stays where it was while a dialog is open");
}

/// A colour added in Tool Settings is a pen on the bar, however many there are.
#[test]
fn every_colour_added_in_tool_settings_is_on_the_bar() {
    let (_tmp, mut h) = studio("p3-all-pens-on-bar");
    let on_bar = |h: &Harness<'_, NocturneApp>| {
        let bar = tool_bar(h);
        let ppp = h.ctx.pixels_per_point();
        h.query_all(egui_kittest::kittest::by())
            .filter(|node| {
                let is_pen =
                    node.accesskit_node().label().is_some_and(|l| ["Pen", "Pencil", "Fountain Pen", "Brush Pen", "Highlighter"].contains(&l.trim()));
                let inside = node
                    .accesskit_node()
                    .bounding_box()
                    .is_some_and(|b| bar.expand(2.0).contains(pos2((b.x0 + b.x1) as f32 / 2.0 / ppp, (b.y0 + b.y1) as f32 / 2.0 / ppp)));
                is_pen && inside
            })
            .count()
    };
    let before = h.state().pen_rack().len();
    assert_eq!(on_bar(&h), before, "every pen of a fresh rack is on the bar");
    open_tool_settings(&mut h);
    for _ in 0..3 {
        let add = reveal(&mut h, "Add a colour to Pen");
        click(&mut h, add);
    }
    let close = named(&h, "Close").expect("a way out").center();
    click(&mut h, close);
    steps(&mut h, 3);
    assert_eq!(h.state().pen_rack().len(), before + 3);
    assert_eq!(on_bar(&h), before + 3, "the three new colours are pens on the bar");
}

/// On the iPad a finger's lift arrives as the touch ending, the button's release and the pointer
/// leaving, all in one frame. The bar still lands where the finger was.
#[test]
fn a_bar_dragged_by_a_finger_lands_where_the_finger_lifted() {
    let (_tmp, mut h) = studio("p3-dock-drag-touch");
    let view = h.state().page_view_rect().expect("the page view");
    let grip = named(&h, "Move the tool bar").expect("the bar has a grip").center();
    let target = pos2(view.center().x, view.min.y + 90.0);
    let touch = |phase, pos| egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(5), phase, pos, force: None };
    h.input_mut().events.extend([touch(egui::TouchPhase::Start, grip), egui::Event::PointerMoved(grip), button(true, grip)]);
    h.step();
    for i in 1..=8 {
        let at = grip.lerp(target, i as f32 / 8.0);
        h.input_mut().events.extend([touch(egui::TouchPhase::Move, at), egui::Event::PointerMoved(at)]);
        h.step();
    }
    h.input_mut().events.extend([touch(egui::TouchPhase::End, target), button(false, target), egui::Event::PointerGone]);
    steps(&mut h, 3);
    assert_eq!(h.state().session().studio_options().toolbar_dock, ToolbarDock::Top);
    // And a bar down the left edge keeps clear of the header.
    run(&mut h, "ui.toolbar_dock", &[("edge", "left")]);
    let (header, bar) = (area(&h, "studio-header-pill").expect("header"), tool_bar(&h));
    assert!(!header.intersects(bar), "the header {header:?} and a left bar {bar:?} do not overlap");
}
