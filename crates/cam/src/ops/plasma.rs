//! Plasma cutting: the torch follows the curves above the sheet, it never goes into the material.
//!
//! Each closed loop (and each open chain) is one cut. The torch comes down at rapid to its
//! pierce height, the post fires it and waits the pierce delay, it feeds down to its cut height
//! and cuts the whole loop there, then rapids up to the travel height, where the post stops it.
//! Holes are cut before the outlines round them, so a part is never cut free before its holes
//! are. Closed cuts no wider than `small_hole_mm` are cut at a share of the feed.

use solidcore_record::{Vec2, Vec3};
use solidcraft_geom::prelude::*;

use super::{Context, OpKind, Side};
use crate::notice::{Issue, Notice, r3};
use crate::offset;
use crate::polyline::{self, Poly};
use crate::setup::Operation;
use crate::tool::ToolKind;
use crate::toolpath::{Move, MoveKind, Toolpath};
use crate::{CamError, Result};

pub fn generate(op: &Operation, ctx: &Context) -> Result<(Toolpath, Notice)> {
    let ToolKind::Plasma { pierce_height, cut_height, pierce_delay_s } = ctx.tool.kind else {
        return Err(CamError::Param(format!("plasma cutting needs a plasma torch (kind: plasma); T{} is not one", ctx.tool.number)));
    };
    let OpKind::PlasmaCut { feed, side, kerf, small_hole_mm, small_hole_feed } = op.kind else {
        return Err(CamError::Param("not a plasma operation".into()));
    };
    crate::positive("feed", feed)?;
    if !(small_hole_mm.is_finite() && (0.0..=10_000.0).contains(&small_hole_mm)) {
        return Err(CamError::Param("`small_hole_mm` must be 0 to 10000 mm".into()));
    }
    if !(small_hole_feed.is_finite() && small_hole_feed > 0.0 && small_hole_feed <= 1.0) {
        return Err(CamError::Param("`small_hole_feed` is a share of the feed, above 0 and up to 1".into()));
    }
    if ctx.geometry.loops.is_empty() && ctx.geometry.open.is_empty() {
        return Err(CamError::Geometry("plasma cutting needs curves: a sketch, loops or faces whose edges to cut".into()));
    }
    let kerf = if kerf > 0.0 { kerf } else { ctx.tool.diameter };
    let loops = polyline::normalize_set(&ctx.geometry.loops);
    let rings: Vec<Poly> = match side {
        Side::On => loops,
        Side::Outside => offset::offset_set(&loops, kerf * 0.5, ctx.tolerance)?,
        Side::Inside => offset::offset_set(&loops, -kerf * 0.5, ctx.tolerance)?,
    };
    // Holes (clockwise once normalized) before outlines; among each, the smaller first.
    let mut rings: Vec<(bool, f64, Poly)> = rings
        .into_iter()
        .filter(|r| r.len() >= 2)
        .map(|r| {
            let a = polyline::signed_area(&r);
            (a >= 0.0, a.abs(), r)
        })
        .collect();
    rings.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));

    let top = ctx.heights.top;
    let pierce_z = top + pierce_height;
    let cut_z = top + cut_height;
    let travel = ctx.heights.retract.max(pierce_z);
    let clearance = ctx.heights.clearance.max(travel);
    let plunge = if ctx.tool.feeds.plunge > 0.0 { ctx.tool.feeds.plunge } else { feed };

    let mut tp = Toolpath::new();
    tp.plasma = true;
    tp.pierce_s = pierce_delay_s;
    let mut first = true;
    let (mut cuts, mut slow) = (0usize, 0usize);
    let mut cut_one = |tp: &mut Toolpath, pts: &[Vec2], closed: bool, f: f64| -> Result<()> {
        let Some(&start) = pts.first() else { return Ok(()) };
        let at = |p: Vec2, z: f64| Vec3::new(p.x, p.y, z);
        tp.push(Move::new(MoveKind::Rapid, at(start, if first { clearance } else { travel }), 0.0))?;
        first = false;
        tp.push(Move::new(MoveKind::Rapid, at(start, pierce_z), 0.0))?;
        if (pierce_z - cut_z).abs() > 1e-9 {
            tp.push(Move::new(MoveKind::Plunge, at(start, cut_z), plunge))?;
        }
        let mut last = start;
        for p in pts.iter().skip(1) {
            tp.push(Move::new(MoveKind::Cut, at(*p, cut_z), f))?;
            last = *p;
        }
        if closed && last.dist(start) > 1e-9 {
            tp.push(Move::new(MoveKind::Cut, at(start, cut_z), f))?;
            last = start;
        }
        tp.push(Move::new(MoveKind::Rapid, at(last, travel), 0.0))?;
        Ok(())
    };
    for (outer, _, ring) in &rings {
        let (lo, hi) = bounds(ring);
        let width = (hi.x - lo.x).max(hi.y - lo.y);
        let small = !outer && small_hole_mm > 0.0 && width <= small_hole_mm;
        if small {
            slow += 1;
        }
        cut_one(&mut tp, ring, true, if small { feed * small_hole_feed } else { feed })?;
        cuts += 1;
    }
    for chain in &ctx.geometry.open {
        cut_one(&mut tp, chain, false, feed)?;
        cuts += 1;
    }
    if let Some(last) = tp.moves.last().map(|m| m.to) {
        tp.push(Move::new(MoveKind::Rapid, Vec3::new(last.x, last.y, clearance), 0.0))?;
    }
    let mut notice = Notice::default();
    notice.value("cuts", cuts);
    notice.value("pierces", cuts);
    notice.value("feed_mm_min", r3(feed));
    notice.value("kerf_mm", r3(kerf));
    notice.value("pierce_height_mm", r3(pierce_height));
    notice.value("cut_height_mm", r3(cut_height));
    notice.value("pierce_delay_s", r3(pierce_delay_s));
    if slow > 0 {
        notice.value("small_holes_slowed", slow);
    }
    notice.issue(Issue::info(
        "torch_height",
        "the torch cuts at a fixed height above the sheet's top (no torch height control); set Z0 on the sheet",
    ));
    Ok((tp, notice))
}

fn bounds(p: &[Vec2]) -> (Vec2, Vec2) {
    let mut lo = p.first().copied().unwrap_or(Vec2::new(0.0, 0.0));
    let mut hi = lo;
    for q in p {
        lo = Vec2::new(lo.x.min(q.x), lo.y.min(q.y));
        hi = Vec2::new(hi.x.max(q.x), hi.y.max(q.y));
    }
    (lo, hi)
}
