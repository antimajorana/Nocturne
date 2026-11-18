//! Opaque-box E2E tests for F1 (`ink.toggle_eraser`) and F2 (`ink.finger_drawing`) across Tiers 1-4.

use nocturne_engine::{EngineError, InkActiveMode, InkPenKind, Output, Params, Session, commands_json, output_json, params_from_json, registry};
use nocturne_store::{Settings, testing::TempDir};
use serde_json::{Value, json};
use std::fs;

fn new_session(t: &TempDir) -> Session {
    Session::new(&t.path().join("notebooks"), Settings::at(t.path().join("settings").join("s.toml"))).expect("create session")
}

fn p(pairs: &[(&str, &str)]) -> Params {
    pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
}

fn parse_json_text(out: Output) -> Value {
    match output_json(&out) {
        Value::String(s) => serde_json::from_str::<Value>(&s).unwrap_or_else(|e| panic!("expected JSON object in Output::Text, got {s:?}: {e}")),
        other => other,
    }
}

fn text_of(out: Output) -> String {
    match out {
        Output::Text(s) => s,
        other => panic!("expected Output::Text, got {other:?}"),
    }
}

fn path_str(out: Output) -> String {
    match out {
        Output::Path(p) => p.to_str().expect("utf8 path").to_string(),
        other => panic!("expected Output::Path, got {other:?}"),
    }
}

fn page_body(out: Output) -> (String, String) {
    match out {
        Output::Page(pg) => (pg.title, pg.body),
        other => panic!("expected Output::Page, got {other:?}"),
    }
}

fn assert_eraser_payload(v: &Value, expected_eraser: bool, expected_mode: &str) {
    assert_eq!(v.get("eraser").and_then(Value::as_bool), Some(expected_eraser), "unexpected eraser field in {v}");
    let mode = v.get("active_mode").and_then(Value::as_str).unwrap_or_else(|| panic!("missing string active_mode in {v}"));
    assert!(mode.eq_ignore_ascii_case(expected_mode), "expected active_mode ~ {expected_mode}, got {mode:?} in {v}");
    assert!(v.get("rack_index").and_then(Value::as_u64).is_some(), "missing integer rack_index in {v}");
    let tool = v.get("writing_tool").and_then(Value::as_str).unwrap_or_else(|| panic!("missing string writing_tool in {v}"));
    assert!(!tool.is_empty(), "writing_tool must not be empty in {v}");
}

fn assert_finger_drawing_payload(v: &Value, expected_enabled: bool) {
    assert_eq!(v.get("finger_drawing").and_then(Value::as_bool), Some(expected_enabled), "expected finger_drawing={expected_enabled} in {v}");
}

fn invalid_non_empty_strings(extra_long: &str) -> Vec<&str> {
    vec![
        "\u{0}",
        "..",
        "../..",
        "C:\\Windows",
        "/etc",
        "a/b/c",
        "\u{1F600}\u{e9}\u{4e2d}",
        "maybe",
        "invalid",
        "toggle\0",
        "true\nfalse",
        "-1",
        "99999999999999999999",
        extra_long,
    ]
}

fn hostile_strings(extra_long: &str) -> Vec<&str> {
    let mut all = vec!["", " "];
    all.extend(invalid_non_empty_strings(extra_long));
    all
}

// ============================================================================
// Tier 1: F1 (`ink.toggle_eraser`) Primary Behavior & Registry Metadata
// ============================================================================

#[test]
fn f1_t1_registry_contains_toggle_eraser_metadata() {
    let spec = registry().iter().find(|c| c.id == "ink.toggle_eraser").expect("ink.toggle_eraser must be registered");
    assert!(!spec.changes_tree, "ink.toggle_eraser must have changes_tree == false");
    assert!(!spec.label.trim().is_empty(), "label must not be empty");
    assert!(spec.label.to_ascii_lowercase().contains("eraser"), "label should describe eraser: {}", spec.label);
    let params_lower = spec.params.to_ascii_lowercase();
    assert!(params_lower.contains("mode"), "params must document mode: {}", spec.params);
    assert!(params_lower.contains("toggle"), "params must document toggle: {}", spec.params);
    assert!(params_lower.contains("eraser"), "params must document eraser: {}", spec.params);
    assert!(params_lower.contains("pen"), "params must document pen: {}", spec.params);
}

#[test]
fn f1_t1_commands_json_exposes_toggle_eraser() {
    let list = commands_json();
    let arr = list.as_array().expect("commands_json must return array");
    let entry = arr
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some("ink.toggle_eraser"))
        .expect("commands_json must include ink.toggle_eraser");
    assert_eq!(entry.get("changesTree").and_then(Value::as_bool), Some(false));
    assert!(!entry.get("label").and_then(Value::as_str).unwrap_or_default().is_empty());
    assert!(entry.get("params").and_then(Value::as_str).unwrap_or_default().to_ascii_lowercase().contains("mode"));
}

#[test]
fn f1_t1_toggle_eraser_default_pen_to_eraser_and_back() {
    let t = TempDir::new("e2e-f1-t1-default");
    let mut s = new_session(&t);

    // Default active tool is Pen (rack_index 0); first toggle switches to Erase.
    let first = parse_json_text(s.run("ink.toggle_eraser", &Params::new()).expect("first toggle"));
    assert_eraser_payload(&first, true, "erase");
    assert_eq!(first.get("rack_index").and_then(Value::as_u64), Some(0));
    let saved_tool = first.get("writing_tool").and_then(Value::as_str).expect("writing_tool").to_string();
    assert!(saved_tool.eq_ignore_ascii_case("pen"), "default writing_tool should be Pen, got {saved_tool:?}");

    // Second toggle restores previous writing tool and rack_index.
    let second = parse_json_text(s.run("ink.toggle_eraser", &Params::new()).expect("second toggle"));
    assert_eraser_payload(&second, false, "draw");
    assert_eq!(second.get("rack_index").and_then(Value::as_u64), Some(0));
    assert_eq!(second.get("writing_tool").and_then(Value::as_str), Some(saved_tool.as_str()));
}

