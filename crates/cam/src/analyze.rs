//! What a part asks of the machining, before any plan (`cam.analyze`, docs/cam-ai-planning.md
//! step 2): its features with their numbers, which directions reach each face, what the
//! features ask of the tools, and whether the part fits the machine. Facts only: the assistant
//! and the plan decide what to do with them.
//!
//! Everything is measured in the setup's frame (its WCS), whose +z is the tool axis: "up" is
//! towards the spindle, a floor faces +z, a wall is square to it.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::json;
use solidcore_record::{Vec2, Vec3};
use solidcraft_geom::Mesh;
use solidcraft_geom::prelude::*;

use crate::dropcutter::DropMesh;
use crate::frame::Frame;
use crate::machine::{Kinematics, Machine};
use crate::notice::{Issue, Notice};
use crate::polyline::{self, Poly};
use crate::tool::{Tool, ToolKind};
use crate::{CamError, Result};

/// A face's exact surface, where the kernel knows it (in the setup's frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Surface {
    Plane { normal: Vec3 },
    Cylinder { origin: Vec3, axis: Vec3, radius: f64 },
    Cone { apex: Vec3, axis: Vec3, half_angle: f64 },
    Sphere { center: Vec3, radius: f64 },
    Torus { center: Vec3, axis: Vec3, major: f64, minor: f64 },
    /// A free-form surface (or one the kernel does not name).
    Other,
}

/// One body to analyse: its mesh in the setup's frame (each triangle tagged with its face in
/// `tri_face`) and each face's surface, by face index.
pub struct BodyInput<'a> {
    pub name: &'a str,
    pub mesh: &'a Mesh,
    pub surfaces: &'a [Surface],
}

/// Caps: a part is hostile input too.
const MAX_TRIANGLES: usize = 2_000_000;
const MAX_FACES: usize = 50_000;
const MAX_GRID: usize = 512;

