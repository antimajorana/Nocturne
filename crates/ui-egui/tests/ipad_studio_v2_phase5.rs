//! iPad Studio UX v2, phase 5: streamlined two-pane library grid (`ui.library_view`),
//! space-preserving multi-word page cards with cached ink and text thumbnails, quick
//! Handwrite / Type / Import pills, library search with platform keyboard reporting,
//! and `<` canvas return without the old top bar.
//!
//! Each test drives the real `Session` and `NocturneApp` against real files in a
//! temporary directory. Nothing is mocked.

use egui::os::OperatingSystem;
use egui::{Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use nocturne_engine::{EngineError, InkActiveMode, LibraryView, Output, Params, Session, StudioOptions, registry};
use nocturne_ink::svg::Flavour;
use nocturne_ink::{Dab, Drawing, Rgb, Stroke, Tool};
use nocturne_store::Settings;
use nocturne_store::testing::TempDir;
use nocturne_ui_egui::library_grid::{LIBRARY_SEARCH_FIELD, RECENT_PAGES_KEY};
use nocturne_ui_egui::theme::Theme;
use nocturne_ui_egui::{NocturneApp, PlatformTextField, ribbon};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[allow(dead_code)]
mod support;

const SCREEN: egui::Vec2 = vec2(1180.0, 820.0);

struct LibraryFixture {
    _tmp: TempDir,
    lecture_notes: PathBuf,
    wave_page: PathBuf,
    spring_page: PathBuf,
}

fn params(pairs: &[(&str, &str)]) -> Params {
    pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
}

fn json_of(out: Output) -> Value {
    match out {
        Output::Json(v) => v,
        other => panic!("expected Output::Json, got {other:?}"),
    }
}

fn path_of(out: Output) -> PathBuf {
    match out {
        Output::Path(p) => p,
        other => panic!("expected Output::Path, got {other:?}"),
    }
}

fn build_library_session(name: &str) -> (LibraryFixture, Session) {
    let tmp = TempDir::new(name);
    let settings = Settings::at(tmp.path().join("cfg/settings.toml"));
    settings.set("privacy_welcome_seen", "true").expect("welcome seen");
    settings.set_default_format(nocturne_model::PageFormat::Cmgfm).expect("cmgfm default");
    let mut session = Session::new(&tmp.path().join("notebooks"), settings).expect("session");

    let nb = path_of(session.run("notebook.create", &params(&[("name", "Studio Atlas"), ("empty", "true")])).expect("create Studio Atlas"));
    let nb_str = nb.to_string_lossy().into_owned();
    let lecture_notes = path_of(
        session.run("node.create", &params(&[("parent", &nb_str), ("kind", "section"), ("title", "Lecture Notes")])).expect("create Lecture Notes"),
    );
    let lecture_str = lecture_notes.to_string_lossy().into_owned();
    let wave_page = path_of(
        session
            .run("node.create", &params(&[("parent", &lecture_str), ("kind", "page"), ("title", "Wave Packet Derivation")]))
            .expect("create Wave Packet Derivation"),
    );
    let spring_page = path_of(
        session
            .run("node.create", &params(&[("parent", &lecture_str), ("kind", "page"), ("title", "Spring Schedule Overview")]))
            .expect("create Spring Schedule Overview"),
    );
    let weekly_plans = path_of(
        session.run("node.create", &params(&[("parent", &nb_str), ("kind", "section"), ("title", "Weekly Plans")])).expect("create Weekly Plans"),
    );
    let weekly_str = weekly_plans.to_string_lossy().into_owned();
    session.run("node.create", &params(&[("parent", &weekly_str), ("kind", "page"), ("title", "Lab Archive")])).expect("create Lab Archive");

    (LibraryFixture { _tmp: tmp, lecture_notes, wave_page, spring_page }, session)
}

fn harness_from_session(session: Session, size: egui::Vec2) -> Harness<'static, NocturneApp> {
    let app = NocturneApp::new(session);
    let mut builder = Harness::builder().with_size(size).with_accessibility_check(false);
    if std::env::var_os("NOCTURNE_UI_VISUAL_DIR").is_some() {
        builder = builder.renderer(support::gpu::renderer());
    }
    let mut h = builder.build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
    h.ctx.set_os(OperatingSystem::IOS);
    steps(&mut h, 4);
    assert!(h.state().is_tablet_studio(&h.ctx), "iPadOS runs in Tablet Studio");
    h
}

