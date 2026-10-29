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

## The setup's origin

Every operation of a setup is measured from its origin (the WCS). It is a coordinate, set in
any of three ways, all through `cam.setup.set` (or `cam.setup.create`):

- `origin: [x, y, z]`: a coordinate in design coordinates, the axes kept;
- `origin: {point: "<name>"}`: a construction point the designer placed for it;
- `origin_at: {of: stock | model, x: left | center | right, y: front | center | back, z: top
  | middle | bottom}`: a point of the stock's or the model's box in the setup's axes (default
  the stock's top front-left corner, where a Makera probes).

A fixed stock (box or cylinder, given from the origin) stays where it is when the origin
moves, unless the same call gives the stock. In the setup editor, **Place the origin** offers the
stock and model points and the design's construction points, and **Pick on Model** places it
interactively: as the pointer moves over the model it snaps to vertices, edge midpoints, circle
centres and face centres, labelled under the pointer, and the click puts the origin there.

## Machining boundaries

Area operations (3D roughing and finishing, multi-axis contour) machine within a boundary:
a source (the stock, the part's silhouette, its bounding box, the selection, or the rest
material), a tool containment (inside, center, outside), an offset, clipping to the stock and
keep-out areas around fixtures. `auto` chooses for the operation: roughing clears a ring a tool
wide around the part rather than the whole stock, wall finishing reaches the walls from
outside, surface finishing keeps to the part, and a selection bounds the operation to what was
picked. Each notice says which boundary was used and why. A boundary set to the selection
with nothing picked is refused rather than machining somewhere else. In the operation editor,
**Use Selected** takes the faces (holes for drilling kinds) or sketch profiles selected in the
viewport as the operation's geometry and sets the boundary to them; an operation started from
the ribbon with something selected starts with it. See [cam-boundaries.md](cam-boundaries.md).

## Setups in sequence: the top and the bottom

Each setup defines the stock and the origin its operations are measured from (its WCS). A
part machined on both sides has two: the first cuts the top, the second turns the part over
and cuts the bottom.

```text
cam.setup.create    {name: Top, machine: carvera, stock: {mode: relative_box, z_pos: 1}}
…operations of the top…
cam.setup.create    {name: Bottom, machine: carvera, flip: x}
…operations of the bottom (rest machining sees what the top cut)…
```

- **Stock carried over.** Every setup after the first starts, by default, from what the setups
  before it left (`stock: {mode: from_setup, setup?}`; `setup` 0 or absent is the one
  before): the earlier setup's stock is swept by its toolpaths and turned into this setup's
  WCS. Turned over along the tool axis the material is exact, column by column, with a bottom
  of its own in each column, so a pocket cut from the top is a step under the bottom's
  material and a cut from the bottom breaks through into it. A setup at another angle starts
  from the box around what is left, and says so (`stock_approximate`).
- **Rest machining** in the later setup cuts only where material is left: the top's cuts are
  air to it. Simulation and Simulate to Here start from the carried-over stock too.
- **The origin.** `flip: x` (or `y`) turns the WCS of the setup before over about its x (y)
  axis and puts the origin at the turned-over stock's front-left top corner, the corner a
  Makera probes; every operation of the setup is measured from it. `cam.setup.set {setup,
  flip}` turns an existing setup over, and the setup editor's Turn Over buttons do the same.
- **Parametric across setups.** A later setup's chain starts from the end of the earlier
  setup's: changing, reordering or regenerating the top makes the bottom's toolpaths stale,
  and `cam.generate {}` brings both current in order. Generating only the bottom uses the top's
  toolpaths as they are, with a warning when they are stale or missing.
- **Removing** the first setup hands its stock to the one after it; a setup named as another's
  source cannot be removed.

A setup opens in the same editor as operations and tools (double-click it, or Edit): name and
program, the stock (its mode and sizes, and which setup it comes from), the origin and axes
with the Turn Over buttons, the machine (a preset, then every property: work area, spindle,
feeds, tool changer, collets, rotary capacity, laser module) and the bodies and fixtures.

## Simulation

`cam.simulate` sweeps every generated operation of a setup through a Z-map of the stock
(3-axis moves cut; tilted moves are checked for shank, holder and fixture collisions) and reports
removed volume, material left above the model, gouges, and rapids through stock. During
`cam.generate` the same stock is carried from operation to operation, so an operation with
`rest_machining: true` only cuts where material is left.

