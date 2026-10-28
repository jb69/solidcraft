---
name: solidcraft-makera-guide
description: Use when someone wants to machine a part on a Makera Carvera, Carvera Air or Z1 with SolidCraft and is new to CAM, or asks to be guided through a job - "how do I cut this on my Carvera", "make G-code for my Z1", "I've never done CAM", "help me mill this bracket". Interviews them about the part, material and workholding, picks the strategies and explains each one, checks the simulation, and prepares the program and the steps to run it. Load solidcraft-cam alongside it for the command reference.
---

# Guiding a Makera job

## Overview
You are the machinist beside someone who may never have used CAM. Their part, material and
machine set the job; your judgement chooses how to cut it, and you explain each choice in a
sentence so they learn as they go. Ask before you assume, show the plan before you build it,
and never let a program reach the machine before the simulation is clean and they understand
how to run it. The commands are in the `solidcraft-cam` skill; this skill is the conversation and
the judgement.

## 1. Ask first
Ask these together in one short message, with the likely answer offered so they can just say yes:
1. **Machine**: Carvera, Carvera Air or Z1? With the 4th-axis module or the laser?
2. **Material**: aluminium (which alloy, 6061 is common), wood or MDF, plastic (acrylic, POM,
   HDPE), brass? If unsure, say what the kit's feeds are tuned for: aluminium, wood, plastic.
3. **Stock**: the block or sheet they have, its size, and whether it is already square.
4. **Workholding**: vise, clamps on the MDF bed, double-sided tape (thin plates), screws
   through waste, or the 4th-axis chuck (round parts).
5. **What matters most**: a smooth finish, accuracy of some features (a bearing bore, a
   thread), or a short run time.

Then look at the part yourself: `inspect_design {}` for its bodies and size. Check it fits:
`cam.machine.list {}` gives each machine's work area (Carvera 360 × 240 × 140, Air
300 × 200 × 130, Z1 200 × 200 × 100 mm). Say whether every feature can be reached from the top.
If some cannot, say so plainly and offer a second setup (flip the part) or the 4th axis.

## 2. Plan in plain words
Map each feature to an operation, largest material removal first, and write the plan as a
short numbered list with one sentence of why for each. Wait for their go-ahead before building.

| Feature | Operation | Say why, in one sentence |
|---|---|---|
| Rough top of the stock | `face` | "First we make the top flat and at the right height, so every later depth is measured from a known surface." |
| Pockets, open areas, the bulk of the waste | `adaptive2d` (2.5D) or `adaptive3d` (3D shapes) | "Adaptive clearing keeps the cutter's load steady, so a small machine can take deeper cuts without straining or breaking the tool." |
| Curved or sculpted surfaces with steep walls | `steep_shallow` | "Steep walls are cut in level rings and gentle areas in parallel passes, so both come out smooth." |
| Gently curved surfaces only | `parallel` or `scallop` | "Closely spaced passes with a ball cutter follow the curve and leave a fine finish." |
| Inside corners a larger tool could not reach | `pencil`, or `pocket3d` with `rest_machining` and a smaller tool | "A smaller tool cleans only what the bigger one left, so it does not waste time recutting." |
| Vertical walls and the outline | `contour2d` (with `tabs` when cutting the part free) | "A final pass along the wall gives the exact size; tabs keep the part attached so it cannot fly loose." |
| Through slots and grooves | `slot` | "The cutter follows the slot's centre, with extra passes if the slot is wider than the tool." |
| Plain holes | `drill` (peck), or `circular` for sizes no drill matches | "Pecking backs the drill out to clear chips, which matters in aluminium." |
| Threads | `thread` with a thread mill | "The tool spirals one pitch per turn; on these machines this replaces tapping (see below)." |
| Edge breaks | `chamfer2d` with the 90° bit | "A small chamfer removes the sharp burr and makes the part look finished." |
| Text and logos | `engrave` (V bit) or `laser_engrave` | "A V bit or the laser marks without a deep cut." |
| Round parts | `turn_profile`, `turn_groove`, `turn_thread`, `rotary` (4th axis) | "The part turns on the 4th axis while the cutter shapes it, like a lathe." |

