//! 2.5D operations: facing, pocketing, adaptive clearing, contouring, chamfering, drilling and
//! boring. They work on closed loops in the operation frame's XY and cut in flat levels.

use solidcraft_geom::{Vec2, Vec3};

use super::{Context, Direction, OpKind, Side, default_stepover, levels, orient_for, start_near};
use crate::linking::{Linker, Pass};
use crate::notice::{Issue, Notice, r3};
use crate::offset;
use crate::polyline::{self, Poly};
use crate::setup::Operation;
use crate::toolpath::{DrillCycle, DrillCycleKind, Move, MoveKind, Toolpath};
use crate::{CamError, Result};

pub fn generate(op: &Operation, ctx: &Context) -> Result<(Toolpath, Notice)> {
    match &op.kind {
        OpKind::Face { stepover, stepdown, angle_deg, direction, extension } => {
            face(op, ctx, *stepover, *stepdown, *angle_deg, *direction, *extension)
        }
        OpKind::Pocket2d { stepover, stepdown, direction, inside_out, finishing_stepover } => {
            pocket(op, ctx, *stepover, *stepdown, *direction, *inside_out, *finishing_stepover, None)
        }
        OpKind::Adaptive2d { optimal_load, stepdown, minimum_radius, direction } => {
            adaptive(op, ctx, *optimal_load, *stepdown, *minimum_radius, *direction)
        }
        OpKind::Contour2d {
            side,
            stepdown,
            direction,
            roughing_passes,
            roughing_stepover,
            finishing_passes,
            finishing_stepover,
            tabs,
            tab_width,
            tab_height,
        } => contour(
            op,
            ctx,
            *side,
            *stepdown,
            *direction,
            *roughing_passes,
            *roughing_stepover,
            *finishing_passes,
            *finishing_stepover,
            (*tabs, *tab_width, *tab_height),
        ),
        OpKind::Chamfer2d { width, tip_offset, side } => chamfer(op, ctx, *width, *tip_offset, *side),
        OpKind::Drill { cycle, peck, dwell_s, pitch, break_through, optimize_order } => {
            drill(op, ctx, *cycle, *peck, *dwell_s, *pitch, *break_through, *optimize_order)
        }
        OpKind::Bore { pitch, direction } => bore(op, ctx, *pitch, *direction),
        OpKind::Slot { width, stepdown, direction } => slot(op, ctx, *width, *stepdown, *direction),
        OpKind::Trace { stepdown, direction } => {
            let lk = linker(op, ctx);
            follow_curves(ctx, &lk, ctx.heights.bottom + ctx.stock_to_leave_axial, *stepdown, *direction)
        }
        OpKind::Engrave { width, stepdown } => engrave(op, ctx, *width, *stepdown),
        OpKind::Circular { stepover, stepdown, direction } => circular(op, ctx, *stepover, *stepdown, *direction),
        OpKind::Thread { pitch, passes, external } => thread(op, ctx, *pitch, *passes, *external),
        _ => Err(CamError::Param("not a 2D operation".into())),
    }
}

fn linker<'a>(op: &Operation, ctx: &'a Context) -> Linker<'a> {
    Linker { tool: ctx.tool, heights: ctx.heights, linking: op.linking, tolerance: ctx.tolerance }
}

/// The loops of the geometry selection, or the stock outline.
fn loops_or_stock(ctx: &Context) -> Vec<Poly> {
    if ctx.geometry.loops.is_empty() { ctx.stock_outline() } else { polyline::normalize_set(&ctx.geometry.loops) }
}

