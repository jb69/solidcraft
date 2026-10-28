//! Machining boundaries: the area, seen along the tool axis, an operation may machine. Every
//! area operation (3D roughing and finishing, multi-axis contour) cuts only inside it; the
//! cutter-location surface still keeps the tool off the part, so a boundary decides where to
//! machine, never whether the part is safe.
//!
//! A boundary is built from:
//! - a **source**: the stock's outline, the part's silhouette (its outline seen along the tool
//!   axis), the part's bounding box, the operation's selection (faces, a sketch, chains or
//!   loops), or the material earlier operations left (rest machining);
//! - a **tool containment**: the whole tool inside the boundary, its centre on it, or its edge
//!   reaching past it by the tool's radius;
//! - an **offset** that grows it (or, negative, shrinks it);
//! - **clipping** to the stock (the tool's edge may reach the stock's edge, no further) and
//!   **keep-out areas** around the setup's fixtures (clamps, vise jaws).
//!
//! `auto` (the default) chooses from what the operation is and what it has to work with:
//! roughing around a part clears a ring a tool wide outside the part's outline; wall finishing
//! (waterline, steep and shallow) runs the tool outside the outline so it can reach the walls;
//! surface finishing keeps the tool's centre on the part; a selection bounds the operation to
//! what was picked, the tool inside it when roughing. Every generated toolpath's notice says which
//! boundary was used and why.

use std::cell::OnceCell;

use serde::{Deserialize, Serialize};
use solidcraft_geom::{Aabb2, Vec2};

use crate::dropcutter::DropMesh;
use crate::heightmap::HeightMap;
use crate::ops::{Context, OpKind};
use crate::polyline::{self, Poly};
use crate::tool::{Tool, ToolKind};
use crate::{CamError, Result, offset};

/// Where a boundary comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Chosen from the operation's kind, its selection and the setup.
    #[default]
    Auto,
    /// The stock's outline.
    Stock,
    /// The part's silhouette: the outline of the setup's bodies seen along the tool axis.
    Part,
    /// The rectangle around the part.
    BoundingBox,
    /// The operation's geometry: picked faces, a sketch, chains or loops.
    Selection,
    /// Where earlier operations left material above the part (rest machining).
    Rest,
}

/// Where the tool may be relative to the boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Containment {
    #[default]
    Auto,
    /// The whole tool stays inside: nothing outside the boundary is touched.
    Inside,
    /// The tool's centre stays inside: the boundary's whole area is machined.
    Center,
    /// The tool's edge reaches past the boundary by the tool's radius: its edges are machined
    /// cleanly from outside.
    Outside,
}

/// An operation's machining boundary, as stored.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Boundary {
    pub source: Source,
    pub containment: Containment,
    /// Grows the area (mm; negative shrinks it), after the containment.
    pub offset: f64,
    /// Keeps the tool within the stock's outline (its edge may reach the stock's edge).
    pub clip_to_stock: bool,
    /// Keeps the tool clear of the setup's fixtures by `fixture_clearance`.
    pub avoid_fixtures: bool,
    pub fixture_clearance: f64,
}

impl Default for Boundary {
    fn default() -> Self {
        Boundary {
            source: Source::Auto,
            containment: Containment::Auto,
            offset: 0.0,
            clip_to_stock: true,
            avoid_fixtures: true,
            fixture_clearance: 1.0,
        }
    }
}

/// What an operation does with its boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Clears material level by level (3D pocket, adaptive).
    Rough,
    /// Finishes walls (waterline, steep and shallow): the tool must reach them from outside.
    Walls,
    /// Finishes surfaces from above (parallel, scallop, spiral…).
    Surface,
}

impl Role {
    pub fn of(kind: &OpKind) -> Role {
        match kind {
            OpKind::Pocket3d { .. } | OpKind::Adaptive3d { .. } => Role::Rough,
            OpKind::Contour3d { .. } | OpKind::SteepShallow { .. } => Role::Walls,
            _ => Role::Surface,
        }
    }
}

/// A boundary with `auto` worked out: what will be used, and why, in a sentence.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub source: Source,
    pub containment: Containment,
    /// Extra growth on top of the containment (mm).
    pub offset: f64,
    /// Further growth in tool radii (what `auto` adds for roughing around the part).
    pub extra_radii: f64,
    pub clip_to_stock: bool,
    pub avoid_fixtures: bool,
    pub fixture_clearance: f64,
    pub why: String,
}

