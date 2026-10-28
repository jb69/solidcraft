# CAM operation types

What milling CAM packages offer, grouped the way the Manufacture ribbon groups them, and what
SolidCraft has. The survey covers Fusion's Manufacture workspace (its 2D, 3D, drilling and
multi-axis strategies, from Autodesk's help and blog), Mastercam (2D dynamic and classic
toolpaths, 3D high-speed roughing and finishing), SolidCAM (2.5D, iMachining, HSR/HSM),
FreeCAD's CAM workbench and Makera Studio. Names differ between packages; the table uses the
most common one and lists the others.

Each operation is a `cam.operation.<kind>` command on the MANUFACTURE tab (a button on its
category's panel) and a `type` of `cam.operation.add`.

## 2D milling (2.5-axis: the tool follows curves, depth by levels)

| Operation | Also called | SolidCraft | Notes |
|---|---|---|---|
| Face | Facing | `face` | raster passes over the stock top; `direction: both` for zigzag |
| 2D Pocket | Pocket, Pocket Shape | `pocket2d` | contour-parallel rings, islands, finishing pass |
| 2D Adaptive | Dynamic Mill, iMachining 2D, Adaptive | `adaptive2d` | inside-out rings at the optimal load, trochoids |
| 2D Contour | Profile, Contour | `contour2d` | outside / inside / on, roughing and finishing passes, tabs |
| Slot | Slot Mill, Peel Mill | `slot` | centre on the curve; offset passes across a wider slot |
| Trace | Engrave (end mill) | `trace` | tool centre along curves in depth passes |
| Engrave | V-Carve, Vcarve | `engrave` | V tool tip along curves; `width` sets the depth |
| 2D Chamfer | Chamfer, Deburr | `chamfer2d` | chamfer mill along edges |
| Bore | Helix, Circular (helical) | `bore` | helical bore of a round hole |
| Circular | Circle Mill | `circular` | helix in, concentric circles out to the wall, per level |
| Thread | Thread Mill | `thread` | single-point thread mill helix, radial passes, internal or external |

## 3D milling (surface-following, 3-axis)

| Operation | Also called | SolidCraft | Notes |
|---|---|---|---|
| 3D Adaptive | Adaptive Clearing, Dynamic OptiRough, iMachining 3D | `adaptive3d` | Z-level roughing with adaptive rings and trochoids |
| 3D Pocket | Pocket Clearing, Offset Roughing, HSR contour | `pocket3d` | Z-level roughing with contour-parallel rings, flat-area levels, rest machining |
| Parallel | Raster, Linear | `parallel` | passes at an angle following the surface, slope limits |
| 3D Contour | Waterline, Z-level finishing | `contour3d` | the surface's level contours at each stepdown |
| Steep and Shallow | Hybrid | `steep_shallow` | waterlines on steep areas, parallel on shallow |
| Scallop | Constant stepover, Equidistant | `scallop` | passes at equal surface distance |
| Pencil | Corner cleanup, Rest finishing | `pencil` | passes where the tool touches two faces |
| Horizontal | Flat, Flat areas | `horizontal` | finds flat areas and clears each at its level |
| Spiral | | `spiral` | outward spiral on the surface |
| Radial | | `radial` | rays from a centre on the surface |
| Morphed Spiral | | `morphed_spiral` | spiral morphed from the boundary |
| Project | Projection, Curve projection | `project` | curves projected onto the surface |
| Morph | Blend, Morph between curves | `morph` | passes morphed between two rails, on the surface |
| Ramp | | planned | waterlines that ramp between levels |
| Geodesic | | planned | constant geodesic distance over free-form surfaces |

## Drilling

| Operation | Also called | SolidCraft | Notes |
|---|---|---|---|
| Drill | Drilling cycles | `drill` | G81, G82 (dwell), G83 (peck), G73 (chip break), G85 (ream / feed out) |
| Tap | Tapping, Rigid tap | `drill` with `cycle: tap` (`cam.operation.tap`) | G84 at the pitch feed; expanded on controls without cycles |
| Bore | Boring | `bore` | listed under drilling too |
| Circular | | `circular` | |
| Thread | | `thread` | |

Spot drilling is `drill` with a spot drill tool; counterboring is `circular` to the counterbore
depth. Hole recognition (every cylindrical face as a hole) is the `holes` geometry with
`cylindrical faces`.

## Multi-axis

| Operation | Also called | SolidCraft | Notes |
|---|---|---|---|
| Rotary | 4-axis rotary, Wrap | `rotary` | continuous 4-axis around the operation frame's x axis |
| Swarf | Flank milling | `swarf` | the tool's side along a wall between two chains |
| Multi-Axis Contour | 5-axis contour, Surface-normal finishing | `multi_axis_contour` | passes with the axis on the normal, lead and tilt |
| Flow | Flowline, Morph (5-axis) | `flow` | passes morphed between rails, surface-normal |
| Multi-Axis Blend | 5-axis blend | planned | surface-normal passes blended between two boundaries |
| 3+2 (indexed) | Positional 5-axis | any 2D/3D kind with `orientation` | the tool axis set per operation |

## Not milling

## Turning (on the rotary axis)

Lathe work done on a mill's 4th axis: the part turns on A, the vertical cutter acts as the
lathe tool. Fusion and Mastercam have these in their turning and mill-turn products
(face, profile/OD turning, groove, thread); Makera Studio has 4-axis relief.

| Operation | Also called | SolidCraft | Notes |
|---|---|---|---|
| Turn Face | Facing | `turn_face` | the part's end faced back in steps |
| Turn Profile | OD turning, Profile | `turn_profile` | the model's profile along the axis |
| Turn Groove | Grooving, Part-off | `turn_groove` | a groove of a width and depth at a position |
| Turn Thread | Thread turning, Single-point threading | `turn_thread` | helix of the pitch, passes of equal area; the tool's form is the thread's |

## Laser

| Operation | Also called | SolidCraft | Notes |
|---|---|---|---|
| Laser Cut | Vector cut, Line | `laser_cut` | curves with the kerf offset to a side, in passes |
| Laser Engrave | Fill, Raster | `laser_engrave` | raster fill of regions, then the outline; image (grey level) engraving is planned |

## Threads

Threads are cut three ways: `thread` mills internal or external threads with a thread mill on
a 3-axis machine; `drill` with `cycle: tap` taps them (G84); `turn_thread` turns them on the
rotary axis.

Water-jet cutting, probing cycles and additive are other categories in the larger packages
and are not part of this module.

## Sources

Autodesk Fusion help and blog (2D toolpaths; 3D adaptive, offset roughing, steep and shallow,
parallel, contour and ramp; manufacturing extension comparison), Mastercam training guides
(dynamic milling, surface high speed), SolidCAM module overviews (2.5D, iMachining, HSR/HSM),
the FreeCAD CAM workbench wiki (operations list) and the Makera CAM user guide.
