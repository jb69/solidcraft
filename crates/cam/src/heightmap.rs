//! Height maps: a regular XY grid of heights. Used for the part's Z-map, the cutter-location
//! surface (where the tool tip may be at every XY), the stock in Z-map simulation and rest
//! material. Also the marching-squares extraction of level contours, which gives waterlines.

use serde::{Deserialize, Serialize};
use solidcraft_geom::{Aabb2, Vec2};

use crate::polyline::{self, Poly};
use crate::{CamError, MAX_CELLS, Result};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeightMap {
    pub origin: Vec2,
    pub cell: f64,
    pub nx: usize,
    pub ny: usize,
    /// Row-major: `z[j * nx + i]` is the height at `(origin.x + i·cell, origin.y + j·cell)`.
    pub z: Vec<f64>,
}

impl HeightMap {
    /// A map covering `bounds` at `cell` spacing (the last sample lies on or past the far
    /// edge), filled with `fill`.
    pub fn new(bounds: Aabb2, cell: f64, fill: f64) -> Result<HeightMap> {
        if !(cell.is_finite() && cell > 0.0) || bounds.is_empty() {
            return Err(CamError::Param("height map needs a positive cell size and non-empty bounds".into()));
        }
        let size = bounds.size();
        let nx = (size.x / cell).ceil() as usize + 1;
        let ny = (size.y / cell).ceil() as usize + 1;
        let total = nx.checked_mul(ny).ok_or_else(|| CamError::TooLarge("height map".into()))?;
        if total > MAX_CELLS {
            return Err(CamError::TooLarge(format!("height map of {nx} x {ny} cells exceeds {MAX_CELLS}; use a coarser tolerance")));
        }
        Ok(HeightMap { origin: bounds.min, cell, nx, ny, z: vec![fill; total] })
    }

    pub fn from_fn(bounds: Aabb2, cell: f64, mut f: impl FnMut(Vec2) -> f64) -> Result<HeightMap> {
        let mut h = HeightMap::new(bounds, cell, 0.0)?;
        for j in 0..h.ny {
            for i in 0..h.nx {
                let p = h.point(i, j);
                h.z[j * h.nx + i] = f(p);
            }
        }
        Ok(h)
    }

    pub fn point(&self, i: usize, j: usize) -> Vec2 {
        Vec2::new(self.origin.x + i as f64 * self.cell, self.origin.y + j as f64 * self.cell)
    }

    pub fn get(&self, i: usize, j: usize) -> Option<f64> {
        if i < self.nx && j < self.ny { self.z.get(j * self.nx + i).copied() } else { None }
    }

    pub fn set(&mut self, i: usize, j: usize, v: f64) {
        if i < self.nx
            && j < self.ny
            && let Some(c) = self.z.get_mut(j * self.nx + i)
        {
            *c = v;
        }
    }

    pub fn bounds(&self) -> Aabb2 {
        let mut b = Aabb2::default();
        b.add(self.origin);
        b.add(self.point(self.nx.saturating_sub(1), self.ny.saturating_sub(1)));
        b
    }

    /// Cell indices (floored) and fractional offsets of `p`, clamped to the grid.
    fn locate(&self, p: Vec2) -> (usize, usize, f64, f64) {
        let fx = ((p.x - self.origin.x) / self.cell).clamp(0.0, (self.nx.saturating_sub(1)) as f64);
        let fy = ((p.y - self.origin.y) / self.cell).clamp(0.0, (self.ny.saturating_sub(1)) as f64);
        let (fx, fy) = (if fx.is_finite() { fx } else { 0.0 }, if fy.is_finite() { fy } else { 0.0 });
        let i = (fx.floor() as usize).min(self.nx.saturating_sub(2));
        let j = (fy.floor() as usize).min(self.ny.saturating_sub(2));
        (i, j, fx - i as f64, fy - j as f64)
    }

    /// Bilinear interpolation (clamped at the edges).
    pub fn sample(&self, p: Vec2) -> f64 {
        if self.nx < 2 || self.ny < 2 {
            return self.z.first().copied().unwrap_or(0.0);
        }
        let (i, j, u, v) = self.locate(p);
        let g = |di: usize, dj: usize| self.get(i + di, j + dj).unwrap_or(0.0);
        let a = g(0, 0) * (1.0 - u) + g(1, 0) * u;
        let b = g(0, 1) * (1.0 - u) + g(1, 1) * u;
        a * (1.0 - v) + b * v
    }

