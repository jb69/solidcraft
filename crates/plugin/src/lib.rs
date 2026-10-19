//! The plugin contract (docs/plugins.md, milestone 2: parametric features).
//!
//! A parametric plugin is one timeline feature type. The host stores a `Plugin` feature with the
//! plugin's id and the values of its parameters, draws the dialog from the plugin's
//! [`Schema`], and on every rebuild calls [`Plugin::evaluate`] with the values (expressions
//! already evaluated) and a read-only context. The plugin builds its geometry only through
//! [`Host::run`]: the same `run(command, args)` JSON surface as MCP and the control protocol, on
//! a scratch design whose bodies become the feature's result. There is no private geometry API.
//!
//! The contract is JSON in and out (`serde_json::Value`), the shape of the WIT world in
//! `wit/plugin.wit`, so a plugin written against this trait can move behind a WebAssembly host
//! without changing what it says. This crate is that contract and the registry of plugins
//! compiled into an app; it depends on nothing else in the workspace.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

mod schema;

pub use schema::{Field, FieldKind, Schema, Unit, check, visible};

/// The contract's version: a plugin built for another one is refused.
pub const API_VERSION: u32 = 1;

/// What a plugin is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Unique id, `publisher.name` (`acme.gear`): it is stored in designs.
    pub id: String,
    /// The plugin's name, for menus and the dialog title.
    pub name: String,
    /// Its own version, for the provenance stored with a feature.
    pub version: String,
    /// [`API_VERSION`] it was built against.
    pub api_version: u32,
    #[serde(default)]
    pub description: String,
    /// The feature's timeline row (label, icon, tooltip, type name) when it is made.
    pub display: Display,
    /// What a design does with the feature when the plugin is not installed.
    #[serde(default)]
    pub fallback: FallbackPolicy,
    /// Whether a dialog may rebuild the feature as its values change: a plugin that takes many
    /// seconds to build says no.
    #[serde(default = "yes")]
    pub preview: bool,
}

fn yes() -> bool {
    true
}

/// How a plugin feature is shown in the timeline and the browser. Stored with the feature, so a
/// design shows it the same way without the plugin.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display {
    /// The primary text (`Gear`).
    pub label: String,
    /// A name from the host's icon set (`loft`, `revolve`, `box`…); unknown names get a plain one.
    #[serde(default)]
    pub icon_id: String,
    #[serde(default)]
    pub tooltip: String,
    /// A short type name (`GearFeature`) for inspection.
    #[serde(default)]
    pub type_name: String,
}

/// What happens to a plugin feature whose plugin is not installed (or fails to load).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackPolicy {
    /// The last result is stored with the feature as a mesh: the design opens and shows it, with
    /// a warning; changing the feature needs the plugin.
    #[default]
    RecordedMesh,
    /// The feature is left out, with a warning.
    SuppressWithWarning,
    /// The feature fails to rebuild.
    ErrorOnRebuild,
}

/// What the host lets a plugin do while it evaluates: run commands on its scratch design.
pub trait Host {
    /// Run one command (`solid.extrude`, `sketch.create`…) with JSON arguments, as MCP's `execute`
    /// does. Errors are the command's own message.
    fn run(&mut self, command: &str, args: &Value) -> Result<Value, String>;
}

/// A parametric plugin.
///
/// `evaluate` returns JSON with optional `notice` (a one-line summary for the feature),
/// `warnings` (a list of strings) and `publish` (any JSON, which later features read from their
/// context as `published.<plugin id>`, so one feature can follow another).
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> Manifest;
    /// The dialog: every parameter with its group, kind, bounds, default and show condition.
    fn schema(&self) -> Schema;
    /// Build the feature: `params` holds every parameter (numbers evaluated: lengths in mm, angles
    /// in degrees), `ctx` the host's read-only context (`feature`, `published`).
    fn evaluate(&self, params: &Value, ctx: &Value, host: &mut dyn Host) -> Result<Value, String>;
    /// The notice and warnings `evaluate` would give, without building anything: the dialog shows
    /// it as values change. `null` when the plugin has none to give quickly.
    fn describe(&self, _params: &Value, _ctx: &Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
}

static REGISTRY: RwLock<BTreeMap<String, Arc<dyn Plugin>>> = RwLock::new(BTreeMap::new());

/// Make a plugin available to every design in this process. A plugin built for another
/// [`API_VERSION`], or with an id another plugin already has, is refused.
pub fn register(plugin: Arc<dyn Plugin>) -> Result<(), String> {
    let m = plugin.manifest();
    if m.api_version != API_VERSION {
        return Err(format!("plugin `{}` is built for plugin API {}, this host has {API_VERSION}", m.id, m.api_version));
    }
    if m.id.trim().is_empty() || !m.id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Err(format!("plugin id `{}` must be letters, digits, `.`, `_` and `-`", m.id));
    }
    let mut r = REGISTRY.write().map_err(|_| "the plugin registry is unavailable".to_string())?;
    if r.contains_key(&m.id) {
        return Err(format!("a plugin `{}` is already registered", m.id));
    }
    r.insert(m.id, plugin);
    Ok(())
}

/// The installed plugin with this id.
pub fn find(id: &str) -> Option<Arc<dyn Plugin>> {
    REGISTRY.read().ok().and_then(|r| r.get(id).cloned())
}

/// Every installed plugin, by id.
pub fn installed() -> Vec<Arc<dyn Plugin>> {
    REGISTRY.read().map(|r| r.values().cloned().collect()).unwrap_or_default()
}

/// A command a plugin runs through [`Host::run`], with the arguments built as JSON.
pub fn run(host: &mut dyn Host, command: &str, args: Value) -> Result<Value, String> {
    host.run(command, &args).map_err(|e| format!("{command}: {e}"))
}
