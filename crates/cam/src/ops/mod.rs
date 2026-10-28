//! The operations: each a parameter struct in [`OpKind`] and a generator that turns the
//! operation's resolved geometry into a [`Toolpath`] with a [`Notice`].
//!
//! Every generator works in the operation's own frame: z is the tool axis, the stock and the
//! model meshes have been taken into that frame, and the heights are numbers. [`generate`]
//! dispatches; the families live in [`two_d`] (facing, pocketing, contouring, adaptive
//! clearing, drilling, boring, chamfering), [`three_d`] (Z-level roughing, parallel, waterline,
//! scallop, pencil, spiral, radial, morphed spiral, project, horizontal, steep and shallow) and
//! [`multi_axis`] (rotary, swarf, multi-axis contour, flow).

pub mod laser;
pub mod multi_axis;
pub mod three_d;
pub mod turning;
pub mod two_d;

use serde::{Deserialize, Serialize};
use solidcraft_geom::{Aabb2, Aabb3, Vec2, Vec3};

use crate::dropcutter::DropMesh;
use crate::frame::Frame;
use crate::heightmap::HeightMap;
use crate::linking::ResolvedHeights;
use crate::notice::{Issue, Notice, r3};
use crate::offset;
use crate::polyline::{self, Poly};
use crate::setup::{Operation, StockShape};
use crate::tool::Tool;
use crate::toolpath::Toolpath;
use crate::{CamError, Result};

/// Milling direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Climb,
    Conventional,
    /// Alternate (zigzag): no retract between passes.
    Both,
}

/// Which side of a contour the tool runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// Tool outside the loop (a boss, the part's outline).
    #[default]
    Outside,
    /// Tool inside the loop (a pocket wall).
    Inside,
    /// Tool centre on the loop (a slot, engraving a line).
    On,
}

