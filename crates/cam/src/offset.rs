//! Offsetting of planar regions. A region is a set of closed polygons, outers counter-clockwise
//! and holes clockwise ([`polyline::normalize_set`]). Offsetting by `d > 0` grows the region,
//! by `d < 0` shrinks it (holes grow). Islands merge, narrow necks pinch off and thin slivers
//! disappear, as a cutter of radius `|d|` would see them.
//!
//! The algorithm: every edge is moved sideways by `d`; at a corner that opens, an arc about the
//! vertex joins the moved edges, and at a corner that closes they are joined directly. The
//! moved edges of every loop are then cut at all their mutual intersections, every piece whose
//! midpoint lies closer than `|d|` to the original boundary (or on the wrong side of it) is
//! dropped, and the remaining pieces are stitched back into closed loops. This is the classic
//! "offset, intersect, prune by distance" scheme, which handles self-intersections, merging and
//! splitting without a general polygon clipper.

use solidcraft_geom::Vec2;

use crate::polyline::{self, MinMaxPt, Poly, SegmentGrid, arc_segments};
use crate::{CamError, Result};

/// Most moved edges one offset may produce.
const MAX_SEGMENTS: usize = 2_000_000;

/// Offsets a normalized region by `d` with chord tolerance `tol`. The result is normalized.
pub fn offset_set(set: &[Poly], d: f64, tol: f64) -> Result<Vec<Poly>> {
    if !(d.is_finite() && tol.is_finite() && tol > 0.0) {
        return Err(CamError::Param("offset distance and tolerance must be finite, tolerance positive".into()));
    }
    // Shrinking a region never fills a hole, and growing it never loses an area: a loop the
    // stitching could not close (a jagged contour at a fine tolerance) would break that, and a
    // pocket would then cut straight through its island. Check, and try a coarser tolerance.
    let set = polyline::normalize_set(set);
    // (Only loops with nothing nested inside them: a probe there is in that loop's area alone.)
    let probes: Vec<(Vec2, bool)> = set
        .iter()
        .enumerate()
        .filter(|(i, p)| !set.iter().enumerate().any(|(k, o)| k != *i && o.first().is_some_and(|q| polyline::contains(p, *q))))
        .map(|(_, p)| p)
        .map(|p| (polyline::interior_probe(p), polyline::is_ccw(p)))
        .collect();
    let mut t = tol;
    for _ in 0..4 {
        let out = offset_set_once(&set, d, t)?;
        let holds = probes.iter().all(|(q, outer)| {
            if d < 0.0 && !outer {
                // A hole's inside (away from its edge by more than d) stays outside the region.
                !polyline::set_contains(&out, *q)
            } else if d > 0.0 && *outer {
                polyline::set_contains(&out, *q)
            } else {
                true
            }
        });
        if holds {
            return Ok(out);
        }
        t *= 4.0;
    }
    Err(CamError::Geometry("an offset lost a hole or an area of the region (the outline is too jagged to offset)".into()))
}

fn offset_set_once(set: &[Poly], d: f64, tol: f64) -> Result<Vec<Poly>> {
    // Points within the tolerance of their neighbours' chord add nothing the result can show,
    // and a jagged input (sampled contours) would leave arc fragments at every micro-corner.
    let set: Vec<Poly> = set.iter().map(|p| polyline::simplify_closed(&polyline::clean(p, tol * 0.01), tol * 0.5)).filter(|p| p.len() >= 3).collect();
    if set.is_empty() {
        return Ok(Vec::new());
    }
    if d.abs() < tol * 1e-3 {
        return Ok(polyline::normalize_set(&set));
    }
    // 1. Raw offset of every loop.
    let mut raw: Vec<(Vec2, Vec2)> = Vec::new();
    for p in &set {
        raw_offset(p, d, tol, &mut raw)?;
    }
    // 2. Split at intersections.
    let pieces = split_at_intersections(&raw, tol)?;
    // 3. Prune: keep pieces at distance |d| from the original boundary, on the right side.
    let grid = SegmentGrid::new(&set);
    // Arc chords sag up to `tol` inside the true offset, so the prune must allow that.
    let keep_tol = tol * 1.5 + d.abs() * 1e-6;
    let grow = d > 0.0;
    let kept: Vec<(Vec2, Vec2)> = pieces
        .into_iter()
        .filter(|(a, b)| {
            if a.dist(*b) < tol * 1e-3 {
                return false;
            }
            let m = a.lerp(*b, 0.5);
            if grid.distance(m) < d.abs() - keep_tol {
                return false;
            }
            // Side: a grown region's boundary lies outside the original, a shrunk one's inside.
            polyline::set_contains(&set, m) != grow
        })
        .collect();
    // 4. Stitch into loops.
    let loops = stitch(&kept, tol * 0.5);
    // A loop thinner than the tolerance (area under tolerance × perimeter) is a sliver.
    let loops: Vec<Poly> = loops
        .into_iter()
        .map(|p| polyline::clean(&p, tol * 0.01))
        .filter(|p| p.len() >= 3 && polyline::signed_area(p).abs() > tol * polyline::perimeter(p))
        .collect();
    Ok(polyline::normalize_set(&loops))
}

