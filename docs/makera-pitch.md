# SolidCraft CAM for Makera machines: a proposal (draft)

*A pitch to Makera for a test machine. Draft of 28 October 2026.*

## In one paragraph

SolidCraft is an open-source, pure-Rust parametric CAD application that an AI assistant can
drive end to end. Its new CAM module plans 2.5-axis, 3-axis, continuous 4-axis and 5-axis
toolpaths, simulates the stock as it is cut, and writes G-code, and it treats the AI assistant as
a first-class user: every step is a JSON command with a self-describing result, and every
result carries a notice that says what went well, what was clamped and what to change. We have
made Makera's Carvera, Carvera Air and Z1 the first machines the module knows by name, with
their work areas, spindles, feeds, tool changers, collets and rotary modules, a post that writes
Makera's dialect, and a tool kit tuned to the machines. What we lack is a machine to cut with.
We are asking Makera for a Carvera with the 4th-axis module to test against, and in return we
offer an open CAD-to-CAM-to-G-code path for Makera's machines that people and their AI
assistants can use for free.

## What exists today

- **CAD**: sketches with a constraint solver, a parametric feature timeline, a B-rep kernel,
  assemblies, sheet metal, rendering, STEP import and export, a desktop app, a CLI and an MCP
  server. Everything a user can do is a command, so an AI assistant does the same work the same way.
- **CAM** (new, in the `cam` branch): facing, 2D pocket, adaptive, contour with tabs, chamfer,
  drill cycles and bore; 3D roughing with rest machining, parallel, waterline, scallop, pencil,
  spiral, radial, morphed spiral, projected, horizontal and steep-and-shallow finishing; 3+2
  indexed work; continuous 4-axis rotary; 5-axis swarf, multi-axis contour and flow. Own
  implementations of 2D offsetting, height maps, an exact drop cutter in any tool-axis frame,
  linking (leads, helix and profile ramps, stay-down), machine kinematics with inverse
  kinematics and linearisation, a Z-map stock simulation with gouge and collision reports, and
  a visual simulator that plays the program back with the stock removed move by move.
- **Makera support**: presets `carvera`, `carvera_air` and `z1` (each with a `_4axis`
  variant), the `makera` post, machine checks and the `makera_essential` tool kit. Details below.
- **Feedback loop for assistants**: every command result carries `{status, info, values,
  issues}`; an issue has a stable code, a message, the parameter and its limit, and a
  suggestion the assistant can apply. Unknown parameter names come back with the nearest valid one.

## Why Makera

- Makera's machines are the desktop mills most likely to sit next to the people who use AI
  assistants to design parts, and the gap between "I have a model" and "I have a program that
  runs on my Carvera" is the part they find hardest.
- Makera publishes its machines' specifications and documents its control's commands, which is
  what let us build this without touching anything proprietary.
- A 4th-axis module on a Carvera is the simplest real continuous 4-axis setup there is, and
  our rotary toolpaths and simulator are built for exactly that.

## What the Makera support does

| | Carvera | Carvera Air | Z1 |
|---|---|---|---|
| Work area | 360 × 240 × 140 mm | 300 × 200 × 130 mm | 200 × 200 × 100 mm |
| Spindle | 200 W, 0–15 000 rev/min | 200 W, 0–13 000 rev/min | 150 W, 0–13 000 rev/min |
| Travel speed | 6 000 mm/min | 4 000 mm/min | 1 200 mm/min |
| Tool changer | 6-slot automatic | quick change by hand | quick change by hand |
| Collets | 1/8" (1/4", 6, 4, 3 mm optional) | 1/8" (1/4", 6, 4 mm optional) | 1/8" (1/4", 6, 4, 3 mm optional) |
| Rotary module | Ø92 × 240 mm | Ø92 × 200 mm | Ø80 × 150 mm |
| Gantry clearance | 120 mm | 120 mm | 115 mm |

Figures from Makera's product pages and Makera's resellers' specification sheets; they are
machine specifications, not promises about finished parts. We will correct any Makera tells us
are wrong.

With a Makera preset on a setup:

- **Checks** on every generate and post: spindle speed or feed past the machine's (the post
  writes the maximum and says so), a shank no collet takes, a toolpath wider than the work
  area, cuts past the rotary module's capacity, and tool changes beyond the six slots (or every
  change on the Air and Z1), each as an issue the assistant can act on.
- **The `makera` post** writes the control's dialect as Makera documents it: millimetres only,
  `M6 T` tool changes with the control measuring the tool length itself, `M7`/`M9` for the air
  blast, `G28` to park, the `A` word for the rotary module, no canned cycles, `;` comments,
  `M2`, saved as `.cnc`.
- **The tool kit** adds Makera's 1/8" bit set (single-flute flat end mills, ball end mills, a
  90° chamfer bit, a 30° engraving bit and drills in 3.175, 2 and 1 mm) with feeds from chip
  loads for aluminium, wood or plastic, scaled for the small cutters and held within the
  machine's limits, plus a starting stepdown.
- **The simulator** shows the stock being cut by the program before it is sent, and reports
  gouges, rapids through stock and holder collisions, so a first-time user sees a crash on
  screen rather than on the machine.

An assistant's whole job on a Carvera is six commands: add the kit, create a setup with
`machine: "carvera"`, add operations, generate, simulate, post. Each answer tells it what to fix.

## What we are asking for

1. **A Carvera with the 4th-axis module** (or a Carvera Air with its module, if the Carvera is
   not possible) on loan or at cost, to run our programs on real stock and feed what we learn
   back into the posts, the feeds and the checks.
2. **Ten minutes of an engineer's time** to confirm the dialect details we took from the
   documentation: the tool-change and parking conventions, how the control treats the rotary
   feed, and anything the Z1 does differently.
3. **Permission to say "tested on a Makera Carvera"** once it is.

## What Makera gets

- An open-source CAD and CAM path to its machines, under MIT or Apache-2.0, with no licence
  fees for its customers and no dependency on anyone's subscription.
- Machines that AI assistants can program safely: the checks, the simulator and the feedback
  loop are built for an assistant that has never seen a mill.
- Test coverage: the Makera presets, post and kit are covered by automated tests that run on
  every change, and we will add Makera-specific regression tests from anything the machine
  teaches us.
- Public credit in the documentation, the release notes and the app.

## What we will do first with the machine

1. Cut the test block the simulator already runs (face, 3D rough, 2D pocket, outside contour,
   drill) in machinable wax, then aluminium, and compare the part with the model.
2. Run the rotary test (a continuous 4-axis finish of a turned profile) on the 4th-axis module
   and confirm the rotation direction and feed behaviour.
3. Measure what the machine does with the kit's feeds and set them from measurements.
4. Publish the programs, the measurements and the results.

## Where to look

- Source: the `cam` branch of `github.com/jb69/solidcraft` (`crates/cam`, `docs/cam.md`).
- Try it without a machine: the desktop app's Manufacture workspace, or the MCP server with the
  `solidcraft-cam` skill, which walks an assistant through a Carvera job.

## Open questions for Makera

- Does the stock firmware accept arbitrary large `A` values on continuous rotary jobs, or should
  the post wrap them (as the community post does)?
- Is `G28` the right park before a tool change on all three machines, and is `M2` the preferred end?
- Does the Z1 share the Carvera's command set in full?
- Which tools, collets and holders does Makera want the kit to carry, and at which feeds?
