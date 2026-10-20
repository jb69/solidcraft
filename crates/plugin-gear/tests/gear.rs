//! The gear plugin compiled in: the sizes standard proportions give, the volume of the tooth
//! outline from its closed-form area, helical twist, profile shift against undercut, gears in
//! mesh at their centre distance with teeth that fit, with a little backlash (and overlap when turned off their phase),
//! a ring and its pinion, and the errors and issues it reports. Every expected number is
//! analytic.

use std::f64::consts::PI;

use serde_json::{Value, json};
use solidcraft_engine::Session;

fn session() -> Session {
    solidcraft_plugin_gear::register();
    Session::default()
}

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn measure(s: &Session, body: &str) -> solidcraft_kernel::BodyMeasure {
    solidcraft_kernel::measure(&s.model.state().body(body).unwrap_or_else(|| panic!("no body {body}")).body).expect("measure")
}

fn inv(a: f64) -> f64 {
    a.tan() - a
}

/// The area inside an involute tooth outline of `z` teeth, from ½∮r²dθ: per tooth the root arc,
/// the tip arc and two flanks, each flank r_b²·t³/6 between its roll parameters t = tan α_r
/// (radial lines below the base circle add nothing). `s` is the arc thickness at the pitch
/// circle of radius `rp`, `alpha` the (transverse) pressure angle, the outline between `r_in`
/// and `r_out`.
fn outline_area(z: f64, rp: f64, alpha: f64, s: f64, r_in: f64, r_out: f64) -> f64 {
    let rb = rp * alpha.cos();
    let half = |r: f64| s / (2.0 * rp) + inv(alpha) - inv((rb / r).min(1.0).acos());
    let t = |r: f64| ((r / rb).powi(2) - 1.0).max(0.0).sqrt();
    let rs = r_in.max(rb);
    let tau = 2.0 * PI / z;
    z * (0.5 * r_in * r_in * (tau - 2.0 * half(rs)) + r_out * r_out * half(r_out) + rb * rb * (t(r_out).powi(3) - t(rs).powi(3)) / 3.0)
}

/// A spur gear: pitch Ø z·m, tip Ø (z + 2)·m, root Ø (z − 2.5)·m; the volume is the outline's
/// area times the face width, less the bore.
#[test]
fn a_spur_gear_has_the_standard_sizes_and_its_outline_area() {
    let mut s = session();
    let r = run(&mut s, "plugin.add", json!({"plugin": "gear", "values": {"teeth": 24, "module": 2, "face_width": 10}, "body_name": "G"}));
    assert_eq!(r["notice"], "Spur gear, 24 teeth, module 2: pitch Ø 48.00, tip Ø 52.00, root Ø 43.00 mm", "{r}");
    assert_eq!(r["values"]["base_diameter_mm"], json!(((48.0 * 20f64.to_radians().cos()) * 1000.0).round() / 1000.0));
    let m = measure(&s, "G");
    assert!((m.bbox.max.x - 26.0).abs() < 1e-6, "tooth 0 on +X reaches the tip: {:?}", m.bbox);
    assert!((m.bbox.max.z - 10.0).abs() < 1e-9);
    let area = outline_area(24.0, 24.0, 20f64.to_radians(), PI, 21.5, 26.0);
    assert!((m.volume / (area * 10.0) - 1.0).abs() < 5e-4, "volume {} vs {}", m.volume, area * 10.0);

    run(&mut s, "plugin.set", json!({"feature": r["name"], "values": {"bore": 12}}));
    let with_bore = measure(&s, "G").volume;
    assert!((with_bore / ((area - PI * 36.0) * 10.0) - 1.0).abs() < 5e-4, "{with_bore}");
    let e = s.execute("plugin.set", &json!({"feature": r["name"], "values": {"bore": 41}})).expect_err("too big").to_string();
    assert!(e.contains("less than one module of rim") && e.contains("at most Ø39"), "{e}");
}

