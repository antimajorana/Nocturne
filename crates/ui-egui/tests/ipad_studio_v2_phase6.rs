//! iPad Studio UX v2, phase 6 (Deliverable 6): non-overlapping floating header clearance above
//! the page title, upright colour-banded dock pens and highlighters with inward active elevation
//! across all four edges, configurable `Picture` and `Math OCR` dock tools, and `< 600`-line
//! `tablet_chrome` module split.
//!
//! Every test drives the real `NocturneApp` and offscreen wgpu renderer. Nothing is mocked.

#[allow(dead_code)]
mod support;

use egui::accesskit::Role;
use egui::os::OperatingSystem;
use egui::{Color32, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use nocturne_engine::ocr::OcrConfig;
use nocturne_engine::{Session, ToolbarDock};
use nocturne_store::Settings;
use nocturne_store::testing::TempDir;
use nocturne_ui_egui::tablet_chrome::{
    ACTIVE_NIB_RAISE, NIB_COLOURS, NibColors, UiLayoutMode, canvas_top_clearance, nearest_dock, nib_colors_for_theme, nib_visual_rect,
};
use nocturne_ui_egui::theme::{Accent, Mode, Theme, Tokens, contrast};
use nocturne_ui_egui::{NocturneApp, ribbon};
use std::path::PathBuf;

const SCREEN: egui::Vec2 = vec2(1180.0, 820.0);
const PEN_LABELS: [&str; 5] = ["Pen", "Pencil", "Fountain Pen", "Brush Pen", "Highlighter"];

fn studio_with_title(name: &str, title: &str, gpu: bool) -> (TempDir, Harness<'static, NocturneApp>, PathBuf) {
    let tmp = TempDir::new(name);
    let settings = Settings::at(tmp.path().join("cfg/settings.toml"));
    settings.set("privacy_welcome_seen", "true").expect("welcome seen");
    settings.set_default_format(nocturne_model::PageFormat::Cmgfm).expect("cmgfm default");
    let mut session = Session::new(&tmp.path().join("notebooks"), settings).expect("session");
    session.run("notebook.create", &[("name".into(), "Mechanics Seminar".into())].into_iter().collect()).expect("create notebook");
    let page = session.page_to_show("mechanics-seminar").expect("first page");
    let page_str = page.to_string_lossy().to_string();
    session
        .run("node.title", &[("path".into(), page_str.clone()), ("title".into(), title.into())].into_iter().collect())
        .expect("set synthetic page title");
    let mut app = NocturneApp::new(session);
    app.open_path(&page_str);
    let builder = Harness::builder().with_size(SCREEN).with_accessibility_check(false);
    let mut h = if gpu {
        builder.renderer(support::gpu::renderer()).build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app)
    } else {
        builder.build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app)
    };
    h.ctx.set_os(OperatingSystem::IOS);
    steps(&mut h, 4);
    assert!(h.state().is_tablet_studio(&h.ctx), "iPad viewport runs Tablet Studio");
    (tmp, h, page)
}

fn steps(h: &mut Harness<'_, NocturneApp>, count: usize) {
    for _ in 0..count {
        let queued = h.state_mut().take_events();
        h.input_mut().events.extend(queued);
        h.step();
    }
}

fn run(h: &mut Harness<'_, NocturneApp>, command: &str, params: &[(&str, &str)]) {
    let params = params.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
    h.state_mut().session_mut().run(command, &params).unwrap_or_else(|e| panic!("{command}: {e}"));
    steps(h, 3);
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
    h.ctx.memory(|m| m.areas().visible_layer_ids().iter().any(|l| l.id == egui::Id::unique(id)).then(|| m.area_rect(egui::Id::unique(id))).flatten())
}

fn header_pill(h: &Harness<'_, NocturneApp>) -> Rect {
    area(h, "studio-header-pill").expect("floating header pill is shown")
}

fn tool_bar(h: &Harness<'_, NocturneApp>) -> Rect {
    area(h, "studio-apple-pencil-pill").expect("floating dock tool bar is shown")
}

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