/// Parallel passes over a region at `angle` with `stepover`, zigzag when `both`. Returns the
/// passes in order (XY), each a straight segment clipped to the region.
pub fn raster_passes(region: &[Poly], stepover: f64, angle: f64, both: bool, tol: f64) -> Result<Vec<Vec<Vec2>>> {
    crate::positive("stepover", stepover)?;
    // Rotate the region so the passes run along x.
    let (s, c) = (-angle).sin_cos();
    let rot = |p: Vec2| Vec2::new(p.x * c - p.y * s, p.x * s + p.y * c);
    let (s2, c2) = angle.sin_cos();
    let unrot = |p: Vec2| Vec2::new(p.x * c2 - p.y * s2, p.x * s2 + p.y * c2);
    let rotated: Vec<Poly> = region.iter().map(|p| p.iter().map(|q| rot(*q)).collect()).collect();
    let b = polyline::bounds_of(&rotated);
    if b.is_empty() {
        return Ok(Vec::new());
    }
    let height = b.size().y;
    let n = ((height / stepover).floor() as usize).min(1_000_000);
    // Centre the passes so the first and last lie `≤ stepover/2` from the edges.
    let total = n as f64 * stepover;
    let y0 = b.min.y + (height - total) * 0.5;
    let mut out = Vec::new();
    let mut flip = false;
    for k in 0..=n {
        let y = (y0 + k as f64 * stepover).clamp(b.min.y + tol, b.max.y - tol);
        let spans = polyline::scanline(&rotated, y);
        let mut row: Vec<Vec<Vec2>> = spans.iter().map(|(a, bb)| vec![unrot(Vec2::new(*a, y)), unrot(Vec2::new(*bb, y))]).collect();
        if both && flip {
            row.reverse();
            for r in &mut row {
                r.reverse();
            }
        }
        out.extend(row);
        flip = !flip;
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn face(
    op: &Operation,
    ctx: &Context,
    stepover: f64,
    stepdown: f64,
    angle_deg: f64,
    direction: Direction,
    extension: f64,
) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let stepover = if stepover > 0.0 { stepover } else { tool.diameter * 0.8 };
    let stepdown = if stepdown > 0.0 { stepdown } else { tool.diameter * 0.1 };
    let ext = if extension > 0.0 { extension } else { tool.radius() };
    let outline = loops_or_stock(ctx);
    // The tool centre runs past the outline by the extension so the cutter clears the edge.
    let region = offset::offset_set(&outline, ext, ctx.tolerance)?;
    let zs = levels(ctx.heights.top, ctx.heights.bottom, stepdown)?;
    let both = direction == Direction::Both;
    let mut tp = Toolpath::new();
    let lk = linker(op, ctx);
    let mut notice = Notice::default();
    let mut from_z = ctx.heights.feed;
    for z in &zs {
        let rows = raster_passes(&region, stepover, angle_deg.to_radians(), both, ctx.tolerance)?;
        let passes: Vec<Pass> =
            rows.into_iter().map(|r| Pass { points: r.into_iter().map(|p| Vec3::new(p.x, p.y, *z)).collect(), closed: false }).collect();
        // Facing starts in the air past the edge, so links between passes stay down.
        let mut linking = op.linking;
        if both {
            linking.stay_down_distance = linking.stay_down_distance.max(stepover * 2.0);
        }
        let lk2 = Linker { linking, ..lk };
        lk2.run(&mut tp, &passes, Some(&region), from_z)?;
        from_z = *z;
    }
    lk.retract_clearance(&mut tp)?;
    notice.value("levels", zs.len());
    notice.value("stepover_mm", r3(stepover));
    Ok((tp, notice))
}

/// Contour-parallel pocketing of `region` (the loops to clear, holes are islands). Shared by
/// 2D pocketing and each level of 3D roughing. `cl_region` is where the tool centre may be:
/// the region offset inward by the tool radius (and stock to leave).
#[allow(clippy::too_many_arguments)]
pub fn pocket_level(
    tp: &mut Toolpath,
    lk: &Linker,
    cl_region: &[Poly],
    z: f64,
    stepover: f64,
    direction: Direction,
    inside_out: bool,
    from_z: f64,
    tol: f64,
) -> Result<usize> {
    if cl_region.is_empty() {
        return Ok(0);
    }
    let family = offset::inward_family(cl_region, stepover, tol, 100_000)?;
    // Collect rings with their depth index; order innermost first within each connected area.
    // Rings are grouped by the outermost ring that contains them.
    let outers: Vec<&Poly> = family[0].iter().filter(|p| polyline::is_ccw(p)).collect();
    let mut groups: Vec<Vec<(usize, Poly)>> = vec![Vec::new(); outers.len().max(1)];
    for (depth, rings) in family.iter().enumerate() {
        for ring in rings {
            let probe = polyline::interior_probe(ring);
            let g = outers.iter().position(|o| polyline::contains(o, probe)).unwrap_or(0);
            if let Some(gr) = groups.get_mut(g) {
                gr.push((depth, ring.clone()));
            }
        }
    }
    let mut count = 0usize;
    let mut last: Option<Vec2> = tp.last_position().map(|p| Vec2::new(p.x, p.y));
    for group in groups {
        if group.is_empty() {
            continue;
        }
        let ordered = super::order_rings(group, inside_out);
        let mut passes = Vec::new();
        for ring in ordered {
            // Holes (clockwise in the normalized region) are islands: the tool runs around them
            // the other way round, so orient by the ring's role.
            let is_outer = polyline::is_ccw(&ring);
            let oriented = orient_for(&ring, direction, is_outer);
            let oriented = match last {
                Some(l) => start_near(&oriented, l),
                None => oriented,
            };
            last = oriented.first().copied();
            passes.push(Pass { points: oriented.iter().map(|p| Vec3::new(p.x, p.y, z)).collect(), closed: true });
            count += 1;
        }
        // Within one area, consecutive rings are a stepover apart: stay down between them.
        let mut linking = lk.linking;
        linking.stay_down_distance = linking.stay_down_distance.max(stepover * 3.0);
        let lk2 = Linker { linking, ..*lk };
        lk2.run(tp, &passes, Some(cl_region), from_z)?;
    }
    Ok(count)
}

#[allow(clippy::too_many_arguments)]
fn pocket(
    op: &Operation,
    ctx: &Context,
    stepover: f64,
    stepdown: f64,
    direction: Direction,
    inside_out: bool,
    finishing_stepover: f64,
    radius_override: Option<f64>,
) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let stepover = if stepover > 0.0 { stepover } else { default_stepover(tool, false) };
    let stepdown = if stepdown > 0.0 { stepdown } else { tool.diameter * 0.5 };
    let r = radius_override.unwrap_or(tool.radius()) + ctx.stock_to_leave_radial + finishing_stepover;
    let region = loops_or_stock(ctx);
    if ctx.geometry.loops.is_empty() {
        return Err(CamError::Geometry("2D pocket needs closed loops (sketch profiles, faces or explicit loops)".into()));
    }
    let cl = offset::offset_set(&region, -r, ctx.tolerance)?;
    let mut notice = Notice::default();
    if cl.is_empty() {
        notice.issue(
            Issue::error("tool_too_large", format!("a {:.2} mm tool does not fit the pocket", tool.diameter))
                .parameter("tool", tool.id)
                .suggest(serde_json::json!({"tool_diameter_max": r3(polyline::largest_inscribed(&region, 64).map_or(0.0, |x| x.1 * 2.0))})),
        );
        return Ok((Toolpath::new(), notice));
    }
    let bottom = ctx.heights.bottom + ctx.stock_to_leave_axial;
    let zs = levels(ctx.heights.top, bottom, stepdown)?;
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    let mut from_z = ctx.heights.feed;
    let mut rings = 0;
    for z in &zs {
        rings += pocket_level(&mut tp, &lk, &cl, *z, stepover, direction, inside_out, from_z, ctx.tolerance)?;
        from_z = *z;
    }
    if finishing_stepover > 0.0 {
        let wall = offset::offset_set(&region, -(tool.radius() + ctx.stock_to_leave_radial), ctx.tolerance)?;
        let passes: Vec<Pass> = wall
            .iter()
            .map(|ring| {
                let oriented = orient_for(ring, direction, polyline::is_ccw(ring));
                Pass { points: oriented.iter().map(|p| Vec3::new(p.x, p.y, bottom)).collect(), closed: true }
            })
            .collect();
        lk.run(&mut tp, &passes, Some(&wall), bottom)?;
    }
    lk.retract_clearance(&mut tp)?;
    notice.value("levels", zs.len());
    notice.value("rings", rings);
    notice.value("area_mm2", r3(polyline::set_area(&region)));
    Ok((tp, notice))
}

