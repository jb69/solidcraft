//! The in-process plugin host: a plugin feature's `evaluate` runs against a scratch session, and
//! the bodies it leaves there become the feature's result (docs/plugins.md). The plugin sees only
//! [`solidcraft_plugin::Host::run`], the engine's command surface, minus the commands that reach
//! outside the scratch design (files, documents, the app).

use std::sync::Arc;

use serde_json::Value;
use solidcraft_doc::FeatureKind;
use solidcraft_doc::plugin_feature::{PluginRunner, set_runner};
use solidcraft_kernel::Body;
use solidcraft_plugin::{FallbackPolicy, Host, Plugin};

use crate::Session;

/// Commands a plugin may not run: they read or write files, open or close designs, or act on
/// the app, none of which a feature's rebuild may do.
fn refused(command: &str) -> bool {
    const PREFIXES: &[&str] = &["file.", "doc.", "app.", "ui.", "library.", "diagnostics.", "plugin.", "edit.", "recovery."];
    PREFIXES.iter().any(|p| command.starts_with(p)) || command.contains("export") || command.contains("import")
}

struct Scratch<'a> {
    session: &'a mut Session,
}

impl Host for Scratch<'_> {
    fn run(&mut self, command: &str, args: &Value) -> Result<Value, String> {
        if refused(command) {
            return Err(format!("a plugin cannot run `{command}`"));
        }
        self.session.execute(command, args).map_err(|e| e.to_string())
    }
}

struct InProcess;

impl PluginRunner for InProcess {
    fn evaluate(&self, plugin: &Arc<dyn Plugin>, params: &Value, ctx: &Value) -> Result<(Vec<Body>, Value), String> {
        let mut session = Session::default();
        // A plugin that panics fails its feature; it never takes the app down.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.evaluate(params, ctx, &mut Scratch { session: &mut session })))
            .map_err(|_| format!("the plugin `{}` failed (it panicked)", plugin.manifest().id))??;
        let state = session.model.state();
        if let Some(e) = session.model.results.iter().find_map(|r| r.error.clone()) {
            return Err(format!("a step of the plugin failed: {e}"));
        }
        Ok((state.bodies.iter().map(|b| b.body.clone()).collect(), result))
    }
}

/// Make plugin features evaluate in this process (once).
pub(crate) fn install() {
    set_runner(Arc::new(InProcess));
}

/// Store each plugin feature's current result with the design, for the ones whose fallback is a
/// recorded mesh (shown when the plugin is missing). A feature whose plugin is missing keeps the
/// result it has.
pub(crate) fn record_results(s: &mut Session) {
    let state = s.model.state();
    let mut results = std::collections::BTreeMap::new();
    for f in &s.doc.features {
        let FeatureKind::Plugin { plugin_id, fallback: FallbackPolicy::RecordedMesh, .. } = &f.kind else { continue };
        if solidcraft_plugin::find(plugin_id).is_none() || f.suppressed {
            if let Some(old) = s.doc.plugin_results.get(&f.id) {
                results.insert(f.id, old.clone());
            }
            continue;
        }
        let meshes: Vec<solidcraft_doc::MeshData> = state
            .bodies
            .iter()
            .filter(|b| b.feature == f.id)
            .filter_map(|b| {
                let m = b.body.display_mesh(0.02).ok()?;
                Some(solidcraft_doc::MeshData {
                    name: b.name.clone(),
                    positions: m.positions.iter().flat_map(|p| [p.x, p.y, p.z]).collect(),
                    triangles: m.triangles.iter().flatten().copied().collect(),
                    color: None,
                })
            })
            .collect();
        if !meshes.is_empty() {
            results.insert(f.id, meshes);
        }
    }
    if results != s.doc.plugin_results {
        s.doc_mut().plugin_results = results;
    }
}
