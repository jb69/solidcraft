//! A plugin feature's dialog, drawn from the plugin's schema: a field shown under its condition,
//! values typed and applied, the feature edited again.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_plugin::{Display, FallbackPolicy, Field, FieldKind, Host, Manifest, Plugin, Schema, Unit};

use crate::dialogs::Kind;
use crate::scenario::Harness;

struct Slab;

fn number(key: &str, label: &str, default: f64, when: &str) -> Field {
    Field {
        key: key.into(),
        label: label.into(),
        group: "Size".into(),
        collapsed: false,
        kind: FieldKind::Number { min: 1.0, default, max: 500.0, unit: Unit::Length, integer: false },
        when: when.into(),
        help: format!("the slab's {label}"),
        unused: false,
    }
}

impl Plugin for Slab {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "uitest.slab".into(),
            name: "Test Slab".into(),
            version: "1".into(),
            api_version: solidcraft_plugin::API_VERSION,
            description: String::new(),
            display: Display { label: "Slab".into(), icon_id: "box".into(), ..Default::default() },
            fallback: FallbackPolicy::RecordedMesh,
            preview: false,
        }
    }
    fn schema(&self) -> Schema {
        Schema {
            fields: vec![
                Field {
                    key: "kind".into(),
                    label: "Kind".into(),
                    group: String::new(),
                    collapsed: false,
                    kind: FieldKind::Choice {
                        options: vec![("FLAT".into(), "Flat".into()), ("RAISED".into(), "Raised".into())],
                        default: "FLAT".into(),
                    },
                    when: String::new(),
                    help: String::new(),
                    unused: false,
                },
                number("length", "Length", 20.0, ""),
                number("width", "Width", 10.0, ""),
                number("raise", "Raise", 5.0, "kind == RAISED"),
            ],
        }
    }
    fn evaluate(&self, p: &Value, _ctx: &Value, host: &mut dyn Host) -> Result<Value, String> {
        let n = |k: &str| p[k].as_f64().unwrap_or(0.0);
        let h = if p["kind"] == json!("RAISED") { 2.0 + n("raise") } else { 2.0 };
        solidcraft_plugin::run(host, "solid.box", json!({"length": n("length"), "width": n("width"), "height": h, "corner": [0, 0, 0]}))?;
        Ok(json!({}))
    }
    fn describe(&self, p: &Value, _ctx: &Value) -> Result<Value, String> {
        Ok(json!({"notice": format!("slab {} x {}", p["length"], p["width"])}))
    }
}

fn harness() -> Harness {
    let _ = solidcraft_plugin::register(Arc::new(Slab));
    let mut h = Harness::new();
    h.app.home.open = false;
    h.frames(2);
    h
}

fn type_into(h: &mut Harness, label: &str, text: &str) {
    h.step(&json!({"click": {"handle": format!("dialog:{label}")}})).unwrap();
    h.step(&json!({"key": "A", "cmd": true})).unwrap();
    h.step(&json!({"text": text})).unwrap();
}

fn ok(h: &mut Harness) {
    h.step(&json!({"click": {"handle": "dialog:OK"}})).unwrap();
    h.frames(3);
    assert!(h.app.dialog.is_none(), "OK closes the dialog: {:?}", h.app.dialog.as_ref().and_then(|d| d.error.clone()));
}

fn shown(_h: &Harness, label: &str) -> bool {
    crate::scenario::handles("dialog:").iter().any(|(n, _)| n == &format!("dialog:{label}"))
}

fn volume(h: &mut Harness) -> f64 {
    h.app.session.execute("inspect.measure", &json!({})).unwrap()["total"]["volume_mm3"].as_f64().unwrap()
}

#[test]
fn a_plugin_dialog_shows_its_fields_under_their_conditions_and_applies() {
    let mut h = harness();
    h.app.start("plugin.add");
    h.frames(3);
    assert!(matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::Plugin(_))), "{:?}", h.app.dialog.as_ref().map(|d| &d.kind));
    assert!(shown(&h, "Length (mm)") && !shown(&h, "Raise (mm)"), "Raise shows only for a raised slab");
    h.step(&json!({"click": {"handle": "dialog:Kind"}})).unwrap();
    h.frames(2);
    h.step(&json!({"click": {"handle": "dialog:Kind:Raised"}})).unwrap();
    h.frames(2);
    assert!(shown(&h, "Raise (mm)"), "a raised slab shows Raise");
    type_into(&mut h, "Length (mm)", "40");
    type_into(&mut h, "Raise (mm)", "8");
    h.frames(2);
    ok(&mut h);
    let v = volume(&mut h);
    assert!((v - 40.0 * 10.0 * 10.0).abs() < 1e-6, "{v}");
    // Edited again: the dialog opens on the stored values.
    let id = h.app.session.doc.features.last().map(|f| f.id).unwrap();
    h.app.edit_feature(id);
    h.frames(3);
    assert!(shown(&h, "Raise (mm)"));
    type_into(&mut h, "Width (mm)", "20");
    h.frames(2);
    ok(&mut h);
    let v = volume(&mut h);
    assert!((v - 40.0 * 20.0 * 10.0).abs() < 1e-6, "{v}");
    assert_eq!(h.app.session.doc.features.len(), 1);
}