#[test]
fn f1_t1_toggle_eraser_explicit_mode_eraser_and_pen() {
    let t = TempDir::new("e2e-f1-t1-explicit");
    let mut s = new_session(&t);

    let to_eraser = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "eraser")])).expect("mode=eraser"));
    assert_eraser_payload(&to_eraser, true, "erase");

    let to_pen = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "pen")])).expect("mode=pen"));
    assert_eraser_payload(&to_pen, false, "draw");
    assert_eq!(to_pen.get("rack_index").and_then(Value::as_u64), Some(0));
}

#[test]
fn f1_t1_toggle_eraser_explicit_mode_toggle_and_params_from_json() {
    let t = TempDir::new("e2e-f1-t1-json-params");
    let mut s = new_session(&t);

    let params = params_from_json(Some(&json!({"mode": "toggle"})));
    let on = parse_json_text(s.run("ink.toggle_eraser", &params).expect("mode=toggle -> eraser"));
    assert_eraser_payload(&on, true, "erase");

    let off = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "toggle")])).expect("mode=toggle -> pen"));
    assert_eraser_payload(&off, false, "draw");
}

#[test]
fn f1_t1_toggle_eraser_output_json_roundtrip_shape() {
    let t = TempDir::new("e2e-f1-t1-output-json");
    let mut s = new_session(&t);
    s.select_writing_tool(InkPenKind::FountainPen, 3);

    let out = s.run("ink.toggle_eraser", &p(&[("mode", "eraser")])).expect("mode=eraser");
    assert!(matches!(out, Output::Json(_)), "expected Output::Json variant, got {out:?}");
    let parsed = output_json(&out);
    let obj = parsed.as_object().expect("expected JSON object from output_json");
    assert_eq!(obj.len(), 4, "expected exact 4 fields in payload, got {parsed}");
    assert_eraser_payload(&parsed, true, "erase");
    assert_eq!(parsed.get("writing_tool").and_then(Value::as_str), Some("fountain-pen"));
    assert_eq!(parsed.get("rack_index").and_then(Value::as_u64), Some(3));
}

#[test]
fn f1_t1_toggle_eraser_restores_all_pen_kinds_and_rack_indices() {
    let t = TempDir::new("e2e-f1-t1-all-pens");
    let mut s = new_session(&t);

    for (idx, kind) in InkPenKind::ALL.into_iter().enumerate() {
        let rack_index = idx + 1;
        s.select_writing_tool(kind, rack_index);
        let before = s.ink_tool_state();
        assert_eq!(before.active_mode, InkActiveMode::Draw);
        assert_eq!(before.previous_writing_tool, kind);
        assert_eq!(before.rack_index, rack_index);

        let erased = parse_json_text(s.run("ink.toggle_eraser", &Params::new()).expect("toggle to eraser"));
        assert_eraser_payload(&erased, true, "erase");
        assert_eq!(erased.get("writing_tool").and_then(Value::as_str), Some(kind.id()));
        assert_eq!(erased.get("rack_index").and_then(Value::as_u64), Some(rack_index as u64));
        assert_eq!(s.ink_tool_state().active_mode, InkActiveMode::Erase);
        assert_eq!(s.ink_tool_state().previous_writing_tool, kind);
        assert_eq!(s.ink_tool_state().rack_index, rack_index);

        let restored = parse_json_text(s.run("ink.toggle_eraser", &Params::new()).expect("toggle back to writing tool"));
        assert_eraser_payload(&restored, false, "draw");
        assert_eq!(restored.get("writing_tool").and_then(Value::as_str), Some(kind.id()));
        assert_eq!(restored.get("rack_index").and_then(Value::as_u64), Some(rack_index as u64));
        assert_eq!(s.ink_tool_state(), before);

        let explicit_erase = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "eraser")])).expect("explicit eraser"));
        assert_eraser_payload(&explicit_erase, true, "erase");
        assert_eq!(explicit_erase.get("writing_tool").and_then(Value::as_str), Some(kind.id()));
        assert_eq!(explicit_erase.get("rack_index").and_then(Value::as_u64), Some(rack_index as u64));

        let explicit_pen = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "pen")])).expect("explicit pen"));
        assert_eraser_payload(&explicit_pen, false, "draw");
        assert_eq!(explicit_pen.get("writing_tool").and_then(Value::as_str), Some(kind.id()));
        assert_eq!(explicit_pen.get("rack_index").and_then(Value::as_u64), Some(rack_index as u64));
        assert_eq!(s.ink_tool_state(), before);
    }
}

