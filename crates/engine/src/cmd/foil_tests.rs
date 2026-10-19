use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn volume(s: &Session, body: &str) -> f64 {
    solidcraft_kernel::measure(&s.model.state().body(body).unwrap().body).unwrap().volume
}

/// The wing of `reference/solidcraft/eval/wing_batch.py` (efoil): five stations on a 400 mm
/// half span, quarter chord swept back 15 % (atan 0.15), 0.6 mm trailing edge. The batch of
/// 17 MCP steps gave a half-wing volume of 562,581 mm³.
fn wing_batch_params() -> Value {
    json!({
        "span": 800, "root_chord": 160, "tip_chord": 50,
        "stations": [
            {"fraction": 0.0,   "chord": 160, "camber": 3.0, "camber_pos": 40, "thickness": 12},
            {"fraction": 0.375, "chord": 150, "camber": 3.0, "camber_pos": 40, "thickness": 11},
            {"fraction": 0.7,   "chord": 125, "camber": 3.0, "camber_pos": 40, "thickness": 10},
            {"fraction": 0.9,   "chord": 90,  "camber": 2.5, "camber_pos": 40, "thickness": 9},
            {"fraction": 1.0,   "chord": 50,  "camber": 2.0, "camber_pos": 40, "thickness": 9}
        ],
        "te_thickness": 0.6, "sweep": 0.15f64.atan().to_degrees(), "pivot": 0.25
    })
}

#[test]
fn foil_wing_builds_the_wing_batch_half_wing() {
    let mut s = Session::default();
    let mut p = wing_batch_params();
    p["mirror"] = json!(false);
    let r = run(&mut s, "foil.wing", p);
    assert_eq!(r["stations"].as_array().unwrap().len(), 5);
    let v = volume(&s, "Wing");
    assert!((v - 562_581.0).abs() / 562_581.0 < 2e-3, "half wing volume {v}");
    // Straight sides between the stations give a little less (the batch script's estimate).
    let ruled = r["half_volume_ruled_mm3"].as_f64().unwrap();
    assert!((ruled - 553_787.0).abs() < 300.0, "ruled estimate {ruled}");
    // One sketch per station, the loft, and nothing else: an editable timeline.
    assert_eq!(s.doc.features.len(), 6);
    let pts: Vec<f64> = r["stations"].as_array().unwrap().iter().map(|st| st["y"].as_f64().unwrap()).collect();
    assert_eq!(pts, vec![0.0, 150.0, 280.0, 360.0, 400.0]);
    // Bounds: the root's leading edge at -0.25 x 160 = -40, the span from 0 to 400 along +Y,
    // the trailing edge furthest back near the 0.7 station (0.15 x 280 + 0.75 x 125 = 135.75).
    let m = solidcraft_kernel::measure(&s.model.state().body("Wing").unwrap().body).unwrap();
    assert!((m.bbox.min.x + 40.0).abs() < 0.1 && (m.bbox.max.x - 135.75).abs() < 1.5, "{:?}", m.bbox);
    assert!((m.bbox.max.y - 400.0).abs() < 1e-6 && m.bbox.min.y.abs() < 1e-6, "{:?}", m.bbox);
}

#[test]
fn foil_wing_mirrors_into_one_body_and_follows_a_span_parameter() {
    let mut s = Session::default();
    run(&mut s, "parameters.add", json!({"name": "span", "expression": "800 mm"}));
    let mut p = wing_batch_params();
    p["span"] = json!("span");
    run(&mut s, "foil.wing", p);
    let full = volume(&s, "Wing");
    assert!((full - 2.0 * 562_581.0).abs() / 562_581.0 < 4e-3, "full wing volume {full}");
    assert_eq!(s.model.state().bodies.len(), 1);
    run(&mut s, "parameters.change", json!({"name": "span", "expression": "1000 mm"}));
    let stretched = volume(&s, "Wing");
    assert!((stretched / full - 1.25).abs() < 2e-3, "span 800 -> 1000: {full} -> {stretched}");
}

