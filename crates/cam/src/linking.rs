//! Heights, leads, ramps and the joining of passes into one toolpath: where the tool retracts
//! to, how it enters the material (plunge, ramp, helix) and leaves it (lead-out), and when it
//! may stay down between passes instead of retracting.

use serde::{Deserialize, Serialize};
use solidcraft_geom::{Vec2, Vec3};

use crate::polyline::{self, Poly};
use crate::tool::Tool;
use crate::toolpath::{Move, MoveKind, Toolpath};
use crate::{CamError, Result};

/// What a height is measured from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HeightRef {
    #[default]
    StockTop,
    StockBottom,
    ModelTop,
    ModelBottom,
    /// The WCS origin plane (z = 0).
    Origin,
    /// An absolute z in the WCS.
    Absolute,
    /// The top of the selected geometry (the top height) — only for the bottom height.
    TopHeight,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Height {
    pub from: HeightRef,
    pub offset: f64,
}

impl Height {
    pub const fn new(from: HeightRef, offset: f64) -> Height {
        Height { from, offset }
    }
}

/// The five heights of an operation, in the operation's frame (tool axis z).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Heights {
    /// Rapids between operations and over clamps travel here.
    pub clearance: Height,
    /// Retracts between passes travel here.
    pub retract: Height,
    /// Rapids descend to here, then feed.
    pub feed: Height,
    /// Where cutting starts.
    pub top: Height,
    /// Where cutting ends.
    pub bottom: Height,
}

impl Default for Heights {
    fn default() -> Self {
        Heights {
            clearance: Height::new(HeightRef::StockTop, 10.0),
            retract: Height::new(HeightRef::StockTop, 5.0),
            feed: Height::new(HeightRef::StockTop, 2.0),
            top: Height::new(HeightRef::StockTop, 0.0),
            bottom: Height::new(HeightRef::ModelBottom, 0.0),
        }
    }
}

/// Heights resolved to numbers.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolvedHeights {
    pub clearance: f64,
    pub retract: f64,
    pub feed: f64,
    pub top: f64,
    pub bottom: f64,
}

impl Heights {
    /// Resolves against the stock and model extents along the tool axis.
    pub fn resolve(&self, stock_z: (f64, f64), model_z: (f64, f64)) -> Result<ResolvedHeights> {
        let one = |h: &Height, top: Option<f64>| -> Result<f64> {
            let base = match h.from {
                HeightRef::StockTop => stock_z.1,
                HeightRef::StockBottom => stock_z.0,
                HeightRef::ModelTop => model_z.1,
                HeightRef::ModelBottom => model_z.0,
                HeightRef::Origin => 0.0,
                HeightRef::Absolute => 0.0,
                HeightRef::TopHeight => top.ok_or_else(|| CamError::Param("only the bottom height may refer to the top height".into()))?,
            };
            crate::finite("height offset", h.offset)?;
            Ok(base + h.offset)
        };
        let top = one(&self.top, None)?;
        let r = ResolvedHeights {
            clearance: one(&self.clearance, None)?,
            retract: one(&self.retract, None)?,
            feed: one(&self.feed, None)?,
            top,
            bottom: one(&self.bottom, Some(top))?,
        };
        if r.bottom > r.top {
            return Err(CamError::Param(format!("bottom height {:.3} is above top height {:.3}", r.bottom, r.top)));
        }
        if r.retract < r.top {
            return Err(CamError::Param(format!("retract height {:.3} is below top height {:.3}", r.retract, r.top)));
        }
        if r.clearance < r.retract {
            return Err(CamError::Param(format!("clearance height {:.3} is below retract height {:.3}", r.clearance, r.retract)));
        }
        if r.feed < r.top {
            return Err(CamError::Param(format!("feed height {:.3} is below top height {:.3}", r.feed, r.top)));
        }
        Ok(r)
    }
}

/// How the tool enters the material.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Ramp {
    /// Straight down at the plunge feed.
    Plunge,
    /// Helix of `diameter` (tool-centre circle) descending at `angle_deg`.
    Helix { diameter: f64, angle_deg: f64 },
    /// Along the first pass, back and forth, descending at `angle_deg`.
    Zigzag { angle_deg: f64 },
    /// Along the pass's own contour, descending at `angle_deg`.
    Profile { angle_deg: f64 },
}

impl Default for Ramp {
    fn default() -> Self {
        Ramp::Helix { diameter: 0.0, angle_deg: 2.0 }
    }
}