/// A face as the analysis sees it.
#[derive(Clone, Debug, Serialize)]
pub struct FaceFacts {
    pub body: String,
    pub face: usize,
    /// `top`, `floor`, `ceiling`, `wall`, `slope`, `hole`, `boss`, `inside_round`, `outside_round`,
    /// `edge_round`, `chamfer`, `freeform`.
    pub role: &'static str,
    pub area_mm2: f64,
    pub z_min: f64,
    pub z_max: f64,
    /// For a cylinder, cone, sphere or torus.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radius_mm: Option<f64>,
    /// The surface's slope from the tool axis (0: a floor, 90: a wall), for a plane.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slope_deg: Option<f64>,
    /// Which of the six directions reach it (`+z`, `-z`, `+x`, `-x`, `+y`, `-y`), each with the
    /// share of the face reached.
    pub reached_from: Vec<(&'static str, f64)>,
}

/// A feature: faces that are machined together, with its numbers.
#[derive(Clone, Debug, Serialize)]
pub struct Feature {
    /// `pocket-1`, `hole-2`… (stable for the same part).
    pub id: String,
    /// `pocket`, `step`, `hole`, `boss`, `island`, `slope`, `freeform`, `undercut`, `top`.
    pub kind: &'static str,
    /// The faces it covers: (body, face index).
    pub faces: Vec<(String, usize)>,
    /// Its numbers: depths and heights from the part's top, sizes, the largest tools that fit.
    pub values: serde_json::Map<String, serde_json::Value>,
}

/// The analysis.
#[derive(Clone, Debug, Serialize)]
pub struct Analysis {
    pub features: Vec<Feature>,
    pub faces: Vec<FaceFacts>,
    /// For each direction: the share of the part's area it reaches, and how many faces it
    /// reaches fully.
    pub access: Vec<serde_json::Value>,
    /// The fewest directions that reach every face that any direction reaches, in order.
    pub directions: Vec<&'static str>,
    /// What the features ask of the tools, against the machine.
    pub limits: serde_json::Map<String, serde_json::Value>,
    /// Size of the part and the stock against the machine.
    pub fit: serde_json::Map<String, serde_json::Value>,
    pub notice: Notice,
}

/// The six directions a setup can face (in the setup's frame).
const DIRECTIONS: [(&str, [f64; 3]); 6] =
    [("+z", [0.0, 0.0, 1.0]), ("-z", [0.0, 0.0, -1.0]), ("+x", [1.0, 0.0, 0.0]), ("-x", [-1.0, 0.0, 0.0]), ("+y", [0.0, 1.0, 0.0]), ("-y", [0.0, -1.0, 0.0])];

/// Common tap drills (metric coarse): drill diameter → thread.
const TAP_DRILLS: [(f64, &str); 8] = [(1.6, "M2"), (2.05, "M2.5"), (2.5, "M3"), (3.3, "M4"), (4.2, "M5"), (5.0, "M6"), (6.8, "M8"), (8.5, "M10")];

/// Per-face sums from its triangles.
#[derive(Clone, Default)]
struct Acc {
    area: f64,
    centroid: Vec3,
    normal: Vec3,
    zmin: f64,
    zmax: f64,
    tris: Vec<usize>,
}

/// Analyses `bodies` (in the setup's frame) for `machine`. `stock` is the stock's box in the
/// same frame, when known; `material_speed` the cutting speed (m/min) the smallest tools should
/// reach.
pub fn analyze(bodies: &[BodyInput], machine: &Machine, stock: Option<(Vec3, Vec3)>, material_speed: f64, tol: f64) -> Result<Analysis> {
    let tol = if tol.is_finite() && tol > 0.0 { tol.clamp(0.001, 1.0) } else { 0.01 };
    let tris: usize = bodies.iter().map(|b| b.mesh.triangles.len()).sum();
    if tris > MAX_TRIANGLES {
        return Err(CamError::TooLarge(format!("the part has {tris} triangles; analysis takes at most {MAX_TRIANGLES}")));
    }
    if tris == 0 {
        return Err(CamError::Geometry("there is no body to analyse".into()));
    }
    // Faces, with their sums.
    let mut keys: Vec<(usize, usize)> = Vec::new();
    let mut accs: Vec<Acc> = Vec::new();
    let mut slot: HashMap<(usize, usize), usize> = HashMap::new();
    let (mut lo, mut hi) = (Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY), Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY));
    for (bi, b) in bodies.iter().enumerate() {
        for (ti, t) in b.mesh.triangles.iter().enumerate() {
            let Some([p, q, r]) = b.mesh.tri(t) else { continue };
            if !(p.is_finite() && q.is_finite() && r.is_finite()) {
                continue;
            }
            let face = b.mesh.tri_face.get(ti).copied().unwrap_or(0) as usize;
            let k = match slot.get(&(bi, face)) {
                Some(k) => *k,
                None => {
                    if keys.len() >= MAX_FACES {
                        return Err(CamError::TooLarge(format!("the part has more than {MAX_FACES} faces")));
                    }
                    keys.push((bi, face));
                    accs.push(Acc { zmin: f64::INFINITY, zmax: f64::NEG_INFINITY, ..Default::default() });
                    slot.insert((bi, face), keys.len() - 1);
                    keys.len() - 1
                }
            };
            let n2 = (q - p).cross(r - p);
            let a = n2.len() * 0.5;
            let Some(acc) = accs.get_mut(k) else { continue };
            acc.area += a;
            acc.centroid = acc.centroid + (p + q + r) * (a / 3.0);
            acc.normal = acc.normal + n2 * 0.5;
            for v in [p, q, r] {
                acc.zmin = acc.zmin.min(v.z);
                acc.zmax = acc.zmax.max(v.z);
                lo = Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
                hi = Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
            }
            acc.tris.push(ti);
        }
    }
    if !(lo.is_finite() && hi.is_finite()) {
        return Err(CamError::Geometry("the part has no finite triangles".into()));
    }
    for a in &mut accs {
        if a.area > 0.0 {
            a.centroid = a.centroid * (1.0 / a.area);
        }
    }
    let size = hi - lo;
    let top = hi.z;
    let bottom = lo.z;
    let flat = tol.max(size.len() * 1e-6);

    // Access: for each direction, which share of each face it reaches.
    let reach = access(bodies, &keys, &accs, lo, hi)?;

    // Roles.
    let up = Vec3::new(0.0, 0.0, 1.0);
    let mut roles: Vec<&'static str> = Vec::with_capacity(keys.len());
    let mut radii: Vec<Option<f64>> = Vec::with_capacity(keys.len());
    let mut slopes: Vec<Option<f64>> = Vec::with_capacity(keys.len());
    for (k, (bi, face)) in keys.iter().enumerate() {
        let acc = &accs[k];
        let surface = bodies.get(*bi).and_then(|b| b.surfaces.get(*face)).copied().unwrap_or(Surface::Other);
        let mean_n = acc.normal.normalized().unwrap_or(up);
        let (role, radius, slope) = match surface {
            Surface::Plane { normal } => {
                let n = normal.normalized().unwrap_or(mean_n);
                let c = n.dot(up);
                let slope = c.clamp(-1.0, 1.0).acos().to_degrees();
                let role = if c > 0.999 {
                    if (acc.zmax - top).abs() <= flat { "top" } else { "floor" }
                } else if c < -0.999 {
                    "ceiling"
                } else if c.abs() < 0.0175 {
                    "wall"
                } else if acc.area < (size.x * size.y).max(1.0) * 0.002 && (acc.zmax - acc.zmin) < size.z * 0.1 {
                    "chamfer"
                } else {
                    "slope"
                };
                (role, None, Some(slope))
            }
            Surface::Cylinder { origin, axis, radius } => {
                let ax = axis.normalized().unwrap_or(up);
                let along_z = ax.dot(up).abs() > 0.999;
                // Concave (a hole, an inside round) when the cylinder's axis, beside the face, is
                // out of the material; convex (a boss, an outside round) when it is inside.
                let concave = if along_z {
                    let at = origin + ax * ((acc.zmin + acc.zmax) * 0.5 - origin.z) * (1.0 / ax.z);
                    bodies.get(*bi).is_some_and(|b| !inside(b.mesh, at))
                } else {
                    let to_axis = {
                        let d = acc.centroid - origin;
                        let radial = d - ax * d.dot(ax);
                        -radial.normalized().unwrap_or(mean_n)
                    };
                    face_normal_sum(bodies, *bi, &acc.tris, |n| n.dot(to_axis)) > 0.0
                };
                let span = angular_span(bodies, *bi, &acc.tris, origin, ax);
                let role = match (along_z, concave, span > 5.5) {
                    (true, true, true) => "hole",
                    (true, false, true) => "boss",
                    (true, true, false) => "inside_round",
                    (true, false, false) => "outside_round",
                    (false, _, _) => "edge_round",
                };
                (role, Some(radius), None)
            }
            Surface::Cone { axis, half_angle, .. } => {
                let along_z = axis.normalized().is_some_and(|a| a.dot(up).abs() > 0.999);
                (if along_z { "chamfer" } else { "freeform" }, None, Some(half_angle.to_degrees()))
            }
            Surface::Sphere { radius, .. } => ("freeform", Some(radius), None),
            Surface::Torus { minor, .. } => ("freeform", Some(minor), None),
            Surface::Other => {
                let c = mean_n.dot(up);
                if c > 0.999 {
                    (if (acc.zmax - top).abs() <= flat { "top" } else { "floor" }, None, Some(0.0))
                } else {
                    ("freeform", None, None)
                }
            }
        };
        roles.push(role);
        radii.push(radius.filter(|r| r.is_finite() && *r > 0.0));
        slopes.push(slope);
    }

    // Adjacency between faces of a body, from the mesh's edges (each lists the faces it bounds).
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); keys.len()];
    for (bi, b) in bodies.iter().enumerate() {
        for fs in &b.mesh.edge_faces {
            let ks: Vec<usize> = fs.iter().filter_map(|f| slot.get(&(bi, *f as usize)).copied()).collect();
            for &a in &ks {
                for &c in &ks {
                    if a != c && adj.get(a).is_some_and(|l| !l.contains(&c)) {
                        if let Some(l) = adj.get_mut(a) {
                            l.push(c);
                        }
                    }
                }
            }
        }
    }

    // Features.
    let mut features: Vec<Feature> = Vec::new();
    let mut taken = vec![false; keys.len()];
    let mut counts: HashMap<&'static str, usize> = HashMap::new();
    let mut next_id = |kind: &'static str| -> String {
        let n = counts.entry(kind).or_insert(0);
        *n += 1;
        format!("{kind}-{n}")
    };
    let face_ref = |k: usize| -> (String, usize) {
        let (bi, f) = keys[k];
        (bodies.get(bi).map_or_else(String::new, |b| b.name.to_string()), f)
    };
    let mut issues: Vec<Issue> = Vec::new();
    let mut smallest_tool: Option<(f64, String)> = None;
    let mut note_tool = |d: f64, why: String| {
        if d.is_finite() && d > 0.0 && smallest_tool.as_ref().is_none_or(|(s, _)| d < *s) {
            smallest_tool = Some((d, why));
        }
    };

    // Holes.
    for k in 0..keys.len() {
        if roles[k] != "hole" {
            continue;
        }
        taken[k] = true;
        let d = radii[k].unwrap_or(0.0) * 2.0;
        let acc = &accs[k];
        let depth = acc.zmax - acc.zmin;
        let through = (acc.zmin - bottom).abs() <= flat && (acc.zmax - top).abs() <= flat;
        let mut v = serde_json::Map::new();
        v.insert("diameter_mm".into(), json!(r3(d)));
        v.insert("depth_mm".into(), json!(r3(depth)));
        v.insert("top_from_part_top_mm".into(), json!(r3(top - acc.zmax)));
        v.insert("through".into(), json!(through));
        v.insert("at".into(), json!([r3(acc.centroid.x), r3(acc.centroid.y)]));
        if let Some((_, m)) = TAP_DRILLS.iter().find(|(t, _)| (t - d).abs() < 0.06) {
            v.insert("tap_drill_for".into(), json!(m));
        }
        if d > 0.0 && depth / d > 5.0 {
            v.insert("depth_to_diameter".into(), json!(r3(depth / d)));
        }
        note_tool(d, format!("a Ø{} mm hole", r3(d)));
        // A blind hole's bottom (flat, or a drill point's cone) belongs to it.
        let mut faces = vec![face_ref(k)];
        for &n in adj.get(k).map_or(&[][..], |l| l.as_slice()) {
            if !taken[n] && matches!(roles[n], "floor" | "chamfer") && (accs[n].zmax - acc.zmin).abs() <= flat.max(d * 0.6) && accs[n].zmin >= acc.zmin - d {
                taken[n] = true;
                faces.push(face_ref(n));
            }
        }
        let id = next_id("hole");
        if d > 0.0 && depth / d > 8.0 {
            issues.push(
                Issue::warning("deep_narrow", format!("{id} is {} mm deep and Ø{} mm: drill in pecks, or it cannot be reached", r3(depth), r3(d)))
                    .parameter(&id, r3(depth / d))
                    .limit(8.0),
            );
        }
        features.push(Feature { id, kind: "hole", faces, values: v });
    }

    // Islands: a raised flat (below the top or flush with it) whose every side is a wall that
    // drops from it to a floor. The part's own top is not one: its outer sides drop to the bottom.
    for k in 0..keys.len() {
        if taken[k] || !matches!(roles[k], "floor" | "top") {
            continue;
        }
        let z = accs[k].zmax;
        let sides: Vec<usize> = adj.get(k).map_or_else(Vec::new, |l| l.clone());
        let drops = |w: &usize| matches!(roles[*w], "wall" | "outside_round" | "slope" | "chamfer") && accs[*w].zmin < z - flat && accs[*w].zmax <= z + flat;
        let lands_on_floor = |w: &usize| adj.get(*w).is_some_and(|l| l.iter().any(|n| roles[*n] == "floor" && accs[*n].zmax < z - flat));
        if sides.is_empty() || !sides.iter().all(|w| drops(w) && lands_on_floor(w)) {
            continue;
        }
        taken[k] = true;
        let mut faces = vec![face_ref(k)];
        for w in &sides {
            if !taken[*w] {
                taken[*w] = true;
                faces.push(face_ref(*w));
            }
        }
        let below = sides.iter().map(|w| accs[*w].zmin).fold(f64::INFINITY, f64::min);
        let mut v = serde_json::Map::new();
        v.insert("height_mm".into(), json!(r3(z - below)));
        v.insert("top_from_part_top_mm".into(), json!(r3(top - z)));
        v.insert("top_area_mm2".into(), json!(r3(accs[k].area)));
        let id = next_id("island");
        features.push(Feature { id, kind: "island", faces, values: v });
    }

    // Pockets and steps: each floor with the walls, rounds and slopes around it that rise from it.
    for k in 0..keys.len() {
        if roles[k] != "floor" || taken[k] {
            continue;
        }
        taken[k] = true;
        let floor_z = accs[k].zmax;
        let mut faces = vec![k];
        let mut stack = vec![k];
        while let Some(f) = stack.pop() {
            for &n in adj.get(f).map_or(&[][..], |l| l.as_slice()) {
                if taken[n] || !matches!(roles[n], "wall" | "inside_round" | "slope" | "chamfer") {
                    continue;
                }
                // Walls that rise from this floor (not ones that drop below it).
                if accs[n].zmin < floor_z - flat {
                    continue;
                }
                taken[n] = true;
                faces.push(n);
                stack.push(n);
            }
        }
        // Closed when every edge of the floor meets a wall that rises from it.
        let rises = adj.get(k).map_or(0, |l| l.iter().filter(|n| faces.contains(n)).count());
        let drops = adj.get(k).map_or(0, |l| l.iter().filter(|n| !faces.contains(n) && accs[**n].zmin < floor_z - flat).count());
        let kind = if drops == 0 && rises > 0 { "pocket" } else { "step" };
        let depth = top - floor_z;
        let inside_r: Option<f64> = faces.iter().filter(|f| roles[**f] == "inside_round").filter_map(|f| radii[*f]).fold(None, |m, r| Some(m.map_or(r, |m: f64| m.min(r))));
        let sharp = sharp_inside_corners(bodies, &keys, &accs, &roles, &faces);
        let loops = face_outline(bodies, keys[k].0, &accs[k].tris);
        let widest = largest_inscribed_radius(&loops, tol).map(|r| r * 2.0);
        let id = next_id(kind);
        let mut v = serde_json::Map::new();
        v.insert("depth_mm".into(), json!(r3(depth)));
        v.insert("floor_area_mm2".into(), json!(r3(accs[k].area)));
        if let Some(w) = widest {
            v.insert("largest_roughing_tool_mm".into(), json!(r3(w)));
        }
        // Finishing the corners: a round tool leaves its own radius in an inside corner.
        let finish = if sharp > 0 { Some(0.0) } else { inside_r.map(|r| r * 2.0) };
        if let Some(f) = finish {
            v.insert("largest_finishing_tool_mm".into(), json!(r3(f)));
        }
        if let Some(r) = inside_r {
            v.insert("smallest_inside_radius_mm".into(), json!(r3(r)));
            note_tool(r * 2.0, format!("the {} mm inside corners of {id}", r3(r)));
        }
        if sharp > 0 {
            v.insert("sharp_inside_corners".into(), json!(sharp));
            issues.push(
                Issue::warning(
                    "sharp_inside_corner",
                    format!("{id} has {sharp} sharp inside corner(s): a round tool leaves its radius there; ask for a corner radius, or accept the round"),
                )
                .parameter(&id, sharp),
            );
        }
        let reach_tool = widest.into_iter().chain(finish.filter(|f| *f > 0.0)).fold(f64::INFINITY, f64::min);
        if reach_tool.is_finite() && reach_tool > 0.0 && depth / reach_tool > 4.0 {
            v.insert("depth_to_tool".into(), json!(r3(depth / reach_tool)));
            issues.push(
                Issue::warning(
                    "deep_narrow",
                    format!("{id} is {} mm deep for a Ø{} mm tool: it needs {} mm of reach and light cuts", r3(depth), r3(reach_tool), r3(depth + 2.0)),
                )
                .parameter(&id, r3(depth / reach_tool))
                .limit(4.0),
            );
        }
        features.push(Feature { id, kind, faces: faces.into_iter().map(face_ref).collect(), values: v });
    }

    // Round bosses.
    for k in 0..keys.len() {
        if taken[k] || !matches!(roles[k], "boss" | "top") {
            continue;
        }
        if roles[k] == "boss" {
            taken[k] = true;
            let d = radii[k].unwrap_or(0.0) * 2.0;
            let mut v = serde_json::Map::new();
            v.insert("diameter_mm".into(), json!(r3(d)));
            v.insert("height_mm".into(), json!(r3(accs[k].zmax - accs[k].zmin)));
            let id = next_id("boss");
            features.push(Feature { id, kind: "boss", faces: vec![face_ref(k)], values: v });
        }
    }
    for k in 0..keys.len() {
        if taken[k] || roles[k] != "top" {
            continue;
        }
        taken[k] = true;
        let mut v = serde_json::Map::new();
        v.insert("area_mm2".into(), json!(r3(accs[k].area)));
        let id = next_id("top");
        features.push(Feature { id, kind: "top", faces: vec![face_ref(k)], values: v });
    }

    // Free-form regions, by adjacency.
    for k in 0..keys.len() {
        if taken[k] || roles[k] != "freeform" {
            continue;
        }
        let mut faces = vec![k];
        taken[k] = true;
        let mut stack = vec![k];
        while let Some(f) = stack.pop() {
            for &n in adj.get(f).map_or(&[][..], |l| l.as_slice()) {
                if !taken[n] && roles[n] == "freeform" {
                    taken[n] = true;
                    faces.push(n);
                    stack.push(n);
                }
            }
        }
        let rmin = faces.iter().filter_map(|f| radii[*f]).fold(f64::INFINITY, f64::min);
        let (zmin, zmax) = faces.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), f| (a.min(accs[*f].zmin), b.max(accs[*f].zmax)));
        let mut v = serde_json::Map::new();
        v.insert("area_mm2".into(), json!(r3(faces.iter().map(|f| accs[*f].area).sum::<f64>())));
        v.insert("height_mm".into(), json!(r3(zmax - zmin)));
        v.insert("depth_mm".into(), json!(r3(top - zmin)));
        if rmin.is_finite() {
            v.insert("smallest_radius_mm".into(), json!(r3(rmin)));
            v.insert("largest_ball_tool_mm".into(), json!(r3(rmin * 2.0)));
        }
        let id = next_id("freeform");
        features.push(Feature { id, kind: "freeform", faces: faces.into_iter().map(face_ref).collect(), values: v });
    }

    // Faces no direction reaches: undercuts. Faces only reached from below: a second side.
    let mut under = Vec::new();
    let mut below_only = Vec::new();
    for k in 0..keys.len() {
        let r = &reach[k];
        let any = r.iter().any(|x| *x > 0.5);
        if !any && accs[k].area > flat * flat {
            under.push(k);
        } else if r.first().is_some_and(|x| *x <= 0.5) && r.get(1).is_some_and(|x| *x > 0.5) && roles[k] != "ceiling" {
            below_only.push(k);
        }
    }
    // Faces facing down above the bottom (and not already an undercut) are cut from below.
    let ceilings: Vec<usize> =
        (0..keys.len()).filter(|k| roles[*k] == "ceiling" && (accs[*k].zmin - bottom).abs() > flat && !under.contains(k) && reach[*k].get(1).is_some_and(|x| *x > 0.5)).collect();
    if !under.is_empty() {
        let id = next_id("undercut");
        let mut v = serde_json::Map::new();
        v.insert("faces".into(), json!(under.len()));
        v.insert("area_mm2".into(), json!(r3(under.iter().map(|k| accs[*k].area).sum::<f64>())));
        issues.push(
            Issue::warning(
                "undercut",
                format!("{id}: {} face(s) no straight tool reaches from any side; a T-slot or dovetail cutter, the 4th axis, or a design change", under.len()),
            )
            .parameter(&id, under.len()),
        );
        features.push(Feature { id, kind: "undercut", faces: under.iter().map(|k| face_ref(*k)).collect(), values: v });
    }
    if !below_only.is_empty() || !ceilings.is_empty() {
        let n = below_only.len() + ceilings.len();
        issues.push(
            Issue::info("needs_second_side", format!("{n} face(s) are only reached from below: a second setup, the part turned over"))
                .suggest(json!({"command": "cam.setup.create", "flip": "x"})),
        );
    }

    // Directions: the fewest that reach every face reached at all (greedy).
    // The flat bottom stays as the stock's own unless the plan says otherwise: it asks no side
    // of its own.
    let on_bottom = |k: usize| roles[k] == "ceiling" && (accs[k].zmin - bottom).abs() <= flat;
    let reachable: Vec<usize> = (0..keys.len()).filter(|k| !on_bottom(*k) && reach[*k].iter().any(|x| *x > 0.5)).collect();
    let mut left: Vec<usize> = reachable.clone();
    let mut directions: Vec<&'static str> = Vec::new();
    while !left.is_empty() && directions.len() < DIRECTIONS.len() {
        let best = (0..DIRECTIONS.len())
            .map(|d| (d, left.iter().filter(|k| reach[**k].get(d).is_some_and(|x| *x > 0.5)).map(|k| accs[*k].area).sum::<f64>()))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        let Some((d, gain)) = best else { break };
        if gain <= 0.0 {
            break;
        }
        directions.push(DIRECTIONS[d].0);
        left.retain(|k| reach[*k].get(d).is_none_or(|x| *x <= 0.5));
    }
    let total_area: f64 = accs.iter().map(|a| a.area).sum::<f64>().max(1e-12);
    let access: Vec<serde_json::Value> = DIRECTIONS
        .iter()
        .enumerate()
        .map(|(d, (name, _))| {
            let share = (0..keys.len()).map(|k| accs[k].area * reach[k].get(d).copied().unwrap_or(0.0)).sum::<f64>() / total_area;
            let full = (0..keys.len()).filter(|k| reach[*k].get(d).is_some_and(|x| *x > 0.99)).count();
            json!({"direction": name, "area_share": r3(share), "faces_reached": full})
        })
        .collect();
    let axes = machine.axis_count();
    if axes <= 3 && directions.len() > 1 {
        issues.push(
            Issue::info(
                "setups_needed",
                format!("a 3-axis machine reaches this part from {} sides: {} setups ({})", directions.len(), directions.len(), directions.join(", ")),
            )
            .parameter("directions", directions.len()),
        );
    }
    if let Kinematics::FourAxis { rotary } = &machine.kinematics {
        let around = rotary.direction.normalized().unwrap_or(Vec3::new(1.0, 0.0, 0.0));
        let sides: Vec<&str> = directions.iter().copied().filter(|d| !matches!(*d, "+x" | "-x") || around.x.abs() < 0.5).collect();
        if !sides.is_empty() {
            issues.push(Issue::info("rotary_reach", format!("the {} turns the part to reach {}", rotary.name, sides.join(", "))));
        }
    }

    // What the features ask of the tools, against the machine.
    let mut limits = serde_json::Map::new();
    let deepest = features.iter().filter_map(|f| f.values.get("depth_mm").and_then(|v| v.as_f64())).fold(0.0, f64::max);
    limits.insert("deepest_feature_mm".into(), json!(r3(deepest)));
    limits.insert("tool_reach_needed_mm".into(), json!(r3(deepest + 2.0)));
    if let Some((d, why)) = &smallest_tool {
        limits.insert("smallest_tool_mm".into(), json!(r3(*d)));
        limits.insert("smallest_tool_for".into(), json!(why));
        if machine.spindle_rpm_max > 0.0 && material_speed > 0.0 && *d > 0.0 {
            let rpm = material_speed * 1000.0 / (std::f64::consts::PI * d);
            limits.insert("rpm_for_smallest_tool".into(), json!(rpm.round()));
            if rpm > machine.spindle_rpm_max {
                issues.push(
                    Issue::info(
                        "spindle_too_slow_for_small_tool",
                        format!(
                            "a Ø{} mm tool wants {} rev/min for {} m/min; the machine turns {} rev/min at most, so feed it lighter",
                            r3(*d),
                            rpm.round(),
                            material_speed,
                            machine.spindle_rpm_max.round()
                        ),
                    )
                    .parameter("spindle_rpm", rpm.round())
                    .limit(machine.spindle_rpm_max),
                );
            }
        }
    }
    if !machine.collets.is_empty() {
        let largest = machine.collets.iter().copied().fold(0.0, f64::max);
        limits.insert("largest_shank_mm".into(), json!(largest));
        limits.insert("collets_mm".into(), json!(machine.collets));
    }
    if let crate::machine::ToolChanger::Automatic { slots } = machine.tool_changer {
        limits.insert("tool_slots".into(), json!(slots));
    }
    limits.insert("axes".into(), json!(axes));

    // Fit.
    let mut fit = serde_json::Map::new();
    fit.insert("part_size_mm".into(), json!([r3(size.x), r3(size.y), r3(size.z)]));
    let stock_size = stock.map(|(a, b)| b - a);
    if let Some(s) = stock_size {
        fit.insert("stock_size_mm".into(), json!([r3(s.x), r3(s.y), r3(s.z)]));
    }
    let wa = machine.work_area;
    if wa.x > 0.0 && wa.y > 0.0 {
        fit.insert("work_area_mm".into(), json!([wa.x, wa.y, wa.z]));
        let check = stock_size.unwrap_or(size);
        let fits = check.x <= wa.x + 1e-6 && check.y <= wa.y + 1e-6 && (wa.z <= 0.0 || check.z <= wa.z + 1e-6);
        let turned = check.y <= wa.x + 1e-6 && check.x <= wa.y + 1e-6;
        fit.insert("fits".into(), json!(fits || turned));
        if !fits && turned {
            issues.push(Issue::info("turn_to_fit", "the part fits the work area turned 90° about the tool axis"));
        } else if !fits {
            issues.push(
                Issue::error(
                    "exceeds_work_area",
                    format!(
                        "the {} is {} × {} × {} mm; the {} works within {} × {} × {} mm",
                        if stock_size.is_some() { "stock" } else { "part" },
                        r3(check.x),
                        r3(check.y),
                        r3(check.z),
                        machine.name,
                        wa.x,
                        wa.y,
                        wa.z
                    ),
                )
                .parameter("size_mm", json!([r3(check.x), r3(check.y), r3(check.z)]))
                .limit(json!([wa.x, wa.y, wa.z])),
            );
        }
    }
    if machine.gantry_clearance > 0.0 {
        let height = stock_size.map_or(size.z, |s| s.z);
        let room = machine.gantry_clearance - height;
        fit.insert("room_for_tool_mm".into(), json!(r3(room)));
        if room < deepest + 2.0 + 10.0 {
            issues.push(
                Issue::warning(
                    "gantry_room",
                    format!(
                        "{} mm under the gantry less {} mm of stock leaves {} mm for the tool and holder; the deepest feature needs {} mm of reach",
                        machine.gantry_clearance,
                        r3(height),
                        r3(room),
                        r3(deepest + 2.0)
                    ),
                )
                .parameter("room_mm", r3(room)),
            );
        }
    }

    let face_facts: Vec<FaceFacts> = (0..keys.len())
        .map(|k| FaceFacts {
            body: face_ref(k).0,
            face: keys[k].1,
            role: roles[k],
            area_mm2: r3(accs[k].area),
            z_min: r3(accs[k].zmin),
            z_max: r3(accs[k].zmax),
            radius_mm: radii[k].map(r3),
            slope_deg: slopes[k].map(r3),
            reached_from: DIRECTIONS.iter().enumerate().filter_map(|(d, (n, _))| reach[k].get(d).filter(|x| **x > 0.0).map(|x| (*n, r3(*x)))).collect(),
        })
        .collect();
    let mut values = serde_json::Map::new();
    for kind in ["pocket", "step", "hole", "boss", "island", "freeform", "undercut", "top"] {
        let n = features.iter().filter(|f| f.kind == kind).count();
        if n > 0 {
            values.insert(format!("{kind}s"), json!(n));
        }
    }
    values.insert("faces".into(), json!(keys.len()));
    let flat_bottom = (0..keys.len()).any(on_bottom);
    values.insert("flat_bottom".into(), json!(flat_bottom));
    values.insert("directions".into(), json!(directions));
    let info = format!(
        "{} feature(s) on {} face(s); reached from {}",
        features.len(),
        keys.len(),
        if directions.is_empty() { "nowhere".to_string() } else { directions.join(", ") }
    );
    Ok(Analysis { features, faces: face_facts, access, directions, limits, fit, notice: Notice { info: Some(info), values, issues } })
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// The sum of `f` over a face's triangle normals (area weighted).
fn face_normal_sum(bodies: &[BodyInput], bi: usize, tris: &[usize], f: impl Fn(Vec3) -> f64) -> f64 {
    let Some(b) = bodies.get(bi) else { return 0.0 };
    tris.iter()
        .filter_map(|t| b.mesh.triangles.get(*t).and_then(|t| b.mesh.tri(t)))
        .map(|[p, q, r]| {
            let n = (q - p).cross(r - p);
            f(n) * 0.5
        })
        .sum()
}