fn studio_library(name: &str) -> (LibraryFixture, Harness<'static, NocturneApp>) {
    let (fixture, session) = build_library_session(name);
    let h = harness_from_session(session, SCREEN);
    (fixture, h)
}

fn maybe_save_visual_png(h: &mut Harness<'_, NocturneApp>, stem: &str) {
    if let Some(dir) = std::env::var_os("NOCTURNE_UI_VISUAL_DIR") {
        let dir = PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(img) = support::gpu::render(h) {
            let _ = img.save(dir.join(format!("{stem}.png")));
        }
    }
}

fn steps(h: &mut Harness<'_, NocturneApp>, count: usize) {
    for _ in 0..count {
        let queued = h.state_mut().take_events();
        h.input_mut().events.extend(queued);
        h.step();
    }
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

fn sidebar_item(h: &Harness<'_, NocturneApp>, title: &str) -> Option<Rect> {
    let prefix_space = format!("{title} ").to_ascii_lowercase();
    let prefix_paren = format!("{title}(").to_ascii_lowercase();
    labelled(h, |l| {
        let low = l.to_ascii_lowercase();
        l.eq_ignore_ascii_case(title) || low.starts_with(&prefix_space) || low.starts_with(&prefix_paren)
    })
}

fn area(h: &Harness<'_, NocturneApp>, id: &str) -> Option<Rect> {
    let target = egui::Id::unique(id);
    h.ctx.memory(|m| m.areas().visible_layer_ids().iter().any(|l| l.id == target).then(|| m.area_rect(target)).flatten())
}

fn old_top_bar_showing(h: &Harness<'_, NocturneApp>) -> bool {
    area(h, "studio-top-bar").is_some()
        || (named(h, "File").is_some_and(|r| r.max.y <= 56.0) && labelled(h, |l| l.ends_with('%')).is_some_and(|r| r.max.y <= 56.0))
}

fn button_ev(pressed: bool, pos: Pos2) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
}

fn click(h: &mut Harness<'_, NocturneApp>, at: Pos2) {
    h.input_mut().events.extend([egui::Event::PointerMoved(at), button_ev(true, at)]);
    h.step();
    h.input_mut().events.push(button_ev(false, at));
    steps(h, 4);
}

fn drag_stroke(h: &mut Harness<'_, NocturneApp>, from: Pos2, to: Pos2) {
    h.input_mut().events.extend([egui::Event::PointerMoved(from), button_ev(true, from)]);
    h.step();
    for i in 1..=16 {
        let p = from.lerp(to, i as f32 / 16.0);
        h.input_mut().events.push(egui::Event::PointerMoved(p));
        h.step();
    }
    h.input_mut().events.push(button_ev(false, to));
    steps(h, 4);
}

fn press_escape(h: &mut Harness<'_, NocturneApp>) {
    let key = |pressed| egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    h.input_mut().events.extend([key(true), key(false)]);
    steps(h, 3);
}

fn write_editable_ink(page_dir: &Path, stroke_count: usize) {
    let mut drawing = Drawing::new(620.0, 420.0);
    for idx in 0..stroke_count {
        let mut stroke = Stroke::new(Tool::Pen, Rgb(0x1a, 0x5f, 0xd0), 3.4);
        let base_y = 90.0 + idx as f32 * 55.0;
        for step in 0..12 {
            let x = 70.0 + step as f32 * 22.0;
            let y = base_y + ((step % 3) as f32) * 8.0;
            stroke.add(Dab::new(x, y, 0.8));
        }
        drawing.add(stroke);
    }
    let svg = nocturne_ink::svg::write(&drawing, Flavour::Editable);
    std::fs::write(page_dir.join("ink.svg"), svg).expect("write editable ink.svg");
}

fn count_pages_in_dir(section_dir: &Path) -> usize {
    std::fs::read_dir(section_dir)
        .expect("read section dir")
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir() && nocturne_store::tree::page_file(&e.path()).is_some())
        .count()
}

// ---------------------------------------------------------------------------------------------
// 1. Engine command `ui.library_view` defaults, switches, persists, and rejects junk.
// ---------------------------------------------------------------------------------------------