#[test]
fn f1_t1_toggle_eraser_from_non_draw_modes_restores_draw_and_writing_tool() {
    let t = TempDir::new("e2e-f1-t1-non-draw-modes");
    let mut s = new_session(&t);

    let cases = [
        (InkActiveMode::Write, InkPenKind::Highlighter, 4usize),
        (InkActiveMode::Pan, InkPenKind::Pencil, 7usize),
        (InkActiveMode::Lasso, InkPenKind::BrushPen, 11usize),
    ];

    for (start_mode, kind, rack_index) in cases {
        s.sync_writing_tool_metadata(kind, rack_index);
        s.set_ink_mode(start_mode);
        let initial = s.ink_tool_state();
        assert_eq!(initial.active_mode, start_mode);
        assert_eq!(initial.previous_writing_tool, kind);
        assert_eq!(initial.rack_index, rack_index);

        let to_erase = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "toggle")])).expect("toggle from non-draw mode"));
        assert_eraser_payload(&to_erase, true, "erase");
        assert_eq!(to_erase.get("writing_tool").and_then(Value::as_str), Some(kind.id()));
        assert_eq!(to_erase.get("rack_index").and_then(Value::as_u64), Some(rack_index as u64));

        let restored = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "toggle")])).expect("toggle back from erase"));
        assert_eraser_payload(&restored, false, "draw");
        assert_eq!(restored.get("writing_tool").and_then(Value::as_str), Some(kind.id()));
        assert_eq!(restored.get("rack_index").and_then(Value::as_u64), Some(rack_index as u64));

        let final_state = s.ink_tool_state();
        assert_eq!(final_state.active_mode, InkActiveMode::Draw);
        assert_eq!(final_state.previous_writing_tool, kind);
        assert_eq!(final_state.rack_index, rack_index);
    }
}

// ============================================================================
// Tier 1: F2 (`ink.finger_drawing`) Primary Behavior & Registry Metadata
// ============================================================================

#[test]
fn f2_t1_registry_contains_finger_drawing_metadata() {
    let spec = registry().iter().find(|c| c.id == "ink.finger_drawing").expect("ink.finger_drawing must be registered");
    assert!(!spec.changes_tree, "ink.finger_drawing must have changes_tree == false");
    assert!(!spec.label.trim().is_empty(), "label must not be empty");
    assert!(spec.label.to_ascii_lowercase().contains("finger"), "label should mention finger: {}", spec.label);
    let params_lower = spec.params.to_ascii_lowercase();
    assert!(params_lower.contains("enabled"), "params must document enabled: {}", spec.params);
    assert!(params_lower.contains("true"), "params must document true: {}", spec.params);
    assert!(params_lower.contains("false"), "params must document false: {}", spec.params);
    assert!(params_lower.contains("toggle"), "params must document toggle: {}", spec.params);
}

#[test]
fn f2_t1_commands_json_exposes_finger_drawing() {
    let list = commands_json();
    let arr = list.as_array().expect("commands_json must return array");
    let entry = arr
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some("ink.finger_drawing"))
        .expect("commands_json must include ink.finger_drawing");
    assert_eq!(entry.get("changesTree").and_then(Value::as_bool), Some(false));
    assert!(!entry.get("label").and_then(Value::as_str).unwrap_or_default().is_empty());
    assert!(entry.get("params").and_then(Value::as_str).unwrap_or_default().to_ascii_lowercase().contains("enabled"));
}

#[test]
fn f2_t1_finger_drawing_default_false_toggles_true_then_false() {
    let t = TempDir::new("e2e-f2-t1-default");
    let mut s = new_session(&t);

    // Default finger_drawing is false; first omitted/toggle call enables it.
    let first = parse_json_text(s.run("ink.finger_drawing", &Params::new()).expect("first toggle"));
    assert_finger_drawing_payload(&first, true);

    let second = parse_json_text(s.run("ink.finger_drawing", &Params::new()).expect("second toggle"));
    assert_finger_drawing_payload(&second, false);
}

#[test]
fn f2_t1_finger_drawing_explicit_enabled_true_and_false() {
    let t = TempDir::new("e2e-f2-t1-explicit");
    let mut s = new_session(&t);

    let on = parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "true")])).expect("enabled=true"));
    assert_finger_drawing_payload(&on, true);

    let off = parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "false")])).expect("enabled=false"));
    assert_finger_drawing_payload(&off, false);
}

#[test]
fn f2_t1_finger_drawing_explicit_toggle_and_params_from_json_bools() {
    let t = TempDir::new("e2e-f2-t1-json-params");
    let mut s = new_session(&t);

    let on = parse_json_text(s.run("ink.finger_drawing", &params_from_json(Some(&json!({"enabled": true})))).expect("json true"));
    assert_finger_drawing_payload(&on, true);

    let off = parse_json_text(s.run("ink.finger_drawing", &params_from_json(Some(&json!({"enabled": false})))).expect("json false"));
    assert_finger_drawing_payload(&off, false);

    let toggled_on = parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "toggle")])).expect("toggle on"));
    assert_finger_drawing_payload(&toggled_on, true);

    let toggled_off = parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "toggle")])).expect("toggle off"));
    assert_finger_drawing_payload(&toggled_off, false);
}

#[test]
fn f2_t1_finger_drawing_sessions_have_isolated_tool_state() {
    let t1 = TempDir::new("e2e-f2-t1-iso-1");
    let t2 = TempDir::new("e2e-f2-t1-iso-2");
    let mut s1 = new_session(&t1);
    let mut s2 = new_session(&t2);

    let s1_on = parse_json_text(s1.run("ink.finger_drawing", &p(&[("enabled", "true")])).expect("s1 true"));
    assert_finger_drawing_payload(&s1_on, true);

    // s2 starts fresh at false, so toggling it must yield true (not false).
    let s2_toggled = parse_json_text(s2.run("ink.finger_drawing", &p(&[("enabled", "toggle")])).expect("s2 toggle"));
    assert_finger_drawing_payload(&s2_toggled, true);
}

// ============================================================================
// Tier 2: F1 (`ink.toggle_eraser`) Hostile Params, Boundaries & Isolation
// ============================================================================