fn in_rect(h: &Harness<'_, NocturneApp>, name: &str, within: Rect) -> Option<Rect> {
    let ppp = h.ctx.pixels_per_point();
    let padded = within.expand(2.0);
    h.query_all(egui_kittest::kittest::by()).find_map(|node| {
        let label = node.accesskit_node().label()?.replace('\n', " ");
        if !label.trim().eq_ignore_ascii_case(name) {
            return None;
        }
        let b = node.accesskit_node().bounding_box()?;
        let rect = Rect::from_min_max(pos2(b.x0 as f32 / ppp, b.y0 as f32 / ppp), pos2(b.x1 as f32 / ppp, b.y1 as f32 / ppp));
        (rect.is_positive() && padded.contains(rect.center())).then_some(rect)
    })
}

fn page_title_rect(h: &Harness<'_, NocturneApp>, expected: &str) -> Rect {
    let ppp = h.ctx.pixels_per_point();
    h.query_all_by_role(Role::MultilineTextInput)
        .chain(h.query_all_by_role(Role::TextInput))
        .find_map(|node| {
            let matches_val = node.value().is_some_and(|v| v.trim() == expected);
            let matches_lbl = node.accesskit_node().label().is_some_and(|l| l.trim() == expected);
            if !matches_val && !matches_lbl {
                return None;
            }
            let b = node.accesskit_node().bounding_box()?;
            let rect = Rect::from_min_max(pos2(b.x0 as f32 / ppp, b.y0 as f32 / ppp), pos2(b.x1 as f32 / ppp, b.y1 as f32 / ppp));
            rect.is_positive().then_some(rect)
        })
        .unwrap_or_else(|| panic!("page title '{expected}' should be visible as a text input"))
}

fn dock_pen_rects(h: &Harness<'_, NocturneApp>, dock: ToolbarDock) -> Vec<Rect> {
    let bar = tool_bar(h);
    let ppp = h.ctx.pixels_per_point();
    let mut rects: Vec<Rect> = h
        .query_all(egui_kittest::kittest::by())
        .filter_map(|node| {
            let label = node.accesskit_node().label()?;
            if !PEN_LABELS.contains(&label.trim()) {
                return None;
            }
            let b = node.accesskit_node().bounding_box()?;
            let rect = Rect::from_min_max(pos2(b.x0 as f32 / ppp, b.y0 as f32 / ppp), pos2(b.x1 as f32 / ppp, b.y1 as f32 / ppp));
            (rect.is_positive() && bar.expand(2.0).contains(rect.center())).then_some(rect)
        })
        .collect();
    rects.sort_by(|a, b| if dock.is_vertical() { a.center().y.total_cmp(&b.center().y) } else { a.center().x.total_cmp(&b.center().x) });
    rects
}

fn open_tool_settings(h: &mut Harness<'_, NocturneApp>) {
    let dots = in_rect(h, "Tool settings", tool_bar(h)).or_else(|| named(h, "Tool settings")).expect("Tool settings button").center();
    click(h, dots);
    assert!(area(h, "studio-tool-settings").is_some(), "Tool Settings modal opened");
}

fn reveal(h: &mut Harness<'_, NocturneApp>, name: &str) -> Pos2 {
    for _ in 0..18 {
        let screen = area(h, "studio-tool-settings").expect("Tool Settings is open");
        let at = named(h, name).unwrap_or_else(|| panic!("{name} should be present in Tool Settings"));
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
    panic!("{name} could not be scrolled into view inside Tool Settings");
}

fn manhattan(a: Color32, b: Color32) -> u32 {
    u32::from(a.r().abs_diff(b.r())) + u32::from(a.g().abs_diff(b.g())) + u32::from(a.b().abs_diff(b.b()))
}

fn topmost_non_panel_y(img: &image::RgbaImage, bar: Rect, slot: Rect, panel: Color32, ppp: f32) -> u32 {
    let x0 = ((slot.center().x - 7.0) * ppp).round().max(0.0) as u32;
    let x1 = ((slot.center().x + 7.0) * ppp).round().min((img.width().saturating_sub(1)) as f32) as u32;
    let y0 = ((bar.min.y + 3.0) * ppp).round().max(0.0) as u32;
    let y1 = ((bar.max.y - 3.0) * ppp).round().min((img.height().saturating_sub(1)) as f32) as u32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let p = img.get_pixel(x, y);
            let px = Color32::from_rgb(p[0], p[1], p[2]);
            if manhattan(px, panel) >= 45 {
                return y;
            }
        }
    }
    panic!("no non-panel pen pixel found in slot {slot:?} inside bar {bar:?}");
}

