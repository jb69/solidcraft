# Gear plugin

A gear studio on the plugin host of [plugins.md](plugins.md) (`crates/plugin-gear`, plugin id
`gear`): one involute gear per timeline feature, rebuilt from its values on every change. It is
the reference plugin for the WebAssembly host: the same code is compiled into an app
(`solidcraft_plugin_gear::register`) or built as a component for the plugin folders.

```sh
rustup target add wasm32-unknown-unknown
cargo xtask plugin solidcraft-plugin-gear gear   # writes target/plugins/gear/
```

Copy `target/plugins/gear/` into the per-user plugins directory and start the app; the Plugins
page of the preferences lists it and SOLID › CREATE › Plugin Feature offers it. Over MCP:
`plugin.add {plugin: "gear", values: {teeth: 24, module: 2, face_width: 10}}`.

## What it builds

| Group | Values |
|---|---|
| Teeth | kind (external, or an internal ring gear), teeth, module (normal), pressure angle (normal, 20° standard), profile shift (external), backlash (the pair's, at the pitch circle; each gear takes half) |
| Helix | helix angle (0 for spur), hand |
| Body | face width, bore (external), rim (a ring's wall outside its root circle) |
| Placement | mesh with nothing (centre, rotation of the first tooth) or with the gear feature before it (mesh direction) |
| Detail | points fitted on each flank |

- **Profile**: ISO 53 basic rack (addendum 1·m, dedendum 1.25·m), transverse values for helical
  gears (m_t = m / cos β, tan α_t = tan α / cos β). Each flank is a fit-point spline through
  points of the involute; tips and roots are arcs, joined to the flanks below the base circle
  by radial lines. The trochoid a hob would cut at the root is not drawn: an undercut is
  reported instead. A tip that would come to a point is cut back to 0.1·m thick.
- **Helical**: the outline swept along the axis with a twist of b·tan β / r (a right-hand helix
  turns anticlockwise as it rises).
- **Ring**: the tooth spaces are an external gear's teeth on the same pitch circle (tip
  r + 1.25·m, root r − m), cut from the rim disc.
- **Mesh**: a gear set to mesh reads the previous gear feature's published values
  (`published.gear`), sits at the working centre distance (a_w = a·cos α / cos α_w with
  inv α_w = inv α + 2 tan α (x1 + x2)/(z1 + z2); inside a ring (z_ring − z_pinion)·m/2) in the
  mesh direction, and is turned so its teeth sit in the other gear's spaces. Chains of gears
  follow one another; a parameter driving the module moves them all.

The notice gives the kind, teeth, module and pitch, tip and root diameters, and for a gear in
mesh the centre distance, ratio and contact ratio. Values: `pitch_diameter_mm`,
`tip_diameter_mm`, `root_diameter_mm`, `base_diameter_mm`, `tooth_thickness_mm`, and
`transverse_module`, `twist_deg`, `center_distance_mm`, `ratio`, `contact_ratio`,
`working_pressure_angle_deg` where they apply. Coded issues, each with the change that clears
it where there is one: `undercut` (the profile shift that avoids it, 1 − z·sin²α_t/(2 cos β)),
`pointed_tip`, `helix_hand`, `ring_interference` (ring and pinion closer than 10 teeth),
`low_contact_ratio` (below 1.2). Errors: teeth meeting at the root, a bore that leaves less
than a module of rim, gears that cannot mesh (module, pressure angle or helix angle differ, two
rings, a ring no bigger than its pinion), nothing to mesh with.

## Tests

Analytic values only (CONTRIBUTING rule 1):

- `tests/gear.rs` (compiled in): standard diameters; the volume against the outline's closed-form
  area (½∮r²dθ: root and tip arcs, and r_b²·t³/6 per flank in the roll parameter t = tan α_r)
  times the width, with and without a bore; the undercut issue and the shift that clears it; a
  helical gear's pitch, twist and volume; a pair, a profile-shifted pair and a pinion in a ring
  at their centre distances, with no overlap (`inspect.interference`) and an overlap once one gear
  is turned 4° off its phase; the textbook contact ratio; refusals and the helix-hand issue.
- `tests/wasm.rs`: the component loaded from a `gear/` plugin folder has the compiled-in plugin's
  manifest, dialog and notice, sends the same commands (numbers within 1e-9) and builds the same
  gear (volume within 1e-6); two loaded gears mesh through their published values and follow a
  parameter.