/// The kind of an operation with its parameters. Lengths are mm, angles degrees.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OpKind {
    /// Clears the top of the stock with parallel passes.
    Face {
        #[serde(default)]
        stepover: f64,
        #[serde(default)]
        stepdown: f64,
        #[serde(default)]
        angle_deg: f64,
        #[serde(default)]
        direction: Direction,
        /// How far the passes run past the outline (default: the tool radius).
        #[serde(default)]
        extension: f64,
    },
    /// Contour-parallel pocketing of closed loops (holes in the loops are islands).
    Pocket2d {
        #[serde(default)]
        stepover: f64,
        #[serde(default)]
        stepdown: f64,
        #[serde(default)]
        direction: Direction,
        /// Cut the innermost ring first and work out (true) or the wall first (false).
        #[serde(default = "yes")]
        inside_out: bool,
        /// A final pass along the wall at the finish feed with this radial stock removed.
        #[serde(default)]
        finishing_stepover: f64,
    },
    /// Engagement-controlled clearing: contour-parallel passes from an inside start at a
    /// stepover no larger than `optimal_load`, corners blended to `minimum_radius`, and
    /// trochoidal loops where the full width would be engaged.
    Adaptive2d {
        #[serde(default)]
        optimal_load: f64,
        #[serde(default)]
        stepdown: f64,
        #[serde(default)]
        minimum_radius: f64,
        #[serde(default)]
        direction: Direction,
    },
    /// Profiling along closed or open loops with the tool on one side.
    Contour2d {
        #[serde(default)]
        side: Side,
        #[serde(default)]
        stepdown: f64,
        #[serde(default)]
        direction: Direction,
        #[serde(default)]
        roughing_passes: u32,
        #[serde(default)]
        roughing_stepover: f64,
        #[serde(default)]
        finishing_passes: u32,
        #[serde(default)]
        finishing_stepover: f64,
        /// Tabs: count per loop (0: none), width and height.
        #[serde(default)]
        tabs: u32,
        #[serde(default)]
        tab_width: f64,
        #[serde(default)]
        tab_height: f64,
    },
    /// A chamfer along loops with a chamfer mill: `width` across the chamfer, the tool tip
    /// `tip_offset` below the chamfer's lower edge.
    Chamfer2d {
        #[serde(default)]
        width: f64,
        #[serde(default)]
        tip_offset: f64,
        #[serde(default)]
        side: Side,
    },
    /// A slot along open chains or loops: the tool centre follows the curve; a slot wider than
    /// the tool is cut with offset passes across its width, from the centre out.
    Slot {
        /// Slot width (0: the tool's diameter, one pass on the curve).
        #[serde(default)]
        width: f64,
        #[serde(default)]
        stepdown: f64,
        #[serde(default)]
        direction: Direction,
    },
    /// Tracing: the tool centre follows the selected curves (open chains or loops) down to the
    /// bottom height in depth passes.
    Trace {
        #[serde(default)]
        stepdown: f64,
        #[serde(default)]
        direction: Direction,
    },
    /// Engraving with a V (chamfer or engraving) tool: the tip follows the curves; `width`
    /// sets the depth from the tool's angle (0: the bottom height).
    Engrave {
        #[serde(default)]
        width: f64,
        #[serde(default)]
        stepdown: f64,
    },
    /// Circular milling of round holes larger than the tool: a helix into the centre, then
    /// concentric circles out to the wall at each depth level.
    Circular {
        #[serde(default)]
        stepover: f64,
        #[serde(default)]
        stepdown: f64,
        #[serde(default)]
        direction: Direction,
    },
    /// Drilling cycles at hole positions.
    Drill {
        #[serde(default)]
        cycle: crate::toolpath::DrillCycleKind,
        #[serde(default)]
        peck: f64,
        #[serde(default)]
        dwell_s: f64,
        /// Thread pitch for the tapping cycle (mm per turn).
        #[serde(default)]
        pitch: f64,
        /// Extra depth past the bottom height (through holes).
        #[serde(default)]
        break_through: f64,
        /// Order holes by nearest neighbour (true) or as selected.
        #[serde(default = "yes")]
        optimize_order: bool,
    },
    /// Helical boring of round holes with an end mill: the tool centre follows a helix of
    /// `pitch` per turn to the bottom, then a flat finishing circle.
    Bore {
        #[serde(default)]
        pitch: f64,
        #[serde(default)]
        direction: Direction,
    },
    /// Z-level roughing of a 3D model between the stock and the surface, contour-parallel at
    /// each level (also the roughing used for 3D adaptive clearing).
    Pocket3d {
        #[serde(default)]
        stepover: f64,
        #[serde(default)]
        stepdown: f64,
        #[serde(default)]
        direction: Direction,
        /// Add levels at the model's flat floors so they are cut clean.
        #[serde(default = "yes")]
        flat_area_detection: bool,
        /// Machine each region to the bottom before the next (true) or each level across the
        /// part (false).
        #[serde(default)]
        order_by_depth: bool,
    },
    /// Parallel finishing: straight passes at an angle, following the surface.
    Parallel {
        #[serde(default)]
        stepover: f64,
        #[serde(default)]
        angle_deg: f64,
        #[serde(default)]
        direction: Direction,
        /// Only machine where the surface slope is in this range (degrees from horizontal).
        #[serde(default)]
        slope_min_deg: f64,
        #[serde(default = "ninety")]
        slope_max_deg: f64,
    },
    /// Waterline finishing: level contours of the surface at each stepdown.
    Contour3d {
        #[serde(default)]
        stepdown: f64,
        #[serde(default)]
        direction: Direction,
        #[serde(default)]
        slope_min_deg: f64,
        #[serde(default = "ninety")]
        slope_max_deg: f64,
        #[serde(default = "yes")]
        flat_area_detection: bool,
    },
    /// Constant stepover along the surface: passes at equal surface distance from the boundary.
    Scallop {
        #[serde(default)]
        stepover: f64,
        #[serde(default)]
        direction: Direction,
    },
    /// Cleans internal corners and fillets the previous tool left: passes where the tool
    /// touches two faces meeting at an inside angle.
    Pencil {
        /// Smallest inside angle between faces to trace (degrees between normals).
        #[serde(default)]
        min_angle_deg: f64,
        #[serde(default)]
        passes: u32,
        #[serde(default)]
        stepover: f64,
    },
    /// An outward spiral from a centre, following the surface.
    Spiral {
        #[serde(default)]
        stepover: f64,
        #[serde(default)]
        center: Option<Vec2>,
        #[serde(default)]
        max_radius: f64,
    },
    /// Radial passes from a centre.
    Radial {
        #[serde(default)]
        angle_step_deg: f64,
        #[serde(default)]
        center: Option<Vec2>,
        #[serde(default)]
        inner_radius: f64,
    },
    /// A spiral morphed from the boundary's shape, following the surface.
    MorphedSpiral {
        #[serde(default)]
        stepover: f64,
    },
    /// Projects the selected loops onto the surface.
    Project {},
    /// Detects flat areas and clears each at its level.
    Horizontal {
        #[serde(default)]
        stepover: f64,
        #[serde(default)]
        direction: Direction,
    },
    /// Steep areas by waterline, shallow by parallel passes, split at `threshold_deg`.
    SteepShallow {
        #[serde(default)]
        threshold_deg: f64,
        #[serde(default)]
        steep_stepdown: f64,
        #[serde(default)]
        shallow_stepover: f64,
        #[serde(default)]
        angle_deg: f64,
        #[serde(default = "yes")]
        steep_first: bool,
    },
    /// Continuous 4-axis finishing: the tool points at the rotary axis (the operation frame's
    /// x axis) and the surface is machined around it.
    Rotary {
        /// Passes along the axis (true) or around it (false).
        #[serde(default = "yes")]
        along_axis: bool,
        /// Stepover: angle between passes around (degrees) when `along_axis`, else distance
        /// between circles along the axis.
        #[serde(default)]
        stepover: f64,
        #[serde(default)]
        angle_from_deg: f64,
        #[serde(default = "full_turn")]
        angle_to_deg: f64,
        /// Tool tips never go below this radius from the axis.
        #[serde(default)]
        inner_radius: f64,
    },
    /// Swarf: the side of the tool along a wall between a bottom and a top chain.
    Swarf {
        /// Tool on the left (true) or right of the bottom chain's direction.
        #[serde(default)]
        left: bool,
        /// Depth passes along the tool axis (1: the whole wall at once).
        #[serde(default = "one")]
        passes: u32,
        /// Tip below the bottom chain by this much.
        #[serde(default)]
        extension: f64,
        #[serde(default)]
        max_tilt_deg: f64,
    },
    /// Multi-axis contour: parallel passes with the tool axis following the surface normal,
    /// leaned by `lead_deg` along the feed and `tilt_deg` across it.
    MultiAxisContour {
        #[serde(default)]
        stepover: f64,
        #[serde(default)]
        angle_deg: f64,
        #[serde(default)]
        lead_deg: f64,
        #[serde(default)]
        tilt_deg: f64,
        #[serde(default = "ninety")]
        max_tilt_deg: f64,
        #[serde(default)]
        direction: Direction,
    },
    /// Flow: passes morphed between two rails, following the surface with the tool axis along
    /// the surface normal.
    Flow {
        #[serde(default)]
        stepover: f64,
        #[serde(default)]
        lead_deg: f64,
        #[serde(default)]
        tilt_deg: f64,
        #[serde(default = "ninety")]
        max_tilt_deg: f64,
    },
    /// Thread milling with a single-point thread mill: a helix of the thread's pitch up the
    /// hole (or around a boss when `external`), in radial passes to the full thread depth.
    Thread {
        /// Thread pitch, mm per turn (required).
        #[serde(default)]
        pitch: f64,
        /// Radial passes to the full depth.
        #[serde(default = "one")]
        passes: u32,
        /// An external thread around a boss (the hole positions are the boss centres and the
        /// diameter the major diameter).
        #[serde(default)]
        external: bool,
    },
    /// 3D adaptive clearing: Z-level roughing with engagement-controlled rings and trochoids at
    /// each level (the 2D adaptive motion on the surface's free region).
    Adaptive3d {
        #[serde(default)]
        optimal_load: f64,
        #[serde(default)]
        stepdown: f64,
        #[serde(default)]
        minimum_radius: f64,
        #[serde(default)]
        direction: Direction,
        #[serde(default = "yes")]
        flat_area_detection: bool,
    },
    /// Morph: 3-axis passes morphed between two rails, following the surface.
    Morph {
        #[serde(default)]
        stepover: f64,
        #[serde(default)]
        direction: Direction,
    },
    /// Laser cutting along curves (geometry: sketch, loops or faces), the kerf offset to the
    /// chosen side, in passes.
    LaserCut {
        /// Beam power, 0..1 of the module's.
        #[serde(default = "full")]
        power: f64,
        /// Speed, mm/min.
        #[serde(default = "laser_cut_speed")]
        speed: f64,
        #[serde(default = "one")]
        passes: u32,
        #[serde(default = "side_on")]
        side: Side,
        /// Kerf width (0: the laser tool's spot diameter).
        #[serde(default)]
        kerf: f64,
        #[serde(default = "yes")]
        air: bool,
    },
    /// Laser engraving: raster fill of the regions inside the loops, then their outline.
    LaserEngrave {
        #[serde(default = "half")]
        power: f64,
        #[serde(default = "laser_engrave_speed")]
        speed: f64,
        /// Distance between raster lines (0: the spot diameter).
        #[serde(default)]
        line_interval: f64,
        #[serde(default)]
        angle_deg: f64,
        #[serde(default = "yes")]
        bidirectional: bool,
        #[serde(default = "yes")]
        outline: bool,
        #[serde(default = "yes")]
        air: bool,
    },
    /// Turning on the rotary axis: facing the part's end back to `position` (0: the model's
    /// end) in steps of `stepdown`.
    TurnFace {
        #[serde(default)]
        stepdown: f64,
        #[serde(default)]
        position: f64,
    },
    /// Turning on the rotary axis: the model's profile, as `rotary` along the axis.
    TurnProfile {
        #[serde(default)]
        stepover: f64,
    },
    /// Turning on the rotary axis: a groove at `position`, `width` wide (0: the tool) and
    /// `depth` deep from the stock's radius.
    TurnGroove {
        #[serde(default)]
        position: f64,
        #[serde(default)]
        width: f64,
        #[serde(default = "two")]
        depth: f64,
        #[serde(default)]
        stepdown: f64,
        #[serde(default)]
        stepover: f64,
    },
    /// Turning on the rotary axis: a thread of `pitch` from `start` over `length` (0: the
    /// part's extent), `depth` below the stock's radius (0: a 60° form), in `passes`.
    TurnThread {
        #[serde(default = "one_mm")]
        pitch: f64,
        #[serde(default)]
        depth: f64,
        #[serde(default = "three")]
        passes: u32,
        #[serde(default)]
        start: f64,
        #[serde(default)]
        length: f64,
        #[serde(default)]
        left_hand: bool,
    },
}

