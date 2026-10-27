//! Planar polygons as closed polylines (the last point joins the first) and the operations
//! 2.5D toolpaths need: area and orientation, containment, scanline clipping, segment tests,
//! simplification and a grid for nearest-segment distance queries.

use solidcraft_geom::{Aabb2, Vec2};

/// A closed polygon: consecutive points are edges and the last point joins the first.
pub type Poly = Vec<Vec2>;

/// Twice the signed area (positive for counter-clockwise).
pub fn signed_area2(p: &[Vec2]) -> f64 {
    let n = p.len();
    if n < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    for i in 0..n {
        let a = p[i];
        let b = p[(i + 1) % n];
        s += a.x * b.y - b.x * a.y;
    }
    s
}

pub fn signed_area(p: &[Vec2]) -> f64 {
    signed_area2(p) * 0.5
}

pub fn is_ccw(p: &[Vec2]) -> bool {
    signed_area2(p) > 0.0
}

pub fn perimeter(p: &[Vec2]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i].dist(p[(i + 1) % n])).sum()
}

pub fn bounds(p: &[Vec2]) -> Aabb2 {
    let mut b = Aabb2::default();
    for q in p {
        b.add(*q);
    }
    b
}

pub fn bounds_of(set: &[Poly]) -> Aabb2 {
    let mut b = Aabb2::default();
    for p in set {
        for q in p {
            b.add(*q);
        }
    }
    b
}

/// Whether `q` is inside the polygon (even-odd rule; points on the boundary count as inside
/// or outside depending on rounding).
pub fn contains(p: &[Vec2], q: Vec2) -> bool {
    !crossings(p, q).is_multiple_of(2)
}

