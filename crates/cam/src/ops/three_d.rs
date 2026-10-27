//! 3-axis surface operations on the model's cutter-location (CL) height map: Z-level roughing,
//! parallel, waterline, scallop, pencil, spiral, radial, morphed spiral, project, horizontal
//! and steep-and-shallow finishing. With an operation orientation they run as 3+2 work.

use solidcraft_geom::{Aabb2, Vec2, Vec3};

use super::two_d::{pocket_level, raster_passes};
use super::{Context, Direction, OpKind, default_stepover, levels, sample_segment, simplify3};
use crate::heightmap::HeightMap;
use crate::linking::{Linker, Pass};
use crate::notice::{Issue, Notice, r3};
use crate::offset;
use crate::polyline::{self, Poly};
use crate::setup::Operation;
use crate::toolpath::Toolpath;
use crate::{CamError, Result};

pub fn generate(op: &Operation, ctx: &Context) -> Result<(Toolpath, Notice)> {
    if ctx.model.is_none_or(|m| m.is_empty()) {
        return Err(CamError::Geometry("3D operations need the model's bodies in the setup".into()));
    }
    match &op.kind {
        OpKind::Pocket3d { stepover, stepdown, direction, flat_area_detection, order_by_depth } => {
            pocket3d(op, ctx, *stepover, *stepdown, *direction, *flat_area_detection, *order_by_depth)
        }
        OpKind::Parallel { stepover, angle_deg, direction, slope_min_deg, slope_max_deg } => {
            parallel(op, ctx, *stepover, *angle_deg, *direction, (*slope_min_deg, *slope_max_deg), None)
        }
        OpKind::Contour3d { stepdown, direction, slope_min_deg, slope_max_deg, flat_area_detection } => {
            waterline(op, ctx, *stepdown, *direction, (*slope_min_deg, *slope_max_deg), *flat_area_detection, None)
        }
        OpKind::Scallop { stepover, direction } => scallop(op, ctx, *stepover, *direction),
        OpKind::Pencil { min_angle_deg, passes, stepover } => pencil(op, ctx, *min_angle_deg, *passes, *stepover),
        OpKind::Spiral { stepover, center, max_radius } => spiral(op, ctx, *stepover, *center, *max_radius),
        OpKind::Radial { angle_step_deg, center, inner_radius } => radial(op, ctx, *angle_step_deg, *center, *inner_radius),
        OpKind::MorphedSpiral { stepover } => morphed_spiral(op, ctx, *stepover),
        OpKind::Project {} => project(op, ctx),
        OpKind::Horizontal { stepover, direction } => horizontal(op, ctx, *stepover, *direction),
        OpKind::SteepShallow { threshold_deg, steep_stepdown, shallow_stepover, angle_deg, steep_first } => {
            steep_shallow(op, ctx, *threshold_deg, *steep_stepdown, *shallow_stepover, *angle_deg, *steep_first)
        }
        _ => Err(CamError::Param("not a 3D operation".into())),
    }
}

fn linker<'a>(op: &Operation, ctx: &'a Context) -> Linker<'a> {
    Linker { tool: ctx.tool, heights: ctx.heights, linking: op.linking, tolerance: ctx.tolerance }
}

/// The CL map over the machining boundary, with a blocked border and blocked cells outside the
/// boundary (`f64::INFINITY`), so regions extracted from it close along the boundary.
struct Surface {
    boundary: Vec<Poly>,
    map: HeightMap,
    floor: f64,
}

const BLOCKED: f64 = f64::INFINITY;

impl Surface {
    fn build(ctx: &Context, tool: &crate::tool::Tool) -> Result<Surface> {
        let boundary = ctx.boundary()?;
        let b = polyline::bounds_of(&boundary);
        let floor = ctx.heights.bottom.min(ctx.model_z().0) - 1.0;
        let cell = ctx.cell_for(&b);
        let bounds = b.inflated(cell * 2.0);
        let mut map = ctx.cl_map(tool, &bounds, floor)?;
        for j in 0..map.ny {
            for i in 0..map.nx {
                let p = map.point(i, j);
                let border = i == 0 || j == 0 || i + 1 == map.nx || j + 1 == map.ny;
                if border || !polyline::set_contains(&boundary, p) {
                    map.set(i, j, BLOCKED);
                }
            }
        }
        Ok(Surface { boundary, map, floor })
    }

    /// Slope of the CL surface at `p`, degrees from horizontal.
    fn slope_deg(&self, p: Vec2) -> f64 {
        let h = self.map.cell;
        let zx = (self.finite_or_floor(p + Vec2::new(h, 0.0)) - self.finite_or_floor(p - Vec2::new(h, 0.0))) / (2.0 * h);
        let zy = (self.finite_or_floor(p + Vec2::new(0.0, h)) - self.finite_or_floor(p - Vec2::new(0.0, h))) / (2.0 * h);
        (zx * zx + zy * zy).sqrt().atan().to_degrees()
    }

    fn finite_or_floor(&self, p: Vec2) -> f64 {
        let z = self.map.sample_max(p);
        if z.is_finite() { z } else { self.floor }
    }

