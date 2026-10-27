//! The drop cutter: the height at which a tool, descending along its axis at a given position,
//! first touches a triangle mesh. Exact for flat, ball and bull-nose end mills (vertex, edge and
//! facet contacts), through the tool profile for conical tools. The tool axis may be any
//! direction: the mesh is queried in world coordinates through its bounding-volume hierarchy and
//! every candidate triangle is taken into the tool's frame, so one engine serves 3-axis work,
//! indexed (3+2) orientations, 4-axis rotary passes (radial frames) and 5-axis surface work.

use solidcraft_geom::{Aabb3, Bvh, Mesh, Vec2, Vec3};

use crate::frame::Frame;
use crate::tool::Tool;

/// A mesh with its triangle bounding boxes and BVH, ready for drop-cutter queries.
pub struct DropMesh {
    pub tris: Vec<[Vec3; 3]>,
    pub normals: Vec<Vec3>,
    pub bvh: Bvh,
    pub bounds: Aabb3,
}

/// Where the tool touched: the tip height, the contact point on the surface and the surface
/// normal there (unit, pointing out of the material), all in the frame used for the drop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub z: f64,
    pub point: Vec3,
    pub normal: Vec3,
    pub triangle: u32,
}

impl DropMesh {
    pub fn new(mesh: &Mesh) -> DropMesh {
        let mut tris = Vec::with_capacity(mesh.triangles.len());
        let mut normals = Vec::with_capacity(mesh.triangles.len());
        let mut boxes = Vec::with_capacity(mesh.triangles.len());
        let mut bounds = Aabb3::default();
        for t in &mesh.triangles {
            let Some(tri) = mesh.tri(t) else { continue };
            if !(tri[0].is_finite() && tri[1].is_finite() && tri[2].is_finite()) {
                continue;
            }
            let n = (tri[1] - tri[0]).cross(tri[2] - tri[0]);
            let Some(n) = n.normalized() else { continue };
            let mut b = Aabb3::default();
            for p in &tri {
                b.add(*p);
                bounds.add(*p);
            }
            tris.push(tri);
            normals.push(n);
            boxes.push(b);
        }
        let bvh = Bvh::new(&boxes);
        DropMesh { tris, normals, bvh, bounds }
    }

    /// Several meshes as one.
    pub fn from_meshes<'a>(meshes: impl IntoIterator<Item = &'a Mesh>) -> DropMesh {
        let mut all = Mesh::default();
        for m in meshes {
            all.append(m, 0);
        }
        DropMesh::new(&all)
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// Drops the tool at local `(x, y)` of `frame` (its z is the tool axis). Returns the tip
    /// height in the frame, or `None` when nothing lies under the tool (down to `floor`).
    pub fn drop(&self, tool: &Tool, frame: &Frame, xy: Vec2, floor: f64) -> Option<Contact> {
        let r = tool.radius();
        // World box of the tool's vertical column above the floor over the frame.
        let zmax = {
            let b = &self.bounds;
            let corners = [
                Vec3::new(b.min.x, b.min.y, b.min.z),
                Vec3::new(b.max.x, b.min.y, b.min.z),
                Vec3::new(b.min.x, b.max.y, b.min.z),
                Vec3::new(b.max.x, b.max.y, b.min.z),
                Vec3::new(b.min.x, b.min.y, b.max.z),
                Vec3::new(b.max.x, b.min.y, b.max.z),
                Vec3::new(b.min.x, b.max.y, b.max.z),
                Vec3::new(b.max.x, b.max.y, b.max.z),
            ];
            corners.iter().map(|c| frame.to_local(*c).z).fold(f64::NEG_INFINITY, f64::max)
        };
        if !zmax.is_finite() || zmax < floor {
            return None;
        }
        let mut col = Aabb3::default();
        for (dx, dy) in [(-r, -r), (r, -r), (-r, r), (r, r)] {
            for z in [floor, zmax + 1.0] {
                col.add(frame.to_world(Vec3::new(xy.x + dx, xy.y + dy, z)));
            }
        }
        let mut best: Option<Contact> = None;
        let rc = tool.corner_radius();
        let flat = tool.flat_radius();
        self.bvh.visit(
            |b| b.meets(&col),
            |t| {
                let (Some(tri), Some(n)) = (self.tris.get(t as usize), self.normals.get(t as usize)) else { return };
                let l = [frame.to_local(tri[0]), frame.to_local(tri[1]), frame.to_local(tri[2])];
                let nl = frame.dir_to_local(*n);
                let c = drop_triangle(tool, r, rc, flat, xy, &l, nl, t);
                if let Some(c) = c
                    && c.z >= floor
                    && best.is_none_or(|b| c.z > b.z)
                {
                    best = Some(c);
                }
            },
        );
        best
    }

    /// Drops in the world frame (3-axis).
    pub fn drop_z(&self, tool: &Tool, xy: Vec2, floor: f64) -> Option<Contact> {
        self.drop(tool, &Frame::IDENTITY, xy, floor)
    }

    /// Whether a cylinder (the tool's shank, radius `radius`, from local height `z0` to `z1`
    /// above the tip at `tip`, along `axis`) comes within `gap` of the mesh. Used for collision
    /// checks of the shank and holder in multi-axis work.
    pub fn cylinder_hits(&self, tip: Vec3, axis: Vec3, radius: f64, z0: f64, z1: f64, gap: f64) -> bool {
        let Some(a) = axis.normalized() else { return false };
        let p0 = tip + a * z0;
        let p1 = tip + a * z1;
        let mut b = Aabb3::default();
        b.add(p0);
        b.add(p1);
        let b = b.inflated(radius + gap);
        let hit = std::cell::Cell::new(false);
        self.bvh.visit(
            |bb| !hit.get() && bb.meets(&b),
            |t| {
                if hit.get() {
                    return;
                }
                if let Some(tri) = self.tris.get(t as usize)
                    && segment_triangle_distance(p0, p1, tri) < radius + gap
                {
                    hit.set(true);
                }
            },
        );
        hit.get()
    }
}