/// Adaptive clearing. The clearing works inside out from the largest inscribed circle: a helix
/// entry there, then rings at the optimal load, so the radial engagement never exceeds it
/// except where a ring first meets uncut material on a narrow neck, where a trochoidal loop is
/// inserted. Inside corners are rounded to the minimum radius by a double offset.
fn adaptive(
    op: &Operation,
    ctx: &Context,
    optimal_load: f64,
    stepdown: f64,
    minimum_radius: f64,
    direction: Direction,
) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let load = if optimal_load > 0.0 { optimal_load } else { tool.diameter * 0.2 };
    let stepdown = if stepdown > 0.0 { stepdown } else { tool.flute_length.min(tool.diameter * 2.0) };
    let min_r = if minimum_radius > 0.0 { minimum_radius } else { tool.radius() * 0.5 };
    if ctx.geometry.loops.is_empty() {
        return Err(CamError::Geometry("2D adaptive needs closed loops".into()));
    }
    let region = polyline::normalize_set(&ctx.geometry.loops);
    let r = tool.radius() + ctx.stock_to_leave_radial;
    let cl = offset::offset_set(&region, -r, ctx.tolerance)?;
    let mut notice = Notice::default();
    if cl.is_empty() {
        notice.issue(Issue::error("tool_too_large", format!("a {:.2} mm tool does not fit the pocket", tool.diameter)).parameter("tool", tool.id));
        return Ok((Toolpath::new(), notice));
    }
    // Round inside corners of the cutter-location region: shrink then grow by the minimum radius.
    let smoothed = {
        let inner = offset::offset_set(&cl, -min_r, ctx.tolerance)?;
        if inner.is_empty() { cl.clone() } else { offset::offset_set(&inner, min_r, ctx.tolerance)? }
    };
    let bottom = ctx.heights.bottom + ctx.stock_to_leave_axial;
    let zs = levels(ctx.heights.top, bottom, stepdown)?;
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    let mut from_z = ctx.heights.feed;
    let mut trochoids = 0usize;
    let mut rings = 0usize;
    for z in &zs {
        let (r, t) = adaptive_level(&mut tp, &lk, &cl, &smoothed, *z, load, direction, from_z, ctx.tolerance)?;
        rings += r;
        trochoids += t;
        from_z = *z;
    }
    lk.retract_clearance(&mut tp)?;
    notice.value("levels", zs.len());
    notice.value("rings", rings);
    notice.value("trochoids", trochoids);
    notice.value("optimal_load_mm", r3(load));
    Ok((tp, notice))
}

/// The cutter-location region with its inside corners rounded to `min_r` (shrink then grow).
pub fn round_corners(cl: &[Poly], min_r: f64, tol: f64) -> Result<Vec<Poly>> {
    let inner = offset::offset_set(cl, -min_r, tol)?;
    if inner.is_empty() { Ok(cl.to_vec()) } else { offset::offset_set(&inner, min_r, tol) }
}

/// One level of adaptive clearing of the cutter-location region `cl` (its corner-rounded form
/// `smoothed` gives the rings): inside-out rings at the optimal load with trochoids where a
/// ring first meets uncut material. Returns the rings and trochoids made.
#[allow(clippy::too_many_arguments)]
pub fn adaptive_level(
    tp: &mut Toolpath,
    lk: &Linker,
    cl: &[Poly],
    smoothed: &[Poly],
    z: f64,
    load: f64,
    direction: Direction,
    from_z: f64,
    tol: f64,
) -> Result<(usize, usize)> {
    let mut trochoids = 0usize;
    let mut rings = 0usize;
    let ctx_tol = tol;
    {
        let z = &z;
        let ctx = CtxTol { tolerance: ctx_tol };
        let cl = cl.to_vec();
        let op_linking = lk.linking;
        // Rings from the inside out at the optimal load.
        let family = offset::inward_family(smoothed, load, ctx.tolerance, 100_000)?;
        let mut passes: Vec<Pass> = Vec::new();
        let mut last = tp.last_position().map(|p| Vec2::new(p.x, p.y));
        for rings_at in family.iter().rev() {
            for ring in rings_at {
                let is_outer = polyline::is_ccw(ring);
                let oriented = orient_for(ring, direction, is_outer);
                let oriented = match last {
                    Some(l) => start_near(&oriented, l),
                    None => oriented,
                };
                // Where this ring is farther than the load from the previous (inner) ring, the
                // tool would take a full-width cut: insert trochoidal loops there.
                let inner: Vec<Poly> = family.iter().rev().skip_while(|f| !std::ptr::eq(*f, rings_at)).nth(1).cloned().unwrap_or_default();
                let mut pts: Vec<Vec3> = Vec::new();
                let dense = polyline::densify(&oriented, load);
                for p in &dense {
                    let engaged = if inner.is_empty() { true } else { polyline::distance_to_set(&inner, *p) > load * 1.5 };
                    if engaged && !inner.is_empty() {
                        // A trochoid: a small circle of radius load about the point, which keeps
                        // the engagement near the load.
                        let k = polyline::arc_segments(load, std::f64::consts::TAU, ctx.tolerance).max(8);
                        for i in 0..=k {
                            let a = i as f64 / k as f64 * std::f64::consts::TAU;
                            let q = *p + Vec2::from_angle(a) * load;
                            if polyline::set_contains(&cl, q) {
                                pts.push(Vec3::new(q.x, q.y, *z));
                            }
                        }
                        trochoids += 1;
                    }
                    pts.push(Vec3::new(p.x, p.y, *z));
                }
                last = oriented.first().copied();
                passes.push(Pass { points: pts, closed: true });
                rings += 1;
            }
        }
        let mut linking = op_linking;
        linking.stay_down_distance = linking.stay_down_distance.max(load * 3.0);
        let lk2 = Linker { linking, ..*lk };
        lk2.run(tp, &passes, Some(&cl), from_z)?;
    }
    Ok((rings, trochoids))
}

