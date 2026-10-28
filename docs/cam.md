# CAM (manufacturing)

SolidCraft computes milling toolpaths for the parts it models: 2.5-axis (facing, pocketing,
adaptive clearing, contouring, chamfering, drilling, boring), 3-axis surface work (Z-level
roughing, parallel, waterline, scallop, pencil, spiral, radial, morphed spiral, project,
horizontal, steep and shallow), indexed 3+2 (any operation with a tool orientation), continuous
4-axis (rotary) and 5-axis (swarf, multi-axis contour, flow); the full list with each package's name for it is in `docs/cam-operations.md`. It simulates the stock, carries
the material each operation removes into the next one (rest machining), checks for gouges and
collisions, and writes G-code through machine kinematics (3-axis, 4-axis, table-table,
head-head, with or without tool-centre-point control).

Everything is a `cam.*` command, so the toolbar, scripts, the CLI, the control channel and the
MCP server reach the same thing. The commands are written for an AI assistant first: plain JSON
in, the computed numbers out, and a **notice** on every result (`status`, `info`, `values`,
`issues` with stable codes and suggestions), the shape the design feedback block uses.

## Model

- **Tools** (`cam.tool.*`): kind (flat, ball, bull, chamfer, drill, spot, tapered ball),
  diameter, corner radius or tip angle, flute and overall length, shank and holder diameters,
  feeds and speeds. The design's library.
- **Setups** (`cam.setup.*`): the bodies to machine, fixtures to avoid, the work coordinate
  system (origin, z and x in design coordinates), the stock (box or cylinder, relative to the
  model or fixed), the machine (`cam.machine.set`: `three_axis`, `four_axis_a`, `trunnion_ac`,
  `trunnion_bc`, `head_head_bc`, or a full description with axis limits and TCP).
- **Operations** (`cam.operation.*`): `type` (see `cam.operation.kinds` for every kind with its
  parameters and defaults), `tool`, `geometry`, `heights`, `linking`, `tolerance`, stock to
  leave, rest machining and an optional `orientation` for 3+2 work. A parameter at 0 takes the
  kind's default (40 % of the diameter for a roughing stepover, 10 % for finishing).
- **Geometry** sources: `stock` (the stock outline), `silhouette` (the model seen along the tool
  axis), `loops` (explicit polygons), `sketch` (a sketch's profiles), `faces` (points on faces:
  planar faces give loops and the floor height, cylindrical faces give holes), `holes` (points
  with a diameter, or on cylindrical faces), `chains` (bottom and top of a wall, for swarf),
  `rails` (two curves, for flow).
- **Heights**: clearance, retract, feed, top and bottom, each `{from, offset}` with `from` one
  of `stock_top`, `stock_bottom`, `model_top`, `model_bottom`, `origin`, `absolute`, or a number
  (absolute). A 2D pocket on picked faces ends at the face.
- **Linking**: ramp (`plunge`, `helix`, `zigzag`, `profile` with angle and helix diameter),
  lead-in and lead-out arcs, stay-down distance, minimum retract.

The model is saved in the design (`manufacture`); toolpaths are not: `cam.generate` recomputes
them and keeps them in the session.

## Workflow

```text
cam.tool.add        {name, kind: flat, diameter: 6}
cam.setup.create    {name: Top, stock: {mode: relative_box, z_pos: 1, x_neg: 1, …}}
cam.operation.add   {setup, type: face, tool, direction: both}
cam.operation.add   {setup, type: pocket3d, tool, stepdown: 2, stock_to_leave_radial: 0.2}
cam.operation.add   {setup, type: pocket2d, tool, geometry: {source: faces, points: [[28, 15, 6]]}}
cam.operation.add   {setup, type: drill, tool: drill, cycle: peck, geometry: {source: holes, points: [[13, 10, 5]]}}
cam.generate        {setup}          → each operation's notice and stats
cam.simulate        {setup}          → removed, rest, gouges, collisions
cam.simulate.show   {setup, progress: 0.5}   → the stock and the tool in the viewport
cam.post            {setup, path: part.nc}   → G-code
```

