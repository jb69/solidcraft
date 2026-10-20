//! Plugins as WebAssembly components: the test plugins of crates/plugin-guest (examples) built
//! for `wasm32-unknown-unknown`, made components, put in plugin folders and loaded from there.
//! A loaded plugin builds its feature as a compiled-in one does, cannot reach past `host.run`,
//! and is stopped by its fuel and memory limits with a message.
//!
//! Needs the target: `rustup target add wasm32-unknown-unknown`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use solidcraft_engine::Session;
use solidcraft_plugin::{Host, Plugin};
use solidcraft_plugin_wasm::{Limits, Loader, install_folder};

/// The test plugins' components, built once per test run: `(slab, spin, hog)`.
fn components() -> &'static [Vec<u8>; 3] {
    static BUILT: OnceLock<[Vec<u8>; 3]> = OnceLock::new();
    BUILT.get_or_init(|| {
        let names = ["slab", "spin", "hog"];
        let modules = build_examples("solidcraft-plugin-guest", &names);
        names.map(|n| componentize(&modules.join(format!("{n}.wasm"))))
    })
}

/// Build a package's examples for wasm32 in their own target directory (the outer build holds
/// the lock on its own); the directory they are in.
fn build_examples(package: &str, examples: &[&str]) -> PathBuf {
    let outer = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let target = outer.join("plugin-components");
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["build", "-p", package, "--target", "wasm32-unknown-unknown", "--target-dir"]).arg(&target);
    for e in examples {
        cmd.args(["--example", e]);
    }
    let out = cmd.env_remove("CARGO_TARGET_DIR").output().expect("cargo runs");
    assert!(
        out.status.success(),
        "building the test plugins failed (is the wasm32-unknown-unknown target installed?):\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    target.join("wasm32-unknown-unknown/debug/examples")
}

/// A core module with wit-bindgen's component type in it, as a component.
fn componentize(module: &Path) -> Vec<u8> {
    let bytes = std::fs::read(module).unwrap_or_else(|e| panic!("{}: {e}", module.display()));
    wit_component::ComponentEncoder::default()
        .module(&bytes)
        .and_then(|e| e.validate(true).encode())
        .unwrap_or_else(|e| panic!("{}: {e:#}", module.display()))
}

/// A fresh plugins directory for one test.
fn plugins_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("solidcraft-plugin-wasm-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A plugin folder: `plugin.json` and the component.
fn folder(dir: &Path, name: &str, manifest: Value, wasm: &[u8]) -> PathBuf {
    let f = dir.join(name);
    std::fs::create_dir_all(&f).expect("folder");
    std::fs::write(f.join("plugin.json"), manifest.to_string()).expect("plugin.json");
    std::fs::write(f.join(format!("{name}.wasm")), wasm).expect("component");
    f
}

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn volume(s: &Session, body: &str) -> f64 {
    solidcraft_kernel::measure(&s.model.state().body(body).unwrap_or_else(|| panic!("no body {body}")).body).expect("measure").volume
}

/// A host for calls that must not reach it.
struct NoHost;

impl Host for NoHost {
    fn run(&mut self, command: &str, _args: &Value) -> Result<Value, String> {
        Err(format!("unexpected `{command}`"))
    }
}

/// The slab installed from its own folder, once per test run (the registry is the process's).
fn slab_installed() -> &'static solidcraft_plugin_wasm::Report {
    static REPORT: OnceLock<solidcraft_plugin_wasm::Report> = OnceLock::new();
    REPORT.get_or_init(|| {
        let dir = plugins_dir("slab");
        folder(&dir, "test-slab", json!({"id": "test-slab", "name": "Test Slab", "version": "1.0.0"}), &components()[0]);
        let report = install_folder(&dir, Limits::default(), &|_| true);
        let _ = std::fs::remove_dir_all(&dir);
        report
    })
}

/// The slab from its folder: registered under its id, built through `host.run`, driven by a
/// parameter, its volume length · width · height.
#[test]
fn a_plugin_folder_loads_and_its_feature_builds() {
    let report = slab_installed();
    assert_eq!(report.installed, ["test-slab"], "{report:?}");
    let p = solidcraft_plugin::find("test-slab").expect("registered");
    assert_eq!((p.manifest().display.label.as_str(), p.schema().fields.len()), ("Slab", 4));

    let mut s = Session::default();
    let list = run(&mut s, "plugin.list", json!({}));
    assert!(list["plugins"].as_array().expect("list").iter().any(|p| p["id"] == "test-slab"), "{list}");
    run(&mut s, "parameters.add", json!({"name": "len", "expression": "30 mm"}));
    let r = run(&mut s, "plugin.add", json!({"plugin": "test-slab", "values": {"length": "len", "width": 20, "height": 5}, "body_name": "S"}));
    assert_eq!((r["notice"].as_str(), r["values"]["volume_mm3"].as_f64()), (Some("slab 30 x 20"), Some(3000.0)), "{r}");
    assert!((volume(&s, "S") - 3000.0).abs() < 1e-6);
    run(&mut s, "parameters.change", json!({"name": "len", "expression": "45 mm"}));
    assert!((volume(&s, "S") - 4500.0).abs() < 1e-6);
    run(&mut s, "plugin.set", json!({"feature": r["name"], "values": {"height": 8}}));
    assert!((volume(&s, "S") - 7200.0).abs() < 1e-6);
    // describe runs without a host.
    assert_eq!(p.describe(&json!({"length": 3, "width": 4}), &json!({})), Ok(json!({"notice": "slab 3 x 4"})));
}

