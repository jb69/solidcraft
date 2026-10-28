//! The manufacturing model a design stores: the tool library, machines, setups (work coordinate
//! system, stock, the bodies to machine) and their operations. Everything here is plain data
//! with serde; the engine owns it in the document and the operations' toolpaths are computed
//! from it by [`crate::ops`].

use serde::{Deserialize, Serialize};
use solidcraft_geom::{Aabb3, Vec2, Vec3};

use crate::frame::Frame;
use crate::linking::{Heights, Linking};
use crate::machine::Machine;
use crate::ops::OpKind;
use crate::polyline::Poly;
use crate::tool::ToolLibrary;
use crate::{CamError, Result};

/// The stock, relative to the setup's WCS.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Stock {
    /// The model's bounding box grown by offsets on each side (mm).
    RelativeBox {
        #[serde(default)]
        x_neg: f64,
        #[serde(default)]
        x_pos: f64,
        #[serde(default)]
        y_neg: f64,
        #[serde(default)]
        y_pos: f64,
        #[serde(default)]
        z_neg: f64,
        #[serde(default)]
        z_pos: f64,
    },
    /// A box between two WCS points.
    FixedBox { min: Vec3, max: Vec3 },
    /// A cylinder about a WCS axis around the model, grown radially and at each end.
    RelativeCylinder {
        axis: Axis,
        #[serde(default)]
        radial: f64,
        #[serde(default)]
        start: f64,
        #[serde(default)]
        end: f64,
    },
    /// A cylinder about a WCS axis: `radius`, from `start` to `end` along the axis.
    FixedCylinder { axis: Axis, radius: f64, start: f64, end: f64 },
    /// What an earlier setup leaves: its stock after its operations, seen in this setup's WCS
    /// (a part turned over to machine its other side). `setup` 0 is the setup before this one.
    FromSetup {
        #[serde(default)]
        setup: u64,
    },
}

impl Default for Stock {
    fn default() -> Self {
        Stock::RelativeBox { x_neg: 1.0, x_pos: 1.0, y_neg: 1.0, y_pos: 1.0, z_neg: 0.0, z_pos: 1.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    #[default]
    X,
    Y,
    Z,
}

impl Axis {
    pub fn dir(self) -> Vec3 {
        match self {
            Axis::X => Vec3::new(1.0, 0.0, 0.0),
            Axis::Y => Vec3::new(0.0, 1.0, 0.0),
            Axis::Z => Vec3::new(0.0, 0.0, 1.0),
        }
    }
}

/// The stock resolved against the model's bounds (WCS).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum StockShape {
    Box { min: Vec3, max: Vec3 },
    Cylinder { axis: Axis, radius: f64, start: f64, end: f64 },
}

impl StockShape {
    /// The stock's largest distance from the X axis (a rotary axis along X): a cylinder's
    /// radius when it lies along X, else the farthest corner of the box in Y and Z.
    pub fn radius_about_x(&self) -> f64 {
        match *self {
            StockShape::Cylinder { axis: Axis::X, radius, .. } => radius,
            _ => {
                let b = self.bounds();
                [b.min.y.abs(), b.max.y.abs()]
                    .iter()
                    .fold(0.0_f64, |m, y| m.max(*y))
                    .hypot([b.min.z.abs(), b.max.z.abs()].iter().fold(0.0_f64, |m, z| m.max(*z)))
            }
        }
    }

    pub fn bounds(&self) -> Aabb3 {
        let mut b = Aabb3::default();
        match *self {
            StockShape::Box { min, max } => {
                b.add(min);
                b.add(max);
            }
            StockShape::Cylinder { axis, radius, start, end } => {
                let d = axis.dir();
                let r = Vec3::new(radius, radius, radius) - d * radius;
                b.add(d * start - r);
                b.add(d * start + r);
                b.add(d * end - r);
                b.add(d * end + r);
            }
        }
        b
    }

    /// The stock's outline in the XY plane of `frame` (the tool-axis view), as a polygon, for
    /// 2.5D operations that machine the whole top.
    pub fn outline_in(&self, frame: &Frame, tol: f64) -> Vec<Poly> {
        let b = self.bounds();
        // Project the eight corners and take their convex hull (a box stays a rectangle when the
        // frame is axis-aligned; tilted frames get the hull).
        let corners = [
            Vec3::new(b.min.x, b.min.y, b.min.z),
            Vec3::new(b.max.x, b.min.y, b.min.z),
            Vec3::new(b.min.x, b.max.y, b.min.z),
            Vec3::new(b.max.x, b.max.y, b.min.z),
            Vec3::new(b.min.x, b.min.y, b.max.z),
            Vec3::new(b.max.x, b.min.y, b.max.z),
            Vec3::new(b.min.x, b.max.y, b.max.z),
            Vec3::new(b.max.x, b.max.y, b.max.z),
        ];
        let pts: Vec<Vec2> = corners.iter().map(|c| frame.to_local(*c)).map(|p| Vec2::new(p.x, p.y)).collect();
        if let StockShape::Cylinder { axis, radius, start, end } = *self
            && (frame.dir_to_local(axis.dir()) - Vec3::new(0.0, 0.0, 1.0)).len() < 1e-9
        {
            let c = frame.to_local(axis.dir() * (start + end) * 0.5);
            return vec![crate::polyline::circle(Vec2::new(c.x, c.y), radius, tol)];
        }
        vec![convex_hull(&pts)]
    }