    /// The region where the tool may sit at height `z` without touching the surface: loops of
    /// `{CL < z}` inside the boundary.
    fn free_region(&self, z: f64) -> Vec<Poly> {
        // contours() returns loops where the map is above the level: negate.
        let neg = HeightMap {
            origin: self.map.origin,
            cell: self.map.cell,
            nx: self.map.nx,
            ny: self.map.ny,
            z: self.map.z.iter().map(|v| if v.is_finite() { -v } else { f64::NEG_INFINITY }).collect(),
        };
        let loops: Vec<Poly> = neg.contours(-z).iter().map(|l| polyline::simplify_closed(l, self.map.cell)).filter(|l| l.len() >= 3).collect();
        // Where the CL surface jumps (a vertical wall), the contour is interpolated between a
        // free and a blocked cell; one cell of shrink keeps the tool centre on the free side.
        offset::offset_set(&loops, -self.map.cell, self.map.cell * 0.1).unwrap_or(loops)
    }

    /// Level contours of the CL surface at `z` (where the tool touches the surface at this
    /// height), as polylines clipped to the unblocked area.
    fn waterlines(&self, z: f64) -> Vec<Poly> {
        let clipped = HeightMap {
            origin: self.map.origin,
            cell: self.map.cell,
            nx: self.map.nx,
            ny: self.map.ny,
            z: self.map.z.iter().map(|v| if v.is_finite() { *v } else { self.floor }).collect(),
        };
        clipped.contours(z).iter().map(|l| polyline::simplify_closed(l, self.map.cell * 0.3)).filter(|l| l.len() >= 3).collect()
    }

    /// Heights of flat areas: CL values shared by many cells.
    fn flat_levels(&self, tol: f64) -> Vec<f64> {
        let mut vals: Vec<f64> = self.map.z.iter().copied().filter(|v| v.is_finite() && *v > self.floor + 1e-6).collect();
        if vals.is_empty() {
            return Vec::new();
        }
        vals.sort_by(f64::total_cmp);
        let n = vals.len();
        let min_run = (n / 200).max(50);
        let mut out = Vec::new();
        let mut i = 0;
        while i < n {
            let mut j = i;
            while j < n && vals[j] - vals[i] <= tol {
                j += 1;
            }
            if j - i >= min_run {
                out.push(vals[(i + j) / 2]);
                i = j;
            } else {
                i += 1;
            }
        }
        out
    }

    /// Cells of the rest stock (`rest`) with material above `z` within the tool radius `r`:
    /// `false` where the tool at `z` would cut only air.
    fn has_material(&self, rest: Option<&HeightMap>, p: Vec2, r: f64, z: f64) -> bool {
        match rest {
            None => true,
            Some(m) => {
                let n = ((r / m.cell).ceil() as i64).max(1);
                let (i0, j0) = ((((p.x - m.origin.x) / m.cell).round()) as i64, (((p.y - m.origin.y) / m.cell).round()) as i64);
                for dj in -n..=n {
                    for di in -n..=n {
                        let (i, j) = (i0 + di, j0 + dj);
                        if i < 0 || j < 0 {
                            continue;
                        }
                        if (di * di + dj * dj) as f64 * m.cell * m.cell <= r * r
                            && let Some(v) = m.get(i as usize, j as usize)
                            && v > z + 1e-6
                        {
                            return true;
                        }
                    }
                }
                false
            }
        }
    }
}