/// Lead-in and lead-out: a tangent arc of `radius` sweeping `sweep_deg`, then (for the lead-in,
/// before) a straight `distance`. A vertical lead adds a quarter arc down to the cut.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lead {
    pub radius: f64,
    pub sweep_deg: f64,
    pub distance: f64,
    pub vertical_radius: f64,
}

impl Default for Lead {
    fn default() -> Self {
        Lead { radius: 0.0, sweep_deg: 90.0, distance: 0.0, vertical_radius: 0.0 }
    }
}

/// Linking settings shared by operations.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Linking {
    pub ramp: Ramp,
    pub lead_in: Lead,
    pub lead_out: Lead,
    /// Farthest two passes may be for the tool to move between them at the cutting level
    /// (0: always retract).
    pub stay_down_distance: f64,
    /// Height above the current level for a stay-down link that leaves the material.
    pub lift_height: f64,
    /// Retract only to the retract height between passes (false: to the clearance height).
    pub minimum_retract: bool,
}

impl Default for Linking {
    fn default() -> Self {
        Linking {
            ramp: Ramp::default(),
            lead_in: Lead::default(),
            lead_out: Lead::default(),
            stay_down_distance: 0.0,
            lift_height: 0.0,
            minimum_retract: true,
        }
    }
}

/// A pass: a polyline at one level, closed or open, with the region the tool may travel in
/// while staying down (the cutter-location region of this level), when known.
#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    pub points: Vec<Vec3>,
    pub closed: bool,
}

/// Builds the toolpath of a list of passes at the same level, linking consecutive passes with
/// stay-down moves when they are close and inside `region` (XY cutter-location region), else
/// with a retract, a rapid and an entry.
pub struct Linker<'a> {
    pub tool: &'a Tool,
    pub heights: ResolvedHeights,
    pub linking: Linking,
    pub tolerance: f64,
}