fn yes() -> bool {
    true
}
fn one() -> u32 {
    1
}
fn three() -> u32 {
    3
}
fn two() -> f64 {
    2.0
}
fn one_mm() -> f64 {
    1.0
}
fn full() -> f64 {
    1.0
}
fn half() -> f64 {
    0.5
}
fn laser_cut_speed() -> f64 {
    300.0
}
fn laser_engrave_speed() -> f64 {
    2000.0
}
fn side_on() -> Side {
    Side::On
}
fn ninety() -> f64 {
    90.0
}
fn full_turn() -> f64 {
    360.0
}

impl OpKind {
    /// The kind's name as the `type` tag.
    pub fn type_name(&self) -> &'static str {
        match self {
            OpKind::Face { .. } => "face",
            OpKind::Pocket2d { .. } => "pocket2d",
            OpKind::Adaptive2d { .. } => "adaptive2d",
            OpKind::Contour2d { .. } => "contour2d",
            OpKind::Chamfer2d { .. } => "chamfer2d",
            OpKind::Drill { .. } => "drill",
            OpKind::Bore { .. } => "bore",
            OpKind::Pocket3d { .. } => "pocket3d",
            OpKind::Parallel { .. } => "parallel",
            OpKind::Contour3d { .. } => "contour3d",
            OpKind::Scallop { .. } => "scallop",
            OpKind::Pencil { .. } => "pencil",
            OpKind::Spiral { .. } => "spiral",
            OpKind::Radial { .. } => "radial",
            OpKind::MorphedSpiral { .. } => "morphed_spiral",
            OpKind::Project {} => "project",
            OpKind::Horizontal { .. } => "horizontal",
            OpKind::SteepShallow { .. } => "steep_shallow",
            OpKind::Rotary { .. } => "rotary",
            OpKind::Swarf { .. } => "swarf",
            OpKind::MultiAxisContour { .. } => "multi_axis_contour",
            OpKind::Flow { .. } => "flow",
            OpKind::Thread { .. } => "thread",
            OpKind::Slot { .. } => "slot",
            OpKind::Trace { .. } => "trace",
            OpKind::Engrave { .. } => "engrave",
            OpKind::Circular { .. } => "circular",
            OpKind::Adaptive3d { .. } => "adaptive3d",
            OpKind::Morph { .. } => "morph",
            OpKind::LaserCut { .. } => "laser_cut",
            OpKind::LaserEngrave { .. } => "laser_engrave",
            OpKind::TurnFace { .. } => "turn_face",
            OpKind::TurnProfile { .. } => "turn_profile",
            OpKind::TurnGroove { .. } => "turn_groove",
            OpKind::TurnThread { .. } => "turn_thread",
        }
    }

    /// Every kind, for listings.
    pub const NAMES: &'static [&'static str] = &[
        "face",
        "pocket2d",
        "adaptive2d",
        "contour2d",
        "slot",
        "trace",
        "engrave",
        "chamfer2d",
        "drill",
        "bore",
        "circular",
        "thread",
        "pocket3d",
        "adaptive3d",
        "parallel",
        "contour3d",
        "scallop",
        "pencil",
        "spiral",
        "radial",
        "morphed_spiral",
        "project",
        "horizontal",
        "steep_shallow",
        "rotary",
        "swarf",
        "multi_axis_contour",
        "flow",
        "morph",
        "laser_cut",
        "laser_engrave",
        "turn_face",
        "turn_profile",
        "turn_groove",
        "turn_thread",
    ];

    /// A kind's parameters at their defaults, as saved (`type` and every parameter), or `None`
    /// for an unknown kind.
    pub fn defaults(ty: &str) -> Option<serde_json::Value> {
        let kind: OpKind = serde_json::from_value(serde_json::json!({ "type": ty })).ok()?;
        serde_json::to_value(kind).ok()
    }

    /// The fixed-choice parameters of a kind (direction, side, drill cycle…) and the values each
    /// takes, in declaration order.
    pub fn choices(ty: &str) -> Vec<(String, Vec<String>)> {
        let Some(serde_json::Value::Object(d)) = OpKind::defaults(ty) else { return Vec::new() };
        let mut out = Vec::new();
        for (k, v) in &d {
            if k == "type" || !v.is_string() {
                continue;
            }
            let mut probe = d.clone();
            probe.insert(k.clone(), serde_json::Value::String(PROBE.into()));
            let names = variants_in(serde_json::from_value::<OpKind>(serde_json::Value::Object(probe)).err());
            if !names.is_empty() {
                out.push((k.clone(), names));
            }
        }
        out
    }

    /// Axes the kind needs: 3 for 2.5D and 3D work (3+2 through the orientation), 4 for rotary,
    /// 5 for swarf and surface-normal work.
    pub fn axes(&self) -> usize {
        match self {
            OpKind::Rotary { .. } | OpKind::TurnFace { .. } | OpKind::TurnProfile { .. } | OpKind::TurnGroove { .. } | OpKind::TurnThread { .. } => 4,
            OpKind::Swarf { .. } | OpKind::MultiAxisContour { .. } | OpKind::Flow { .. } => 5,
            _ => 3,
        }
    }

    /// Whether the operation reads the model's surface (needs meshes).
    pub fn needs_model(&self) -> bool {
        !matches!(
            self,
            OpKind::Face { .. }
                | OpKind::Pocket2d { .. }
                | OpKind::Adaptive2d { .. }
                | OpKind::Contour2d { .. }
                | OpKind::Chamfer2d { .. }
                | OpKind::Drill { .. }
                | OpKind::Bore { .. }
                | OpKind::Swarf { .. }
                | OpKind::Thread { .. }
                | OpKind::Slot { .. }
                | OpKind::Trace { .. }
                | OpKind::Engrave { .. }
                | OpKind::Circular { .. }
                | OpKind::LaserCut { .. }
                | OpKind::LaserEngrave { .. }
                | OpKind::TurnFace { .. }
                | OpKind::TurnGroove { .. }
                | OpKind::TurnThread { .. }
        )
    }
}