/// Just a tolerance, so the adaptive ring code reads `ctx.tolerance` whether it runs in 2D or
/// per 3D level.
struct CtxTol {
    tolerance: f64,
}

/// The cutter-location loops for a contour: offset by the tool radius to the chosen side.
fn contour_cl(loops: &[Poly], side: Side, r: f64, tol: f64) -> Result<Vec<Poly>> {
    match side {
        Side::On => Ok(loops.to_vec()),
        Side::Outside => offset::offset_set(loops, r, tol),
        Side::Inside => offset::offset_set(loops, -r, tol),
    }
}

#[allow(clippy::too_many_arguments)]
fn contour(
    op: &Operation,
    ctx: &Context,
    side: Side,
    stepdown: f64,
    direction: Direction,
    roughing_passes: u32,
    roughing_stepover: f64,
    finishing_passes: u32,
    finishing_stepover: f64,
    tabs: (u32, f64, f64),
) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let stepdown = if stepdown > 0.0 { stepdown } else { tool.diameter * 0.5 };
    let loops = if ctx.geometry.loops.is_empty() && ctx.geometry.open.is_empty() {
        ctx.stock_outline()
    } else {
        polyline::normalize_set(&ctx.geometry.loops)
    };
    let mut notice = Notice::default();
    let bottom = ctx.heights.bottom + ctx.stock_to_leave_axial;
    let zs = levels(ctx.heights.top, bottom, stepdown)?;
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    let r = tool.radius() + ctx.stock_to_leave_radial;
    // Radial offsets of the passes, outermost (most stock) first: roughing passes stepping in,
    // then the finishing passes.
    let mut offsets: Vec<(f64, bool)> = Vec::new();
    let rs = if roughing_stepover > 0.0 { roughing_stepover } else { tool.diameter * 0.5 };
    for k in (1..=roughing_passes).rev() {
        offsets.push((k as f64 * rs, false));
    }
    let fs = if finishing_stepover > 0.0 { finishing_stepover } else { 0.0 };
    let fp = finishing_passes.max(1);
    for k in (0..fp).rev() {
        offsets.push((k as f64 * fs, true));
    }
    let mut from_z = ctx.heights.feed;
    let mut pass_count = 0usize;
    for z in &zs {
        for (extra, _finish) in &offsets {
            let d = match side {
                Side::On => 0.0,
                Side::Outside => r + extra,
                Side::Inside => r + extra,
            };
            let cl = if side == Side::On { loops.clone() } else { contour_cl(&loops, side, d, ctx.tolerance)? };
            if cl.is_empty() && side == Side::Inside {
                notice.issue(
                    Issue::error("tool_too_large", format!("a {:.2} mm tool does not fit inside the contour", tool.diameter))
                        .parameter("tool", tool.id),
                );
                break;
            }
            let mut passes: Vec<Pass> = Vec::new();
            for ring in &cl {
                // Outside a boss the tool is outside the loop; inside a pocket, inside it.
                let pocket_like = side == Side::Inside;
                let oriented = orient_for(ring, direction, pocket_like);
                let pts = with_tabs(&oriented, *z, bottom, tabs, ctx.tolerance);
                passes.push(Pass { points: pts, closed: true });
                pass_count += 1;
            }
            for chain in &ctx.geometry.open {
                let pts: Vec<Vec3> = chain.iter().map(|p| Vec3::new(p.x, p.y, *z)).collect();
                passes.push(Pass { points: pts, closed: false });
                pass_count += 1;
            }
            lk.run(&mut tp, &passes, None, from_z)?;
        }
        from_z = *z;
    }
    lk.retract_clearance(&mut tp)?;
    notice.value("levels", zs.len());
    notice.value("passes", pass_count);
    Ok((tp, notice))
}

/// The loop at level `z` with `tabs.0` tabs of width `tabs.1` and height `tabs.2` above the
/// bottom, raised where the tool passes over a tab (only on the bottom level).
fn with_tabs(ring: &[Vec2], z: f64, bottom: f64, tabs: (u32, f64, f64), tol: f64) -> Vec<Vec3> {
    let (n, w, h) = tabs;
    let tab_top = bottom + h;
    if n == 0 || w <= 0.0 || h <= 0.0 || z >= tab_top - 1e-9 {
        return ring.iter().map(|p| Vec3::new(p.x, p.y, z)).collect();
    }
    let per = polyline::perimeter(ring);
    if per <= w * n as f64 {
        return ring.iter().map(|p| Vec3::new(p.x, p.y, z)).collect();
    }
    let spacing = per / n as f64;
    let dense = polyline::densify(ring, (tol * 10.0).max(0.2));
    let mut out = Vec::with_capacity(dense.len() + 4 * n as usize);
    let mut run = 0.0;
    let m = dense.len();
    for i in 0..=m {
        let p = dense[i % m];
        if i > 0 {
            run += dense[i - 1].dist(p);
        }
        // In a tab when within w/2 of a multiple of the spacing (centred at spacing/2).
        let phase = (run + spacing * 0.5) % spacing;
        let in_tab = phase < w * 0.5 || phase > spacing - w * 0.5;
        out.push(Vec3::new(p.x, p.y, if in_tab { tab_top } else { z }));
    }
    out.pop();
    out
}