impl Linker<'_> {
    /// Appends a retract from the current position to the retract (or clearance) height.
    pub fn retract(&self, tp: &mut Toolpath) -> Result<()> {
        let Some(cur) = tp.last_position() else { return Ok(()) };
        let h = if self.linking.minimum_retract { self.heights.retract } else { self.heights.clearance };
        if cur.z < h {
            let feed = self.tool.feeds.retract;
            let kind = if feed > 0.0 { MoveKind::Retract } else { MoveKind::Rapid };
            tp.push(Move::new(kind, Vec3::new(cur.x, cur.y, h), feed))?;
        }
        Ok(())
    }

    /// Appends a retract to the clearance height.
    pub fn retract_clearance(&self, tp: &mut Toolpath) -> Result<()> {
        let Some(cur) = tp.last_position() else { return Ok(()) };
        if cur.z < self.heights.clearance {
            tp.push(Move::new(MoveKind::Rapid, Vec3::new(cur.x, cur.y, self.heights.clearance), 0.0))?;
        }
        Ok(())
    }

    /// Rapid over to `xy` at the current (retract) height, then down to the feed height.
    fn approach(&self, tp: &mut Toolpath, xy: Vec2, target_z: f64) -> Result<()> {
        let h = tp.last_position().map_or(self.heights.clearance, |p| p.z.max(self.heights.retract));
        tp.rapid(Vec3::new(xy.x, xy.y, h))?;
        let feed_z = self.heights.feed.max(target_z);
        if feed_z < h {
            tp.rapid(Vec3::new(xy.x, xy.y, feed_z))?;
        }
        Ok(())
    }

    /// Enters the material at the start of `pass` (whose points are at the cutting level `z`),
    /// from `from_z` (the previous level's floor or the feed height), by the configured ramp.
    /// `region` is where the tool centre may be at this level.
    fn enter(&self, tp: &mut Toolpath, pass: &Pass, from_z: f64, region: Option<&[Poly]>) -> Result<()> {
        let Some(&start) = pass.points.first() else { return Ok(()) };
        let z = start.z;
        let depth = (from_z - z).max(0.0);
        let f = &self.tool.feeds;
        let start_xy = Vec2::new(start.x, start.y);
        // Lead-in before the pass: an arc tangent to the first segment, offset to the left.
        let lead = self.linking.lead_in;
        let lead_pts = lead_arc(pass, lead, true, self.tolerance);
        let entry_xy = lead_pts.first().map_or(start_xy, |p| Vec2::new(p.x, p.y));
        let entry_ok = region.is_none_or(|r| polyline::set_contains(r, entry_xy));
        let entry_xy = if entry_ok { entry_xy } else { start_xy };
        if depth <= 1e-9 {
            // Already at the level: move straight in.
            tp.push(Move::new(MoveKind::Link, Vec3::new(entry_xy.x, entry_xy.y, z), f.cutting))?;
        } else {
            match self.linking.ramp {
                Ramp::Plunge => {
                    self.approach(tp, entry_xy, from_z)?;
                    tp.push(Move::new(MoveKind::Plunge, Vec3::new(entry_xy.x, entry_xy.y, z), f.plunge))?;
                }
                Ramp::Helix { angle_deg, .. } if region.is_none() => {
                    // No region to keep a helix inside: ramp along the pass itself.
                    let angle = angle_deg.clamp(0.1, 45.0).to_radians();
                    let along = ramp_track(pass, false, depth / angle.tan(), self.tolerance);
                    let Some(first) = along.first() else { return Ok(()) };
                    let first_xy = Vec2::new(first.x, first.y);
                    self.approach(tp, first_xy, from_z)?;
                    tp.push(Move::new(MoveKind::Ramp, Vec3::new(first.x, first.y, from_z), f.ramp))?;
                    let total: f64 = along.windows(2).map(|w| Vec2::new(w[0].x, w[0].y).dist(Vec2::new(w[1].x, w[1].y))).sum();
                    let mut run = 0.0;
                    for w in along.windows(2) {
                        run += Vec2::new(w[0].x, w[0].y).dist(Vec2::new(w[1].x, w[1].y));
                        let zz = from_z - depth * (run / total.max(1e-9)).min(1.0);
                        tp.push(Move::new(MoveKind::Ramp, Vec3::new(w[1].x, w[1].y, zz), f.ramp))?;
                    }
                    tp.push(Move::new(MoveKind::Link, Vec3::new(entry_xy.x, entry_xy.y, z), f.cutting))?;
                }
                Ramp::Helix { diameter, angle_deg } => {
                    // Helix about a centre inside the region, of the given diameter (default: a
                    // size that fits the tool's flat tip, so the centre is cut too).
                    let r_tool = self.tool.radius();
                    let mut d = if diameter > 0.0 { diameter } else { (self.tool.flat_radius() * 1.5).max(r_tool * 0.5) };
                    // A helix narrower than a quarter of the tool is no better than a plunge
                    // and takes thousands of moves to descend: plunge instead.
                    let least = (r_tool * 0.5).max(self.tolerance * 4.0);
                    if let Some(r) = region {
                        // Shrink until the helix circle stays inside the region.
                        let mut tries = 0;
                        while tries < 8 && d >= least && !circle_inside(r, start_xy, d * 0.5, self.tolerance) {
                            d *= 0.5;
                            tries += 1;
                        }
                        if !circle_inside(r, start_xy, d * 0.5, self.tolerance) {
                            d = 0.0;
                        }
                    }
                    if d < least {
                        self.approach(tp, entry_xy, from_z)?;
                        tp.push(Move::new(MoveKind::Plunge, Vec3::new(entry_xy.x, entry_xy.y, z), f.plunge))?;
                    } else {
                        let radius = d * 0.5;
                        let angle = angle_deg.clamp(0.1, 45.0).to_radians();
                        let centre = start_xy - Vec2::new(radius, 0.0);
                        let drop_per_turn = std::f64::consts::TAU * radius * angle.tan();
                        let turns = (depth / drop_per_turn).max(1e-6);
                        let seg_per_turn = polyline::arc_segments(radius, std::f64::consts::TAU, self.tolerance).max(12);
                        let n = ((turns * seg_per_turn as f64).ceil() as usize).clamp(1, 200_000);
                        let first = centre + Vec2::new(radius, 0.0);
                        self.approach(tp, first, from_z)?;
                        tp.push(Move::new(MoveKind::Ramp, Vec3::new(first.x, first.y, from_z), f.ramp))?;
                        for k in 1..=n {
                            let t = k as f64 / n as f64;
                            let a = t * turns * std::f64::consts::TAU;
                            let p = centre + Vec2::from_angle(a) * radius;
                            tp.push(Move::new(MoveKind::Ramp, Vec3::new(p.x, p.y, from_z - depth * t), f.ramp))?;
                        }
                        // Finish the circle flat so the floor of the helix is cut.
                        let flat = seg_per_turn;
                        let a0 = turns * std::f64::consts::TAU;
                        for k in 1..=flat {
                            let a = a0 + k as f64 / flat as f64 * std::f64::consts::TAU;
                            let p = centre + Vec2::from_angle(a) * radius;
                            tp.push(Move::new(MoveKind::Ramp, Vec3::new(p.x, p.y, z), f.ramp))?;
                        }
                        tp.push(Move::new(MoveKind::Link, Vec3::new(entry_xy.x, entry_xy.y, z), f.cutting))?;
                    }
                }
                Ramp::Zigzag { angle_deg } | Ramp::Profile { angle_deg } => {
                    let angle = angle_deg.clamp(0.1, 45.0).to_radians();
                    let along = ramp_track(pass, matches!(self.linking.ramp, Ramp::Zigzag { .. }), depth / angle.tan(), self.tolerance);
                    let Some(first) = along.first() else { return Ok(()) };
                    let first_xy = Vec2::new(first.x, first.y);
                    self.approach(tp, first_xy, from_z)?;
                    tp.push(Move::new(MoveKind::Ramp, Vec3::new(first.x, first.y, from_z), f.ramp))?;
                    let total: f64 = along.windows(2).map(|w| Vec2::new(w[0].x, w[0].y).dist(Vec2::new(w[1].x, w[1].y))).sum();
                    let mut run = 0.0;
                    for w in along.windows(2) {
                        run += Vec2::new(w[0].x, w[0].y).dist(Vec2::new(w[1].x, w[1].y));
                        let zz = from_z - depth * (run / total.max(1e-9)).min(1.0);
                        tp.push(Move::new(MoveKind::Ramp, Vec3::new(w[1].x, w[1].y, zz), f.ramp))?;
                    }
                    tp.push(Move::new(MoveKind::Link, Vec3::new(entry_xy.x, entry_xy.y, z), f.cutting))?;
                }
            }
        }
        if entry_ok {
            for p in &lead_pts {
                tp.push(Move::new(MoveKind::Lead, Vec3::new(p.x, p.y, z), f.lead_in))?;
            }
        }
        Ok(())
    }

    /// Cuts along the pass (the tool is at its first point).
    fn cut_pass(&self, tp: &mut Toolpath, pass: &Pass) -> Result<()> {
        let f = self.tool.feeds.cutting;
        for p in pass.points.iter().skip(1) {
            tp.cut(*p, f)?;
        }
        if pass.closed
            && let Some(first) = pass.points.first()
        {
            tp.cut(*first, f)?;
        }
        Ok(())
    }

    fn leave(&self, tp: &mut Toolpath, pass: &Pass, region: Option<&[Poly]>) -> Result<()> {
        let pts = lead_arc(pass, self.linking.lead_out, false, self.tolerance);
        if let Some(last) = pts.last()
            && region.is_none_or(|r| polyline::set_contains(r, Vec2::new(last.x, last.y)))
        {
            let z = pass.points.first().map_or(0.0, |p| p.z);
            for p in &pts {
                tp.push(Move::new(MoveKind::Lead, Vec3::new(p.x, p.y, z), self.tool.feeds.lead_out))?;
            }
        }
        Ok(())
    }

    /// Links and cuts `passes` in order. Passes at one level that lie within the stay-down
    /// distance and whose connecting segment stays inside `region` are joined without a
    /// retract. `from_z` is where entries begin (the previous level, or the feed height).
    pub fn run(&self, tp: &mut Toolpath, passes: &[Pass], region: Option<&[Poly]>, from_z: f64) -> Result<()> {
        for pass in passes {
            if pass.points.len() < 2 && !(pass.points.len() == 1 && !pass.closed) {
                continue;
            }
            let Some(&start) = pass.points.first() else { continue };
            let stay = match tp.last_position() {
                Some(cur) if (cur.z - start.z).abs() < 1e-9 && self.linking.stay_down_distance > 0.0 => {
                    let a = Vec2::new(cur.x, cur.y);
                    let b = Vec2::new(start.x, start.y);
                    a.dist(b) <= self.linking.stay_down_distance && region.is_some_and(|r| polyline::segment_inside(r, a, b, self.tolerance))
                }
                _ => false,
            };
            if stay {
                tp.push(Move::new(MoveKind::Link, start, self.tool.feeds.cutting))?;
            } else {
                self.retract(tp)?;
                self.enter(tp, pass, from_z, region)?;
            }
            self.cut_pass(tp, pass)?;
            self.leave(tp, pass, region)?;
        }
        Ok(())
    }
}

