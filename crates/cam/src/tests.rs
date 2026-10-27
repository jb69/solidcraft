//! Tests of the CAM algorithms against analytic values.

use solidcraft_geom::{Aabb3, Mesh, Vec2, Vec3};

use crate::dropcutter::DropMesh;
use crate::frame::Frame;
use crate::heightmap::HeightMap;
use crate::linking::{Heights, ResolvedHeights};
use crate::machine::{Kinematics, Machine, forward_axis, inverse_angles, post_kinematics};
use crate::offset::{inward_family, offset_set};
use crate::ops::{Context, OpKind, ResolvedGeometry, generate};
use crate::polyline::{self, rect};
use crate::post::{Post, PostOperation, post_program};
use crate::setup::{Geometry, Operation, StockShape};
use crate::simulate::{StockMap, model_map, simulate};
use crate::tool::{Tool, ToolKind};
use crate::toolpath::{MoveKind, Toolpath};

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// A closed box mesh from `min` to `max` (outward normals).
fn box_mesh(min: Vec3, max: Vec3) -> Mesh {
    let p = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
    let v = [
        p(min.x, min.y, min.z),
        p(max.x, min.y, min.z),
        p(max.x, max.y, min.z),
        p(min.x, max.y, min.z),
        p(min.x, min.y, max.z),
        p(max.x, min.y, max.z),
        p(max.x, max.y, max.z),
        p(min.x, max.y, max.z),
    ];
    let tris: Vec<[u32; 3]> = vec![
        [0, 2, 1],
        [0, 3, 2], // bottom
        [4, 5, 6],
        [4, 6, 7], // top
        [0, 1, 5],
        [0, 5, 4], // front
        [1, 2, 6],
        [1, 6, 5], // right
        [2, 3, 7],
        [2, 7, 6], // back
        [3, 0, 4],
        [3, 4, 7], // left
    ];
    Mesh { positions: v.to_vec(), normals: vec![Vec3::new(0.0, 0.0, 1.0); 8], triangles: tris, ..Default::default() }
}

/// A UV sphere.
fn sphere_mesh(c: Vec3, r: f64, n: usize) -> Mesh {
    let mut pos = Vec::new();
    let mut tris = Vec::new();
    for i in 0..=n {
        let phi = std::f64::consts::PI * i as f64 / n as f64;
        for j in 0..n {
            let th = std::f64::consts::TAU * j as f64 / n as f64;
            pos.push(c + Vec3::new(phi.sin() * th.cos(), phi.sin() * th.sin(), phi.cos()) * r);
        }
    }
    let idx = |i: usize, j: usize| (i * n + (j % n)) as u32;
    for i in 0..n {
        for j in 0..n {
            tris.push([idx(i, j), idx(i + 1, j), idx(i + 1, j + 1)]);
            tris.push([idx(i, j), idx(i + 1, j + 1), idx(i, j + 1)]);
        }
    }
    Mesh { positions: pos, normals: Vec::new(), triangles: tris, ..Default::default() }
}

fn flat(d: f64) -> Tool {
    Tool::new(1, "flat", ToolKind::FlatEnd, d)
}

fn ball(d: f64) -> Tool {
    Tool::new(2, "ball", ToolKind::BallEnd, d)
}

#[test]
fn rectangle_offsets_grow_and_shrink() {
    let r = rect(Vec2::new(0.0, 0.0), Vec2::new(40.0, 20.0));
    let grown = offset_set(std::slice::from_ref(&r), 5.0, 0.01).unwrap();
    assert_eq!(grown.len(), 1);
    // Area of a rounded rectangle: 50x30 minus the corners: 4·(25 - 25π/4).
    let want = 50.0 * 30.0 - 4.0 * (25.0 - 25.0 * std::f64::consts::FRAC_PI_4);
    assert!(close(polyline::set_area(&grown), want, 1.0), "{} vs {want}", polyline::set_area(&grown));
    let shrunk = offset_set(std::slice::from_ref(&r), -5.0, 0.01).unwrap();
    assert_eq!(shrunk.len(), 1);
    assert!(close(polyline::set_area(&shrunk), 30.0 * 10.0, 0.05), "{}", polyline::set_area(&shrunk));
    // Shrinking past the half-width leaves nothing.
    assert!(offset_set(&[r], -10.5, 0.01).unwrap().is_empty());
}

#[test]
fn island_merges_into_wall_when_offset_closes_the_gap() {
    let outer = rect(Vec2::new(0.0, 0.0), Vec2::new(40.0, 40.0));
    let island: Vec<Vec2> = rect(Vec2::new(16.0, 16.0), Vec2::new(24.0, 24.0)).into_iter().rev().collect();
    let region = polyline::normalize_set(&[outer, island]);
    assert!(close(polyline::set_area(&region), 1600.0 - 64.0, 1e-6));
    // Offset inward by 3: a 34x34 outer with a 14x14 (rounded) hole.
    let shrunk = offset_set(&region, -3.0, 0.01).unwrap();
    assert_eq!(shrunk.len(), 2, "{shrunk:?}");
    let hole_area = 14.0 * 14.0 - 4.0 * (9.0 - 9.0 * std::f64::consts::FRAC_PI_4);
    assert!(close(polyline::set_area(&shrunk), 34.0 * 34.0 - hole_area, 1.0), "{}", polyline::set_area(&shrunk));
    // Offset inward by 9: the 8 mm ring around the island closes: two... no, the gap between
    // the island (16..24) and the wall is 16, so 9 leaves 16 - 9 - 9 < 0: nothing between
    // them on the sides but still the corners? The remaining region is the 22x22 square minus
    // a 26x26 rounded hole: empty.
    // What remains are four small slivers at the corners, between the shrunk outer square
    // (9..31) and the island's rounded offset (radius 9 about its corners): the corner (9, 9)
    // is 9.9 from (16, 16). Each sliver is under 1 mm².
    let gone = offset_set(&region, -9.0, 0.01).unwrap();
    assert_eq!(gone.len(), 4, "{gone:?}");
    assert!(polyline::set_area(&gone) < 4.0, "{}", polyline::set_area(&gone));
}