impl Plan {
    /// The plan for an operation of `kind` with boundary `b`; `has_selection`: the operation's
    /// geometry gave loops; `has_model`: the setup has bodies; `has_rest`: rest stock is known.
    pub fn new(b: &Boundary, kind: &OpKind, has_selection: bool, has_model: bool, has_rest: bool) -> Plan {
        let role = Role::of(kind);
        let mut notes: Vec<&str> = Vec::new();
        let source = match b.source {
            Source::Auto if has_selection => Source::Selection,
            Source::Auto if has_model => Source::Part,
            Source::Auto => Source::Stock,
            Source::Selection if !has_selection => {
                notes.push("nothing is selected, so the part's outline stands in");
                if has_model { Source::Part } else { Source::Stock }
            }
            Source::Part | Source::BoundingBox if !has_model => {
                notes.push("the setup has no bodies, so the stock's outline stands in");
                Source::Stock
            }
            Source::Rest if !has_rest => {
                notes.push("rest material is only known with rest machining on, so the part's outline stands in");
                if has_model { Source::Part } else { Source::Stock }
            }
            s => s,
        };
        let auto = b.containment == Containment::Auto;
        let containment = match (b.containment, source, role) {
            (Containment::Auto, Source::Selection, Role::Rough) => Containment::Inside,
            (Containment::Auto, Source::Selection, _) => Containment::Center,
            (Containment::Auto, Source::Part, Role::Surface) => Containment::Center,
            (Containment::Auto, _, _) => Containment::Outside,
            (c, _, _) => c,
        };
        let why = match (source, containment) {
            (Source::Part, Containment::Outside) if role == Role::Rough && auto => {
                "roughing around the part: its outline grown so a ring a tool wide is cleared outside it, within the stock"
            }
            (Source::Part, Containment::Outside) if role == Role::Walls => {
                "finishing walls: the tool runs outside the part's outline so it reaches every wall"
            }
            (Source::Part, Containment::Center) => {
                "finishing the part: the tool's centre stays on the part's outline, so no passes run over the stock around it"
            }
            (Source::Part, _) => "the part's outline",
            (Source::Selection, Containment::Inside) => "inside the selection: the whole tool stays within what was picked",
            (Source::Selection, Containment::Center) => "the selection: the tool's centre stays within what was picked, so all of it is machined",
            (Source::Selection, _) => "the selection, the tool's edge reaching past it",
            (Source::Stock, _) => "the whole stock",
            (Source::BoundingBox, _) => "the rectangle around the part",
            (Source::Rest, _) => "where earlier operations left material",
            (Source::Auto, _) => "the stock",
        };
        let mut why = why.to_string();
        for n in notes {
            why.push_str("; ");
            why.push_str(n);
        }
        // Roughing around the part clears an extra tool radius, so the finishing tool has room.
        let extra = if auto && b.source == Source::Auto && source == Source::Part && role == Role::Rough { 1.0 } else { 0.0 };
        Plan {
            source,
            containment,
            // Input is hostile: no boundary needs to grow further than a machine's travel.
            offset: if b.offset.is_finite() { b.offset.clamp(-10_000.0, 10_000.0) } else { 0.0 },
            extra_radii: extra,
            clip_to_stock: b.clip_to_stock,
            avoid_fixtures: b.avoid_fixtures,
            fixture_clearance: if b.fixture_clearance.is_finite() { b.fixture_clearance.clamp(0.0, 1_000.0) } else { 1.0 },
            why,
        }
    }

    /// A plan with no `auto` in it: `source` with `containment`, clipped to the stock.
    pub fn fixed(source: Source, containment: Containment) -> Plan {
        Plan {
            source,
            containment,
            offset: 0.0,
            extra_radii: 0.0,
            clip_to_stock: true,
            avoid_fixtures: true,
            fixture_clearance: 1.0,
            why: String::new(),
        }
    }

    /// The growth beyond the containment for a tool of radius `r` (mm).
    fn growth(&self, r: f64) -> f64 {
        self.offset + self.extra_radii * r
    }
}

/// What the boundary turned out to be, for the operation's notice.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub source: Source,
    pub containment: Containment,
    /// The total growth applied to the source (mm): containment and offset.
    pub grow_mm: f64,
    pub area_mm2: f64,
    pub loops: usize,
    pub clipped_to_stock: bool,
    pub fixtures_avoided: bool,
    pub why: String,
}

/// A context's boundary, worked out once.
pub type Cache = OnceCell<std::result::Result<(Vec<Poly>, Report), String>>;

fn name(s: Source) -> &'static str {
    match s {
        Source::Auto => "auto",
        Source::Stock => "stock",
        Source::Part => "part",
        Source::BoundingBox => "bounding_box",
        Source::Selection => "selection",
        Source::Rest => "rest",
    }
}

fn cname(c: Containment) -> &'static str {
    match c {
        Containment::Auto => "auto",
        Containment::Inside => "inside",
        Containment::Center => "center",
        Containment::Outside => "outside",
    }
}