    /// The highest of the four samples around `p` (a conservative sample for clearance).
    pub fn sample_max(&self, p: Vec2) -> f64 {
        if self.nx < 2 || self.ny < 2 {
            return self.z.first().copied().unwrap_or(0.0);
        }
        let (i, j, _, _) = self.locate(p);
        [(0, 0), (1, 0), (0, 1), (1, 1)].iter().filter_map(|(a, b)| self.get(i + a, j + b)).fold(f64::NEG_INFINITY, f64::max)
    }

    /// The highest sample within horizontal distance `r` of `p` (the tool's footprint), with
    /// `profile(rho)` added to each (the inverse tool offset): the tip height at which a tool
    /// with that profile touches this surface. The classic Z-map drop cutter.
    pub fn drop_profile(&self, p: Vec2, r: f64, profile: impl Fn(f64) -> f64) -> f64 {
        let i0 = (((p.x - r - self.origin.x) / self.cell).floor().max(0.0)) as usize;
        let j0 = (((p.y - r - self.origin.y) / self.cell).floor().max(0.0)) as usize;
        let i1 = ((((p.x + r - self.origin.x) / self.cell).ceil().max(0.0)) as usize).min(self.nx.saturating_sub(1));
        let j1 = ((((p.y + r - self.origin.y) / self.cell).ceil().max(0.0)) as usize).min(self.ny.saturating_sub(1));
        let mut best = f64::NEG_INFINITY;
        for j in j0..=j1 {
            for i in i0..=i1 {
                let q = self.point(i, j);
                let rho = q.dist(p);
                if rho <= r
                    && let Some(z) = self.get(i, j)
                {
                    best = best.max(z - profile(rho));
                }
            }
        }
        best
    }

    pub fn min(&self) -> f64 {
        self.z.iter().copied().filter(|v| v.is_finite()).fold(f64::INFINITY, f64::min)
    }

    pub fn max(&self) -> f64 {
        self.z.iter().copied().filter(|v| v.is_finite()).fold(f64::NEG_INFINITY, f64::max)
    }

    /// Sum over cells of `f(z)` times the cell area (a volume when `f` is a height).
    pub fn integrate(&self, f: impl Fn(f64) -> f64) -> f64 {
        self.z.iter().map(|z| f(*z)).sum::<f64>() * self.cell * self.cell
    }

    /// A map of the same grid with `f(z_self, z_other)` per cell; the other must share the grid.
    pub fn combine(&self, other: &HeightMap, f: impl Fn(f64, f64) -> f64) -> Result<HeightMap> {
        if other.nx != self.nx || other.ny != self.ny {
            return Err(CamError::Geometry("height maps have different grids".into()));
        }
        Ok(HeightMap {
            origin: self.origin,
            cell: self.cell,
            nx: self.nx,
            ny: self.ny,
            z: self.z.iter().zip(&other.z).map(|(a, b)| f(*a, *b)).collect(),
        })
    }

