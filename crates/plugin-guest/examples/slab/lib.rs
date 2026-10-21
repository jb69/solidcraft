//! A test plugin for the WebAssembly host: a slab of the length, width and height asked for,
//! built with one `solid.box` through `host.run`. Its volume is length · width · height.

use serde_json::{Value, json};
use solidcraft_plugin::{Display, FallbackPolicy, Field, FieldKind, Host, Manifest, Plugin, Schema, Unit};

pub struct Slab;

fn length(key: &str, label: &str, default: f64) -> Field {
    Field {
        key: key.into(),
        label: label.into(),
        group: "Size".into(),
        collapsed: false,
        kind: FieldKind::Number { min: 1.0, default, max: 500.0, unit: Unit::Length, integer: false },
        when: String::new(),
        help: format!("the slab's {label}"),
        unused: false,
    }
}

impl Plugin for Slab {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "test-slab".into(),
            name: "Test Slab".into(),
            version: "1.0.0".into(),
            api_version: solidcraft_plugin::API_VERSION,
            description: "A slab, for the WebAssembly host's tests.".into(),
            display: Display { label: "Slab".into(), icon_id: "box".into(), ..Default::default() },
            fallback: FallbackPolicy::RecordedMesh,
            preview: true,
        }
    }
    fn schema(&self) -> Schema {
        let probe = Field {
            key: "probe".into(),
            label: "Probe the sandbox".into(),
            group: String::new(),
            collapsed: false,
            kind: FieldKind::Bool { default: false },
            when: String::new(),
            help: "try a command a plugin may not run, and report the refusal".into(),
            unused: false,
        };
        Schema { fields: vec![length("length", "Length", 20.0), length("width", "Width", 10.0), length("height", "Height", 2.0), probe] }
    }
    fn evaluate(&self, p: &Value, _ctx: &Value, host: &mut dyn Host) -> Result<Value, String> {
        let n = |k: &str| p[k].as_f64().unwrap_or(0.0);
        solidcraft_plugin::run(host, "solid.box", json!({"length": n("length"), "width": n("width"), "height": n("height"), "corner": [0, 0, 0]}))?;
        if p["probe"].as_bool() == Some(true) {
            // What a plugin may not do: the host refuses it and the plugin reports the refusal.
            let refused = solidcraft_plugin::run(host, "file.save", json!({"path": "/tmp/x.solidcraft"})).err().unwrap_or_default();
            return Ok(json!({"notice": "probed", "values": {"refused": refused}}));
        }
        Ok(
            json!({"notice": format!("slab {} x {} x {}", n("length"), n("width"), n("height")), "values": {"volume_mm3": n("length") * n("width") * n("height")}}),
        )
    }
    fn describe(&self, p: &Value, _ctx: &Value) -> Result<Value, String> {
        let n = |k: &str| p[k].as_f64().unwrap_or(0.0);
        Ok(json!({"notice": format!("slab {} x {}", n("length"), n("width"))}))
    }
}

solidcraft_plugin_guest::export_plugin!(Slab);