/// The moved edges of one loop with their joins, appended to `out`.
fn raw_offset(p: &[Vec2], d: f64, tol: f64, out: &mut Vec<(Vec2, Vec2)>) -> Result<()> {
    let n = p.len();
    // Right-hand normal of each edge, scaled by d: for a counter-clockwise outer the right side
    // is outside, so d > 0 grows it; for a clockwise hole the right side is inside the hole, so
    // d > 0 shrinks the hole. Both grow the region.
    let edge = |i: usize| -> Option<(Vec2, Vec2, Vec2)> {
        let a = p[i % n];
        let b = p[(i + 1) % n];
        let dir = (b - a).normalized()?;
        let normal = Vec2::new(dir.y, -dir.x);
        Some((a + normal * d, b + normal * d, dir))
    };
    let mut prev = edge(n - 1);
    let first_out = out.len();
    // The trim of the loop's last segment where it meets the first (applied after the loop).
    let mut last_trim: Option<Vec2> = None;
    for (i, &v) in p.iter().enumerate() {
        let Some((mut a, b, dir)) = edge(i) else { continue };
        if let Some((pa, pb, pdir)) = prev {
            // Join at vertex p[i] between the previous edge's end pb and this edge's start a.
            let turn = pdir.cross(dir);
            if turn * d <= 0.0 {
                // The corner closes on the offset side: the moved edges cross. Trim both at the
                // crossing instead of bridging them, so no sliver is left for the pruning to
                // miss. Without a usable crossing (near-parallel edges), bridge as before.
                let trimmed = line_intersection(pa, pb, a, b)
                    .filter(|(x, tp, tc)| x.is_finite() && *tp > 0.0 && *tp <= 1.0 + 1e-9 && *tc >= -1e-9 && *tc < 1.0);
                if let Some((x, _, _)) = trimmed {
                    if i == 0 {
                        last_trim = Some(x);
                    } else if let Some(last) = out.last_mut() {
                        last.1 = x;
                    }
                    a = x;
                } else if pb.dist(a) > 1e-12 {
                    out.push((pb, a));
                }
            } else {
                // The corner opens on the offset side: arc about the vertex from pb to a.
                let a0 = (pb - v).angle();
                let mut sweep = (a - v).angle() - a0;
                // The arc rounds the outside of the corner in the turn's own sense: a left turn
                // of a counter-clockwise loop growing outward gets a counter-clockwise arc.
                if turn > 0.0 {
                    while sweep < 0.0 {
                        sweep += std::f64::consts::TAU;
                    }
                } else {
                    while sweep > 0.0 {
                        sweep -= std::f64::consts::TAU;
                    }
                }
                let k = arc_segments(d.abs(), sweep, tol);
                let mut last = pb;
                for j in 1..=k {
                    let ang = a0 + sweep * j as f64 / k as f64;
                    let q = v + Vec2::from_angle(ang) * d.abs();
                    out.push((last, q));
                    last = q;
                }
                if last.dist(a) > 1e-12 {
                    out.push((last, a));
                }
            }
        }
        out.push((a, b));
        prev = Some((a, b, dir));
        if out.len() > MAX_SEGMENTS {
            return Err(CamError::TooLarge("offset produces too many segments".into()));
        }
    }
    if let Some(x) = last_trim
        && out.len() > first_out
        && let Some(last) = out.last_mut()
    {
        last.1 = x;
    }
    Ok(())
}