#[test]
fn foil_wing_refuses_bad_stations_and_leaves_the_design_as_it_was() {
    let mut s = Session::default();
    let n = s.doc.features.len();
    for (bad, why) in [
        (json!({"span": 800, "root_chord": 160, "tip_chord": 50, "stations": [{"fraction": 0.5}, {"fraction": 1}]}), "root first"),
        (json!({"span": 800, "root_chord": 160, "tip_chord": 50, "stations": [{"fraction": 0}, {"fraction": 0}]}), "increasing"),
        (json!({"span": 0, "root_chord": 160, "tip_chord": 50}), "span"),
        (json!({"span": 800, "root_chord": 160}), "tip chord"),
    ] {
        assert!(s.execute("foil.wing", &bad).is_err(), "{why}");
        assert_eq!(s.doc.features.len(), n, "{why}: nothing added");
    }
    // Two stations by default: a plain tapered wing.
    run(&mut s, "foil.wing", json!({"span": 600, "root_chord": 120, "tip_chord": 60, "mirror": false}));
    assert!(volume(&s, "Wing") > 0.0);
}

#[test]
fn sketch_plane_origin_expressions_follow_their_parameter() {
    let mut s = Session::default();
    run(&mut s, "parameters.add", json!({"name": "h", "expression": "30 mm"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Base"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [-10, -10], "p1": [10, 10]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": {"origin": [0, 0, "h"], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}, "name": "Top"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [-10, -10], "p1": [10, 10]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.loft", json!({"sections": [{"sketch": "Base"}, {"sketch": "Top"}], "body_name": "Prism"}));
    assert!((volume(&s, "Prism") - 400.0 * 30.0).abs() < 1e-6);
    run(&mut s, "parameters.change", json!({"name": "h", "expression": "50 mm"}));
    assert!((volume(&s, "Prism") - 400.0 * 50.0).abs() < 1e-6);
    assert!(s.execute("sketch.create", &json!({"plane": {"origin": [0, 0, "nope"], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}})).is_err());
}

#[test]
fn loft_through_coplanar_sections_is_refused() {
    let mut s = Session::default();
    for name in ["A", "B"] {
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": name}));
        run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 10}));
        run(&mut s, "sketch.finish", json!({}));
    }
    let e = s.execute("solid.loft", &json!({"sections": [{"sketch": "A"}, {"sketch": "B"}]})).unwrap_err().to_string();
    assert!(e.contains("same plane"), "{e}");
    assert!(s.model.state().bodies.is_empty());
}

#[test]
fn foil_wing_station_sweep_drop_and_twist_move_the_sections() {
    let mut s = Session::default();
    let p = json!({"span": 800, "root_chord": 100, "tip_chord": 50, "mirror": false, "x_offset": 500, "z_offset": -20, "twist": -1,
        "stations": [{"fraction": 0}, {"fraction": 1, "sweep": 40, "drop": 30, "twist": -2}]});
    let r = run(&mut s, "foil.wing", p);
    let tip = &r["stations"][1];
    assert_eq!(tip["pivot"], json!([540.0, -50.0]));
    let m = solidcraft_kernel::measure(&s.model.state().body("Wing").unwrap().body).unwrap();
    // The root's leading edge is 25 mm ahead of its pivot at x 500 (turned 1° nose down); the tip 30 mm lower.
    assert!((m.bbox.min.x - 475.0).abs() < 0.5 && (m.bbox.max.x - 577.5).abs() < 1.0, "{:?}", m.bbox);
    assert!(m.bbox.min.z < -50.0 && m.bbox.min.z > -56.0, "{:?}", m.bbox);
    assert!(volume(&s, "Wing") > 0.0);
    assert!(s.execute("foil.wing", &json!({"span": 800, "root_chord": 100, "tip_chord": 50, "twist": 60})).is_err());
}

// ---- The parametric foil features