/// Few teeth are undercut: the issue names the profile shift that clears it,
/// x = 1 − z·sin²α/2, and with it the tip grows to (z + 2 + 2x)·m.
#[test]
fn few_teeth_report_undercut_and_the_shift_that_clears_it() {
    let mut s = session();
    let r = run(&mut s, "plugin.add", json!({"plugin": "gear", "values": {"teeth": 12, "module": 2, "face_width": 5}, "body_name": "P"}));
    let issue = r["issues"].as_array().and_then(|a| a.iter().find(|i| i["code"] == "undercut")).cloned().unwrap_or_else(|| panic!("{r}"));
    let x_min = 1.0 - 12.0 * 20f64.to_radians().sin().powi(2) / 2.0;
    assert_eq!(issue["suggestion"]["profile_shift"], json!((x_min * 100.0).ceil() / 100.0), "{issue}");
    let r = run(&mut s, "plugin.set", json!({"feature": r["feature"], "values": {"profile_shift": 0.3}}));
    assert!(!r["issues"].as_array().is_some_and(|a| a.iter().any(|i| i["code"] == "undercut")), "{r}");
    assert_eq!(r["values"]["tip_diameter_mm"], json!(29.2), "{r}");
    let m = measure(&s, "P");
    assert!((m.bbox.max.x - 14.6).abs() < 1e-6, "{:?}", m.bbox);
}

/// A helical gear: transverse module m / cos β, the teeth twisted by b·tan β / r over the face,
/// the volume the transverse outline's area times the width.
#[test]
fn a_helical_gear_is_its_transverse_outline_twisted_along_the_face() {
    let mut s = session();
    let beta = 15f64.to_radians();
    let r = run(
        &mut s,
        "plugin.add",
        json!({"plugin": "gear", "values": {"teeth": 20, "module": 2, "helix_angle": 15, "face_width": 12}, "body_name": "H"}),
    );
    let mt = 2.0 / beta.cos();
    let rp = 10.0 * mt;
    assert_eq!(r["values"]["pitch_diameter_mm"], json!((2.0 * rp * 1000.0).round() / 1000.0), "{r}");
    let twist = (12.0 * beta.tan() / rp).to_degrees();
    assert_eq!(r["values"]["twist_deg"], json!((twist * 1000.0).round() / 1000.0), "{r}");
    let at = (20f64.to_radians().tan() / beta.cos()).atan();
    let area = outline_area(20.0, rp, at, mt * PI / 2.0, rp - 2.5, rp + 2.0);
    let v = measure(&s, "H").volume;
    assert!((v / (area * 12.0) - 1.0).abs() < 2e-3, "volume {v} vs {}", area * 12.0);
}

fn interference(s: &mut Session) -> f64 {
    let r = run(s, "inspect.interference", json!({}));
    r["interferences"].as_array().map(|a| a.iter().filter_map(|i| i["volume"].as_f64().or(i["volume_mm3"].as_f64())).sum()).unwrap_or(0.0)
}

/// Turn a body about the Z axis through `c`.
fn turn(s: &mut Session, body: &str, c: [f64; 2], deg: f64) {
    run(s, "solid.move", json!({"bodies": [body], "translate": [-c[0], -c[1], 0]}));
    run(s, "solid.move", json!({"bodies": [body], "axis": [0, 0, 1], "angle": format!("{deg} deg")}));
    run(s, "solid.move", json!({"bodies": [body], "translate": [c[0], c[1], 0]}));
}

/// Two gears in mesh: the second sits at (z1 + z2)·m/2 in the mesh direction, its teeth centred
/// in the first's spaces (flanks (j/2)·cos α apart) (no overlap, with 0.1 mm backlash), and overlaps once turned off that phase; the
/// contact ratio is the textbook (√(ra1²−rb1²) + √(ra2²−rb2²) − a·sin α) / (π·m·cos α).
#[test]
fn gears_in_mesh_sit_at_their_centre_distance_with_the_teeth_between_each_other() {
    let mut s = session();
    run(
        &mut s,
        "plugin.add",
        json!({"plugin": "gear", "values": {"teeth": 18, "module": 2, "face_width": 6, "rotation": 7, "backlash": 0.1}, "body_name": "A"}),
    );
    let r = run(
        &mut s,
        "plugin.add",
        json!({"plugin": "gear", "values": {"teeth": 30, "module": 2, "face_width": 6, "mesh": "PREVIOUS", "mesh_angle": 30, "backlash": 0.1}, "body_name": "B"}),
    );
    assert_eq!(r["values"]["center_distance_mm"], json!(48.0), "{r}");
    let c = [48.0 * 30f64.to_radians().cos(), 48.0 * 30f64.to_radians().sin()];
    let b = measure(&s, "B");
    let mid = [(b.bbox.min.x + b.bbox.max.x) / 2.0, (b.bbox.min.y + b.bbox.max.y) / 2.0];
    assert!((mid[0] - c[0]).abs() < 1.0 && (mid[1] - c[1]).abs() < 1.0, "centre {mid:?} vs {c:?}");
    let a = 20f64.to_radians();
    let reach = |z: f64| ((z + 2.0).powi(2) - (z * a.cos()).powi(2)).sqrt();
    let ratio = (reach(18.0) + reach(30.0) - 48.0 * a.sin()) / (PI * 2.0 * a.cos());
    assert!((r["values"]["contact_ratio"].as_f64().unwrap_or(0.0) - ratio).abs() < 2e-3, "{r} vs {ratio}");
    assert!(r["notice"].as_str().unwrap_or_default().contains("meshes with the 18-tooth gear at 48.00 mm (ratio 1.667"), "{r}");
    let fit = interference(&mut s);
    assert!(fit < 1e-3, "the teeth fit: {fit} mm³ overlap");
    // Centred in the spaces: half the backlash each side, (j/2)·cos α along the line of action
    // (to 5 %, 0.002 mm: the flanks are splines fitted through 8 points of the involute).
    let g = gap(&mut s);
    assert!((g / (0.05 * a.cos()) - 1.0).abs() < 0.05, "flanks {g} mm apart");
    turn(&mut s, "B", c, 4.0);
    assert!(interference(&mut s) > 0.1, "turned off its phase it overlaps");
}

