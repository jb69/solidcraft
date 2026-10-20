//! The `foil-wing` plugin as a WebAssembly component: `cargo xtask plugin solidcraft-plugin-foil wing`
//! writes its plugin folder (docs/plugins.md, docs/foil-plugins.md).

solidcraft_plugin_guest::export_plugin!(solidcraft_plugin_foil::plugins::FoilPlugin(solidcraft_plugin_foil::Tool::Wing));
