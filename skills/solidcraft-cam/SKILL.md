---
name: solidcraft-cam
description: Use when machining a SolidCraft part through its MCP server or CLI - tools, setups, 2.5D / 3-axis / 4-axis / 5-axis operations, toolpath generation, stock simulation, G-code posting (cam.* commands) - or when the user asks for toolpaths, CNC, milling, roughing, finishing, drilling, a post or a machining simulation.
---

# SolidCraft CAM

## Overview
Machining is a chain of `cam.*` commands run with `execute` or `batch`: tool → setup →
operations → `cam.generate` → `cam.simulate` → `cam.post`. Every result carries a `notice`
(`status`, `values`, `issues` with stable `code`s and a `suggestion`); read it instead of
guessing, and fix issues before posting. `docs/cam.md` is the reference.

## Before the first call
1. `inspect_design` (or `measure`): know the part's bodies and bounding box. The WCS defaults to
   the design's origin and axes; z is the tool axis, so pick the setup's `wcs` so +z points out
   of the stock's top.
2. `cam.operation.kinds {}` once per session: every operation type with its parameters and
   defaults. Do not invent parameter names; an unknown one is reported as `unknown_parameter`
   with the nearest valid name. `cam.operation.<kind>` (face, pocket2d, adaptive2d, contour2d,
   slot, trace, engrave, chamfer2d, drill, tap, bore, circular, thread, pocket3d, adaptive3d,
   parallel, contour3d, steep_shallow, scallop, pencil, horizontal, spiral, radial,
   morphed_spiral, project, morph, rotary, swarf, multi_axis_contour, flow) is
   `cam.operation.add` with the type set; `setup` and `tool` default to the only setup and
   the first tool. `docs/cam-operations.md` says which to use for what. Laser work
   (`laser_cut`, `laser_engrave`) needs a laser tool (`cam.tool.kit` adds one for a machine
   with a module) and turning (`turn_face`, `turn_profile`, `turn_groove`, `turn_thread`) a
   `_4axis` machine with a cylinder stock along x.
3. `cam.machine.list` and `cam.post.list` when the machine matters (4/5-axis work needs a
   4/5-axis machine on the setup and the `fiveaxis` post). For a Makera Carvera, Carvera Air
   or Z1 put the preset on the setup (`machine: "carvera"`, `"carvera_air"`, `"z1"`, or the
   `_4axis` variant with the rotary module) and start the library with
   `cam.tool.kit {material}`: the generate and post notices then check speeds, feeds, collets,
   the work area and tool changes against the machine, and `cam.post` writes the `makera`
   dialect (`.cnc`) by itself.

4. Operations are parametric steps: changing one (or its tool, the setup or the model) makes
   it and every later one stale, and `cam.generate` regenerates only those. Reorder with
   `cam.operation.move`; read `regenerated` and `reused` in the result.
## Build
- **Tools** first: `cam.tool.add {name, kind, diameter, flute_length, overall_length, feeds}`.
  Kinds: `flat`, `ball`, `bull` (+ `corner_radius`), `chamfer`, `drill`, `spot`, `tapered_ball`.
- **Setup**: `cam.setup.create {name, stock, machine?, wcs?}`. Relative stock is the default
  (1 mm around, flush at the bottom); set `z_pos` for a facing allowance.
- **Both sides**: a second setup `cam.setup.create {name: Bottom, flip: x}` turns the part over
  (origin at the new front-left top corner) and starts from what the first left
  (`from_setup`, the default after the first setup); use `rest_machining: true` so it cuts only
  material that is left. Generate everything with `cam.generate {}` so the top regenerates first.