/// Splits a polyline into pieces where `keep` holds.
fn split_where(points: &[Vec3], keep: impl Fn(&Vec3) -> bool) -> Vec<Vec<Vec3>> {
    let mut out = Vec::new();
    let mut cur: Vec<Vec3> = Vec::new();
    for p in points {
        if keep(p) {
            cur.push(*p);
        } else if cur.len() >= 2 {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.clear();
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out
}

fn pocket3d(
    op: &Operation,
    ctx: &Context,
    stepover: f64,
    stepdown: f64,
    direction: Direction,
    flat_area_detection: bool,
    order_by_depth: bool,
) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let vtool = ctx.virtual_tool();
    let stepover = if stepover > 0.0 { stepover } else { default_stepover(tool, false) };
    let stepdown = if stepdown > 0.0 { stepdown } else { tool.diameter * 0.5 };
    let surf = Surface::build(ctx, &vtool)?;
    let (_, stock_top) = ctx.stock_z();
    let top = ctx.heights.top.min(stock_top);
    let bottom = ctx.heights.bottom.max(ctx.model_z().0) + ctx.stock_to_leave_axial;
    let mut zs = levels(top, bottom, stepdown)?;
    if flat_area_detection {
        for f in surf.flat_levels(ctx.tolerance) {
            if f > bottom - 1e-6 && f < top - 1e-6 && !zs.iter().any(|z| (z - f).abs() < ctx.tolerance * 2.0) {
                zs.push(f);
            }
        }
        zs.sort_by(|a, b| b.total_cmp(a));
    }
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    let mut notice = Notice::default();
    let mut from_z = ctx.heights.feed;
    let mut rings = 0usize;
    let eps = ctx.tolerance * 0.5;
    // Regions per level, each a set of loops; with order_by_depth, regions are grouped by
    // the area they lie in and each area is cut to the bottom before the next.
    let mut per_level: Vec<(f64, Vec<Poly>)> = Vec::new();
    for z in &zs {
        let mut region = surf.free_region(z + eps);
        if ctx.rest.is_some() {
            // Drop loops that enclose only air in the rest stock: keep a loop when any of its
            // points has material within the tool radius.
            let r = tool.radius();
            region.retain(|loop_| polyline::densify(loop_, surf.map.cell * 2.0).iter().any(|p| surf.has_material(ctx.rest, *p, r, *z)));
            region = polyline::normalize_set(&region);
        }
        if !region.is_empty() {
            per_level.push((*z, region));
        }
    }
    if order_by_depth {
        // Group by the outer loop at the top level each region's probe falls in.
        let tops: Vec<Poly> = per_level.first().map(|(_, r)| r.iter().filter(|p| polyline::is_ccw(p)).cloned().collect()).unwrap_or_default();
        let mut groups: Vec<Vec<(f64, Vec<Poly>)>> = vec![Vec::new(); tops.len().max(1)];
        for (z, region) in &per_level {
            let mut by_group: Vec<Vec<Poly>> = vec![Vec::new(); tops.len().max(1)];
            for loop_ in region {
                let probe = polyline::interior_probe(loop_);
                let g = tops.iter().position(|t| polyline::contains(t, probe)).unwrap_or(0);
                if let Some(v) = by_group.get_mut(g) {
                    v.push(loop_.clone());
                }
            }
            for (g, loops) in by_group.into_iter().enumerate() {
                if !loops.is_empty()
                    && let Some(gr) = groups.get_mut(g)
                {
                    gr.push((*z, polyline::normalize_set(&loops)));
                }
            }
        }
        for group in groups {
            let mut fz = from_z;
            for (z, region) in group {
                rings += pocket_level(&mut tp, &lk, &region, z, stepover, direction, true, fz, ctx.tolerance)?;
                fz = z;
            }
            lk.retract(&mut tp)?;
        }
    } else {
        for (z, region) in &per_level {
            rings += pocket_level(&mut tp, &lk, region, *z, stepover, direction, true, from_z, ctx.tolerance)?;
            from_z = *z;
        }
    }
    lk.retract_clearance(&mut tp)?;
    notice.value("levels", zs.len());
    notice.value("rings", rings);
    notice.value("cl_cells", surf.map.z.len());
    Ok((tp, notice))
}

/// Samples a 2D polyline onto the surface: exact drops at `step` spacing, clamped to the
/// bottom height and simplified to the tolerance.
fn drop_polyline(ctx: &Context, surf: &Surface, tool: &crate::tool::Tool, pts: &[Vec2], step: f64, slope: Option<(f64, f64)>) -> Vec<Vec<Vec3>> {
    let bottom = ctx.heights.bottom;
    let top = ctx.heights.top;
    let mut dense: Vec<Vec2> = Vec::new();
    for w in pts.windows(2) {
        let s = sample_segment(w[0], w[1], step);
        if dense.is_empty() {
            dense.extend(s);
        } else {
            dense.extend(s.into_iter().skip(1));
        }
    }
    if pts.len() == 1 {
        dense.push(pts[0]);
    }
    let dropped: Vec<Vec3> = dense
        .iter()
        .map(|p| {
            let z = ctx.drop(tool, *p, surf.floor).map_or(surf.floor, |c| c.z);
            Vec3::new(p.x, p.y, z.clamp(bottom, top.max(bottom)))
        })
        .collect();
    let in_bounds = |p: &Vec3| polyline::set_contains(&surf.boundary, Vec2::new(p.x, p.y));
    let in_slope = |p: &Vec3| {
        slope.is_none_or(|(lo, hi)| {
            let s = surf.slope_deg(Vec2::new(p.x, p.y));
            s >= lo - 1e-9 && s <= hi + 1e-9
        })
    };
    split_where(&dropped, |p| in_bounds(p) && in_slope(p)).into_iter().map(|piece| simplify3(&piece, ctx.tolerance)).collect()
}

/// The sample spacing along a pass for a chord error of the tolerance on a surface the tool
/// radius can follow.
fn sample_step(ctx: &Context) -> f64 {
    (8.0 * ctx.tolerance * ctx.tool.radius()).sqrt().clamp(ctx.tolerance * 2.0, ctx.tool.diameter)
}

#[allow(clippy::too_many_arguments)]
fn parallel(
    op: &Operation,
    ctx: &Context,
    stepover: f64,
    angle_deg: f64,
    direction: Direction,
    slope: (f64, f64),
    surface: Option<&Surface>,
) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let vtool = ctx.virtual_tool();
    let stepover = if stepover > 0.0 { stepover } else { default_stepover(tool, true) };
    let built;
    let surf = match surface {
        Some(s) => s,
        None => {
            built = Surface::build(ctx, &vtool)?;
            &built
        }
    };
    let rows = raster_passes(&surf.boundary, stepover, angle_deg.to_radians(), direction == Direction::Both, ctx.tolerance)?;
    let step = sample_step(ctx);
    let slope_opt = if slope.0 <= 0.0 && slope.1 >= 90.0 { None } else { Some(slope) };
    let mut passes: Vec<Pass> = Vec::new();
    for row in &rows {
        for piece in drop_polyline(ctx, surf, &vtool, row, step, slope_opt) {
            passes.push(Pass { points: piece, closed: false });
        }
    }
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    run_surface_passes(&lk, &mut tp, &passes, ctx, surf, &vtool, stepover * 2.5)?;
    lk.retract_clearance(&mut tp)?;
    let mut notice = Notice::default();
    notice.value("passes", passes.len());
    notice.value("stepover_mm", r3(stepover));
    if tool.corner_radius() <= 0.0 {
        notice.issue(Issue::info("flat_tool_finishing", "a flat end mill leaves steps on sloped surfaces; a ball or bull-nose tool follows them"));
    }
    Ok((tp, notice))
}

