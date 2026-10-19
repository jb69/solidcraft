# Foil plugins (this fork)

The Onshape Foil design tool's **Foil wing** and **Foil stabiliser** (FeatureScript build 246) and
the **B-Series Propeller** (build 99) as SolidCraft plugins (`crates/plugin-foil`), on the plugin
host of [plugins.md](plugins.md). Each is one parametric timeline feature holding every value of
its Onshape dialog and rebuilt from them on every change. They are fork-only: a port of a
FeatureScript checked against Onshape cannot go upstream (CONTRIBUTING, rule 1). The host
(`crates/plugin` and the `Plugin` feature) carries nothing of them.

| Plugin | Id | Onshape feature |
|---|---|---|
| Foil Wing | `foil.wing` | Foil wing |
| Foil Stabiliser | `foil.stabiliser` | Foil stabiliser |
| B-Series Propeller | `foil.propeller` | B-Series Propeller |

In the app: SOLID › CREATE › Plugin Feature, pick the plugin. Over MCP:
`plugin.add {plugin: "foil.wing", values: {targetArea: 1050, …}}`, `plugin.set {feature,
values}`, `plugin.fields {plugin | feature}` (values by the FeatureScript's names). The desktop
app and the CLI (and so its MCP server) register them at start-up
(`solidcraft_plugin_foil::register`).

## Where things are

- `src/plugins.rs`: the three plugins. The schema is the FeatureScript dialog (every field, its
  group, bounds, default, show condition, help; fields not built yet are marked `unused`). The
  geometry goes through the host's commands only: a wing or stab is `solid.loft_points` through
  its stations (the foil spline and the straight trailing edge) and `solid.mirror` with
  `combine`; the propeller is a revolved hub cut for its mount, a blade lofted through its
  cylinder-wrapped sections, trimmed, copied round with `solid.move`, each copy cut where the
  mount reaches it and joined with `solid.combine`, then turned aft up. The wing publishes its
  area, AR, thicknesses and incidence; a linked stab reads them.
- `src/schema_gen.rs` (dialogs) and `src/tables_gen.rs` (profile and B-series tables) are
  generated from the FeatureScript by `gen/*.py`; regenerate them when the FeatureScript
  changes, then `cargo fmt -p solidcraft-plugin-foil`.
- `src/section.rs` (NACA 4-digit on the 4-digit, 6-series a-line and reflexed mean lines, the
  tabulated profiles, the tip-section blend), `src/wing.rs` (`buildFoil`: planform solve, chord
  laws, stations, sweep, drop, gull, winglets, tubercles, washout, thickness, tip cap, min tip,
  the stab's arm, height and link), `src/prop.rs` (`buildBlade` and the fixed hub).
- `tests/onshape.rs`: the checks below and the feature behaviour; `tests/planform.rs`: the laws
  against the Python port.

## Checked against Onshape

The same dialog values in the Onshape tool (test copies, 2026-10-19), volume and bounds.

| Set | Onshape volume mm³ | SolidCraft | Bounds |
|---|---|---|---|
| Silk 1050 (no winglet, no tubercles) | 1,013,913.8 | −0.009 % | within 0.02 mm, span exact |
| Silk 1050 tuned (winglets up, tubercles, min tip) | 1,013,172.4 | −0.003 % | within 0.006 mm |
| U Surf 135 on a 560 arm (tubercles, cap end raised to 45 %) | 39,324.3 | −0.027 % | within 0.001 mm |
| Propeller 140 x 3, eFoil set (nose, bolted mount, no root fillet) | 112,441.3 | −0.0002 % | extents within 0.08 mm |

A blade's root cap lies on the plane through its root section's ends (Onshape caps it on the
cylinder r = 0.2R); the difference is inside the hub, so the propeller agrees.

Rebuild times on this Mac: a wing or stab 1 to 3 s (the tuned Silk, 73 stations: about 2.5 s);
the propeller about a minute (its blade joins), so its dialog has no live preview.

## Not built yet

These stay in the dialogs with their values (dimmed, `unused`) and are kept in designs:

- **Wing, Fuselage joint group**: tube sockets and nacelle, flat hub with pocket, flat mount,
  root fairing.
- **Stabiliser**: the mount pad (block, tail), mount bolts, and the seat shift onto a wing
  socket's axis (Height is the pivot's offset here).
- **Section polar, Log inputs**: the performance report and console log only.
- **Foil performance, Foil mast, Foil fuselage, Foil mold**: whole features still to port.
- **Propeller**: folding blades, the root fillet, the hub core, the operating check, the
  propeller mold, the debugging switches. A folding hub builds with fixed blades and says so.
- **Notices**: the roll response, the stab's tail volume and decalage against the wing.

## Next steps

1. The wing joints and the stab mount, which the fuselage and the molds need.
2. Foil mast and Foil fuselage as plugins.
3. Foil performance as a report (a plugin command that reads the published models).
4. Molds.
5. Speed: the propeller's blade joins.
