//! Stock simulation and checks. Three-axis moves (tool axis along z of the operation frame)
//! are swept through a Z-map of the stock, which gives the material removed, the stock left
//! for the next operation (rest machining), gouges below the model and rapids that would hit
//! material. Multi-axis moves are checked for shank and holder collisions against the model.

use serde::{Deserialize, Serialize};
use solidcore_record::{Vec2, Vec3};
use solidcraft_geom::prelude::*;
use solidcraft_geom::{Aabb2, Mesh};

use crate::dropcutter::DropMesh;
use crate::heightmap::HeightMap;
use crate::notice::{Issue, Notice, r3};
use crate::setup::StockShape;
use crate::tool::Tool;
use crate::toolpath::{MoveKind, Toolpath};
use crate::{CamError, Result};

/// The stock as a Z-map: the height of material at each cell (the stock bottom where it is
/// cut through, `NEG_INFINITY` outside the stock's outline). Stock carried over from a setup
/// that machined the other side has a bottom of its own in each cell (`floor`); a cell whose
/// top is at its floor is cut through.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StockMap {
    pub map: HeightMap,
    pub bottom: f64,
    /// The bottom of the material in each cell, when it is not `bottom` everywhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor: Option<HeightMap>,
}

impl StockMap {
    /// A stock map of `shape` (operation frame) at `cell`.
    pub fn new(shape: &StockShape, cell: f64) -> Result<StockMap> {
        let b = shape.bounds();
        let mut b2 = Aabb2::default();
        b2.add(Vec2::new(b.min.x, b.min.y));
        b2.add(Vec2::new(b.max.x, b.max.y));
        let b2 = b2.inflated(cell);
        let mut map = HeightMap::new(b2, cell, f64::NEG_INFINITY)?;
        let bottom = b.min.z;
        for j in 0..map.ny {
            for i in 0..map.nx {
                let p = map.point(i, j);
                let inside = match shape {
                    StockShape::Box { min, max } => p.x >= min.x && p.x < max.x && p.y >= min.y && p.y < max.y,
                    StockShape::Cylinder { axis, radius, start, end } => {
                        let d = axis.dir();
                        // Along z: a disc; else a rectangle seen from above (the cylinder's side).
                        if d.z.abs() > 0.5 {
                            p.len() < *radius
                        } else {
                            let along = Vec3::new(p.x, p.y, 0.0).dot(d);
                            let across = (Vec3::new(p.x, p.y, 0.0) - d * along).len();
                            along >= *start && along < *end && across < *radius
                        }
                    }
                };
                if inside {
                    let top = match shape {
                        StockShape::Box { max, .. } => max.z,
                        StockShape::Cylinder { axis, radius, .. } => {
                            let d = axis.dir();
                            if d.z.abs() > 0.5 {
                                b.max.z
                            } else {
                                let along = Vec3::new(p.x, p.y, 0.0).dot(d);
                                let across = (Vec3::new(p.x, p.y, 0.0) - d * along).len();
                                (radius * radius - across * across).max(0.0).sqrt()
                            }
                        }
                    };
                    map.set(i, j, top);
                }
            }
        }
        Ok(StockMap { map, bottom, floor: None })
    }

    /// The bottom of the material in cell (`i`, `j`).
    pub fn floor_at(&self, i: usize, j: usize) -> f64 {
        self.floor.as_ref().and_then(|f| f.get(i, j)).filter(|z| z.is_finite()).unwrap_or(self.bottom)
    }

    pub fn volume(&self) -> f64 {
        match &self.floor {
            None => self.map.integrate(|z| if z.is_finite() { (z - self.bottom).max(0.0) } else { 0.0 }),
            Some(f) => {
                let sum: f64 = self
                    .map
                    .z
                    .iter()
                    .zip(&f.z)
                    .map(|(z, lo)| if z.is_finite() { (z - if lo.is_finite() { *lo } else { self.bottom }).max(0.0) } else { 0.0 })
                    .sum();
                sum * self.map.cell * self.map.cell
            }
        }
    }

    /// The material's tops for rest machining: a column with no material left (cut through
    /// to its own floor) reads as empty, as outside the stock does.
    pub fn tops(&self) -> HeightMap {
        let mut m = self.map.clone();
        if self.floor.is_some() {
            for j in 0..m.ny {
                for i in 0..m.nx {
                    if m.get(i, j).is_some_and(|z| z.is_finite() && z <= self.floor_at(i, j) + 1e-9) {
                        m.set(i, j, f64::NEG_INFINITY);
                    }
                }
            }
        }
        m
    }

    /// The extent of the material left (in this map's frame), empty when none is.
    pub fn material_bounds(&self) -> solidcraft_geom::Aabb3 {
        let mut b = solidcraft_geom::Aabb3::default();
        let half = self.map.cell * 0.5;
        for j in 0..self.map.ny {
            for i in 0..self.map.nx {
                let Some(top) = self.map.get(i, j).filter(|z| z.is_finite()) else { continue };
                let lo = self.floor_at(i, j);
                if top <= lo + 1e-9 {
                    continue;
                }
                let p = self.map.point(i, j);
                b.add(Vec3::new(p.x - half, p.y - half, lo));
                b.add(Vec3::new(p.x + half, p.y + half, top));
            }
        }
        b
    }

    /// This stock (in the frame `from`, design coordinates to map) seen in the frame `to`: the
    /// material of each column, turned over or moved as the frames are. Exact when the two
    /// frames' tool axes are parallel or opposite (a part turned over for its other side), the
    /// columns staying columns; otherwise the material's bounding box in `to`, and `false`.
    pub fn transfer(&self, from: &crate::frame::Frame, to: &crate::frame::Frame) -> Result<(StockMap, bool)> {
        let za = from.dir_to_world(Vec3::new(0.0, 0.0, 1.0));
        let zb = to.dir_to_world(Vec3::new(0.0, 0.0, 1.0));
        let along = za.dot(zb);
        // The material's corners in `to`.
        let mb = self.material_bounds();
        if mb.is_empty() {
            return Err(CamError::Geometry("the earlier setup leaves no material".into()));
        }
        let mut bb = solidcraft_geom::Aabb3::default();
        for x in [mb.min.x, mb.max.x] {
            for y in [mb.min.y, mb.max.y] {
                for z in [mb.min.z, mb.max.z] {
                    bb.add(to.to_local(from.to_world(Vec3::new(x, y, z))));
                }
            }
        }
        if along.abs() < 1.0 - 1e-6 {
            let shape = crate::setup::StockShape::Box { min: bb.min, max: bb.max };
            return Ok((StockMap::new(&shape, self.map.cell)?, false));
        }
        let cell = self.map.cell;
        let mut b2 = Aabb2::default();
        b2.add(Vec2::new(bb.min.x, bb.min.y));
        b2.add(Vec2::new(bb.max.x, bb.max.y));
        let mut map = HeightMap::new(b2.inflated(cell), cell, f64::NEG_INFINITY)?;
        let mut floor = HeightMap { origin: map.origin, cell, nx: map.nx, ny: map.ny, z: vec![f64::NEG_INFINITY; map.z.len()] };
        let mut bottom = f64::INFINITY;
        for j in 0..map.ny {
            for i in 0..map.nx {
                let p = map.point(i, j);
                let q = from.to_local(to.to_world(Vec3::new(p.x, p.y, 0.0)));
                let si = ((q.x - self.map.origin.x) / cell).round();
                let sj = ((q.y - self.map.origin.y) / cell).round();
                if si < 0.0 || sj < 0.0 {
                    continue;
                }
                let (si, sj) = (si as usize, sj as usize);
                let Some(top) = self.map.get(si, sj).filter(|z| z.is_finite()) else { continue };
                let lo = self.floor_at(si, sj);
                if top <= lo + 1e-9 {
                    continue;
                }
                let z1 = to.to_local(from.to_world(Vec3::new(q.x, q.y, lo))).z;
                let z2 = to.to_local(from.to_world(Vec3::new(q.x, q.y, top))).z;
                map.set(i, j, z1.max(z2));
                floor.set(i, j, z1.min(z2));
                bottom = bottom.min(z1.min(z2));
            }
        }
        if !bottom.is_finite() {
            return Err(CamError::Geometry("the earlier setup leaves no material".into()));
        }
        Ok((StockMap { map, bottom, floor: Some(floor) }, true))
    }

