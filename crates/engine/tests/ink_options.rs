//! The eraser's kind and size, and hold to straighten: commands, limits, and the settings file.

use nocturne_engine::{ERASER_SIZES, EngineError, EraserKind, InkOptions, Output, Params, Session, registry};
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
fn a_fresh_session_has_the_object_eraser_at_its_small_size_and_straightening_on() {
    let t = TempDir::new("ink-options-fresh");
    let s = session(&t);
    assert_eq!(s.ink_options(), InkOptions { eraser_kind: EraserKind::Object, eraser_width: 6.0, hold_to_straighten: true });
    assert_eq!(ERASER_SIZES, [("s", 6.0), ("m", 12.0), ("l", 24.0), ("xl", 40.0)]);
    for id in ["ink.eraser_mode", "ink.eraser_width", "ink.hold_to_straighten"] {
        let spec = registry().iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{id} is a command"));
        assert!(!spec.changes_tree, "{id} does not change the notebooks");
    }
}

#[test]
fn the_eraser_mode_is_set_toggled_and_refuses_what_it_does_not_know() {
    let t = TempDir::new("ink-options-mode");
    let mut s = session(&t);
    assert_eq!(json_of(s.run("ink.eraser_mode", &p(&[("mode", " Pixel ")])).expect("pixel")), json!({ "eraser_mode": "pixel" }));
    assert_eq!(s.ink_options().eraser_kind, EraserKind::Pixel);
    assert_eq!(json_of(s.run("ink.eraser_mode", &Params::new()).expect("toggle")), json!({ "eraser_mode": "object" }));
    assert_eq!(json_of(s.run("ink.eraser_mode", &p(&[("mode", "toggle")])).expect("toggle")), json!({ "eraser_mode": "pixel" }));
    assert!(matches!(s.run("ink.eraser_mode", &p(&[("mode", "smudge")])), Err(EngineError::BadParam(_))));
    assert_eq!(s.ink_options().eraser_kind, EraserKind::Pixel, "a refused mode changes nothing");
}

#[test]
fn the_eraser_width_takes_a_named_size_or_points_within_its_limits() {
    let t = TempDir::new("ink-options-width");
    let mut s = session(&t);
    for (name, width) in ERASER_SIZES {
        assert_eq!(json_of(s.run("ink.eraser_width", &p(&[("width", name)])).expect("named size")), json!({ "eraser_width": width }));
        assert_eq!(s.ink_options().eraser_width, width);
    }
    assert_eq!(json_of(s.run("ink.eraser_width", &p(&[("width", " XL ")])).expect("padded name")), json!({ "eraser_width": 40.0 }));
    assert_eq!(json_of(s.run("ink.eraser_width", &p(&[("width", "17.5")])).expect("points")), json!({ "eraser_width": 17.5 }));
    for bad in ["", "huge", "1.9", "80.1", "-4", "NaN", "inf"] {
        assert!(matches!(s.run("ink.eraser_width", &p(&[("width", bad)])), Err(EngineError::BadParam(_))), "{bad:?} is refused");
    }
    assert!(matches!(s.run("ink.eraser_width", &Params::new()), Err(EngineError::BadParam(_))), "a width is needed");
    assert_eq!(s.ink_options().eraser_width, 17.5, "a refused width changes nothing");
    assert!(!s.set_eraser_width(f32::NAN));
}

#[test]
fn hold_to_straighten_is_set_and_toggled() {
    let t = TempDir::new("ink-options-straighten");
    let mut s = session(&t);
    assert_eq!(json_of(s.run("ink.hold_to_straighten", &Params::new()).expect("toggle")), json!({ "hold_to_straighten": false }));
    assert_eq!(json_of(s.run("ink.hold_to_straighten", &p(&[("enabled", "TRUE")])).expect("true")), json!({ "hold_to_straighten": true }));
    assert_eq!(json_of(s.run("ink.hold_to_straighten", &p(&[("enabled", "false")])).expect("false")), json!({ "hold_to_straighten": false }));
    assert!(matches!(s.run("ink.hold_to_straighten", &p(&[("enabled", "maybe")])), Err(EngineError::BadParam(_))));
    assert!(!s.ink_options().hold_to_straighten);
}

#[test]
fn the_choices_are_remembered_by_the_next_session_and_junk_in_the_file_is_the_default() {
    let t = TempDir::new("ink-options-persist");
    let mut s = session(&t);
    s.run("ink.eraser_mode", &p(&[("mode", "pixel")])).expect("mode");
    s.run("ink.eraser_width", &p(&[("width", "l")])).expect("width");
    s.run("ink.hold_to_straighten", &p(&[("enabled", "false")])).expect("straighten");
    drop(s);
    let again = session(&t);
    assert_eq!(again.ink_options(), InkOptions { eraser_kind: EraserKind::Pixel, eraser_width: 24.0, hold_to_straighten: false });
    drop(again);

    let settings = Settings::at(t.path().join("settings").join("s.toml"));
    settings.set("eraser_mode", "smudge").expect("junk mode");
    settings.set("eraser_width", "9000").expect("junk width");
    settings.set("hold_to_straighten", "perhaps").expect("junk switch");
    let s = Session::new(&t.path().join("notebooks"), settings).expect("session");
    assert_eq!(s.ink_options(), InkOptions::default(), "a settings file that was meddled with does not break the eraser");
}