fn save_visual_png(img: &image::RgbaImage, filename: &str) {
    if let Some(dir) = std::env::var_os("NOCTURNE_UI_VISUAL_DIR") {
        let dir = PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        let _ = img.save(dir.join(filename));
    }
}

// ---------------------------------------------------------------------------------------------
// 1. Top-left floating header pill vs page title clearance across docks, search, and RTL.
// ---------------------------------------------------------------------------------------------

#[test]
fn floating_header_never_overlaps_page_title_across_docks_search_and_rtl() {
    let title = "Harmonic Oscillator Notes";
    let (_tmp, mut h, page) = studio_with_title("p6-title-clearance", title, false);

    for dock in ToolbarDock::ALL {
        run(&mut h, "ui.toolbar_dock", &[("edge", dock.id())]);
        let view = h.state().page_view_rect().expect("page view rect");
        assert!(view.min.y < 24.0, "{dock:?}: borderless canvas starts near top of window, got {}", view.min.y);
        let header_pill_rect = header_pill(&h);
        let pencil_pill_rect = tool_bar(&h);
        let title_rect = page_title_rect(&h, title);

        assert!(!header_pill_rect.intersects(title_rect), "{dock:?}: header pill {header_pill_rect:?} overlaps title {title_rect:?}");
        assert!(
            title_rect.min.y >= header_pill_rect.max.y + 4.0,
            "{dock:?}: title {title_rect:?} must clear header pill {header_pill_rect:?} by >= 4pt"
        );
        if dock == ToolbarDock::Top {
            assert!(!pencil_pill_rect.intersects(title_rect), "Top dock: pencil pill {pencil_pill_rect:?} overlaps title {title_rect:?}");
            assert!(
                title_rect.min.y >= pencil_pill_rect.max.y + 4.0,
                "Top dock: title {title_rect:?} must clear top-docked pencil pill {pencil_pill_rect:?}"
            );
        }
        let clearance = canvas_top_clearance(view, Some(header_pill_rect), Some(pencil_pill_rect), dock);
        assert!(
            clearance >= header_pill_rect.max.y - view.min.y + 4.0,
            "{dock:?}: canvas_top_clearance ({clearance}) must reserve room below header pill {header_pill_rect:?}"
        );
    }

    // Expand "Search all pages" inside the floating header pill and verify non-overlap.
    run(&mut h, "ui.toolbar_dock", &[("edge", "bottom")]);
    let more = in_rect(&h, "More...", header_pill(&h)).or_else(|| named(&h, "More...")).expect("More... on header pill").center();
    click(&mut h, more);
    let search_item = named(&h, "Search all pages").expect("Search all pages menu item").center();
    click(&mut h, search_item);
    steps(&mut h, 3);
    let wide_header = header_pill(&h);
    let title_while_searching = page_title_rect(&h, title);
    assert!(wide_header.width() > 300.0, "opening search widens the floating header pill: {wide_header:?}");
    assert!(!wide_header.intersects(title_while_searching), "expanded search header {wide_header:?} overlaps title {title_while_searching:?}");
    assert!(title_while_searching.min.y >= wide_header.max.y + 4.0);

    // Close search, switch to an Arabic RTL title, and verify non-overlap on the bidi title path.
    let more_again = in_rect(&h, "More...", header_pill(&h)).or_else(|| named(&h, "More...")).expect("More... on header pill").center();
    click(&mut h, more_again);
    let close_search = named(&h, "Search all pages").expect("Search all pages toggle").center();
    click(&mut h, close_search);

    let rtl_title = "\u{0645}\u{0639}\u{0627}\u{062f}\u{0644}\u{0627}\u{062a} \u{0627}\u{0644}\u{062d}\u{0631}\u{0643}\u{0629}";
    let page_str = page.to_string_lossy().to_string();
    run(&mut h, "node.title", &[("path", &page_str), ("title", rtl_title)]);
    run(&mut h, "page.direction", &[("path", &page_str), ("dir", "rtl")]);
    h.state_mut().open_path(&page_str);
    steps(&mut h, 4);
    assert!(!h.state().title_is_focused(), "RTL title starts unfocused so bidi run layout is active");
    let header_rtl = header_pill(&h);
    let rtl_rect = page_title_rect(&h, rtl_title);
    assert!(!header_rtl.intersects(rtl_rect), "header pill {header_rtl:?} overlaps Arabic RTL title {rtl_rect:?}");
    assert!(rtl_rect.min.y >= header_rtl.max.y + 4.0, "Arabic RTL title {rtl_rect:?} clears header pill {header_rtl:?}");
}

