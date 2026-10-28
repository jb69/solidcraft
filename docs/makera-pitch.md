# SolidCraft CAM for Makera machines: a proposal (draft)

*A request to Makera for test machines, from an independent contributor to the open-source
SolidCraft project. Draft of 28 October 2026.*

## In one paragraph

SolidCraft is a public, open-source (MIT or Apache-2.0), pure-Rust parametric CAD application
that an AI assistant can drive end to end, developed in the open at
`github.com/tangency-labs/solidcraft`. I am building a CAM module for it, on my own time, to
contribute to that repository: it plans 2D, 3D, rotary and multi-axis toolpaths, turning on the
rotary axis and laser work, simulates the stock as it is cut, and writes G-code. Makera's
Carvera, Carvera Air and Z1 are the first machines it knows by name: their work areas,
spindles, feeds, tool changers, collets, rotary modules and laser modules, a post for Makera's
dialect, and a tool kit for the machines. It is at an early stage and none of it has run on a
Makera machine yet. I am asking Makera to lend (or sell at cost) the machines to test it on. In
return every Makera owner gets a free, open CAD-to-G-code path for intricate work, tested on
real hardware and maintained in public, that people and their AI assistants can use.

## Who I am and why

I am an independent contributor, not a company and not speaking for the SolidCraft
maintainers: I write the code and offer it to the project, and it joins the main repository
when its maintainers have reviewed and accepted it. There is no commercial interest behind this.

I like Makera's machines: an enclosed, desktop mill with an automatic tool changer, probing, a
rotary module and a laser is a remarkable thing to have on a bench. Makera Studio's guided modes
are a good way in, and they get a first part cut quickly. Where I want more is intricate work:
rest machining that knows what earlier operations removed, a choice of 3D finishing strategies
for steep and shallow areas, pencil passes in corners, turning and threads on the rotary axis,
true multi-axis toolpaths, and a simulation that shows the stock, the tool and every move before
the machine runs. Professional CAM packages offer that depth but cost a subscription and are not
built around these machines. This module aims to give Makera owners that depth in free, open
software that knows their machine, so the hardware can do everything it is capable of.

## What exists today

- **CAD**: sketches with a constraint solver, a parametric timeline, a B-rep kernel,
  assemblies, sheet metal, STEP import and export, a desktop app, a CLI and an MCP server.
- **CAM** (in development, to be contributed to the main repository): a Manufacture workspace
  whose ribbon has a button per operation kind, grouped by category, and a Manufacture folder in
  the Browser where each setup's operations are parametric steps (changing one regenerates it
  and the ones after it):

| Category | Operations |
|---|---|
| 2D milling | face, 2D pocket, 2D adaptive, 2D contour (with tabs), slot, trace, engrave (V bit), 2D chamfer |
| Drilling | drill (G81, G82, G83, G73, G85), tap (G84), bore, circular, thread milling |
| 3D milling | 3D adaptive, 3D pocket (with rest machining), parallel, 3D contour (waterline), steep and shallow, scallop, pencil, horizontal, spiral, radial, morphed spiral, project, morph; ramp and geodesic are planned |
| Multi-axis | rotary (continuous 4-axis), swarf, multi-axis contour, flow, 3+2 indexing of any 2D or 3D kind; multi-axis blend is planned |
| Turning on the A axis | turn face, turn profile, turn groove, turn thread (the part turns on A, a vertical cutter cuts it) |
| Laser | laser cut (kerf offset, passes) and laser engrave (raster) |

  Turning and laser are the newest and still being finished; the whole module is early-stage.
- **Simulation**: a Z-map of the stock cut move by move, with gouge, rapid-through-stock and
  holder collision reports, and a visual player in the viewport.

## The Makera support

| | Carvera | Carvera Air | Z1 |
|---|---|---|---|
| Work area | 360 × 240 × 140 mm | 300 × 200 × 130 mm | 200 × 200 × 100 mm |
| Spindle | 200 W, to 15 000 rev/min | 200 W, to 13 000 rev/min | 150 W, to 13 000 rev/min |
| Maximum feed | 6 000 mm/min | 4 000 mm/min | 1 200 mm/min |
| Tool changer | 6-slot automatic | by hand (quick change) | by hand (quick change) |
| Collets | 1/8" (1/4", 6, 4, 3 mm optional) | 1/8" (1/4", 6, 4 mm optional) | 1/8" (1/4", 6, 4, 3 mm optional) |
| 4th-axis module | Ø92 × 240 mm | Ø92 × 200 mm | Ø80 × 150 mm |
| Gantry clearance | 120 mm | 120 mm | 115 mm |
| Laser | 2.5 W, built in | 5 W module | 5 W module |

These are Makera's published figures, as SolidCraft's presets carry them (`carvera`,
`carvera_air`, `z1`, each with a `_4axis` variant). They are machine specifications, not claims
about finished parts. I will correct any that Makera tells me are wrong.

With a Makera preset on a setup:

- **Checks** run on every generate and post: spindle speed or feed past the machine's (the post
  writes the maximum and says so), a shank no collet takes, a toolpath larger than the work
  area, cuts beyond the 4th-axis module's capacity, tool changes beyond the six slots (or any
  change on the Air and Z1), and a laser program on a machine without a laser.
- **The `makera` post** writes the dialect as Makera documents it: millimetres only, `M6 T`
  tool changes with the control measuring tool length (no `G43`), `M7`/`M9` air blast, `G28`
  park, the `A` word for the 4th axis, no canned cycles (drilling and tapping are expanded),
  `;` comments, `M2`, saved as `.cnc`. Laser mode: `M321` to enter, `G0 Z0` to the focus, no Z
  words, `S` as a 0..1 fraction of the module's power, `M7` for air assist, `M9` and `M322` to
  leave.