/// The Silk V2 1050 front wing of the tuned set (designs/jason/silk-1050-usurf-135-tuned.md),
/// without its winglets and tubercles: the brief's check set.
fn silk_1050() -> Value {
    json!({
        "targetArea": 1050, "aspectRatio": 9, "profile": "NACA4", "camber": 2.5, "camberPos": 40,
        "rootThickness": 12, "tipThickness": 10, "tipProfile": "NACA4", "tipCamber": 1.0, "tipCamberPos": 50, "blendFrom": 40,
        "chordShape": "SUPERELLIPSE", "superN": 1.4, "endRatio": 0.26, "planformAlign": 78, "sweepTip": 70, "sweepExp": 4,
        "tipDrop": 45, "dropLaw": "SCURVE", "dropFrom": 3, "dropTo": 91, "tipCap": "ROUND_PLAN", "capLength": 18, "capEnd": 25,
        "stations": 13, "teThickness": 0.8, "pointsPerSurface": 50
    })
}

/// Bounds and volume against the Onshape tool's (build 246) for the same dialog values.
fn check_against(s: &Session, body: &str, vol: f64, min: [f64; 3], max: [f64; 3], vol_tol: f64, tol: f64) {
    let m = solidcraft_kernel::measure(&s.model.state().body(body).unwrap().body).unwrap();
    println!("{body}: volume {} ({:+.4} %) bbox {:?}", m.volume, 100.0 * (m.volume / vol - 1.0), m.bbox);
    assert!((m.volume - vol).abs() / vol < vol_tol, "{body} volume {} vs Onshape {vol}", m.volume);
    let (a, b) = ([m.bbox.min.x, m.bbox.min.y, m.bbox.min.z], [m.bbox.max.x, m.bbox.max.y, m.bbox.max.z]);
    for i in 0..3 {
        assert!((a[i] - min[i]).abs() < tol && (b[i] - max[i]).abs() < tol, "{body} bounds {a:?} {b:?} vs Onshape {min:?} {max:?}");
    }
}

/// The Silk 1050 of the brief in the Onshape tool (the test4 copy of the Foil designer, Silk V2
/// studio, 2026-10-19): 1,013,913.8 mm³, (-37.657, -486.056, -47.598) to (152.090, 486.056, 12.798).
#[test]
fn foil_wing_feature_builds_the_silk_1050_as_onshape_does() {
    let mut s = Session::default();
    let t = std::time::Instant::now();
    let r = run(&mut s, "foil.wing_feature", json!({"values": silk_1050()}));
    println!("built in {:?}: {r}", t.elapsed());
    // Onshape's notice, word for word up to its roll note
    assert!(
        r["notice"]
            .as_str()
            .unwrap()
            .starts_with("Area 1050 cm² projected (1057 developed) · span 972 mm · AR 9 · root 151 mm · end 47 mm · MAC 118 mm"),
        "{r}"
    );
    check_against(&s, "Wing", 1_013_913.8, [-37.6575, -486.0556, -47.5982], [152.0899, 486.0556, 12.7978], 5e-4, 0.05);
    assert_eq!(s.doc.features.len(), 1);
}

/// The tuned Silk 1050 in full: winglets up (15 cm², 25°, 60 mm bend), tubercles 5 mm at 62 mm
/// from 18 to 82 %, Min tip 1 mm. Onshape: 1,013,172.4 mm³, (-37.657, -484.619, -47.598) to
/// (152.109, 484.619, 12.798), "winglets up 15 cm² each (14.4 projected), 9 mm high at 25°,
/// projected span 969 mm".
#[test]
fn foil_wing_winglets_and_tubercles_as_onshape_does() {
    let mut s = Session::default();
    let mut v = silk_1050();
    for (k, x) in [
        ("winglet", json!("UP")),
        ("wingletArea", json!(15)),
        ("wingletAngle", json!(25)),
        ("wingletRadius", json!(60)),
        ("bumpOn", json!(true)),
        ("bumpAmp", json!(5)),
        ("bumpPitch", json!(62)),
        ("bumpFrom", json!(18)),
        ("bumpTo", json!(82)),
        ("minTipOn", json!(true)),
    ] {
        v[k] = x;
    }
    let t = std::time::Instant::now();
    let r = run(&mut s, "foil.wing_feature", json!({"values": v}));
    println!("built in {:?}: {r}", t.elapsed());
    assert!(
        r["notice"].as_str().unwrap().contains(
            "winglets up 15 cm² each (14.4 projected), 9 mm high at 25°, projected span 969 mm · tip section NACA4 1% camber from 40% span"
        ),
        "{r}"
    );
    check_against(&s, "Wing", 1_013_172.44, [-37.6575, -484.619, -47.5982], [152.109, 484.619, 12.7978], 5e-4, 0.05);
}

