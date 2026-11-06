---
name: solidcraft-cam-planning
description: Use when planning a machining job for a SolidCraft part - choosing workholding, setups, tools, operation order, depths and feeds - before or while building it with cam.* commands, or when a job leaves material uncut, cuts the part loose, or stalls the spindle.
---

# Planning a CAM job

The method is in docs/cam-ai-planning.md; commands in skills/solidcraft-cam/SKILL.md.

## Plan
1. `cam.job.set {material}`, then `cam.analyze` against the machine.
2. **Workholding first.** It decides the setups and the stock. Leave a frame of stock round the
   part and a web under it; every setup clamps the frame; a contour with tabs frees the part last.
3. **Each side cuts every surface facing it to its full depth.** Only the ring round the part
   stops short, to leave the web. The turned-over side stays inside the outline
   (`boundary: {source: part, containment: inside}`).
4. **Order:** clear (largest tool) → adaptive rough (`rest_machining`) → rest rough (smaller tool)
   → flats (`horizontal`) → verticals (`contour3d`, `slope_min_deg: 80`, flat end) → steep faces
   (`contour3d` 50-80°, ball) → `parallel` on shallow areas (`slope_max_deg: 50`, boundary on
   the part) → holes → chamfers. A steep face no profile reaches gets a second `parallel` turned
   to run along it.
5. **Holes:** helix-mill (`bore`, `circular`) where a cutter fits; drill only where none does.
6. **Reach:** clear the ring wide enough for a short tool's shank and collet.

## Loads
Roughing notices carry `spindle_load`. Keep the nominal load at 50-65 % of the spindle: adaptive
corners engage harder, and a Makera Z1 stalls at about 75 %. A shallow, wide cut loads the X/Y
drives less than a deep, narrow one at the same removal rate. Full-width cuts (a slot, the first
ring of a pocket, a web cut) take the most power: step down less.

## Check
- `cam.generate`: answer every issue (`tool_too_short`, `spindle_overload`, `axis_overload`).
- `cam.simulate` each setup: 0 `gouge_cells`, 0 collisions, no `uncut` warning.
- The web cut reports `tabs_placed`; `tabs_unsupported` means tabs would meet air. Cut the web
  round the part's outline at its height: `geometry: {source: silhouette, within_heights: true}`.
- A rounded part with no flat waist gets **sprues**, not tabs: small box bodies at the edge,
  in every setup's `fixtures`, at the height where the edge is nearest; 3D operations keep
  clear of them only at their heights. Each reaches past the ring cut (offset + tool diameter
  + 1 mm); the last cut, from below with rest machining, goes up to the sprues.
- Hold with tabs or with sprues, not both, unless each holds where the other cannot (tabs on a
  flat edge, a sprue at a rounded tip). Make sprues as small as still hold: the fewest (three
  spread round the part), the narrowest that takes the finishing load, the gap's length only.
- Show the person the plan and the numbers (time, loads, what is left) before posting.

## Red flags
| Thought | Do instead |
|---|---|
| "Split the part half way for the two sides" | Each side to its full depth. |
| "No gouges, so it is done" | Also no `uncut`, every tab placed. |
| "Push the feed, it will cope" | Read `spindle_load`; stay near 50-65 %. |