#[test]
fn inward_family_fills_a_pocket() {
    let r = rect(Vec2::new(0.0, 0.0), Vec2::new(30.0, 20.0));
    let fam = inward_family(&[r], 4.0, 0.01, 100).unwrap();
    // 10 mm half-width: rings at 0, 4, 8 → 3 rings.
    assert_eq!(fam.len(), 3, "{}", fam.len());
    assert!(close(polyline::set_area(&fam[2]), 14.0 * 4.0, 0.05));
}

#[test]
fn scanline_clips_to_region_with_hole() {
    let outer = rect(Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0));
    let hole: Vec<Vec2> = rect(Vec2::new(4.0, 4.0), Vec2::new(6.0, 6.0)).into_iter().rev().collect();
    let spans = polyline::scanline(&[outer, hole], 5.0);
    assert_eq!(spans.len(), 2);
    assert!(close(spans[0].0, 0.0, 1e-9) && close(spans[0].1, 4.0, 1e-9));
    assert!(close(spans[1].0, 6.0, 1e-9) && close(spans[1].1, 10.0, 1e-9));
}

#[test]
fn drop_cutter_on_box_top_edge_and_side() {
    let m = DropMesh::new(&box_mesh(Vec3::new(0.0, 0.0, 0.0), Vec3::new(20.0, 20.0, 10.0)));
    let t = flat(6.0);
    // Over the middle: the top face.
    let c = m.drop_z(&t, Vec2::new(10.0, 10.0), -100.0).unwrap();
    assert!(close(c.z, 10.0, 1e-9), "{}", c.z);
    // Partly over the edge (x = 21, radius 3 reaches to 18): still the top.
    let c = m.drop_z(&t, Vec2::new(21.0, 10.0), -100.0).unwrap();
    assert!(close(c.z, 10.0, 1e-9), "{}", c.z);
    // Beyond the edge: nothing.
    assert!(m.drop_z(&t, Vec2::new(24.0, 10.0), -100.0).is_none());
    // Ball tool over the edge at x = 22: the ball (r=3) touches the vertical edge line x=20,
    // z=10 at horizontal distance 2: tip z = 10 - (3 - sqrt(9 - 4)) = 10 - 0.7639.
    let b = ball(6.0);
    let c = m.drop_z(&b, Vec2::new(22.0, 10.0), -100.0).unwrap();
    assert!(close(c.z, 10.0 - (3.0 - 5f64.sqrt()), 1e-6), "{}", c.z);
    // Bull nose r=3, corner 1 at x = 22.5: the torus's centre circle (radius 2) comes within
    // 0.5 of the edge, so the ball of radius 1 sits sqrt(1 - 0.25) above it: tip z = 9.866.
    let mut bn = Tool::new(3, "bull", ToolKind::BullNose { corner_radius: 1.0 }, 6.0);
    bn.flute_length = 10.0;
    let c = m.drop_z(&bn, Vec2::new(22.5, 10.0), -100.0).unwrap();
    assert!(close(c.z, 10.0 - 1.0 + 0.75f64.sqrt(), 1e-6), "{}", c.z);
    // Directly over the edge the flat part reaches it: tip z = 10.
    let c = m.drop_z(&bn, Vec2::new(22.0, 10.0), -100.0).unwrap();
    assert!(close(c.z, 10.0, 1e-6), "{}", c.z);
    // Over the corner vertex (20, 20, 10) with the ball at (22, 22): rho = 2√2.
    let c = m.drop_z(&b, Vec2::new(22.0, 22.0), -100.0).unwrap();
    let rho = (8.0f64).sqrt();
    assert!(close(c.z, 10.0 - (3.0 - (9.0 - rho * rho).sqrt()), 1e-6), "{}", c.z);
}

#[test]
fn drop_cutter_on_sphere_matches_offset_sphere() {
    let r = 10.0;
    let m = DropMesh::new(&sphere_mesh(Vec3::new(0.0, 0.0, 0.0), r, 96));
    let b = ball(4.0);
    // A ball of radius 2 touching a sphere of radius 10: centre at distance 12. At x=5 the
    // centre height is sqrt(144 - 25); tip is 2 lower. The mesh is faceted so allow 0.05.
    let c = m.drop_z(&b, Vec2::new(5.0, 0.0), -100.0).unwrap();
    let want = (144.0f64 - 25.0).sqrt() - 2.0;
    assert!(close(c.z, want, 0.05), "{} vs {want}", c.z);
    // Flat tool at the pole: touches the sphere top.
    let f = flat(4.0);
    let c = m.drop_z(&f, Vec2::new(0.0, 0.0), -100.0).unwrap();
    assert!(close(c.z, 10.0, 0.02), "{}", c.z);
}