/// Number of polygon edges a ray from `q` in +x crosses.
fn crossings(p: &[Vec2], q: Vec2) -> usize {
    let n = p.len();
    let mut c = 0;
    for i in 0..n {
        let a = p[i];
        let b = p[(i + 1) % n];
        if (a.y > q.y) != (b.y > q.y) {
            let x = a.x + (q.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if x > q.x {
                c += 1;
            }
        }
    }
    c
}

/// Whether `q` is inside the region bounded by the polygon set (even-odd over all loops: a hole
/// inside an outer loop is outside the region, an island inside a hole is inside again).
pub fn set_contains(set: &[Poly], q: Vec2) -> bool {
    set.iter().map(|p| crossings(p, q)).sum::<usize>() % 2 == 1
}

/// Depth of a loop in the nesting of a set: how many other loops contain its first point.
pub fn nesting_depth(set: &[Poly], i: usize) -> usize {
    let Some(q) = set.get(i).and_then(|p| p.first()) else { return 0 };
    // Use a point slightly inside the loop rather than a vertex, which may lie on another loop.
    let probe = set.get(i).map(|p| interior_probe(p)).unwrap_or(*q);
    set.iter().enumerate().filter(|(j, p)| *j != i && contains(p, probe)).count()
}

/// A point just inside the loop: the midpoint of the first edge moved to the inside by a small
/// fraction of the loop's size.
pub fn interior_probe(p: &[Vec2]) -> Vec2 {
    if p.len() < 2 {
        return p.first().copied().unwrap_or_default();
    }
    let b = bounds(p);
    let eps = b.diagonal().max(1e-9) * 1e-6;
    let inward = if is_ccw(p) { 1.0 } else { -1.0 };
    // Try each edge until the probe lands inside (a tiny edge may not work).
    let n = p.len();
    for i in 0..n {
        let a = p[i];
        let c = p[(i + 1) % n];
        let d = c - a;
        if d.len() < 1e-12 {
            continue;
        }
        let m = a.lerp(c, 0.5);
        let left = Vec2::new(-d.y, d.x).normalized().unwrap_or_default();
        let q = m + left * (eps * inward);
        if contains(p, q) {
            return q;
        }
    }
    p[0]
}

/// Orients every loop of a set so outers run counter-clockwise and holes clockwise (by
/// nesting depth), dropping loops with fewer than three points or no area.
pub fn normalize_set(set: &[Poly]) -> Vec<Poly> {
    let kept: Vec<Poly> = set.iter().filter(|p| p.len() >= 3 && signed_area2(p).abs() > 1e-18).cloned().collect();
    (0..kept.len())
        .map(|i| {
            let want_ccw = nesting_depth(&kept, i).is_multiple_of(2);
            let p = &kept[i];
            if is_ccw(p) == want_ccw { p.clone() } else { p.iter().rev().copied().collect() }
        })
        .collect()
}

/// Total area of a normalized set (outers minus holes).
pub fn set_area(set: &[Poly]) -> f64 {
    set.iter().map(|p| signed_area(p)).sum()
}

/// Distance from `q` to the segment `a`–`b`, and the parameter of the closest point.
pub fn point_segment(q: Vec2, a: Vec2, b: Vec2) -> (f64, f64) {
    let d = b - a;
    let l2 = d.len2();
    if l2 < 1e-24 {
        return (q.dist(a), 0.0);
    }
    let t = ((q - a).dot(d) / l2).clamp(0.0, 1.0);
    (q.dist(a + d * t), t)
}

/// Distance from `q` to the boundary of a polygon set.
pub fn distance_to_set(set: &[Poly], q: Vec2) -> f64 {
    let mut best = f64::INFINITY;
    for p in set {
        let n = p.len();
        for i in 0..n {
            best = best.min(point_segment(q, p[i], p[(i + 1) % n]).0);
        }
    }
    best
}

/// Intersection of segments `a`–`b` and `c`–`d` as parameters `(t, u)` along each, when they
/// cross or touch within `tol` (parallel overlapping segments report nothing).
pub fn segments_intersect(a: Vec2, b: Vec2, c: Vec2, d: Vec2, tol: f64) -> Option<(f64, f64)> {
    let r = b - a;
    let s = d - c;
    let den = r.cross(s);
    let qp = c - a;
    if den.abs() < 1e-14 * (r.len() * s.len()).max(1e-30) {
        return None;
    }
    let t = qp.cross(s) / den;
    let u = qp.cross(r) / den;
    let tt = tol / r.len().max(1e-30);
    let tu = tol / s.len().max(1e-30);
    if t >= -tt && t <= 1.0 + tt && u >= -tu && u <= 1.0 + tu { Some((t.clamp(0.0, 1.0), u.clamp(0.0, 1.0))) } else { None }
}

/// Whether the open segment `a`–`b` crosses any edge of the set (touching an endpoint of the
/// segment within `tol` does not count).
pub fn segment_crosses_set(set: &[Poly], a: Vec2, b: Vec2, tol: f64) -> bool {
    let len = a.dist(b);
    if len < 1e-12 {
        return false;
    }
    let margin = (tol / len).min(0.49);
    for p in set {
        let n = p.len();
        for i in 0..n {
            if let Some((t, _)) = segments_intersect(a, b, p[i], p[(i + 1) % n], 0.0)
                && t > margin
                && t < 1.0 - margin
            {
                return true;
            }
        }
    }
    false
}

/// Whether the segment `a`–`b` lies inside the region: its midpoint is inside and it crosses
/// no boundary edge (with its ends allowed on the boundary).
pub fn segment_inside(set: &[Poly], a: Vec2, b: Vec2, tol: f64) -> bool {
    set_contains(set, a.lerp(b, 0.5)) && !segment_crosses_set(set, a, b, tol)
}

/// Where the horizontal line at `y` is inside the region: sorted `(x_start, x_end)` spans.
pub fn scanline(set: &[Poly], y: f64) -> Vec<(f64, f64)> {
    let mut xs: Vec<f64> = Vec::new();
    for p in set {
        let n = p.len();
        for i in 0..n {
            let a = p[i];
            let b = p[(i + 1) % n];
            if (a.y > y) != (b.y > y) {
                xs.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
            }
        }
    }
    xs.sort_by(f64::total_cmp);
    xs.as_chunks::<2>().0.iter().map(|c| (c[0], c[1])).filter(|(a, b)| b > a).collect()
}

/// Removes consecutive duplicate points (within `tol`) and collinear middle points of a closed
/// polygon.
pub fn clean(p: &[Vec2], tol: f64) -> Poly {
    let mut out: Poly = Vec::with_capacity(p.len());
    for q in p {
        if out.last().is_none_or(|l| l.dist(*q) > tol) {
            out.push(*q);
        }
    }
    while out.len() > 1 && out[0].dist(out[out.len() - 1]) <= tol {
        out.pop();
    }
    if out.len() < 3 {
        return out;
    }
    // Collinear removal.
    let mut res: Poly = Vec::with_capacity(out.len());
    let n = out.len();
    for i in 0..n {
        let a = out[(i + n - 1) % n];
        let b = out[i];
        let c = out[(i + 1) % n];
        let (d, _) = point_segment(b, a, c);
        if d > tol * 0.1 {
            res.push(b);
        }
    }
    if res.len() < 3 { out } else { res }
}

/// Douglas–Peucker simplification of an open polyline to within `tol`.
pub fn simplify_open(p: &[Vec2], tol: f64) -> Vec<Vec2> {
    if p.len() <= 2 {
        return p.to_vec();
    }
    let mut keep = vec![false; p.len()];
    keep[0] = true;
    keep[p.len() - 1] = true;
    let mut stack = vec![(0usize, p.len() - 1)];
    let mut guard = 0usize;
    while let Some((i, j)) = stack.pop() {
        guard += 1;
        if guard > p.len() * 4 + 16 {
            break;
        }
        if j <= i + 1 {
            continue;
        }
        let (mut best, mut bd) = (i + 1, -1.0);
        for k in i + 1..j {
            let d = point_segment(p[k], p[i], p[j]).0;
            if d > bd {
                bd = d;
                best = k;
            }
        }
        if bd > tol {
            keep[best] = true;
            stack.push((i, best));
            stack.push((best, j));
        }
    }
    p.iter().zip(keep).filter(|(_, k)| *k).map(|(q, _)| *q).collect()
}

/// Douglas–Peucker simplification of a closed polygon.
pub fn simplify_closed(p: &[Vec2], tol: f64) -> Poly {
    if p.len() < 4 {
        return p.to_vec();
    }
    // Split at the two points farthest apart so both halves are open polylines.
    let mut far = (0usize, 0usize, -1.0);
    let b = bounds(p);
    let (lo, hi) = (b.min, b.max);
    let i0 = p.iter().enumerate().min_by(|a, c| (a.1.x - lo.x).total_cmp(&(c.1.x - lo.x))).map_or(0, |x| x.0);
    let i1 = p.iter().enumerate().max_by(|a, c| (a.1.x - hi.x).total_cmp(&(c.1.x - hi.x))).map_or(0, |x| x.0);
    far.0 = i0.min(i1);
    far.1 = i0.max(i1);
    if far.0 == far.1 {
        return p.to_vec();
    }
    let first: Vec<Vec2> = p[far.0..=far.1].to_vec();
    let mut second: Vec<Vec2> = p[far.1..].to_vec();
    second.extend_from_slice(&p[..=far.0]);
    let mut out = simplify_open(&first, tol);
    let s = simplify_open(&second, tol);
    out.extend(s.iter().skip(1).take(s.len().saturating_sub(2)).copied());
    out
}

/// Points along the polygon at most `step` apart (every vertex is kept).
pub fn densify(p: &[Vec2], step: f64) -> Poly {
    let step = if step.is_finite() && step > 1e-6 { step } else { 1.0 };
    let n = p.len();
    let mut out = Vec::new();
    for i in 0..n {
        let a = p[i];
        let b = p[(i + 1) % n];
        let d = a.dist(b);
        let k = ((d / step).ceil() as usize).clamp(1, 100_000);
        for j in 0..k {
            out.push(a.lerp(b, j as f64 / k as f64));
        }
    }
    out
}

/// A uniform grid over the edges of a polygon set, for nearest-edge distance queries.
pub struct SegmentGrid {
    segs: Vec<(Vec2, Vec2)>,
    cells: Vec<Vec<u32>>,
    origin: Vec2,
    cell: f64,
    nx: usize,
    ny: usize,
}

impl SegmentGrid {
    pub fn new(set: &[Poly]) -> SegmentGrid {
        let mut segs = Vec::new();
        for p in set {
            let n = p.len();
            for i in 0..n {
                segs.push((p[i], p[(i + 1) % n]));
            }
        }
        let b = bounds_of(set);
        let total: f64 = segs.iter().map(|(a, c)| a.dist(*c)).sum();
        let mean = if segs.is_empty() { 1.0 } else { (total / segs.len() as f64).max(1e-6) };
        let size = b.size();
        let cell = mean.max(size.x.max(size.y) / 512.0).max(1e-6);
        let nx = ((size.x / cell).ceil() as usize + 1).clamp(1, 1024);
        let ny = ((size.y / cell).ceil() as usize + 1).clamp(1, 1024);
        let cell = cell.max(size.x / nx as f64).max(size.y / ny as f64);
        let origin = b.min;
        let mut cells = vec![Vec::new(); nx * ny];
        for (k, (a, c)) in segs.iter().enumerate() {
            let (i0, j0) = Self::index_of(origin, cell, nx, ny, a.min_pt(*c));
            let (i1, j1) = Self::index_of(origin, cell, nx, ny, a.max_pt(*c));
            for j in j0..=j1 {
                for i in i0..=i1 {
                    if let Some(c) = cells.get_mut(j * nx + i) {
                        c.push(k as u32);
                    }
                }
            }
        }
        SegmentGrid { segs, cells, origin, cell, nx, ny }
    }

    fn index_of(origin: Vec2, cell: f64, nx: usize, ny: usize, p: Vec2) -> (usize, usize) {
        let i = ((p.x - origin.x) / cell).floor();
        let j = ((p.y - origin.y) / cell).floor();
        let i = if i.is_finite() { (i.max(0.0) as usize).min(nx - 1) } else { 0 };
        let j = if j.is_finite() { (j.max(0.0) as usize).min(ny - 1) } else { 0 };
        (i, j)
    }

    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }

    /// Distance from `q` to the nearest edge (searching outward ring by ring, so it costs
    /// little when an edge is near).
    pub fn distance(&self, q: Vec2) -> f64 {
        if self.segs.is_empty() {
            return f64::INFINITY;
        }
        let (ci, cj) = Self::index_of(self.origin, self.cell, self.nx, self.ny, q);
        let mut best = f64::INFINITY;
        let max_ring = self.nx.max(self.ny);
        for ring in 0..=max_ring {
            // Any edge in a farther ring is at least (ring - 1) cells away.
            let lower = (ring as f64 - 1.0) * self.cell - self.outside_distance(q);
            if lower > best {
                break;
            }
            let r = ring as isize;
            for dj in -r..=r {
                for di in -r..=r {
                    if di.abs() != r && dj.abs() != r {
                        continue;
                    }
                    let i = ci as isize + di;
                    let j = cj as isize + dj;
                    if i < 0 || j < 0 || i >= self.nx as isize || j >= self.ny as isize {
                        continue;
                    }
                    if let Some(cell) = self.cells.get(j as usize * self.nx + i as usize) {
                        for &k in cell {
                            if let Some((a, b)) = self.segs.get(k as usize) {
                                best = best.min(point_segment(q, *a, *b).0);
                            }
                        }
                    }
                }
            }
        }
        best
    }

    /// How far `q` lies outside the grid (0 inside), which loosens the ring bound.
    fn outside_distance(&self, q: Vec2) -> f64 {
        let maxx = self.origin.x + self.cell * self.nx as f64;
        let maxy = self.origin.y + self.cell * self.ny as f64;
        let dx = (self.origin.x - q.x).max(q.x - maxx).max(0.0);
        let dy = (self.origin.y - q.y).max(q.y - maxy).max(0.0);
        (dx * dx + dy * dy).sqrt()
    }
}