- **Operations** in cutting order, each with the right geometry source:
  - whole top: `face` (geometry `stock`), then `pocket3d` for roughing with `stock_to_leave_*`;
  - a pocket or boss: `pocket2d` / `adaptive2d` / `contour2d` on `faces` (a point on the
    floor or wall face), a `sketch` or explicit `loops`;
  - holes: `drill` or `bore` on `holes` (points on the cylindrical faces, or points + diameter);
  - surfaces: `parallel`, `contour3d`, `scallop`, `pencil`, `steep_shallow`;
  - 3+2: any operation with `orientation: {origin, z, x}` relative to the WCS;
  - 4-axis: `rotary` (the operation frame's x is the rotary axis; set `orientation` so it is);
  - 5-axis: `swarf` on `chains`, `multi_axis_contour`, `flow` on `rails`; use a ball tool.
- **Boundaries**: area operations machine within `boundary` (default `auto`: roughing around the
  part's outline, finishing on the part, a selection bounds it). Read the notice's `boundary`
  issue; set `boundary: "stock"` to clear the whole stock, `"selection"` with `geometry` for one
  region, `{containment: inside | center | outside, offset}` to adjust (docs/cam-boundaries.md).
  `"selection"` without picked geometry is refused. Pick faces by a point on each:
  `geometry: {source: "faces", points: [[x, y, z]]}` (a pocket floor keeps its islands).
- **The origin**: `cam.setup.set {setup, origin: [x, y, z]}` (a coordinate),
  `{origin: {point: "<construction point>"}}` or `{origin_at: {of: stock | model, x: left |
  center | right, y: front | center | back, z: top | middle | bottom}}`. Every operation is
  measured from it; put it where the machine will be zeroed (a Makera probes the stock's top
  front-left corner, the default of `origin_at`).
- **Editing clears the toolpath**: after `cam.operation.set` (other than a rename) the
  operation is not generated until `cam.generate`. `cam.operation.move {operation, setup}`
  moves it to another setup.
- **Check for gouges**: `cam.simulate` reports `gouge_cells` per operation; the visual
  simulation draws them red on the stock. Any gouge is a wrong cut: fix it before posting.
- Leave a parameter at 0 for its default. `direction: both` zigzags without retracts.
- **Change** an operation or a tool by setting its JSON directly: `cam.operation.set
  {operation, …}` or `cam.tool.set {tool, …}` with only the keys to change (a new `type` starts
  that kind's parameters at their defaults). Never open or drive the editor; it is the person's
  view of the same JSON.
- One `batch` can hold the whole chain; a failed step rolls the batch back.

## Verify
- `cam.generate {setup}`: every operation `status` must be `ok`; `no_toolpath`, `tool_too_large`
  and `tool_too_short` say what to change. Check `values.time_s` and `cutting_length_mm`
  against expectations (a pocket's area ÷ stepover gives the order of its path length).
- `cam.simulate {setup}`: `gouge_cells` and `rapid_collisions` must be 0; `rest_volume_mm3`
  is what finishing has left to do (a roughing-only program leaves `stock_to_leave` plus the
  corner fillets of the tool radius).
- Look: `cam.simulate.show {setup, progress: 1}` then `screenshot {"source":"model"}`; step
  with `progress` or `step` to watch the tool. The app switches to its Manufacture workspace
  (the MANUFACTURE tab and a panel listing tools, setups and operations) while the simulation
  shows. `cam.simulate.hide` afterwards.
- `cam.notices {}` after any design change: `stale: true` means generate again.

## Finish
`cam.post {setup, path: "part.nc"}` (post by the machine's axes; `post: grbl` for routers).
Say which post and machine the program is for, and quote the notice's `lines` and `time_s`.

## Red flags

| Thought | Do instead |
|---|---|
| "Guess the parameter name" | `cam.operation.kinds {type}`; read `unknown_parameter` issues. |
| "The geometry is the whole stock" for a pocket | Pick the floor face (`faces` with a point on it) or the sketch. |
| "Post straight after generate" | `cam.simulate` first: gouges and rapid collisions are errors. |
| "Tilted toolpath, 3-axis machine" | `cam.machine.set {setup, machine: trunnion_ac}` (or `four_axis_a`), post `fiveaxis`. |
| "The design changed, toolpaths are fine" | `cam.notices`: stale toolpaths need `cam.generate`. |