/// Links surface passes: consecutive passes whose ends are within `stay_down` are joined by a
/// move along the surface (dropped samples), others by a retract.
fn run_surface_passes(
    lk: &Linker,
    tp: &mut Toolpath,
    passes: &[Pass],
    ctx: &Context,
    surf: &Surface,
    tool: &crate::tool::Tool,
    stay_down: f64,
) -> Result<()> {
    let f = ctx.tool.feeds;
    for pass in passes {
        let Some(&start) = pass.points.first() else { continue };
        let linked = match tp.last_position() {
            Some(cur) if cur.z < ctx.heights.retract && Vec2::new(cur.x, cur.y).dist(Vec2::new(start.x, start.y)) <= stay_down => {
                // Link along the surface.
                let a = Vec2::new(cur.x, cur.y);
                let b = Vec2::new(start.x, start.y);
                let link = drop_polyline(ctx, surf, tool, &[a, b], sample_step(ctx), None);
                if link.len() == 1 {
                    for p in link[0].iter().skip(1) {
                        tp.push(crate::toolpath::Move::new(
                            crate::toolpath::MoveKind::Link,
                            Vec3::new(p.x, p.y, p.z + ctx.stock_to_leave_axial.max(0.0)),
                            f.cutting,
                        ))?;
                    }
                    tp.push(crate::toolpath::Move::new(crate::toolpath::MoveKind::Link, start, f.cutting))?;
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if !linked {
            lk.retract(tp)?;
            let h = tp.last_position().map_or(ctx.heights.clearance, |p| p.z.max(ctx.heights.retract));
            tp.rapid(Vec3::new(start.x, start.y, h))?;
            tp.rapid(Vec3::new(start.x, start.y, ctx.heights.feed.max(start.z)))?;
            tp.push(crate::toolpath::Move::new(crate::toolpath::MoveKind::Plunge, start, f.plunge))?;
        }
        for p in pass.points.iter().skip(1) {
            tp.cut(*p, f.cutting)?;
        }
        if pass.closed
            && let Some(first) = pass.points.first()
        {
            tp.cut(*first, f.cutting)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn waterline(
    op: &Operation,
    ctx: &Context,
    stepdown: f64,
    direction: Direction,
    slope: (f64, f64),
    flat_area_detection: bool,
    surface: Option<&Surface>,
) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let vtool = ctx.virtual_tool();
    let stepdown = if stepdown > 0.0 { stepdown } else { default_stepover(tool, true) };
    let built;
    let surf = match surface {
        Some(s) => s,
        None => {
            built = Surface::build(ctx, &vtool)?;
            &built
        }
    };
    let top = ctx.heights.top.min(ctx.model_z().1);
    let bottom = ctx.heights.bottom.max(ctx.model_z().0);
    let mut zs = levels(top, bottom, stepdown)?;
    if flat_area_detection {
        for f in surf.flat_levels(ctx.tolerance) {
            if f > bottom - 1e-6 && f < top - 1e-6 && !zs.iter().any(|z| (z - f).abs() < ctx.tolerance * 2.0) {
                zs.push(f + ctx.tolerance * 0.5);
            }
        }
        zs.sort_by(|a, b| b.total_cmp(a));
    }
    let slope_opt = if slope.0 <= 0.0 && slope.1 >= 90.0 { None } else { Some(slope) };
    let mut passes: Vec<Pass> = Vec::new();
    for z in &zs {
        for loop_ in surf.waterlines(*z) {
            let oriented = super::orient_for(&loop_, direction, !polyline::is_ccw(&loop_));
            let mut pts: Vec<Vec3> = oriented.iter().map(|p| Vec3::new(p.x, p.y, *z)).collect();
            if let Some(first) = pts.first().copied() {
                pts.push(first);
            }
            let keep = |p: &Vec3| {
                polyline::set_contains(&surf.boundary, Vec2::new(p.x, p.y))
                    && slope_opt.is_none_or(|(lo, hi)| {
                        let s = surf.slope_deg(Vec2::new(p.x, p.y));
                        s >= lo && s <= hi
                    })
            };
            let pieces = split_where(&pts, keep);
            let whole = pieces.len() == 1 && pieces[0].len() == pts.len();
            for piece in pieces {
                let simplified = simplify3(&piece, ctx.tolerance);
                passes.push(Pass { points: simplified, closed: whole });
            }
        }
    }
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    run_surface_passes(&lk, &mut tp, &passes, ctx, surf, &vtool, stepdown * 2.0)?;
    lk.retract_clearance(&mut tp)?;
    let mut notice = Notice::default();
    notice.value("levels", zs.len());
    notice.value("passes", passes.len());
    Ok((tp, notice))
}

fn scallop(op: &Operation, ctx: &Context, stepover: f64, direction: Direction) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let vtool = ctx.virtual_tool();
    let stepover = if stepover > 0.0 { stepover } else { default_stepover(tool, true) };
    let surf = Surface::build(ctx, &vtool)?;
    // Geodesic distance over the CL surface from the boundary (Dijkstra on the grid).
    let m = &surf.map;
    let n = m.z.len();
    let mut dist = vec![f64::INFINITY; n];
    let mut heap: std::collections::BinaryHeap<std::cmp::Reverse<(OrdF, usize)>> = std::collections::BinaryHeap::new();
    let idx = |i: usize, j: usize| j * m.nx + i;
    for j in 0..m.ny {
        for i in 0..m.nx {
            let k = idx(i, j);
            if !m.z[k].is_finite() {
                continue;
            }
            // Boundary cell: a neighbour is blocked.
            let mut edge = false;
            for (di, dj) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                let (ii, jj) = (i as i64 + di, j as i64 + dj);
                if ii < 0 || jj < 0 || ii >= m.nx as i64 || jj >= m.ny as i64 || !m.z[idx(ii as usize, jj as usize)].is_finite() {
                    edge = true;
                }
            }
            if edge {
                dist[k] = 0.0;
                heap.push(std::cmp::Reverse((OrdF(0.0), k)));
            }
        }
    }
    let mut guard = 0usize;
    while let Some(std::cmp::Reverse((OrdF(d), k))) = heap.pop() {
        guard += 1;
        if guard > n * 10 {
            break;
        }
        if d > dist[k] {
            continue;
        }
        let (i, j) = (k % m.nx, k / m.nx);
        for (di, dj) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
            let (ii, jj) = (i as i64 + di, j as i64 + dj);
            if ii < 0 || jj < 0 || ii >= m.nx as i64 || jj >= m.ny as i64 {
                continue;
            }
            let kk = idx(ii as usize, jj as usize);
            if !m.z[kk].is_finite() {
                continue;
            }
            let dxy = ((di * di + dj * dj) as f64).sqrt() * m.cell;
            let dz = m.z[kk] - m.z[k];
            let nd = d + (dxy * dxy + dz * dz).sqrt();
            if nd < dist[kk] {
                dist[kk] = nd;
                heap.push(std::cmp::Reverse((OrdF(nd), kk)));
            }
        }
    }
    let dmap =
        HeightMap { origin: m.origin, cell: m.cell, nx: m.nx, ny: m.ny, z: dist.iter().map(|d| if d.is_finite() { *d } else { -1.0 }).collect() };
    let max_d = dist.iter().copied().filter(|d| d.is_finite()).fold(0.0, f64::max);
    let rings = ((max_d / stepover).floor() as usize).min(100_000);
    let step = sample_step(ctx);
    let mut passes: Vec<Pass> = Vec::new();
    // From the boundary inward: the boundary itself first (distance stepover/2 in), then each
    // stepover.
    let mut last: Option<Vec2> = None;
    for k in 0..=rings {
        let level = stepover * 0.5 + k as f64 * stepover;
        if level > max_d {
            break;
        }
        for loop_ in dmap.contours(level) {
            let oriented = super::orient_for(&loop_, direction, true);
            let oriented = match last {
                Some(l) => super::start_near(&oriented, l),
                None => oriented,
            };
            last = oriented.first().copied();
            let mut pts2 = oriented.clone();
            if let Some(f) = pts2.first().copied() {
                pts2.push(f);
            }
            for piece in drop_polyline(ctx, &surf, &vtool, &pts2, step, None) {
                passes.push(Pass { points: piece, closed: false });
            }
        }
    }
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    run_surface_passes(&lk, &mut tp, &passes, ctx, &surf, &vtool, stepover * 3.0)?;
    lk.retract_clearance(&mut tp)?;
    let mut notice = Notice::default();
    notice.value("passes", passes.len());
    notice.value("surface_depth_mm", r3(max_d));
    Ok((tp, notice))
}

#[derive(PartialEq)]
struct OrdF(f64);
impl Eq for OrdF {}
impl PartialOrd for OrdF {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for OrdF {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

fn pencil(op: &Operation, ctx: &Context, min_angle_deg: f64, passes_n: u32, stepover: f64) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let vtool = ctx.virtual_tool();
    let min_angle = if min_angle_deg > 0.0 { min_angle_deg } else { 30.0 };
    let surf = Surface::build(ctx, &vtool)?;
    let m = &surf.map;
    // Contact normals per cell.
    let mut normals: Vec<Option<Vec3>> = vec![None; m.z.len()];
    for j in 0..m.ny {
        for i in 0..m.nx {
            if !m.z[j * m.nx + i].is_finite() {
                continue;
            }
            normals[j * m.nx + i] = ctx.drop(&vtool, m.point(i, j), surf.floor).map(|c| c.normal);
        }
    }
    // A pencil cell: the contact normal turns by more than the angle to a neighbour and the CL
    // surface is concave there (valley).
    let mut cells: Vec<(usize, usize)> = Vec::new();
    for j in 1..m.ny.saturating_sub(1) {
        for i in 1..m.nx.saturating_sub(1) {
            let k = j * m.nx + i;
            let (Some(n0), true) = (normals[k], m.z[k].is_finite()) else { continue };
            let lap = [(i + 1, j), (i - 1, j), (i, j + 1), (i, j - 1)]
                .iter()
                .filter_map(|(a, b)| m.get(*a, *b))
                .filter(|v| v.is_finite())
                .map(|v| v - m.z[k])
                .sum::<f64>();
            if lap <= m.cell * 0.05 {
                continue;
            }
            let turns = [(i + 1, j), (i, j + 1), (i - 1, j), (i, j - 1)]
                .iter()
                .filter_map(|(a, b)| normals[b * m.nx + a])
                .any(|n1| crate::frame::angle_between(n0, n1).to_degrees() >= min_angle);
            if turns {
                cells.push((i, j));
            }
        }
    }
    // Chain cells greedily into polylines.
    let mut used = vec![false; cells.len()];
    let mut chains: Vec<Vec<Vec2>> = Vec::new();
    for s in 0..cells.len() {
        if used[s] {
            continue;
        }
        used[s] = true;
        let mut chain = vec![m.point(cells[s].0, cells[s].1)];
        let mut cur = cells[s];
        loop {
            let mut best: Option<(usize, i64)> = None;
            for (k, c) in cells.iter().enumerate() {
                if used[k] {
                    continue;
                }
                let d = (c.0 as i64 - cur.0 as i64).pow(2) + (c.1 as i64 - cur.1 as i64).pow(2);
                if d <= 8 && best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((k, d));
                }
            }
            match best {
                Some((k, _)) => {
                    used[k] = true;
                    cur = cells[k];
                    chain.push(m.point(cur.0, cur.1));
                }
                None => break,
            }
        }
        if chain.len() >= 3 {
            chains.push(polyline::simplify_open(&chain, m.cell));
        }
    }
    let step = sample_step(ctx);
    let mut passes: Vec<Pass> = Vec::new();
    let n_passes = passes_n.max(1);
    let so = if stepover > 0.0 { stepover } else { tool.diameter * 0.1 };
    for chain in &chains {
        for k in 0..n_passes {
            // Extra passes offset sideways alternately.
            let off = if k == 0 { 0.0 } else { so * k.div_ceil(2) as f64 * if k % 2 == 1 { 1.0 } else { -1.0 } };
            let shifted: Vec<Vec2> = if off == 0.0 {
                chain.clone()
            } else {
                chain
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let a = chain[i.saturating_sub(1)];
                        let b = chain[(i + 1).min(chain.len() - 1)];
                        let t = (b - a).normalized().unwrap_or(Vec2::new(1.0, 0.0));
                        *p + Vec2::new(-t.y, t.x) * off
                    })
                    .collect()
            };
            for piece in drop_polyline(ctx, &surf, &vtool, &shifted, step, None) {
                passes.push(Pass { points: piece, closed: false });
            }
        }
    }
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    run_surface_passes(&lk, &mut tp, &passes, ctx, &surf, &vtool, tool.diameter)?;
    lk.retract_clearance(&mut tp)?;
    let mut notice = Notice::default();
    notice.value("corners", chains.len());
    notice.value("passes", passes.len());
    if chains.is_empty() {
        notice.issue(Issue::info("no_corners", format!("no inside corners sharper than {min_angle:.0}° were found for this tool")));
    }
    Ok((tp, notice))
}

