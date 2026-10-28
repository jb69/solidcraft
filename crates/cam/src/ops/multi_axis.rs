//! Continuous 4- and 5-axis operations: rotary (the tool points at the operation frame's x
//! axis and sweeps the surface around it), swarf (the side of the tool along a wall between a
//! bottom and a top chain), multi-axis contour (surface-normal passes with lead and tilt) and
//! flow (passes morphed between two rails, surface-normal).

use solidcraft_geom::{Vec2, Vec3};

use super::two_d::raster_passes;
use super::{Context, Direction, OpKind, default_stepover, sample_segment, simplify3};
use crate::frame::{Frame, angle_between, rotate};
use crate::notice::{Issue, Notice, r3};
use crate::polyline;
use crate::setup::Operation;
use crate::toolpath::{Move, MoveKind, Toolpath};
use crate::{CamError, Result};

pub fn generate(op: &Operation, ctx: &Context) -> Result<(Toolpath, Notice)> {
    match &op.kind {
        OpKind::Rotary { along_axis, stepover, angle_from_deg, angle_to_deg, inner_radius } => {
            rotary(op, ctx, *along_axis, *stepover, (*angle_from_deg, *angle_to_deg), *inner_radius)
        }
        OpKind::Swarf { left, passes, extension, max_tilt_deg } => swarf(op, ctx, *left, *passes, *extension, *max_tilt_deg),
        OpKind::MultiAxisContour { stepover, angle_deg, lead_deg, tilt_deg, max_tilt_deg, direction } => {
            multi_axis_contour(op, ctx, *stepover, *angle_deg, (*lead_deg, *tilt_deg, *max_tilt_deg), *direction)
        }
        OpKind::Flow { stepover, lead_deg, tilt_deg, max_tilt_deg } => flow(op, ctx, *stepover, (*lead_deg, *tilt_deg, *max_tilt_deg)),
        _ => Err(CamError::Param("not a multi-axis operation".into())),
    }
}

/// The radial frame at `theta` about the operation's x axis: z points away from the axis.
pub(super) fn radial_frame(theta: f64) -> Frame {
    let x = Vec3::new(1.0, 0.0, 0.0);
    let z = rotate(Vec3::new(0.0, 0.0, 1.0), x, theta);
    Frame { origin: Vec3::new(0.0, 0.0, 0.0), x, y: z.cross(x), z }
}

/// Turning the model's profile: rotary finishing along the axis, the full way round.
pub(super) fn rotary_along(op: &Operation, ctx: &Context, stepover: f64) -> Result<(Toolpath, Notice)> {
    rotary(op, ctx, true, stepover, (0.0, 360.0), 0.0)
}