#[test]
fn command_ui_library_view_defaults_to_studio_grid_switches_persists_and_rejects_junk() {
    let tmp = TempDir::new("p5-cmd-library-view");
    let settings_path = tmp.path().join("cfg/settings.toml");
    let mut s = Session::new(&tmp.path().join("notebooks"), Settings::at(settings_path.clone())).expect("session");

    assert_eq!(s.library_view(), LibraryView::StudioGrid);
    assert_eq!(s.studio_options().library_view, LibraryView::StudioGrid);
    assert_eq!(LibraryView::StudioGrid.id(), "studio_grid");
    assert_eq!(LibraryView::ClassicTree.id(), "classic_tree");
    assert_eq!(LibraryView::parse(" StudioGrid "), Some(LibraryView::StudioGrid));
    assert_eq!(LibraryView::parse("classic-tree"), Some(LibraryView::ClassicTree));
    assert_eq!(LibraryView::parse("bogus"), None);
    assert!(
        registry().iter().any(|c| c.id == "ui.library_view" && !c.changes_tree),
        "ui.library_view must be registered in the command table with changes_tree = false"
    );

    let initial = json_of(s.run("ui.library_view", &Params::new()).expect("query ui.library_view"));
    assert_eq!(initial, json!({ "library_view": "studio_grid" }));

    let classic = json_of(s.run("ui.library_view", &params(&[("view", "classic_tree")])).expect("set classic_tree"));
    assert_eq!(classic, json!({ "library_view": "classic_tree" }));
    assert_eq!(s.library_view(), LibraryView::ClassicTree);
    drop(s);

    let mut reopened = Session::new(&tmp.path().join("notebooks"), Settings::at(settings_path.clone())).expect("reopen session");
    assert_eq!(reopened.library_view(), LibraryView::ClassicTree, "choice persists in settings.toml");

    let grid = json_of(reopened.run("ui.library_view", &params(&[("view", "StudioGrid")])).expect("set StudioGrid"));
    assert_eq!(grid, json!({ "library_view": "studio_grid" }));
    assert_eq!(reopened.library_view(), LibraryView::StudioGrid);

    let toggled = json_of(reopened.run("ui.library_view", &params(&[("view", "toggle")])).expect("toggle 1"));
    assert_eq!(toggled, json!({ "library_view": "classic_tree" }));
    let toggled_back = json_of(reopened.run("ui.library_view", &params(&[("view", "toggle")])).expect("toggle 2"));
    assert_eq!(toggled_back, json!({ "library_view": "studio_grid" }));

    assert!(
        matches!(reopened.run("ui.library_view", &params(&[("view", "bogus")])), Err(EngineError::BadParam(_))),
        "junk view parameter must return EngineError::BadParam"
    );
    assert_eq!(reopened.library_view(), LibraryView::StudioGrid, "refused view mutates nothing");

    reopened.set_library_view(LibraryView::ClassicTree);
    drop(reopened);
    let settings = Settings::at(settings_path);
    settings.set("library_view", "bogus").expect("write junk setting");
    let fallback = Session::new(&tmp.path().join("notebooks"), settings).expect("fallback session");
    assert_eq!(fallback.studio_options(), StudioOptions::default());
}

// ---------------------------------------------------------------------------------------------
// 2. Two-pane library grid with no open page, old top bar suppressed, zero Freenotes clutter.
// ---------------------------------------------------------------------------------------------

#[test]
fn no_open_page_in_studio_grid_shows_two_pane_library_and_suppresses_old_top_bar() {
    let (_lib, mut h) = studio_library("p5-two-pane-no-top-bar");
    maybe_save_visual_png(&mut h, "d5_library_grid_two_pane");

    assert!(h.state().is_studio_grid(&h.ctx), "StudioGrid is active by default on iPadOS");
    assert_eq!(area(&h, "studio-top-bar"), None, "studio-top-bar area must not exist");
    assert!(!old_top_bar_showing(&h), "old full-width Studio top bar must be suppressed");
    assert_eq!(h.state().page_view_rect(), None, "no page canvas is open");

    for item in ["Recents", "Studio Atlas", "Lecture Notes", "Weekly Plans"] {
        let r = sidebar_item(&h, item).unwrap_or_else(|| panic!("sidebar must display {item:?}"));
        assert!(r.center().x < 340.0, "{item:?} sits in the left sidebar pane: {r:?}");
    }
    let handwrite = named(&h, "Handwrite").expect("Handwrite quick pill in right pane");
    assert!(handwrite.min.x >= 220.0, "right pane sits beside left sidebar: {handwrite:?}");

    for clutter in ["Template Library", "Upgrade to Premium", "Recently Deleted", "iCloud", "On My iPad", "School Portal"] {
        assert_eq!(labelled(&h, |l| l.contains(clutter)), None, "clutter label {clutter:?} must never appear");
    }

    let weekly = sidebar_item(&h, "Weekly Plans").expect("Weekly Plans").center();
    click(&mut h, weekly);
    assert!(named(&h, "Lab Archive").is_some(), "selecting Weekly Plans shows its page card");
    assert_eq!(named(&h, "Wave Packet Derivation"), None);
}