fn spiral(op: &Operation, ctx: &Context, stepover: f64, center: Option<Vec2>, max_radius: f64) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let vtool = ctx.virtual_tool();
    let stepover = if stepover > 0.0 { stepover } else { default_stepover(tool, true) };
    let surf = Surface::build(ctx, &vtool)?;
    let b = polyline::bounds_of(&surf.boundary);
    let c = center.unwrap_or_else(|| b.center());
    let rmax = if max_radius > 0.0 { max_radius } else { b.diagonal() * 0.5 };
    let step = sample_step(ctx);
    let mut pts: Vec<Vec2> = vec![c];
    let mut theta = 0.0f64;
    let mut guard = 0usize;
    loop {
        guard += 1;
        if guard > 5_000_000 {
            return Err(CamError::TooLarge("spiral has too many points".into()));
        }
        let r = stepover * theta / std::f64::consts::TAU;
        if r > rmax {
            break;
        }
        pts.push(c + Vec2::from_angle(theta) * r);
        theta += step / r.max(step);
    }
    let passes: Vec<Pass> = drop_polyline(ctx, &surf, &vtool, &pts, step, None).into_iter().map(|p| Pass { points: p, closed: false }).collect();
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    run_surface_passes(&lk, &mut tp, &passes, ctx, &surf, &vtool, stepover * 3.0)?;
    lk.retract_clearance(&mut tp)?;
    let mut notice = Notice::default();
    notice.value("passes", passes.len());
    notice.value("max_radius_mm", r3(rmax));
    Ok((tp, notice))
}

