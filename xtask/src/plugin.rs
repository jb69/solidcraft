//! `cargo xtask plugin <package> <example> [--out DIR]`: package a plugin for the WebAssembly
//! host (docs/plugins.md). The example (a `cdylib` that calls
//! `solidcraft_plugin_guest::export_plugin!`) is built for `wasm32-unknown-unknown` in release,
//! made a component, and written with the `plugin.json` beside its source as the plugin folder
//! `DIR/<id>/` (`plugin.json`, `<id>.wasm`). Installing it is copying that folder into the
//! per-user plugins directory.

use std::path::{Path, PathBuf};

use serde_json::Value;

pub fn run(root: &Path, metadata: &Value, args: &[&str]) -> Result<(), String> {
    let (package, example) = match args {
        [p, e, ..] if !p.starts_with('-') && !e.starts_with('-') => (*p, *e),
        _ => return Err("usage: cargo xtask plugin <package> <example> [--out DIR]".into()),
    };
    let out = args.iter().position(|a| *a == "--out").and_then(|i| args.get(i + 1)).map(PathBuf::from).unwrap_or_else(|| root.join("target/plugins"));
    let source = example_source(metadata, package, example)?;
    let manifest_path = source.parent().map(|d| d.join("plugin.json")).ok_or("the example has no folder")?;
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e} (the example's folder needs its plugin.json)", manifest_path.display()))?;
    let mut manifest: Value = serde_json::from_str(&manifest_text).map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    // A plugin.json without a version takes the package's (what `env!("CARGO_PKG_VERSION")` gives
    // the plugin's own manifest), so the two agree as the host checks.
    if manifest.get("version").is_none() {
        manifest["version"] = Value::String(package_version(metadata, package)?);
    }
    let id = manifest["id"].as_str().ok_or_else(|| format!("{}: no id", manifest_path.display()))?;

    let mut c = crate::cargo();
    c.args(["build", "--release", "--target", "wasm32-unknown-unknown", "-p", package, "--example", example]);
    // No debug info in a plugin that is handed out: it is most of the file.
    c.env("CARGO_PROFILE_RELEASE_DEBUG", "false").env("CARGO_PROFILE_RELEASE_STRIP", "true");
    crate::run(c, &format!("cargo build {package} example {example} for wasm32"))?;
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"));
    let module_path = target.join("wasm32-unknown-unknown/release/examples").join(format!("{example}.wasm"));
    let module = std::fs::read(&module_path).map_err(|e| format!("{}: {e}", module_path.display()))?;
    let component = wit_component::ComponentEncoder::default()
        .module(&module)
        .and_then(|e| e.validate(true).encode())
        .map_err(|e| format!("{} is not a plugin module: {e:#}", module_path.display()))?;

    let folder = out.join(id);
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    for old in std::fs::read_dir(&folder).map_err(|e| e.to_string())?.flatten() {
        if old.path().extension().is_some_and(|x| x == "wasm") {
            std::fs::remove_file(old.path()).map_err(|e| e.to_string())?;
        }
    }
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())? + "\n";
    std::fs::write(folder.join("plugin.json"), text).map_err(|e| e.to_string())?;
    std::fs::write(folder.join(format!("{id}.wasm")), &component).map_err(|e| e.to_string())?;
    println!("plugin: {} ({} KiB component)", folder.display(), component.len() / 1024);
    Ok(())
}

fn find_package(metadata: &Value, package: &str) -> Result<Value, String> {
    let packages = metadata["packages"].as_array().ok_or("cargo metadata: no packages")?;
    packages.iter().find(|p| p["name"] == package).cloned().ok_or_else(|| format!("no package `{package}` in the workspace"))
}

fn package_version(metadata: &Value, name: &str) -> Result<String, String> {
    find_package(metadata, name)?["version"].as_str().map(str::to_string).ok_or_else(|| "cargo metadata: no version".into())
}

/// The source file of `package`'s example `example`.
fn example_source(metadata: &Value, package: &str, example: &str) -> Result<PathBuf, String> {
    let p = find_package(metadata, package)?;
    let t = p["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["name"] == example && t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "example")))
        .ok_or_else(|| format!("`{package}` has no example `{example}`"))?;
    t["src_path"].as_str().map(PathBuf::from).ok_or_else(|| "cargo metadata: no src_path".into())
}