/// Whether a circle lies inside the region.
fn circle_inside(region: &[Poly], c: Vec2, r: f64, tol: f64) -> bool {
    if !polyline::set_contains(region, c) {
        return false;
    }
    polyline::distance_to_set(region, c) >= r - tol
}

/// The points of a lead arc: tangent to the pass's first (lead-in) or last (lead-out) segment,
/// curving to the left of the direction of travel (into the already-cut side for climb milling
/// on an outer contour running counter-clockwise around the material… the operation orients
/// the pass so the lead lands on the open side), ending at the pass start (lead-in) or starting
/// at its end (lead-out). Empty when the lead has no radius and no distance.
pub fn lead_arc(pass: &Pass, lead: Lead, lead_in: bool, tol: f64) -> Vec<Vec3> {
    if lead.radius <= 0.0 && lead.distance <= 0.0 {
        return Vec::new();
    }
    let n = pass.points.len();
    if n < 2 {
        return Vec::new();
    }
    let (p, q) = if lead_in { (pass.points[0], pass.points[1]) } else { (pass.points[n - 1], pass.points[n - 2]) };
    let z = p.z;
    let p2 = Vec2::new(p.x, p.y);
    let q2 = Vec2::new(q.x, q.y);
    // Direction of travel at the point: into the pass (lead-in) or out of it (lead-out).
    let Some(dir) = (if lead_in { q2 - p2 } else { p2 - q2 }).normalized() else { return Vec::new() };
    let left = Vec2::new(-dir.y, dir.x);
    let mut pts: Vec<Vec2> = Vec::new();
    let sweep = lead.sweep_deg.clamp(0.0, 180.0).to_radians();
    if lead.radius > 0.0 && sweep > 0.0 {
        let c = p2 + left * lead.radius;
        let k = polyline::arc_segments(lead.radius, sweep, tol).max(2);
        // Angle of p about c is -left; the arc goes back (lead-in) along -dir.
        let a0 = (p2 - c).angle();
        for i in 0..=k {
            let t = i as f64 / k as f64;
            // Lead-in: from the arc's far end to p. Lead-out: from p outward.
            let a = if lead_in { a0 - sweep * (1.0 - t) } else { a0 - sweep * t };
            pts.push(c + Vec2::from_angle(a) * lead.radius);
        }
    } else {
        pts.push(p2);
    }
    if lead.distance > 0.0 {
        // Straight part tangent at the arc's outer end.
        if lead_in {
            let first = pts[0];
            let tangent = if lead.radius > 0.0 && sweep > 0.0 {
                let a = (first - (p2 + left * lead.radius)).angle();
                Vec2::from_angle(a + std::f64::consts::FRAC_PI_2)
            } else {
                dir
            };
            pts.insert(0, first - tangent * lead.distance);
        } else {
            let last = *pts.last().unwrap_or(&p2);
            let tangent = if lead.radius > 0.0 && sweep > 0.0 {
                let a = (last - (p2 + left * lead.radius)).angle();
                Vec2::from_angle(a + std::f64::consts::FRAC_PI_2)
            } else {
                dir
            };
            pts.push(last + tangent * lead.distance);
        }
    }
    if lead_in {
        pts.pop();
    } else {
        pts.remove(0);
    }
    pts.into_iter().map(|v| Vec3::new(v.x, v.y, z)).collect()
}