impl Report {
    /// Into a notice: values, and the sentence saying which boundary and why.
    pub fn add_to(&self, notice: &mut crate::notice::Notice) {
        notice.value("boundary_source", name(self.source));
        notice.value("boundary_containment", cname(self.containment));
        notice.value("boundary_grow_mm", crate::notice::r3(self.grow_mm));
        notice.value("boundary_area_mm2", crate::notice::r3(self.area_mm2));
        let mut text = format!("boundary: {}", self.why);
        if self.fixtures_avoided {
            text.push_str("; kept clear of the fixtures");
        }
        notice.issue(crate::notice::Issue::info("boundary", text));
    }
}

/// The region the tool's centre may occupy, for `ctx`.
pub fn region(ctx: &Context) -> Result<(Vec<Poly>, Report)> {
    let plan = &ctx.boundary;
    let r = ctx.tool.radius();
    let tol = ctx.tolerance;
    let base: Vec<Poly> = match plan.source {
        Source::Stock | Source::Auto => ctx.stock_outline(),
        Source::Selection => ctx.geometry.loops.clone(),
        Source::Part => match ctx.model {
            Some(m) => silhouette(m, tol)?,
            None => ctx.stock_outline(),
        },
        Source::BoundingBox => {
            let b = ctx.model_bounds;
            if b.is_empty() { ctx.stock_outline() } else { vec![polyline::rect(Vec2::new(b.min.x, b.min.y), Vec2::new(b.max.x, b.max.y))] }
        }
        Source::Rest => match (ctx.rest, ctx.model) {
            (Some(rest), Some(m)) => rest_outline(rest, m, tol)?,
            _ => ctx.stock_outline(),
        },
    };
    if base.is_empty() {
        return Err(CamError::Geometry(format!("the {} boundary is empty", name(plan.source))));
    }
    let contain = match plan.containment {
        Containment::Inside => -r,
        Containment::Center | Containment::Auto => 0.0,
        Containment::Outside => r,
    };
    let grow = contain + plan.growth(r);
    let mut out = if grow.abs() > 1e-9 { offset::offset_set(&base, grow, tol)? } else { polyline::normalize_set(&base) };
    if out.is_empty() {
        return Err(CamError::Geometry(format!(
            "the boundary vanishes: {} mm of growth leaves nothing of the {} (a smaller tool, or containment center)",
            crate::notice::r3(grow),
            name(plan.source)
        )));
    }
    let mut clipped = false;
    if plan.clip_to_stock && plan.source != Source::Stock {
        let stock = offset::offset_set(&ctx.stock_outline(), r, tol)?;
        if let Some(clip) = stock.iter().find(|p| polyline::is_ccw(p)) {
            let before = polyline::set_area(&out);
            out = polyline::normalize_set(&out.iter().map(|p| clip_convex(p, clip)).filter(|p| p.len() >= 3).collect::<Vec<_>>());
            clipped = (polyline::set_area(&out) - before).abs() > tol * tol;
        }
    }
    let mut avoided = false;
    if plan.avoid_fixtures
        && let Some(fx) = ctx.fixtures.filter(|f| !f.is_empty())
    {
        let keep_out = offset::offset_set(&silhouette(fx, tol)?, r + plan.fixture_clearance, tol)?;
        if !keep_out.is_empty() {
            out = subtract(&out, &keep_out, tol)?;
            avoided = true;
        }
    }
    if out.is_empty() {
        return Err(CamError::Geometry("the boundary is empty once clipped to the stock and kept clear of the fixtures".into()));
    }
    let report = Report {
        source: plan.source,
        containment: plan.containment,
        grow_mm: grow,
        area_mm2: polyline::set_area(&out),
        loops: out.len(),
        clipped_to_stock: clipped,
        fixtures_avoided: avoided,
        why: plan.why.clone(),
    };
    Ok((out, report))
}

/// The outline of `mesh` seen along z: the outer loops of where it stands, holes through it
/// left out (a through hole is no part of the outline).
pub fn silhouette(mesh: &DropMesh, tol: f64) -> Result<Vec<Poly>> {
    let b = mesh.bounds;
    if b.is_empty() {
        return Ok(Vec::new());
    }
    let mut b2 = Aabb2::default();
    b2.add(Vec2::new(b.min.x - 1.0, b.min.y - 1.0));
    b2.add(Vec2::new(b.max.x + 1.0, b.max.y + 1.0));
    let cell = (tol * 2.0).max(0.02).max(b2.diagonal() / 1200.0);
    let probe = Tool::new(0, "probe", ToolKind::FlatEnd, 1e-3);
    let floor = b.min.z - 1.0;
    let map = HeightMap::from_fn(b2, cell, |p| mesh.drop_z(&probe, p, floor).map_or(floor, |c| c.z))?;
    let loops: Vec<Poly> = map.contours(floor + 0.5).iter().map(|l| polyline::simplify_closed(l, cell * 0.5)).filter(|l| l.len() >= 3).collect();
    let outer: Vec<Poly> = loops.iter().enumerate().filter(|(i, _)| polyline::nesting_depth(&loops, *i) == 0).map(|(_, l)| l.clone()).collect();
    // The contour lies between the last cell on the part and the first off it, about half a
    // cell outside the edge on average: bring it back.
    let outer = polyline::normalize_set(&outer);
    Ok(offset::offset_set(&outer, -cell * 0.5, tol.max(cell * 0.25)).unwrap_or(outer))
}

