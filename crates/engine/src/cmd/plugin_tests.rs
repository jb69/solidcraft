//! Plugin features through two test plugins: a block (a choice, fields shown under conditions, a
//! published value) and a follower that sizes itself from what the block published.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_plugin::{Display, FallbackPolicy, Field, FieldKind, Host, Manifest, Plugin, Schema, Unit};

use crate::Session;

fn number(key: &str, label: &str, min: f64, default: f64, max: f64, unit: Unit, when: &str) -> Field {
    Field {
        key: key.into(),
        label: label.into(),
        group: "Size".into(),
        collapsed: false,
        kind: FieldKind::Number { min, default, max, unit, integer: false },
        when: when.into(),
        help: String::new(),
        unused: false,
    }
}

struct Block;

impl Plugin for Block {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "test.block".into(),
            name: "Test Block".into(),
            version: "1.0.0".into(),
            api_version: solidcraft_plugin::API_VERSION,
            description: "a box or a cylinder".into(),
            display: Display { label: "Block".into(), icon_id: "box".into(), tooltip: "a test block".into(), type_name: "BlockFeature".into() },
            fallback: FallbackPolicy::RecordedMesh,
            preview: true,
        }
    }
    fn schema(&self) -> Schema {
        Schema {
            fields: vec![
                Field {
                    key: "shape".into(),
                    label: "Shape".into(),
                    group: String::new(),
                    collapsed: false,
                    kind: FieldKind::Choice {
                        options: vec![("BOX".into(), "Box".into()), ("CYLINDER".into(), "Cylinder".into())],
                        default: "BOX".into(),
                    },
                    when: String::new(),
                    help: "what it is".into(),
                    unused: false,
                },
                number("length", "Length", 1.0, 20.0, 200.0, Unit::Length, "shape == BOX"),
                number("width", "Width", 1.0, 10.0, 200.0, Unit::Length, "shape == BOX"),
                number("diameter", "Diameter", 1.0, 10.0, 200.0, Unit::Length, "shape == CYLINDER"),
                number("height", "Height", 1.0, 5.0, 200.0, Unit::Length, ""),
                Field {
                    key: "panic".into(),
                    label: "Panic".into(),
                    group: "Faults".into(),
                    collapsed: true,
                    kind: FieldKind::Bool { default: false },
                    when: String::new(),
                    help: String::new(),
                    unused: false,
                },
                Field {
                    key: "escape".into(),
                    label: "Write a file".into(),
                    group: "Faults".into(),
                    collapsed: true,
                    kind: FieldKind::Bool { default: false },
                    when: String::new(),
                    help: String::new(),
                    unused: false,
                },
            ],
        }
    }
    fn evaluate(&self, p: &Value, _ctx: &Value, host: &mut dyn Host) -> Result<Value, String> {
        let n = |k: &str| p[k].as_f64().unwrap_or(0.0);
        if p["panic"] == json!(true) {
            panic!("the test plugin panics on purpose");
        }
        if p["escape"] == json!(true) {
            solidcraft_plugin::run(host, "file.save_as", json!({"path": "escaped.solidcraft"}))?;
        }
        let volume = if p["shape"] == json!("CYLINDER") {
            solidcraft_plugin::run(host, "solid.cylinder", json!({"radius": n("diameter") / 2.0, "height": n("height")}))?;
            std::f64::consts::PI * n("diameter") * n("diameter") / 4.0 * n("height")
        } else {
            solidcraft_plugin::run(
                host,
                "solid.box",
                json!({"length": n("length"), "width": n("width"), "height": n("height"), "corner": [0, 0, 0]}),
            )?;
            n("length") * n("width") * n("height")
        };
        let mut issues = Vec::new();
        if n("height") > 50.0 {
            let tall = solidcraft_plugin::Issue::new(solidcraft_plugin::Severity::Warning, "tall_block", "taller than 50 mm")
                .on("height", n("height"))
                .limit(50.0)
                .suggest("height", 50.0);
            issues.push(tall);
            issues.push(solidcraft_plugin::Issue::new(solidcraft_plugin::Severity::Info, "tall_noted", "noted as tall"));
        }
        Ok(json!({"notice": format!("volume {volume:.1} mm³"), "values": {"volume_mm3": volume}, "issues": issues,
            "publish": {"height": n("height")}}))
    }
    fn describe(&self, p: &Value, _ctx: &Value) -> Result<Value, String> {
        Ok(json!({"notice": format!("a {} {} mm high", p["shape"].as_str().unwrap_or("?").to_lowercase(), p["height"])}))
    }
}

/// A cube as tall as the last block published.
struct Follower;