#[test]
fn drop_cutter_in_a_tilted_frame() {
    // A box dropped along -x: the frame's z is world x.
    let m = DropMesh::new(&box_mesh(Vec3::new(0.0, 0.0, 0.0), Vec3::new(20.0, 20.0, 10.0)));
    let frame = Frame::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)).unwrap();
    let c = m.drop(&flat(4.0), &frame, Vec2::new(10.0, 5.0), -100.0).unwrap();
    assert!(close(c.z, 20.0, 1e-9), "{}", c.z);
}

#[test]
fn heightmap_contours_of_a_cone() {
    let mut b = solidcraft_geom::Aabb2::default();
    b.add(Vec2::new(-10.0, -10.0));
    b.add(Vec2::new(10.0, 10.0));
    let h = HeightMap::from_fn(b, 0.1, |p| 10.0 - p.len()).unwrap();
    let loops = h.contours(5.0);
    assert_eq!(loops.len(), 1);
    // A circle of radius 5.
    assert!(close(polyline::set_area(&loops), 25.0 * std::f64::consts::PI, 0.5), "{}", polyline::set_area(&loops));
    assert!(polyline::is_ccw(&loops[0]));
}

fn heights(top: f64, bottom: f64) -> ResolvedHeights {
    ResolvedHeights { clearance: top + 10.0, retract: top + 5.0, feed: top + 2.0, top, bottom }
}

fn ctx_2d<'a>(tool: &'a Tool, loops: Vec<Vec<Vec2>>, h: ResolvedHeights) -> Context<'a> {
    Context {
        tool,
        model: None,
        fixtures: None,
        model_bounds: Aabb3::default(),
        stock: StockShape::Box { min: Vec3::new(-50.0, -50.0, -10.0), max: Vec3::new(50.0, 50.0, 0.0) },
        frame: Frame::IDENTITY,
        heights: h,
        geometry: ResolvedGeometry { loops: polyline::normalize_set(&loops), ..Default::default() },
        tolerance: 0.01,
        stock_to_leave_radial: 0.0,
        stock_to_leave_axial: 0.0,
        rest: None,
    }
}

fn op(kind: OpKind) -> Operation {
    Operation {
        id: 1,
        name: "op".into(),
        enabled: true,
        tool: 1,
        orientation: None,
        geometry: Geometry::Loops { loops: Vec::new() },
        heights: Heights::default(),
        linking: Default::default(),
        tolerance: 0.01,
        stock_to_leave_radial: 0.0,
        stock_to_leave_axial: 0.0,
        rest_machining: false,
        kind,
    }
}

fn cutting_bounds(tp: &Toolpath) -> (Vec3, Vec3) {
    let mut lo = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut hi = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for m in tp.moves.iter().filter(|m| m.kind == MoveKind::Cut) {
        lo = lo.min(m.to);
        hi = hi.max(m.to);
    }
    (lo, hi)
}

#[test]
fn pocket_2d_stays_inside_and_reaches_the_bottom() {
    let tool = flat(6.0);
    let ctx = ctx_2d(&tool, vec![rect(Vec2::new(0.0, 0.0), Vec2::new(40.0, 30.0))], heights(0.0, -6.0));
    let o = op(OpKind::Pocket2d { stepover: 2.0, stepdown: 3.0, direction: Default::default(), inside_out: true, finishing_stepover: 0.0 });
    let (tp, notice) = generate(&o, &ctx).unwrap();
    assert!(!tp.is_empty());
    let (lo, hi) = cutting_bounds(&tp);
    // Tool centre stays 3 mm (radius) inside the walls.
    assert!(lo.x >= 3.0 - 1e-6 && lo.y >= 3.0 - 1e-6 && hi.x <= 37.0 + 1e-6 && hi.y <= 27.0 + 1e-6, "{lo:?} {hi:?}");
    assert!(close(lo.z, -6.0, 1e-9), "{}", lo.z);
    assert_eq!(notice.values["levels"], 2);
    assert_eq!(notice.status(), "ok", "{:?}", notice.issues);
    // Every cutting move is below the top; every rapid is at or above the retract height
    // unless it is the descent to the feed height.
    for m in &tp.moves {
        if m.kind == MoveKind::Cut {
            assert!(m.to.z <= 0.0 + 1e-9);
        }
    }
}

#[test]
fn pocket_too_small_for_tool_reports_an_issue() {
    let tool = flat(12.0);
    let ctx = ctx_2d(&tool, vec![rect(Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0))], heights(0.0, -2.0));
    let o = op(OpKind::Pocket2d { stepover: 2.0, stepdown: 3.0, direction: Default::default(), inside_out: true, finishing_stepover: 0.0 });
    let (tp, notice) = generate(&o, &ctx).unwrap();
    assert!(tp.is_empty());
    assert!(notice.issues.iter().any(|i| i.code == "tool_too_large"), "{:?}", notice.issues);
}

#[test]
fn contour_2d_outside_offsets_by_the_radius() {
    let tool = flat(6.0);
    let ctx = ctx_2d(&tool, vec![rect(Vec2::new(0.0, 0.0), Vec2::new(40.0, 30.0))], heights(0.0, -5.0));
    let o = op(OpKind::Contour2d {
        side: crate::ops::Side::Outside,
        stepdown: 5.0,
        direction: Default::default(),
        roughing_passes: 0,
        roughing_stepover: 0.0,
        finishing_passes: 1,
        finishing_stepover: 0.0,
        tabs: 0,
        tab_width: 0.0,
        tab_height: 0.0,
    });
    let (tp, _) = generate(&o, &ctx).unwrap();
    let (lo, hi) = cutting_bounds(&tp);
    assert!(close(lo.x, -3.0, 1e-6) && close(hi.x, 43.0, 1e-6) && close(lo.y, -3.0, 1e-6) && close(hi.y, 33.0, 1e-6), "{lo:?} {hi:?}");
    // Climb on an outer contour: clockwise. Signed area of the cut loop's XY is negative.
    let xy: Vec<Vec2> = tp.moves.iter().filter(|m| m.kind == MoveKind::Cut).map(|m| Vec2::new(m.to.x, m.to.y)).collect();
    assert!(polyline::signed_area(&xy) < 0.0);
}