fn chamfer(op: &Operation, ctx: &Context, width: f64, tip_offset: f64, side: Side) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let crate::tool::ToolKind::Chamfer { tip_angle_deg } = tool.kind else {
        return Err(CamError::Param("2D chamfer needs a chamfer mill".into()));
    };
    let width = if width > 0.0 { width } else { 1.0 };
    let half = (tip_angle_deg * 0.5).to_radians();
    let loops = polyline::normalize_set(&ctx.geometry.loops);
    if loops.is_empty() {
        return Err(CamError::Geometry("2D chamfer needs closed loops (the edges to chamfer)".into()));
    }
    // The tool's cutting point on the chamfer's lower edge sits `tip_offset` above the tip:
    // the tip is radially tip_offset·tan(half) from the axis there... The tool centre offset from
    // the edge is the radius of the cone at the chamfer's lower edge plus the chamfer width.
    let r_at_edge = tip_offset * half.tan() + width;
    let z = ctx.heights.top - width / half.tan().max(1e-6) - tip_offset;
    let cl = contour_cl(&loops, side, r_at_edge, ctx.tolerance)?;
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    let passes: Vec<Pass> = cl
        .iter()
        .map(|ring| Pass {
            points: orient_for(ring, Direction::Climb, side == Side::Inside).iter().map(|p| Vec3::new(p.x, p.y, z)).collect(),
            closed: true,
        })
        .collect();
    lk.run(&mut tp, &passes, None, ctx.heights.feed)?;
    lk.retract_clearance(&mut tp)?;
    let mut notice = Notice::default();
    notice.value("chamfer_z_mm", r3(z));
    Ok((tp, notice))
}

#[allow(clippy::too_many_arguments)]
fn drill(
    _op: &Operation,
    ctx: &Context,
    cycle: DrillCycleKind,
    peck: f64,
    dwell_s: f64,
    pitch: f64,
    break_through: f64,
    optimize_order: bool,
) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    if cycle == DrillCycleKind::Tap && pitch <= 0.0 {
        return Err(CamError::Param("tapping needs `pitch` (mm per turn)".into()));
    }
    // Tapping feeds at the pitch times the spindle speed.
    let tap_feed = pitch * tool.feeds.spindle_rpm;
    let mut holes = ctx.geometry.holes.clone();
    if holes.is_empty() {
        return Err(CamError::Geometry("drilling needs hole positions (cylindrical faces or points)".into()));
    }
    let mut notice = Notice::default();
    let d = ctx.geometry.hole_diameter;
    if d > 0.0 && (d - tool.diameter).abs() > 0.05 {
        notice.issue(
            Issue::warning("tool_diameter_mismatch", format!("the holes are {:.2} mm but the tool is {:.2} mm", d, tool.diameter))
                .parameter("tool", tool.id)
                .limit(r3(d)),
        );
    }
    if optimize_order {
        let mut ordered: Vec<Vec3> = Vec::with_capacity(holes.len());
        let mut cur = tp_start(ctx);
        while !holes.is_empty() {
            let k = holes.iter().enumerate().min_by(|a, b| a.1.dist(cur).total_cmp(&b.1.dist(cur))).map_or(0, |x| x.0);
            cur = holes.remove(k);
            ordered.push(cur);
        }
        holes = ordered;
    }
    let mut tp = Toolpath::new();
    let f = &tool.feeds;
    let retract = ctx.heights.retract;
    let tip_extra = match tool.kind {
        crate::tool::ToolKind::Drill { tip_angle_deg } | crate::tool::ToolKind::SpotDrill { tip_angle_deg } => {
            tool.radius() / (tip_angle_deg * 0.5).to_radians().tan()
        }
        _ => 0.0,
    };
    let peck = if peck > 0.0 { peck } else { tool.diameter };
    for h in &holes {
        let top = Vec3::new(h.x, h.y, ctx.heights.top.max(h.z.min(ctx.heights.top)));
        // The hole top is the point's own height when faces were picked; the bottom is the
        // bottom height plus break-through and the drill tip.
        let bottom_z = ctx.heights.bottom - break_through - if break_through > 0.0 { tip_extra } else { 0.0 };
        let depth = top.z - bottom_z;
        if depth <= 0.0 {
            continue;
        }
        tp.rapid(Vec3::new(h.x, h.y, tp.last_position().map_or(ctx.heights.clearance, |p| p.z.max(retract))))?;
        tp.rapid(Vec3::new(h.x, h.y, retract))?;
        tp.rapid(Vec3::new(h.x, h.y, ctx.heights.feed.min(retract).max(top.z)))?;
        let feed = if cycle == DrillCycleKind::Tap { tap_feed } else { f.plunge };
        tp.cycles.push(DrillCycle { kind: cycle, top, axis: Vec3::new(0.0, 0.0, 1.0), depth, retract_height: retract, peck, dwell_s, feed });
        match cycle {
            DrillCycleKind::Simple | DrillCycleKind::Dwell | DrillCycleKind::FeedOut | DrillCycleKind::Tap => {
                tp.push(Move::new(MoveKind::Plunge, Vec3::new(h.x, h.y, bottom_z), feed))?;
                if cycle == DrillCycleKind::FeedOut || cycle == DrillCycleKind::Tap {
                    tp.push(Move::new(MoveKind::Retract, Vec3::new(h.x, h.y, retract), feed))?;
                } else {
                    tp.rapid(Vec3::new(h.x, h.y, retract))?;
                }
            }
            DrillCycleKind::Peck | DrillCycleKind::ChipBreak => {
                let mut z = top.z;
                let mut guard = 0;
                while z > bottom_z + 1e-9 && guard < 100_000 {
                    guard += 1;
                    let next = (z - peck).max(bottom_z);
                    tp.push(Move::new(MoveKind::Plunge, Vec3::new(h.x, h.y, next), f.plunge))?;
                    if next > bottom_z + 1e-9 {
                        if cycle == DrillCycleKind::Peck {
                            tp.rapid(Vec3::new(h.x, h.y, retract))?;
                            tp.rapid(Vec3::new(h.x, h.y, next + 0.5))?;
                        } else {
                            tp.rapid(Vec3::new(h.x, h.y, next + 0.5))?;
                        }
                    }
                    z = next;
                }
                tp.rapid(Vec3::new(h.x, h.y, retract))?;
            }
        }
    }
    if let Some(p) = tp.last_position() {
        tp.rapid(Vec3::new(p.x, p.y, ctx.heights.clearance))?;
    }
    notice.value("holes", holes.len());
    notice.value("cycle", format!("{cycle:?}").to_lowercase());
    if cycle == DrillCycleKind::Tap {
        notice.value("tap_feed_mm_min", r3(tap_feed));
        notice.issue(Issue::info("tapping", "the control must tap rigidly (G84) or hold the tap in a tension-compression holder; without canned cycles the post expands the cycle as a feed in and a feed out at the pitch feed"));
    }
    Ok((tp, notice))
}