/// Intersection of the lines through `a`–`b` and `c`–`d`: the point and its parameters along
/// each segment (`None` for parallel lines).
fn line_intersection(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> Option<(Vec2, f64, f64)> {
    let r = b - a;
    let s = d - c;
    let den = r.cross(s);
    if den.abs() < 1e-14 * (r.len() * s.len()).max(1e-30) {
        return None;
    }
    let qp = c - a;
    let t = qp.cross(s) / den;
    let u = qp.cross(r) / den;
    Some((a + r * t, t, u))
}

/// Cuts every segment at its intersections with the others.
fn split_at_intersections(segs: &[(Vec2, Vec2)], tol: f64) -> Result<Vec<(Vec2, Vec2)>> {
    let n = segs.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut lo = Vec2::new(f64::INFINITY, f64::INFINITY);
    let mut hi = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut total = 0.0;
    for (a, b) in segs {
        lo = lo.min_pt(*a).min_pt(*b);
        hi = hi.max_pt(*a).max_pt(*b);
        total += a.dist(*b);
    }
    let size = hi - lo;
    let mean = (total / n as f64).max(tol);
    let cell = mean.max(size.x.max(size.y) / 1024.0).max(1e-9);
    let nx = ((size.x / cell).ceil() as usize + 1).clamp(1, 2048);
    let ny = ((size.y / cell).ceil() as usize + 1).clamp(1, 2048);
    let cell = cell.max(size.x / nx as f64 * 1.0001).max(size.y / ny as f64 * 1.0001);
    let idx = |p: Vec2| -> (usize, usize) {
        let i = ((p.x - lo.x) / cell).floor();
        let j = ((p.y - lo.y) / cell).floor();
        ((if i.is_finite() { i.max(0.0) as usize } else { 0 }).min(nx - 1), (if j.is_finite() { j.max(0.0) as usize } else { 0 }).min(ny - 1))
    };
    let mut cells: Vec<Vec<u32>> = vec![Vec::new(); nx * ny];
    let mut work = 0usize;
    for (k, (a, b)) in segs.iter().enumerate() {
        let (i0, j0) = idx(a.min_pt(*b) - Vec2::new(tol, tol));
        let (i1, j1) = idx(a.max_pt(*b) + Vec2::new(tol, tol));
        for j in j0..=j1 {
            for i in i0..=i1 {
                if let Some(c) = cells.get_mut(j * nx + i) {
                    c.push(k as u32);
                    work += 1;
                }
            }
        }
        if work > 50_000_000 {
            return Err(CamError::TooLarge("offset intersection grid is too large".into()));
        }
    }
    let mut cuts: Vec<Vec<f64>> = vec![Vec::new(); n];
    let mut tests = 0usize;
    for c in &cells {
        for (x, &i) in c.iter().enumerate() {
            for &j in &c[x + 1..] {
                tests += 1;
                if tests > 200_000_000 {
                    return Err(CamError::TooLarge("offset has too many segment pairs".into()));
                }
                let (i, j) = (i as usize, j as usize);
                let (a, b) = segs[i];
                let (c2, d) = segs[j];
                // Adjacent segments share an endpoint; only a crossing in their interiors counts.
                if let Some((t, u)) = polyline::segments_intersect(a, b, c2, d, tol * 0.1) {
                    let eps_i = tol * 0.1 / a.dist(b).max(1e-30);
                    let eps_j = tol * 0.1 / c2.dist(d).max(1e-30);
                    if t > eps_i && t < 1.0 - eps_i {
                        cuts[i].push(t);
                    }
                    if u > eps_j && u < 1.0 - eps_j {
                        cuts[j].push(u);
                    }
                }
            }
        }
    }
    let mut out = Vec::with_capacity(n + cuts.iter().map(Vec::len).sum::<usize>());
    for (k, (a, b)) in segs.iter().enumerate() {
        let ts = &mut cuts[k];
        ts.sort_by(f64::total_cmp);
        ts.dedup_by(|x, y| (*x - *y).abs() < 1e-9);
        let mut last = *a;
        for &t in ts.iter() {
            let q = a.lerp(*b, t);
            out.push((last, q));
            last = q;
        }
        out.push((last, *b));
        if out.len() > MAX_SEGMENTS {
            return Err(CamError::TooLarge("offset produces too many pieces".into()));
        }
    }
    Ok(out)
}

/// Joins directed segments end to start into closed loops; open chains are dropped. At a
/// fork the straightest continuation is tried first; a continuation that dead-ends is undone
/// and the next tried (fragments of a collapsed corner can look like the way on).
fn stitch(segs: &[(Vec2, Vec2)], tol: f64) -> Vec<Poly> {
    use std::collections::HashMap;
    let q = |p: Vec2| -> (i64, i64) {
        let s = 1.0 / tol.max(1e-12);
        ((p.x * s).round() as i64, (p.y * s).round() as i64)
    };
    let mut starts: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (k, (a, _)) in segs.iter().enumerate() {
        starts.entry(q(*a)).or_default().push(k);
    }
    let mut used = vec![false; segs.len()];
    let mut loops = Vec::new();
    // Unused segments starting within reach of `p`, straightest first.
    let candidates = |p: Vec2, dir: Vec2, used: &[bool]| -> Vec<usize> {
        let (cx, cy) = q(p);
        let mut found: Vec<(usize, f64)> = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(cands) = starts.get(&(cx + dx, cy + dy)) {
                    for &k in cands {
                        if used[k] {
                            continue;
                        }
                        let (a, b) = segs[k];
                        if a.dist(p) > tol * 2.0 {
                            continue;
                        }
                        let d = (b - a).normalized().unwrap_or(dir);
                        found.push((k, -d.dot(dir)));
                    }
                }
            }
        }
        found.sort_by(|x, y| x.1.total_cmp(&y.1));
        found.into_iter().map(|(k, _)| k).collect()
    };
    for start in 0..segs.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let origin = segs[start].0;
        // The chain as a stack of (segment, untried continuations from its end).
        let mut stack: Vec<(usize, Vec<usize>)> = Vec::new();
        let first_dir = (segs[start].1 - segs[start].0).normalized().unwrap_or(Vec2::new(1.0, 0.0));
        let mut first = candidates(segs[start].1, first_dir, &used);
        first.reverse();
        stack.push((start, first));
        let mut closed = false;
        let mut steps = 0usize;
        loop {
            steps += 1;
            if steps > segs.len() * 4 + 16 {
                break;
            }
            let depth = stack.len();
            let Some((k, _)) = stack.last() else { break };
            let b = segs[*k].1;
            if depth > 1 && b.dist(origin) <= tol * 2.0 {
                closed = true;
                break;
            }
            let next = stack.last_mut().and_then(|(_, cands)| cands.pop());
            match next {
                Some(next) if !used[next] => {
                    used[next] = true;
                    let (na, nb) = segs[next];
                    let dir = (nb - na).normalized().unwrap_or(Vec2::new(1.0, 0.0));
                    // Reaching the origin closes the chain.
                    if nb.dist(origin) <= tol * 2.0 {
                        stack.push((next, Vec::new()));
                        closed = true;
                        break;
                    }
                    let mut c = candidates(nb, dir, &used);
                    c.reverse(); // pop() takes the last: straightest last
                    stack.push((next, c));
                }
                Some(_) => continue,
                None => {
                    // Dead end: undo this segment (it stays used: a fragment) and try another
                    // continuation from the previous one.
                    stack.pop();
                    if stack.is_empty() {
                        break;
                    }
                }
            }
        }
        if closed && stack.len() >= 3 {
            let mut chain: Vec<Vec2> = stack.iter().map(|(k, _)| segs[*k].0).collect();
            chain.dedup_by(|x, y| x.dist(*y) < tol * 0.5);
            loops.push(chain);
        }
    }
    loops
}

/// Successive inward offsets of a region at `step` until nothing is left: `result[0]` is the
/// region itself; each later entry is `step` farther inside. Used for contour-parallel pocketing.
pub fn inward_family(set: &[Poly], step: f64, tol: f64, max_rings: usize) -> Result<Vec<Vec<Poly>>> {
    if !(step.is_finite() && step > 0.0) {
        return Err(CamError::Param("stepover must be positive".into()));
    }
    let mut family = vec![polyline::normalize_set(set)];
    let mut current = family[0].clone();
    for _ in 0..max_rings {
        let next = offset_set(&current, -step, tol)?;
        if next.is_empty() {
            break;
        }
        family.push(next.clone());
        current = next;
    }
    Ok(family)
}