    /// Sweeps a 3-axis cut from `a` to `b` (tip positions) with `tool`.
    pub fn cut(&mut self, tool: &Tool, a: Vec3, b: Vec3) {
        let bottom = self.bottom;
        let floor = &self.floor;
        let r = tool.radius();
        let m = &mut self.map;
        let lo = Vec2::new(a.x.min(b.x) - r, a.y.min(b.y) - r);
        let hi = Vec2::new(a.x.max(b.x) + r, a.y.max(b.y) + r);
        let i0 = (((lo.x - m.origin.x) / m.cell).floor().max(0.0)) as usize;
        let j0 = (((lo.y - m.origin.y) / m.cell).floor().max(0.0)) as usize;
        let i1 = ((((hi.x - m.origin.x) / m.cell).ceil().max(0.0)) as usize).min(m.nx.saturating_sub(1));
        let j1 = ((((hi.y - m.origin.y) / m.cell).ceil().max(0.0)) as usize).min(m.ny.saturating_sub(1));
        let a2 = Vec2::new(a.x, a.y);
        let b2 = Vec2::new(b.x, b.y);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let p = m.point(i, j);
                let Some(z) = lowest_bottom(tool, p, a, b, a2, b2, r) else { continue };
                if let Some(cur) = m.get(i, j)
                    && cur.is_finite()
                    && z < cur
                {
                    let lo = floor.as_ref().and_then(|f| f.get(i, j)).filter(|z| z.is_finite()).unwrap_or(bottom);
                    m.set(i, j, z.max(lo));
                }
            }
        }
    }

    /// Whether the tool at `a`–`b` (a rapid) would pass through material: the stock is higher
    /// than the tool bottom somewhere under the footprint.
    pub fn rapid_hits(&self, tool: &Tool, a: Vec3, b: Vec3, tol: f64) -> bool {
        let r = tool.radius();
        let m = &self.map;
        let lo = Vec2::new(a.x.min(b.x) - r, a.y.min(b.y) - r);
        let hi = Vec2::new(a.x.max(b.x) + r, a.y.max(b.y) + r);
        let i0 = (((lo.x - m.origin.x) / m.cell).floor().max(0.0)) as usize;
        let j0 = (((lo.y - m.origin.y) / m.cell).floor().max(0.0)) as usize;
        let i1 = ((((hi.x - m.origin.x) / m.cell).ceil().max(0.0)) as usize).min(m.nx.saturating_sub(1));
        let j1 = ((((hi.y - m.origin.y) / m.cell).ceil().max(0.0)) as usize).min(m.ny.saturating_sub(1));
        let a2 = Vec2::new(a.x, a.y);
        let b2 = Vec2::new(b.x, b.y);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let p = m.point(i, j);
                // Below the stock's bottom there is nothing to hit.
                let lo = self.floor_at(i, j);
                let Some(z) = lowest_bottom(tool, p, a, b, a2, b2, r).map(|z| z.max(lo)) else { continue };
                if let Some(cur) = m.get(i, j)
                    && cur.is_finite()
                    && cur > lo + tol
                    && z < cur - tol
                {
                    return true;
                }
            }
        }
        false
    }
}

/// The lowest height of the tool's bottom over the cell at `p` as the tip travels from `a` to
/// `b`; `None` when the tool never covers `p`. Over the stretch of the move where the cell is
/// under the tool, that height is the tip's height plus the tool's profile at the cell's
/// distance from the axis: for a level move it is lowest where the axis passes closest, but on
/// a sloped one a ball reaches lower a little downhill of that point (by up to slope² × r / 2),
/// and taking the closest point there leaves material the cut removes. Every tool's profile
/// rises convexly from its axis and the distance to a line is convex along it, so the height is
/// convex along the move: exact for flat and ball ends, a golden-section search for the rest.
fn lowest_bottom(tool: &Tool, p: Vec2, a: Vec3, b: Vec3, a2: Vec2, b2: Vec2, r: f64) -> Option<f64> {
    let u = b2 - a2;
    let w = p - a2;
    let uu = u.dot(u);
    let dz = b.z - a.z;
    let height = |t: f64| a.z + dz * t + tool.profile((w - u * t).len().min(r));
    if uu < 1e-18 {
        // A plunge (or no move across): lowest at the lower end.
        return (w.len() <= r).then(|| height(if dz < 0.0 { 1.0 } else { 0.0 }));
    }
    // The stretch where the cell is under the tool: |w - t u| <= r.
    let half_b = w.dot(u);
    let disc = half_b * half_b - uu * (w.dot(w) - r * r);
    if disc < 0.0 {
        return None;
    }
    let root = disc.sqrt();
    let (lo, hi) = (((half_b - root) / uu).max(0.0), ((half_b + root) / uu).min(1.0));
    if lo > hi {
        return None;
    }
    let closest = (half_b / uu).clamp(lo, hi);
    if dz.abs() < 1e-12 {
        return Some(height(closest));
    }
    // A convex height's lowest point on the stretch is its lowest point anywhere, clamped to the
    // stretch. A flat end is level under its whole face: lowest at the downhill end. A ball is
    // lowest where its sphere touches the cylinder it sweeps: m·√((r² − y²)/(1 + m²)) downhill
    // of the closest point (m the move's slope, y the cell's distance across it).
    match tool.kind {
        crate::tool::ToolKind::FlatEnd => return Some(height(if dz < 0.0 { hi } else { lo })),
        crate::tool::ToolKind::BallEnd => {
            let len = uu.sqrt();
            let slope = dz / len;
            let along = half_b / len;
            let across2 = (w.dot(w) - along * along).max(0.0);
            let shift = slope * ((r * r - across2).max(0.0) / (1.0 + slope * slope)).sqrt();
            return Some(height(((along - shift) / len).clamp(lo, hi)));
        }
        _ => {}
    }
    let (mut x0, mut x1) = (lo, hi);
    let g = 0.5 * (5f64.sqrt() - 1.0);
    let (mut c, mut d) = (x1 - g * (x1 - x0), x0 + g * (x1 - x0));
    let (mut fc, mut fd) = (height(c), height(d));
    for _ in 0..40 {
        if (x1 - x0) * uu.sqrt() < 1e-5 {
            break;
        }
        if fc < fd {
            x1 = d;
            (d, fd) = (c, fc);
            c = x1 - g * (x1 - x0);
            fc = height(c);
        } else {
            x0 = c;
            (c, fc) = (d, fd);
            d = x0 + g * (x1 - x0);
            fd = height(d);
        }
    }
    Some([height(lo), height(hi), height(closest), fc, fd].into_iter().fold(f64::INFINITY, f64::min))
}