`cam.notices` lists every operation's report (with `stale: true` once the design changed);
`cam.inspect` says what exists and what needs attention; `cam.toolpath` returns the moves.

## Simulation

`cam.simulate` sweeps every generated operation of a setup through a Z-map of the stock
(3-axis moves cut; tilted moves are checked for shank, holder and fixture collisions) and reports
removed volume, material left above the model, gouges, and rapids through stock. During
`cam.generate` the same stock is carried from operation to operation, so an operation with
`rest_machining: true` only cuts where material is left.

`cam.simulate.show` is the visual simulator: it steps the program to a moment (`progress` 0..1,
`time_s`, an `operation` and `move`, or `step: n` moves on) and the viewport shows the stock as
cut so far, the tool where it stands, the path made and the path to come. The **Machining
Simulation** panel plays it back with a timeline, speed and single steps. `cam.simulate.hide`
ends it.

## The Manufacture workspace

CAM lives in the **Manufacture** workspace (the workspace switcher at the top left, or
`ui.workspace {"workspace": "manufacture"}` over the control channel), not in Design. Its
MANUFACTURE ribbon tab carries the commands in seven panels: SETUP (Create Setup, Add Tool,
Tool Kit, Set Machine), 2D MILLING, 3D MILLING, DRILLING and MULTI-AXIS (a button per
operation kind, see `docs/cam-operations.md`), TOOLPATHS (Generate Toolpaths, Show Simulation,
Simulate, Post Process) and INSPECT (Inspect Manufacturing, CAM Notices, Operation Kinds). A
button for a kind opens the command box with `cam.operation.<kind>` filled in (its parameters
at their defaults and the geometry it reads); `setup` and `tool` default to the only setup and
the library's first tool. A panel on
the right lists the tools, setups and operations with each one's status and issues, with
Generate, Simulate and Post buttons. The buttons for commands that take parameters open the
command box with a filled-in call to edit and run (there are no CAM dialogs yet); the others run
at once, and `cam.simulate`, `cam.simulate.show` and `cam.post` take the design's only setup when
`setup` is left out. Post Process asks where to save the `.nc` file. A simulation shown from a
script or the agent switches to the workspace by itself, and leaving the workspace ends it.

## Makera machines (Carvera, Carvera Air, Z1)

CAM stays generic, and the Makera desktop machines are first-class: presets `carvera`,
`carvera_air` and `z1`, each with a `_4axis` variant for the maker's rotary module, carry the
published work area, gantry clearance, spindle range and power, maximum travel speed, tool
changer (6-slot automatic on the Carvera, by hand on the Air and Z1), the collet sizes and the
rotary module's stock capacity (`cam.machine.list` shows every number). With one of them on a
setup:

- `cam.generate` and `cam.post` check every tool and toolpath against the machine: a spindle
  speed or feed past the machine's (`spindle_speed`, `feed_limit`: the post writes the maximum
  and says so), a shank no collet takes (`collet`), a toolpath wider than the work area
  (`work_area`, an error) or the rotary module's capacity (`rotary_capacity`), and tools beyond
  the changer's slots or every change on a manual machine (`manual_tool_change`).
- `cam.post` picks the `makera` post by itself. It writes the control's own dialect as Makera
  documents it: millimetres only (`G21 G90 G94 G17`), `M6 T<n>` tool changes with the control
  measuring the tool length itself (no `G43`), `M7`/`M9` for the air blast, `G28` to park, the
  `A` word for the 4th-axis module, no canned cycles (drilling is expanded), `;` comments and
  `M2`. Programs are `.cnc` files for Makera's controller software.
- `cam.tool.kit {kit: makera_essential, material: aluminium | wood | plastic}` adds Makera's
  1/8" bit set (single-flute flat end mills, ball end mills, a 90° chamfer bit, a 30° engraving
  bit and drills in 3.175, 2 and 1 mm) with feeds from chip loads for the material, scaled for
  the small cutters and held within the machine's spindle and feed limits, and a starting
  stepdown per diameter in the notice.