/// The U Surf 135 stab on its 560 arm in the same studio: tubercles 1.5 mm at 30 mm from 15 to
/// 85 %, Min tip 1 mm (Onshape raised Cap end to 45 %): 39,324.3 mm³, (546.155, -174.284,
/// -2.652) to (601.542, 174.284, 2.854).
#[test]
fn foil_stabiliser_builds_the_u_surf_135_as_onshape_does() {
    let mut s = Session::default();
    let r = run(
        &mut s,
        "foil.stabiliser_feature",
        json!({"values": {
            "profile": "NACA4", "camber": 0, "rootThickness": 9.4, "tipThickness": 7.4, "targetArea": 135, "aspectRatio": 9,
            "chordShape": "SUPERELLIPSE", "superN": 1.4, "endRatio": 0.2, "minTipOn": true, "minTip": 1, "stations": 13,
            "planformAlign": 19, "sweepTip": 15, "sweepExp": 4, "tipDrop": 0, "dropLaw": "POWER", "dropExp": 1,
            "bumpOn": true, "bumpAmp": 1.5, "bumpPitch": 30, "bumpFrom": 15, "bumpTo": 85,
            "incidence": -1.5, "arm": 560, "height": 0, "tipCap": "ROUND_PLAN", "capLength": 8, "capEnd": 25,
            "teThickness": 0.6, "pointsPerSurface": 36
        }}),
    );
    let n = r["notice"].as_str().unwrap();
    assert!(
        n.starts_with(
            "Area 135 cm² projected · span 349 mm · AR 9 · root 55 mm · end 15 mm · MAC 43 mm · Cap end raised to 45 % for a 0.5 mm cap end"
        ),
        "{n}"
    );
    check_against(&s, "Stabiliser", 39_324.26, [546.155, -174.2843, -2.6522], [601.5423, 174.2843, 2.854], 2e-3, 0.05);
}

fn bbox(s: &Session, body: &str) -> solidcraft_geom::Aabb3 {
    solidcraft_kernel::measure(&s.model.state().body(body).unwrap().body).unwrap().bbox
}

#[test]
fn foil_features_rebuild_when_a_value_or_a_parameter_changes() {
    let mut s = Session::default();
    // A unit-less parameter: Area cm^2 is a plain number, as in the Onshape dialog.
    run(&mut s, "parameters.add", json!({"name": "area", "expression": "800", "unit": ""}));
    let mut v = silk_1050();
    v["targetArea"] = json!("area");
    v["pointsPerSurface"] = json!(24);
    v["stations"] = json!(7);
    run(&mut s, "foil.wing_feature", json!({"values": v}));
    // span = sqrt(AR x area): 848.5 mm at 800 cm²
    let b = bbox(&s, "Wing");
    assert!((b.max.y - 424.26).abs() < 0.05, "{b:?}");
    run(&mut s, "parameters.change", json!({"name": "area", "expression": "1250"}));
    let b = bbox(&s, "Wing");
    assert!((b.max.y - 530.33).abs() < 0.05, "a parameter drives the wing: {b:?}");
    // An option changed through foil.set: a linear taper, flat tip, no drop.
    let r =
        run(&mut s, "foil.set", json!({"feature": "FoilWing1", "values": {"chordShape": "LINEAR", "tipRatio": 0.4, "tipCap": "FLAT", "tipDrop": 0}}));
    assert!(r["notice"].as_str().unwrap().contains("tip "), "{r}");
    let b = bbox(&s, "Wing");
    assert!(b.min.z > -15.0, "the drop is gone: {b:?}");
    // Out of range: refused with the field's own words.
    let e = s.execute("foil.set", &json!({"feature": "FoilWing1", "values": {"aspectRatio": 40}})).unwrap_err().to_string();
    assert!(e.contains("Aspect ratio must be within 2…20"), "{e}");
    // timeline.edit reaches the values too.
    run(&mut s, "timeline.edit", json!({"feature": "FoilWing1", "set": {"values": {"aspectRatio": "10"}}}));
    let b = bbox(&s, "Wing");
    assert!((b.max.y - (10.0f64 * 125_000.0).sqrt() / 2.0).abs() < 0.05, "{b:?}");
}

