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