/// The XY track of a ramp of length `length` along the pass: back and forth over its start
/// (zigzag) or around it (profile).
fn ramp_track(pass: &Pass, zigzag: bool, length: f64, tol: f64) -> Vec<Vec3> {
    let pts: Vec<Vec2> = pass.points.iter().map(|p| Vec2::new(p.x, p.y)).collect();
    let n = pts.len();
    if n < 2 {
        return pass.points.clone();
    }
    let mut out = vec![pts[0]];
    let mut remaining = length.max(tol);
    let mut i = 0usize;
    let mut forward = true;
    let mut guard = 0usize;
    while remaining > 0.0 && guard < 1_000_000 {
        guard += 1;
        let j = if pass.closed && !zigzag {
            (i + 1) % n
        } else if forward {
            if i + 1 >= n {
                forward = false;
                continue;
            }
            i + 1
        } else {
            if i == 0 {
                forward = true;
                continue;
            }
            i - 1
        };
        let seg = pts[j] - pts[i];
        let len = seg.len();
        if len <= 1e-12 {
            i = j;
            continue;
        }
        if len >= remaining {
            out.push(pts[i] + seg * (remaining / len));
            remaining = 0.0;
        } else {
            out.push(pts[j]);
            remaining -= len;
            i = j;
        }
    }
    // Return to the pass start so the cut begins there.
    let z = pass.points[0].z;
    let mut res: Vec<Vec3> = out.iter().map(|p| Vec3::new(p.x, p.y, z)).collect();
    // Reverse: the ramp descends while travelling, ending at the start of the pass.
    res.reverse();
    res
}