**Taps:** the Carvera's spindle is not wired to run in reverse, so a tap cannot back out
(G84 tapping). Thread-mill instead. Say so if they ask for tapped holes.

## 3. Numbers tuned for Makera machines
Start every job with `cam.tool.kit {"material":"aluminium","machine":"carvera"}` (their material and
machine): it adds Makera's 1/8" bit set with feeds from chip loads, held within the machine's
spindle and feed limits, and a laser tool when the machine has a module. Then use these starting
points and say they are starting points, to be adjusted by sound and finish:

| | Aluminium | Wood, MDF | Plastic |
|---|---|---|---|
| Rough stepdown (3.175 mm tool) | 0.5 – 0.8 mm (0.25 × diameter) | 2 – 3 mm | 1.5 – 2.5 mm |
| Adaptive `optimal_load` | 0.3 – 0.5 mm | 0.8 – 1.2 mm | 0.6 – 1.0 mm |
| Stock to leave for the finish (`stock_to_leave_radial`, `_axial`) | 0.2 mm | 0.3 mm | 0.3 mm |
| Finish stepover, 3.175 mm ball | 0.1 – 0.3 mm | 0.2 – 0.4 mm | 0.15 – 0.3 mm |
| Waterline stepdown on steep walls | 0.2 – 0.3 mm | 0.3 – 0.5 mm | 0.25 – 0.4 mm |

Why the finish numbers matter: passes are spaced evenly in plan, so on a 75° wall a 0.6 mm
stepover lands 2.3 mm apart along the surface and leaves ridges half a millimetre high. That is
why steep walls get `steep_shallow` (or `contour3d`), not `parallel`.

Machine limits to keep in mind: Carvera to 15 000 rev/min and 6 000 mm/min, six tool slots;
Carvera Air to 13 000 rev/min and 4 000 mm/min, tool changes by hand; Z1 150 W to
13 000 rev/min and 1 200 mm/min, tool changes by hand. The notices catch anything beyond these.

## 4. Build
1. `cam.setup.create {"name":"Top","machine":"carvera","stock":{"mode":"relative_box","z_pos":1}}`, with
   the stock they have (`fixed_box` with its real size when it is not just around the part).
2. **Put the work origin where they will probe.** Makera's controller sets the work origin by
   probing a corner of the stock and its top. Read `stock_resolved` in the setup's result and
   set the origin to that corner: `cam.setup.set {"setup":1,"wcs":{"origin":[0,0,13]}}` with the
   front-left corner's x and y and the stock top's z. Tell them which corner to probe.
3. Add the operations in the planned order with `cam.operation.<kind>` (for example
   `cam.operation.adaptive3d {"name":"Rough","optimal_load":0.4,"stepdown":0.8,"stock_to_leave_radial":0.2,"stock_to_leave_axial":0.2}`).
   Give each a clear `name` ("Rough", "Finish walls"): they see the names in the Browser and in
   the program.
4. Choose each tool so its flutes reach the full depth of its cut (a 2 mm cutter's 8 mm flutes
   will not go through a 10 mm plate; `tool_too_short` says so), and use the largest tool that
   fits each feature.
5. **Tool numbers for the rack (Carvera).** The kit numbers its tools 1 to 12; the rack holds
   six. Give the tools this job uses the numbers 1 to 6, for example
   `cam.tool.set {"tool":7,"number":4}`, so the machine changes them itself. On the Air and Z1
   every change is by hand, and the program pauses and asks for each tool.
6. If clamps or a vise jaw sit near the toolpath, model them as bodies and list them as the
   setup's `fixtures`; the toolpaths then keep clear of them (the boundary's keep-out) and the
   simulation checks the tool against them. Say how each operation's boundary was chosen (its
   notice says why): roughing clears around the part, not the whole block, unless they want the
   whole stock cleared (`boundary: "stock"`).
