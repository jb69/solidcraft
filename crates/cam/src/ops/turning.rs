//! Turning on a rotary axis: the part turns on the machine's A axis (the operation frame's x
//! axis, as for `rotary`) while a cutter held vertical cuts it, the way a lathe tool would.
//! Faces, grooves and threads come from the stock's radius and the parameters, not the model;
//! `turn_profile` is the rotary finishing of the model's shape.
//!
//! Points are in workpiece coordinates with the tool axis radial: the machine's inverse
//! kinematics turns a point at angle `theta` around the axis into an A angle and a vertical
//! tool. A thread is a helix: the part turns while the tool advances one pitch a turn.

use solidcraft_geom::Vec3;

use super::multi_axis::radial_frame;
use super::{Context, OpKind, levels};
use crate::notice::{Issue, Notice, r3};
use crate::polyline;
use crate::setup::Operation;
use crate::toolpath::{Move, MoveKind, Toolpath};
use crate::{CamError, Result};

pub fn generate(op: &Operation, ctx: &Context) -> Result<(Toolpath, Notice)> {
    match &op.kind {
        OpKind::TurnFace { stepdown, position } => face(ctx, *stepdown, *position),
        OpKind::TurnGroove { position, width, depth, stepdown, stepover } => groove(ctx, *position, *width, *depth, *stepdown, *stepover),
        OpKind::TurnThread { pitch, depth, passes, start, length, left_hand } => thread(ctx, *pitch, *depth, *passes, *start, *length, *left_hand),
        OpKind::TurnProfile { stepover } => super::multi_axis::rotary_along(op, ctx, *stepover),
        _ => Err(CamError::Param("not a turning operation".into())),
    }
}

/// The stock's radius about the axis and the clearance radius beyond it.
fn radii(ctx: &Context) -> (f64, f64) {
    let stock_r = ctx.stock.radius_about_x();
    (stock_r, stock_r + (ctx.heights.clearance - ctx.stock_z().1).max(2.0))
}

/// The part's extent along the axis (the selected loops' x range, else the model's, else the
/// stock's).
fn extent(ctx: &Context) -> (f64, f64) {
    if !ctx.geometry.loops.is_empty() {
        let b = polyline::bounds_of(&ctx.geometry.loops);
        (b.min.x, b.max.x)
    } else if !ctx.model_bounds.is_empty() {
        (ctx.model_bounds.min.x, ctx.model_bounds.max.x)
    } else {
        let sb = ctx.stock.bounds();
        (sb.min.x, sb.max.x)
    }
}

/// A point at `x` along the axis, radius `r`, angle `theta`, with the tool axis radial.
fn at(x: f64, r: f64, theta: f64) -> (Vec3, Vec3) {
    let fr = radial_frame(theta);
    (fr.to_world(Vec3::new(x, 0.0, r)), fr.z)
}

fn angular_step(r: f64, tol: f64) -> f64 {
    (polyline::arc_segments(r.max(0.5), std::f64::consts::TAU, tol).max(24) as f64).recip() * std::f64::consts::TAU
}

/// One full turn of the part at constant `x` and `r`, the tool cutting all the way round.
fn turn_circle(tp: &mut Toolpath, x: f64, r: f64, feed: f64, tol: f64) -> Result<()> {
    let d = angular_step(r, tol);
    let n = (std::f64::consts::TAU / d).ceil() as usize;
    for i in 1..=n {
        let theta = std::f64::consts::TAU * i as f64 / n as f64;
        let (p, a) = at(x, r, theta);
        tp.push(Move::new(MoveKind::Cut, p, feed).with_axis(a))?;
    }
    Ok(())
}

/// Rapid to the clearance radius at `x`, angle 0, then feed in to `r`.
fn approach(tp: &mut Toolpath, x: f64, r: f64, clearance_r: f64, plunge: f64) -> Result<()> {
    if let Some(cur) = tp.last_position() {
        let axis = tp.last().map_or(Vec3::new(0.0, 0.0, 1.0), |m| m.axis);
        let out = axis * clearance_r + Vec3::new(cur.x, 0.0, 0.0);
        tp.push(Move::new(MoveKind::Rapid, out, 0.0).with_axis(axis))?;
    }
    let (p, a) = at(x, clearance_r, 0.0);
    tp.push(Move::new(MoveKind::Rapid, p, 0.0).with_axis(a))?;
    let (p, a) = at(x, r, 0.0);
    tp.push(Move::new(MoveKind::Plunge, p, plunge).with_axis(a))?;
    Ok(())
}

