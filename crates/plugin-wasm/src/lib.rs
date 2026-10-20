//! The WebAssembly plugin host (docs/plugins.md): plugins as components of the
//! `solidcraft:plugin` world (`crates/plugin/wit/plugin.wit`), loaded from the per-user plugin
//! folders (`<data dir>/plugins/<id>/plugin.json` and one `.wasm` file) and registered beside
//! the plugins compiled into the app. A loaded plugin is a [`solidcraft_plugin::Plugin`] like any
//! other, so `FeatureKind::Plugin`, its dialog and the `plugin.*` commands do not know the
//! difference.
//!
//! The sandbox: the component's one import is `host.run`, the same scratch-session runner a
//! compiled-in plugin gets (crates/engine/src/plugin_host.rs, with its refused commands); it has
//! no WASI, so no files, clock, network or environment. Every call runs in a fresh instance with
//! a fuel allowance (CPU) and a memory limit ([`Limits`]); a plugin that runs out fails its
//! feature with a message instead of hanging the app. Time spent in `host.run` (the engine's own
//! geometry) is not the plugin's and uses no fuel.
//!
//! Desktop only: on `wasm32` (the browser build) [`install_folder`] loads nothing.

use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
mod host;

#[cfg(not(target_arch = "wasm32"))]
pub use host::{Loader, WasmPlugin};

/// What one call into a plugin may use. Each `evaluate`, `describe`, `manifest` and `schema`
/// call gets the whole allowance afresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Fuel: about one unit per WebAssembly instruction the plugin runs.
    pub fuel: u64,
    /// The plugin's linear memory, in bytes.
    pub memory_bytes: usize,
    /// The plugin's call stack, in bytes.
    pub stack_bytes: usize,
}

impl Default for Limits {
    /// Ten billion instructions (seconds of the plugin's own work), 1 GiB of memory, 2 MiB of
    /// stack.
    fn default() -> Limits {
        Limits { fuel: 10_000_000_000, memory_bytes: 1 << 30, stack_bytes: 2 << 20 }
    }
}

/// A folder under the plugins directory that was not loaded, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub folder: String,
    pub reason: String,
}

/// What [`install_folder`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The ids of the plugins it registered.
    pub installed: Vec<String>,
    pub skipped: Vec<Skipped>,
}

/// `plugin.json` larger than this is refused (as the Preferences' Plugins page does).
const MAX_MANIFEST_BYTES: u64 = 16 * 1024;
/// A component larger than this is refused.
const MAX_WASM_BYTES: u64 = 256 << 20;

/// The plugins directory under the per-user data directory (`data_dir()` of
/// `solidcraft_engine::recovery`).
pub fn plugins_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("plugins")
}

/// A plugin folder's `plugin.json`: what the preferences' Plugins page reads (#2718), by its rules.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolderManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
}

/// An id a plugin folder may have, as the Plugins page allows it: 1–64 lowercase letters, digits
/// and hyphens, not starting or ending with a hyphen.
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    (1..=64).contains(&b.len())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && b.first() != Some(&b'-')
        && b.last() != Some(&b'-')
}

/// A plugin folder's `plugin.json`, read and checked against the folder's name.
pub fn read_manifest(folder: &Path) -> Result<FolderManifest, String> {
    let path = folder.join("plugin.json");
    let meta = std::fs::metadata(&path).map_err(|_| "no plugin.json".to_string())?;
    if !meta.is_file() || meta.len() > MAX_MANIFEST_BYTES {
        return Err("plugin.json is missing or larger than 16 KiB".into());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("plugin.json could not be read ({e})"))?;
    let m: FolderManifest = serde_json::from_str(&text).map_err(|e| format!("plugin.json is not a plugin manifest ({e})"))?;
    let name = folder.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if m.id != name {
        return Err(format!("plugin.json id `{}` does not match the folder `{name}`", m.id));
    }
    if !valid_id(&m.id) {
        return Err("the id must be 1–64 lowercase letters, digits and hyphens".into());
    }
    Ok(m)
}

/// The folder's component: its one `.wasm` file.
pub fn component_file(folder: &Path) -> Result<PathBuf, String> {
    let entries = std::fs::read_dir(folder).map_err(|e| format!("the folder could not be read ({e})"))?;
    let mut wasm: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "wasm")).collect();
    match wasm.len() {
        0 => Err("no .wasm file".into()),
        1 => {
            let file = wasm.remove(0);
            if std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0) > MAX_WASM_BYTES {
                return Err(format!("{} is larger than 256 MiB", file.display()));
            }
            Ok(file)
        }
        _ => Err("more than one .wasm file".into()),
    }
}

/// Load every plugin folder in `dir` that `enabled` allows (the Preferences' Plugins page turns
/// plugins off by id) and register each with [`solidcraft_plugin::register`]. A plugin whose id
/// is already registered (one compiled into the app) stays; the folder is skipped. A missing
/// directory loads nothing.
pub fn install_folder(dir: &Path, limits: Limits, enabled: &dyn Fn(&str) -> bool) -> Report {
    let mut report = Report::default();
    let Ok(entries) = std::fs::read_dir(dir) else { return report };
    let mut folders: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
        .filter(|(name, _)| !name.starts_with('.'))
        .collect();
    folders.sort();
    if folders.is_empty() {
        return report;
    }
    let loader = match loader(limits) {
        Ok(l) => l,
        Err(reason) => {
            report.skipped = folders.into_iter().map(|(folder, _)| Skipped { folder, reason: reason.clone() }).collect();
            return report;
        }
    };
    for (folder, path) in folders {
        let installed = read_manifest(&path).and_then(|m| if enabled(&m.id) { install(&loader, &path, &m) } else { Err("turned off".into()) });
        match installed {
            Ok(id) => report.installed.push(id),
            Err(reason) => report.skipped.push(Skipped { folder, reason }),
        }
    }
    for s in &report.skipped {
        log::warn!("plugin folder `{}` not loaded: {}", s.folder, s.reason);
    }
    report
}

#[cfg(not(target_arch = "wasm32"))]
fn loader(limits: Limits) -> Result<Loader, String> {
    Loader::new(limits)
}

#[cfg(not(target_arch = "wasm32"))]
fn install(loader: &Loader, folder: &Path, m: &FolderManifest) -> Result<String, String> {
    let plugin = loader.load_file(&component_file(folder)?)?;
    let got = solidcraft_plugin::Plugin::manifest(&plugin);
    if (got.id.as_str(), got.version.as_str()) != (m.id.as_str(), m.version.as_str()) {
        return Err(format!("the component is `{}` {}, plugin.json says `{}` {}", got.id, got.version, m.id, m.version));
    }
    solidcraft_plugin::register(std::sync::Arc::new(plugin))?;
    Ok(got.id)
}

#[cfg(target_arch = "wasm32")]
struct Loader;

#[cfg(target_arch = "wasm32")]
fn loader(_: Limits) -> Result<Loader, String> {
    Err("plugins load on the desktop only".into())
}

#[cfg(target_arch = "wasm32")]
fn install(_: &Loader, _: &Path, _: &FolderManifest) -> Result<String, String> {
    Err("plugins load on the desktop only".into())
}