/// What the simulation found.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SimulationReport {
    pub removed_volume_mm3: f64,
    pub remaining_volume_mm3: f64,
    /// Material left above the model (within the stock), mm³.
    pub rest_volume_mm3: f64,
    pub rest_max_mm: f64,
    /// The same over the part's own outline only (columns where the model stands above the
    /// floor): what the cuts left on the part, without the frame of stock around it.
    pub rest_on_part_mm3: f64,
    pub rest_on_part_max_mm: f64,
    /// The part's area left more than [`UNCUT_MM`] high in the columns this run cut (a cut that
    /// stopped short of the surface), counting a column only when its four
    /// neighbours are too (a column astride a wall reads the wall's height), and the highest
    /// such column (x, y, how high).
    pub uncut_area_mm2: f64,
    pub uncut_at: Option<Vec3>,
    /// Cells cut below the model's surface by more than the tolerance.
    pub gouge_cells: usize,
    pub gouge_max_mm: f64,
    /// Where the deepest gouge is (x, y of the cell, the model's height there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gouge_at: Option<Vec3>,
    pub rapid_collisions: usize,
    pub shank_collisions: usize,
    pub holder_collisions: usize,
    pub fixture_collisions: usize,
    /// Moves simulated as 3-axis sweeps, and moves only collision-checked (tilted axis).
    pub swept_moves: usize,
    pub checked_moves: usize,
}

/// Material left on the part higher than this (mm) counts as uncut, not as finishing stock.
pub const UNCUT_MM: f64 = 1.0;
/// Uncut area (mm²) worth a warning: less is a corner the tool's radius cannot reach.
pub const UNCUT_WARN_MM2: f64 = 4.0;

/// The model's own Z-map (its top surface) over the stock map's grid, for gouge and rest checks.
pub fn model_map(model: &DropMesh, grid: &HeightMap, floor: f64) -> HeightMap {
    let probe = Tool::new(0, "probe", crate::tool::ToolKind::FlatEnd, 1e-3);
    let mut out = HeightMap { origin: grid.origin, cell: grid.cell, nx: grid.nx, ny: grid.ny, z: vec![floor; grid.z.len()] };
    for j in 0..grid.ny {
        for i in 0..grid.nx {
            let p = grid.point(i, j);
            let z = model.drop_z(&probe, p, floor).map_or(floor, |c| c.z);
            out.set(i, j, z);
        }
    }
    out
}

/// The stock map's columns cut below the model's surface (gouges): each column and how deep
/// (mm). On a near-vertical wall the model's height is ambiguous within a cell, so a cell is a
/// gouge only when its neighbours agree it lies on the surface.
pub fn gouges(stock: &StockMap, model: &HeightMap, tol: f64) -> Vec<(usize, usize, f64)> {
    let nx = stock.map.nx;
    let mut out = Vec::new();
    for (k, (s, mz)) in stock.map.z.iter().zip(&model.z).enumerate() {
        if !s.is_finite() {
            continue;
        }
        let d = s - mz;
        let (i, j) = (k % nx, k / nx);
        if d < -tol && *mz > stock.floor_at(i, j) + tol {
            let wall = [(i.wrapping_sub(1), j), (i + 1, j), (i, j.wrapping_sub(1)), (i, j + 1)]
                .iter()
                .filter_map(|(a, b)| model.get(*a, *b))
                .any(|n| (mz - n) > stock.map.cell * 2.0);
            if !wall {
                out.push((i, j, -d));
            }
        }
    }
    out
}

/// The gouges as a mesh to draw over the stock: for each gouged column, its cell on the cut
/// surface (lifted a little to show above it) and a wall up to where the model's surface is.
pub fn gouge_mesh(stock: &StockMap, model: &HeightMap, tol: f64) -> Mesh {
    let mut mesh = Mesh::default();
    let h = stock.map.cell * 0.5;
    // Far enough above the cut surface that the stock drawn under it never hides it.
    let lift = (stock.map.cell * 0.5).max(0.05);
    // Input is hostile: a wildly wrong program draws at most this many columns.
    for (i, j, depth) in gouges(stock, model, tol).into_iter().take(400_000) {
        let p = stock.map.point(i, j);
        let Some(top) = stock.map.get(i, j) else { continue };
        let z = top + lift;
        let base = mesh.positions.len() as u32;
        for (dx, dy) in [(-h, -h), (h, -h), (h, h), (-h, h)] {
            mesh.positions.push(Vec3::new(p.x + dx, p.y + dy, z));
            mesh.normals.push(Vec3::new(0.0, 0.0, 1.0));
        }
        mesh.triangles.push([base, base + 1, base + 2]);
        mesh.triangles.push([base, base + 2, base + 3]);
        mesh.tri_face.extend([0, 0]);
        // A post up to the model's surface, so a deep gouge shows from the side as well.
        if depth > stock.map.cell {
            let b = mesh.positions.len() as u32;
            let w = h * 0.4;
            for (q, n) in [(Vec3::new(p.x - w, p.y, z), Vec3::new(0.0, -1.0, 0.0)), (Vec3::new(p.x + w, p.y, z), Vec3::new(0.0, -1.0, 0.0))] {
                mesh.positions.push(q);
                mesh.normals.push(n);
            }
            mesh.positions.push(Vec3::new(p.x + w, p.y, top + depth));
            mesh.normals.push(Vec3::new(0.0, -1.0, 0.0));
            mesh.positions.push(Vec3::new(p.x - w, p.y, top + depth));
            mesh.normals.push(Vec3::new(0.0, -1.0, 0.0));
            mesh.triangles.push([b, b + 1, b + 2]);
            mesh.triangles.push([b, b + 2, b + 3]);
            mesh.tri_face.extend([1, 1]);
        }
    }
    mesh
}