/// Whether `p` is inside a closed mesh: the parity of the triangles a ray from it crosses (the
/// ray leans a little off the axes, so it does not run along an edge).
fn inside(mesh: &Mesh, p: Vec3) -> bool {
    let d = Vec3::new(0.00131, 0.00071, 1.0);
    let mut n = 0usize;
    for t in &mesh.triangles {
        let Some([a, b, c]) = mesh.tri(t) else { continue };
        let (e1, e2) = (b - a, c - a);
        let h = d.cross(e2);
        let det = e1.dot(h);
        if det.abs() < 1e-14 {
            continue;
        }
        let f = 1.0 / det;
        let sv = p - a;
        let u = f * sv.dot(h);
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = sv.cross(e1);
        let v = f * d.dot(q);
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        if f * e2.dot(q) > 1e-9 {
            n += 1;
        }
    }
    n % 2 == 1
}

/// How far round its axis a face reaches (radians): a full hole or boss is about 2π.
fn angular_span(bodies: &[BodyInput], bi: usize, tris: &[usize], origin: Vec3, axis: Vec3) -> f64 {
    let Some(b) = bodies.get(bi) else { return 0.0 };
    let u = axis.any_perp().normalized().unwrap_or(Vec3::new(1.0, 0.0, 0.0));
    let v = axis.cross(u);
    let mut angles: Vec<f64> = tris
        .iter()
        .filter_map(|t| b.mesh.triangles.get(*t).and_then(|t| b.mesh.tri(t)))
        .flat_map(|ps| ps.into_iter())
        .map(|p| {
            let d = p - origin;
            d.dot(v).atan2(d.dot(u))
        })
        .filter(|a| a.is_finite())
        .collect();
    if angles.len() < 2 {
        return 0.0;
    }
    angles.sort_by(f64::total_cmp);
    // The span is the circle less the largest gap between neighbouring angles.
    let mut gap = angles.first().zip(angles.last()).map_or(0.0, |(a, b)| a + std::f64::consts::TAU - b);
    for w in angles.windows(2) {
        gap = gap.max(w[1] - w[0]);
    }
    std::f64::consts::TAU - gap
}