The Carvera's rotary module turns about X (`A`), continuous; Makera's firmware limits the rotary
feed itself, so the post writes ordinary feeds. The firmware's manual-change prompt handles tools
beyond the six slots, and all tools on the Air and Z1. Probing and work offsets are done on
the machine (the controller's probing cycles set `G54`), not in the program.

Laser: each preset carries its module (`laser`: the Carvera's built-in 2.5 W, the 5 W modules
of the Air and Z1; 445 nm). The kit adds a laser tool (`kind: laser`, the spot as its
diameter). `laser_cut` follows curves with the kerf offset to a side in passes; `laser_engrave`
fills the regions inside loops with raster lines, then their outline. Moves carry the beam's
power (0..1); the `makera` post enters laser mode with `M321`, puts the beam at its focus with
`G0 Z0`, writes no Z words, writes power as `S0.5`, air assist as `M7`, and leaves with `M9` and
`M322`. The `grbl` post uses `M4` with `S` 0..1000. A laser operation on a machine without a
module is a `no_laser` error; the stock simulation leaves laser work alone.

Turning on the rotary axis (a 4-axis machine; the part turns on A while a vertical cutter
cuts it like a lathe tool): `turn_face` faces the part's end back in steps, `turn_profile` is
the model's profile along the axis, `turn_groove` cuts a groove of a width and depth, and
`turn_thread` cuts a helix of the pitch over a length in passes (equal area per pass), the
tool's form giving the thread's (a 60° V bit for a V thread). Thread milling of holes and
bosses on a 3-axis machine is `thread`.

## Parametric operations in the Browser

A setup's operations are a chain of parametric steps. Each has a signature that hashes its own
definition, its tool, the setup (bodies, fixtures, WCS, stock, machine), the model's geometry
and the signature of the operation before it, so editing, suppressing or moving an operation,
changing its tool or the setup, or changing the model makes it and every operation after it
stale, and leaves the ones before it as they are. `cam.generate` regenerates only the stale
operations, in order, sweeping the stock through the ones it keeps so rest machining sees
everything removed before it (`all: true` regenerates all; the result says how many were
regenerated and reused). `cam.operation.move` reorders (to an index, before or after another
operation, up or down) and `cam.operation.duplicate` copies an operation after itself.

The Browser has a Manufacture folder (when the design has tools or setups): each setup with its
operations numbered in order, each row coloured by its state (stale and warnings in amber,
errors in red, suppressed dimmed) and the Tools. The eye suppresses an operation;
double-click opens it in the command box as `cam.operation.set` with its current parameters;
the menu has Edit, Regenerate, Simulate to Here, Move Up and Down, Duplicate, Suppress and
Delete, and a setup's menu Generate, Regenerate All, Simulate and Post Process.

## Post processors

`makera` (Makera Carvera, Carvera Air and Z1, see above), `generic` (RS-274 for most 3-axis controls: tool changes, length offsets, canned drilling
cycles), `grbl` (routers: no tool changer or canned cycles) and `fiveaxis` (A/B/C words through
the machine's kinematics; `G43.4` TCP when the machine has it, else machine coordinates with the
moves linearised to the machine's tolerance). `cam.post.list` lists them.

## Notice codes

`no_toolpath`, `tool_too_large`, `tool_too_short`, `below_bottom`, `tilt_limit`,
`shank_collision`, `holder_collision`, `fixture_collision`, `rapid_collision`, `gouge`,
`unknown_parameter` (with the nearest valid name), `machine_axes` (the machine has fewer axes
than the operation needs), `not_generated`, `sketch_not_level`, `face_not_level`,
`hole_not_aligned`, `generation_failed`.

## Where things live

- `crates/cam`: the algorithms (geometry only): offsetting, height maps, the drop cutter in any
  frame, linking, operations, kinematics, simulation, posts, the stored model.
- `crates/engine/src/cmd/cam.rs`: the commands, geometry resolution from bodies, faces and
  sketches, the session's toolpath cache and simulation preview.
- `crates/ui-egui/src/manufacture_workspace.rs`: the Manufacture workspace (ribbon tab, panel,
  button behaviour); `crates/ui-egui/src/cam_panel.rs` and the viewport: the simulation display
  and playback.
- `skills/solidcraft-cam`: how an agent machines a part.