#[test]
fn f1_t2_invalid_mode_values_return_bad_param_never_panic() {
    let t = TempDir::new("e2e-f1-t2-bad-mode");
    let mut s = new_session(&t);
    let long = "x".repeat(10_000);
    let mut invalid = invalid_non_empty_strings(&long);
    invalid.extend(["erase", "stylus", "highlighter", "true", "false"]);

    for val in invalid {
        let res = s.run("ink.toggle_eraser", &p(&[("mode", val)]));
        assert!(!matches!(res, Err(EngineError::Panicked(_))), "ink.toggle_eraser panicked on mode={val:?}");
        assert!(matches!(res, Err(EngineError::BadParam(_))), "expected BadParam for mode={val:?}, got {res:?}");
    }

    // Empty and whitespace optional `mode` must toggle relative to prior state.
    let mut expect_eraser = s.ink_tool_state().active_mode == InkActiveMode::Erase;
    for blank in ["", " ", "\t\n "] {
        let res = s.run("ink.toggle_eraser", &p(&[("mode", blank)]));
        assert!(res.is_ok(), "blank mode={blank:?} should toggle cleanly, got {res:?}");
        expect_eraser = !expect_eraser;
        let expected_mode = if expect_eraser { "erase" } else { "draw" };
        let v = parse_json_text(res.expect("checked ok"));
        assert_eraser_payload(&v, expect_eraser, expected_mode);
        assert_eq!(s.ink_tool_state().active_mode == InkActiveMode::Erase, expect_eraser);
    }

    // Non-string JSON payloads for `mode` must fail with BadParam and never panic.
    for payload in [json!({"mode": 123}), json!({"mode": true}), json!({"mode": null}), json!({"mode": ["eraser"]})] {
        let params = params_from_json(Some(&payload));
        let res = s.run("ink.toggle_eraser", &params);
        assert!(!matches!(res, Err(EngineError::Panicked(_))), "panicked on json payload={payload}");
        assert!(matches!(res, Err(EngineError::BadParam(_))), "expected BadParam on json payload={payload}, got {res:?}");
    }
}

#[test]
fn f1_t2_whitespace_and_case_padded_valid_modes_preserve_writing_tool() {
    let t = TempDir::new("e2e-f1-t2-padded-valid");
    let mut s = new_session(&t);
    s.select_writing_tool(InkPenKind::Highlighter, 5);

    let to_eraser = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", " ERASER ")])).expect("padded ERASER"));
    assert_eraser_payload(&to_eraser, true, "erase");
    assert_eq!(to_eraser.get("writing_tool").and_then(Value::as_str), Some("highlighter"));
    assert_eq!(to_eraser.get("rack_index").and_then(Value::as_u64), Some(5));

    let to_pen = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", " Pen ")])).expect("padded Pen"));
    assert_eraser_payload(&to_pen, false, "draw");
    assert_eq!(to_pen.get("writing_tool").and_then(Value::as_str), Some("highlighter"));
    assert_eq!(to_pen.get("rack_index").and_then(Value::as_u64), Some(5));

    let toggled_on = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "\tToGgLe\n")])).expect("padded ToGgLe"));
    assert_eraser_payload(&toggled_on, true, "erase");
    assert_eq!(s.ink_tool_state().active_mode, InkActiveMode::Erase);
    assert_eq!(s.ink_tool_state().previous_writing_tool, InkPenKind::Highlighter);
    assert_eq!(s.ink_tool_state().rack_index, 5);
}

#[test]
fn f1_t2_hostile_unknown_keys_never_panic_and_preserve_state() {
    let t = TempDir::new("e2e-f1-t2-unknown-keys");
    let mut s = new_session(&t);
    let long = "x".repeat(10_000);

    // Put session into eraser mode first with non-default tool metadata.
    s.select_writing_tool(InkPenKind::Pencil, 2);
    let initial = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "eraser")])).expect("set eraser"));
    assert_eraser_payload(&initial, true, "erase");

    let unknown_keys = ["name", "parent", "path", "body", "enabled", "unknown_key"];
    let mut expected_eraser = true;
    for val in hostile_strings(&long) {
        let mut only_unknown = Params::new();
        for key in unknown_keys {
            only_unknown.insert((*key).to_string(), val.to_string());
        }
        let r1 = s.run("ink.toggle_eraser", &only_unknown);
        assert!(r1.is_ok(), "unknown keys alone should toggle cleanly, got {r1:?} for val={val:?}");
        expected_eraser = !expected_eraser;
        let expected_mode = if expected_eraser { "erase" } else { "draw" };
        let v = parse_json_text(r1.expect("checked ok"));
        assert_eraser_payload(&v, expected_eraser, expected_mode);
        assert_eq!(v.get("writing_tool").and_then(Value::as_str), Some("pencil"));
        assert_eq!(v.get("rack_index").and_then(Value::as_u64), Some(2));
        assert_eq!(s.ink_tool_state().active_mode == InkActiveMode::Erase, expected_eraser);
    }

    assert!(expected_eraser, "even number of hostile_strings must land back on eraser");
    assert_eq!(s.ink_tool_state().active_mode, InkActiveMode::Erase);

    for val in invalid_non_empty_strings(&long) {
        let mut with_bad_mode = Params::new();
        for key in unknown_keys {
            with_bad_mode.insert((*key).to_string(), val.to_string());
        }
        with_bad_mode.insert("mode".to_string(), val.to_string());
        let r2 = s.run("ink.toggle_eraser", &with_bad_mode);
        assert!(!matches!(r2, Err(EngineError::Panicked(_))), "panicked with bad mode + unknown keys={val:?}");
        assert!(matches!(r2, Err(EngineError::BadParam(_))), "expected BadParam for mode={val:?}, got {r2:?}");
    }

    // Failed calls must not have corrupted eraser state or saved tool metadata: toggling now returns to Pencil at rack 2.
    let back = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "toggle")])).expect("toggle back"));
    assert_eraser_payload(&back, false, "draw");
    assert_eq!(back.get("writing_tool").and_then(Value::as_str), Some("pencil"));
    assert_eq!(back.get("rack_index").and_then(Value::as_u64), Some(2));
}