/// The sandbox: the plugin's commands go through the host's refusals; a second copy of an
/// installed id, a folder that does not match its manifest, a turned-off plugin and a folder
/// without exactly one component are skipped with their reasons.
#[test]
fn a_loaded_plugin_is_held_to_the_host_and_bad_folders_are_skipped() {
    let [slab, ..] = components();
    let dir = plugins_dir("folders");
    folder(&dir, "test-slab", json!({"id": "test-slab", "name": "Test Slab", "version": "1.0.0"}), slab);
    folder(&dir, "test-other", json!({"id": "test-different", "name": "x", "version": "1"}), slab);
    folder(&dir, "test-stale", json!({"id": "test-stale", "name": "x", "version": "1"}), slab);
    folder(&dir, "test-off", json!({"id": "test-off", "name": "x", "version": "1"}), slab);
    let two = folder(&dir, "test-two", json!({"id": "test-two", "name": "x", "version": "1"}), slab);
    std::fs::write(two.join("second.wasm"), slab).expect("second");
    let none = folder(&dir, "test-none", json!({"id": "test-none", "name": "x", "version": "1"}), slab);
    std::fs::remove_file(none.join("test-none.wasm")).expect("remove");
    folder(&dir, "test.dotted", json!({"id": "test.dotted", "name": "x", "version": "1"}), slab);
    folder(&dir, "test-extra", json!({"id": "test-extra", "name": "x", "version": "1", "display": {}}), slab);
    std::fs::create_dir_all(dir.join(".hidden")).expect("hidden");
    slab_installed();
    let report = install_folder(&dir, Limits::default(), &|id| id != "test-off");
    let reason =
        |f: &str| report.skipped.iter().find(|s| s.folder == f).map(|s| s.reason.clone()).unwrap_or_else(|| panic!("{f} not skipped: {report:?}"));
    assert!(report.installed.is_empty(), "{report:?}");
    assert!(reason("test-slab").contains("already registered"), "{}", reason("test-slab"));
    assert!(reason("test-other").contains("does not match the folder"), "{}", reason("test-other"));
    assert!(reason("test-stale").contains("the component is `test-slab` 1.0.0"), "{}", reason("test-stale"));
    assert_eq!(reason("test-off"), "turned off");
    assert_eq!(reason("test-two"), "more than one .wasm file");
    assert_eq!(reason("test-none"), "no .wasm file");
    // The Plugins page's rules (#2718): ids of lowercase letters, digits and hyphens, and a
    // plugin.json of its four fields only.
    assert!(reason("test.dotted").contains("lowercase letters, digits and hyphens"), "{}", reason("test.dotted"));
    assert!(reason("test-extra").contains("unknown field `display`"), "{}", reason("test-extra"));
    assert_eq!(report.skipped.len(), 8, "{report:?}");

    let mut s = Session::default();
    let r = run(&mut s, "plugin.add", json!({"plugin": "test-slab", "values": {"probe": true}}));
    assert!(r["values"]["refused"].as_str().unwrap_or_default().contains("a plugin cannot run `file.save`"), "{r}");
    assert!(!Path::new("/tmp/x.solidcraft").exists());
    // A component that is not one is refused when it is loaded.
    let loader = Loader::new(Limits::default()).expect("loader");
    let e = loader.load(b"\0asm\x01\0\0\0").err().unwrap_or_default();
    assert!(e.contains("not a WebAssembly component") || e.contains("does not export"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A plugin that loops forever is stopped by its fuel; one that eats memory by its limit. Each
/// fails its call (and its feature) with a message, and the host carries on.
#[test]
fn a_runaway_plugin_is_stopped_with_a_message() {
    let [slab, spin, hog] = components();
    let limits = Limits { fuel: 200_000_000, memory_bytes: 64 << 20, ..Limits::default() };
    let loader = Loader::new(limits).expect("loader");
    let spin = loader.load(spin).expect("spin loads");
    let start = Instant::now();
    let e = spin.evaluate(&json!({}), &json!({}), &mut NoHost).expect_err("stopped");
    assert!(e.contains("test-spin") && e.contains("whole allowance of 200 million instructions"), "{e}");
    assert!(start.elapsed() < Duration::from_secs(20), "stopped in {:?}", start.elapsed());
    let hog = loader.load(hog).expect("hog loads");
    let e = hog.evaluate(&json!({}), &json!({}), &mut NoHost).expect_err("stopped");
    assert!(e.contains("test-hog") && e.contains("ran out of memory") && e.contains("the limit is 64 MiB"), "{e}");

    // In a design: the spinning plugin's feature fails with the same message.
    let _ = solidcraft_plugin::register(std::sync::Arc::new(spin));
    let mut s = Session::default();
    let e = s.execute("plugin.add", &json!({"plugin": "test-spin"})).expect_err("fails").to_string();
    assert!(e.contains("whole allowance"), "{e}");
    assert!(s.doc.features.is_empty());
    // The same engine still runs a well-behaved plugin.
    let slab = loader.load(slab).expect("slab loads");
    let mut scratch = Session::default();
    struct Scratch<'a>(&'a mut Session);
    impl Host for Scratch<'_> {
        fn run(&mut self, command: &str, args: &Value) -> Result<Value, String> {
            self.0.execute(command, args).map_err(|e| e.to_string())
        }
    }
    let r = slab.evaluate(&json!({"length": 2, "width": 3, "height": 4}), &json!({}), &mut Scratch(&mut scratch)).expect("slab");
    assert_eq!(r["values"]["volume_mm3"], json!(24.0));
    assert!(slab.fuel_used() > 0 && slab.fuel_used() < 10_000_000, "{}", slab.fuel_used());
}