    /// The extent along the tool axis of `frame`.
    pub fn z_range_in(&self, frame: &Frame) -> (f64, f64) {
        let b = self.bounds();
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for x in [b.min.x, b.max.x] {
            for y in [b.min.y, b.max.y] {
                for z in [b.min.z, b.max.z] {
                    let l = frame.to_local(Vec3::new(x, y, z)).z;
                    lo = lo.min(l);
                    hi = hi.max(l);
                }
            }
        }
        (lo, hi)
    }
}

/// Convex hull (Andrew's monotone chain), counter-clockwise.
pub fn convex_hull(pts: &[Vec2]) -> Poly {
    let mut p: Vec<Vec2> = pts.iter().copied().filter(|v| v.is_finite()).collect();
    p.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    p.dedup_by(|a, b| a.dist(*b) < 1e-9);
    if p.len() < 3 {
        return p;
    }
    let mut lower: Vec<Vec2> = Vec::new();
    for &q in &p {
        while lower.len() >= 2 && (lower[lower.len() - 1] - lower[lower.len() - 2]).cross(q - lower[lower.len() - 2]) <= 0.0 {
            lower.pop();
        }
        lower.push(q);
    }
    let mut upper: Vec<Vec2> = Vec::new();
    for &q in p.iter().rev() {
        while upper.len() >= 2 && (upper[upper.len() - 1] - upper[upper.len() - 2]).cross(q - upper[upper.len() - 2]) <= 0.0 {
            upper.pop();
        }
        upper.push(q);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

impl Stock {
    /// The stock around a model whose WCS bounds are `model`.
    pub fn resolve(&self, model: &Aabb3) -> Result<StockShape> {
        let check = |v: f64, what: &str| if v.is_finite() { Ok(v) } else { Err(CamError::Param(format!("stock {what} must be finite"))) };
        match self {
            Stock::RelativeBox { x_neg, x_pos, y_neg, y_pos, z_neg, z_pos } => {
                if model.is_empty() {
                    return Err(CamError::Geometry("relative stock needs a model with bounds".into()));
                }
                for (v, w) in [(x_neg, "x_neg"), (x_pos, "x_pos"), (y_neg, "y_neg"), (y_pos, "y_pos"), (z_neg, "z_neg"), (z_pos, "z_pos")] {
                    check(*v, w)?;
                }
                Ok(StockShape::Box {
                    min: Vec3::new(model.min.x - x_neg, model.min.y - y_neg, model.min.z - z_neg),
                    max: Vec3::new(model.max.x + x_pos, model.max.y + y_pos, model.max.z + z_pos),
                })
            }
            Stock::FixedBox { min, max } => {
                if !(min.is_finite() && max.is_finite()) || min.x >= max.x || min.y >= max.y || min.z >= max.z {
                    return Err(CamError::Param("fixed stock box needs min below max on every axis".into()));
                }
                Ok(StockShape::Box { min: *min, max: *max })
            }
            Stock::RelativeCylinder { axis, radial, start, end } => {
                if model.is_empty() {
                    return Err(CamError::Geometry("relative stock needs a model with bounds".into()));
                }
                check(*radial, "radial")?;
                check(*start, "start")?;
                check(*end, "end")?;
                let d = axis.dir();
                let (lo, hi) = (model.min.dot(d), model.max.dot(d));
                // Radius: farthest model corner from the axis.
                let mut r = 0.0f64;
                for x in [model.min.x, model.max.x] {
                    for y in [model.min.y, model.max.y] {
                        for z in [model.min.z, model.max.z] {
                            let p = Vec3::new(x, y, z);
                            let radial_v = p - d * p.dot(d);
                            r = r.max(radial_v.len());
                        }
                    }
                }
                Ok(StockShape::Cylinder { axis: *axis, radius: r + radial, start: lo - start, end: hi + end })
            }
            Stock::FromSetup { .. } => Err(CamError::Param("stock from an earlier setup is worked out from that setup".into())),
            Stock::FixedCylinder { axis, radius, start, end } => {
                crate::positive("stock radius", *radius)?;
                if !(start.is_finite() && end.is_finite()) || start >= end {
                    return Err(CamError::Param("cylinder stock needs start below end".into()));
                }
                Ok(StockShape::Cylinder { axis: *axis, radius: *radius, start: *start, end: *end })
            }
        }
    }
}

/// What an operation machines: the geometry selection, resolved by the engine into loops,
/// points or faces in the operation's frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Geometry {
    /// The whole stock outline (face, roughing).
    #[default]
    Stock,
    /// The model's silhouette seen along the tool axis.
    Silhouette,
    /// Explicit closed loops in the operation frame's XY (mm).
    Loops { loops: Vec<Poly> },
    /// Closed loops from a sketch feature (its profiles).
    Sketch { feature: u64 },
    /// Planar faces picked by a point on each (the engine resolves them): their outer loops and
    /// holes, and their height.
    Faces { points: Vec<Vec3> },
    /// Holes: the cylindrical faces picked by a point on each (drilling), or explicit points.
    Holes {
        #[serde(default)]
        points: Vec<Vec3>,
        #[serde(default)]
        diameter: f64,
    },
    /// Two chains for swarf: the bottom and the top of the wall, as polylines (WCS).
    Chains { bottom: Vec<Vec3>, top: Vec<Vec3> },
    /// Rails for flow or morph operations: two polylines (WCS).
    Rails { first: Vec<Vec3>, second: Vec<Vec3> },
}

/// A machining operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    pub id: u64,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub tool: u64,
    /// Tool orientation for indexed (3+2) work, relative to the setup's WCS (`None`: the WCS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orientation: Option<Frame>,
    #[serde(default)]
    pub geometry: Geometry,
    #[serde(default)]
    pub heights: Heights,
    #[serde(default)]
    pub linking: Linking,
    /// Chord tolerance of the toolpath (mm).
    #[serde(default = "default_tolerance")]
    pub tolerance: f64,
    /// Radial and axial stock to leave (mm; negative for an over-cut).
    #[serde(default)]
    pub stock_to_leave_radial: f64,
    #[serde(default)]
    pub stock_to_leave_axial: f64,
    /// Rest machining: cut only where the previous operations of the setup left material.
    #[serde(default)]
    pub rest_machining: bool,
    /// Where the operation may machine (area operations: 3D roughing and finishing).
    #[serde(default)]
    pub boundary: crate::boundary::Boundary,
    #[serde(flatten)]
    pub kind: OpKind,
}

fn default_true() -> bool {
    true
}
fn default_tolerance() -> f64 {
    0.01
}

/// A setup: one fixturing of the part with its WCS, stock and operations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Setup {
    pub id: u64,
    pub name: String,
    /// Bodies to machine (names); empty means every body.
    #[serde(default)]
    pub bodies: Vec<String>,
    /// Fixture bodies: avoided, never machined.
    #[serde(default)]
    pub fixtures: Vec<String>,
    /// The work coordinate system in design coordinates.
    #[serde(default)]
    pub wcs: Frame,
    #[serde(default)]
    pub stock: Stock,
    #[serde(default)]
    pub machine: Machine,
    #[serde(default)]
    pub operations: Vec<Operation>,
    /// Program number / name for the post.
    #[serde(default)]
    pub program: String,
}