fn rotary(_op: &Operation, ctx: &Context, along_axis: bool, stepover: f64, angles: (f64, f64), inner_radius: f64) -> Result<(Toolpath, Notice)> {
    let Some(mesh) = ctx.model else {
        return Err(CamError::Geometry("rotary machining needs the model's bodies".into()));
    };
    let tool = ctx.tool;
    let vtool = ctx.virtual_tool();
    let (a0, a1) = (angles.0.to_radians(), angles.1.to_radians());
    if !(a1 > a0) {
        return Err(CamError::Param("angle_to must be greater than angle_from".into()));
    }
    // Extent along the axis: the selected loops' x range, else the model's.
    let (x0, x1) = if ctx.geometry.loops.is_empty() {
        (ctx.model_bounds.min.x, ctx.model_bounds.max.x)
    } else {
        let b = polyline::bounds_of(&ctx.geometry.loops);
        (b.min.x, b.max.x)
    };
    // Stock radius about the axis, clearance beyond it.
    let stock_r = ctx.stock.radius_about_x();
    let clearance_r = stock_r + (ctx.heights.clearance - ctx.stock_z().1).max(2.0);
    let floor = inner_radius.max(0.0);
    let step = (8.0 * ctx.tolerance * tool.radius()).sqrt().clamp(ctx.tolerance * 2.0, tool.diameter);
    let f = tool.feeds;
    let mut tp = Toolpath::new();
    let mut notice = Notice::default();
    let mut passes = 0usize;
    // Where the tool meets nothing (past the part's end, or inside the inner radius) the pass
    // breaks rather than diving to the axis.
    let drop_at = |theta: f64, x: f64| -> Option<(Vec3, Vec3)> {
        let fr = radial_frame(theta);
        let r = mesh.drop(&vtool, &fr, Vec2::new(x, 0.0), floor)?.z + ctx.stock_to_leave_axial;
        Some((fr.to_world(Vec3::new(x, 0.0, r.max(floor))), fr.z))
    };
    let emit_pass = |tp: &mut Toolpath, pts: Vec<(Vec3, Vec3)>, first_pass: bool, prev_axis: Option<Vec3>| -> Result<()> {
        let Some(&(start, axis0)) = pts.first() else { return Ok(()) };
        // Approach: at the clearance radius along the start's radial direction.
        let radial = axis0;
        let approach = radial * clearance_r + Vec3::new(start.x, 0.0, 0.0);
        if first_pass || prev_axis.is_some_and(|a| angle_between(a, axis0) > 0.02) {
            if let Some(cur) = tp.last_position() {
                let cur_axis = tp.last().map_or(axis0, |m| m.axis);
                let out = cur_axis * clearance_r + Vec3::new(cur.x, 0.0, 0.0);
                tp.push(Move::new(MoveKind::Rapid, out, 0.0).with_axis(cur_axis))?;
            }
            tp.push(Move::new(MoveKind::Rapid, approach, 0.0).with_axis(axis0))?;
            tp.push(Move::new(MoveKind::Plunge, start, f.plunge).with_axis(axis0))?;
        } else {
            tp.push(Move::new(MoveKind::Link, start, f.cutting).with_axis(axis0))?;
        }
        for (p, a) in pts.iter().skip(1) {
            tp.push(Move::new(MoveKind::Cut, *p, f.cutting).with_axis(*a))?;
        }
        Ok(())
    };
    if along_axis {
        let d_theta = if stepover > 0.0 { stepover.to_radians() } else { (default_stepover(tool, true) / stock_r.max(1.0)).max(0.002) };
        let n = (((a1 - a0) / d_theta).ceil() as usize).clamp(1, 1_000_000);
        let xs = sample_segment(Vec2::new(x0, 0.0), Vec2::new(x1, 0.0), step);
        let mut prev_axis = None;
        for k in 0..=n {
            if k == n && (a1 - a0 - std::f64::consts::TAU).abs() < 1e-9 {
                break; // a full turn: the last pass repeats the first
            }
            let theta = a0 + (a1 - a0) * k as f64 / n as f64;
            let mut pts: Vec<(Vec3, Vec3)> = xs.iter().filter_map(|x| drop_at(theta, x.x)).collect();
            if k % 2 == 1 {
                pts.reverse();
            }
            if pts.len() < 2 {
                continue;
            }
            let simplified = simplify_pairs(&pts, ctx.tolerance);
            emit_pass(&mut tp, simplified, passes == 0, prev_axis)?;
            prev_axis = Some(radial_frame(theta).z);
            passes += 1;
        }
    } else {
        let dx = if stepover > 0.0 { stepover } else { default_stepover(tool, true) };
        let xs = sample_segment(Vec2::new(x0, 0.0), Vec2::new(x1, 0.0), dx);
        let d_theta = (step / stock_r.max(1.0)).max(0.002);
        let n = (((a1 - a0) / d_theta).ceil() as usize).clamp(8, 1_000_000);
        for (k, x) in xs.iter().enumerate() {
            let pts: Vec<(Vec3, Vec3)> = (0..=n).filter_map(|i| drop_at(a0 + (a1 - a0) * i as f64 / n as f64, x.x)).collect();
            if pts.len() < 2 {
                continue;
            }
            let simplified = simplify_pairs(&pts, ctx.tolerance);
            // Circles join at the same angle: the first point of each is at a0, so step along.
            emit_pass(&mut tp, simplified, passes == 0, Some(radial_frame(a0).z))?;
            let _ = k;
            passes += 1;
        }
    }
    if let Some(cur) = tp.last_position() {
        let axis = tp.last().map_or(Vec3::new(0.0, 0.0, 1.0), |m| m.axis);
        tp.push(Move::new(MoveKind::Rapid, axis * clearance_r + Vec3::new(cur.x, 0.0, 0.0), 0.0).with_axis(axis))?;
    }
    notice.value("passes", passes);
    notice.value("stock_radius_mm", r3(stock_r));
    notice.value("clearance_radius_mm", r3(clearance_r));
    notice.issue(Issue::info(
        "rotary_axis",
        "the rotary axis is the operation frame's x axis; set the operation orientation so it matches the machine's A axis",
    ));
    Ok((tp, notice))
}