    /// Closed level contours at height `level` by marching squares: the loops bounding the
    /// region where the map is above `level`, oriented as a normalized region (outers CCW).
    pub fn contours(&self, level: f64) -> Vec<Poly> {
        if self.nx < 2 || self.ny < 2 {
            return Vec::new();
        }
        // Segments per cell, then stitched. Each crossing point is identified by its edge so the
        // stitching is exact.
        let mut segs: Vec<(Vec2, Vec2)> = Vec::new();
        let above = |i: usize, j: usize| self.get(i, j).is_some_and(|z| z > level);
        let cross = |i0: usize, j0: usize, i1: usize, j1: usize| -> Vec2 {
            let a = self.get(i0, j0).unwrap_or(level);
            let b = self.get(i1, j1).unwrap_or(level);
            // A blocked (infinite) cell puts the crossing halfway, so a straight border stays a
            // straight line (snapping to the finite cell would make a staircase).
            let t = if !a.is_finite() || !b.is_finite() || (b - a).abs() < 1e-18 { 0.5 } else { ((level - a) / (b - a)).clamp(0.0, 1.0) };
            self.point(i0, j0).lerp(self.point(i1, j1), t)
        };
        for j in 0..self.ny - 1 {
            for i in 0..self.nx - 1 {
                // Corners: 0 = (i,j), 1 = (i+1,j), 2 = (i+1,j+1), 3 = (i,j+1).
                let c = [above(i, j), above(i + 1, j), above(i + 1, j + 1), above(i, j + 1)];
                let code = (c[0] as u8) | (c[1] as u8) << 1 | (c[2] as u8) << 2 | (c[3] as u8) << 3;
                if code == 0 || code == 15 {
                    continue;
                }
                // Edge crossing points: bottom (0-1), right (1-2), top (2-3), left (3-0).
                let bottom = || cross(i, j, i + 1, j);
                let right = || cross(i + 1, j, i + 1, j + 1);
                let top = || cross(i + 1, j + 1, i, j + 1);
                let left = || cross(i, j + 1, i, j);
                // Segments directed so the "above" side is on the left (CCW around high areas).
                let mut emit = |a: Vec2, b: Vec2| segs.push((a, b));
                match code {
                    1 => emit(left(), bottom()),
                    2 => emit(bottom(), right()),
                    3 => emit(left(), right()),
                    4 => emit(right(), top()),
                    5 => {
                        // Saddle: decide by the centre value.
                        let centre =
                            [self.get(i, j), self.get(i + 1, j), self.get(i + 1, j + 1), self.get(i, j + 1)].iter().flatten().sum::<f64>() / 4.0;
                        if centre > level {
                            emit(left(), top());
                            emit(right(), bottom());
                        } else {
                            emit(left(), bottom());
                            emit(right(), top());
                        }
                    }
                    6 => emit(bottom(), top()),
                    7 => emit(left(), top()),
                    8 => emit(top(), left()),
                    9 => emit(top(), bottom()),
                    10 => {
                        let centre =
                            [self.get(i, j), self.get(i + 1, j), self.get(i + 1, j + 1), self.get(i, j + 1)].iter().flatten().sum::<f64>() / 4.0;
                        if centre > level {
                            emit(bottom(), left());
                            emit(top(), right());
                        } else {
                            emit(bottom(), right());
                            emit(top(), left());
                        }
                    }
                    11 => emit(top(), right()),
                    12 => emit(right(), left()),
                    13 => emit(right(), bottom()),
                    14 => emit(bottom(), left()),
                    _ => {}
                }
            }
        }
        let loops = stitch_exact(&segs, self.cell * 1e-6);
        let loops: Vec<Poly> = loops.into_iter().filter(|p| p.len() >= 3 && polyline::signed_area(p).abs() > self.cell * self.cell * 0.01).collect();
        polyline::normalize_set(&loops)
    }
}

/// Stitches directed segments whose shared endpoints coincide within `tol`.
fn stitch_exact(segs: &[(Vec2, Vec2)], tol: f64) -> Vec<Poly> {
    use std::collections::HashMap;
    let s = 1.0 / tol.max(1e-12);
    let key = |p: Vec2| ((p.x * s).round() as i64, (p.y * s).round() as i64);
    let mut starts: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (k, (a, _)) in segs.iter().enumerate() {
        starts.entry(key(*a)).or_default().push(k);
    }
    let mut used = vec![false; segs.len()];
    let mut out = Vec::new();
    for start in 0..segs.len() {
        if used[start] {
            continue;
        }
        let mut chain = vec![segs[start].0];
        let mut k = start;
        let mut closed = false;
        for _ in 0..=segs.len() {
            used[k] = true;
            let b = segs[k].1;
            if b.dist(segs[start].0) <= tol * 4.0 {
                closed = true;
                break;
            }
            chain.push(b);
            let kb = key(b);
            let mut next = None;
            'outer: for dx in -1..=1 {
                for dy in -1..=1 {
                    if let Some(c) = starts.get(&(kb.0 + dx, kb.1 + dy)) {
                        for &n in c {
                            if !used[n] && segs[n].0.dist(b) <= tol * 4.0 {
                                next = Some(n);
                                break 'outer;
                            }
                        }
                    }
                }
            }
            match next {
                Some(n) => k = n,
                None => break,
            }
        }
        if closed {
            out.push(chain);
        }
    }
    out
}