// ---------------------------------------------------------------------------------------------
// 2. Active pen inward elevation across all four dock edges, slot switching, and Eraser reset.
// ---------------------------------------------------------------------------------------------

#[test]
fn active_pen_is_elevated_inward_relative_to_inactive_pens_on_every_edge() {
    const { assert!(ACTIVE_NIB_RAISE >= 8.0) };
    let sample_slot = Rect::from_min_size(pos2(100.0, 200.0), vec2(28.0, 42.0));
    let raised = nib_visual_rect(sample_slot, ToolbarDock::Bottom, true);
    let recessed = nib_visual_rect(sample_slot, ToolbarDock::Bottom, false);
    assert!(raised.min.y <= recessed.min.y - 8.0, "nib_visual_rect shifts active pen inward: {raised:?} vs {recessed:?}");

    let (_tmp, mut h, _page) = studio_with_title("p6-active-elevation", "Harmonic Oscillator Notes", false);
    h.state_mut().ribbon_act_for_test(ribbon::Act::InkPen(0));
    steps(&mut h, 3);

    for dock in ToolbarDock::ALL {
        run(&mut h, "ui.toolbar_dock", &[("edge", dock.id())]);
        let rects = dock_pen_rects(&h, dock);
        assert!(rects.len() >= 3, "{dock:?}: expected pen rack on dock, found {}", rects.len());
        let active = rects[0];
        for (idx, other) in rects[1..].iter().enumerate() {
            match dock {
                ToolbarDock::Bottom => {
                    assert!(active.min.y <= other.min.y - 8.0, "Bottom slot {}: active {active:?} vs inactive {other:?}", idx + 1);
                }
                ToolbarDock::Top => {
                    assert!(active.max.y >= other.max.y + 8.0, "Top slot {}: active {active:?} vs inactive {other:?}", idx + 1);
                }
                ToolbarDock::Left => {
                    assert!(active.max.x >= other.max.x + 8.0, "Left slot {}: active {active:?} vs inactive {other:?}", idx + 1);
                }
                ToolbarDock::Right => {
                    assert!(active.min.x <= other.min.x - 8.0, "Right slot {}: active {active:?} vs inactive {other:?}", idx + 1);
                }
            }
        }
    }

    // Tap the second pen on the bottom dock: slot 1 rises >= 8pt while slot 0 recesses flush.
    run(&mut h, "ui.toolbar_dock", &[("edge", "bottom")]);
    let initial = dock_pen_rects(&h, ToolbarDock::Bottom);
    click(&mut h, initial[1].center());
    let after_second = dock_pen_rects(&h, ToolbarDock::Bottom);
    assert!(after_second[1].min.y <= after_second[0].min.y - 8.0, "second pen is now raised: {:?}", &after_second[..3]);
    assert!(after_second[1].min.y <= after_second[2].min.y - 8.0, "second pen rises above third: {:?}", &after_second[..3]);
    assert!((after_second[0].min.y - after_second[2].min.y).abs() < 0.5, "first pen recessed back in line with inactive pens");

    // Switch to Eraser: zero pens remain elevated.
    let eraser =
        labelled(&h, |l| l.starts_with("Switch between the eraser") || l.eq_ignore_ascii_case("Eraser")).expect("Eraser button on dock").center();
    click(&mut h, eraser);
    assert_eq!(h.state().inspect(&h.ctx)["ink"]["pointer"], "erase");
    let while_erasing = dock_pen_rects(&h, ToolbarDock::Bottom);
    for (idx, r) in while_erasing[1..].iter().enumerate() {
        assert!((while_erasing[0].min.y - r.min.y).abs() < 0.5, "with Eraser active, slot {} should sit flush: {:?}", idx + 1, while_erasing);
    }
}