/// Inside corners between two walls of `faces` with no round between them.
fn sharp_inside_corners(bodies: &[BodyInput], keys: &[(usize, usize)], accs: &[Acc], roles: &[&str], faces: &[usize]) -> usize {
    let mut n = 0;
    for (bi, b) in bodies.iter().enumerate() {
        for (e, fs) in b.mesh.edge_faces.iter().enumerate() {
            let [f1, f2] = fs.as_slice() else { continue };
            let k1 = faces.iter().copied().find(|k| keys.get(*k) == Some(&(bi, *f1 as usize)));
            let k2 = faces.iter().copied().find(|k| keys.get(*k) == Some(&(bi, *f2 as usize)));
            let (Some(k1), Some(k2)) = (k1, k2) else { continue };
            if roles.get(k1) != Some(&"wall") || roles.get(k2) != Some(&"wall") {
                continue;
            }
            // A vertical edge…
            let Some(pts) = b.mesh.edges.get(e) else { continue };
            let (Some(a), Some(c)) = (pts.first(), pts.last()) else { continue };
            let along = (*c - *a).normalized();
            if !along.is_some_and(|d| d.z.abs() > 0.99) {
                continue;
            }
            // …where each wall stands in front of the other: an inside corner.
            let (n1, n2) = (accs[k1].normal.normalized(), accs[k2].normal.normalized());
            let (Some(n1), Some(n2)) = (n1, n2) else { continue };
            if (accs[k2].centroid - accs[k1].centroid).dot(n1) > 1e-6 && (accs[k1].centroid - accs[k2].centroid).dot(n2) > 1e-6 {
                n += 1;
            }
        }
    }
    n
}

