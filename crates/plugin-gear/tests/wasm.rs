//! The gear plugin as a WebAssembly component (examples/gear), loaded from a plugin folder by
//! the WebAssembly host: the same dialog and notices as the compiled-in plugin, the same gear,
//! and a second loaded gear meshing with the first through its published values. This test
//! binary registers no compiled-in plugin, so `gear` here is the loaded component; the
//! compiled-in plugin is called directly for the comparison.
//!
//! Needs the target: `rustup target add wasm32-unknown-unknown`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use serde_json::{Value, json};
use solidcraft_engine::Session;
use solidcraft_plugin::{Host, Plugin};
use solidcraft_plugin_gear::GearPlugin;
use solidcraft_plugin_wasm::{Limits, Loader, Report, install_folder};

/// The component, built for wasm32 once per test run in its own target directory.
fn component() -> &'static Vec<u8> {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let outer =
            std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
        let target = outer.join("plugin-components");
        let out = Command::new(env!("CARGO"))
            .args(["build", "-p", "solidcraft-plugin-gear", "--example", "gear", "--target", "wasm32-unknown-unknown", "--target-dir"])
            .arg(&target)
            .env_remove("CARGO_TARGET_DIR")
            .output()
            .expect("cargo runs");
        assert!(
            out.status.success(),
            "building the gear component failed (is the wasm32-unknown-unknown target installed?):\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let module = std::fs::read(target.join("wasm32-unknown-unknown/debug/examples/gear.wasm")).expect("module");
        wit_component::ComponentEncoder::default().module(&module).and_then(|c| c.validate(true).encode()).expect("component")
    })
}

/// The `gear` plugin folder (plugin.json as the example has it, version from the package),
/// installed once per test run.
fn installed() -> &'static Report {
    static DONE: OnceLock<Report> = OnceLock::new();
    DONE.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("solidcraft-gear-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut manifest: Value = serde_json::from_str(
            &std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/gear/plugin.json")).expect("plugin.json"),
        )
        .expect("json");
        manifest["version"] = json!(env!("CARGO_PKG_VERSION"));
        let folder = dir.join("gear");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join("plugin.json"), manifest.to_string()).expect("write");
        std::fs::write(folder.join("gear.wasm"), component()).expect("write");
        let report = install_folder(&dir, Limits::default(), &|_| true);
        let _ = std::fs::remove_dir_all(&dir);
        report
    })
}

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn measure(s: &Session, body: &str) -> solidcraft_kernel::BodyMeasure {
    solidcraft_kernel::measure(&s.model.state().body(body).unwrap_or_else(|| panic!("no body {body}")).body).expect("measure")
}

/// A scratch design that keeps every command a plugin runs on it.
struct Scratch(Session, Vec<(String, Value)>);

impl Host for Scratch {
    fn run(&mut self, command: &str, args: &Value) -> Result<Value, String> {
        self.1.push((command.to_string(), args.clone()));
        self.0.execute(command, args).map_err(|e| e.to_string())
    }
}

/// A plugin's `evaluate` run directly on a scratch design, as the engine's runner does: the one
/// body it leaves and the commands it ran.
fn build_directly(plugin: &dyn Plugin, values: &Value) -> (solidcraft_kernel::BodyMeasure, Vec<(String, Value)>) {
    let mut params = plugin.schema().defaults();
    params.extend(values.as_object().expect("values").clone());
    let mut scratch = Scratch(Session::default(), Vec::new());
    plugin.evaluate(&Value::Object(params), &json!({"published": {}}), &mut scratch).expect("evaluate");
    let state = scratch.0.model.state();
    assert_eq!(state.bodies.len(), 1);
    (solidcraft_kernel::measure(&state.bodies[0].body).expect("measure"), scratch.1)
}

/// The largest difference between two JSON values that differ only in their numbers; `None`
/// when they differ in anything else.
fn number_gap(a: &Value, b: &Value) -> Option<f64> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => Some((x.as_f64()? - y.as_f64()?).abs()),
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x.iter().zip(y).try_fold(0.0f64, |m, (p, q)| Some(m.max(number_gap(p, q)?))),
        (Value::Object(x), Value::Object(y)) if x.len() == y.len() => x.iter().try_fold(0.0f64, |m, (k, p)| Some(m.max(number_gap(p, y.get(k)?)?))),
        (Value::String(x), Value::String(y)) if x.ends_with(" deg") && y.ends_with(" deg") => {
            let n = |s: &str| s.trim_end_matches(" deg").parse::<f64>().ok();
            Some((n(x)? - n(y)?).abs())
        }
        _ if a == b => Some(0.0),
        _ => None,
    }
}

