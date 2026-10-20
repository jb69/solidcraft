//! A gear studio for SolidCraft (docs/plugins.md): the `gear` plugin builds one involute gear per
//! feature, spur or helical, external or a ring gear, with profile shift, backlash, a bore or a
//! rim, and places it on its own or in mesh with the gear feature before it, at the working
//! centre distance and turned so the teeth fit. Its notice and values give the sizes, centre
//! distance, ratio and contact ratio; undercut, pointed tips, wrong helix hands, ring
//! interference and a low contact ratio are coded issues with the change that clears them.
//!
//! The same plugin is compiled into an app ([`register`]) or built as a WebAssembly component
//! (`examples/gear`, `cargo xtask plugin solidcraft-plugin-gear gear`) for the plugin folders.
//!
//! - [`involute`]: the gear's sizes, its tooth outline and how two gears mesh;
//! - [`plugin`]: the plugin (schema, notice and issues, the sketch and extrude or sweep).

pub mod involute;
pub mod plugin;

pub use plugin::{GearPlugin, ID};

/// Register the plugin (an app calls it at start-up; a second call does nothing).
pub fn register() {
    let _ = solidcraft_plugin::register(std::sync::Arc::new(GearPlugin));
}