/// A planar face's outline in XY: its boundary edges (those of one of its triangles only)
/// chained into loops, normalized.
fn face_outline(bodies: &[BodyInput], bi: usize, tris: &[usize]) -> Vec<Poly> {
    let Some(b) = bodies.get(bi) else { return Vec::new() };
    let key = |p: Vec3| ((p.x * 1e5).round() as i64, (p.y * 1e5).round() as i64);
    let mut count: HashMap<((i64, i64), (i64, i64)), (i32, Vec2, Vec2)> = HashMap::new();
    for t in tris {
        let Some(tri) = b.mesh.triangles.get(*t).and_then(|t| b.mesh.tri(t)) else { continue };
        for (p, q) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            let (kp, kq) = (key(p), key(q));
            if kp == kq {
                continue;
            }
            let (a, c, sign) = if kp < kq { (kp, kq, 1) } else { (kq, kp, -1) };
            let (pa, pc) = if sign > 0 { (Vec2::new(p.x, p.y), Vec2::new(q.x, q.y)) } else { (Vec2::new(q.x, q.y), Vec2::new(p.x, p.y)) };
            let e = count.entry((a, c)).or_insert((0, pa, pc));
            e.0 += sign;
        }
    }
    // Directed boundary edges: those whose two uses don't cancel.
    let mut next: HashMap<(i64, i64), Vec<((i64, i64), Vec2)>> = HashMap::new();
    for ((a, c), (s, pa, pc)) in &count {
        match s.signum() {
            1 => next.entry(*a).or_default().push((*c, *pc)),
            -1 => next.entry(*c).or_default().push((*a, *pa)),
            _ => {}
        }
        let _ = pa;
    }
    let mut starts: HashMap<(i64, i64), Vec2> = HashMap::new();
    for ((a, c), (s, pa, pc)) in &count {
        match s.signum() {
            1 => {
                starts.insert(*a, *pa);
            }
            -1 => {
                starts.insert(*c, *pc);
            }
            _ => {}
        }
    }
    let mut loops: Vec<Poly> = Vec::new();
    let mut guard = 0usize;
    let mut keys: Vec<(i64, i64)> = next.keys().copied().collect();
    keys.sort_unstable();
    for k0 in keys {
        while next.get(&k0).is_some_and(|l| !l.is_empty()) {
            let mut poly: Poly = vec![starts.get(&k0).copied().unwrap_or(Vec2::new(0.0, 0.0))];
            let mut k = k0;
            loop {
                guard += 1;
                if guard > 4_000_000 {
                    return polyline::normalize_set(&loops);
                }
                let Some((nk, p)) = next.get_mut(&k).and_then(|l| l.pop()) else { break };
                k = nk;
                if k == k0 {
                    break;
                }
                poly.push(p);
            }
            if poly.len() >= 3 {
                loops.push(poly);
            }
        }
    }
    polyline::normalize_set(&loops)
}