fn retract(tp: &mut Toolpath, clearance_r: f64) -> Result<()> {
    if let Some(cur) = tp.last_position() {
        let axis = tp.last().map_or(Vec3::new(0.0, 0.0, 1.0), |m| m.axis);
        tp.push(Move::new(MoveKind::Rapid, axis * clearance_r + Vec3::new(cur.x, 0.0, 0.0), 0.0).with_axis(axis))?;
    }
    Ok(())
}

/// Facing the end of the part: the cutter stands beside the end face (its side cutting it)
/// at radii its flutes cover, the part turning; the face is cut back from the stock's end to
/// `position` in steps of `stepdown`.
fn face(ctx: &Context, stepdown: f64, position: f64) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    let (stock_r, clearance_r) = radii(ctx);
    let sb = ctx.stock.bounds();
    let (_, part_end) = extent(ctx);
    let target = if position != 0.0 { position } else { part_end };
    let stock_end = sb.max.x;
    if stock_end <= target + 1e-9 {
        return Err(CamError::Param(format!("nothing to face: the stock ends at x = {stock_end:.2}, the face is at x = {target:.2}")));
    }
    let stepdown = if stepdown > 0.0 { stepdown } else { tool.diameter * 0.5 };
    let xs = levels(stock_end, target, stepdown)?;
    // Radial positions of the tip so the flutes cover the whole face.
    let fl = tool.flute_length.max(tool.diameter);
    let mut tips: Vec<f64> = Vec::new();
    let mut r = (stock_r - fl).max(0.0);
    loop {
        tips.push(r);
        if r <= 1e-9 {
            break;
        }
        r = (r - fl).max(0.0);
    }
    let f = tool.feeds;
    let mut tp = Toolpath::new();
    let mut turns = 0usize;
    for x in &xs {
        // The tip beside the face: the tool's radius past it.
        let tool_x = x + tool.radius();
        for (i, r) in tips.iter().enumerate() {
            if i == 0 {
                approach(&mut tp, tool_x, *r, clearance_r, f.plunge)?;
            } else {
                let (p, a) = at(tool_x, *r, 0.0);
                tp.push(Move::new(MoveKind::Link, p, f.cutting).with_axis(a))?;
            }
            turn_circle(&mut tp, tool_x, *r, f.cutting, ctx.tolerance)?;
            turns += 1;
        }
    }
    retract(&mut tp, clearance_r)?;
    let mut notice = Notice::default();
    notice.value("levels", xs.len());
    notice.value("turns", turns);
    notice.value("face_x_mm", r3(target));
    notice.value("stock_radius_mm", r3(stock_r));
    notice.issue(Issue::info("rotary_axis", "the rotary axis is the operation frame's x axis; the part turns on A while the tool stands vertical"));
    Ok((tp, notice))
}