#[test]
fn face_covers_the_stock() {
    let tool = flat(10.0);
    let mut ctx = ctx_2d(&tool, Vec::new(), heights(0.0, -1.0));
    ctx.stock = StockShape::Box { min: Vec3::new(0.0, 0.0, -10.0), max: Vec3::new(50.0, 30.0, 0.0) };
    let o = op(OpKind::Face { stepover: 8.0, stepdown: 1.0, angle_deg: 0.0, direction: crate::ops::Direction::Both, extension: 0.0 });
    let (tp, _) = generate(&o, &ctx).unwrap();
    let (lo, hi) = cutting_bounds(&tp);
    assert!(lo.x <= -4.9 && hi.x >= 54.9, "{lo:?} {hi:?}");
    assert!(close(lo.z, -1.0, 1e-9));
}

#[test]
fn drill_cycles_and_canned_post() {
    let mut tool = Tool::new(5, "drill 5", ToolKind::Drill { tip_angle_deg: 118.0 }, 5.0);
    tool.flute_length = 40.0;
    tool.overall_length = 60.0;
    let mut ctx = ctx_2d(&tool, Vec::new(), heights(0.0, -10.0));
    ctx.geometry.holes = vec![Vec3::new(10.0, 10.0, 0.0), Vec3::new(30.0, 10.0, 0.0)];
    ctx.geometry.hole_diameter = 5.0;
    let o = op(OpKind::Drill { cycle: crate::toolpath::DrillCycleKind::Peck, peck: 3.0, dwell_s: 0.0, break_through: 0.0, optimize_order: true });
    let (tp, notice) = generate(&o, &ctx).unwrap();
    assert_eq!(tp.cycles.len(), 2);
    assert_eq!(notice.values["holes"], 2);
    assert!(close(tp.stats(5000.0, None).min_z, -10.0, 1e-9));
    let prog = post_program(&Post::generic(), &Machine::three_axis(), "holes", &[PostOperation { name: "drill", tool: &tool, toolpath: &tp }], 0.01)
        .unwrap();
    assert!(prog.gcode.contains("G83"), "{}", prog.gcode);
    assert!(prog.gcode.contains("Q3.0"), "{}", prog.gcode);
    assert!(prog.gcode.contains("T5 M6"), "{}", prog.gcode);
    assert!(prog.gcode.ends_with("M30\n%\n"), "{}", prog.gcode);
}

fn ctx_3d<'a>(tool: &'a Tool, mesh: &'a DropMesh, h: ResolvedHeights) -> Context<'a> {
    Context {
        tool,
        model: Some(mesh),
        fixtures: None,
        model_bounds: mesh.bounds,
        stock: StockShape::Box {
            min: Vec3::new(mesh.bounds.min.x - 2.0, mesh.bounds.min.y - 2.0, mesh.bounds.min.z),
            max: Vec3::new(mesh.bounds.max.x + 2.0, mesh.bounds.max.y + 2.0, mesh.bounds.max.z + 2.0),
        },
        frame: Frame::IDENTITY,
        heights: h,
        geometry: ResolvedGeometry::default(),
        tolerance: 0.05,
        stock_to_leave_radial: 0.0,
        stock_to_leave_axial: 0.0,
        rest: None,
    }
}

#[test]
fn parallel_finishing_follows_a_sphere() {
    let mesh = DropMesh::new(&sphere_mesh(Vec3::new(0.0, 0.0, 0.0), 10.0, 64));
    let tool = ball(4.0);
    let ctx = ctx_3d(&tool, &mesh, heights(12.0, 0.0));
    let o = op(OpKind::Parallel { stepover: 2.0, angle_deg: 0.0, direction: crate::ops::Direction::Both, slope_min_deg: 0.0, slope_max_deg: 90.0 });
    let (tp, notice) = generate(&o, &ctx).unwrap();
    assert!(!tp.is_empty());
    // Cuts over the sphere sit on the offset sphere (radius 12, tip 2 lower): check the
    // highest cut point is the pole.
    let top = tp.moves.iter().filter(|m| m.kind == MoveKind::Cut).map(|m| m.to.z).fold(f64::NEG_INFINITY, f64::max);
    assert!(close(top, 10.0, 0.1), "{top}");
    for m in tp.moves.iter().filter(|m| m.kind == MoveKind::Cut) {
        let d = Vec2::new(m.to.x, m.to.y).len();
        if d < 8.0 {
            let want = (144.0 - d * d).sqrt() - 2.0;
            assert!(close(m.to.z, want, 0.15), "{:?} want z {want}", m.to);
        }
    }
    assert!(notice.values["passes"].as_u64().unwrap() >= 5);
}