/// The geometry selection resolved into the operation frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedGeometry {
    /// Closed loops in XY (normalized: outers CCW, holes CW).
    pub loops: Vec<Poly>,
    /// Open chains in XY (for contours with `On`).
    pub open: Vec<Vec<Vec2>>,
    /// The height of the selected faces' plane, when faces were selected.
    pub face_z: Option<f64>,
    /// Hole positions (top centre) and diameter.
    pub holes: Vec<Vec3>,
    pub hole_diameter: f64,
    /// Chains for swarf and rails for flow (3D, operation frame).
    pub chain_a: Vec<Vec3>,
    pub chain_b: Vec<Vec3>,
}

/// Everything a generator reads.
pub struct Context<'a> {
    pub tool: &'a Tool,
    /// The bodies to machine, in the operation frame.
    pub model: Option<&'a DropMesh>,
    /// Fixtures to avoid, in the operation frame.
    pub fixtures: Option<&'a DropMesh>,
    pub model_bounds: Aabb3,
    pub stock: StockShape,
    /// Frame of the operation relative to the stock's (WCS) frame, so stock outlines and ranges
    /// can be taken in the operation frame.
    pub frame: Frame,
    pub heights: ResolvedHeights,
    pub geometry: ResolvedGeometry,
    pub tolerance: f64,
    pub stock_to_leave_radial: f64,
    pub stock_to_leave_axial: f64,
    /// The stock left by the previous operations, as a height map in the operation frame, for
    /// rest machining (`None`: machine the full stock).
    pub rest: Option<&'a HeightMap>,
}