/// Componentwise min and max of two points.
pub trait MinMaxPt {
    fn min_pt(self, o: Vec2) -> Vec2;
    fn max_pt(self, o: Vec2) -> Vec2;
}

impl MinMaxPt for Vec2 {
    fn min_pt(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x.min(o.x), self.y.min(o.y))
    }
    fn max_pt(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x.max(o.x), self.y.max(o.y))
    }
}

/// A circle as a polygon with enough points for a chord error of `tol`.
pub fn circle(center: Vec2, radius: f64, tol: f64) -> Poly {
    let n = arc_segments(radius, std::f64::consts::TAU, tol);
    (0..n).map(|i| center + Vec2::from_angle(i as f64 / n as f64 * std::f64::consts::TAU) * radius).collect()
}

/// Number of straight segments that keep an arc of `radius` spanning `angle` within `tol`.
pub fn arc_segments(radius: f64, angle: f64, tol: f64) -> usize {
    let tol = if tol.is_finite() && tol > 0.0 { tol } else { 0.01 };
    if !(radius.is_finite() && radius > tol) {
        return 1.max((angle.abs() / 1.0).ceil() as usize);
    }
    let step = 2.0 * (1.0 - tol / radius).clamp(-1.0, 1.0).acos();
    ((angle.abs() / step.max(1e-6)).ceil() as usize).clamp(1, 100_000)
}