/// A groove around the part at `position`, `width` wide and `depth` deep from the stock's
/// radius: the tool steps in by `stepdown` and across by `stepover`, one turn per position.
fn groove(ctx: &Context, position: f64, width: f64, depth: f64, stepdown: f64, stepover: f64) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    crate::positive("depth", depth)?;
    let (stock_r, clearance_r) = radii(ctx);
    let width = if width > 0.0 { width } else { tool.diameter };
    if width < tool.diameter - 1e-9 {
        return Err(CamError::Param(format!("a {:.2} mm tool cannot cut a {:.2} mm groove", tool.diameter, width)));
    }
    let stepdown = if stepdown > 0.0 { stepdown } else { tool.diameter * 0.5 };
    let stepover = if stepover > 0.0 { stepover } else { tool.diameter * 0.5 };
    let r_bottom = (stock_r - depth).max(0.0);
    let rs = levels(stock_r, r_bottom, stepdown)?;
    // Tool positions across the width, centre first then out to the walls.
    let span = width - tool.diameter;
    let mut xs = vec![position];
    if span > ctx.tolerance {
        let n = (span * 0.5 / stepover).ceil().max(1.0) as usize;
        for k in 1..=n {
            let o = span * 0.5 * k as f64 / n as f64;
            xs.push(position + o);
            xs.push(position - o);
        }
    }
    let f = tool.feeds;
    let mut tp = Toolpath::new();
    let mut turns = 0usize;
    for (i, r) in rs.iter().enumerate() {
        for (j, x) in xs.iter().enumerate() {
            if i == 0 && j == 0 {
                approach(&mut tp, *x, *r, clearance_r, f.plunge)?;
            } else {
                let (p, a) = at(*x, *r, 0.0);
                tp.push(Move::new(MoveKind::Link, p, f.cutting).with_axis(a))?;
            }
            turn_circle(&mut tp, *x, *r, f.cutting, ctx.tolerance)?;
            turns += 1;
        }
    }
    retract(&mut tp, clearance_r)?;
    let mut notice = Notice::default();
    notice.value("turns", turns);
    notice.value("radius_mm", r3(r_bottom));
    notice.value("width_mm", r3(width));
    Ok((tp, notice))
}

/// A thread around the part: a helix of `pitch` per turn from `start` over `length`, cut in
/// `passes` to `depth` (0: a 60° form's depth) below the stock's radius. The tool's form (a
/// V bit for a V thread, an end mill for a square one, a ball for a round one) gives the
/// thread its profile.
fn thread(ctx: &Context, pitch: f64, depth: f64, passes: u32, start: f64, length: f64, left_hand: bool) -> Result<(Toolpath, Notice)> {
    let tool = ctx.tool;
    crate::positive("pitch", pitch)?;
    let (stock_r, clearance_r) = radii(ctx);
    let (x0, x1) = extent(ctx);
    let start = if start != 0.0 { start } else { x0 };
    let length = if length > 0.0 { length } else { x1 - start };
    if length <= 0.0 {
        return Err(CamError::Param("the thread needs a positive `length` along the axis".into()));
    }
    let depth = if depth > 0.0 { depth } else { 0.6134 * pitch };
    if depth >= stock_r {
        return Err(CamError::Param(format!("a {depth:.2} mm thread depth is deeper than the stock's {stock_r:.2} mm radius")));
    }
    let n_pass = passes.clamp(1, 50);
    let turns = length / pitch;
    let r_min = stock_r - depth;
    let d_theta = angular_step(stock_r, ctx.tolerance);
    let n = ((turns * std::f64::consts::TAU / d_theta).ceil() as usize).clamp(2, 2_000_000);
    let dir = if left_hand { -1.0 } else { 1.0 };
    let f = tool.feeds;
    let mut tp = Toolpath::new();
    for k in 1..=n_pass {
        // Depth of this pass: equal area per pass (deeper first, as a lathe would).
        let r = stock_r - depth * (k as f64 / n_pass as f64).sqrt();
        approach(&mut tp, start, r, clearance_r, f.plunge)?;
        for i in 1..=n {
            let t = i as f64 / n as f64;
            let theta = dir * turns * std::f64::consts::TAU * t;
            let (p, a) = at(start + length * t, r, theta);
            tp.push(Move::new(MoveKind::Cut, p, f.cutting).with_axis(a))?;
        }
        retract(&mut tp, clearance_r)?;
    }
    let mut notice = Notice::default();
    notice.value("turns", r3(turns));
    notice.value("passes", n_pass);
    notice.value("thread_depth_mm", r3(depth));
    notice.value("minor_radius_mm", r3(r_min));
    notice.value("major_diameter_mm", r3(stock_r * 2.0));
    if !matches!(tool.kind, crate::tool::ToolKind::Chamfer { .. }) {
        notice.issue(Issue::info(
            "thread_form",
            "the thread's form is the tool's: a 60° V bit cuts a V thread, an end mill a square one, a ball a round one",
        ));
    }
    Ok((tp, notice))
}