fn radial(op: &Operation, ctx: &Context, angle_step_deg: f64, center: Option<Vec2>, inner_radius: f64) -> Result<(Toolpath, Notice)> {
    let vtool = ctx.virtual_tool();
    let surf = Surface::build(ctx, &vtool)?;
    let b = polyline::bounds_of(&surf.boundary);
    let c = center.unwrap_or_else(|| b.center());
    let rmax = b.diagonal() * 0.5 + c.dist(b.center());
    let step_deg = if angle_step_deg > 0.0 { angle_step_deg } else { (default_stepover(ctx.tool, true) / rmax).to_degrees().max(0.1) };
    let n = ((360.0 / step_deg).ceil() as usize).clamp(1, 100_000);
    let step = sample_step(ctx);
    let mut passes: Vec<Pass> = Vec::new();
    for k in 0..n {
        let a = (k as f64 * step_deg).to_radians();
        let dir = Vec2::from_angle(a);
        let mut line = vec![c + dir * inner_radius.max(0.0), c + dir * rmax];
        if k % 2 == 1 {
            line.reverse();
        }
        for piece in drop_polyline(ctx, &surf, &vtool, &line, step, None) {
            passes.push(Pass { points: piece, closed: false });
        }
    }
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    run_surface_passes(&lk, &mut tp, &passes, ctx, &surf, &vtool, step_deg.to_radians() * rmax * 2.0)?;
    lk.retract_clearance(&mut tp)?;
    let mut notice = Notice::default();
    notice.value("rays", n);
    Ok((tp, notice))
}