/// Simulates a toolpath against the stock map. Moves with the tool axis along z are swept;
/// tilted moves are checked for collisions of the shank and holder against the model and
/// fixtures. `model` is the model's Z-map on the stock grid (for gouges), when available.
#[allow(clippy::too_many_arguments)]
pub fn simulate(
    stock: &mut StockMap,
    tool: &Tool,
    tp: &Toolpath,
    start: Option<Vec3>,
    model: Option<&HeightMap>,
    mesh: Option<&DropMesh>,
    fixtures: Option<&DropMesh>,
    tol: f64,
) -> Result<SimulationReport> {
    crate::positive("tolerance", tol)?;
    let before = stock.volume();
    // The columns this run cuts: a surface is uncut only where the run's tools came down.
    let start_z = stock.map.z.clone();
    let mut rep = SimulationReport::default();
    if tp.laser {
        // A laser marks or cuts through; the Z-map stock does not model either.
        rep.remaining_volume_mm3 = before;
        return Ok(rep);
    }
    let z = Vec3::new(0.0, 0.0, 1.0);
    let mut prev = start;
    for m in &tp.moves {
        let vertical = (m.axis - z).len() < 1e-6;
        if let Some(p) = prev {
            if vertical && (p - m.to).len() > 0.0 {
                match m.kind {
                    MoveKind::Rapid => {
                        if stock.rapid_hits(tool, p, m.to, tol) {
                            rep.rapid_collisions += 1;
                        }
                    }
                    _ => {
                        stock.cut(tool, p, m.to);
                        rep.swept_moves += 1;
                    }
                }
            } else if !vertical {
                rep.checked_moves += 1;
                if let Some(mesh) = mesh {
                    let shank_r = tool.radius_at(tool.flute_length + 0.01);
                    if mesh.cylinder_hits(m.to, m.axis, shank_r, tool.flute_length, tool.overall_length, 0.0) {
                        rep.shank_collisions += 1;
                    }
                    if tool.holder_diameter > 0.0
                        && mesh.cylinder_hits(m.to, m.axis, tool.holder_diameter * 0.5, tool.overall_length, tool.overall_length + 50.0, 0.0)
                    {
                        rep.holder_collisions += 1;
                    }
                }
            }
        }
        if let Some(fx) = fixtures
            && fx.cylinder_hits(m.to, m.axis, tool.radius().max(tool.radius_at(tool.flute_length + 0.01)), 0.0, tool.overall_length, 0.0)
        {
            rep.fixture_collisions += 1;
        }
        prev = Some(m.to);
    }
    let after = stock.volume();
    rep.removed_volume_mm3 = (before - after).max(0.0);
    rep.remaining_volume_mm3 = after;
    if let Some(model) = model {
        let mut rest = 0.0;
        let mut rest_max = 0.0f64;
        let mut gouge_max = 0.0f64;
        // Columns off the part carry the model map's floor, its lowest value.
        let floor = model.z.iter().copied().filter(|z| z.is_finite()).fold(f64::INFINITY, f64::min);
        let (mut on_part, mut on_part_max) = (0.0, 0.0f64);
        for (s, mz) in stock.map.z.iter().zip(&model.z) {
            let d = s - mz;
            if s.is_finite() && d > tol {
                rest += d;
                rest_max = rest_max.max(d);
                if *mz > floor + tol {
                    on_part += d;
                    on_part_max = on_part_max.max(d);
                }
            }
        }
        rep.rest_on_part_mm3 = on_part * stock.map.cell * stock.map.cell;
        rep.rest_on_part_max_mm = on_part_max;
        (rep.uncut_area_mm2, rep.uncut_at) = uncut(stock, &start_z, model, tol);
        for (i, j, depth) in gouges(stock, model, tol) {
            rep.gouge_cells += 1;
            if depth > gouge_max {
                gouge_max = depth;
                let p = stock.map.point(i, j);
                rep.gouge_at = Some(Vec3::new(p.x, p.y, model.get(i, j).unwrap_or(0.0)));
            }
        }
        rep.rest_volume_mm3 = rest * stock.map.cell * stock.map.cell;
        rep.rest_max_mm = rest_max;
        rep.gouge_max_mm = gouge_max;
    }
    Ok(rep)
}

/// The part's area left more than [`UNCUT_MM`] above `model` in the columns cut since `start`
/// (the stock's heights before): a cut that stopped short of a surface facing the tool. A
/// column counts only when its four neighbours do too (a column astride a wall reads the
/// wall's height). Returns the area (mm²) and the highest column (x, y, how high).
pub fn uncut(stock: &StockMap, start: &[f64], model: &HeightMap, tol: f64) -> (f64, Option<Vec3>) {
    // Columns off the part carry the model map's floor, its lowest value.
    let floor = model.z.iter().copied().filter(|z| z.is_finite()).fold(f64::INFINITY, f64::min);
    let (nx, ny) = (stock.map.nx, stock.map.ny);
    let high = |i: usize, j: usize| {
        let k = j * nx + i;
        let (s, mz) = (stock.map.z.get(k).copied()?, model.z.get(k).copied()?);
        let cut = start.get(k).is_some_and(|z0| s < z0 - tol);
        (cut && s.is_finite() && mz > floor + tol && s - mz > UNCUT_MM).then_some(s - mz)
    };
    let mut cells = 0usize;
    let mut at = None;
    let mut top = 0.0f64;
    for j in 1..ny.saturating_sub(1) {
        for i in 1..nx.saturating_sub(1) {
            let Some(d) = high(i, j) else { continue };
            if [(i - 1, j), (i + 1, j), (i, j - 1), (i, j + 1)].iter().all(|&(a, b)| high(a, b).is_some()) {
                cells += 1;
                if d > top {
                    top = d;
                    let p = stock.map.point(i, j);
                    at = Some(Vec3::new(p.x, p.y, d));
                }
            }
        }
    }
    (cells as f64 * stock.map.cell * stock.map.cell, at)
}

