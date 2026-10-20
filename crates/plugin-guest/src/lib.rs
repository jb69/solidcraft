//! The guest side of WebAssembly plugins (docs/plugins.md): a plugin written against
//! [`solidcraft_plugin::Plugin`] becomes a component of the `solidcraft:plugin` world
//! (`crates/plugin/wit/plugin.wit`) with one line,
//!
//! ```ignore
//! solidcraft_plugin_guest::export_plugin!(my_crate::Gear);
//! ```
//!
//! in a `cdylib` crate (or example) built for `wasm32-unknown-unknown`, then made a component
//! (`cargo xtask plugin`). The plugin's [`Host`] is the world's one import, `host.run`; the
//! plugin itself is the same code an app compiles in. Built for any other target the crate
//! compiles too (so a workspace builds, tests and lints it), but exports nothing and has no host
//! to call.

#![allow(unsafe_code)] // the bindings below: calling the imported `host.run`

use serde_json::Value;
use solidcraft_plugin::{Host, Plugin};

/// The bindings of the `solidcraft:plugin` world, for [`export_plugin!`].
#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        path: "../plugin/wit",
        world: "parametric-plugin",
        pub_export_macro: true,
        export_macro_name: "__export_world",
        default_bindings_module: "solidcraft_plugin_guest::bindings",
    });
}

/// The component's host: each command crosses to the app's `host.run` as JSON text.
pub struct ComponentHost;

impl Host for ComponentHost {
    fn run(&mut self, command: &str, args: &Value) -> Result<Value, String> {
        let out = bindings::solidcraft::plugin::host::run(command, &args.to_string())?;
        serde_json::from_str(&out).map_err(|e| format!("the host's answer is not JSON ({e})"))
    }
}

fn parse(what: &str, text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|e| format!("the {what} are not JSON ({e})"))
}

/// The world's `manifest`, as JSON.
#[doc(hidden)]
pub fn manifest(plugin: &dyn Plugin) -> String {
    serde_json::to_string(&plugin.manifest()).unwrap_or_default()
}

/// The world's `schema`, as JSON.
#[doc(hidden)]
pub fn schema(plugin: &dyn Plugin) -> String {
    serde_json::to_string(&plugin.schema()).unwrap_or_default()
}

/// The world's `evaluate`: the plugin runs its commands through [`ComponentHost`].
#[doc(hidden)]
pub fn evaluate(plugin: &dyn Plugin, params: &str, ctx: &str) -> Result<String, String> {
    let out = plugin.evaluate(&parse("parameters", params)?, &parse("context", ctx)?, &mut ComponentHost)?;
    Ok(out.to_string())
}

/// The world's `describe`.
#[doc(hidden)]
pub fn describe(plugin: &dyn Plugin, params: &str, ctx: &str) -> Result<String, String> {
    Ok(plugin.describe(&parse("parameters", params)?, &parse("context", ctx)?)?.to_string())
}

/// Export a plugin as this component's `solidcraft:plugin/plugin` interface. `$plugin` is an
/// expression giving the plugin (a unit struct, usually); it is evaluated on every call, so it
/// should be cheap. One component exports one plugin: its folder's `plugin.json` names it.
#[macro_export]
macro_rules! export_plugin {
    ($plugin:expr) => {
        // Only a WebAssembly build exports anything: the export names (`…@1.0.0#evaluate`) are
        // not symbols a native library can export, and a native build of the example (which
        // `cargo test` makes) has no host to be called by.
        #[cfg(target_arch = "wasm32")]
        #[allow(unsafe_code)] // the component's exported functions
        mod __solidcraft_plugin_exports {
            #[allow(unused_imports)]
            use super::*;

            struct Exported;

            impl $crate::bindings::exports::solidcraft::plugin::plugin::Guest for Exported {
                fn manifest() -> String {
                    $crate::manifest(&$plugin)
                }
                fn schema() -> String {
                    $crate::schema(&$plugin)
                }
                fn evaluate(params: String, ctx: String) -> Result<String, String> {
                    $crate::evaluate(&$plugin, &params, &ctx)
                }
                fn describe(params: String, ctx: String) -> Result<String, String> {
                    $crate::describe(&$plugin, &params, &ctx)
                }
            }

            $crate::bindings::__export_world!(Exported with_types_in $crate::bindings);
        }
    };
}