impl Context<'_> {
    /// The stock outline in the operation frame.
    pub fn stock_outline(&self) -> Vec<Poly> {
        polyline::normalize_set(&self.stock.outline_in(&self.frame, self.tolerance))
    }

    pub fn stock_z(&self) -> (f64, f64) {
        self.stock.z_range_in(&self.frame)
    }

    /// The model's extent along the tool axis.
    pub fn model_z(&self) -> (f64, f64) {
        if self.model_bounds.is_empty() { self.stock_z() } else { (self.model_bounds.min.z, self.model_bounds.max.z) }
    }

    /// The tool with the radial and axial stock to leave folded in: a tool of larger radius
    /// (same corner radius) dropped `stock_to_leave_axial` lower.
    pub fn virtual_tool(&self) -> Tool {
        let mut t = self.tool.clone();
        if self.stock_to_leave_radial.abs() > 1e-9 {
            let r = (t.radius() + self.stock_to_leave_radial).max(0.01);
            t.diameter = r * 2.0;
            if let crate::tool::ToolKind::BullNose { corner_radius } = t.kind {
                t.kind = crate::tool::ToolKind::BullNose { corner_radius: (corner_radius + self.stock_to_leave_radial).clamp(0.001, r - 0.001) };
            }
        }
        t
    }

    /// The cell size for height maps: the tolerance-derived sampling, capped so the map stays
    /// under the cell limit over `bounds`.
    pub fn cell_for(&self, bounds: &Aabb2) -> f64 {
        let size = bounds.size();
        let mut cell = (self.tolerance * 4.0).max(0.02);
        let max_cells = 4_000_000.0;
        let need = (size.x / cell + 1.0) * (size.y / cell + 1.0);
        if need > max_cells {
            cell = ((size.x * size.y) / max_cells).sqrt().max(cell);
        }
        cell
    }

    /// Cutter-location height map of the model for `tool` over `bounds`: the tip height where
    /// the tool touches the surface at each cell, `floor` where it touches nothing.
    pub fn cl_map(&self, tool: &Tool, bounds: &Aabb2, floor: f64) -> Result<HeightMap> {
        let Some(mesh) = self.model else {
            return Err(CamError::Geometry("this operation needs the model's bodies".into()));
        };
        let cell = self.cell_for(bounds);
        let axial = self.stock_to_leave_axial;
        let mut map = HeightMap::new(*bounds, cell, floor)?;
        for j in 0..map.ny {
            for i in 0..map.nx {
                let p = map.point(i, j);
                let z = mesh.drop(tool, &Frame::IDENTITY, p, floor).map_or(floor, |c| c.z + axial);
                map.set(i, j, z.max(floor));
            }
        }
        Ok(map)
    }

    /// Drops the tool at `xy` in the operation frame, with the stock to leave applied.
    pub fn drop(&self, tool: &Tool, xy: Vec2, floor: f64) -> Option<crate::dropcutter::Contact> {
        self.model?.drop(tool, &Frame::IDENTITY, xy, floor).map(|c| crate::dropcutter::Contact { z: c.z + self.stock_to_leave_axial, ..c })
    }

    /// The machining boundary for 3D operations: where the tool centre may go in XY. The
    /// selected loops, else the stock outline, grown by the tool radius so edges are machined.
    pub fn boundary(&self) -> Result<Vec<Poly>> {
        let r = self.tool.radius();
        let base = if self.geometry.loops.is_empty() { self.stock_outline() } else { self.geometry.loops.clone() };
        if base.is_empty() {
            return Err(CamError::Geometry("no machining boundary".into()));
        }
        offset::offset_set(&base, r, self.tolerance)
    }
}