fn morphed_spiral(op: &Operation, ctx: &Context, stepover: f64) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let vtool = ctx.virtual_tool();
    let stepover = if stepover > 0.0 { stepover } else { default_stepover(tool, true) };
    let surf = Surface::build(ctx, &vtool)?;
    // Rings of the boundary stepping inward; one spiral per outer area, from the inside out.
    let family = offset::inward_family(&surf.boundary, stepover, ctx.tolerance, 100_000)?;
    let outers: Vec<Poly> = family[0].iter().filter(|p| polyline::is_ccw(p)).cloned().collect();
    let step = sample_step(ctx);
    let mut passes: Vec<Pass> = Vec::new();
    for outer in &outers {
        // The rings of this area: the ring of each depth that lies inside `outer` (first found).
        let rings: Vec<Poly> = family
            .iter()
            .filter_map(|rs| rs.iter().find(|r| polyline::is_ccw(r) && polyline::contains(outer, polyline::interior_probe(r))).cloned())
            .collect();
        if rings.is_empty() {
            continue;
        }
        let n = (polyline::perimeter(outer) / step).ceil().clamp(16.0, 100_000.0) as usize;
        let resampled: Vec<Vec<Vec2>> = rings.iter().map(|r| resample_closed(r, n)).collect();
        // Align ring starts to the outer ring's start.
        let start = resampled[0][0];
        let aligned: Vec<Vec<Vec2>> = resampled.iter().map(|r| super::start_near(r, start)).collect();
        let mut pts: Vec<Vec2> = Vec::new();
        // Inside out: from the innermost ring, blending to the next outer one over one turn.
        for k in (0..aligned.len()).rev() {
            let inner = &aligned[k];
            let outer_r = if k > 0 { &aligned[k - 1] } else { &aligned[k] };
            for i in 0..n {
                let t = if k > 0 { i as f64 / n as f64 } else { 0.0 };
                pts.push(inner[i].lerp(outer_r[i], t));
            }
        }
        if let Some(f) = aligned[0].first() {
            pts.push(*f);
        }
        for piece in drop_polyline(ctx, &surf, &vtool, &pts, step, None) {
            passes.push(Pass { points: piece, closed: false });
        }
    }
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    run_surface_passes(&lk, &mut tp, &passes, ctx, &surf, &vtool, stepover * 3.0)?;
    lk.retract_clearance(&mut tp)?;
    let mut notice = Notice::default();
    notice.value("areas", outers.len());
    notice.value("passes", passes.len());
    Ok((tp, notice))
}

/// `n` points evenly spaced by arc length around a closed loop.
pub fn resample_closed(p: &[Vec2], n: usize) -> Vec<Vec2> {
    let per = polyline::perimeter(p);
    if p.is_empty() || per <= 0.0 {
        return p.to_vec();
    }
    let m = p.len();
    let mut out = Vec::with_capacity(n);
    let mut seg = 0usize;
    let mut seg_start = 0.0;
    let mut seg_len = p[0].dist(p[1 % m]);
    for k in 0..n {
        let s = per * k as f64 / n as f64;
        let mut guard = 0;
        while s > seg_start + seg_len && guard < m * 2 {
            seg += 1;
            seg_start += seg_len;
            seg_len = p[seg % m].dist(p[(seg + 1) % m]);
            guard += 1;
        }
        let t = if seg_len > 0.0 { ((s - seg_start) / seg_len).clamp(0.0, 1.0) } else { 0.0 };
        out.push(p[seg % m].lerp(p[(seg + 1) % m], t));
    }
    out
}