/// Simplifies a (point, axis) polyline by the points, keeping each kept point's axis.
fn simplify_pairs(pts: &[(Vec3, Vec3)], tol: f64) -> Vec<(Vec3, Vec3)> {
    let points: Vec<Vec3> = pts.iter().map(|p| p.0).collect();
    let kept = simplify3(&points, tol);
    let mut out = Vec::with_capacity(kept.len());
    let mut j = 0usize;
    for k in kept {
        while j < pts.len() && pts[j].0.dist(k) > 1e-12 {
            j += 1;
        }
        if j < pts.len() {
            out.push(pts[j]);
        }
    }
    out
}

/// Resamples an open polyline to `n` points by arc length.
pub fn resample_open(p: &[Vec3], n: usize) -> Vec<Vec3> {
    if p.len() < 2 || n < 2 {
        return p.to_vec();
    }
    let total: f64 = p.windows(2).map(|w| w[0].dist(w[1])).sum();
    if total <= 0.0 {
        return vec![p[0]; n];
    }
    let mut out = Vec::with_capacity(n);
    let mut seg = 0usize;
    let mut seg_start = 0.0;
    let mut seg_len = p[0].dist(p[1]);
    for k in 0..n {
        let s = total * k as f64 / (n - 1) as f64;
        while seg + 2 < p.len() && s > seg_start + seg_len {
            seg += 1;
            seg_start += seg_len;
            seg_len = p[seg].dist(p[seg + 1]);
        }
        let t = if seg_len > 0.0 { ((s - seg_start) / seg_len).clamp(0.0, 1.0) } else { 0.0 };
        out.push(p[seg].lerp(p[seg + 1], t));
    }
    out
}

/// Smooths a sequence of unit axes with a three-point moving average, keeping the ends.
fn smooth_axes(axes: &mut [Vec3], rounds: usize) {
    for _ in 0..rounds {
        let copy = axes.to_vec();
        for i in 1..axes.len().saturating_sub(1) {
            if let Some(a) = (copy[i - 1] + copy[i] * 2.0 + copy[i + 1]).normalized() {
                axes[i] = a;
            }
        }
    }
}

/// Clamps `axis` to within `max_tilt` of z, rotating it towards z in its own plane. Returns
/// whether it was clamped.
fn clamp_tilt(axis: Vec3, max_tilt: f64) -> (Vec3, bool) {
    let z = Vec3::new(0.0, 0.0, 1.0);
    let tilt = angle_between(axis, z);
    if tilt <= max_tilt {
        return (axis, false);
    }
    let Some(about) = z.cross(axis).normalized() else { return (z, true) };
    (rotate(z, about, max_tilt), true)
}