/// Levels from `top` down to `bottom` at most `stepdown` apart, evenly spaced, bottom included
/// (top excluded: the stock's top is air).
pub fn levels(top: f64, bottom: f64, stepdown: f64) -> Result<Vec<f64>> {
    crate::positive("stepdown", stepdown)?;
    let depth = top - bottom;
    if depth <= 1e-9 {
        return Ok(vec![bottom]);
    }
    let n = (depth / stepdown).ceil().max(1.0);
    if n > 100_000.0 {
        return Err(CamError::TooLarge("more than 100000 depth levels".into()));
    }
    let step = depth / n;
    Ok((1..=n as usize).map(|k| top - step * k as f64).collect())
}

/// The default stepover: 40 % of the tool diameter for roughing, 10 % for finishing.
pub fn default_stepover(tool: &Tool, finishing: bool) -> f64 {
    tool.diameter * if finishing { 0.1 } else { 0.4 }
}

/// Simplifies a 3D polyline to within `tol` (Douglas–Peucker).
pub fn simplify3(p: &[Vec3], tol: f64) -> Vec<Vec3> {
    if p.len() <= 2 {
        return p.to_vec();
    }
    let mut keep = vec![false; p.len()];
    keep[0] = true;
    keep[p.len() - 1] = true;
    let mut stack = vec![(0usize, p.len() - 1)];
    let mut guard = 0usize;
    while let Some((i, j)) = stack.pop() {
        guard += 1;
        if guard > p.len() * 4 + 16 || j <= i + 1 {
            continue;
        }
        let (mut best, mut bd) = (i + 1, -1.0);
        for k in i + 1..j {
            let d = p[k].dist_to_segment(p[i], p[j]);
            if d > bd {
                bd = d;
                best = k;
            }
        }
        if bd > tol {
            keep[best] = true;
            stack.push((i, best));
            stack.push((best, j));
        }
    }
    p.iter().zip(keep).filter(|(_, k)| *k).map(|(q, _)| *q).collect()
}