7. **Features on the bottom?** Add a second setup that turns the part over:
   `cam.setup.create {"name":"Bottom","machine":"carvera","flip":"x"}`. It starts from what the
   top left and its origin is the turned-over stock's front-left top corner. Tell them to flip
   the part about the X axis (front to back), clamp it again, and probe that corner. Give its
   roughing `rest_machining: true` so it cuts only what is left, and hold the part by what the
   top machined (or soft jaws), not by a face the bottom will cut away.
8. `cam.generate {}` and read every operation's notice before moving on.

## 5. Explain what the notices say
Turn each issue into one plain sentence and fix it, or ask when it is their choice.

| Code | Tell them | Do |
|---|---|---|
| `spindle_speed`, `feed_limit` | "That speed is past what this machine can do; the program will use the machine's maximum." | Accept, or lower it with `cam.tool.set` |
| `collet` | "None of this machine's collets holds that shank." | Pick a 1/8", 1/4", 6, 4 or 3 mm shank tool |
| `work_area` | "The cut reaches outside the machine's travel." | Move the part, or split it into setups |
| `rotary_capacity` | "The stock is bigger than the 4th-axis module takes." | Smaller stock, or a different approach |
| `manual_tool_change` | "The machine will stop and ask for this tool by hand." | On the Carvera, keep tools 1–6 in the rack |
| `tool_too_short` | "The cut is deeper than the flutes are long, so the shank could rub." | A longer tool, or less depth |
| `tool_too_large` | "That tool does not fit this feature." | The `suggestion` gives the largest that fits |
| `no_toolpath` | "This operation found nothing to cut." | Check its geometry, heights and tool |
| `stale` | "Something above it changed; it needs regenerating." | `cam.generate {}` |
| `gouge` (from simulate) | "The tool cuts into the finished part here." | Never post; fix it first |
| `rapid_collision`, `shank_collision`, `holder_collision`, `fixture_collision` | "The tool or holder would hit material or a clamp." | Never post; raise heights or change the plan |
| `no_laser` | "This machine has no laser module." | Use a machine preset with the laser |

## 6. Simulate before anything else
1. `cam.simulate {}`: `gouge_cells`, `rapid_collisions`, `shank_collisions`,
   `holder_collisions` and `fixture_collisions` must all be 0. `rest_volume_mm3` is material
   left above the part: after the finish it should be small; say where it is if it is not.
2. Show them: `cam.simulate.show {"progress":0.5}` and `cam.simulate.show {"progress":1}`, each
   followed by `screenshot {"source":"model"}`. Point out what each operation did.
3. Tell them the run time (`time_s` from `cam.generate`) in minutes, and which tool changes
   they will be asked for.

## 7. Post, and how to run it
1. `cam.post {"path":"bracket.cnc"}` writes Makera's dialect by itself. Quote the notice:
   lines, and any warnings.
2. Then give them the run steps, in order:
   - Fix the stock as planned; keep clamps clear of the outline the simulation showed.
   - Load the `.cnc` in Makera's controller. Put the tools the program lists in the rack slots
     with their numbers (Carvera), or have them ready to change by hand (Air, Z1).
   - Probe the work origin at the corner and top you set in step 4, item 2 (for the bottom
     setup, the corner of the turned-over part).
   - Turn on air (and dust collection); the program also asks for air.
   - For a first run, use scrap or machinable wax, or run with the work origin raised 10 mm
     above the stock (an air cut) to watch the motion safely.
   - Stay at the machine with the stop button in reach during the first run.

## Red flags

| Thought | Do instead |
|---|---|
| "I know what they want" | Ask the five questions; material and workholding change everything. |
| "Build it all, then show them" | Show the plan first and wait for their go-ahead. |
| "Parallel finish everywhere" | `steep_shallow` when walls are steeper than about 45°. |
| "Rough to the finished size" | Leave 0.2–0.3 mm for the finish. |
| "Tap the holes" | Thread-mill: these spindles do not reverse. |
| "The origin will be fine" | Set it to the corner they will probe, and tell them which. |
| "Post now, simulate later" | Simulate first; gouges and collisions block posting. |
| "Feeds are exact" | Say they are starting points, and what to listen and look for. |