/// A simple offset of an open polyline: each point moved along the average normal of its
/// neighbouring segments by `d` (left of the direction of travel for positive `d`).
fn offset_open(pts: &[Vec2], d: f64) -> Vec<Vec2> {
    let n = pts.len();
    if n < 2 || d.abs() < 1e-12 {
        return pts.to_vec();
    }
    let seg_normal = |i: usize| -> Vec2 {
        let a = pts[i];
        let b = pts[i + 1];
        let t = (b - a).normalized().unwrap_or(Vec2::new(1.0, 0.0));
        Vec2::new(-t.y, t.x)
    };
    (0..n)
        .map(|i| {
            let nrm = if i == 0 {
                seg_normal(0)
            } else if i == n - 1 {
                seg_normal(n - 2)
            } else {
                (seg_normal(i - 1) + seg_normal(i)).normalized().unwrap_or(seg_normal(i))
            };
            pts[i] + nrm * d
        })
        .collect()
}

/// The curves of the geometry selection, as passes at height `z`: loops closed, chains open.
fn curve_passes(ctx: &Context, z: f64, direction: Direction) -> Vec<Pass> {
    let mut passes = Vec::new();
    for ring in polyline::normalize_set(&ctx.geometry.loops) {
        let oriented = orient_for(&ring, direction, false);
        passes.push(Pass { points: oriented.iter().map(|p| Vec3::new(p.x, p.y, z)).collect(), closed: true });
    }
    for chain in &ctx.geometry.open {
        passes.push(Pass { points: chain.iter().map(|p| Vec3::new(p.x, p.y, z)).collect(), closed: false });
    }
    passes
}

/// The tool centre along the selected curves to `bottom` in depth passes (tracing, engraving).
fn follow_curves(ctx: &Context, lk: &Linker, bottom: f64, stepdown: f64, direction: Direction) -> Result<(Toolpath, Notice)> {
    if ctx.geometry.loops.is_empty() && ctx.geometry.open.is_empty() {
        return Err(CamError::Geometry("tracing needs curves: a sketch, loops or faces whose edges to follow".into()));
    }
    let stepdown = if stepdown > 0.0 { stepdown } else { ctx.tool.diameter.max(0.1) };
    let zs = levels(ctx.heights.top, bottom, stepdown)?;
    let mut tp = Toolpath::new();
    let mut from_z = ctx.heights.feed;
    let mut count = 0usize;
    for z in &zs {
        let passes = curve_passes(ctx, *z, direction);
        count += passes.len();
        lk.run(&mut tp, &passes, None, from_z)?;
        from_z = *z;
    }
    lk.retract_clearance(&mut tp)?;
    let mut notice = Notice::default();
    notice.value("levels", zs.len());
    notice.value("passes", count);
    notice.value("depth_mm", r3(ctx.heights.top - bottom));
    Ok((tp, notice))
}

fn slot(op: &Operation, ctx: &Context, width: f64, stepdown: f64, direction: Direction) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    if ctx.geometry.loops.is_empty() && ctx.geometry.open.is_empty() {
        return Err(CamError::Geometry("a slot needs its centre curves: a sketch, loops or faces whose edges to follow".into()));
    }
    let mut notice = Notice::default();
    let d = tool.diameter;
    let width = if width > 0.0 { width } else { d };
    if width < d - 1e-9 {
        notice.issue(
            Issue::error("tool_too_large", format!("a {:.2} mm tool cannot cut a {:.2} mm slot", d, width))
                .parameter("tool", tool.id)
                .limit(r3(width)),
        );
        return Ok((Toolpath::new(), notice));
    }
    let stepdown = if stepdown > 0.0 { stepdown } else { d * 0.5 };
    // Passes across the width: the centre, then pairs stepping out to the walls.
    let extra = (width - d) * 0.5 - ctx.stock_to_leave_radial;
    let mut offsets: Vec<f64> = vec![0.0];
    if extra > ctx.tolerance {
        let n = (extra / (d * 0.5)).ceil().max(1.0) as usize;
        for k in 1..=n {
            let o = extra * k as f64 / n as f64;
            offsets.push(o);
            offsets.push(-o);
        }
    }
    let bottom = ctx.heights.bottom + ctx.stock_to_leave_axial;
    let zs = levels(ctx.heights.top, bottom, stepdown)?;
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    let mut from_z = ctx.heights.feed;
    let mut count = 0usize;
    for z in &zs {
        let mut passes: Vec<Pass> = Vec::new();
        for o in &offsets {
            for ring in polyline::normalize_set(&ctx.geometry.loops) {
                let rings = if o.abs() < 1e-12 { vec![ring.clone()] } else { offset::offset_set(std::slice::from_ref(&ring), *o, ctx.tolerance)? };
                for r in rings {
                    let oriented = orient_for(&r, direction, false);
                    passes.push(Pass { points: oriented.iter().map(|p| Vec3::new(p.x, p.y, *z)).collect(), closed: true });
                }
            }
            for chain in &ctx.geometry.open {
                let dense = polyline::densify(chain, (ctx.tolerance * 20.0).max(0.2));
                let pts = offset_open(&dense, *o);
                let pts = polyline::simplify_open(&pts, ctx.tolerance * 0.5);
                passes.push(Pass { points: pts.iter().map(|p| Vec3::new(p.x, p.y, *z)).collect(), closed: false });
            }
        }
        count += passes.len();
        let mut linking = op.linking;
        linking.stay_down_distance = linking.stay_down_distance.max(width);
        let lk2 = Linker { linking, ..lk };
        lk2.run(&mut tp, &passes, None, from_z)?;
        from_z = *z;
    }
    lk.retract_clearance(&mut tp)?;
    notice.value("levels", zs.len());
    notice.value("passes", count);
    notice.value("width_mm", r3(width));
    Ok((tp, notice))
}