/// Points along the straight segment `a`–`b` at most `step` apart, both ends included.
pub fn sample_segment(a: Vec2, b: Vec2, step: f64) -> Vec<Vec2> {
    let d = a.dist(b);
    let n = ((d / step.max(1e-6)).ceil() as usize).clamp(1, 1_000_000);
    (0..=n).map(|k| a.lerp(b, k as f64 / n as f64)).collect()
}

/// Orders loops for machining: by nesting depth descending (innermost first) when `inside_out`,
/// then nearest-neighbour from the previous end.
pub fn order_rings(rings: Vec<(usize, Poly)>, inside_out: bool) -> Vec<Poly> {
    let mut rings = rings;
    if inside_out {
        rings.sort_by_key(|r| std::cmp::Reverse(r.0));
    } else {
        rings.sort_by_key(|r| r.0);
    }
    rings.into_iter().map(|(_, p)| p).collect()
}

/// Rotates a closed loop so it starts at the point nearest `from`.
pub fn start_near(p: &[Vec2], from: Vec2) -> Poly {
    if p.is_empty() {
        return Vec::new();
    }
    let k = p.iter().enumerate().min_by(|a, b| a.1.dist(from).total_cmp(&b.1.dist(from))).map_or(0, |x| x.0);
    let mut out = p[k..].to_vec();
    out.extend_from_slice(&p[..k]);
    out
}

/// A loop oriented for climb milling with the material on the `material_left` side: a tool
/// running counter-clockwise around an outer boundary has the material on its right, so the
/// pass is clockwise for climb on an outer loop and counter-clockwise inside a pocket.
pub fn orient_for(p: &[Vec2], direction: Direction, pocket: bool) -> Poly {
    // Inside a pocket (tool inside the loop): climb = counter-clockwise. Around an outer boss:
    // climb = clockwise. Conventional is the reverse.
    let want_ccw = match direction {
        Direction::Climb | Direction::Both => pocket,
        Direction::Conventional => !pocket,
    };
    if polyline::is_ccw(p) == want_ccw { p.to_vec() } else { p.iter().rev().copied().collect() }
}