impl SimulationReport {
    pub fn notice(&self) -> Notice {
        let mut n = Notice::new(format!(
            "removed {:.0} mm³, {:.0} mm³ left above the model, {} gouged cells",
            self.removed_volume_mm3, self.rest_volume_mm3, self.gouge_cells
        ));
        n.value("removed_volume_mm3", r3(self.removed_volume_mm3));
        n.value("remaining_volume_mm3", r3(self.remaining_volume_mm3));
        n.value("rest_volume_mm3", r3(self.rest_volume_mm3));
        n.value("rest_max_mm", r3(self.rest_max_mm));
        n.value("rest_on_part_mm3", r3(self.rest_on_part_mm3));
        n.value("rest_on_part_max_mm", r3(self.rest_on_part_max_mm));
        n.value("uncut_area_mm2", r3(self.uncut_area_mm2));
        if let Some(p) = self.uncut_at {
            n.value("uncut_at", serde_json::json!([r3(p.x), r3(p.y), r3(p.z)]));
        }
        n.value("gouge_cells", self.gouge_cells);
        n.value("gouge_max_mm", r3(self.gouge_max_mm));
        n.value("rapid_collisions", self.rapid_collisions);
        n.value("shank_collisions", self.shank_collisions);
        n.value("holder_collisions", self.holder_collisions);
        n.value("fixture_collisions", self.fixture_collisions);
        n.value("swept_moves", self.swept_moves);
        n.value("checked_moves", self.checked_moves);
        if self.gouge_cells > 0 {
            let at = self.gouge_at.map_or(String::new(), |p| format!(" (deepest at x {:.2}, y {:.2})", p.x, p.y));
            n.issue(
                Issue::error("gouge", format!("{} cells are cut up to {:.3} mm below the model{at}", self.gouge_cells, self.gouge_max_mm))
                    .limit(r3(self.gouge_max_mm))
                    .suggest(serde_json::json!({"gouge_at": self.gouge_at})),
            );
        }
        if let Some(p) = self.uncut_at.filter(|_| self.uncut_area_mm2 > UNCUT_WARN_MM2) {
            n.issue(
                Issue::warning(
                    "uncut",
                    format!(
                        "{:.0} mm² of the part's surface facing this setup is left over {UNCUT_MM} mm high, up to {:.1} mm at x {:.2}, y {:.2}: \
                         the operations stop above it (their bottom height) or no tool reaches it; it stays unless a setup from another side reaches it",
                        self.uncut_area_mm2, p.z, p.x, p.y
                    ),
                )
                .limit(r3(p.z))
                .suggest(serde_json::json!({"uncut_at": [r3(p.x), r3(p.y)]})),
            );
        }
        if self.rapid_collisions > 0 {
            n.issue(
                Issue::error(
                    "rapid_collision",
                    format!("{} rapid moves pass through stock; raise the retract or clearance height", self.rapid_collisions),
                )
                .parameter("heights.retract", serde_json::Value::Null),
            );
        }
        if self.shank_collisions > 0 {
            n.issue(Issue::error("shank_collision", format!("the tool shank meets the model on {} moves", self.shank_collisions)));
        }
        if self.holder_collisions > 0 {
            n.issue(Issue::error("holder_collision", format!("the holder meets the model on {} moves", self.holder_collisions)));
        }
        if self.fixture_collisions > 0 {
            n.issue(Issue::error("fixture_collision", format!("the tool meets a fixture on {} moves", self.fixture_collisions)));
        }
        n
    }
}

/// The stock map's cell size for a setup: the tolerance scaled so the map stays under the cap.
pub fn stock_cell(shape: &StockShape, tolerance: f64) -> Result<f64> {
    let b = shape.bounds();
    let size = b.size();
    if !(size.x > 0.0 && size.y > 0.0) {
        return Err(CamError::Geometry("stock has no area".into()));
    }
    let mut cell = (tolerance * 5.0).max(0.05);
    let max_cells = 2_500_000.0;
    while (size.x / cell + 3.0) * (size.y / cell + 3.0) > max_cells {
        cell *= 1.5;
    }
    Ok(cell)
}

/// A program to simulate: operations in order, each with its tool and toolpath (WCS).
pub struct SimOperation {
    pub id: u64,
    pub name: String,
    pub tool: Tool,
    pub toolpath: std::sync::Arc<Toolpath>,
}

/// Where the simulation stands: operation index, moves of it done, and the fraction of the
/// current move.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cursor {
    pub operation: usize,
    pub move_index: usize,
    pub fraction: f64,
}

/// A stepping simulator: the stock as it is after every move up to a cursor, with the tool
/// where the cursor is. Seeking forward sweeps the moves in between; seeking backwards starts
/// from the checkpoint taken at the start of the operation.
pub struct Simulator {
    pub shape: StockShape,
    pub operations: Vec<SimOperation>,
    pub stock: StockMap,
    /// The stock at the start of each operation (filled as the simulation passes them).
    checkpoints: Vec<Option<StockMap>>,
    pub cursor: Cursor,
    /// The tool tip and axis at the cursor.
    pub tip: Vec3,
    pub axis: Vec3,
    pub total_moves: usize,
    /// Running time in seconds at the cursor.
    pub time_s: f64,
    pub total_time_s: f64,
    /// The program's one clock: for each operation, the time at which each of its moves ends
    /// (the first move from where the previous operation ended), cumulative over the program.
    /// Seeking by time, the time at the cursor and the total all read it, so they agree.
    ends: Vec<Vec<f64>>,
    /// The time each operation starts.
    starts: Vec<f64>,
}

impl Simulator {
    pub fn new(shape: StockShape, operations: Vec<SimOperation>, cell: f64, rapid_feed: f64) -> Result<Simulator> {
        let stock = StockMap::new(&shape, cell)?;
        Simulator::with_stock(shape, stock, operations, rapid_feed)
    }

    /// A simulator starting from `stock` (the material an earlier setup left).
    pub fn with_stock(shape: StockShape, stock: StockMap, operations: Vec<SimOperation>, rapid_feed: f64) -> Result<Simulator> {
        let n = operations.len();
        let total_moves = operations.iter().map(|o| o.toolpath.len()).sum();
        let feed_of = |m: &crate::toolpath::Move| if m.feed > 0.0 { m.feed } else { rapid_feed.max(1e-9) };
        let mut ends = Vec::with_capacity(n);
        let mut starts = Vec::with_capacity(n);
        let mut clock = 0.0;
        let mut last: Option<Vec3> = None;
        for op in &operations {
            starts.push(clock);
            let mut e = Vec::with_capacity(op.toolpath.len());
            for m in &op.toolpath.moves {
                let from = last.unwrap_or(m.to);
                clock += from.dist(m.to) / feed_of(m) * 60.0;
                e.push(clock);
                last = Some(m.to);
            }
            ends.push(e);
        }
        let total_time_s = clock;
        let first = operations.first().and_then(|o| o.toolpath.moves.first()).map(|m| (m.to, m.axis));
        let (tip, axis) = first.unwrap_or((Vec3::new(0.0, 0.0, shape.bounds().max.z + 10.0), Vec3::new(0.0, 0.0, 1.0)));
        let mut checkpoints = vec![None; n];
        if let Some(c) = checkpoints.first_mut() {
            *c = Some(stock.clone());
        }
        Ok(Simulator {
            shape,
            operations,
            stock,
            checkpoints,
            cursor: Cursor::default(),
            tip,
            axis,
            total_moves,
            time_s: 0.0,
            total_time_s,
            ends,
            starts,
        })
    }

    /// The time at a cursor, by the program's clock.
    pub fn time_at(&self, c: Cursor) -> f64 {
        let Some(e) = self.ends.get(c.operation) else { return self.total_time_s };
        let Some(&end) = e.get(c.move_index) else { return self.starts.get(c.operation).copied().unwrap_or(0.0) };
        let start = if c.move_index == 0 { self.starts.get(c.operation).copied().unwrap_or(0.0) } else { e[c.move_index - 1] };
        start + (end - start) * c.fraction.clamp(0.0, 1.0)
    }

