//! The foil plugins as WebAssembly components (examples/), loaded from plugin folders by the
//! WebAssembly host (crates/plugin-wasm): the same dialogs as the compiled-in plugins, and the
//! same wing. This test binary registers no compiled-in plugin, so `foil.*` here are the loaded
//! components; the compiled-in plugin is called directly for the comparison.
//!
//! Needs the target: `rustup target add wasm32-unknown-unknown`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use serde_json::{Value, json};
use solidcraft_engine::Session;
use solidcraft_plugin::{Host, Plugin};
use solidcraft_plugin_foil::Tool;
use solidcraft_plugin_foil::plugins::FoilPlugin;
use solidcraft_plugin_wasm::{Limits, Loader, Report, install_folder};

const EXAMPLES: [(&str, &str); 3] = [("wing", "foil-wing"), ("stabiliser", "foil-stabiliser"), ("propeller", "foil-propeller")];

/// The three components, built for wasm32 once per test run in their own target directory.
fn components() -> &'static Vec<Vec<u8>> {
    static BUILT: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let outer =
            std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
        let target = outer.join("plugin-components");
        let mut cmd = Command::new(env!("CARGO"));
        cmd.args(["build", "-p", "solidcraft-plugin-foil", "--target", "wasm32-unknown-unknown", "--target-dir"]).arg(&target);
        for (e, _) in EXAMPLES {
            cmd.args(["--example", e]);
        }
        let out = cmd.env_remove("CARGO_TARGET_DIR").output().expect("cargo runs");
        assert!(
            out.status.success(),
            "building the foil components failed (is the wasm32-unknown-unknown target installed?):\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        EXAMPLES
            .iter()
            .map(|(e, _)| {
                let module = std::fs::read(target.join(format!("wasm32-unknown-unknown/debug/examples/{e}.wasm"))).expect("module");
                wit_component::ComponentEncoder::default().module(&module).and_then(|c| c.validate(true).encode()).expect("component")
            })
            .collect()
    })
}

/// The three plugin folders (plugin.json as the examples have it, version from the package),
/// installed once per test run.
fn installed() -> &'static (Report, PathBuf) {
    static DONE: OnceLock<(Report, PathBuf)> = OnceLock::new();
    DONE.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("solidcraft-foil-plugins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for ((example, id), wasm) in EXAMPLES.iter().zip(components()) {
            let mut manifest: Value = serde_json::from_str(
                &std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join(example).join("plugin.json"))
                    .expect("plugin.json"),
            )
            .expect("json");
            manifest["version"] = json!(env!("CARGO_PKG_VERSION"));
            let folder = dir.join(id);
            std::fs::create_dir_all(&folder).expect("folder");
            std::fs::write(folder.join("plugin.json"), manifest.to_string()).expect("write");
            std::fs::write(folder.join(format!("{id}.wasm")), wasm).expect("write");
        }
        (install_folder(&dir, Limits::default(), &|_| true), dir)
    })
}

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn measure(s: &Session, body: &str) -> solidcraft_kernel::BodyMeasure {
    solidcraft_kernel::measure(&s.model.state().body(body).unwrap_or_else(|| panic!("no body {body}")).body).expect("measure")
}

/// The Silk-like 1050 cm², AR 9 wing of tests/plugins.rs.
fn wing_1050() -> Value {
    json!({
        "targetArea": 1050, "aspectRatio": 9, "profile": "NACA4", "camber": 2.5, "camberPos": 40,
        "rootThickness": 12, "tipThickness": 10, "tipProfile": "NACA4", "tipCamber": 1.0, "tipCamberPos": 50, "blendFrom": 40,
        "chordShape": "SUPERELLIPSE", "superN": 1.4, "endRatio": 0.26, "planformAlign": 78, "sweepTip": 70, "sweepExp": 4,
        "tipDrop": 45, "dropLaw": "SCURVE", "dropFrom": 3, "dropTo": 91, "tipCap": "ROUND_PLAN", "capLength": 18, "capEnd": 25,
        "stations": 13, "teThickness": 0.8, "pointsPerSurface": 50
    })
}

/// A plugin's `evaluate` run directly on a scratch design, as the engine's runner does; the
/// one body it leaves.
fn build_directly(plugin: &dyn Plugin, values: &Value) -> solidcraft_kernel::BodyMeasure {
    struct Scratch(Session);
    impl Host for Scratch {
        fn run(&mut self, command: &str, args: &Value) -> Result<Value, String> {
            self.0.execute(command, args).map_err(|e| e.to_string())
        }
    }
    let mut params = plugin.schema().defaults();
    params.extend(values.as_object().expect("values").clone());
    let mut scratch = Scratch(Session::default());
    plugin.evaluate(&Value::Object(params), &json!({"published": {}}), &mut scratch).expect("evaluate");
    let state = scratch.0.model.state();
    assert_eq!(state.bodies.len(), 1);
    solidcraft_kernel::measure(&state.bodies[0].body).expect("measure")
}