/// The highest tip height at which a tool at `(x, y)` touches the triangle `tri` (local frame,
/// tool axis z). `n` is the triangle's unit normal.
#[allow(clippy::too_many_arguments)]
fn drop_triangle(tool: &Tool, r: f64, rc: f64, flat: f64, xy: Vec2, tri: &[Vec3; 3], n: Vec3, index: u32) -> Option<Contact> {
    let mut best: Option<Contact> = None;
    let mut consider = |z: f64, point: Vec3, normal: Vec3| {
        if z.is_finite() && best.is_none_or(|b| z > b.z) {
            best = Some(Contact { z, point, normal, triangle: index });
        }
    };
    let exact = tool.is_end_mill();
    // Vertex contacts.
    for v in tri {
        let rho = Vec2::new(v.x, v.y).dist(xy);
        if rho <= r + 1e-12 {
            let h = tool.profile(rho);
            let normal = if exact { profile_normal(rc, flat, xy, *v) } else { Vec3::new(0.0, 0.0, 1.0) };
            consider(v.z - h, *v, normal);
        }
    }
    // Facet contact: only for a facet facing the tool (normal with positive z); a vertical or
    // downward facet cannot be touched by the tool bottom.
    if n.z > 1e-9 {
        let (cx, cy) = (xy.x, xy.y);
        let nxy = Vec2::new(n.x, n.y);
        let nxy_len = nxy.len();
        // Point on the tool bottom that touches a plane of this normal: the ball-centre circle
        // point farthest "uphill", then down the ball by rc along -n.
        let q_xy = if nxy_len > 1e-12 { Vec2::new(cx, cy) - nxy * (flat / nxy_len) } else { Vec2::new(cx, cy) };
        if exact {
            // Ball centre Q at height cz above the tip (z = rc): n·(Q + (0,0,cz)) = n·P0 + rc.
            let d0 = n.dot(tri[0]);
            let cz = (d0 + rc - n.x * q_xy.x - n.y * q_xy.y) / n.z;
            let centre = Vec3::new(q_xy.x, q_xy.y, cz);
            let contact = centre - n * rc;
            if point_in_triangle_xy(Vec2::new(contact.x, contact.y), tri) {
                consider(cz - rc, contact, n);
            }
        } else {
            // Conical tools: the tool profile sampled against the facet along the uphill
            // direction (the max of z_plane(rho) - profile(rho) over the footprint).
            let dir = if nxy_len > 1e-12 { nxy * (-1.0 / nxy_len) } else { Vec2::new(1.0, 0.0) };
            let steps = 16;
            let d0 = n.dot(tri[0]);
            for k in 0..=steps {
                let rho = r * k as f64 / steps as f64;
                let p = xy + dir * rho;
                if !point_in_triangle_xy(p, tri) {
                    continue;
                }
                let zp = (d0 - n.x * p.x - n.y * p.y) / n.z;
                consider(zp - tool.profile(rho), Vec3::new(p.x, p.y, zp), n);
            }
        }
    }
    // Edge contacts.
    for k in 0..3 {
        let a = tri[k];
        let b = tri[(k + 1) % 3];
        if exact {
            edge_contact(r, rc, flat, xy, a, b, &mut consider);
        } else {
            edge_contact_profile(tool, r, xy, a, b, &mut consider);
        }
    }
    best
}