    /// The cursor for a fraction `t` in 0..1 of the whole program's time.
    pub fn cursor_at_fraction(&self, t: f64) -> Cursor {
        let want = self.total_time_s * if t.is_finite() { t.clamp(0.0, 1.0) } else { 1.0 };
        for (oi, e) in self.ends.iter().enumerate() {
            // The first move that ends at or after the wanted time.
            let Some(&last) = e.last() else { continue };
            if last < want {
                continue;
            }
            let mi = e.partition_point(|end| *end < want);
            let start = if mi == 0 { self.starts.get(oi).copied().unwrap_or(0.0) } else { e[mi - 1] };
            let end = e[mi];
            let f = if end > start { ((want - start) / (end - start)).clamp(0.0, 1.0) } else { 1.0 };
            return Cursor { operation: oi, move_index: mi, fraction: f };
        }
        let last = self.operations.len().saturating_sub(1);
        Cursor { operation: last, move_index: self.operations.get(last).map_or(0, |o| o.toolpath.len().saturating_sub(1)), fraction: 1.0 }
    }

    /// Sweeps the stock to `target` and places the tool there.
    pub fn seek(&mut self, target: Cursor) -> Result<()> {
        let n = self.operations.len();
        if n == 0 {
            return Ok(());
        }
        let mut target = target;
        target.operation = target.operation.min(n - 1);
        let op_len = self.operations[target.operation].toolpath.len();
        target.move_index = target.move_index.min(op_len.saturating_sub(1));
        target.fraction = if target.fraction.is_finite() { target.fraction.clamp(0.0, 1.0) } else { 1.0 };
        let backwards = target.operation < self.cursor.operation
            || (target.operation == self.cursor.operation && (target.move_index, target.fraction) < (self.cursor.move_index, self.cursor.fraction));
        if backwards {
            // Restart from the nearest checkpoint at or before the target operation.
            let mut from = target.operation;
            while from > 0 && self.checkpoints[from].is_none() {
                from -= 1;
            }
            match self.checkpoints[from].clone() {
                Some(st) => self.stock = st,
                None => self.stock = StockMap::new(&self.shape, self.stock.map.cell)?,
            }
            self.cursor = Cursor { operation: from, move_index: 0, fraction: 0.0 };
        }
        // Sweep forward.
        let mut guard = 0usize;
        while self.cursor.operation < target.operation
            || (self.cursor.operation == target.operation && (self.cursor.move_index, self.cursor.fraction) < (target.move_index, target.fraction))
        {
            guard += 1;
            if guard > self.total_moves + n + 8 {
                break;
            }
            let oi = self.cursor.operation;
            let op = &self.operations[oi];
            if self.cursor.move_index == 0 && self.cursor.fraction == 0.0 && self.checkpoints[oi].is_none() {
                self.checkpoints[oi] = Some(self.stock.clone());
            }
            if op.toolpath.is_empty() || self.cursor.move_index >= op.toolpath.len() {
                self.cursor = Cursor { operation: oi + 1, move_index: 0, fraction: 0.0 };
                if self.cursor.operation >= n {
                    self.cursor = Cursor { operation: n - 1, move_index: op.toolpath.len().saturating_sub(1), fraction: 1.0 };
                    break;
                }
                continue;
            }
            let mi = self.cursor.move_index;
            let m = op.toolpath.moves[mi];
            let from_pt = if mi == 0 { self.previous_end(oi).unwrap_or(m.to) } else { op.toolpath.moves[mi - 1].to };
            let at_target_move = oi == target.operation && mi == target.move_index;
            let f1 = if at_target_move { target.fraction } else { 1.0 };
            let f0 = self.cursor.fraction;
            if f1 > f0 {
                let a = from_pt.lerp(m.to, f0);
                let b = from_pt.lerp(m.to, f1);
                let vertical = (m.axis - Vec3::new(0.0, 0.0, 1.0)).len() < 1e-6;
                if vertical && !op.toolpath.laser && m.kind != MoveKind::Rapid && !(m.kind == MoveKind::Retract && m.feed <= 0.0) {
                    self.stock.cut(&op.tool, a, b);
                }
                self.tip = b;
                self.axis = m.axis;
            }
            if at_target_move {
                self.cursor = target;
                break;
            }
            self.cursor = Cursor { operation: oi, move_index: mi + 1, fraction: 0.0 };
            if self.cursor.move_index >= op.toolpath.len() {
                if oi + 1 < n {
                    self.cursor = Cursor { operation: oi + 1, move_index: 0, fraction: 0.0 };
                } else {
                    self.cursor = Cursor { operation: oi, move_index: op.toolpath.len() - 1, fraction: 1.0 };
                    break;
                }
            }
        }
        self.time_s = self.time_at(self.cursor);
        Ok(())
    }

    fn previous_end(&self, oi: usize) -> Option<Vec3> {
        (0..oi).rev().find_map(|k| self.operations[k].toolpath.last_position())
    }

    /// The fraction of the program done at the cursor (by time).
    pub fn progress(&self) -> f64 {
        if self.total_time_s > 0.0 { (self.time_s / self.total_time_s).clamp(0.0, 1.0) } else { 1.0 }
    }

    /// The stock as a closed triangle mesh: the height field on top, skirts down to the bottom
    /// around the outline and a flat bottom. `step` cells per vertex (1 for full detail).
    pub fn stock_mesh(&self, step: usize) -> Mesh {
        stock_mesh(&self.stock, step.max(1))
    }

    /// The tool at the cursor as a mesh: the cutting part by its profile, the shank above it.
    pub fn tool_mesh(&self) -> Mesh {
        match self.operations.get(self.cursor.operation) {
            Some(op) => tool_mesh(&op.tool, self.tip, self.axis),
            None => Mesh::default(),
        }
    }
}

