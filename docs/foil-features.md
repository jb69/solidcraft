# Foil features

Parametric ports of the Onshape Foil design tool (FeatureScript build 246) and the B-Series
propeller generator (build 99). One timeline feature per Onshape feature, holding every value
of its dialog and rebuilt from them on every evaluation, as the FeatureScript is.

| Feature | Command | Onshape feature |
|---|---|---|
| Foil wing | `foil.wing_feature` | Foil wing |
| Foil stabiliser | `foil.stabiliser_feature` | Foil stabiliser |
| B-Series Propeller | `foil.propeller_feature` | B-Series Propeller |

`foil.set {feature, values}` (or `timeline.edit {feature, set: {values: {...}}}`) changes values;
`foil.fields {tool | feature}` lists every field with its group, bounds, default, options,
FeatureScript show condition and help. In the app the features sit under SOLID › CREATE and
their dialogs follow Onshape's: the same groups and order, a field shown when Onshape shows it,
the FeatureScript's description as its tooltip, and the feature's notice (area, span, AR,
chords, MAC, winglet, raised ratios) worked out live above the fields.

## Where things are

- `crates/foil`: the laws, no geometry kernel. `schema_gen.rs` (dialogs) and `tables_gen.rs`
  (profile and B-series tables) are generated from the FeatureScript by `crates/foil/gen/*.py`;
  regenerate them when the FeatureScript changes, then `cargo fmt -p solidcraft-foil`.
  `section.rs` (NACA 4-digit on the 4-digit, 6-series a-line and reflexed mean lines, the
  tabulated profiles, the tip-section blend), `wing.rs` (`buildFoil`: planform solve, chord
  laws, stations, sweep, drop, gull, winglets, tubercles, washout, thickness, tip cap, min tip,
  the stab's arm, height and link), `prop.rs` (`buildBlade` and the fixed hub).
- `crates/doc/src/foil.rs`: the stored values (`FoilParams`: every number an expression and a
  feature input, so parameters drive it and it gets a `dN` name), and the solids:
  `kernel::loft_beziers` through the stations, mirrored and joined; the blade wrapped onto its
  cylinders, trimmed, cut and joined to the hub.
- `crates/kernel/src/build.rs`: `loft_beziers`, a smooth loft through sections given as 3D
  Bézier chains, one face per section edge (a foil: the surface and the trailing edge).
- `vendor/truck-geometry`: B-spline surfaces evaluate from their non-zero basis functions only.
- `crates/ui-egui/src/dialogs_foil.rs`: the dialogs.

## Checked against Onshape

The same dialog values in the Onshape tool (test copies, 2026-10-19), volume and bounds; the
tests are in `crates/engine/src/cmd/foil_tests.rs` and `crates/doc/src/foil.rs`.

| Set | Onshape volume mm³ | SolidCraft | Bounds |
|---|---|---|---|
| Silk 1050 (no winglet, no tubercles) | 1,013,913.8 | −0.009 % | within 0.02 mm, span exact |
| Silk 1050 tuned (winglets up, tubercles, min tip) | 1,013,172.4 | −0.003 % | within 0.006 mm |
| U Surf 135 on a 560 arm (tubercles, cap end raised to 45 %) | 39,324.3 | −0.027 % | within 0.001 mm |
| Propeller 140 x 3, eFoil set (nose, bolted mount, no root fillet) | 112,441.3 | +0.001 % | extents within 0.02 mm |
| Single blade (Blade test, valid) | 5,716.6 | +3.5 % | within 0.06 mm but the root |

The single blade's extra volume is its root cap: Onshape's loft caps the root section on its
cylinder (r = 0.2R), this one on the plane through the section's ends, up to 2.4 mm further in.
On a propeller that stub is inside the hub, so the propeller's volume agrees.

Rebuild times on this Mac: a wing or stab 1 to 3 s (the Silk 1050 with winglets and tubercles
has 73 stations: 2.6 s), the propeller about a minute (its blade joins). The propeller has no
live preview in the dialog for that reason.

## Not built yet

These stay in the dialogs with their values (dimmed, with a note in the tooltip) and are kept
in saved designs, so a design set up now builds them when they are.

- **Wing, Fuselage joint group**: round and square tube sockets with their nacelle, the flat hub
  with pocket (standard and custom, nose fairing, V floor, nut inserts), the flat mount with its
  pad and bolts, the root fairing.
- **Stabiliser**: the mount pad (and block, tail), the mount bolts and countersinks, and the
  seat shift that puts the stab on a wing socket's axis (Height is the pivot's offset here).
- **Section polar, Log inputs**: they feed the performance report and the console log only.
- **Foil performance, Foil mast, Foil fuselage, Foil mold**: whole features still to port.
- **Propeller**: folding blades (pins, bosses, tongue, tail relief, fold trim), the root
  fillet, the hub core, the operating check, the propeller mold, and the sections-only and
  keep-sections debugging switches. A folding hub builds with fixed blades and says so.
- **Notices**: the roll response (vortex lattice), the stab's tail volume and decalage against
  the wing, the propeller's operating check.

## Next steps

1. The wing joints and the stab mount, which the fuselage and the molds need.
2. Foil mast and Foil fuselage.
3. Foil performance (lifting line, polars, trim, the speed sweep) as a report command.
4. Molds (the wing mold and the propeller mold).
5. Speed: the propeller's blade joins (most of its minute) and the root cap on the cylinder.
