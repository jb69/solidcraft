# Machining boundaries

An area operation (3D roughing, 3D finishing, multi-axis contour) needs to know *where* to
machine as well as *how*. The cutter-location surface keeps the tool off the part whatever
the boundary is; the boundary decides which area is worth machining. Without one, every
operation covers the whole stock: roughing clears material far from the part, and finishing
runs passes over flat stock around it.

## What CAM systems use

Survey of the concepts common to CAM packages and their public documentation (the names below
are SolidCraft's own):

| Concept | What it bounds | Typical use |
|---|---|---|
| **Stock** | the stock's outline | clearing everything, facing |
| **Part silhouette** | the outline of the part seen along the tool axis (its "shadow") | roughing around a part, finishing it without passes over the stock |
| **Bounding box** | the rectangle around the part | quick, predictable limits |
| **Selection** | picked faces, a sketch, chains or closed curves | one pocket, one surface, one region |
| **Rest material** | where earlier operations left material | clean-up with a smaller tool |
| **Steep / shallow split** | areas by surface slope | waterline on steep walls, parallel on gentle areas (SolidCraft's `steep_shallow` and the slope limits of `parallel` / `contour3d`) |
| **Keep-out areas** | clamps, vise jaws, fixtures | never driving the tool into the workholding |

Each source is combined with a **tool containment**, how the tool may sit relative to the
boundary:

- **Inside**: the whole tool stays within it. Nothing outside is touched; the very edge of the
  area may be left uncut. Right for roughing a selected pocket.
- **Center**: the tool's centre stays within it, so the whole area is machined and the tool's
  edge may reach past it by its radius. Right for finishing a surface or a selection.
- **Outside**: the tool's edge reaches past the boundary by the tool's radius, so the boundary's
  own edges are machined cleanly from outside. Right for walls and for roughing around a part.

and an **additional offset** that grows (or shrinks) the area further, for example a ring of
clearance around a part so a finishing tool has room.

Sources: Autodesk Inventor CAM documentation on boundary modes and tool containment
([parallel](https://help.autodesk.com/cloudhelp/2020/ENU/INVCAM/files/GUID-5218835D-6DB8-4426-8A76-6D2D5832B339.htm),
[contour](https://help.autodesk.com/cloudhelp/2020/ENU/INVCAM/files/GUID-069D91DB-615F-48FC-9CF1-49B6678D0FEA.htm)),
an overview of chains, boundaries and containment
([IMAGINiT](https://resources.imaginit.com/computer-aided-manufacturing/fusion-360-milling-chains-boundaries-and-containment)),
and Vectric's machining limit boundaries for 3D roughing and finishing
([3D rough toolpath](https://docs.vectric.com/docs/V12.0/Aspire/ENU/Help/form/Rough%20Machining%20Toolpath)).
Only the concepts were taken; no text, names or behaviour were copied.

## SolidCraft's boundary

Every area operation has a `boundary`:

```jsonc
"boundary": {
  "source": "auto",          // auto | stock | part | bounding_box | selection | rest
  "containment": "auto",     // auto | inside | center | outside
  "offset": 0,               // mm, grows the area after the containment (negative shrinks)
  "clip_to_stock": true,     // the tool's edge reaches the stock's edge, no further
  "avoid_fixtures": true,    // keep out of the setup's fixtures…
  "fixture_clearance": 1     // …by the tool's radius and this much
}
```

`cam.operation.set {operation, boundary: "part"}` sets the source alone.

### Auto

`auto` chooses from what the operation does and what it has to work with:

| The operation | Source | Containment | Why |
|---|---|---|---|
| anything with a selection (faces, sketch, loops, silhouette geometry), roughing | selection | inside | the whole tool stays within what was picked |
| anything with a selection, finishing | selection | center | all of what was picked is machined |
| 3D roughing (`pocket3d`, `adaptive3d`) on a part | part | outside, plus one tool radius | a ring a tool wide is cleared around the part, so walls are open for finishing, and the stock far from the part is left alone |
| wall finishing (`contour3d`, `steep_shallow`) | part | outside | the tool reaches every wall from outside |
| surface finishing (`parallel`, `scallop`, `spiral`, `radial`, `pencil`, `horizontal`, `morphed_spiral`, `project`, `morph`, `multi_axis_contour`) | part | center | no passes run over the stock around the part |
| a setup with no bodies | stock | outside | there is no part to follow |

A source that cannot be used falls back and says so: `selection` with nothing selected, `part`
without bodies, `rest` without rest machining.

The area is then clipped to the stock (grown by the tool radius) and, when the setup has
fixtures, the fixtures' outlines (grown by the tool radius and the clearance) are taken out.

### What the notice says

Every generated area operation reports its boundary:

- `boundary_source`, `boundary_containment`, `boundary_grow_mm`, `boundary_area_mm2` values;
- a `boundary` info issue in words, for example *"boundary: roughing around the part: its
  outline grown so a ring a tool wide is cleared outside it, within the stock (+1 tool
  radius)"*.

The operation editor's Boundary section shows the same sentence under the fields.
