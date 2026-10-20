//! The `gear` plugin as a WebAssembly component: `cargo xtask plugin solidcraft-plugin-gear gear`
//! writes its plugin folder (docs/plugins.md).

solidcraft_plugin_guest::export_plugin!(solidcraft_plugin_gear::GearPlugin);
