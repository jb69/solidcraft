# Plugins (design)

Design companion to [#1221](https://github.com/bherbruck/solidcraft/issues/1221): sandboxed
WebAssembly plugins with a WIT interface, the same `run(command, args)` JSON as
[MCP](mcp.md) and the [control protocol](control-protocol.md). Browser hosting is in
[web-runtime.md](web-runtime.md).

This document adds **parametric timeline features**: a generic host-owned feature shell that
plugins fill with parameters, display metadata, and rebuild logic, without a second timeline
model or a private geometry API.

## Goals (unchanged from #1221)

- Sandboxed by default: manifest permissions, fuel/memory limits on desktop, broker enforcement on web.
- Any language that compiles to Wasm components; Rust first.
- **One command surface:** plugins change the model only through engine commands (undoable, journaled).
- Versioned WIT world (`solidcraft:plugin@1.0.0`).

## Two plugin shapes

| Shape | Timeline | Rebuild | Milestone |
|-------|----------|---------|-----------|
| **One-shot** | Optional; often no persistent feature | N/A | 1 (first ship) |
| **Parametric** | One **`Plugin` feature row** per plugin instance | Host calls plugin on param/timeline rebuild | 2 |

One-shot plugins call `execute(params)` once; they may emit ordinary native features (sketch,
extrude, …) inside that transaction. Parametric plugins still use `run()` inside rebuild; the
difference is that the document **records** a single feature whose params the user edits later.

## Implemented: parametric plugins, compiled in or loaded as WebAssembly

Milestone 2's contract runs with plugins compiled into the app and with plugins loaded from the
per-user plugin folders as WebAssembly components. A plugin is the same code either way.

- **Contract** (`crates/plugin`, no workspace dependencies): `Plugin { manifest, schema,
  evaluate, describe }`, `Host { run }`, `Manifest` (id, name, version, `api_version`, `display`,
  `fallback`, `preview`), the parameter `Schema` (fields with group, kind: number with unit,
  bounds, default and integer flag / bool / choice, a `when` show condition, help, `unused`), the
  condition language and value checks, and the registry (`register`, `find`, `installed`). All
  values cross as JSON; `crates/plugin/wit/plugin.wit` is the same world for the Wasm host.
- **Compiled in.** An app registers the plugins it ships at start-up
  (`solidcraft_plugin::register`); tests do the same. These run in-process: not sandboxed, and
  a plugin that loops forever hangs the rebuild.
- **WebAssembly** (`crates/plugin-wasm`, wasmtime with the component model and Cranelift).
  `install_folder(dir, limits, enabled)` loads every folder `<data dir>/plugins/<id>/` that
  holds a `plugin.json` (its `id` the folder's name; the file the Preferences' Plugins page of
  #2718 reads) and exactly one `.wasm` component of the `solidcraft:plugin` world, checks that
  the component's own manifest has the same id and version, and registers it through an adapter
  (`WasmPlugin`) that implements `Plugin`. A folder that does not load is skipped with its
  reason (logged); an id already registered keeps the compiled-in plugin. The desktop app loads
  the folders at start-up (not in safe mode), `solidcraft-cli` for the commands that build
  designs (`SOLIDCRAFT_NO_PLUGINS=1` leaves them out); the browser build loads none.
- **Sandbox.** The component's only import is `host.run`, answered by the same scratch-session
  runner as a compiled-in plugin (its refused commands included); there is no WASI, so no files,
  clock, network or environment. Each call (`evaluate`, `describe`, and `manifest` and `schema`
  once at load) gets a fresh instance with `Limits`: fuel (about one unit per instruction; ten
  billion by default), memory (1 GiB) and stack (2 MiB). A plugin that runs out is stopped and its
  feature fails with the reason (`used its whole allowance of … instructions`, `ran out of
  memory`, `ran out of stack`, `stopped (it panicked)`); the app carries on. Time spent inside
  `host.run` is the engine's and uses no fuel. `describe` cannot run commands.
- **Writing one** (`crates/plugin-guest`): implement `Plugin` as for a compiled-in plugin and
  export it from a `cdylib` (a crate or an example) with
  `solidcraft_plugin_guest::export_plugin!(MyPlugin)`; put its `plugin.json` beside the source
  (without a `version`, it gets the package's).
  `cargo xtask plugin <package> <example>` builds it for `wasm32-unknown-unknown` (release),
  makes the module a component and writes the folder `target/plugins/<id>/`; installing is
  copying that folder into the plugins directory. One component is one plugin. The guest crate
  allows `unsafe_code` for wit-bindgen's export glue only, so a plugin crate in this workspace
  denies it rather than forbidding it.
- **Plugin folders.** The preferences' Plugins page (`crates/ui-egui/src/plugins.rs`, #2718)
  lists `{data}/plugins/<id>/plugin.json` and records the plugins turned off. The loader reads
  `plugin.json` by the same rules (`id`, `name`, `version`, `description` and nothing else; the
  id 1–64 lowercase letters, digits and hyphens, the folder's name), and the desktop app loads
  the folders after its preferences, leaving out the plugins turned off there. The component's
  own manifest is the one registered.
- **Why in-process first.** The contract is JSON plus `run()`, so the Wasm adapter was
  mechanical and no plugin changed for it.
- **Feature shell** (`FeatureKind::Plugin`, `crates/doc/src/plugin_feature.rs`): `plugin_id`,
  `api_version`, `params` (every number an expression and a feature input with a `dN` name, so
  parameters drive it; `inputs` keeps each number's label and unit, so the design walks and
  opens without the plugin), `display`, `provenance`, `fallback`, `operation`, `targets`.
- **Rebuild.** The document evaluates a plugin feature through a runner the engine registers:
  `evaluate` runs against a scratch session through `run()` (file, document, app, library,
  edit and plugin commands refused), and the bodies left there become the feature's result. A
  plugin that panics fails its feature. The plugin's installed version is part of the feature's
  fingerprint, so installing or updating a plugin rebuilds its features.
- **Published values.** `evaluate` may return `publish`; later plugin features read it from their
  context as `published.<plugin id>` (part of the model state's digest, so they rebuild when it
  changes).
- **Fallback.** `recorded_mesh`: the engine stores each such feature's last result in the design
  (`plugin_results`, beside the features, so recording does not change a feature) whenever it is
  saved; without the plugin the design shows it with a warning. `suppress_with_warning` and
  `error_on_rebuild` as above. `recorded_script` is not built.
- **Commands**: `plugin.list`, `plugin.add {plugin, values}`, `plugin.set {feature, values}`,
  `plugin.fields {plugin | feature}`; `timeline.edit` and `timeline.redefine` work on plugin
  features as on built-in ones.
- **Dialog** (`crates/ui-egui/src/dialogs_plugin.rs`): SOLID › CREATE › Plugin Feature picks an
  installed plugin; groups are collapsible as the schema says, a field shows under its
  condition, help is the tooltip, `unused` fields are dimmed, and `describe` gives the notice
  as values are typed. The timeline row uses `display.icon_id` from the host's icons.
- **Tests**: `crates/engine/src/cmd/plugin_tests.rs` (build, parameters, choices and conditions,
  bounds, published values, refused commands, panics, save and reopen, every fallback),
  `crates/ui-egui/src/plugin_dialog_tests.rs` (the dialog), `crates/plugin-wasm/tests/wasm.rs`
  (the test plugins of `crates/plugin-guest/examples` built as components and loaded from
  folders: a feature driven by a parameter, refused commands, skipped folders, fuel and memory
  limits; needs `rustup target add wasm32-unknown-unknown`).
- **Plugins built on it**: the Foil wing, Foil stabiliser and B-Series Propeller,
  [foil-plugins.md](foil-plugins.md) (`crates/plugin-foil`).

## Generic feature shell (host-owned)

Add one variant to the parametric document (name illustrative):

```text
FeatureKind::Plugin {
  plugin_id: String,           // "gear_generator"
  api_version: u32,            // matches WIT world
  params: Value,               // JSON owned by the plugin schema
  display: PluginDisplay,      // timeline + browser presentation
  provenance: PluginProvenance, // optional: manifest hash, build id
}
```

**`PluginDisplay`** (stored in the file, not recomputed from plugin code at open):

| Field | Role |
|-------|------|
| `label` | Primary timeline / browser text |
| `icon_id` | Host icon registry name (same system as [`feature_icon`](https://github.com/bherbruck/solidcraft/blob/main/crates/ui-egui/src/workspace.rs)) |
| `tooltip` | Hover text, `inspect` type string |
| `type_name` | short feature type name for power users |

The timeline renders **any** `Plugin` feature through one code path: read `display`, draw icon +
label, wire suppress/rename/delete/reorder like built-in features. No per-plugin egui in milestone 2.

Optional later: **`parameter_schema()`** in WIT returns `{ id, label, kind, min, max, … }` so the
host renders sliders and dimension rows without custom UI per plugin.

## WIT entry points (phase 2 additions)

Milestone 1 (#1221): `manifest()`, `execute(params) -> result<string, string>`.

Milestone 2 parametric:

| Export | Purpose |
|--------|---------|
| `evaluate(params, ctx) -> result<string, string>` | Rebuild: plugin runs `run()` commands; result is success JSON or error |
| `parameter_schema() -> string` | Optional JSON schema for generic parameter UI |
| `fallback_policy() -> string` | Declares missing-plugin behaviour (see below) |

`ctx` is host-provided read-only hints (configuration name, units, feature id); plugins must not
 rely on it for correctness.

Inside **`evaluate`**, the plugin uses the same **`run(command, args)`** import as one-shot mode.
The host applies commands to the current eval context (or a scratch session merged by the host).
No second registry of geometry ops.

## Rebuild flow

```text
Timeline rebuild reaches FeatureKind::Plugin
  -> load plugin (wasmtime desktop / broker web) if permitted + present
  -> evaluate(stored params)
       -> plugin calls run("sketch.line", …), run("solid.extrude", …), …
  -> host merges geometry like any native feature
  -> errors surface on the feature row like native eval errors
```

Param changes (parameter panel, `parameters.change`, config switch) re-run **`evaluate`** for
affected plugin features only when dependencies say so (same invalidation rules as native features).

## Missing plugin and API drift

Manifest declares **`fallback_policy`**:

| Policy | Behaviour |
|--------|-----------|
| `recorded_mesh` | Optional B-rep or mesh blob stored beside params; open works, edit disabled without plugin |
| `recorded_script` | Stored list of native commands; rebuild without plugin if every command is built-in |
| `suppress_with_warning` | Feature suppressed; user sees clear message |
| `error_on_rebuild` | Hard fail on eval (strict teams) |

Design files shared in CollabCraft should prefer a policy that **opens safely** (`recorded_mesh` or
`recorded_script`). CI for plugins (#1221) should include: uninstall plugin, reopen file, assert
document still opens and policy matches manifest.

## Icons and metadata

Plugins declare allowed **`icon_id`** values in `manifest()`; the host rejects unknown ids. Icons
stay in the app registry so untrusted plugins cannot inject arbitrary raster assets into the tree
without a reviewed icon pack.

**Rename** by the user updates the feature user label; **`display.label`** remains the default from
creation unless the plugin updates it via a host callback (optional future).

Command naming: prefer **`plugin_id.function`** in `run()` or registered command aliases to avoid
collisions ([#1221 discussion](https://github.com/bherbruck/solidcraft/issues/1221)).

## Group header variant (lighter-weight v2a)

Before full **`evaluate`**, a stepping stone: one-shot plugin creates a **feature folder** whose
header carries `PluginDisplay` metadata while children are ordinary native features. Rebuild does
not need the plugin until the user edits plugin-owned params on the header. Migrates cleanly to
full `FeatureKind::Plugin` when `evaluate` lands.

## Relationship to craft-libs

Plugin **document model and eval** stay in SolidCraft (`crates/doc`, `crates/engine`). craft-libs
provides worker/broker hosting only ([web-runtime.md](web-runtime.md)). Neutral serde types
(`VersionedParams`, `PluginDisplay`) may move to a shared crate later if multiple apps need them;
CAD plugins remain SolidCraft-specific.

## Phasing

1. WIT + wasmtime host + one-shot `execute` + permissions tests (#1221 milestone 1).
2. `FeatureKind::Plugin` + `PluginDisplay` + timeline generic row + `evaluate` + fallback policy.
3. Web broker runs same WIT; parametric features offline only if plugin module cached and policy
   allows.
4. Parameter schema UI, cloud-signed plugin index, inspector extensions.

## Tests (parametric)

- Create plugin feature, change param, rebuild, measure geometry (oracle or bbox).
- Undo/redo across plugin eval.
- Remove plugin binary, reopen design, assert fallback policy.
- Icon id from manifest appears on timeline (scenario or screenshot harness).