impl Plugin for Follower {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "test.follower".into(),
            name: "Test Follower".into(),
            version: "1.0.0".into(),
            api_version: solidcraft_plugin::API_VERSION,
            description: String::new(),
            display: Display { label: "Follower".into(), ..Default::default() },
            fallback: FallbackPolicy::SuppressWithWarning,
            preview: true,
        }
    }
    fn schema(&self) -> Schema {
        Schema { fields: vec![number("x", "X", -500.0, 100.0, 500.0, Unit::Length, "")] }
    }
    fn evaluate(&self, p: &Value, ctx: &Value, host: &mut dyn Host) -> Result<Value, String> {
        let h = ctx["published"]["test.block"]["height"].as_f64().ok_or("no block above")?;
        solidcraft_plugin::run(host, "solid.box", json!({"length": h, "width": h, "height": h, "corner": [p["x"], 0, 0]}))?;
        Ok(json!({}))
    }
}

fn install() {
    // The registry is the process's: the first test to get here registers them.
    let _ = solidcraft_plugin::register(Arc::new(Block));
    let _ = solidcraft_plugin::register(Arc::new(Follower));
}

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn volume(s: &Session, body: &str) -> f64 {
    solidcraft_kernel::measure(&s.model.state().body(body).unwrap_or_else(|| panic!("no body {body}")).body).unwrap().volume
}

#[test]
fn a_plugin_feature_builds_follows_parameters_and_changes() {
    install();
    let mut s = Session::default();
    let list = run(&mut s, "plugin.list", json!({}));
    assert!(list["plugins"].as_array().unwrap().iter().any(|p| p["id"] == "test.block"), "{list}");
    run(&mut s, "parameters.add", json!({"name": "len", "expression": "30 mm"}));
    let r = run(&mut s, "plugin.add", json!({"plugin": "test.block", "values": {"length": "len", "width": 10, "height": 4}, "body_name": "B"}));
    assert_eq!(r["name"], "Block1");
    assert_eq!(r["notice"], "a box 4.0 mm high");
    assert!((volume(&s, "B") - 1200.0).abs() < 1e-6);
    // Its numbers are inputs with names, and a parameter drives them.
    assert_eq!(s.doc.features[0].kind.inputs().len(), 4);
    run(&mut s, "parameters.change", json!({"name": "len", "expression": "50 mm"}));
    assert!((volume(&s, "B") - 2000.0).abs() < 1e-6);
    // A choice changes what the dialog shows and what is built.
    run(&mut s, "plugin.set", json!({"feature": "Block1", "values": {"shape": "Cylinder", "diameter": 10}}));
    let v = volume(&s, "B");
    assert!((v / (std::f64::consts::PI * 25.0 * 4.0) - 1.0).abs() < 1e-3, "{v}");
    let f = run(&mut s, "plugin.fields", json!({"feature": "Block1"}));
    let shown = |k: &str| f["fields"].as_array().unwrap().iter().find(|x| x["key"] == k).unwrap()["shown"].clone();
    assert_eq!((shown("diameter"), shown("length")), (json!(true), json!(false)), "{f}");
    // Values are checked against the plugin's bounds.
    let e = s.execute("plugin.set", &json!({"feature": "Block1", "values": {"height": 500}})).unwrap_err().to_string();
    assert!(e.contains("Height must be within 1…200 mm"), "{e}");
    let e = s.execute("plugin.add", &json!({"plugin": "test.nothing"})).unwrap_err().to_string();
    assert!(e.contains("no plugin `test.nothing` is installed"), "{e}");
    // timeline.edit reaches the parameters too.
    run(&mut s, "timeline.edit", json!({"feature": "Block1", "set": {"params": {"values": {"height": "8"}}}}));
    assert!((volume(&s, "B") / (std::f64::consts::PI * 25.0 * 8.0) - 1.0).abs() < 1e-3);
}

#[test]
fn a_plugin_reads_what_an_earlier_plugin_feature_published() {
    install();
    let mut s = Session::default();
    run(&mut s, "plugin.add", json!({"plugin": "test.block", "values": {"height": 6}, "body_name": "B"}));
    run(&mut s, "plugin.add", json!({"plugin": "test.follower", "body_name": "F"}));
    assert!((volume(&s, "F") - 216.0).abs() < 1e-6);
    run(&mut s, "plugin.set", json!({"feature": "Block1", "values": {"height": 7}}));
    assert!((volume(&s, "F") - 343.0).abs() < 1e-6, "the follower follows");
}

#[test]
fn a_plugin_cannot_reach_outside_its_scratch_design_or_take_the_app_down() {
    install();
    let mut s = Session::default();
    let e = s.execute("plugin.add", &json!({"plugin": "test.block", "values": {"escape": true}})).unwrap_err().to_string();
    assert!(e.contains("a plugin cannot run `file.save_as`"), "{e}");
    assert!(!std::path::Path::new("escaped.solidcraft").exists());
    let e = s.execute("plugin.add", &json!({"plugin": "test.block", "values": {"panic": true}})).unwrap_err().to_string();
    assert!(e.contains("panicked"), "{e}");
    // The design is as it was.
    assert!(s.doc.features.is_empty());
}