/// Surface normal implied by a vertex contact: from the ball centre nearest the vertex to the
/// vertex, or straight up on the flat bottom.
fn profile_normal(rc: f64, flat: f64, xy: Vec2, v: Vec3) -> Vec3 {
    let d = Vec2::new(v.x, v.y) - xy;
    let rho = d.len();
    if rc <= 0.0 || rho <= flat + 1e-12 {
        return Vec3::new(0.0, 0.0, 1.0);
    }
    let dir = d * (1.0 / rho);
    let q = xy + dir * flat;
    let dz = rho - flat;
    // Vertex lies on the torus: normal is from the ball centre (at height rc) down to it.
    let n = Vec3::new(v.x - q.x, v.y - q.y, -(rc * rc - dz * dz).max(0.0).sqrt());
    n.normalized().map(|n| n * -1.0).unwrap_or(Vec3::new(0.0, 0.0, 1.0))
}

fn point_in_triangle_xy(p: Vec2, tri: &[Vec3; 3]) -> bool {
    let a = Vec2::new(tri[0].x, tri[0].y);
    let b = Vec2::new(tri[1].x, tri[1].y);
    let c = Vec2::new(tri[2].x, tri[2].y);
    let d1 = (b - a).cross(p - a);
    let d2 = (c - b).cross(p - b);
    let d3 = (a - c).cross(p - c);
    let eps = -1e-9 * ((b - a).len() + (c - b).len() + (a - c).len());
    (d1 >= eps && d2 >= eps && d3 >= eps) || (d1 <= -eps && d2 <= -eps && d3 <= -eps)
}

/// Edge contact of an end mill: a ball of radius `rc` centred on the circle of radius `flat`
/// about the axis (at height `rc` over the tip) tangent to the segment. The circle is sampled
/// and the best sample refined, which is exact to well below machining tolerances.
fn edge_contact(r: f64, rc: f64, flat: f64, xy: Vec2, a: Vec3, b: Vec3, consider: &mut impl FnMut(f64, Vec3, Vec3)) {
    let d = b - a;
    let dxy = Vec2::new(d.x, d.y);
    let (ax, ay) = (a.x, a.y);
    // Quick reject: the edge's XY segment must come within r of the axis.
    let (dist_xy, _) = crate::polyline::point_segment(xy, Vec2::new(ax, ay), Vec2::new(b.x, b.y));
    if dist_xy > r + 1e-12 {
        return;
    }
    if rc <= 1e-12 {
        // Flat tool: the bottom circle of radius r meets the edge's XY projection where the
        // edge is highest over the disc rim.
        for t in circle_segment_params(xy, r, Vec2::new(ax, ay), dxy) {
            if (0.0..=1.0).contains(&t) {
                let p = a + d * t;
                consider(p.z, p, Vec3::new(0.0, 0.0, 1.0));
            }
        }
        return;
    }
    // Ball/bull: for a ball centre Q on the circle (radius flat about xy, height rc above the
    // tip), tangency to the line gives the tip height; maximise over the circle.
    let eval = |ang: f64| -> Option<(f64, f64)> {
        let q = xy + Vec2::from_angle(ang) * flat;
        // Line: a + t d. Distance from C = (q, cz) to the line equals rc; solve for cz (upper).
        // |(C - a) × d|² = rc² |d|².
        // C - a = w + cz·e with C = (q, cz) absolute.
        let w = Vec3::new(q.x - ax, q.y - ay, -a.z);
        let e = Vec3::new(0.0, 0.0, 1.0);
        let wxd = w.cross(d);
        let exd = e.cross(d);
        // |wxd + cz exd|² = rc² |d|²  →  A cz² + B cz + C = 0.
        let aa = exd.len2();
        let bb = 2.0 * wxd.dot(exd);
        let cc = wxd.len2() - rc * rc * d.len2();
        if aa < 1e-18 {
            return None; // vertical edge: handled by vertex contacts
        }
        let disc = bb * bb - 4.0 * aa * cc;
        if disc < 0.0 {
            return None;
        }
        let cz = (-bb + disc.sqrt()) / (2.0 * aa);
        // Contact parameter along the segment.
        let c = Vec3::new(q.x, q.y, cz);
        let t = (c - a).dot(d) / d.len2();
        if !(0.0..=1.0).contains(&t) {
            return None;
        }
        Some((cz - rc, t))
    };
    let n = if flat <= 1e-12 { 1 } else { 24 };
    let mut best: Option<(f64, f64, f64)> = None;
    for k in 0..n {
        let ang = k as f64 / n as f64 * std::f64::consts::TAU;
        if let Some((z, t)) = eval(ang)
            && best.is_none_or(|(bz, _, _)| z > bz)
        {
            best = Some((z, ang, t));
        }
    }
    let Some((mut z, ang, mut t)) = best else { return };
    if n > 1 {
        // Golden-section refinement around the best sample.
        let step = std::f64::consts::TAU / n as f64;
        let (mut lo, mut hi) = (ang - step, ang + step);
        let gr = 0.618_033_988_75;
        for _ in 0..30 {
            let m1 = hi - gr * (hi - lo);
            let m2 = lo + gr * (hi - lo);
            let f1 = eval(m1).map(|x| x.0).unwrap_or(f64::NEG_INFINITY);
            let f2 = eval(m2).map(|x| x.0).unwrap_or(f64::NEG_INFINITY);
            if f1 < f2 {
                lo = m1;
            } else {
                hi = m2;
            }
        }
        if let Some((zz, tt)) = eval((lo + hi) * 0.5)
            && zz > z
        {
            z = zz;
            t = tt;
        }
    }
    let p = a + d * t;
    let ang_best = if n > 1 { ang } else { 0.0 };
    let q = xy + Vec2::from_angle(ang_best) * flat;
    let centre = Vec3::new(q.x, q.y, z + rc);
    let normal = (centre - p).normalized().unwrap_or(Vec3::new(0.0, 0.0, 1.0));
    consider(z, p, normal);
}