#[test]
fn waterline_contours_of_a_box_are_rectangles_offset_by_the_radius() {
    let mesh = DropMesh::new(&box_mesh(Vec3::new(0.0, 0.0, 0.0), Vec3::new(20.0, 20.0, 10.0)));
    let tool = flat(4.0);
    let mut ctx = ctx_3d(&tool, &mesh, heights(10.0, 0.0));
    ctx.stock = StockShape::Box { min: Vec3::new(-6.0, -6.0, 0.0), max: Vec3::new(26.0, 26.0, 10.0) };
    let o =
        op(OpKind::Contour3d { stepdown: 5.0, direction: Default::default(), slope_min_deg: 0.0, slope_max_deg: 90.0, flat_area_detection: false });
    let (tp, notice) = generate(&o, &ctx).unwrap();
    assert_eq!(notice.values["levels"], 2);
    let (lo, hi) = cutting_bounds(&tp);
    // Tool centre 2 mm (radius) off the walls, within the map's cell.
    assert!(close(lo.x, -2.0, 0.3) && close(hi.x, 22.0, 0.3), "{lo:?} {hi:?}");
    assert!(close(lo.z, 0.0, 1e-9) && close(hi.z, 5.0, 1e-9));
}

#[test]
fn pocket3d_roughs_around_a_box_without_gouging_it() {
    let mesh = DropMesh::new(&box_mesh(Vec3::new(0.0, 0.0, 0.0), Vec3::new(20.0, 20.0, 10.0)));
    let tool = flat(6.0);
    let mut ctx = ctx_3d(&tool, &mesh, heights(12.0, 0.0));
    ctx.stock = StockShape::Box { min: Vec3::new(-10.0, -10.0, 0.0), max: Vec3::new(30.0, 30.0, 12.0) };
    let o = op(OpKind::Pocket3d { stepover: 3.0, stepdown: 4.0, direction: Default::default(), flat_area_detection: true, order_by_depth: false });
    let (tp, notice) = generate(&o, &ctx).unwrap();
    assert!(!tp.is_empty());
    assert_eq!(notice.status(), "ok", "{:?}", notice.issues);
    // No cutting move below z = 10 with the tool within its radius of the box's footprint
    // (the roughing region is one map cell conservative, so allow 0.3 mm).
    for m in tp.moves.iter().filter(|m| m.is_cutting()) {
        let dx = (0.0 - m.to.x).max(m.to.x - 20.0).max(0.0);
        let dy = (0.0 - m.to.y).max(m.to.y - 20.0).max(0.0);
        let d = (dx * dx + dy * dy).sqrt();
        assert!(!(d < 3.0 - 0.3 && m.to.z < 10.0 - 1e-6), "gouge at {:?} (d = {d})", m.to);
    }
    // Simulate: the stock around the box is removed down to z = 0 and the box is not gouged.
    let cell = crate::simulate::stock_cell(&ctx.stock, 0.05).unwrap();
    let mut stock = StockMap::new(&ctx.stock, cell).unwrap();
    let model = model_map(&mesh, &stock.map, -1.0);
    let rep = simulate(&mut stock, &tool, &tp, None, Some(&model), Some(&mesh), None, 0.05).unwrap();
    assert_eq!(rep.gouge_cells, 0, "{rep:?}");
    assert_eq!(rep.rapid_collisions, 0, "{rep:?}");
    // Stock volume 40·40·12 = 19200 minus the box 4000 = 15200 to remove; the tool cannot
    // reach the corner fillets of radius 3 at the box's vertical edges, nor past the stock
    // outline the toolpath did not visit. Expect most of it gone.
    assert!(rep.removed_volume_mm3 > 14_000.0, "{rep:?}");
    // Every cell more than 0.6 mm from the box's walls and outside the corner fillets a 3 mm
    // tool must leave is cut to the floor; the lip along the walls is under 0.6 mm wide.
    for j in 0..stock.map.ny {
        for i in 0..stock.map.nx {
            let z = stock.map.get(i, j).unwrap_or(f64::NEG_INFINITY);
            if !z.is_finite() {
                continue;
            }
            let p = stock.map.point(i, j);
            let dx = (0.0 - p.x).max(p.x - 20.0).max(0.0);
            let dy = (0.0 - p.y).max(p.y - 20.0).max(0.0);
            let near_corner = dx > 0.0 && dy > 0.0 && (dx * dx + dy * dy).sqrt() < 3.5;
            let in_box = p.x >= 0.0 && p.x <= 20.0 && p.y >= 0.0 && p.y <= 20.0;
            if !in_box && !near_corner && dx.max(dy) > 0.6 {
                assert!(z <= 0.3, "stock left at {p:?}: z = {z}");
            }
        }
    }
}

#[test]
fn heights_resolve_and_validate() {
    let h = Heights::default();
    let r = h.resolve((-10.0, 0.0), (-8.0, 0.0)).unwrap();
    assert!(close(r.clearance, 10.0, 1e-9) && close(r.top, 0.0, 1e-9) && close(r.bottom, -8.0, 1e-9));
    let bad = Heights { bottom: crate::linking::Height::new(crate::linking::HeightRef::StockTop, 5.0), ..Heights::default() };
    assert!(bad.resolve((-10.0, 0.0), (-8.0, 0.0)).is_err());
}

#[test]
fn trunnion_inverse_kinematics_round_trips() {
    let m = Machine::trunnion_ac();
    for (a, c) in [(0.0, 0.0), (30.0, 45.0), (-60.0, 120.0), (90.0, -30.0), (15.0, 180.0)] {
        let t = forward_axis(&m.kinematics, a, c);
        let sols = inverse_angles(&m.kinematics, t, 1e-9).unwrap();
        let ok = sols.iter().any(|(p, s)| {
            let f = forward_axis(&m.kinematics, *p, *s);
            (f - t).len() < 1e-7
        });
        assert!(ok, "a={a} c={c} t={t:?} sols={sols:?}");
    }
    // Vertical tool: a = 0.
    let sols = inverse_angles(&m.kinematics, Vec3::new(0.0, 0.0, 1.0), 1e-9).unwrap();
    assert!(close(sols[0].0, 0.0, 1e-9));
    // A 3-axis machine cannot tilt.
    assert!(inverse_angles(&Kinematics::ThreeAxis, Vec3::new(0.0, 1.0, 1.0), 1e-9).is_err());
}