// ---------------------------------------------------------------------------------------------
// 3. Multi-word card titles preserve spaces, cache ink + text thumbnails, and render 14 themes.
// ---------------------------------------------------------------------------------------------

#[test]
fn page_cards_preserve_spaces_in_multi_word_titles_and_render_cached_ink_and_text_thumbnails() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for rel in ["src/library_grid.rs", "tests/ipad_studio_v2_phase5.rs"] {
        let path = manifest_dir.join(rel);
        let lines = std::fs::read_to_string(&path).expect("read module").lines().count();
        assert!(lines < 600, "{} must stay under 600 lines (got {lines})", path.display());
    }

    let (lib, mut session) = build_library_session("p5-cards-thumbs-themes");
    write_editable_ink(&lib.wave_page, 2);
    let spring_str = lib.spring_page.to_string_lossy().into_owned();
    session
        .run(
            "page.save",
            &params(&[
                ("path", &spring_str),
                ("body", "Quantum harmonic oscillator lecture plan\nBoundary value homework schedule\nOffice hours agenda"),
            ]),
        )
        .expect("save markdown body");

    let mut h = harness_from_session(session, SCREEN);
    maybe_save_visual_png(&mut h, "d5_library_grid_two_pane");

    let r1 = named(&h, "Wave Packet Derivation").expect("card preserves spaces in Wave Packet Derivation");
    let r2 = named(&h, "Spring Schedule Overview").expect("card preserves spaces in Spring Schedule Overview");
    assert_eq!(named(&h, "WavePacketDerivation"), None, "spaces must never be stripped");
    assert_eq!(named(&h, "SpringScheduleOverview"), None, "spaces must never be stripped");
    assert!((r1.min.x - r2.min.x).abs() > 60.0, "1180pt landscape places cards in multiple columns: r1={r1:?}, r2={r2:?}");

    let (_, wave_strokes) = h.state().cached_library_thumb_counts(&lib.wave_page).expect("Wave Packet Derivation thumbnail cached");
    assert!(wave_strokes >= 2, "expected cached ink strokes >= 2, got {wave_strokes}");

    let (spring_lines, spring_strokes) = h.state().cached_library_thumb_counts(&lib.spring_page).expect("Spring Schedule Overview thumbnail cached");
    assert!(spring_lines > 0, "expected cached text lines > 0, got {spring_lines}");
    assert_eq!(spring_strokes, 0);

    let themes = Theme::all();
    assert_eq!(themes.len(), 14);
    for theme in themes {
        h.state_mut().set_theme(theme);
        steps(&mut h, 2);
        assert!(named(&h, "Wave Packet Derivation").is_some(), "renders cleanly in {}", theme.id());
    }
}

// ---------------------------------------------------------------------------------------------
// 4. Tapping card opens canvas; `<` returns to library, updates Recents, refreshes thumbnail.
// ---------------------------------------------------------------------------------------------