fn helical() -> Value {
    json!({"teeth": 17, "module": 1.5, "helix_angle": 20, "hand": "LEFT", "profile_shift": 0.2, "face_width": 9, "bore": 8})
}

/// The folder loads, with the compiled-in plugin's manifest, dialog and notice; the loaded gear
/// is the compiled-in gear.
#[test]
fn the_gear_loads_from_its_folder_and_builds_the_compiled_in_gear() {
    assert_eq!(installed().installed, ["gear"], "{:?}", installed());
    let loaded = solidcraft_plugin::find("gear").expect("registered");
    assert_eq!(loaded.manifest(), GearPlugin.manifest());
    assert_eq!(loaded.schema(), GearPlugin.schema());
    let values = Value::Object(GearPlugin.schema().defaults());
    assert_eq!(loaded.describe(&values, &json!({"published": {}})), GearPlugin.describe(&values, &json!({"published": {}})));

    // The component sends the commands the compiled-in plugin sends; its numbers differ only in
    // the last bits (WebAssembly's libm against the system's).
    let wasm = Loader::new(Limits::default()).expect("loader").load(component()).expect("gear");
    let (loaded, sent) = build_directly(&wasm, &helical());
    let (compiled, expected) = build_directly(&GearPlugin, &helical());
    assert_eq!(sent.len(), expected.len());
    for ((c1, a1), (c2, a2)) in sent.iter().zip(&expected) {
        assert_eq!(c1, c2);
        let gap = number_gap(a1, a2).unwrap_or_else(|| panic!("{c1}: {a1} vs {a2}"));
        assert!(gap < 1e-9, "{c1}: numbers {gap} apart");
    }
    assert!((loaded.volume - compiled.volume).abs() / compiled.volume < 1e-6, "{} vs {}", loaded.volume, compiled.volume);

    // From its folder, in a design: the same gear.
    let mut s = Session::default();
    run(&mut s, "plugin.add", json!({"plugin": "gear", "values": helical(), "body_name": "G"}));
    let from_folder = measure(&s, "G");
    assert!((from_folder.volume - compiled.volume).abs() / compiled.volume < 1e-6, "{} vs {}", from_folder.volume, compiled.volume);
    for (a, b) in [(from_folder.bbox.min, compiled.bbox.min), (from_folder.bbox.max, compiled.bbox.max)] {
        assert!(a.dist(b) < 1e-6, "{:?} vs {:?}", from_folder.bbox, compiled.bbox);
    }

    // Its own work against the fuel allowance.
    let used = wasm.fuel_used();
    eprintln!("gear: {used} fuel ({:.3} % of the default allowance)", 100.0 * used as f64 / Limits::default().fuel as f64);
    assert!(used > 0 && used < Limits::default().fuel / 100, "{used}");
}

/// Two loaded gears in mesh: the second reads the first's published values across the
/// component boundary and sits at (z1 + z2)·m/2, and a parameter drives both.
#[test]
fn two_loaded_gears_mesh_through_their_published_values() {
    installed();
    let mut s = Session::default();
    run(&mut s, "parameters.add", json!({"name": "gear_module", "expression": "2", "unit": ""}));
    run(
        &mut s,
        "plugin.add",
        json!({"plugin": "gear", "values": {"teeth": 15, "module": "gear_module", "face_width": 5, "backlash": 0.1}, "body_name": "A"}),
    );
    let r = run(
        &mut s,
        "plugin.add",
        json!({"plugin": "gear", "values": {"teeth": 45, "module": "gear_module", "face_width": 5, "backlash": 0.1, "mesh": "PREVIOUS"}, "body_name": "B"}),
    );
    assert_eq!(r["values"]["center_distance_mm"], json!(60.0), "{r}");
    run(&mut s, "parameters.change", json!({"name": "gear_module", "expression": "3"}));
    let b = measure(&s, "B");
    assert!(((b.bbox.min.x + b.bbox.max.x) / 2.0 - 90.0).abs() < 1.0, "the second gear moved out to 90: {:?}", b.bbox);
    let fit = run(&mut s, "inspect.interference", json!({}));
    assert!(fit["interferences"].as_array().is_none_or(|a| a.is_empty()), "{fit}");
}