/// Edge contact through the tool profile (conical tools): sample the edge where it passes
/// over the footprint.
fn edge_contact_profile(tool: &Tool, r: f64, xy: Vec2, a: Vec3, b: Vec3, consider: &mut impl FnMut(f64, Vec3, Vec3)) {
    let d = b - a;
    let len = Vec2::new(d.x, d.y).len();
    if len < 1e-12 {
        return;
    }
    let steps = ((len / (r * 0.1).max(1e-3)).ceil() as usize).clamp(2, 64);
    for k in 0..=steps {
        let p = a + d * (k as f64 / steps as f64);
        let rho = Vec2::new(p.x, p.y).dist(xy);
        if rho <= r {
            consider(p.z - tool.profile(rho), p, Vec3::new(0.0, 0.0, 1.0));
        }
    }
}

/// Parameters `t` where the segment `a + t d` (2D) meets the circle about `c` of radius `r`.
fn circle_segment_params(c: Vec2, r: f64, a: Vec2, d: Vec2) -> Vec<f64> {
    let f = a - c;
    let aa = d.dot(d);
    if aa < 1e-24 {
        return Vec::new();
    }
    let bb = 2.0 * f.dot(d);
    let cc = f.dot(f) - r * r;
    let disc = bb * bb - 4.0 * aa * cc;
    if disc < 0.0 {
        return Vec::new();
    }
    let s = disc.sqrt();
    vec![(-bb - s) / (2.0 * aa), (-bb + s) / (2.0 * aa)]
}

/// Distance between the segment `p0`–`p1` and a triangle (sampled along the segment and against
/// the triangle's plane and edges; exact enough for clearance checks).
pub fn segment_triangle_distance(p0: Vec3, p1: Vec3, tri: &[Vec3; 3]) -> f64 {
    let n = (tri[1] - tri[0]).cross(tri[2] - tri[0]).normalized();
    let mut best = f64::INFINITY;
    let steps = 8;
    for k in 0..=steps {
        let p = p0.lerp(p1, k as f64 / steps as f64);
        best = best.min(point_triangle_distance(p, tri, n));
    }
    // Triangle vertices to the segment.
    for v in tri {
        best = best.min(v.dist_to_segment(p0, p1));
    }
    // Segment crossing the triangle's plane inside it means distance 0.
    if let Some(n) = n {
        let d0 = (p0 - tri[0]).dot(n);
        let d1 = (p1 - tri[0]).dot(n);
        if (d0 > 0.0) != (d1 > 0.0) {
            let t = d0 / (d0 - d1);
            let p = p0.lerp(p1, t);
            if point_in_triangle_3d(p, tri, n) {
                return 0.0;
            }
        }
    }
    best
}

fn point_in_triangle_3d(p: Vec3, tri: &[Vec3; 3], n: Vec3) -> bool {
    let c0 = (tri[1] - tri[0]).cross(p - tri[0]).dot(n);
    let c1 = (tri[2] - tri[1]).cross(p - tri[1]).dot(n);
    let c2 = (tri[0] - tri[2]).cross(p - tri[2]).dot(n);
    (c0 >= -1e-12 && c1 >= -1e-12 && c2 >= -1e-12) || (c0 <= 1e-12 && c1 <= 1e-12 && c2 <= 1e-12)
}

/// Distance from a point to a triangle.
pub fn point_triangle_distance(p: Vec3, tri: &[Vec3; 3], n: Option<Vec3>) -> f64 {
    if let Some(n) = n {
        let d = (p - tri[0]).dot(n);
        let q = p - n * d;
        if point_in_triangle_3d(q, tri, n) {
            return d.abs();
        }
    }
    (0..3).map(|k| p.dist_to_segment(tri[k], tri[(k + 1) % 3])).fold(f64::INFINITY, f64::min)
}