#[test]
fn tapping_card_opens_canvas_and_header_back_chevron_returns_to_library_with_fresh_thumbnail() {
    let (lib, session) = build_library_session("p5-card-open-and-back");
    write_editable_ink(&lib.wave_page, 1);
    let mut h = harness_from_session(session, SCREEN);

    let (_, initial_strokes) = h.state().cached_library_thumb_counts(&lib.wave_page).expect("initial cached thumb");
    assert_eq!(initial_strokes, 1);

    let card = named(&h, "Wave Packet Derivation").expect("Wave Packet Derivation card").center();
    click(&mut h, card);

    assert!(h.state().page_view_rect().is_some(), "tapping card opens canvas");
    assert!(area(&h, "studio-header-pill").is_some(), "floating studio header pill is visible");
    assert!(named(&h, "Back to library").is_some(), "header pill shows Back to library chevron");

    h.state_mut().ribbon_act_for_test(ribbon::Act::InkPen(0));
    steps(&mut h, 2);
    assert_eq!(h.state().ink_stroke_count(), 1);
    let page = h.state().page_rect().expect("open page rect");
    drag_stroke(&mut h, pos2(page.min.x + 180.0, page.min.y + 280.0), pos2(page.min.x + 420.0, page.min.y + 300.0));
    assert_eq!(h.state().ink_stroke_count(), 2, "new stroke drawn on open page");

    let back = named(&h, "Back to library").expect("Back to library chevron").center();
    click(&mut h, back);

    assert_eq!(h.state().page_view_rect(), None, "canvas closed back to library");
    assert_eq!(area(&h, "studio-top-bar"), None, "studio-top-bar remains suppressed");
    assert!(!old_top_bar_showing(&h), "old top bar remains suppressed after returning");

    let saved_recents = h.state().session().settings.get(RECENT_PAGES_KEY).unwrap_or_default();
    assert!(!saved_recents.is_empty(), "RECENT_PAGES_KEY persisted in settings");

    let recents = sidebar_item(&h, "Recents").expect("Recents sidebar item").center();
    click(&mut h, recents);
    assert!(named(&h, "Wave Packet Derivation").is_some(), "Recents contains the opened Wave Packet Derivation page");
    steps(&mut h, 2);
    let (_, updated_strokes) = h.state().cached_library_thumb_counts(&lib.wave_page).expect("refreshed cached thumb");
    assert_eq!(updated_strokes, 2, "thumbnail cache invalidated and rebuilt with 2 strokes");
}

// ---------------------------------------------------------------------------------------------
// 5. Quick Handwrite, Type, and Import pills create pages with matching tool and focus.
// ---------------------------------------------------------------------------------------------

#[test]
fn quick_handwrite_and_type_pills_create_pages_with_matching_tool_and_focus() {
    let (lib, mut h) = studio_library("p5-quick-pills");
    assert_eq!(count_pages_in_dir(&lib.lecture_notes), 2);

    let import = named(&h, "Import").expect("Import pill").center();
    click(&mut h, import);
    assert!(named(&h, "Import document").is_some(), "Import menu reveals Import document");
    assert!(named(&h, "Import picture").is_some(), "Import menu reveals Import picture");
    press_escape(&mut h);

    let handwrite = named(&h, "Handwrite").expect("Handwrite pill").center();
    click(&mut h, handwrite);
    assert_eq!(count_pages_in_dir(&lib.lecture_notes), 3, "Handwrite creates a page in Lecture Notes");
    assert!(h.state().page_view_rect().is_some(), "Handwrite opens the canvas");
    assert_eq!(h.state().session().ink_tool_state().active_mode, InkActiveMode::Draw, "Handwrite switches active ink mode to Draw");

    let back = named(&h, "Back to library").expect("Back to library").center();
    click(&mut h, back);
    assert_eq!(h.state().page_view_rect(), None);

    let type_pill = named(&h, "Type").expect("Type pill").center();
    click(&mut h, type_pill);
    assert_eq!(count_pages_in_dir(&lib.lecture_notes), 4, "Type creates a page in Lecture Notes");
    assert!(h.state().page_view_rect().is_some(), "Type opens the canvas");
    assert_eq!(h.state().session().ink_tool_state().active_mode, InkActiveMode::Write, "Type switches active ink mode to Write");
    assert!(h.state().title_is_focused() || h.state().focused_text_field(&h.ctx).is_some(), "Type focuses the new page title/editor immediately");
}

// ---------------------------------------------------------------------------------------------
// 6. Library search filters by title and body, preserves spaces for IME, and clears cleanly.
// ---------------------------------------------------------------------------------------------