#[test]
fn a_plugin_feature_round_trips_and_opens_without_its_plugin() {
    install();
    let mut s = Session::default();
    run(&mut s, "plugin.add", json!({"plugin": "test.block", "values": {"length": 12, "width": 5, "height": 2}, "body_name": "B"}));
    let dir = std::env::temp_dir().join(format!("plugin_rt_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("block.json");
    run(&mut s, "file.save_as", json!({"path": path.to_str().unwrap()}));
    let mut t = Session::default();
    run(&mut t, "doc.open", json!({"path": path.to_str().unwrap()}));
    assert_eq!(s.doc.features[0].kind, t.doc.features[0].kind);
    assert!((volume(&t, "B") - 120.0).abs() < 1e-6);
    // The saved design names a plugin this app does not have: each policy.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"plugin_results\""), "the result is recorded");
    let gone = text.replace("\"test.block\"", "\"test.gone\"");
    for (policy, body) in [("recorded_mesh", true), ("suppress_with_warning", false), ("error_on_rebuild", false)] {
        let p = dir.join(format!("{policy}.json"));
        std::fs::write(&p, gone.replace("\"fallback\": \"recorded_mesh\"", &format!("\"fallback\": \"{policy}\""))).unwrap();
        let mut u = Session::default();
        run(&mut u, "doc.open", json!({"path": p.to_str().unwrap()}));
        let r = u.model.results[0].clone();
        match policy {
            "recorded_mesh" => {
                assert!(r.warning.as_deref().unwrap_or_default().contains("its last result is shown as a mesh"), "{r:?}");
                // The recorded mesh: the box's volume to its triangles.
                assert!((volume(&u, "B") - 120.0).abs() < 1e-3);
            }
            "suppress_with_warning" => assert!(r.warning.as_deref().unwrap_or_default().contains("left out"), "{r:?}"),
            _ => assert!(r.error.as_deref().unwrap_or_default().contains("not installed"), "{r:?}"),
        }
        assert_eq!(u.model.state().body("B").is_some(), body, "{policy}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// A plugin's coded issues and values reach the command's reply and stay with the feature's
/// evaluation; its warnings and errors also show as the feature's warning, its notes do not.
#[test]
fn a_plugin_feature_reports_its_values_and_coded_issues() {
    install();
    let mut s = Session::default();
    let r = run(&mut s, "plugin.add", json!({"plugin": "test.block", "values": {"length": 10, "width": 10, "height": 60}}));
    let id = r["feature"].as_u64().unwrap();
    assert_eq!(r["values"]["volume_mm3"], json!(6000.0), "{r}");
    assert_eq!(
        r["issues"][0],
        json!({"severity": "warning", "code": "tall_block", "message": "taller than 50 mm", "parameter": "height", "value": 60.0,
            "limit": 50.0, "suggestion": {"height": 50.0}}),
        "{r}"
    );
    assert_eq!(r["issues"][1]["severity"], "info", "{r}");
    let warning = s.model.result(id).and_then(|f| f.warning.clone()).unwrap_or_default();
    assert!(warning.contains("taller than 50 mm") && !warning.contains("noted as tall"), "{warning}");
    let out = s.model.result(id).and_then(|f| f.state.plugin_output(id)).unwrap();
    assert_eq!(out["values"]["volume_mm3"], json!(6000.0));
    // The suggestion clears it.
    let r = run(&mut s, "plugin.set", json!({"feature": id, "values": {"height": 50}}));
    assert_eq!(r["issues"], json!([]), "{r}");
    assert!(s.model.result(id).and_then(|f| f.warning.clone()).is_none());
}

/// A design that opens from its saved model keeps what its plugin features published and said:
/// changing only the follower afterwards still finds the block's height, and the block's values
/// are there without rebuilding it.
#[test]
fn a_saved_model_keeps_what_plugin_features_published() {
    install();
    let mut s = Session::default();
    let block = run(&mut s, "plugin.add", json!({"plugin": "test.block", "values": {"height": 6}, "body_name": "B"}))["feature"].as_u64().unwrap();
    run(&mut s, "plugin.add", json!({"plugin": "test.follower", "body_name": "F"}));
    let dir = std::env::temp_dir().join(format!("plugin_saved_model_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("saved.solidcraft");
    run(&mut s, "file.save_as", json!({"path": path.to_str().unwrap()}));
    let mut t = Session::default();
    run(&mut t, "doc.open", json!({"path": path.to_str().unwrap()}));
    assert_eq!(t.model.last_recomputed, 0, "opened from its saved model, nothing rebuilt");
    let said = |s: &Session| s.model.result(block).and_then(|r| r.state.plugin_output(block)).map(|o| o["values"]["volume_mm3"].clone());
    assert!(said(&s).is_some_and(|v| v.is_number()));
    assert_eq!(said(&t), said(&s), "the block's values came with the saved model");
    run(&mut t, "plugin.set", json!({"feature": "Follower1", "values": {"x": 20}}));
    assert!((volume(&t, "F") - 216.0).abs() < 1e-6, "the follower still finds the block's height");
    std::fs::remove_dir_all(&dir).ok();
}