/// Everything manufacturing a design stores.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Manufacture {
    pub tools: ToolLibrary,
    pub setups: Vec<Setup>,
    pub next_id: u64,
}

impl Manufacture {
    pub fn is_empty(&self) -> bool {
        self.tools.tools.is_empty() && self.setups.is_empty()
    }
    pub fn next_id(&mut self) -> u64 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        id
    }
    pub fn setup(&self, id: u64) -> Option<&Setup> {
        self.setups.iter().find(|s| s.id == id)
    }
    pub fn setup_mut(&mut self, id: u64) -> Option<&mut Setup> {
        self.setups.iter_mut().find(|s| s.id == id)
    }
    /// The operation and its setup.
    pub fn operation(&self, id: u64) -> Option<(&Setup, &Operation)> {
        self.setups.iter().find_map(|s| s.operations.iter().find(|o| o.id == id).map(|o| (s, o)))
    }
    pub fn operation_mut(&mut self, id: u64) -> Option<&mut Operation> {
        self.setups.iter_mut().find_map(|s| s.operations.iter_mut().find(|o| o.id == id))
    }
    /// Whether a tool is used by any operation.
    pub fn tool_uses(&self, tool: u64) -> usize {
        self.setups.iter().flat_map(|s| &s.operations).filter(|o| o.tool == tool).count()
    }
}