#[test]
fn f1_t2_explicit_mode_eraser_and_pen_are_idempotent() {
    let t = TempDir::new("e2e-f1-t2-idempotent");
    let mut s = new_session(&t);

    for _ in 0..4 {
        let out = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "eraser")])).expect("idempotent eraser"));
        assert_eraser_payload(&out, true, "erase");
        assert_eq!(out.get("rack_index").and_then(Value::as_u64), Some(0));
    }
    for _ in 0..4 {
        let out = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "pen")])).expect("idempotent pen"));
        assert_eraser_payload(&out, false, "draw");
        assert_eq!(out.get("rack_index").and_then(Value::as_u64), Some(0));
    }
}

#[test]
fn f1_t2_rapid_repeated_toggles_alternate_cleanly() {
    let t = TempDir::new("e2e-f1-t2-rapid");
    let mut s = new_session(&t);

    for step in 1..=200 {
        let out = parse_json_text(s.run("ink.toggle_eraser", &Params::new()).expect("rapid toggle"));
        let expect_eraser = step % 2 == 1;
        let expect_mode = if expect_eraser { "erase" } else { "draw" };
        assert_eraser_payload(&out, expect_eraser, expect_mode);
        assert_eq!(out.get("rack_index").and_then(Value::as_u64), Some(0));
    }
}

#[test]
fn f1_t2_toggle_eraser_does_not_write_outside_notebooks_root() {
    let t = TempDir::new("e2e-f1-t2-fs-isolation");
    let mut s = new_session(&t);
    s.run("notebook.create", &p(&[("name", "Work")])).expect("notebook");
    let outside = t.path().join("outside.txt");
    fs::write(&outside, "keep").expect("write sentinel");
    let tree_before = text_of(s.run("tree.read", &Params::new()).expect("tree before"));

    for mode in ["eraser", "pen", "toggle", "toggle"] {
        s.run("ink.toggle_eraser", &p(&[("mode", mode)])).expect("valid toggle");
    }
    for bad in ["..", "/etc", "C:\\Windows", "\u{0}"] {
        let _ = s.run("ink.toggle_eraser", &p(&[("mode", bad)]));
    }

    assert_eq!(fs::read_to_string(&outside).expect("read sentinel"), "keep");
    assert_eq!(text_of(s.run("tree.read", &Params::new()).expect("tree after")), tree_before);
}

// ============================================================================
// Tier 2: F2 (`ink.finger_drawing`) Hostile Params, Boundaries & Isolation
// ============================================================================

#[test]
fn f2_t2_invalid_enabled_values_return_bad_param_never_panic() {
    let t = TempDir::new("e2e-f2-t2-bad-enabled");
    let mut s = new_session(&t);
    let long = "x".repeat(10_000);
    let mut invalid = invalid_non_empty_strings(&long);
    invalid.extend(["yes", "no", "1", "0", "on", "off", "eraser"]);

    for val in invalid {
        let res = s.run("ink.finger_drawing", &p(&[("enabled", val)]));
        assert!(!matches!(res, Err(EngineError::Panicked(_))), "ink.finger_drawing panicked on enabled={val:?}");
        assert!(matches!(res, Err(EngineError::BadParam(_))), "expected BadParam for enabled={val:?}, got {res:?}");
    }

    // Empty and whitespace optional `enabled` must toggle relative to prior state.
    let mut expect_enabled = s.finger_drawing();
    for blank in ["", " ", "\t\n "] {
        let res = s.run("ink.finger_drawing", &p(&[("enabled", blank)]));
        assert!(res.is_ok(), "blank enabled={blank:?} should toggle cleanly, got {res:?}");
        expect_enabled = !expect_enabled;
        let v = parse_json_text(res.expect("checked ok"));
        assert_finger_drawing_payload(&v, expect_enabled);
        assert_eq!(s.finger_drawing(), expect_enabled);
    }

    // Non-string JSON payloads for `enabled` must fail with BadParam and never panic.
    for payload in [json!({"enabled": 1}), json!({"enabled": 0}), json!({"enabled": null}), json!({"enabled": ["true"]})] {
        let params = params_from_json(Some(&payload));
        let res = s.run("ink.finger_drawing", &params);
        assert!(!matches!(res, Err(EngineError::Panicked(_))), "panicked on json payload={payload}");
        assert!(matches!(res, Err(EngineError::BadParam(_))), "expected BadParam on json payload={payload}, got {res:?}");
        assert_eq!(s.finger_drawing(), expect_enabled);
    }
}

#[test]
fn f2_t2_whitespace_and_case_padded_valid_enabled_values() {
    let t = TempDir::new("e2e-f2-t2-padded-valid");
    let mut s = new_session(&t);

    let on = parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", " TRUE ")])).expect("padded TRUE"));
    assert_finger_drawing_payload(&on, true);
    assert!(s.finger_drawing());

    let off = parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", " False ")])).expect("padded False"));
    assert_finger_drawing_payload(&off, false);
    assert!(!s.finger_drawing());

    let toggled = parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "\tToGgLe\n")])).expect("padded ToGgLe"));
    assert_finger_drawing_payload(&toggled, true);
    assert!(s.finger_drawing());
}