// ---------------------------------------------------------------------------------------------
// 3. 14-theme WCAG contrast and offscreen Lavapipe GPU upright colour-banded pixel verification.
// ---------------------------------------------------------------------------------------------

#[test]
fn upright_nibs_pass_all_fourteen_themes_and_render_banded_upright_pixels() {
    let themes = Theme::all();
    assert_eq!(themes.len(), 14);
    for theme in themes {
        let tokens: Tokens = theme.tokens();
        for rgb in NIB_COLOURS {
            for active in [false, true] {
                let c: NibColors = nib_colors_for_theme(&tokens, rgb, active);
                let stroke_k = contrast(c.barrel_stroke, tokens.panel);
                assert!(stroke_k >= 3.0, "{} {:?} active={active}: barrel_stroke contrast {stroke_k:.2} < 3.0", theme.label(), rgb);
                let band_k = contrast(c.band_stroke, c.barrel_fill);
                assert!(band_k >= 2.2, "{} {:?} active={active}: band_stroke contrast {band_k:.2} < 2.2", theme.label(), rgb);
            }
        }
    }

    let (_tmp, mut h, _page) = studio_with_title("p6-upright-gpu", "Harmonic Oscillator Notes", true);
    // Select Theme::DEFAULT (Light) and slot 1 (red pen #d02121 on fresh rack) so active pen has unmistakable red ink bands.
    h.state_mut().set_theme(Theme::DEFAULT);
    h.state_mut().ribbon_act_for_test(ribbon::Act::InkPen(1));
    steps(&mut h, 3);

    let img = support::gpu::render(&mut h).expect("Lavapipe offscreen render of bottom dock");
    save_visual_png(&img, "d6_bottom_dock_upright_nibs.png");

    let ppp = h.ctx.pixels_per_point();
    let bar = tool_bar(&h);
    let rects = dock_pen_rects(&h, ToolbarDock::Bottom);
    let (active_rect, inactive_rect) = (rects[1], rects[2]);
    let light_tokens = Theme::DEFAULT.tokens();
    let red_colors = nib_colors_for_theme(&light_tokens, h.state().pen_rack()[1].colour, true);

    let active_top_y = topmost_non_panel_y(&img, bar, active_rect, light_tokens.panel, ppp);
    let inactive_top_y = topmost_non_panel_y(&img, bar, inactive_rect, light_tokens.panel, ppp);
    assert!(active_top_y + 7 <= inactive_top_y, "active pen topmost pixel y={active_top_y} must sit >= 7px higher than inactive y={inactive_top_y}");

    // Check upright anatomy inside active red pen: red ink at upper tip and lower barrel band,
    // separated by neutral barrel cone in the middle.
    let is_red_ink = |px: Color32| px.r() >= px.g().saturating_add(45) && px.r() >= px.b().saturating_add(45);
    let cx0 = ((active_rect.center().x - 6.0) * ppp).round().max(0.0) as u32;
    let cx1 = ((active_rect.center().x + 6.0) * ppp).round().min((img.width().saturating_sub(1)) as f32) as u32;
    let sample_band = |v_low: f32, v_high: f32| -> Vec<Color32> {
        let y_top = ((active_rect.max.y - v_high * active_rect.height()) * ppp).floor().max(0.0) as u32;
        let y_bot = ((active_rect.max.y - v_low * active_rect.height()) * ppp).ceil().min((img.height().saturating_sub(1)) as f32) as u32;
        let mut out = Vec::new();
        for y in y_top..=y_bot {
            for x in cx0..=cx1 {
                let p = img.get_pixel(x, y);
                out.push(Color32::from_rgb(p[0], p[1], p[2]));
            }
        }
        out
    };
    assert!(sample_band(0.80, 0.99).into_iter().any(is_red_ink), "upper writing tip must contain ink-coloured pixels");
    assert!(sample_band(0.14, 0.35).into_iter().any(is_red_ink), "lower barrel colour band must contain ink-coloured pixels");

    let mid_x = (active_rect.center().x * ppp).round() as u32;
    let mid_y = ((active_rect.max.y - 0.50 * active_rect.height()) * ppp).round() as u32;
    let mid_p = img.get_pixel(mid_x, mid_y);
    let mid_px = Color32::from_rgb(mid_p[0], mid_p[1], mid_p[2]);
    assert!(
        !is_red_ink(mid_px) && manhattan(mid_px, red_colors.barrel_fill) <= 45,
        "neutral cone/barrel at v=0.50 ({mid_px:?}) must separate upper tip from lower colour band (expected ~{:?})",
        red_colors.barrel_fill
    );

    if std::env::var_os("NOCTURNE_UI_VISUAL_DIR").is_some() {
        run(&mut h, "ui.toolbar_dock", &[("edge", "top")]);
        if let Ok(top_img) = support::gpu::render(&mut h) {
            save_visual_png(&top_img, "d6_top_dock_title_clearance.png");
        }
        run(&mut h, "ui.toolbar_dock", &[("edge", "left")]);
        open_tool_settings(&mut h);
        if let Ok(left_img) = support::gpu::render(&mut h) {
            save_visual_png(&left_img, "d6_left_dock_tool_settings.png");
        }
        let close = named(&h, "Close").expect("Close Tool Settings").center();
        click(&mut h, close);
        run(&mut h, "ui.toolbar_dock", &[("edge", "bottom")]);
        h.state_mut().set_theme(Theme { mode: Mode::Dark, accent: Accent::Default });
        steps(&mut h, 3);
        if let Ok(dark_img) = support::gpu::render(&mut h) {
            save_visual_png(&dark_img, "d6_dark_theme_dock.png");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 4. Picture and Math OCR in `ink.configure_tools`, Tool Settings, and deterministic dock actions.
// ---------------------------------------------------------------------------------------------

#[test]
fn picture_and_math_ocr_are_configurable_dock_tools_with_deterministic_actions() {
    let (_tmp, mut h, _page) = studio_with_title("p6-dock-picture-ocr", "Harmonic Oscillator Notes", false);

    run(&mut h, "ink.configure_tools", &[("tools", "picture,math_ocr,-shapes")]);
    let saved_roster = h.state().session().studio_options().roster.write();
    assert!(saved_roster.starts_with("picture,math_ocr,"), "roster order saved: {saved_roster}");
    assert!(saved_roster.contains("-shapes"), "hidden shapes saved: {saved_roster}");

    let bar = tool_bar(&h);
    let pic_btn = in_rect(&h, "Insert picture", bar).expect("Insert picture button on dock");
    let ocr_btn = in_rect(&h, "Recognise equation", bar).expect("Recognise equation button on dock");
    assert!(pic_btn.center().x < ocr_btn.center().x, "Insert picture precedes Recognise equation: {pic_btn:?} vs {ocr_btn:?}");
    assert!(in_rect(&h, "Shapes", bar).is_none(), "hidden Shapes tool is omitted from dock");

    // Verify Picture and Math OCR checkboxes exist inside Tool Settings.
    open_tool_settings(&mut h);
    let _ = reveal(&mut h, "Picture");
    let _ = reveal(&mut h, "Math OCR");
    let close = named(&h, "Close").expect("Close button in Tool Settings").center();
    click(&mut h, close);
    steps(&mut h, 2);
    assert!(area(&h, "studio-tool-settings").is_none(), "Tool Settings closed");

    // When math_ocr_enabled == false, tapping "Recognise equation" opens Tool Settings without enabling OCR.
    assert!(!OcrConfig::load(&h.state().session().settings).enabled, "Math OCR starts disabled by default");
    let ocr_center = in_rect(&h, "Recognise equation", tool_bar(&h)).expect("Recognise equation button").center();
    click(&mut h, ocr_center);
    assert!(area(&h, "studio-tool-settings").is_some(), "tapping Recognise equation while disabled opens Tool Settings");
    assert!(!OcrConfig::load(&h.state().session().settings).enabled, "opening Tool Settings must not silently enable Math OCR");

    let close_again = named(&h, "Close").expect("Close button in Tool Settings").center();
    click(&mut h, close_again);
    steps(&mut h, 2);

    // Enable Math OCR with zero ink held: tapping "Recognise equation" switches pointer to lasso.
    run(&mut h, "ink.math_ocr_toggle", &[("enabled", "true")]);
    assert!(OcrConfig::load(&h.state().session().settings).enabled);
    h.state_mut().ribbon_act_for_test(ribbon::Act::InkPen(0));
    steps(&mut h, 2);
    assert_eq!(h.state().inspect(&h.ctx)["ink"]["pointer"], "draw");
    assert_eq!(h.state().inspect(&h.ctx)["ink"]["holding"], 0);

    let ocr_enabled_center = in_rect(&h, "Recognise equation", tool_bar(&h)).expect("Recognise equation button").center();
    click(&mut h, ocr_enabled_center);
    assert!(area(&h, "studio-tool-settings").is_none(), "enabled Math OCR dock button does not reopen Tool Settings");
    assert_eq!(h.state().inspect(&h.ctx)["ink"]["pointer"], "lasso", "with no ink held, Recognise equation switches pointer to lasso");
}

// ---------------------------------------------------------------------------------------------
// 5. `< 600`-line module split guard and re-export verification.
// ---------------------------------------------------------------------------------------------

#[test]
fn tablet_chrome_modules_and_phase6_test_stay_under_600_lines() {
    let chrome_lines = include_str!("../src/tablet_chrome.rs").lines().count();
    let dock_lines = include_str!("../src/tablet_dock.rs").lines().count();
    let nibs_lines = include_str!("../src/tablet_nibs.rs").lines().count();
    let test_lines = include_str!("ipad_studio_v2_phase6.rs").lines().count();

    assert!(chrome_lines < 600, "tablet_chrome.rs must stay under 600 lines, got {chrome_lines}");
    assert!((20..600).contains(&dock_lines), "tablet_dock.rs must be non-empty and under 600 lines, got {dock_lines}");
    assert!((20..600).contains(&nibs_lines), "tablet_nibs.rs must be non-empty and under 600 lines, got {nibs_lines}");
    assert!(test_lines < 600, "ipad_studio_v2_phase6.rs must stay under 600 lines, got {test_lines}");

    let view = Rect::from_min_max(pos2(0.0, 0.0), pos2(1180.0, 820.0));
    assert_eq!(nearest_dock(view, pos2(590.0, 810.0)), ToolbarDock::Bottom);
    assert_eq!(nearest_dock(view, pos2(590.0, 10.0)), ToolbarDock::Top);
    assert_eq!(nearest_dock(view, pos2(10.0, 410.0)), ToolbarDock::Left);
    assert_eq!(nearest_dock(view, pos2(1170.0, 410.0)), ToolbarDock::Right);
    assert_eq!(UiLayoutMode::parse("tablet"), UiLayoutMode::TabletStudio);
    assert!(UiLayoutMode::Auto.is_tablet_studio(OperatingSystem::IOS, false));
}