fn project(op: &Operation, ctx: &Context) -> Result<(Toolpath, Notice)> {
    let vtool = ctx.virtual_tool();
    if ctx.geometry.loops.is_empty() && ctx.geometry.open.is_empty() {
        return Err(CamError::Geometry("project needs loops or chains to project".into()));
    }
    // The boundary is the stock here: the loops are the paths, not a limit.
    let outline = ctx.stock_outline();
    let b = polyline::bounds_of(&outline).inflated(ctx.tool.radius());
    let floor = ctx.heights.bottom.min(ctx.model_z().0) - 1.0;
    let map = HeightMap::new(b, ctx.cell_for(&b), floor)?;
    let surf = Surface { boundary: vec![polyline::rect(b.min, b.max)], map, floor };
    let step = sample_step(ctx);
    let mut passes: Vec<Pass> = Vec::new();
    for l in &ctx.geometry.loops {
        let mut pts = l.clone();
        if let Some(f) = pts.first().copied() {
            pts.push(f);
        }
        for piece in drop_polyline(ctx, &surf, &vtool, &pts, step, None) {
            passes.push(Pass { points: piece, closed: false });
        }
    }
    for c in &ctx.geometry.open {
        for piece in drop_polyline(ctx, &surf, &vtool, c, step, None) {
            passes.push(Pass { points: piece, closed: false });
        }
    }
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    run_surface_passes(&lk, &mut tp, &passes, ctx, &surf, &vtool, 0.0)?;
    lk.retract_clearance(&mut tp)?;
    let mut notice = Notice::default();
    notice.value("passes", passes.len());
    Ok((tp, notice))
}

fn horizontal(op: &Operation, ctx: &Context, stepover: f64, direction: Direction) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let vtool = ctx.virtual_tool();
    let stepover = if stepover > 0.0 { stepover } else { default_stepover(tool, false) };
    let surf = Surface::build(ctx, &vtool)?;
    let flats = surf.flat_levels(ctx.tolerance);
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    let mut areas = 0usize;
    let tol = ctx.tolerance * 2.0;
    for z in flats.iter().rev() {
        if *z < ctx.heights.bottom - 1e-6 || *z > ctx.heights.top + 1e-6 {
            continue;
        }
        // The flat region: cells within tol of the level.
        let m = &surf.map;
        let flat = HeightMap {
            origin: m.origin,
            cell: m.cell,
            nx: m.nx,
            ny: m.ny,
            z: m.z.iter().map(|v| if v.is_finite() { -(v - z).abs() } else { f64::NEG_INFINITY }).collect(),
        };
        let region = flat.contours(-tol);
        if region.is_empty() {
            continue;
        }
        areas += region.iter().filter(|p| polyline::is_ccw(p)).count();
        pocket_level(&mut tp, &lk, &region, *z, stepover, direction, true, ctx.heights.feed, ctx.tolerance)?;
        lk.retract(&mut tp)?;
    }
    lk.retract_clearance(&mut tp)?;
    let mut notice = Notice::default();
    notice.value("flat_levels", flats.len());
    notice.value("areas", areas);
    if flats.is_empty() {
        notice.issue(Issue::info("no_flat_areas", "no flat areas were found within the boundary"));
    }
    Ok((tp, notice))
}

fn steep_shallow(
    op: &Operation,
    ctx: &Context,
    threshold_deg: f64,
    steep_stepdown: f64,
    shallow_stepover: f64,
    angle_deg: f64,
    steep_first: bool,
) -> Result<(Toolpath, Notice)> {
    let vtool = ctx.virtual_tool();
    let threshold = if threshold_deg > 0.0 { threshold_deg } else { 45.0 };
    let surf = Surface::build(ctx, &vtool)?;
    let steep = || waterline(op, ctx, steep_stepdown, Direction::Climb, (threshold, 90.0), true, Some(&surf));
    let shallow = || parallel(op, ctx, shallow_stepover, angle_deg, Direction::Both, (0.0, threshold), Some(&surf));
    let (first, second) = if steep_first { (steep()?, shallow()?) } else { (shallow()?, steep()?) };
    let mut tp = first.0;
    tp.extend(&second.0)?;
    let mut notice = Notice::default();
    notice.value("threshold_deg", threshold);
    notice.value(
        "steep_passes",
        if steep_first { first.1.values.get("passes").cloned() } else { second.1.values.get("passes").cloned() }.unwrap_or_default(),
    );
    notice.value(
        "shallow_passes",
        if steep_first { second.1.values.get("passes").cloned() } else { first.1.values.get("passes").cloned() }.unwrap_or_default(),
    );
    Ok((tp, notice))
}

/// The boundary bounds helper for tests.
pub fn boundary_bounds(ctx: &Context) -> Result<Aabb2> {
    Ok(polyline::bounds_of(&ctx.boundary()?))
}