/// The stock map as a closed mesh (WCS). Cells outside the stock are skipped; a skirt is built
/// where a kept cell meets a skipped one or the map edge.
pub fn stock_mesh(stock: &StockMap, step: usize) -> Mesh {
    let m = &stock.map;
    let step = step.max(1);
    let nx = m.nx.div_ceil(step);
    let ny = m.ny.div_ceil(step);
    let cell_of = |i: usize, j: usize| ((i * step).min(m.nx.saturating_sub(1)), (j * step).min(m.ny.saturating_sub(1)));
    // The bottom under a vertex; a column cut through (its top at its own floor) is not drawn.
    let fl = |i: usize, j: usize| -> f64 {
        let (ii, jj) = cell_of(i, j);
        stock.floor_at(ii, jj)
    };
    let at = |i: usize, j: usize| -> Option<f64> {
        let (ii, jj) = cell_of(i, j);
        m.get(ii, jj).filter(|z| z.is_finite()).filter(|z| stock.floor.is_none() || *z > stock.floor_at(ii, jj) + 1e-9)
    };
    let pt = |i: usize, j: usize, z: f64| -> Vec3 {
        let p = m.point((i * step).min(m.nx.saturating_sub(1)), (j * step).min(m.ny.saturating_sub(1)));
        Vec3::new(p.x, p.y, z)
    };
    let mut mesh = Mesh::default();
    let mut index: Vec<Option<u32>> = vec![None; nx * ny];
    // Top vertices.
    for j in 0..ny {
        for i in 0..nx {
            if let Some(z) = at(i, j) {
                index[j * nx + i] = Some(mesh.positions.len() as u32);
                mesh.positions.push(pt(i, j, z));
                mesh.normals.push(Vec3::new(0.0, 0.0, 1.0));
            }
        }
    }
    let bottom = stock.bottom;
    // The floor under each top vertex.
    let floors: Vec<f64> = (0..mesh.positions.len()).map(|_| bottom).collect();
    let mut floors = floors;
    for j in 0..ny {
        for i in 0..nx {
            if let Some(k) = index[j * nx + i]
                && let Some(f) = floors.get_mut(k as usize)
            {
                *f = fl(i, j);
            }
        }
    }
    // Where each top vertex sits on the grid, and the outline's outward direction there: the
    // kept cells, blurred over a few cells, fall away across the outline. On a curved wall the
    // grid makes a staircase of x- and y-facing skirts; shading them by the blurred outline
    // instead of their own edges draws the wall smooth.
    let mut grid_of: Vec<(usize, usize)> = vec![(0, 0); mesh.positions.len()];
    for j in 0..ny {
        for i in 0..nx {
            if let Some(k) = index[j * nx + i]
                && let Some(g) = grid_of.get_mut(k as usize)
            {
                *g = (i, j);
            }
        }
    }
    let kept: Vec<f64> = index.iter().map(|k| if k.is_some() { 1.0 } else { 0.0 }).collect();
    let blurred = blur(&kept, nx, ny, SIGMA);
    let outward = |k: u32| -> Option<Vec3> {
        let &(i, j) = grid_of.get(k as usize)?;
        let b = |di: isize, dj: isize| -> f64 {
            let (ii, jj) = (i as isize + di, j as isize + dj);
            if ii < 0 || jj < 0 || ii >= nx as isize || jj >= ny as isize { 0.0 } else { blurred[jj as usize * nx + ii as usize] }
        };
        Vec3::new(b(-1, 0) - b(1, 0), b(0, -1) - b(0, 1), 0.0).normalized()
    };
    // Top triangles, counter-clockwise from above: two for a cell with all four corners kept,
    // one for a cell with three, so a curved outline steps across the diagonal rather than
    // round it.
    let mut top: Vec<[u32; 3]> = Vec::new();
    for j in 0..ny.saturating_sub(1) {
        for i in 0..nx.saturating_sub(1) {
            let corners = [index[j * nx + i], index[j * nx + i + 1], index[(j + 1) * nx + i + 1], index[(j + 1) * nx + i]];
            match corners {
                [Some(a), Some(b), Some(c), Some(d)] => {
                    top.push([a, b, c]);
                    top.push([a, c, d]);
                }
                _ => {
                    let kept: Vec<u32> = corners.iter().flatten().copied().collect();
                    if let [a, b, c] = kept[..] {
                        top.push([a, b, c]);
                    }
                }
            }
        }
    }
    for t in &top {
        mesh.triangles.push(*t);
        mesh.tri_face.push(0);
    }
    // Skirts: down to the floor along every edge of the top used by one triangle only (the
    // outline, and the rims of holes cut through). The top lies on such an edge's left.
    let inner: std::collections::HashSet<(u32, u32)> = top.iter().flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])]).collect();
    let add_skirt = |mesh: &mut Mesh, a: u32, b: u32| {
        let pa = mesh.positions[a as usize];
        let pb = mesh.positions[b as usize];
        let (fa, fb) = (floors.get(a as usize).copied().unwrap_or(bottom), floors.get(b as usize).copied().unwrap_or(bottom));
        let base = mesh.positions.len() as u32;
        let n = (pb - pa).cross(Vec3::new(0.0, 0.0, 1.0)).normalized().unwrap_or(Vec3::new(1.0, 0.0, 0.0));
        // The smoothed direction, where it faces the same side as the edge.
        let smooth = |k: u32| outward(k).filter(|o| o.dot(n) > 0.0).unwrap_or(n);
        let (na, nb) = (smooth(a), smooth(b));
        for (p, nn) in [(pa, na), (pb, nb), (Vec3::new(pb.x, pb.y, fb), nb), (Vec3::new(pa.x, pa.y, fa), na)] {
            mesh.positions.push(p);
            mesh.normals.push(nn);
        }
        mesh.triangles.push([base, base + 2, base + 1]);
        mesh.tri_face.push(1);
        mesh.triangles.push([base, base + 3, base + 2]);
        mesh.tri_face.push(1);
    };
    for t in &top {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            if !inner.contains(&(b, a)) {
                add_skirt(&mut mesh, a, b);
            }
        }
    }
    // Bottom: the top's triangles at the floor under each vertex, facing down.
    let h = m.cell * step as f64;
    let floor_wide = stock.floor.as_ref().map(|_| blur(&(0..nx * ny).map(|k| fl(k % nx, k / nx)).collect::<Vec<_>>(), nx, ny, SIGMA));
    let base = mesh.positions.len() as u32;
    for (k, &(i, j)) in grid_of.iter().enumerate() {
        mesh.positions.push(pt(i, j, floors.get(k).copied().unwrap_or(bottom)));
        // A floor of its own (stock carried over from the other side) has steps: shade them from
        // the floor's slope, as the top is shaded from its own.
        let n = match &floor_wide {
            Some(wide) => {
                let f = |di: isize, dj: isize| -> f64 {
                    let (ii, jj) = (i as isize + di, j as isize + dj);
                    if ii < 0 || jj < 0 || ii >= nx as isize || jj >= ny as isize || index[jj as usize * nx + ii as usize].is_none() {
                        return fl(i, j);
                    }
                    fl(ii as usize, jj as usize)
                };
                let (gx, gy) = grid_slope(&f, wide, i, j, nx, ny, h);
                Vec3::new(gx, gy, -1.0).normalized().unwrap_or(Vec3::new(0.0, 0.0, -1.0))
            }
            None => Vec3::new(0.0, 0.0, -1.0),
        };
        mesh.normals.push(n);
    }
    for t in &top {
        mesh.triangles.push([base + t[0], base + t[2], base + t[1]]);
        mesh.tri_face.push(2);
    }
    // Smooth top normals from the height field (`grid_slope`).
    let heights: Vec<f64> = (0..nx * ny).map(|k| at(k % nx, k / nx).unwrap_or(bottom)).collect();
    let wide = blur(&heights, nx, ny, SIGMA);
    for j in 0..ny {
        for i in 0..nx {
            let Some(k) = index[j * nx + i] else { continue };
            let z = |di: isize, dj: isize| -> f64 {
                let (ii, jj) = (i as isize + di, j as isize + dj);
                if ii < 0 || jj < 0 || ii >= nx as isize || jj >= ny as isize {
                    return at(i, j).unwrap_or(bottom);
                }
                at(ii as usize, jj as usize).unwrap_or_else(|| at(i, j).unwrap_or(bottom))
            };
            let (gx, gy) = grid_slope(&z, &wide, i, j, nx, ny, h);
            let n = Vec3::new(-gx, -gy, 1.0).normalized().unwrap_or(Vec3::new(0.0, 0.0, 1.0));
            if let Some(nn) = mesh.normals.get_mut(k as usize) {
                *nn = n;
            }
        }
    }
    mesh
}