fn swarf(_op: &Operation, ctx: &Context, left: bool, passes_n: u32, extension: f64, max_tilt_deg: f64) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let bottom = &ctx.geometry.chain_a;
    let top = &ctx.geometry.chain_b;
    if bottom.len() < 2 || top.len() < 2 {
        return Err(CamError::Geometry("swarf needs a bottom chain and a top chain of at least two points each".into()));
    }
    let step = (8.0 * ctx.tolerance * tool.radius()).sqrt().clamp(ctx.tolerance * 2.0, tool.diameter);
    let len: f64 = bottom.windows(2).map(|w| w[0].dist(w[1])).sum();
    let n = ((len / step).ceil() as usize).clamp(2, 200_000);
    let a = resample_open(bottom, n);
    let b = resample_open(top, n);
    let r = tool.radius() + ctx.stock_to_leave_radial;
    let max_tilt = if max_tilt_deg > 0.0 { max_tilt_deg.to_radians() } else { std::f64::consts::PI };
    let mut axes: Vec<Vec3> = (0..n).map(|i| (b[i] - a[i]).normalized().unwrap_or(Vec3::new(0.0, 0.0, 1.0))).collect();
    smooth_axes(&mut axes, 2);
    let mut clamped = 0usize;
    let mut tips: Vec<Vec3> = Vec::with_capacity(n);
    let mut heights: Vec<f64> = Vec::with_capacity(n);
    for i in 0..n {
        let prev = a[i.saturating_sub(1)];
        let next = a[(i + 1).min(n - 1)];
        let t = (next - prev).normalized().unwrap_or(Vec3::new(1.0, 0.0, 0.0));
        let (axis, c) = clamp_tilt(axes[i], max_tilt);
        if c {
            clamped += 1;
        }
        axes[i] = axis;
        // t × axis points to the right of travel (travel +x, axis +z gives -y).
        let side = t.cross(axis).normalized().unwrap_or(Vec3::new(0.0, -1.0, 0.0)) * if left { -1.0 } else { 1.0 };
        tips.push(a[i] + side * r - axis * extension);
        heights.push((b[i] - a[i]).dot(axis));
    }
    let wall_h = heights.iter().copied().fold(0.0, f64::max);
    let f = tool.feeds;
    let mut tp = Toolpath::new();
    let mut notice = Notice::default();
    if wall_h + extension > tool.flute_length + 1e-6 {
        notice.issue(
            Issue::warning(
                "tool_too_short",
                format!("the wall is {:.2} mm high but the flutes are {:.2} mm long", wall_h + extension, tool.flute_length),
            )
            .parameter("flute_length", tool.flute_length)
            .limit(r3(wall_h + extension)),
        );
    }
    let passes = passes_n.max(1);
    let clear = (ctx.heights.clearance - ctx.heights.top).max(5.0);
    let mut shank_hits = 0usize;
    for k in 0..passes {
        // Depth fraction: pass 1 at the top of the wall (most material left), last at the bottom.
        let frac = if passes == 1 { 0.0 } else { 1.0 - (k + 1) as f64 / passes as f64 };
        let pts: Vec<Vec3> = (0..n).map(|i| tips[i] + axes[i] * (heights[i] * frac)).collect();
        let (pts, axs): (Vec<Vec3>, Vec<Vec3>) =
            if k % 2 == 1 { (pts.into_iter().rev().collect(), axes.iter().rev().copied().collect()) } else { (pts, axes.clone()) };
        let pairs: Vec<(Vec3, Vec3)> = pts.iter().copied().zip(axs.iter().copied()).collect();
        let simplified = simplify_pairs(&pairs, ctx.tolerance);
        let Some(&(start, axis0)) = simplified.first() else { continue };
        if k == 0 {
            tp.push(Move::new(MoveKind::Rapid, start + axis0 * (wall_h + clear), 0.0).with_axis(axis0))?;
            tp.push(Move::new(MoveKind::Plunge, start, f.plunge).with_axis(axis0))?;
        } else {
            tp.push(Move::new(MoveKind::Link, start, f.cutting).with_axis(axis0))?;
        }
        for (p, ax) in simplified.iter().skip(1) {
            if let Some(mesh) = ctx.model
                && mesh.cylinder_hits(*p, *ax, tool.radius_at(tool.flute_length + 0.01), tool.flute_length, tool.overall_length, 0.0)
            {
                shank_hits += 1;
            }
            tp.push(Move::new(MoveKind::Cut, *p, f.cutting).with_axis(*ax))?;
        }
    }
    if let Some(last) = tp.last().copied() {
        tp.push(Move::new(MoveKind::Rapid, last.to + last.axis * (wall_h + clear), 0.0).with_axis(last.axis))?;
    }
    notice.value("points", n);
    notice.value("wall_height_mm", r3(wall_h));
    notice.value("passes", passes);
    if clamped > 0 {
        notice.issue(
            Issue::warning(
                "tilt_limit",
                format!("{clamped} points needed more tilt than the {:.0}° limit; the wall is undercut there", max_tilt.to_degrees()),
            )
            .parameter("max_tilt_deg", max_tilt_deg),
        );
    }
    if shank_hits > 0 {
        notice.issue(
            Issue::warning("shank_collision", format!("the tool shank meets the model at {shank_hits} points"))
                .suggest(serde_json::json!({"overall_length": r3(tool.overall_length + wall_h)})),
        );
    }
    Ok((tp, notice))
}