fn engrave(op: &Operation, ctx: &Context, width: f64, stepdown: f64) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let half = match tool.kind {
        crate::tool::ToolKind::Chamfer { tip_angle_deg } | crate::tool::ToolKind::SpotDrill { tip_angle_deg } => {
            Some((tip_angle_deg.clamp(1.0, 179.0) * 0.5).to_radians())
        }
        _ => None,
    };
    let mut lk = linker(op, ctx);
    let mut bottom = ctx.heights.bottom + ctx.stock_to_leave_axial;
    let mut pre = Notice::default();
    // The engraved surface: the model's top when it lies below the top height (stock above
    // the part is faced off first).
    let surface = ctx.heights.top.min(ctx.model_z().1);
    match half {
        Some(h) if width > 0.0 => {
            // The groove's width at the surface sets the tip depth.
            let depth = (width * 0.5) / h.tan();
            bottom = surface - depth;
            lk.heights.bottom = bottom;
            lk.heights.top = surface;
            pre.value("depth_mm", r3(depth));
        }
        Some(_) => {}
        None => {
            pre.issue(
                Issue::warning("v_tool", "engraving expects a V (chamfer or engraving) tool; the tool's tip width is cut instead")
                    .parameter("tool", tool.id),
            );
        }
    }
    let (tp, mut notice) = follow_curves(ctx, &lk, bottom, if stepdown > 0.0 { stepdown } else { ctx.heights.top - bottom }, Direction::Climb)?;
    notice.merge(pre);
    notice.value("width_mm", r3(width));
    Ok((tp, notice))
}

fn circular(op: &Operation, ctx: &Context, stepover: f64, stepdown: f64, direction: Direction) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let holes = &ctx.geometry.holes;
    let d = ctx.geometry.hole_diameter;
    if holes.is_empty() || d <= 0.0 {
        return Err(CamError::Geometry("circular milling needs round holes with a diameter".into()));
    }
    let r_wall = d * 0.5 - tool.radius() - ctx.stock_to_leave_radial;
    let mut notice = Notice::default();
    if r_wall <= 0.0 {
        notice.issue(
            Issue::error("tool_too_large", format!("a {:.2} mm tool cannot mill a {:.2} mm hole", tool.diameter, d))
                .parameter("tool", tool.id)
                .limit(r3(d)),
        );
        return Ok((Toolpath::new(), notice));
    }
    let stepover = if stepover > 0.0 { stepover } else { default_stepover(tool, false) };
    let stepdown = if stepdown > 0.0 { stepdown } else { tool.diameter * 0.5 };
    // Radii from the wall inward, then cut inside out.
    let mut radii: Vec<f64> = Vec::new();
    let mut r = r_wall;
    while r > ctx.tolerance {
        radii.push(r);
        r -= stepover;
    }
    radii.reverse();
    let bottom = ctx.heights.bottom + ctx.stock_to_leave_axial;
    let zs = levels(ctx.heights.top, bottom, stepdown)?;
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    let mut rings = 0usize;
    for h in holes {
        let c = Vec2::new(h.x, h.y);
        let region = vec![polyline::circle(c, r_wall, ctx.tolerance)];
        let mut from_z = ctx.heights.feed;
        for z in &zs {
            let mut passes: Vec<Pass> = Vec::new();
            for r in &radii {
                let ring = polyline::circle(c, *r, ctx.tolerance);
                let oriented = orient_for(&ring, direction, true);
                passes.push(Pass { points: oriented.iter().map(|p| Vec3::new(p.x, p.y, *z)).collect(), closed: true });
                rings += 1;
            }
            let mut linking = op.linking;
            linking.stay_down_distance = linking.stay_down_distance.max(stepover * 3.0);
            let lk2 = Linker { linking, ..lk };
            lk2.run(&mut tp, &passes, Some(&region), from_z)?;
            from_z = *z;
        }
        lk.retract(&mut tp)?;
    }
    lk.retract_clearance(&mut tp)?;
    notice.value("holes", holes.len());
    notice.value("levels", zs.len());
    notice.value("rings", rings);
    Ok((tp, notice))
}