/// A profile-shifted pair moves apart to the working centre distance
/// a_w = a·cos α / cos α_w, inv α_w = inv α + 2 tan α (x1 + x2)/(z1 + z2), and still fits.
#[test]
fn a_shifted_pair_meshes_at_the_working_centre_distance() {
    let mut s = session();
    run(
        &mut s,
        "plugin.add",
        json!({"plugin": "gear", "values": {"teeth": 12, "module": 3, "profile_shift": 0.4, "face_width": 5, "backlash": 0.1}, "body_name": "A"}),
    );
    let r = run(
        &mut s,
        "plugin.add",
        json!({"plugin": "gear", "values": {"teeth": 25, "module": 3, "face_width": 5, "mesh": "PREVIOUS", "backlash": 0.1}, "body_name": "B"}),
    );
    let a = 20f64.to_radians();
    let target = inv(a) + 2.0 * a.tan() * 0.4 / 37.0;
    let (mut lo, mut hi) = (0.0f64, 1.2f64);
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if inv(mid) < target { lo = mid } else { hi = mid }
    }
    let aw = 37.0 * 3.0 / 2.0 * a.cos() / lo.cos();
    assert!((r["values"]["center_distance_mm"].as_f64().unwrap_or(0.0) - aw).abs() < 1e-3, "{r} vs {aw}");
    assert!(interference(&mut s) < 1e-3, "the shifted teeth fit");
}

/// A ring gear and a pinion inside it: centre distance (z_ring − z_pinion)·m/2, the ring's volume
/// the rim disc less the tooth spaces' outline (widened by half the backlash), the pinion's teeth in the ring's spaces.
#[test]
fn a_pinion_meshes_inside_a_ring_gear() {
    let mut s = session();
    let r = run(
        &mut s,
        "plugin.add",
        json!({"plugin": "gear", "values": {"kind": "INTERNAL", "teeth": 60, "module": 1.5, "rim": 6, "face_width": 8, "backlash": 0.1}, "body_name": "Ring"}),
    );
    assert!(r["notice"].as_str().unwrap_or_default().starts_with("Ring gear, 60 teeth"), "{r}");
    let rp = 45.0;
    let rim = rp + 1.25 * 1.5 + 6.0;
    // The spaces are half a circular pitch wide, plus half the 0.1 mm backlash.
    let spaces = outline_area(60.0, rp, 20f64.to_radians(), 1.5 * PI / 2.0 + 0.05, rp - 1.5, rp + 1.25 * 1.5);
    let v = measure(&s, "Ring").volume;
    assert!((v / ((PI * rim * rim - spaces) * 8.0) - 1.0).abs() < 5e-4, "ring volume {v}");
    let r = run(
        &mut s,
        "plugin.add",
        json!({"plugin": "gear", "values": {"teeth": 20, "module": 1.5, "face_width": 8, "mesh": "PREVIOUS", "mesh_angle": 90, "backlash": 0.1}, "body_name": "Pinion"}),
    );
    assert_eq!(r["values"]["center_distance_mm"], json!(30.0), "{r}");
    assert!(interference(&mut s) < 1e-3, "the pinion's teeth sit in the ring's spaces");
    turn(&mut s, "Pinion", [0.0, 30.0], 4.0);
    assert!(interference(&mut s) > 0.1, "turned off its phase it overlaps the ring");
}