#[test]
fn foil_stabiliser_sits_on_its_arm_and_follows_a_linked_wing() {
    let mut s = Session::default();
    run(&mut s, "foil.wing_feature", json!({"values": {"targetArea": 1000, "aspectRatio": 8, "stations": 7, "pointsPerSurface": 24}}));
    let r = run(
        &mut s,
        "foil.stabiliser_feature",
        json!({"values": {"stabLink": "LINKED", "arm": 600, "height": 10, "stations": 7, "pointsPerSurface": 24}}),
    );
    // linked: 15 % of the wing, AR 8 - 1.5
    let n = r["notice"].as_str().unwrap();
    assert!(n.starts_with("Area 150 cm²") && n.contains("AR 6.5"), "{n}");
    let b = bbox(&s, "Stabiliser");
    let span = (6.5f64 * 15_000.0).sqrt();
    assert!((b.max.y - span / 2.0).abs() < 0.05, "{b:?}");
    assert!(b.min.x > 550.0 && b.max.x < 720.0, "on its arm: {b:?}");
    // The wing changes: the stab follows.
    run(&mut s, "foil.set", json!({"feature": "FoilWing1", "values": {"targetArea": 1200}}));
    let b = bbox(&s, "Stabiliser");
    assert!((b.max.y - (6.5f64 * 18_000.0).sqrt() / 2.0).abs() < 0.05, "{b:?}");
}

#[test]
fn foil_propeller_builds_a_b_series_propeller() {
    let mut s = Session::default();
    let t = std::time::Instant::now();
    let r = run(&mut s, "foil.propeller_feature", json!({"values": {"sectionCount": 12}}));
    println!("built in {:?}: {r}", t.elapsed());
    let m = solidcraft_kernel::measure(&s.model.state().body("Propeller").unwrap().body).unwrap();
    println!("volume {} bbox {:?}", m.volume, m.bbox);
    // 140 mm: the blade tips reach 70 mm from the shaft
    let reach = m.bbox.max.x.max(-m.bbox.min.x).max(m.bbox.max.y).max(-m.bbox.min.y);
    assert!((reach - 70.0).abs() < 1.0, "{:?}", m.bbox);
    assert!(m.volume > 20_000.0, "{}", m.volume);
}

#[test]
fn foil_features_round_trip_through_a_saved_design() {
    let mut s = Session::default();
    let mut v = silk_1050();
    v["pointsPerSurface"] = json!(20);
    v["stations"] = json!(5);
    v["camber"] = json!("2.5 + 0");
    run(&mut s, "foil.wing_feature", json!({"values": v}));
    run(&mut s, "foil.stabiliser_feature", json!({"values": {"stations": 5, "pointsPerSurface": 20, "profile": "MH18", "invertSection": true}}));
    let dir = std::env::temp_dir().join(format!("foil_rt_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("set.solidcraft");
    run(&mut s, "file.save_as", json!({"path": path.to_str().unwrap()}));
    let before: Vec<_> = s.doc.features.iter().map(|f| f.kind.clone()).collect();
    let vol = volume(&s, "Wing");
    let mut t = Session::default();
    run(&mut t, "doc.open", json!({"path": path.to_str().unwrap()}));
    let after: Vec<_> = t.doc.features.iter().map(|f| f.kind.clone()).collect();
    assert_eq!(before, after);
    assert!((volume(&t, "Wing") - vol).abs() < 1e-6 * vol);
    assert!(t.model.state().body("Stabiliser").is_some());
    std::fs::remove_dir_all(&dir).ok();
}