fn thread(op: &Operation, ctx: &Context, pitch: f64, passes: u32, external: bool) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    crate::positive("pitch", pitch)?;
    let holes = &ctx.geometry.holes;
    let d = ctx.geometry.hole_diameter;
    if holes.is_empty() || d <= 0.0 {
        return Err(CamError::Geometry("thread milling needs round holes (or bosses) with their major diameter".into()));
    }
    let mut notice = Notice::default();
    // Thread depth for a 60° form.
    let depth_of_thread = 0.6134 * pitch;
    let r_full = if external { d * 0.5 + tool.radius() } else { d * 0.5 - tool.radius() };
    if !external && r_full <= 0.0 {
        notice.issue(
            Issue::error("tool_too_large", format!("a {:.2} mm thread mill cannot enter a {:.2} mm thread", tool.diameter, d))
                .parameter("tool", tool.id)
                .limit(r3(d)),
        );
        return Ok((Toolpath::new(), notice));
    }
    let n_pass = passes.clamp(1, 20);
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    let f = &tool.feeds;
    let bottom = ctx.heights.bottom + ctx.stock_to_leave_axial;
    let top = ctx.heights.top;
    let depth = top - bottom;
    if depth <= 1e-9 {
        return Err(CamError::Param("the thread's top must be above its bottom".into()));
    }
    let turns = depth / pitch;
    let sign = if external { -1.0 } else { 1.0 };
    for h in holes {
        let c = Vec2::new(h.x, h.y);
        for k in 1..=n_pass {
            // Radial passes: the first takes most of the thread depth, the last the full form.
            let bite = depth_of_thread * (1.0 - k as f64 / n_pass as f64);
            let r = r_full - sign * bite;
            let seg = polyline::arc_segments(r.abs().max(0.1), std::f64::consts::TAU, ctx.tolerance).max(12);
            let n = ((turns * seg as f64).ceil() as usize).clamp(1, 1_000_000);
            let start = c + Vec2::new(r, 0.0);
            lk.retract(&mut tp)?;
            // Approach: above the centre (internal) or the start (external), down to the bottom,
            // then a lead to the thread radius.
            let entry = if external { start + Vec2::new(tool.diameter, 0.0) } else { c };
            tp.rapid(Vec3::new(entry.x, entry.y, tp.last_position().map_or(ctx.heights.clearance, |p| p.z.max(ctx.heights.retract))))?;
            tp.rapid(Vec3::new(entry.x, entry.y, ctx.heights.feed.max(top)))?;
            tp.push(Move::new(MoveKind::Plunge, Vec3::new(entry.x, entry.y, bottom), f.plunge))?;
            tp.push(Move::new(MoveKind::Lead, Vec3::new(start.x, start.y, bottom), f.lead_in))?;
            // Climb up the thread: counter-clockwise for a right-hand internal thread.
            for i in 1..=n {
                let t = i as f64 / n as f64;
                let a = t * turns * std::f64::consts::TAU;
                let p = c + Vec2::from_angle(a) * r;
                tp.cut(Vec3::new(p.x, p.y, bottom + depth * t), f.cutting)?;
            }
            let end = c + Vec2::from_angle(turns * std::f64::consts::TAU) * r;
            let exit = if external { end + (end - c).normalized().unwrap_or(Vec2::new(1.0, 0.0)) * tool.diameter } else { c };
            tp.push(Move::new(MoveKind::Lead, Vec3::new(exit.x, exit.y, top), f.lead_out))?;
            tp.rapid(Vec3::new(exit.x, exit.y, ctx.heights.retract))?;
        }
    }
    lk.retract_clearance(&mut tp)?;
    notice.value("holes", holes.len());
    notice.value("turns", r3(turns));
    notice.value("thread_depth_mm", r3(depth_of_thread));
    notice.value("passes", n_pass);
    notice.issue(Issue::info(
        "single_point",
        "a single-point thread mill helix: a multi-tooth thread mill cuts the whole depth in one turn; set the bottom height to the thread's depth",
    ));
    Ok((tp, notice))
}

fn tp_start(ctx: &Context) -> Vec3 {
    Vec3::new(ctx.model_bounds.min.x, ctx.model_bounds.min.y, ctx.heights.clearance)
}

fn bore(op: &Operation, ctx: &Context, pitch: f64, direction: Direction) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let holes = &ctx.geometry.holes;
    let d = ctx.geometry.hole_diameter;
    if holes.is_empty() || d <= 0.0 {
        return Err(CamError::Geometry("boring needs round holes with a diameter".into()));
    }
    let r = d * 0.5 - tool.radius() - ctx.stock_to_leave_radial;
    let mut notice = Notice::default();
    if r <= 0.0 {
        notice.issue(
            Issue::error("tool_too_large", format!("a {:.2} mm tool cannot bore a {:.2} mm hole", tool.diameter, d))
                .parameter("tool", tool.id)
                .limit(r3(d)),
        );
        return Ok((Toolpath::new(), notice));
    }
    let pitch = if pitch > 0.0 { pitch } else { tool.diameter * 0.1 };
    let lk = linker(op, ctx);
    let mut tp = Toolpath::new();
    let f = &tool.feeds;
    let bottom = ctx.heights.bottom + ctx.stock_to_leave_axial;
    let ccw = direction != Direction::Conventional;
    for h in holes {
        let c = Vec2::new(h.x, h.y);
        let top = ctx.heights.top;
        let depth = top - bottom;
        let turns = (depth / pitch).max(1e-6);
        let k = polyline::arc_segments(r, std::f64::consts::TAU, ctx.tolerance).max(12);
        let n = ((turns * k as f64).ceil() as usize).clamp(1, 1_000_000);
        let start = c + Vec2::new(r, 0.0);
        lk.retract(&mut tp)?;
        tp.rapid(Vec3::new(start.x, start.y, tp.last_position().map_or(ctx.heights.clearance, |p| p.z.max(ctx.heights.retract))))?;
        tp.rapid(Vec3::new(start.x, start.y, ctx.heights.feed))?;
        tp.push(Move::new(MoveKind::Ramp, Vec3::new(start.x, start.y, top), f.ramp))?;
        for i in 1..=n {
            let t = i as f64 / n as f64;
            let a = t * turns * std::f64::consts::TAU * if ccw { 1.0 } else { -1.0 };
            let p = c + Vec2::from_angle(a) * r;
            tp.push(Move::new(MoveKind::Ramp, Vec3::new(p.x, p.y, top - depth * t), f.ramp))?;
        }
        let a0 = turns * std::f64::consts::TAU * if ccw { 1.0 } else { -1.0 };
        for i in 1..=k {
            let a = a0 + i as f64 / k as f64 * std::f64::consts::TAU * if ccw { 1.0 } else { -1.0 };
            let p = c + Vec2::from_angle(a) * r;
            tp.cut(Vec3::new(p.x, p.y, bottom), f.cutting)?;
        }
        // Lead off the wall to the centre before retracting.
        tp.push(Move::new(MoveKind::Lead, Vec3::new(c.x, c.y, bottom), f.lead_out))?;
        tp.rapid(Vec3::new(c.x, c.y, ctx.heights.retract))?;
    }
    lk.retract_clearance(&mut tp)?;
    notice.value("holes", holes.len());
    notice.value("helix_radius_mm", r3(r));
    Ok((tp, notice))
}