/// All three load from their folders, with the compiled-in plugins' manifests and dialogs.
#[test]
fn the_foil_plugins_load_from_their_folders_with_their_dialogs() {
    let (report, _) = installed();
    assert_eq!(report.installed, ["foil-propeller", "foil-stabiliser", "foil-wing"], "{report:?}");
    for (tool, id) in [(Tool::Wing, "foil-wing"), (Tool::Stabiliser, "foil-stabiliser"), (Tool::Propeller, "foil-propeller")] {
        let loaded = solidcraft_plugin::find(id).expect("registered");
        let compiled = FoilPlugin(tool);
        assert_eq!(loaded.manifest(), compiled.manifest(), "{id}");
        assert_eq!(loaded.schema(), compiled.schema(), "{id}");
        let values = Value::Object(compiled.schema().defaults());
        assert_eq!(loaded.describe(&values, &json!({"published": {}})), compiled.describe(&values, &json!({"published": {}})), "{id}");
    }
}

/// `foil-wing` from its folder builds the wing the compiled-in plugin builds: the planform the
/// dialog asks for (span² = AR · area plus the cap, tests/planform.rs) and the same volume.
#[test]
fn the_loaded_wing_builds_the_compiled_in_wing() {
    installed();
    let mut s = Session::default();
    let r = run(&mut s, "plugin.add", json!({"plugin": "foil-wing", "values": wing_1050(), "body_name": "Wing"}));
    let n = r["notice"].as_str().unwrap_or_default();
    assert!(n.starts_with("Area 1050 cm² projected") && n.contains("AR 9"), "{n}");
    let loaded = measure(&s, "Wing");
    assert!((loaded.bbox.max.y + loaded.bbox.min.y).abs() < 1e-6, "symmetric: {:?}", loaded.bbox);
    assert!((2.0 * loaded.bbox.max.y - 972.111).abs() < 0.05, "span {}", 2.0 * loaded.bbox.max.y);

    let compiled = build_directly(&FoilPlugin(Tool::Wing), &wing_1050());
    let dv = (loaded.volume - compiled.volume).abs() / compiled.volume;
    assert!(dv < 1e-9, "volume {} loaded, {} compiled in ({dv:e})", loaded.volume, compiled.volume);
    for (a, b) in [(loaded.bbox.min, compiled.bbox.min), (loaded.bbox.max, compiled.bbox.max)] {
        assert!(a.dist(b) < 1e-6, "{:?} vs {:?}", loaded.bbox, compiled.bbox);
    }

    // Its own work (not the engine's) against the fuel allowance.
    let wasm = Loader::new(Limits::default()).expect("loader").load(&components()[0]).expect("wing");
    let again = build_directly(&wasm, &wing_1050());
    assert!((again.volume - compiled.volume).abs() / compiled.volume < 1e-9);
    let used = wasm.fuel_used();
    eprintln!("foil-wing: {used} fuel ({:.2} % of the default allowance)", 100.0 * used as f64 / Limits::default().fuel as f64);
    assert!(used > 0 && used < Limits::default().fuel / 10, "{used}");
}

/// A loaded wing and a loaded linked stabiliser: the wing's published values cross the
/// component boundary, and a parameter drives both (span² = AR · area).
#[test]
fn a_loaded_stabiliser_follows_a_loaded_wing() {
    installed();
    let mut s = Session::default();
    run(&mut s, "parameters.add", json!({"name": "area", "expression": "1000", "unit": ""}));
    run(
        &mut s,
        "plugin.add",
        json!({"plugin": "foil-wing", "body_name": "Wing", "values": {"targetArea": "area", "aspectRatio": 8, "stations": 7, "pointsPerSurface": 24}}),
    );
    let r = run(
        &mut s,
        "plugin.add",
        json!({"plugin": "foil-stabiliser", "body_name": "Stab", "values": {"stabLink": "LINKED", "arm": 600, "stations": 7, "pointsPerSurface": 24}}),
    );
    let n = r["notice"].as_str().unwrap_or_default();
    assert!(n.starts_with("Area 150 cm²") && n.contains("AR 6.5"), "{r}");
    let half = |s: &Session, b: &str| measure(s, b).bbox.max.y;
    assert!((half(&s, "Wing") - (8.0f64 * 100_000.0).sqrt() / 2.0).abs() < 0.05);
    assert!((half(&s, "Stab") - (6.5f64 * 15_000.0).sqrt() / 2.0).abs() < 0.05);
    run(&mut s, "parameters.change", json!({"name": "area", "expression": "1200"}));
    assert!((half(&s, "Wing") - (8.0f64 * 120_000.0).sqrt() / 2.0).abs() < 0.05, "the parameter drives the wing");
    assert!((half(&s, "Stab") - (6.5f64 * 18_000.0).sqrt() / 2.0).abs() < 0.05, "the stab follows the wing");
}