/// A rectangle from `min` to `max`, counter-clockwise.
pub fn rect(min: Vec2, max: Vec2) -> Poly {
    vec![min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)]
}

/// The largest inscribed circle of a region, approximately: the point of a sample grid farthest
/// from the boundary (`(center, radius)`), refined on a finer grid around it.
pub fn largest_inscribed(set: &[Poly], samples: usize) -> Option<(Vec2, f64)> {
    let b = bounds_of(set);
    if b.is_empty() {
        return None;
    }
    let grid = SegmentGrid::new(set);
    let n = samples.clamp(4, 400);
    let mut best: Option<(Vec2, f64)> = None;
    let size = b.size();
    let search = |lo: Vec2, span: Vec2, n: usize, best: &mut Option<(Vec2, f64)>| {
        for j in 0..=n {
            for i in 0..=n {
                let q = Vec2::new(lo.x + span.x * i as f64 / n as f64, lo.y + span.y * j as f64 / n as f64);
                if !set_contains(set, q) {
                    continue;
                }
                let d = grid.distance(q);
                if best.is_none_or(|(_, r)| d > r) {
                    *best = Some((q, d));
                }
            }
        }
    };
    search(b.min, size, n, &mut best);
    if let Some((c, _)) = best {
        let span = Vec2::new(size.x / n as f64 * 2.0, size.y / n as f64 * 2.0);
        search(c - span * 0.5, span, 16, &mut best);
    }
    best
}