/// Tool axis for a contact normal `n` with the feed direction `feed`: the normal leaned forward
/// by `lead` (about the across-feed axis) and sideways by `tilt` (about the feed), clamped to
/// `max_tilt` from z.
fn lean_axis(n: Vec3, feed: Vec3, lead: f64, tilt: f64, max_tilt: f64) -> (Vec3, bool) {
    let mut axis = n;
    if let Some(across) = n.cross(feed).normalized() {
        axis = rotate(axis, across, lead);
    }
    if let Some(fd) = (feed - n * feed.dot(n)).normalized() {
        axis = rotate(axis, fd, tilt);
    }
    clamp_tilt(axis.normalized().unwrap_or(n), max_tilt)
}

/// Surface-normal passes: for each row of XY sample points, the vertical drop gives the
/// contact point and normal; the tool axis follows the normal with lead and tilt and the tip is
/// placed so the ball touches the contact point.
fn normal_passes(ctx: &Context, rows: &[Vec<Vec2>], lead_tilt: (f64, f64, f64), tp: &mut Toolpath, notice: &mut Notice) -> Result<usize> {
    let tool = ctx.tool;
    let vtool = ctx.virtual_tool();
    let (lead, tilt, max_tilt) =
        (lead_tilt.0.to_radians(), lead_tilt.1.to_radians(), if lead_tilt.2 > 0.0 { lead_tilt.2.to_radians() } else { std::f64::consts::FRAC_PI_2 });
    let rc = tool.corner_radius();
    if rc <= 0.0 {
        notice.issue(Issue::warning(
            "flat_tool_multi_axis",
            "surface-normal machining expects a ball or bull-nose tool; a flat tool's corner gouges when the axis tilts",
        ));
    }
    let step = (8.0 * ctx.tolerance * tool.radius().max(0.5)).sqrt().clamp(ctx.tolerance * 2.0, tool.diameter);
    let floor = ctx.heights.bottom.min(ctx.model_z().0) - 1.0;
    let f = tool.feeds;
    let mut clamped = 0usize;
    let mut count = 0usize;
    let clear = (ctx.heights.clearance - ctx.heights.top).max(5.0);
    for row in rows {
        let mut dense: Vec<Vec2> = Vec::new();
        for w in row.windows(2) {
            let s = sample_segment(w[0], w[1], step);
            if dense.is_empty() {
                dense.extend(s);
            } else {
                dense.extend(s.into_iter().skip(1));
            }
        }
        let mut pts: Vec<(Vec3, Vec3)> = Vec::new();
        for (i, p) in dense.iter().enumerate() {
            let Some(c) = ctx.drop(&vtool, *p, floor) else {
                if pts.len() >= 2 {
                    emit_normal_pass(tp, &pts, f.cutting, f.plunge, clear, ctx.tolerance)?;
                    count += 1;
                }
                pts.clear();
                continue;
            };
            let next = dense.get(i + 1).or_else(|| dense.get(i.saturating_sub(1))).copied().unwrap_or(*p);
            let feed_dir = Vec3::new(next.x - p.x, next.y - p.y, 0.0).normalized().unwrap_or(Vec3::new(1.0, 0.0, 0.0));
            let n = if c.normal.z < 0.0 { c.normal * -1.0 } else { c.normal };
            let (axis, cl) = lean_axis(n, feed_dir, lead, tilt, max_tilt);
            if cl {
                clamped += 1;
            }
            // Ball centre above the contact along the normal; tip back along the axis.
            let centre = c.point + n * rc;
            let tip = centre - axis * rc;
            pts.push((tip, axis));
        }
        if pts.len() >= 2 {
            emit_normal_pass(tp, &pts, f.cutting, f.plunge, clear, ctx.tolerance)?;
            count += 1;
        }
    }
    if let Some(last) = tp.last().copied() {
        tp.push(Move::new(MoveKind::Rapid, last.to + last.axis * clear, 0.0).with_axis(last.axis))?;
    }
    if clamped > 0 {
        notice.issue(Issue::warning("tilt_limit", format!("{clamped} points were held at the tilt limit")).parameter("max_tilt_deg", lead_tilt.2));
    }
    Ok(count)
}