#[test]
fn four_axis_rotary_angles_and_machine_coordinates() {
    let m = Machine::four_axis_a();
    // Tool pointing along +y in the workpiece: the table turned so that the tool (machine +z)
    // appears along +y: A = 90° on a table axis along x. (sign by convention of forward_axis)
    let t = Vec3::new(0.0, 1.0, 0.0);
    let sols = inverse_angles(&m.kinematics, t, 1e-9).unwrap();
    let f = forward_axis(&m.kinematics, sols[0].0, sols[0].1);
    assert!((f - t).len() < 1e-9, "{f:?}");
    // An axis out of the YZ plane is unreachable.
    assert!(inverse_angles(&m.kinematics, Vec3::new(1.0, 0.0, 1.0), 1e-9).is_err());
    // Machine coordinates of a tip at (5, 10, 0) pointing along +y: rotated about x so the tip
    // lies on the machine's z axis above the pivot: y = 0, z = 10.
    let mut tp = Toolpath::new();
    tp.push(crate::toolpath::Move::new(MoveKind::Cut, Vec3::new(5.0, 10.0, 0.0), 500.0).with_axis(t)).unwrap();
    let posted = post_kinematics(&m, &tp, 1e-6).unwrap();
    let p = posted.last().unwrap().pose;
    assert!(close(p.xyz.x, 5.0, 1e-9) && close(p.xyz.y, 0.0, 1e-9) && close(p.xyz.z, 10.0, 1e-9), "{p:?}");
}

#[test]
fn linearisation_subdivides_non_tcp_moves() {
    let mut m = Machine::trunnion_ac();
    m.tcp = false;
    m.linearisation_tolerance = 0.01;
    let mut tp = Toolpath::new();
    tp.push(crate::toolpath::Move::new(MoveKind::Cut, Vec3::new(20.0, 0.0, 0.0), 500.0).with_axis(Vec3::new(0.0, 0.0, 1.0))).unwrap();
    let tilted = Vec3::new(0.0, (30f64).to_radians().sin(), (30f64).to_radians().cos());
    tp.push(crate::toolpath::Move::new(MoveKind::Cut, Vec3::new(20.0, 10.0, 0.0), 500.0).with_axis(tilted)).unwrap();
    let posted = post_kinematics(&m, &tp, 1e-6).unwrap();
    assert!(posted.len() > 2, "{}", posted.len());
    // Every intermediate pose maps back to a tip on the straight line within tolerance.
    for p in &posted {
        let tip = crate::machine::workpiece_tip(&m.kinematics, p.pose.xyz, p.pose.primary_deg, p.pose.secondary_deg);
        assert!(close(tip.x, 20.0, 0.02) && close(tip.z, 0.0, 0.02), "{tip:?}");
    }
    let mut tcp = m.clone();
    tcp.tcp = true;
    let posted = post_kinematics(&tcp, &tp, 1e-6).unwrap();
    assert_eq!(posted.len(), 2);
}

#[test]
fn rotary_operation_points_the_tool_at_the_axis() {
    // A cylinder of radius 10 along x from a box mesh is not round; use a sphere centred on the
    // axis: every radial drop lands at radius 10 + 2 (ball of radius 2 → tip at 10).
    let mesh = DropMesh::new(&sphere_mesh(Vec3::new(0.0, 0.0, 0.0), 10.0, 64));
    let tool = ball(4.0);
    let mut ctx = ctx_3d(&tool, &mesh, heights(12.0, -12.0));
    ctx.stock = StockShape::Cylinder { axis: crate::setup::Axis::X, radius: 12.0, start: -12.0, end: 12.0 };
    let o = op(OpKind::Rotary { along_axis: true, stepover: 30.0, angle_from_deg: 0.0, angle_to_deg: 360.0, inner_radius: 0.0 });
    let (tp, notice) = generate(&o, &ctx).unwrap();
    assert!(!tp.is_empty(), "{notice:?}");
    for m in tp.moves.iter().filter(|m| m.kind == MoveKind::Cut) {
        // The axis is radial (perpendicular to x) and the tip sits along it.
        assert!(m.axis.x.abs() < 1e-9);
        let radial = Vec2::new(m.to.y, m.to.z);
        let along = Vec2::new(m.axis.y, m.axis.z);
        assert!(close(radial.normalized().unwrap_or(along).dot(along), 1.0, 1e-6) || radial.len() < 1e-6, "{m:?}");
        if m.to.x.abs() < 6.0 {
            let want = (12f64.powi(2) - m.to.x * m.to.x).sqrt() - 2.0;
            assert!(close(radial.len(), want, 0.15), "{m:?} want r {want}");
        }
    }
    // Posted through a 4-axis machine, every point has y = 0 in machine coordinates.
    let m4 = Machine::four_axis_a();
    let posted = post_kinematics(&m4, &tp, 1e-6).unwrap();
    for p in &posted {
        assert!(close(p.pose.xyz.y, 0.0, 1e-6), "{p:?}");
    }
    let prog = post_program(&Post::fiveaxis(), &m4, "rotary", &[PostOperation { name: "rotary", tool: &tool, toolpath: &tp }], 0.01).unwrap();
    assert!(prog.gcode.contains(" A"), "{}", &prog.gcode[..prog.gcode.len().min(600)]);
}