/// The slope (dz/dx, dz/dy) of a height field on the grid at vertex (`i`, `j`), `z` giving
/// the heights round it and `wide` the field smoothed along the wall (`blur`). A 3 x 3
/// (Sobel) slope, so the grid's steps on a sloped floor average out instead of shading as
/// stripes; where it is steep (a wall standing between neighbouring vertices) the staircase
/// along a curved wall turns the slope between x and y from one vertex to the next, so the wall
/// faces the way the smoothed field falls instead.
fn grid_slope(z: &dyn Fn(isize, isize) -> f64, wide: &[f64], i: usize, j: usize, nx: usize, ny: usize, h: f64) -> (f64, f64) {
    let gx = (z(1, -1) + 2.0 * z(1, 0) + z(1, 1) - z(-1, -1) - 2.0 * z(-1, 0) - z(-1, 1)) / (8.0 * h);
    let gy = (z(-1, 1) + 2.0 * z(0, 1) + z(1, 1) - z(-1, -1) - 2.0 * z(0, -1) - z(1, -1)) / (8.0 * h);
    let slope = (gx * gx + gy * gy).sqrt();
    if slope <= 1.0 {
        return (gx, gy);
    }
    let w = |di: isize, dj: isize| -> f64 {
        let (ii, jj) = ((i as isize + di).clamp(0, nx as isize - 1), (j as isize + dj).clamp(0, ny as isize - 1));
        wide.get(jj as usize * nx + ii as usize).copied().unwrap_or(0.0)
    };
    let (wx, wy) = (w(1, 0) - w(-1, 0), w(0, 1) - w(0, -1));
    let len = (wx * wx + wy * wy).sqrt();
    if len > 1e-12 { (wx / len * slope, wy / len * slope) } else { (gx, gy) }
}

/// How widely (cells) a wall's facing is averaged along a curved staircase: a narrower
/// Gaussian lets the direction ripple from step to step (7° at 1.5 cells, 3° at 3).
const SIGMA: f64 = 3.0;

/// `field` (`nx` by `ny`) smoothed by a Gaussian of `sigma` cells, the map's edge clamped. A
/// Gaussian weighs every direction alike, so the slope of the smoothed field follows a curved
/// staircase's true direction where a square window would bend it towards the grid's axes.
fn blur(field: &[f64], nx: usize, ny: usize, sigma: f64) -> Vec<f64> {
    let reach = (sigma * 3.0).ceil() as isize;
    let weights: Vec<f64> = (-reach..=reach).map(|k| (-((k * k) as f64) / (2.0 * sigma * sigma)).exp()).collect();
    let total: f64 = weights.iter().sum();
    let pass = |src: &[f64], along_x: bool| -> Vec<f64> {
        let mut out = vec![0.0; src.len()];
        for j in 0..ny {
            for i in 0..nx {
                let mut sum = 0.0;
                for (w, k) in weights.iter().zip(-reach..=reach) {
                    let (ii, jj) = if along_x {
                        ((i as isize + k).clamp(0, nx as isize - 1) as usize, j)
                    } else {
                        (i, (j as isize + k).clamp(0, ny as isize - 1) as usize)
                    };
                    sum += w * src.get(jj * nx + ii).copied().unwrap_or(0.0);
                }
                if let Some(o) = out.get_mut(j * nx + i) {
                    *o = sum / total;
                }
            }
        }
        out
    };
    pass(&pass(field, true), false)
}

/// The tool as a mesh: a surface of revolution of its profile (tip, corner, flutes, shank),
/// at `tip` along `axis`.
pub fn tool_mesh(tool: &Tool, tip: Vec3, axis: Vec3) -> Mesh {
    let frame = match crate::frame::Frame::from_axis(tip, axis) {
        Ok(f) => f,
        Err(_) => return Mesh::default(),
    };
    let r = tool.radius();
    // Profile samples (rho, h) from the tip up: the cutting profile, then the flutes, the shank.
    let mut prof: Vec<(f64, f64)> = Vec::new();
    let steps = 8;
    for k in 0..=steps {
        let rho = r * k as f64 / steps as f64;
        prof.push((rho, tool.profile(rho)));
    }
    prof.push((r, tool.flute_length));
    let shank = if tool.shank_diameter > 0.0 { tool.shank_diameter * 0.5 } else { r };
    prof.push((shank, tool.flute_length));
    prof.push((shank, tool.overall_length));
    prof.push((0.0, tool.overall_length));
    let segs = 24usize;
    let mut mesh = Mesh::default();
    for (rho, h) in &prof {
        for s in 0..segs {
            let a = s as f64 / segs as f64 * std::f64::consts::TAU;
            let local = Vec3::new(rho * a.cos(), rho * a.sin(), *h);
            mesh.positions.push(frame.to_world(local));
            let n = Vec3::new(a.cos(), a.sin(), 0.0);
            mesh.normals.push(frame.dir_to_world(n));
        }
    }
    let rings = prof.len();
    for k in 0..rings - 1 {
        for s in 0..segs {
            let a = (k * segs + s) as u32;
            let b = (k * segs + (s + 1) % segs) as u32;
            let c = ((k + 1) * segs + (s + 1) % segs) as u32;
            let d = ((k + 1) * segs + s) as u32;
            mesh.triangles.push([a, b, c]);
            mesh.tri_face.push(0);
            mesh.triangles.push([a, c, d]);
            mesh.tri_face.push(0);
        }
    }
    // Recompute vertex normals from the faces for a correct shading of the tip.
    let mut acc = vec![Vec3::new(0.0, 0.0, 0.0); mesh.positions.len()];
    for t in &mesh.triangles {
        if let Some([a, b, c]) = mesh.tri(t) {
            let n = (b - a).cross(c - a);
            for i in t {
                if let Some(v) = acc.get_mut(*i as usize) {
                    *v += n;
                }
            }
        }
    }
    for (k, n) in acc.into_iter().enumerate() {
        if let (Some(v), Some(nn)) = (n.normalized(), mesh.normals.get_mut(k)) {
            *nn = v;
        }
    }
    mesh
}
