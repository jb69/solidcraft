# Foil plugins

The **Foil wing** and **Foil stabiliser** of Jason Bird's Foil design tool (an Onshape
FeatureScript he wrote and publishes, build 246) and the **B-Series Propeller** of his propeller
generator (build 99) as SolidCraft plugins (`crates/plugin-foil`), on the plugin host of [plugins.md](plugins.md). Each is one parametric timeline feature holding every
value of its dialog and rebuilt from them on every change. The host (`crates/plugin` and the
`Plugin` feature) carries nothing of them.

| Plugin | Id |
|---|---|
| Foil Wing | `foil-wing` |
| Foil Stabiliser | `foil-stabiliser` |
| B-Series Propeller | `foil-propeller` |

In the app: SOLID › CREATE › Plugin Feature, pick the plugin. Over MCP:
`plugin.add {plugin: "foil-wing", values: {targetArea: 1050, …}}`, `plugin.set {feature,
values}`, `plugin.fields {plugin | feature}` (values by the FeatureScript's parameter names). The
desktop app and the CLI (and so its MCP server) register them at start-up
(`solidcraft_plugin_foil::register`).

## Where things are

- `src/plugins.rs`: the plugins. The schema is the FeatureScript's dialog (every field, its
  group, bounds, default, show condition, help; fields not built yet are marked `unused`). The
  geometry goes through the host's commands only: a wing or stabiliser is `solid.loft_points`
  through its stations (the foil spline and the straight trailing edge) and `solid.mirror` with
  `combine`; the propeller is a revolved hub cut for its mount, a blade lofted through its
  cylinder-wrapped sections, trimmed, copied round with `solid.move`, each copy cut where the
  mount reaches it and joined with `solid.combine`, then turned aft up. The wing publishes its area, aspect ratio, thicknesses and incidence; a linked
  stabiliser reads them.
- `src/schema_gen.rs` (the dialogs) and `src/tables_gen.rs` (the tabulated profiles and the
  B-series tables) are
  generated from the FeatureScript by `gen/*.py`; regenerate them when it changes, then
  `cargo fmt -p solidcraft-plugin-foil`.
- `src/section.rs` (NACA 4-digit on the 4-digit, 6-series a-line and reflexed mean lines, the
  tabulated profiles, the tip-section blend), `src/wing.rs` (planform solve, chord laws,
  stations, sweep, drop, gull, winglets, tubercles, washout, thickness, tip cap, minimum tip, the
  stabiliser's arm, height and link), `src/prop.rs` (the B-series blade sections and the fixed
  hub).
- Tests: `tests/planform.rs` (the planform laws station by station against the author's Python
  port of the same FeatureScript), `tests/plugins.rs` (span from area and aspect ratio, a
  symmetric one-piece wing, the stabiliser on its arm, parameters driving a wing, a linked
  stabiliser following it, options and bounds, save and reopen; the propeller one body, as long
  as its hub and nose, within its diameter).

## As WebAssembly components

The same three plugins also build as components for the WebAssembly host of
[plugins.md](plugins.md), one per plugin (`examples/wing`, `examples/stabiliser`,
`examples/propeller`: `export_plugin!` of the compiled-in plugin, and its `plugin.json`):

```sh
rustup target add wasm32-unknown-unknown
cargo xtask plugin solidcraft-plugin-foil wing   # writes target/plugins/foil-wing/
```

Copy the folder into the per-user plugins directory (`~/Library/Application
Support/SolidCraft/plugins` on macOS, `%LOCALAPPDATA%\SolidCraft\plugins` on Windows,
`~/.local/share/solidcraft/plugins` elsewhere). An app that compiles the foil plugins in keeps
those and skips the folders (same ids). `tests/wasm.rs` loads all three from folders, checks
their manifests, dialogs and notices against the compiled-in plugins, builds the 1050 cm² wing
of `tests/plugins.rs` from its component (the same span and volume as the compiled-in plugin)
and a linked stabiliser following a loaded wing.

Rebuild time on a laptop: a wing or stabiliser 1 to 3 s (73 stations: about 2.5 s); the
propeller about a minute (its blade joins), so its dialog has no live preview.

## Not built yet

These stay in the dialogs with their values (dimmed, `unused`) and are kept in designs:

- **Wing, Fuselage joint group**: tube sockets and nacelle, flat hub with pocket, flat mount,
  root fairing.
- **Stabiliser**: the mount pad, mount bolts, and the seat shift onto a wing socket's axis
  (Height is the pivot's offset here).
- **Section polar, Log inputs**: the performance report and console log only.
- **Propeller**: folding blades, the root fillet, the hub core, the operating check, the
  propeller mold, the debugging switches. A folding hub builds with fixed blades and says so.
- **Notices**: the roll response, the stabiliser's tail volume and decalage against the wing.
