//! Where the Tablet Studio tool bar sits and what is on it: commands, limits, the settings file.

use nocturne_engine::{DockRoster, DockTool, EngineError, Output, Params, Session, StudioOptions, ToolbarDock, registry};
use nocturne_store::Settings;
use nocturne_store::testing::TempDir;
use serde_json::{Value, json};

fn session(t: &TempDir) -> Session {
    Session::new(&t.path().join("notebooks"), Settings::at(t.path().join("settings").join("s.toml"))).expect("session")
}

fn p(pairs: &[(&str, &str)]) -> Params {
    pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
}

fn json_of(out: Output) -> Value {
    match out {
        Output::Json(v) => v,
        other => panic!("expected Output::Json, got {other:?}"),
    }
}

#[test]
fn a_fresh_session_has_the_bar_at_the_bottom_with_every_tool_in_its_usual_order() {
    let t = TempDir::new("studio-options-fresh");
    let s = session(&t);
    assert_eq!(s.studio_options(), StudioOptions::default());
    assert_eq!(s.studio_options().toolbar_dock, ToolbarDock::Bottom);
    assert_eq!(s.studio_options().roster.shown().collect::<Vec<_>>(), DockTool::ALL.to_vec());
    assert_eq!(s.studio_options().roster.write(), "history,write,pens,eraser,lasso,select,shapes");
    for id in ["ui.toolbar_dock", "ink.configure_tools"] {
        assert!(registry().iter().any(|c| c.id == id && !c.changes_tree), "{id} is a command that leaves the notebooks alone");
    }
}

#[test]
fn the_bar_moves_to_each_edge_and_refuses_a_place_that_is_not_one() {
    let t = TempDir::new("studio-options-dock");
    let mut s = session(&t);
    for dock in ToolbarDock::ALL {
        assert_eq!(json_of(s.run("ui.toolbar_dock", &p(&[("edge", dock.id())])).expect("edge")), json!({ "toolbar_dock": dock.id() }));
        assert_eq!(s.studio_options().toolbar_dock, dock);
        assert_eq!(dock.is_vertical(), matches!(dock, ToolbarDock::Left | ToolbarDock::Right));
    }
    assert_eq!(json_of(s.run("ui.toolbar_dock", &p(&[("edge", " LEFT ")])).expect("padded")), json!({ "toolbar_dock": "left" }));
    for bad in ["", "middle", "bottom-left"] {
        assert!(matches!(s.run("ui.toolbar_dock", &p(&[("edge", bad)])), Err(EngineError::BadParam(_))), "{bad:?} is refused");
    }
    assert!(matches!(s.run("ui.toolbar_dock", &Params::new()), Err(EngineError::BadParam(_))), "an edge is needed");
    assert_eq!(s.studio_options().toolbar_dock, ToolbarDock::Left, "a refused edge changes nothing");
}

#[test]
fn the_tools_are_reordered_and_hidden_and_a_hidden_tool_keeps_its_place() {
    let t = TempDir::new("studio-options-roster");
    let mut s = session(&t);
    let out = json_of(s.run("ink.configure_tools", &p(&[("tools", "pens, eraser, -lasso, write")])).expect("configure"));
    assert_eq!(out, json!({ "tools": "pens,eraser,-lasso,write,history,select,shapes" }), "tools left out follow, shown");
    let roster = s.studio_options().roster;
    assert_eq!(
        roster.shown().collect::<Vec<_>>(),
        [DockTool::Pens, DockTool::Eraser, DockTool::Write, DockTool::History, DockTool::Select, DockTool::Shapes]
    );
    assert_eq!(json_of(s.run("ink.configure_tools", &Params::new()).expect("report")), out, "without a list it reports the one it has");

    assert!(matches!(s.run("ink.configure_tools", &p(&[("tools", "pens,crayon")])), Err(EngineError::BadParam(_))));
    assert_eq!(s.studio_options().roster, roster, "a list with a slip in it changes nothing");

    let mut edited = roster;
    edited.set_shown(DockTool::Lasso, true);
    assert!(edited.shift(DockTool::Lasso, true));
    assert!(!edited.shift(DockTool::Pens, true), "the first tool cannot go earlier");
    assert!(!edited.shift(DockTool::Shapes, false), "nor the last later");
    assert_eq!(edited.write(), "pens,lasso,eraser,write,history,select,shapes");
    assert_eq!(
        DockRoster::parse("pens,pens,-pens").expect("repeats").write(),
        "pens,history,write,eraser,lasso,select,shapes",
        "a tool named twice counts once"
    );
}

#[test]
fn the_layout_is_remembered_and_junk_in_the_file_is_the_default() {
    let t = TempDir::new("studio-options-persist");
    let mut s = session(&t);
    s.run("ui.toolbar_dock", &p(&[("edge", "right")])).expect("edge");
    s.run("ink.configure_tools", &p(&[("tools", "-shapes,history,pens")])).expect("tools");
    let before = s.studio_options();
    drop(s);
    assert_eq!(session(&t).studio_options(), before);

    let settings = Settings::at(t.path().join("settings").join("s.toml"));
    settings.set("toolbar_dock", "ceiling").expect("junk edge");
    settings.set("studio_dock_tools", "pens,crayon").expect("junk tools");
    let s = Session::new(&t.path().join("notebooks"), settings).expect("session");
    assert_eq!(s.studio_options(), StudioOptions::default());
}