#[test]
fn f2_t2_hostile_unknown_keys_never_panic_and_preserve_state() {
    let t = TempDir::new("e2e-f2-t2-unknown-keys");
    let mut s = new_session(&t);
    let long = "x".repeat(10_000);

    let on = parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "true")])).expect("set true"));
    assert_finger_drawing_payload(&on, true);

    let unknown_keys = ["name", "parent", "path", "body", "mode", "unknown_key"];
    let mut expected_fd = true;
    for val in hostile_strings(&long) {
        let mut only_unknown = Params::new();
        for key in unknown_keys {
            only_unknown.insert((*key).to_string(), val.to_string());
        }
        let r1 = s.run("ink.finger_drawing", &only_unknown);
        assert!(r1.is_ok(), "unknown keys alone should toggle cleanly, got {r1:?} for val={val:?}");
        expected_fd = !expected_fd;
        let v = parse_json_text(r1.expect("checked ok"));
        assert_finger_drawing_payload(&v, expected_fd);
        assert_eq!(s.finger_drawing(), expected_fd);
    }

    assert!(expected_fd, "even number of hostile_strings must land back on true");
    assert!(s.finger_drawing());

    for val in invalid_non_empty_strings(&long) {
        let mut with_bad_enabled = Params::new();
        for key in unknown_keys {
            with_bad_enabled.insert((*key).to_string(), val.to_string());
        }
        with_bad_enabled.insert("enabled".to_string(), val.to_string());
        let r2 = s.run("ink.finger_drawing", &with_bad_enabled);
        assert!(!matches!(r2, Err(EngineError::Panicked(_))), "panicked with bad enabled + unknown keys={val:?}");
        assert!(matches!(r2, Err(EngineError::BadParam(_))), "expected BadParam for enabled={val:?}, got {r2:?}");
    }

    // State remained true across invalid inputs; toggling now produces false.
    let flipped = parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "toggle")])).expect("toggle off"));
    assert_finger_drawing_payload(&flipped, false);
    assert!(!s.finger_drawing());
}

#[test]
fn f2_t2_explicit_enabled_true_and_false_are_idempotent() {
    let t = TempDir::new("e2e-f2-t2-idempotent");
    let mut s = new_session(&t);

    for _ in 0..4 {
        let out = parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "true")])).expect("idempotent true"));
        assert_finger_drawing_payload(&out, true);
    }
    for _ in 0..4 {
        let out = parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "false")])).expect("idempotent false"));
        assert_finger_drawing_payload(&out, false);
    }
}

#[test]
fn f2_t2_rapid_repeated_toggles_alternate_cleanly() {
    let t = TempDir::new("e2e-f2-t2-rapid");
    let mut s = new_session(&t);

    for step in 1..=200 {
        let out = parse_json_text(s.run("ink.finger_drawing", &Params::new()).expect("rapid toggle"));
        assert_finger_drawing_payload(&out, step % 2 == 1);
    }
}

#[test]
fn f2_t2_finger_drawing_does_not_write_outside_notebooks_root() {
    let t = TempDir::new("e2e-f2-t2-fs-isolation");
    let mut s = new_session(&t);
    s.run("notebook.create", &p(&[("name", "Work")])).expect("notebook");
    let outside = t.path().join("outside.txt");
    fs::write(&outside, "keep").expect("write sentinel");
    let tree_before = text_of(s.run("tree.read", &Params::new()).expect("tree before"));

    for enabled in ["true", "false", "toggle", "toggle"] {
        s.run("ink.finger_drawing", &p(&[("enabled", enabled)])).expect("valid finger_drawing");
    }
    for bad in ["..", "/etc", "C:\\Windows", "\u{0}"] {
        let _ = s.run("ink.finger_drawing", &p(&[("enabled", bad)]));
    }

    assert_eq!(fs::read_to_string(&outside).expect("read sentinel"), "keep");
    assert_eq!(text_of(s.run("tree.read", &Params::new()).expect("tree after")), tree_before);
}

// ============================================================================
// Tier 3: Cross-Feature Combinations (F1 + F2 + Notebook/Node/Page Commands)
// ============================================================================

#[test]
fn tier3_eraser_and_finger_drawing_states_are_orthogonal() {
    let t = TempDir::new("e2e-t3-orthogonal");
    let mut s = new_session(&t);

    // (eraser=false, finger_drawing=true)
    assert_finger_drawing_payload(&parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "true")])).expect("fd on")), true);
    assert_eraser_payload(&parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "pen")])).expect("pen")), false, "draw");

    // (eraser=true, finger_drawing=true): toggling eraser does not turn off finger_drawing.
    assert_eraser_payload(&parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "toggle")])).expect("eraser on")), true, "erase");
    assert_finger_drawing_payload(&parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "toggle")])).expect("fd off")), false);

    // (eraser=true, finger_drawing=false): toggling finger_drawing did not revert eraser.
    assert_eraser_payload(&parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "toggle")])).expect("eraser off")), false, "draw");
}