`cam.simulate.show` is the visual simulator: it steps the program to a moment (`progress` 0..1,
`time_s`, an `operation` and `move`, or `step: n` moves on) and the viewport shows the stock as
cut so far, the tool where it stands, the path made and the path to come. The **Machining
Simulation** panel plays it back with a timeline, speed (1× real time to 1024×) and single
steps. `cam.simulate.hide` ends it. The simulator keeps one clock (the time each move ends,
the hop from one operation to the next included), so a moment asked for and the moment shown
agree; Play keeps its own time and takes the simulator's position only after a single step. An
operation whose eye is off in the browser draws no path in the simulation either. Wherever the
stock has been cut below the model's surface (a gouge) the simulation draws it in red on the
stock, with a post up to where the surface should be, and its notice gives `gouge_cells`.

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
definition, its tool, the setup (bodies, fixtures, WCS, stock, machine), the model's geometry,
the signature of the operation before it and the generation id of that operation's toolpath
(a fresh id each time a toolpath is generated). Editing an operation (anything but its name)
or suppressing it clears its toolpath: nothing of it is drawn and it shows as not generated
until it is generated again. Moving an operation, changing its tool or the setup, or changing
the model makes it and every operation after it stale; regenerating an operation on its own (`cam.generate {operation}`, the row's
Regenerate) makes every operation below it stale, since their rest machining read its old
toolpath; and the ones before it stay as they are. Regenerate Chain (`cam.generate {operation,
chain: true}`) brings the whole chain current through it: the stale operations above it (its
dependencies) regenerate first, then it, then every operation below it. `cam.generate` regenerates only the stale
operations, in order, sweeping the stock through the ones it keeps so rest machining sees
everything removed before it (`all: true` regenerates all; the result says how many were
regenerated and reused). `cam.operation.move` reorders (to an index, before or after another
operation, up or down) or moves an operation to another setup (`setup`, at its end unless
`to`, `before` or `after` say where), and `cam.operation.duplicate` copies an operation after
itself.

The Browser has a Manufacture folder (when the design has tools or setups), grouped by machine:
each machine holds its setups, each setup its operations numbered in order, and the machine's
Tools (the whole library when there is one machine; with several, the tools each one's
operations use, unused ones under the first). Each operation row is coloured by its state
(stale and warnings in amber, errors in red, suppressed dimmed). Dragging an operation onto
another one puts it before that one, in the same setup or another; dropping it on a setup puts
it at the setup's end. A new operation from the ribbon goes into the setup picked in the
Browser (or the picked operation's setup), and opens in the operation editor before it is
added: the same two tabs, with a Setup choice; OK adds it (`cam.operation.add`). In the Manufacture workspace the viewport
draws every generated, enabled operation's toolpath on the workpiece (feed moves in the accent
colour, rapids faint); an operation's eye shows or hides its toolpath and a setup's eye all of
its operations' (`cam.toolpath.visibility {operation | operations | setup | all, visible}`,
view state that is neither saved nor undone). Suppressing, which leaves the operation out of
generation, simulation and posting, is in the menu. Double-click (or Edit in the menu) opens the operation editor, with two tabs over one draft
of the operation. **Fields** is the friendly view: name, type, tool and enabled, then sections
for the kind's own parameters (fixed choices as dropdowns, lengths in mm, angles in degrees),
stock to leave, orientation, geometry (its source, and the picked points and loops as rows to
edit, add or remove), the five heights (what each is measured from and its offset) and
linking (ramp, leads, stay-down). The form is built from the operation's saved form and its
kind's defaults, so it covers every value of every kind. **JSON** is the advanced view: the
same draft as the text `cam.operation.set` takes, to read, copy or paste. Edits carry across
when switching tabs; JSON that does not parse keeps its tab and blocks Apply. Apply and OK run
`cam.operation.set` with the draft as one undo step; a refusal keeps the draft and says why.
When the operation changes elsewhere (an assistant, undo), an untouched draft follows it and an
edited one offers Reload. A tool in the Browser's Tools folder opens in the same editor: its Fields tab has the
name, tool number and length offset, the kind with the kind's own sizes (corner radius, tip
angle, wavelength…), the size (diameter, flute length, shank, stick-out, holder, flutes) and
the feeds and speeds, and says how many operations use the tool; Apply runs `cam.tool.set`.
Assistants never use the editor: they set the JSON directly with `cam.operation.set` and
`cam.tool.set`. The menu has Edit,
Regenerate, Regenerate Chain, Simulate to Here, Move Up and Down, Duplicate, Show/Hide Toolpath, Suppress and
Delete, and a setup's menu Generate, Regenerate All, Simulate and Post Process.

Generating from the ribbon, the panel or the Browser runs off the window thread: the window
prepares each operation (its geometry and meshes) and a worker computes the toolpaths in
order, sweeping the stock between them. The Browser animates the operation being computed
(a spinning arc on its row, its setup's and the Manufacture folder's, the name in the accent
colour), shows the ones still waiting as queued, and takes each toolpath in as it finishes; the
status line reports how many were generated and how many were unchanged. A second Generate
while one runs is refused. `cam.generate` from a script, the CLI or an agent still runs to the
end and returns its result.

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
- `skills/solidcraft-makera-guide`: how an agent guides someone new to CAM through a job on a
  Makera machine (the interview, strategy choices explained, Makera-tuned starting numbers,
  notices in plain words, the simulation review and the run steps).