/// What cannot be built is an error, what will not run well an issue with the change that
/// clears it.
#[test]
fn mismatched_gears_are_refused_and_wrong_hands_reported() {
    let mut s = session();
    let e = s.execute("plugin.add", &json!({"plugin": "gear", "values": {"mesh": "PREVIOUS"}})).expect_err("nothing to mesh with").to_string();
    assert!(e.contains("no gear feature before this one"), "{e}");
    run(
        &mut s,
        "plugin.add",
        json!({"plugin": "gear", "values": {"teeth": 20, "module": 2, "helix_angle": 20, "hand": "RIGHT", "face_width": 8, "backlash": 0.1}}),
    );
    let e = s
        .execute("plugin.add", &json!({"plugin": "gear", "values": {"module": 2.5, "helix_angle": 20, "mesh": "PREVIOUS"}}))
        .expect_err("module")
        .to_string();
    assert!(e.contains("module 2.5 cannot mesh with module 2"), "{e}");
    let r = run(
        &mut s,
        "plugin.add",
        json!({"plugin": "gear", "values": {"teeth": 31, "module": 2, "helix_angle": 20, "hand": "RIGHT", "face_width": 8, "mesh": "PREVIOUS", "backlash": 0.1}}),
    );
    let hand = r["issues"].as_array().and_then(|a| a.iter().find(|i| i["code"] == "helix_hand")).cloned().unwrap_or_else(|| panic!("{r}"));
    assert_eq!(hand["suggestion"]["hand"], "LEFT", "{hand}");
    let r = run(&mut s, "plugin.set", json!({"feature": r["feature"], "values": {"hand": "LEFT"}}));
    assert!(!r["issues"].as_array().is_some_and(|a| a.iter().any(|i| i["code"] == "helix_hand")), "{r}");
    assert!(interference(&mut s) < 1e-2, "opposite hands mesh along the whole face");
}

/// Turn a body about the line through `at` along `axis`.
fn turn_about(s: &mut Session, body: &str, at: [f64; 3], axis: [f64; 3], deg: f64) {
    run(s, "solid.move", json!({"bodies": [body], "translate": [-at[0], -at[1], -at[2]]}));
    run(s, "solid.move", json!({"bodies": [body], "axis": axis, "angle": format!("{deg} deg")}));
    run(s, "solid.move", json!({"bodies": [body], "translate": at}));
}

fn bevel(values: Value) -> Value {
    use solidcraft_plugin::Plugin;
    let plugin = solidcraft_plugin_gear::GearPlugin;
    let mut p = plugin.schema().defaults();
    p.extend(values.as_object().cloned().unwrap_or_default());
    p.insert("kind".into(), json!("BEVEL"));
    plugin.describe(&Value::Object(p), &json!({"published": {}})).expect("describe")
}

fn r3(v: f64) -> Value {
    json!((v * 1000.0).round() / 1000.0)
}

/// A straight bevel gear on its cone: tan δ = sin Σ / (z_mate/z + cos Σ), cone distance
/// R = r / sin δ, tip Ø m·(z + 2 cos δ) on the back cone, and the face b along the cone, so the
/// gear is b·cos δ high.
#[test]
fn a_bevel_gear_sits_on_its_pitch_cone() {
    let mut s = session();
    let r = run(
        &mut s,
        "plugin.add",
        json!({"plugin": "gear", "values": {"kind": "BEVEL", "teeth": 20, "mate_teeth": 40, "module": 3, "face_width": 15}, "body_name": "P"}),
    );
    let d = 0.5f64.atan();
    assert_eq!(r["values"]["pitch_cone_angle_deg"], r3(d.to_degrees()), "{r}");
    assert_eq!(r["values"]["cone_distance_mm"], r3(1.5 * 2000f64.sqrt()), "{r}");
    assert_eq!(r["values"]["tip_diameter_mm"], r3(3.0 * (20.0 + 2.0 * d.cos())), "{r}");
    assert!(r["notice"].as_str().unwrap_or_default().starts_with("Bevel gear, 20 teeth, module 3, for a 40-tooth mate at 90°"), "{r}");
    let m = measure(&s, "P");
    assert!((m.bbox.max.x - 1.5 * (20.0 + 2.0 * d.cos())).abs() < 1e-3, "the tips end on the back cone at the tip circle: {:?}", m.bbox);
    assert!((m.bbox.max.z - 15.0 * d.cos()).abs() < 1e-6 && m.bbox.min.z.abs() < 1e-6, "{:?}", m.bbox);

    // Another shaft angle: Σ = 60°, 18 and 24 teeth.
    let v = bevel(json!({"teeth": 18, "mate_teeth": 24, "shaft_angle": 60, "module": 2, "face_width": 8}));
    let d = (60f64.to_radians().sin()).atan2(24.0 / 18.0 + 60f64.to_radians().cos());
    assert_eq!(v["values"]["pitch_cone_angle_deg"], r3(d.to_degrees()), "{v}");
    assert_eq!(v["values"]["cone_distance_mm"], r3(18.0 / d.sin()), "{v}");
    // Few virtual teeth are undercut; a face over a third of the cone distance is flagged.
    let v = bevel(json!({"teeth": 9, "mate_teeth": 10, "module": 2, "face_width": 5}));
    assert!(v["issues"].as_array().is_some_and(|a| a.iter().any(|i| i["code"] == "undercut")), "{v}");
    let v = bevel(json!({"teeth": 20, "mate_teeth": 40, "module": 3, "face_width": 30}));
    let wide = v["issues"].as_array().and_then(|a| a.iter().find(|i| i["code"] == "face_too_wide")).cloned().unwrap_or_else(|| panic!("{v}"));
    assert_eq!(wide["suggestion"]["face_width"], json!((1.5 * 2000f64.sqrt() / 3.0 * 10.0).floor() / 10.0), "{wide}");
}