/// Where the rest stock stands above the part by more than the tolerance.
fn rest_outline(rest: &HeightMap, model: &DropMesh, tol: f64) -> Result<Vec<Poly>> {
    let probe = Tool::new(0, "probe", ToolKind::FlatEnd, 1e-3);
    let margin = tol.max(0.05);
    let mut field = HeightMap { origin: rest.origin, cell: rest.cell, nx: rest.nx, ny: rest.ny, z: vec![-1.0; rest.z.len()] };
    let floor = model.bounds.min.z - 1.0;
    for j in 0..rest.ny {
        for i in 0..rest.nx {
            let Some(top) = rest.get(i, j).filter(|z| z.is_finite()) else { continue };
            let p = rest.point(i, j);
            let part = model.drop_z(&probe, p, floor).map_or(floor, |c| c.z);
            field.set(i, j, top - part - margin);
        }
    }
    let loops: Vec<Poly> = field.contours(0.0).iter().map(|l| polyline::simplify_closed(l, rest.cell * 0.5)).filter(|l| l.len() >= 3).collect();
    Ok(polyline::normalize_set(&loops))
}

/// `subject` clipped to the convex, counter-clockwise polygon `clip` (Sutherland–Hodgman).
pub fn clip_convex(subject: &[Vec2], clip: &[Vec2]) -> Poly {
    let mut out: Vec<Vec2> = subject.to_vec();
    let n = clip.len();
    for k in 0..n {
        if out.is_empty() {
            break;
        }
        let (a, b) = (clip[k], clip[(k + 1) % n]);
        let inside = |p: Vec2| (b - a).cross(p - a) >= 0.0;
        let input = std::mem::take(&mut out);
        let m = input.len();
        for i in 0..m {
            let cur = input[i];
            let prev = input[(i + m - 1) % m];
            let (ci, pi) = (inside(cur), inside(prev));
            if ci != pi {
                let d = cur - prev;
                let denom = (b - a).cross(d);
                if denom.abs() > 1e-15 {
                    let t = (b - a).cross(a - prev) / denom;
                    out.push(prev + d * t);
                }
            }
            if ci {
                out.push(cur);
            }
        }
    }
    out
}

/// `a` with `b` taken away, sampled on a grid finer than the tolerance allows (at most about
/// four million cells): each row's spans are exact, the outline between rows is interpolated.
pub fn subtract(a: &[Poly], b: &[Poly], tol: f64) -> Result<Vec<Poly>> {
    let ba = polyline::bounds_of(a);
    if ba.is_empty() {
        return Ok(Vec::new());
    }
    let size = ba.size();
    let mut cell = (tol * 2.0).max(0.02);
    while (size.x / cell + 4.0) * (size.y / cell + 4.0) > 4_000_000.0 {
        cell *= 1.5;
    }
    let bounds = ba.inflated(cell * 2.0);
    let mut field = HeightMap::new(bounds, cell, -1.0)?;
    for j in 0..field.ny {
        let y = field.point(0, j).y;
        let sa = polyline::scanline(a, y);
        let sb = polyline::scanline(b, y);
        for i in 0..field.nx {
            let x = field.point(i, j).x;
            let da = span_distance(&sa, x);
            let db = span_distance(&sb, x);
            field.set(i, j, da.min(-db));
        }
    }
    let loops: Vec<Poly> = field.contours(0.0).iter().map(|l| polyline::simplify_closed(l, cell * 0.5)).filter(|l| l.len() >= 3).collect();
    Ok(polyline::normalize_set(&loops))
}

/// Signed distance along x from `x` to the edge of `spans`: positive inside a span.
fn span_distance(spans: &[(f64, f64)], x: f64) -> f64 {
    let mut best = f64::INFINITY;
    let mut inside = false;
    for (lo, hi) in spans {
        if x >= *lo && x <= *hi {
            inside = true;
        }
        best = best.min((x - lo).abs()).min((x - hi).abs());
    }
    let d = if best.is_finite() { best } else { 1.0e6 };
    if inside { d } else { -d }
}