/// The radius of the largest circle inside a region (to `tol`), by shrinking it until it is
/// gone.
fn largest_inscribed_radius(region: &[Poly], tol: f64) -> Option<f64> {
    if region.is_empty() {
        return None;
    }
    let area: f64 = region.iter().map(|p| polyline::signed_area(p)).sum();
    if area <= 0.0 {
        return None;
    }
    let (mut lo, mut hi) = (0.0_f64, area.sqrt());
    for _ in 0..24 {
        let mid = (lo + hi) * 0.5;
        let alive = crate::offset::offset_set(region, -mid, tol).map(|r| !r.is_empty());
        match alive {
            Ok(true) => lo = mid,
            Ok(false) => hi = mid,
            Err(_) => return (lo > 0.0).then_some(lo),
        }
        if hi - lo < tol {
            break;
        }
    }
    (lo > 0.0).then_some(lo)
}

/// For each face, the share of its area each of the six directions reaches: a point is
/// reached from a direction when nothing of the part lies above it along that direction, just
/// in front of the face.
fn access(bodies: &[BodyInput], keys: &[(usize, usize)], accs: &[Acc], lo: Vec3, hi: Vec3) -> Result<Vec<Vec<f64>>> {
    let mut all = Mesh::default();
    for b in bodies {
        let base = all.positions.len() as u32;
        all.positions.extend_from_slice(&b.mesh.positions);
        all.triangles.extend(b.mesh.triangles.iter().map(|t| [t[0] + base, t[1] + base, t[2] + base]));
    }
    let size = (hi - lo).len().max(1e-6);
    let mut out = vec![vec![0.0; DIRECTIONS.len()]; keys.len()];
    let probe = Tool::new(0, "probe", ToolKind::FlatEnd, 1e-3);
    for (d, (_, dir)) in DIRECTIONS.iter().enumerate() {
        let z = Vec3::new(dir[0], dir[1], dir[2]);
        let frame = Frame::from_axis(Vec3::new(0.0, 0.0, 0.0), z)?;
        let local = Mesh {
            positions: all.positions.iter().map(|p| frame.to_local(*p)).collect(),
            triangles: all.triangles.clone(),
            ..Default::default()
        };
        let drop = DropMesh::new(&local);
        let (mut llo, mut lhi) = (Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY));
        let mut floor = f64::INFINITY;
        for p in &local.positions {
            llo = Vec2::new(llo.x.min(p.x), llo.y.min(p.y));
            lhi = Vec2::new(lhi.x.max(p.x), lhi.y.max(p.y));
            floor = floor.min(p.z);
        }
        let cell = ((lhi.x - llo.x).max(lhi.y - llo.y) / MAX_GRID as f64).max(size * 1e-4);
        let floor = floor - 1.0;
        // A cache of the part's height above each cell.
        let nx = (((lhi.x - llo.x) / cell).ceil() as usize + 1).min(MAX_GRID + 2);
        let ny = (((lhi.y - llo.y) / cell).ceil() as usize + 1).min(MAX_GRID + 2);
        let mut heights: Vec<Option<f64>> = vec![None; nx * ny];
        let mut height_at = |p: Vec2| -> f64 {
            // Beside the part, nothing is above the point.
            if p.x < llo.x - cell * 0.5 || p.y < llo.y - cell * 0.5 || p.x > lhi.x + cell * 0.5 || p.y > lhi.y + cell * 0.5 {
                return floor;
            }
            let i = (((p.x - llo.x) / cell).round().max(0.0) as usize).min(nx - 1);
            let j = (((p.y - llo.y) / cell).round().max(0.0) as usize).min(ny - 1);
            let idx = j * nx + i;
            if let Some(Some(h)) = heights.get(idx) {
                return *h;
            }
            let at = Vec2::new(llo.x + i as f64 * cell, llo.y + j as f64 * cell);
            let h = drop.drop_z(&probe, at, floor).map_or(floor, |c| c.z);
            if let Some(slot) = heights.get_mut(idx) {
                *slot = Some(h);
            }
            h
        };
        for (k, (bi, _)) in keys.iter().enumerate() {
            let Some(b) = bodies.get(*bi) else { continue };
            let acc = &accs[k];
            let (mut seen, mut total) = (0.0, 0.0);
            for t in acc.tris.iter().take(4000) {
                let Some([p, q, r]) = b.mesh.triangles.get(*t).and_then(|t| b.mesh.tri(t)) else { continue };
                let n2 = (q - p).cross(r - p);
                let a = n2.len() * 0.5;
                let Some(n) = n2.normalized() else { continue };
                total += a;
                // Facing away from the tool: not reached.
                if n.dot(z) < -0.02 {
                    continue;
                }
                // Just in front of the face, is the part's top along this direction below it?
                let c = (p + q + r) * (1.0 / 3.0) + n * (cell * 1.5);
                let l = frame.to_local(c);
                if l.z >= height_at(Vec2::new(l.x, l.y)) - cell * 0.5 {
                    seen += a;
                }
            }
            if let Some(row) = out.get_mut(k)
                && let Some(x) = row.get_mut(d)
            {
                *x = if total > 0.0 { seen / total } else { 0.0 };
            }
        }
    }
    Ok(out)
}