#[test]
fn library_search_filters_cards_by_title_and_body_and_reports_platform_text_field() {
    let (lib, mut session) = build_library_session("p5-library-search");
    let spring_str = lib.spring_page.to_string_lossy().into_owned();
    session.run("page.save", &params(&[("path", &spring_str), ("body", "Colloquium room booking and faculty seminar roster")])).expect("save body");
    let mut h = harness_from_session(session, SCREEN);

    assert!(named(&h, "Wave Packet Derivation").is_some());
    assert!(named(&h, "Spring Schedule Overview").is_some());

    h.ctx.memory_mut(|m| m.request_focus(egui::Id::unique(LIBRARY_SEARCH_FIELD)));
    steps(&mut h, 3);
    assert_eq!(
        h.state().focused_text_field(&h.ctx),
        Some(PlatformTextField { text: String::new(), selection: (0, 0) }),
        "focused LIBRARY_SEARCH_FIELD must be reported to the platform keyboard"
    );

    for piece in ["faculty", " ", "seminar"] {
        h.input_mut().events.push(egui::Event::Text(piece.to_string()));
        steps(&mut h, 2);
    }
    assert_eq!(
        h.state().focused_text_field(&h.ctx),
        Some(PlatformTextField { text: "faculty seminar".to_string(), selection: (15, 15) }),
        "platform text field preserves typed spaces in search query"
    );
    assert!(named(&h, "Spring Schedule Overview").is_some(), "body match remains visible");
    assert_eq!(named(&h, "Wave Packet Derivation"), None, "non-matching card is filtered out");
    assert_eq!(area(&h, "search-results"), None, "desktop floating search popup stays hidden");

    let clear = named(&h, "Clear search").expect("Clear search button").center();
    click(&mut h, clear);
    assert!(named(&h, "Spring Schedule Overview").is_some(), "cards restored after Clear search");
    assert!(named(&h, "Wave Packet Derivation").is_some(), "cards restored after Clear search");

    h.ctx.memory_mut(|m| m.request_focus(egui::Id::unique(LIBRARY_SEARCH_FIELD)));
    steps(&mut h, 2);
    h.input_mut().events.push(egui::Event::Text("Wave Packet".to_string()));
    steps(&mut h, 3);
    assert!(named(&h, "Wave Packet Derivation").is_some(), "title match remains visible");
    assert_eq!(named(&h, "Spring Schedule Overview"), None, "non-matching title is filtered out");
}

// ---------------------------------------------------------------------------------------------
// 7. Bottom `+` buttons open dialogs and `More...` switches between StudioGrid and ClassicTree.
// ---------------------------------------------------------------------------------------------

#[test]
fn bottom_plus_buttons_open_dialogs_and_more_menu_switches_between_studio_grid_and_classic_tree() {
    let (_lib, mut h) = studio_library("p5-dialogs-and-view-switch");
    assert_eq!(h.state().session().library_view(), LibraryView::StudioGrid);

    let new_nb = named(&h, "New notebook").expect("bottom-left New notebook button").center();
    click(&mut h, new_nb);
    assert!(area(&h, "new-notebook-modal-backdrop").is_some(), "New notebook button opens Dialog::NewNotebook");
    let cancel = named(&h, "Cancel").expect("Cancel New notebook").center();
    click(&mut h, cancel);
    assert_eq!(area(&h, "new-notebook-modal-backdrop"), None);

    let new_sec = named(&h, "+ New section").expect("bottom-left + New section button").center();
    click(&mut h, new_sec);
    assert!(area(&h, "add-modal-backdrop").is_some(), "+ New section button opens Dialog::Add");
    let cancel = named(&h, "Cancel").expect("Cancel + New section").center();
    click(&mut h, cancel);
    assert_eq!(area(&h, "add-modal-backdrop"), None);

    let more = named(&h, "More...").expect("Library sidebar More... button").center();
    click(&mut h, more);
    let use_tree = named(&h, "Use classic tree").expect("Use classic tree menu item").center();
    click(&mut h, use_tree);

    assert_eq!(h.state().session().library_view(), LibraryView::ClassicTree);
    assert!(!h.state().is_studio_grid(&h.ctx));

    let view_menu_trigger = named(&h, "More...").or_else(|| labelled(&h, |l| l.ends_with('%'))).expect("ClassicTree view menu trigger").center();
    click(&mut h, view_menu_trigger);
    let use_grid = named(&h, "Use library grid").expect("Use library grid menu item").center();
    click(&mut h, use_grid);

    assert_eq!(h.state().session().library_view(), LibraryView::StudioGrid);
    assert!(h.state().is_studio_grid(&h.ctx));
    assert!(!old_top_bar_showing(&h));
}