fn emit_normal_pass(tp: &mut Toolpath, pts: &[(Vec3, Vec3)], cut: f64, plunge: f64, clear: f64, tol: f64) -> Result<()> {
    let mut axes: Vec<Vec3> = pts.iter().map(|p| p.1).collect();
    smooth_axes(&mut axes, 3);
    let pairs: Vec<(Vec3, Vec3)> = pts.iter().zip(axes).map(|(p, a)| (p.0, a)).collect();
    let simplified = simplify_pairs(&pairs, tol);
    let Some(&(start, axis0)) = simplified.first() else { return Ok(()) };
    if let Some(cur) = tp.last().copied() {
        tp.push(Move::new(MoveKind::Rapid, cur.to + cur.axis * clear, 0.0).with_axis(cur.axis))?;
    }
    tp.push(Move::new(MoveKind::Rapid, start + axis0 * clear, 0.0).with_axis(axis0))?;
    tp.push(Move::new(MoveKind::Plunge, start, plunge).with_axis(axis0))?;
    for (p, a) in simplified.iter().skip(1) {
        tp.push(Move::new(MoveKind::Cut, *p, cut).with_axis(*a))?;
    }
    Ok(())
}

fn multi_axis_contour(
    op: &Operation,
    ctx: &Context,
    stepover: f64,
    angle_deg: f64,
    lead_tilt: (f64, f64, f64),
    direction: Direction,
) -> Result<(Toolpath, Notice)> {
    if ctx.model.is_none_or(|m| m.is_empty()) {
        return Err(CamError::Geometry("multi-axis contour needs the model's bodies".into()));
    }
    let _ = op;
    let stepover = if stepover > 0.0 { stepover } else { default_stepover(ctx.tool, true) };
    let boundary = ctx.boundary()?;
    let rows = raster_passes(&boundary, stepover, angle_deg.to_radians(), direction == Direction::Both, ctx.tolerance)?;
    let mut tp = Toolpath::new();
    let mut notice = Notice::default();
    let n = normal_passes(ctx, &rows, lead_tilt, &mut tp, &mut notice)?;
    notice.value("passes", n);
    notice.value("lead_deg", lead_tilt.0);
    notice.value("tilt_deg", lead_tilt.1);
    Ok((tp, notice))
}