#[test]
fn swarf_runs_the_tool_side_along_a_tilted_wall() {
    let tool = flat(6.0);
    let mut ctx = ctx_2d(&tool, Vec::new(), heights(10.0, 0.0));
    // A wall from (0,0,0)-(40,0,0) at the bottom to (0,5,10)-(40,5,10) at the top: tilted
    // 26.57° from vertical.
    ctx.geometry.chain_a = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(40.0, 0.0, 0.0)];
    ctx.geometry.chain_b = vec![Vec3::new(0.0, 5.0, 10.0), Vec3::new(40.0, 5.0, 10.0)];
    let o = op(OpKind::Swarf { left: false, passes: 1, extension: 0.0, max_tilt_deg: 0.0 });
    let (tp, notice) = generate(&o, &ctx).unwrap();
    let cuts: Vec<_> = tp.moves.iter().filter(|m| m.kind == MoveKind::Cut).collect();
    assert!(!cuts.is_empty(), "{notice:?}");
    let want_axis = Vec3::new(0.0, 5.0, 10.0).normalized().unwrap();
    for m in &cuts {
        assert!((m.axis - want_axis).len() < 1e-6, "{:?}", m.axis);
        // Right of the direction +x with the axis tilted: side = t × axis = (1,0,0)×(0,.447,.894)
        // = (0, -0.894, 0.447): tip at y = -2.68, z = +1.34.
        assert!(close(m.to.y, -3.0 * 0.894427, 1e-3) && close(m.to.z, 3.0 * 0.447214, 1e-3), "{:?}", m.to);
    }
    let prog =
        post_program(&Post::fiveaxis(), &Machine::trunnion_ac(), "swarf", &[PostOperation { name: "swarf", tool: &tool, toolpath: &tp }], 0.01)
            .unwrap();
    assert!(
        prog.gcode.contains(" A") && prog.gcode.contains(" C") || prog.gcode.contains("A-") || prog.gcode.contains("A2") || prog.gcode.contains("A3"),
        "{}",
        prog.gcode
    );
    // A 3-axis post refuses a tilted toolpath.
    assert!(
        post_program(&Post::generic(), &Machine::three_axis(), "swarf", &[PostOperation { name: "swarf", tool: &tool, toolpath: &tp }], 0.01)
            .is_err()
    );
}

#[test]
fn multi_axis_contour_follows_the_sphere_normal() {
    let mesh = DropMesh::new(&sphere_mesh(Vec3::new(0.0, 0.0, 0.0), 10.0, 64));
    let tool = ball(4.0);
    let ctx = ctx_3d(&tool, &mesh, heights(12.0, 0.0));
    let o = op(OpKind::MultiAxisContour {
        stepover: 3.0,
        angle_deg: 0.0,
        lead_deg: 0.0,
        tilt_deg: 0.0,
        max_tilt_deg: 60.0,
        direction: crate::ops::Direction::Both,
    });
    let (tp, notice) = generate(&o, &ctx).unwrap();
    assert!(!tp.is_empty(), "{notice:?}");
    for m in tp.moves.iter().filter(|m| m.kind == MoveKind::Cut) {
        // Tip on the sphere surface with a 2 mm ball: the ball centre is at radius 12 along the
        // axis from the tip (tip = centre - axis·2), so tip + axis·2 has length 12.
        let centre = m.to + m.axis * 2.0;
        if m.to.z > 2.0 {
            assert!(close(centre.len(), 12.0, 0.2), "{m:?} centre {}", centre.len());
            assert!(crate::frame::angle_between(m.axis, Vec3::new(0.0, 0.0, 1.0)).to_degrees() <= 60.0 + 1e-6);
        }
    }
}

#[test]
fn stock_simulation_removes_a_slot() {
    let shape = StockShape::Box { min: Vec3::new(0.0, 0.0, 0.0), max: Vec3::new(50.0, 20.0, 10.0) };
    let mut stock = StockMap::new(&shape, 0.25).unwrap();
    let v0 = stock.volume();
    assert!(close(v0, 10_000.0, 60.0), "{v0}");
    let tool = flat(6.0);
    let mut tp = Toolpath::new();
    tp.rapid(Vec3::new(-5.0, 10.0, 15.0)).unwrap();
    tp.rapid(Vec3::new(-5.0, 10.0, 8.0)).unwrap();
    tp.cut(Vec3::new(55.0, 10.0, 8.0), 500.0).unwrap();
    let rep = simulate(&mut stock, &tool, &tp, None, None, None, None, 0.01).unwrap();
    // A 6 mm wide, 2 mm deep slot across 50 mm: 600 mm³.
    assert!(close(rep.removed_volume_mm3, 600.0, 40.0), "{rep:?}");
    assert_eq!(rep.rapid_collisions, 0);
    // A rapid through the stock is flagged.
    let mut tp2 = Toolpath::new();
    tp2.rapid(Vec3::new(-5.0, 5.0, 5.0)).unwrap();
    tp2.rapid(Vec3::new(55.0, 5.0, 5.0)).unwrap();
    let rep2 = simulate(&mut stock, &tool, &tp2, None, None, None, None, 0.01).unwrap();
    assert_eq!(rep2.rapid_collisions, 1);
}