/// Runs the operation's generator.
pub fn generate(op: &Operation, ctx: &Context) -> Result<(Toolpath, Notice)> {
    ctx.tool.validate()?;
    crate::positive("tolerance", ctx.tolerance)?;
    let (tp, mut notice) = match &op.kind {
        OpKind::Face { .. }
        | OpKind::Pocket2d { .. }
        | OpKind::Adaptive2d { .. }
        | OpKind::Contour2d { .. }
        | OpKind::Chamfer2d { .. }
        | OpKind::Drill { .. }
        | OpKind::Bore { .. }
        | OpKind::Slot { .. }
        | OpKind::Trace { .. }
        | OpKind::Engrave { .. }
        | OpKind::Circular { .. }
        | OpKind::Thread { .. } => two_d::generate(op, ctx)?,
        OpKind::Pocket3d { .. }
        | OpKind::Adaptive3d { .. }
        | OpKind::Morph { .. }
        | OpKind::Parallel { .. }
        | OpKind::Contour3d { .. }
        | OpKind::Scallop { .. }
        | OpKind::Pencil { .. }
        | OpKind::Spiral { .. }
        | OpKind::Radial { .. }
        | OpKind::MorphedSpiral { .. }
        | OpKind::Project {}
        | OpKind::Horizontal { .. }
        | OpKind::SteepShallow { .. } => three_d::generate(op, ctx)?,
        OpKind::Rotary { .. } | OpKind::Swarf { .. } | OpKind::MultiAxisContour { .. } | OpKind::Flow { .. } => multi_axis::generate(op, ctx)?,
        OpKind::LaserCut { .. } | OpKind::LaserEngrave { .. } => laser::generate(op, ctx)?,
        OpKind::TurnFace { .. } | OpKind::TurnProfile { .. } | OpKind::TurnGroove { .. } | OpKind::TurnThread { .. } => turning::generate(op, ctx)?,
    };
    let st = tp.stats(5000.0, None);
    notice.value("moves", st.moves);
    notice.value("cutting_length_mm", r3(st.cutting_length));
    notice.value("time_s", r3(st.time_s));
    notice.value("min_z_mm", r3(st.min_z));
    notice.value("max_tilt_deg", r3(st.max_tilt_deg));
    if tp.is_empty() {
        notice.issue(Issue::warning("no_toolpath", "the operation produced no moves: check the geometry selection, the heights and the tool size"));
    }
    // Work about a rotary axis (rotary, turning) and multi-axis work are not depths along Z;
    // laser work has no depth.
    let along_z = !matches!(
        op.kind,
        OpKind::Rotary { .. }
            | OpKind::Swarf { .. }
            | OpKind::MultiAxisContour { .. }
            | OpKind::Flow { .. }
            | OpKind::TurnFace { .. }
            | OpKind::TurnProfile { .. }
            | OpKind::TurnGroove { .. }
            | OpKind::TurnThread { .. }
            | OpKind::LaserCut { .. }
            | OpKind::LaserEngrave { .. }
    );
    if st.min_z < ctx.heights.bottom - 1e-6 && along_z && !matches!(op.kind, OpKind::Drill { .. }) {
        notice.issue(
            Issue::warning("below_bottom", format!("moves reach z {:.3}, below the bottom height {:.3}", st.min_z, ctx.heights.bottom))
                .parameter("bottom", ctx.heights.bottom),
        );
    }
    // Flute length check: the deepest cut below the top of the surrounding material.
    let depth = ctx.heights.top - st.min_z;
    if along_z && depth > ctx.tool.flute_length + 1e-6 && !tp.is_empty() {
        notice.issue(
            Issue::warning(
                "tool_too_short",
                format!("the cut is {:.2} mm deep but the tool's flutes are {:.2} mm long; the shank may rub", depth, ctx.tool.flute_length),
            )
            .parameter("flute_length", ctx.tool.flute_length)
            .limit(r3(depth)),
        );
    }
    if notice.info.is_none() {
        notice.info = Some(format!("{}: {} moves, {:.0} mm of cutting, {:.0} s", op.kind.type_name(), st.moves, st.cutting_length, st.time_s));
    }
    Ok((tp, notice))
}

/// A value no fixed-choice type accepts, to make it list the ones it does.
const PROBE: &str = "\u{1}";

/// The values a fixed-choice type `T` takes, read from what it says when given one it does
/// not: `probe` is the type's JSON with [`PROBE`] where the choice goes (`"\u{1}"` for a plain
/// choice, `{"type": "\u{1}"}` for a tagged one). Empty when `T` is not a fixed choice.
pub fn variants<T: serde::de::DeserializeOwned>(probe: serde_json::Value) -> Vec<String> {
    let probe = match probe {
        serde_json::Value::Null => serde_json::Value::String(PROBE.into()),
        other => other,
    };
    variants_in(serde_json::from_value::<T>(probe).err())
}

/// The names in serde's "unknown variant `x`, expected one of `a`, `b`" message.
fn variants_in(err: Option<serde_json::Error>) -> Vec<String> {
    let Some(e) = err else { return Vec::new() };
    let msg = e.to_string();
    let Some(rest) = msg.split_once("unknown variant").map(|x| x.1) else { return Vec::new() };
    let Some(rest) = rest.split_once("expected").map(|x| x.1) else { return Vec::new() };
    rest.split('`').skip(1).step_by(2).map(str::to_string).collect()
}