fn flow(op: &Operation, ctx: &Context, stepover: f64, lead_tilt: (f64, f64, f64)) -> Result<(Toolpath, Notice)> {
    let _ = op;
    let a = &ctx.geometry.chain_a;
    let b = &ctx.geometry.chain_b;
    if a.len() < 2 || b.len() < 2 {
        return Err(CamError::Geometry("flow needs two rails of at least two points each".into()));
    }
    let stepover = if stepover > 0.0 { stepover } else { default_stepover(ctx.tool, true) };
    let step = (8.0 * ctx.tolerance * ctx.tool.radius().max(0.5)).sqrt().clamp(ctx.tolerance * 2.0, ctx.tool.diameter);
    let len: f64 = a.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>().max(b.windows(2).map(|w| w[0].dist(w[1])).sum());
    let n = ((len / step).ceil() as usize).clamp(2, 200_000);
    let ra = resample_open(a, n);
    let rb = resample_open(b, n);
    let width = ra.iter().zip(&rb).map(|(p, q)| p.dist(*q)).fold(0.0, f64::max);
    let m = ((width / stepover).ceil() as usize).clamp(1, 100_000);
    let mut tp = Toolpath::new();
    let mut notice = Notice::default();
    if ctx.model.is_some_and(|mm| !mm.is_empty()) {
        // Passes morphed between the rails in XY, dropped to the surface with surface-normal axes.
        let rows: Vec<Vec<Vec2>> = (0..=m)
            .map(|k| {
                let t = k as f64 / m as f64;
                let mut row: Vec<Vec2> = (0..n).map(|i| ra[i].lerp(rb[i], t)).map(|p| Vec2::new(p.x, p.y)).collect();
                if k % 2 == 1 {
                    row.reverse();
                }
                row
            })
            .collect();
        let count = normal_passes(ctx, &rows, lead_tilt, &mut tp, &mut notice)?;
        notice.value("passes", count);
    } else {
        // No model: the ruled surface between the rails is the surface; its normal is the axis.
        let f = ctx.tool.feeds;
        let rc = ctx.tool.corner_radius();
        let clear = (ctx.heights.clearance - ctx.heights.top).max(5.0);
        for k in 0..=m {
            let t = k as f64 / m as f64;
            let mut pts: Vec<(Vec3, Vec3)> = Vec::with_capacity(n);
            for i in 0..n {
                let p = ra[i].lerp(rb[i], t);
                let du = ra[(i + 1).min(n - 1)].lerp(rb[(i + 1).min(n - 1)], t) - ra[i.saturating_sub(1)].lerp(rb[i.saturating_sub(1)], t);
                let dv = rb[i] - ra[i];
                let mut nrm = du.cross(dv).normalized().unwrap_or(Vec3::new(0.0, 0.0, 1.0));
                if nrm.z < 0.0 {
                    nrm = nrm * -1.0;
                }
                let (axis, _) = lean_axis(
                    nrm,
                    du.normalized().unwrap_or(Vec3::new(1.0, 0.0, 0.0)),
                    lead_tilt.0.to_radians(),
                    lead_tilt.1.to_radians(),
                    if lead_tilt.2 > 0.0 { lead_tilt.2.to_radians() } else { std::f64::consts::PI },
                );
                pts.push((p + nrm * rc - axis * rc, axis));
            }
            if k % 2 == 1 {
                pts.reverse();
            }
            emit_normal_pass(&mut tp, &pts, f.cutting, f.plunge, clear, ctx.tolerance)?;
        }
        if let Some(last) = tp.last().copied() {
            tp.push(Move::new(MoveKind::Rapid, last.to + last.axis * clear, 0.0).with_axis(last.axis))?;
        }
        notice.value("passes", m + 1);
        notice.issue(Issue::info("ruled_surface", "no model bodies in the setup: the passes follow the ruled surface between the rails"));
    }
    notice.value("width_mm", r3(width));
    Ok((tp, notice))
}