fn gap(s: &mut Session) -> f64 {
    let r = run(s, "inspect.clearance", json!({"nearest": 1}));
    r["nearest"].as_object().and_then(|n| n.values().next()).and_then(|v| v[0]["distance_mm"].as_f64()).unwrap_or_else(|| panic!("{r}"))
}

/// Two bevel gears at right angles: the second shares the first's apex, its axis square to the
/// first's, its back at its own apex height R·cos δ₂; its teeth sit in the first's spaces with a
/// gap (at most the normal backlash at the outer end, (j/2)·cos α, less towards the apex), and
/// turned off its phase it touches.
#[test]
fn a_bevel_pair_meshes_on_a_shared_apex() {
    let mut s = session();
    run(
        &mut s,
        "plugin.add",
        json!({"plugin": "gear", "values": {"kind": "BEVEL", "teeth": 10, "mate_teeth": 20, "module": 2, "face_width": 6, "backlash": 0.1}, "body_name": "Pinion"}),
    );
    let r = run(
        &mut s,
        "plugin.add",
        json!({"plugin": "gear", "values": {"kind": "BEVEL", "teeth": 20, "module": 2, "face_width": 6, "backlash": 0.1, "mesh": "PREVIOUS"}, "body_name": "Wheel"}),
    );
    let rr = 500f64.sqrt();
    let (d1, d2) = (0.5f64.atan(), 2f64.atan());
    assert_eq!(r["values"]["pitch_cone_angle_deg"], r3(d2.to_degrees()), "{r}");
    assert_eq!(r["values"]["ratio"], json!(2.0), "{r}");
    let apex = [0.0, 0.0, rr * d1.cos()];
    assert_eq!(r["values"]["apex_mm"], json!([0.0, 0.0, (apex[2] * 1000.0).round() / 1000.0]), "{r}");
    assert_eq!(r["values"]["axis"], json!([-1.0, 0.0, 0.0]), "{r}");
    let w = measure(&s, "Wheel");
    assert!((w.bbox.max.x - rr * d2.cos()).abs() < 1e-6, "the wheel's back at R·cos δ₂ from the apex: {:?}", w.bbox);
    let outer = 0.05 * 20f64.to_radians().cos();
    let g = gap(&mut s);
    assert!(g > 0.5 * outer * (rr - 6.0) / rr && g <= outer + 1e-4, "the teeth sit in the spaces: {g} mm apart");
    turn_about(&mut s, "Wheel", apex, [1.0, 0.0, 0.0], 1.0);
    assert!(gap(&mut s) < 1e-6, "turned off its phase it touches");

    let e = s
        .execute("plugin.add", &json!({"plugin": "gear", "values": {"teeth": 30, "module": 2, "mesh": "PREVIOUS"}}))
        .expect_err("spur on bevel")
        .to_string();
    assert!(e.contains("cannot mesh with a bevel gear"), "{e}");
    let e = s
        .execute("plugin.add", &json!({"plugin": "gear", "values": {"kind": "BEVEL", "teeth": 30, "module": 2, "mesh": "PREVIOUS"}}))
        .expect_err("wrong mate")
        .to_string();
    assert!(e.contains("made for a 10-tooth mate, this one has 30 teeth"), "{e}");
}