#[test]
fn tier3_tool_commands_do_not_disturb_notebook_tree_or_saved_pages() {
    let t = TempDir::new("e2e-t3-tree-page-intact");
    let mut s = new_session(&t);
    let nb = path_str(s.run("notebook.create", &p(&[("name", "Study"), ("format", "cmgfm")])).expect("notebook"));
    let sec = path_str(s.run("node.create", &p(&[("parent", &nb), ("kind", "section"), ("title", "Math")])).expect("section"));
    let page = path_str(s.run("node.create", &p(&[("parent", &sec), ("kind", "page"), ("title", "Algebra")])).expect("page"));
    let body = "- [ ] Solve problem set\n\nQuadratic formula notes.\n";
    s.run("page.save", &p(&[("path", &page), ("body", body)])).expect("save page");

    let tree_before = text_of(s.run("tree.read", &Params::new()).expect("tree before"));
    let tree_len_before = s.tree().len();

    s.run("ink.toggle_eraser", &p(&[("mode", "eraser")])).expect("eraser");
    s.run("ink.finger_drawing", &p(&[("enabled", "true")])).expect("fd true");
    s.run("ink.toggle_eraser", &p(&[("mode", "toggle")])).expect("toggle back");
    s.run("ink.finger_drawing", &p(&[("enabled", "toggle")])).expect("fd toggle");

    assert_eq!(s.tree().len(), tree_len_before);
    assert_eq!(text_of(s.run("tree.read", &Params::new()).expect("tree after")), tree_before);
    let (loaded_title, loaded_body) = page_body(s.run("page.load", &p(&[("path", &page)])).expect("load page"));
    assert_eq!(loaded_title, "Algebra");
    assert_eq!(loaded_body, body);
}

#[test]
fn tier3_tree_mutating_commands_preserve_session_tool_state() {
    let t = TempDir::new("e2e-t3-state-across-tree-ops");
    let mut s = new_session(&t);

    s.select_writing_tool(InkPenKind::Highlighter, 6);
    assert_eraser_payload(&parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "eraser")])).expect("eraser")), true, "erase");
    assert_finger_drawing_payload(&parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "true")])).expect("fd true")), true);

    // Execute tree-changing and page commands (triggers Session::refresh).
    let nb = path_str(s.run("notebook.create", &p(&[("name", "Notes")])).expect("nb"));
    let sec = path_str(s.run("node.create", &p(&[("parent", &nb), ("kind", "section"), ("title", "Daily")])).expect("sec"));
    let p1 = path_str(s.run("node.create", &p(&[("parent", &sec), ("kind", "page"), ("title", "Day 1"), ("format", "cmgfm")])).expect("p1"));
    s.run("page.save", &p(&[("path", &p1), ("body", "Entry 1\n")])).expect("save");
    s.run("page.load", &p(&[("path", &p1)])).expect("load");
    s.run("tree.refresh", &Params::new()).expect("refresh");

    // Both tool flags and remembered writing tool metadata must still be active, so toggling restores Highlighter@6.
    let restored = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "toggle")])).expect("toggle eraser off"));
    assert_eraser_payload(&restored, false, "draw");
    assert_eq!(restored.get("writing_tool").and_then(Value::as_str), Some("highlighter"));
    assert_eq!(restored.get("rack_index").and_then(Value::as_u64), Some(6));
    assert_finger_drawing_payload(&parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "toggle")])).expect("toggle fd off")), false);
}

#[test]
fn tier3_invalid_tool_commands_mid_session_preserve_tool_and_page_state() {
    let t = TempDir::new("e2e-t3-errors-preserve-all");
    let mut s = new_session(&t);
    let nb = path_str(s.run("notebook.create", &p(&[("name", "Work"), ("format", "cmgfm")])).expect("nb"));
    let page = std::path::Path::new(&nb).join("new-section-1").join("untitled-page");
    let page = page.to_str().expect("utf8 page path").to_string();
    s.run("page.save", &p(&[("path", &page), ("body", "Keep me safe\n")])).expect("save");

    assert_finger_drawing_payload(&parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "true")])).expect("fd true")), true);
    assert_eraser_payload(&parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "pen")])).expect("pen")), false, "draw");

    assert!(matches!(s.run("ink.toggle_eraser", &p(&[("mode", "bogus")])), Err(EngineError::BadParam(_))));
    assert!(matches!(s.run("ink.finger_drawing", &p(&[("enabled", "bogus")])), Err(EngineError::BadParam(_))));

    let (_, loaded_body) = page_body(s.run("page.load", &p(&[("path", &page)])).expect("load"));
    assert_eq!(loaded_body, "Keep me safe\n");
    assert_eraser_payload(&parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "toggle")])).expect("toggle eraser on")), true, "erase");
    assert_finger_drawing_payload(&parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "toggle")])).expect("toggle fd off")), false);
}

// ============================================================================
// Tier 4: Multi-Step Realistic Application Scenarios
// ============================================================================

