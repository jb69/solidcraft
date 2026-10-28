//! Stock simulation and checks. Three-axis moves (tool axis along z of the operation frame)
//! are swept through a Z-map of the stock, which gives the material removed, the stock left
//! for the next operation (rest machining), gouges below the model and rapids that would hit
//! material. Multi-axis moves are checked for shank and holder collisions against the model.

use serde::{Deserialize, Serialize};
use solidcraft_geom::{Aabb2, Mesh, Vec2, Vec3};

use crate::dropcutter::DropMesh;
use crate::heightmap::HeightMap;
use crate::notice::{Issue, Notice, r3};
use crate::setup::StockShape;
use crate::tool::Tool;
use crate::toolpath::{MoveKind, Toolpath};
use crate::{CamError, Result};

/// The stock as a Z-map: the height of material at each cell (the stock bottom where it is
/// cut through, `NEG_INFINITY` outside the stock's outline).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StockMap {
    pub map: HeightMap,
    pub bottom: f64,
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
        Ok(StockMap { map, bottom })
    }

    pub fn volume(&self) -> f64 {
        self.map.integrate(|z| if z.is_finite() { (z - self.bottom).max(0.0) } else { 0.0 })
    }

    /// Sweeps a 3-axis cut from `a` to `b` (tip positions) with `tool`.
    pub fn cut(&mut self, tool: &Tool, a: Vec3, b: Vec3) {
        let bottom = self.bottom;
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
                    m.set(i, j, z.max(bottom));
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
                let Some(z) = lowest_bottom(tool, p, a, b, a2, b2, r).map(|z| z.max(self.bottom)) else { continue };
                if let Some(cur) = m.get(i, j)
                    && cur.is_finite()
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
/// `b`: the closest point of the segment, and either end when it lies within the radius (a
/// plunge is lowest at its end; a ramp at its lower end). `None` when the tool never covers `p`.
fn lowest_bottom(tool: &Tool, p: Vec2, a: Vec3, b: Vec3, a2: Vec2, b2: Vec2, r: f64) -> Option<f64> {
    let (d, t) = crate::polyline::point_segment(p, a2, b2);
    let mut best: Option<f64> = None;
    let mut consider = |d: f64, t: f64| {
        if d <= r {
            let z = a.z + (b.z - a.z) * t + tool.profile(d);
            if best.is_none_or(|bz| z < bz) {
                best = Some(z);
            }
        }
    };
    consider(d, t);
    consider(p.dist(a2), 0.0);
    consider(p.dist(b2), 1.0);
    best
}

/// What the simulation found.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SimulationReport {
    pub removed_volume_mm3: f64,
    pub remaining_volume_mm3: f64,
    /// Material left above the model (within the stock), mm³.
    pub rest_volume_mm3: f64,
    pub rest_max_mm: f64,
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
        let nx = stock.map.nx;
        for (k, (s, mz)) in stock.map.z.iter().zip(&model.z).enumerate() {
            if !s.is_finite() {
                continue;
            }
            let d = s - mz;
            if d > tol {
                rest += d;
                rest_max = rest_max.max(d);
            } else if d < -tol && *mz > stock.bottom + tol {
                // On a near-vertical wall the model's height is ambiguous within a cell: the
                // cell is a gouge only when its neighbours agree it lies on the surface.
                let (i, j) = (k % nx, k / nx);
                let wall = [(i.wrapping_sub(1), j), (i + 1, j), (i, j.wrapping_sub(1)), (i, j + 1)]
                    .iter()
                    .filter_map(|(a, b)| model.get(*a, *b))
                    .any(|n| (mz - n) > stock.map.cell * 2.0);
                if !wall {
                    rep.gouge_cells += 1;
                    if -d > gouge_max {
                        gouge_max = -d;
                        let p = stock.map.point(i, j);
                        rep.gouge_at = Some(Vec3::new(p.x, p.y, *mz));
                    }
                }
            }
        }
        rep.rest_volume_mm3 = rest * stock.map.cell * stock.map.cell;
        rep.rest_max_mm = rest_max;
        rep.gouge_max_mm = gouge_max;
    }
    Ok(rep)
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
    let at = |i: usize, j: usize| -> Option<f64> {
        let (ii, jj) = ((i * step).min(m.nx.saturating_sub(1)), (j * step).min(m.ny.saturating_sub(1)));
        m.get(ii, jj).filter(|z| z.is_finite())
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
    let push_tri = |mesh: &mut Mesh, a: u32, b: u32, c: u32, face: u32| {
        mesh.triangles.push([a, b, c]);
        mesh.tri_face.push(face);
    };
    // Top quads.
    for j in 0..ny.saturating_sub(1) {
        for i in 0..nx.saturating_sub(1) {
            if let (Some(a), Some(b), Some(c), Some(d)) =
                (index[j * nx + i], index[j * nx + i + 1], index[(j + 1) * nx + i + 1], index[(j + 1) * nx + i])
            {
                push_tri(&mut mesh, a, b, c, 0);
                push_tri(&mut mesh, a, c, d, 0);
            }
        }
    }
    // Skirts: for every kept cell edge whose neighbour is missing, a quad down to the bottom.
    let mut bottom_pts: Vec<(usize, usize)> = Vec::new();
    let add_skirt = |mesh: &mut Mesh, a: u32, b: u32| {
        let pa = mesh.positions[a as usize];
        let pb = mesh.positions[b as usize];
        let base = mesh.positions.len() as u32;
        let n = (pb - pa).cross(Vec3::new(0.0, 0.0, 1.0)).normalized().unwrap_or(Vec3::new(1.0, 0.0, 0.0));
        for p in [pa, pb, Vec3::new(pb.x, pb.y, bottom), Vec3::new(pa.x, pa.y, bottom)] {
            mesh.positions.push(p);
            mesh.normals.push(n);
        }
        mesh.triangles.push([base, base + 2, base + 1]);
        mesh.tri_face.push(1);
        mesh.triangles.push([base, base + 3, base + 2]);
        mesh.tri_face.push(1);
    };
    for j in 0..ny {
        for i in 0..nx {
            let Some(here) = index[j * nx + i] else { continue };
            bottom_pts.push((i, j));
            // Right neighbour missing: skirt along the cell's right edge needs the vertex above.
            if i + 1 < nx
                && index[j * nx + i + 1].is_none()
                && j + 1 < ny
                && let Some(up) = index[(j + 1) * nx + i]
            {
                add_skirt(&mut mesh, up, here);
            }
            if (i == 0 || index[j * nx + i - 1].is_none())
                && j + 1 < ny
                && let Some(up) = index[(j + 1) * nx + i]
            {
                add_skirt(&mut mesh, here, up);
            }
            if j + 1 < ny
                && index[(j + 1) * nx + i].is_none()
                && i + 1 < nx
                && let Some(right) = index[j * nx + i + 1]
            {
                add_skirt(&mut mesh, here, right);
            }
            if (j == 0 || index[(j - 1) * nx + i].is_none())
                && i + 1 < nx
                && let Some(right) = index[j * nx + i + 1]
            {
                add_skirt(&mut mesh, right, here);
            }
            if i + 1 == nx
                && j + 1 < ny
                && let Some(up) = index[(j + 1) * nx + i]
            {
                add_skirt(&mut mesh, up, here);
            }
            if j + 1 == ny
                && i + 1 < nx
                && let Some(right) = index[j * nx + i + 1]
            {
                add_skirt(&mut mesh, here, right);
            }
        }
    }
    // Bottom: the kept cells' quads at the bottom height, facing down.
    let base = mesh.positions.len() as u32;
    let mut bottom_index: Vec<Option<u32>> = vec![None; nx * ny];
    for (i, j) in &bottom_pts {
        bottom_index[j * nx + i] = Some(mesh.positions.len() as u32);
        mesh.positions.push(pt(*i, *j, bottom));
        mesh.normals.push(Vec3::new(0.0, 0.0, -1.0));
    }
    let _ = base;
    for j in 0..ny.saturating_sub(1) {
        for i in 0..nx.saturating_sub(1) {
            if let (Some(a), Some(b), Some(c), Some(d)) =
                (bottom_index[j * nx + i], bottom_index[j * nx + i + 1], bottom_index[(j + 1) * nx + i + 1], bottom_index[(j + 1) * nx + i])
            {
                push_tri(&mut mesh, a, c, b, 2);
                push_tri(&mut mesh, a, d, c, 2);
            }
        }
    }
    // Smooth top normals from the height field.
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
            let h = m.cell * step as f64;
            let n =
                Vec3::new(-(z(1, 0) - z(-1, 0)) / (2.0 * h), -(z(0, 1) - z(0, -1)) / (2.0 * h), 1.0).normalized().unwrap_or(Vec3::new(0.0, 0.0, 1.0));
            if let Some(nn) = mesh.normals.get_mut(k as usize) {
                *nn = n;
            }
        }
    }
    mesh
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