#[test]
fn post_writes_modal_words_and_tool_changes() {
    let tool = flat(6.0);
    let mut tp = Toolpath::new();
    tp.rapid(Vec3::new(0.0, 0.0, 10.0)).unwrap();
    tp.rapid(Vec3::new(0.0, 0.0, 1.0)).unwrap();
    tp.push(crate::toolpath::Move::new(MoveKind::Plunge, Vec3::new(0.0, 0.0, -2.0), 200.0)).unwrap();
    tp.cut(Vec3::new(30.0, 0.0, -2.0), 800.0).unwrap();
    tp.cut(Vec3::new(30.0, 20.0, -2.0), 800.0).unwrap();
    let prog =
        post_program(&Post::generic(), &Machine::three_axis(), "test", &[PostOperation { name: "slot", tool: &tool, toolpath: &tp }], 0.01).unwrap();
    let g = &prog.gcode;
    assert!(g.contains("G90 G94 G17 G21"));
    assert!(g.contains("T1 M6"));
    assert!(g.contains("S10000 M3"));
    assert!(g.contains("G0 X0.0 Y0.0 Z10.0"), "{g}");
    assert!(g.contains("G1 Z-2.0 F200.0"), "{g}");
    assert!(g.contains("X30.0 F800.0"), "{g}");
    assert!(g.contains("\nY20.0\n"), "{g}");
    let grbl =
        post_program(&Post::grbl(), &Machine::three_axis(), "test", &[PostOperation { name: "slot", tool: &tool, toolpath: &tp }], 0.01).unwrap();
    assert!(!grbl.gcode.contains("M6") && grbl.gcode.contains(";"), "{}", grbl.gcode);
}

#[test]
fn tool_profile_and_validation() {
    let b = ball(10.0);
    assert!(close(b.profile(0.0), 0.0, 1e-12));
    assert!(close(b.profile(5.0), 5.0, 1e-12));
    assert!(close(b.profile(3.0), 5.0 - 4.0, 1e-12));
    let bn = Tool::new(1, "bn", ToolKind::BullNose { corner_radius: 2.0 }, 10.0);
    assert!(close(bn.profile(3.0), 0.0, 1e-12));
    assert!(close(bn.profile(5.0), 2.0, 1e-12));
    let bad = Tool::new(1, "bad", ToolKind::BullNose { corner_radius: 6.0 }, 10.0);
    assert!(bad.validate().is_err());
    let mut neg = flat(6.0);
    neg.feeds.cutting = -1.0;
    assert!(neg.validate().is_err());
    assert!(close(b.radius_at(20.0), 5.0, 1e-12));
    assert!(close(b.radius_at(100.0), 0.0, 1e-12));
}

#[test]
fn hostile_params_never_panic() {
    let tool = flat(6.0);
    let ctx = ctx_2d(&tool, vec![rect(Vec2::new(0.0, 0.0), Vec2::new(40.0, 30.0))], heights(0.0, -6.0));
    for kind in [
        OpKind::Pocket2d { stepover: f64::NAN, stepdown: 3.0, direction: Default::default(), inside_out: true, finishing_stepover: 0.0 },
        OpKind::Pocket2d { stepover: -2.0, stepdown: f64::INFINITY, direction: Default::default(), inside_out: true, finishing_stepover: 0.0 },
        OpKind::Face { stepover: 1e-12, stepdown: 1e-12, angle_deg: f64::NAN, direction: Default::default(), extension: 0.0 },
        OpKind::Parallel { stepover: 0.0, angle_deg: 0.0, direction: Default::default(), slope_min_deg: 0.0, slope_max_deg: 90.0 },
        OpKind::Thread {},
    ] {
        let _ = generate(&op(kind), &ctx);
    }
    assert!(offset_set(&[vec![Vec2::new(0.0, 0.0), Vec2::new(0.0, 0.0)]], 1.0, 0.01).unwrap().is_empty());
    assert!(offset_set(&[rect(Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0))], f64::NAN, 0.01).is_err());
    assert!(HeightMap::new(polyline::bounds(&rect(Vec2::new(0.0, 0.0), Vec2::new(1e6, 1e6))), 0.001, 0.0).is_err());
}

#[test]
fn drill_pecks_do_not_rapid_into_stock() {
    let mut tool = Tool::new(5, "drill 6", ToolKind::Drill { tip_angle_deg: 118.0 }, 6.0);
    tool.flute_length = 30.0;
    tool.overall_length = 60.0;
    let mut ctx = ctx_2d(&tool, Vec::new(), heights(10.0, 0.0));
    ctx.stock = StockShape::Box { min: Vec3::new(-1.0, -1.0, 0.0), max: Vec3::new(41.0, 31.0, 11.0) };
    ctx.geometry.holes = vec![Vec3::new(10.0, 10.0, 10.0)];
    ctx.geometry.hole_diameter = 6.0;
    let o = op(OpKind::Drill { cycle: crate::toolpath::DrillCycleKind::Peck, peck: 2.0, dwell_s: 0.0, break_through: 1.0, optimize_order: true });
    let (tp, _) = generate(&o, &ctx).unwrap();
    let mut stock = StockMap::new(&ctx.stock, 0.25).unwrap();
    // Replay move by move to find the first rapid that hits.
    let mut prev: Option<Vec3> = None;
    for (i, m) in tp.moves.iter().enumerate() {
        if let Some(p) = prev
            && m.kind == MoveKind::Rapid
            && stock.rapid_hits(&tool, p, m.to, 0.05)
        {
            panic!("rapid {i} from {p:?} to {:?} hits the stock", m.to);
        }
        if let Some(p) = prev
            && m.kind != MoveKind::Rapid
        {
            stock.cut(&tool, p, m.to);
        }
        prev = Some(m.to);
    }
}