#[test]
fn tier4_multi_page_note_session_with_tool_switching_and_reload() {
    let t = TempDir::new("e2e-t4-multi-page");
    let mut s = new_session(&t);

    let nb = path_str(s.run("notebook.create", &p(&[("name", "Coursework"), ("empty", "true")])).expect("create notebook"));
    let lectures =
        path_str(s.run("node.create", &p(&[("parent", &nb), ("kind", "section"), ("title", "Lectures"), ("format", "cmgfm")])).expect("lectures"));
    let diagrams =
        path_str(s.run("node.create", &p(&[("parent", &nb), ("kind", "section"), ("title", "Diagrams"), ("format", "html")])).expect("diagrams"));

    let lec1 = path_str(s.run("node.create", &p(&[("parent", &lectures), ("kind", "page"), ("title", "Lecture 1")])).expect("lec1"));
    let initial_md = "# Lecture 1\n\n- [ ] Review notes\n";
    s.run("page.save", &p(&[("path", &lec1), ("body", initial_md)])).expect("save lec1 initial");

    // Touch-only sketching with non-default Highlighter: enable finger drawing, erase a mistake, restore highlighter, update notes.
    s.select_writing_tool(InkPenKind::Highlighter, 3);
    assert_finger_drawing_payload(&parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "true")])).expect("enable fd")), true);
    assert_eraser_payload(&parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "eraser")])).expect("enable eraser")), true, "erase");
    let restored_hl = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "pen")])).expect("restore highlighter"));
    assert_eraser_payload(&restored_hl, false, "draw");
    assert_eq!(restored_hl.get("writing_tool").and_then(Value::as_str), Some("highlighter"));
    assert_eq!(restored_hl.get("rack_index").and_then(Value::as_u64), Some(3));

    let updated_md = "# Lecture 1\n\n- [x] Review notes\n\nAdded sketch annotations.\n";
    s.run("page.save", &p(&[("path", &lec1), ("body", updated_md)])).expect("save lec1 updated");

    // Switch to HTML diagrams page with BrushPen, turn off finger drawing for scrolling, double-tap eraser toggle.
    let sketch = path_str(s.run("node.create", &p(&[("parent", &diagrams), ("kind", "page"), ("title", "Sketches")])).expect("sketch page"));
    s.select_writing_tool(InkPenKind::BrushPen, 8);
    assert_finger_drawing_payload(&parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "false")])).expect("disable fd")), false);
    assert_eraser_payload(&parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "toggle")])).expect("double-tap to eraser")), true, "erase");
    let restored_brush = parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "toggle")])).expect("double-tap to brush pen"));
    assert_eraser_payload(&restored_brush, false, "draw");
    assert_eq!(restored_brush.get("writing_tool").and_then(Value::as_str), Some("brush-pen"));
    assert_eq!(restored_brush.get("rack_index").and_then(Value::as_u64), Some(8));

    let sketch_html = "<p>Diagram notes</p>";
    s.run("page.save", &p(&[("path", &sketch), ("body", sketch_html)])).expect("save sketch");

    // Search & reload both pages to verify full persistence.
    match s.run("search.query", &p(&[("q", "annotations")])).expect("search") {
        Output::Hits(hits) => assert_eq!(hits.len(), 1, "expected 1 search hit, got {hits:?}"),
        other => panic!("expected Output::Hits, got {other:?}"),
    }

    let (t1_title, t1_body) = page_body(s.run("page.load", &p(&[("path", &lec1)])).expect("reload lec1"));
    assert_eq!(t1_title, "Lecture 1");
    assert_eq!(t1_body, updated_md);

    let (t2_title, t2_body) = page_body(s.run("page.load", &p(&[("path", &sketch)])).expect("reload sketch"));
    assert_eq!(t2_title, "Sketches");
    assert_eq!(t2_body, sketch_html);

    match s.run("notebook.check", &p(&[("path", &nb)])).expect("check notebook") {
        Output::Report(report) => assert!(report.problems.is_empty(), "unexpected notebook problems: {:?}", report.problems),
        other => panic!("expected Output::Report, got {other:?}"),
    }
}

#[test]
fn tier4_subpage_recycle_and_tool_state_continuity() {
    let t = TempDir::new("e2e-t4-subpage-recycle");
    let mut s = new_session(&t);

    let nb = path_str(s.run("notebook.create", &p(&[("name", "Journal"), ("format", "cmgfm")])).expect("nb"));
    let sec = std::path::Path::new(&nb).join("new-section-1");
    let sec = sec.to_str().expect("utf8 section path").to_string();
    let week = path_str(s.run("node.create", &p(&[("parent", &sec), ("kind", "page"), ("title", "Week 1")])).expect("week"));
    let draft = path_str(s.run("node.create", &p(&[("parent", &sec), ("kind", "page"), ("title", "Draft")])).expect("draft"));
    let draft = path_str(s.run("node.demote", &p(&[("path", &draft)])).expect("demote draft"));

    s.run("page.save", &p(&[("path", &week), ("body", "Weekly summary\n")])).expect("save week");
    s.run("page.save", &p(&[("path", &draft), ("body", "Scratch pad\n")])).expect("save draft");

    // Set tool state before deleting and restoring subpage.
    assert_finger_drawing_payload(&parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "true")])).expect("fd on")), true);
    assert_eraser_payload(&parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "eraser")])).expect("eraser on")), true, "erase");

    s.run("node.delete", &p(&[("path", &draft)])).expect("delete draft");
    let recycled_json = output_json(&s.run("recycle.list", &p(&[("notebook", &nb)])).expect("recycle.list"));
    let recycled_id = recycled_json
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .expect("recycled item id")
        .to_string();

    s.run("recycle.restore", &p(&[("notebook", &nb), ("id", &recycled_id)])).expect("restore draft");

    // Tool state remains active after delete + restore; toggling switches cleanly back.
    assert_eraser_payload(&parse_json_text(s.run("ink.toggle_eraser", &p(&[("mode", "toggle")])).expect("eraser off")), false, "draw");
    assert_finger_drawing_payload(&parse_json_text(s.run("ink.finger_drawing", &p(&[("enabled", "toggle")])).expect("fd off")), false);

    let (_, week_body) = page_body(s.run("page.load", &p(&[("path", &week)])).expect("reload week"));
    assert_eq!(week_body, "Weekly summary\n");
    let (_, draft_body) = page_body(s.run("page.load", &p(&[("path", &draft)])).expect("reload restored draft"));
    assert_eq!(draft_body, "Scratch pad\n");
}