- **The `makera_essential` kit** adds Makera's 1/8" bit set (flat and ball end mills, a 90°
  chamfer bit, a 30° engraving bit, drills) with feeds for aluminium, wood or plastic, held
  within the machine's limits, plus a laser tool for the machine's module when it has one.

## Test plan

Each test is a small part programmed in SolidCraft, simulated, posted with the `makera` post
and cut. Every test also compares the cut part with the simulator's stock.

| Category | Machine and module | Test part | Passes when |
|---|---|---|---|
| 2D milling | Carvera (ATC, multi-tool job) | plate with pocket, island, slot, contour with tabs, chamfer, V-engraving | pocket and contour dimensions within ±0.05 mm; tool changes run unattended |
| Drilling | Carvera | hole pattern: drilled, pecked, bored, circular-milled, tapped and thread-milled holes | hole diameters within ±0.05 mm; M3 / M4 threads take a GO gauge and refuse a NO-GO |
| 3D milling | Carvera | 3D test block: adaptive rough, then parallel, waterline, scallop and pencil finishes | surface within ±0.1 mm of the model; finish (Ra) and scallop height near the predicted value |
| Multi-axis | Carvera + 4th-axis module | rotary wrap and multi-axis contour on round stock | profile within ±0.1 mm; A direction and rotary feed as the simulator shows |
| Turning | Carvera + 4th-axis module | stepped shaft: turn face, profile, groove, thread | diameters within ±0.05 mm; groove width and position within ±0.1 mm; an external thread takes its nut or ring gauge |
| Laser | Carvera (2.5 W built in); Air or Z1 (5 W module) | cut-out and raster test card in plywood, card, acrylic | cut dimensions within ±0.1 mm of the kerf-compensated path; a power and speed grid that sets the kit's defaults |
| Machine limits | Carvera Air and Z1, with 4th-axis modules | the same parts scaled to fit | checks catch every over-limit case; manual tool changes prompt as expected; feeds clamped to 4 000 and 1 200 mm/min |

I will measure with calipers, a micrometer, thread gauges and a surface roughness gauge, and
publish every program, measurement and photo. Tolerances above are my targets; I will agree
them with Makera before cutting.

## What I ask for

| Tier | Machines | What it covers |
|---|---|---|
| **Ideal** | a Carvera with the 4th-axis module, plus a Carvera Air or Z1 with its laser and 4th-axis modules | every row of the test plan, on two machines |
| **Minimum** | one Carvera with the 4th-axis module | milling, drilling, rotary, turning, threads, and the 2.5 W built-in laser |

Either tier on **loan** for the test period (I return it at my cost) or **at cost**. I will
cover materials, tooling wear and measuring equipment. I also ask for:

- **One short call with an engineer** to answer the open questions below.
- **Permission to say "tested on a Makera Carvera"** (and Air or Z1) once the tests pass.

## What Makera gets

- **Open-source CAD and CAM that targets Makera first**, in a public project under MIT or
  Apache-2.0, with no licence fees or subscriptions for its customers. Once accepted, the
  Makera presets, post and kit live in the main repository, so anyone can read, fix and extend
  them, and they are maintained in public with the rest of SolidCraft rather than in a private
  fork.
- **An AI-assistant-first workflow.** Every step is a JSON command with a self-describing
  result and a notice: what went well, what was clamped, what to change, with a stable code
  and a suggested fix. A whole Carvera job is six commands: add the kit, create a setup with
  `machine: "carvera"`, add operations, generate, simulate, post.
- **Posts tested on Makera's dialect**, with automated regression tests built from what the
  machines teach me, run by the project's CI on every change to the main repository.
- **Published results**: a public report with programs, measurements and photos, and credit
  in the module's documentation (and in SolidCraft's release notes and app, if its maintainers
  agree).

## Timeline

| Weeks | Work |
|---|---|
| 1–2 | 2D milling and drilling on the Carvera; feeds set from measurements |
| 3–4 | 3D finishing, rotary, turning and threads on the 4th-axis module |
| 5 | Laser cutting and engraving; the Air or Z1 limit tests |
| 6 | Fixes contributed to the main repository; public report published |

## Where to look

- Project: `github.com/tangency-labs/solidcraft`, the public SolidCraft repository the CAM
  module is being contributed to.
- Work in progress: the `cam` branch of `github.com/jb69/solidcraft` (`crates/cam`,
  `docs/cam.md`, `docs/cam-operations.md`) until it is merged upstream.
- Try it without a machine: the desktop app's Manufacture workspace, or the MCP server with the
  `solidcraft-cam` skill, which walks an assistant through a Carvera job.

## Open questions for Makera

Milling and rotary:

- Does the stock firmware accept large `A` values on continuous rotary jobs, or should the post
  wrap them?
- Is `G28` the right park before a tool change on all three machines, and is `M2` the preferred end?
- Does the Z1 share the Carvera's command set in full?
- Can the spindle reverse for tapping (expanded G84), or should taps be thread-milled only?
- Which tools, collets and holders should the kit carry, and at which feeds?

Laser:

- Is `S` a 0..1 fraction of the module's power on all three machines (2.5 W and 5 W alike)?
- Is the focus set by `M321` and then `G0 Z0` on every machine, and should the program write no
  Z words in laser mode?
- Does air assist use `M7` / `M9` in laser mode, as in milling?
- Which feed and power ranges does Makera recommend for each module and material?

---

[Your name] · [email or other contact] · independent contributor to SolidCraft
