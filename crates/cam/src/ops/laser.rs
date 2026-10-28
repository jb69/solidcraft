//! Laser operations for the diode modules of desktop mills (Makera's 2.5 W and 5 W modules):
//! cutting along curves and raster engraving of regions. The beam is a tool of the spot's
//! diameter; moves carry the beam's power (0..1), which the post writes as the control's `S`
//! word, and the toolpath is flagged `laser` so the stock simulation leaves it alone and the
//! post enters the control's laser mode.

use solidcraft_geom::{Vec2, Vec3};

use super::two_d::raster_passes;
use super::{Context, OpKind, Side};
use crate::notice::{Issue, Notice, r3};
use crate::offset;
use crate::polyline::{self, Poly};
use crate::setup::Operation;
use crate::tool::ToolKind;
use crate::toolpath::{Move, MoveKind, Toolpath};
use crate::{CamError, Result};

pub fn generate(op: &Operation, ctx: &Context) -> Result<(Toolpath, Notice)> {
    if !matches!(ctx.tool.kind, ToolKind::Laser { .. }) {
        return Err(CamError::Param(format!("laser operations need a laser tool (kind: laser); T{} is a cutter", ctx.tool.number)));
    }
    match &op.kind {
        OpKind::LaserCut { power, speed, passes, side, kerf, air } => cut(ctx, *power, *speed, *passes, *side, *kerf, *air),
        OpKind::LaserEngrave { power, speed, line_interval, angle_deg, bidirectional, outline, air } => {
            engrave(ctx, *power, *speed, *line_interval, *angle_deg, *bidirectional, *outline, *air)
        }
        _ => Err(CamError::Param("not a laser operation".into())),
    }
}

/// The beam's focus height: the stock top (the control's laser mode puts its focus at Z0 of
/// the work; the post writes no Z in laser mode).
fn focus_z(ctx: &Context) -> f64 {
    ctx.heights.top
}

fn power_of(p: f64) -> Result<f64> {
    if !(p.is_finite() && p > 0.0 && p <= 1.0) {
        return Err(CamError::Param("laser `power` is a fraction of the module's power, above 0 and up to 1".into()));
    }
    Ok(p)
}

fn travel(tp: &mut Toolpath, to: Vec2, z: f64) -> Result<()> {
    tp.push(Move::new(MoveKind::Rapid, Vec3::new(to.x, to.y, z), 0.0))
}

fn burn(tp: &mut Toolpath, to: Vec2, z: f64, speed: f64, power: f64) -> Result<()> {
    tp.push(Move::new(MoveKind::Cut, Vec3::new(to.x, to.y, z), speed).with_power(power))
}

/// Cutting: the beam along the curves, the kerf offset to the chosen side, `passes` times.
fn cut(ctx: &Context, power: f64, speed: f64, passes: u32, side: Side, kerf: f64, air: bool) -> Result<(Toolpath, Notice)> {
    let power = power_of(power)?;
    crate::positive("speed", speed)?;
    if ctx.geometry.loops.is_empty() && ctx.geometry.open.is_empty() {
        return Err(CamError::Geometry("laser cutting needs curves: a sketch, loops or faces whose edges to cut".into()));
    }
    let kerf = if kerf > 0.0 { kerf } else { ctx.tool.diameter };
    let loops = polyline::normalize_set(&ctx.geometry.loops);
    let cl: Vec<Poly> = match side {
        Side::On => loops.clone(),
        Side::Outside => offset::offset_set(&loops, kerf * 0.5, ctx.tolerance)?,
        Side::Inside => offset::offset_set(&loops, -kerf * 0.5, ctx.tolerance)?,
    };
    let z = focus_z(ctx);
    let mut tp = Toolpath::new();
    tp.laser = true;
    tp.air = air;
    let passes = passes.clamp(1, 100);
    let mut count = 0usize;
    for _ in 0..passes {
        for ring in &cl {
            let Some(&first) = ring.first() else { continue };
            travel(&mut tp, first, z)?;
            for p in ring.iter().skip(1) {
                burn(&mut tp, *p, z, speed, power)?;
            }
            burn(&mut tp, first, z, speed, power)?;
            count += 1;
        }
        for chain in &ctx.geometry.open {
            let Some(&first) = chain.first() else { continue };
            travel(&mut tp, first, z)?;
            for p in chain.iter().skip(1) {
                burn(&mut tp, *p, z, speed, power)?;
            }
            count += 1;
        }
    }
    let mut notice = Notice::default();
    notice.value("curves", count);
    notice.value("passes", passes);
    notice.value("power", r3(power));
    notice.value("speed_mm_min", r3(speed));
    notice.value("kerf_mm", r3(kerf));
    if passes > 1 {
        notice.issue(Issue::info(
            "multi_pass",
            format!("{passes} passes over the same curves: for a through cut, match the passes to the material's thickness"),
        ));
    }
    Ok((tp, notice))
}

/// Engraving: the regions inside the loops filled with raster lines at `line_interval`, the
/// beam on along each line and off between them; then the outline, when asked.
#[allow(clippy::too_many_arguments)]
fn engrave(
    ctx: &Context,
    power: f64,
    speed: f64,
    line_interval: f64,
    angle_deg: f64,
    bidirectional: bool,
    outline: bool,
    air: bool,
) -> Result<(Toolpath, Notice)> {
    let power = power_of(power)?;
    crate::positive("speed", speed)?;
    if ctx.geometry.loops.is_empty() {
        return Err(CamError::Geometry("laser engraving needs closed loops to fill: a sketch, faces or loops".into()));
    }
    let interval = if line_interval > 0.0 { line_interval } else { ctx.tool.diameter.max(0.05) };
    let region = polyline::normalize_set(&ctx.geometry.loops);
    let rows = raster_passes(&region, interval, angle_deg.to_radians(), bidirectional, ctx.tolerance)?;
    let z = focus_z(ctx);
    let mut tp = Toolpath::new();
    tp.laser = true;
    tp.air = air;
    let mut lines = 0usize;
    let mut length = 0.0;
    for row in &rows {
        let Some(&first) = row.first() else { continue };
        travel(&mut tp, first, z)?;
        for p in row.iter().skip(1) {
            burn(&mut tp, *p, z, speed, power)?;
        }
        if let (Some(a), Some(b)) = (row.first(), row.last()) {
            length += a.dist(*b);
        }
        lines += 1;
    }
    if outline {
        for ring in &region {
            let Some(&first) = ring.first() else { continue };
            travel(&mut tp, first, z)?;
            for p in ring.iter().skip(1) {
                burn(&mut tp, *p, z, speed, power)?;
            }
            burn(&mut tp, first, z, speed, power)?;
        }
    }
    let mut notice = Notice::default();
    notice.value("lines", lines);
    notice.value("line_interval_mm", r3(interval));
    notice.value("area_mm2", r3(polyline::set_area(&region)));
    notice.value("burn_length_mm", r3(length));
    notice.value("power", r3(power));
    notice.value("speed_mm_min", r3(speed));
    notice.issue(Issue::info("vector_fill", "the fill is the regions inside the selected loops; image engraving (grey levels) is not built"));
    Ok((tp, notice))
}
