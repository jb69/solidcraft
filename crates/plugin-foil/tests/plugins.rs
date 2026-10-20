//! The foil plugins in a design: the planform they promise (span from area and aspect ratio, a
//! symmetric wing, a stabiliser on its arm), parameters driving them, a linked stabiliser
//! following its wing, options and bounds, save and reopen.

use serde_json::{Value, json};
use solidcraft_engine::Session;

fn session() -> Session {
    solidcraft_plugin_foil::register();
    Session::default()
}

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn measure(s: &Session, body: &str) -> solidcraft_kernel::BodyMeasure {
    solidcraft_kernel::measure(&s.model.state().body(body).unwrap_or_else(|| panic!("no body {body}")).body).unwrap()
}

/// A Silk-like 1050 cm², AR 9 wing: superellipse chord, swept and dropped tips, rounded caps.
fn wing_1050() -> Value {
    json!({
        "targetArea": 1050, "aspectRatio": 9, "profile": "NACA4", "camber": 2.5, "camberPos": 40,
        "rootThickness": 12, "tipThickness": 10, "tipProfile": "NACA4", "tipCamber": 1.0, "tipCamberPos": 50, "blendFrom": 40,
        "chordShape": "SUPERELLIPSE", "superN": 1.4, "endRatio": 0.26, "planformAlign": 78, "sweepTip": 70, "sweepExp": 4,
        "tipDrop": 45, "dropLaw": "SCURVE", "dropFrom": 3, "dropTo": 91, "tipCap": "ROUND_PLAN", "capLength": 18, "capEnd": 25,
        "stations": 13, "teThickness": 0.8, "pointsPerSurface": 50
    })
}

/// Span and chord: the planform solve gives span² = AR · area (the cap adds a little:
/// tests/planform.rs has the station-by-station numbers); the body is one piece, symmetric.
#[test]
fn a_wing_builds_the_planform_its_dialog_asks_for() {
    let mut s = session();
    let r = run(&mut s, "plugin.add", json!({"plugin": "foil-wing", "values": wing_1050(), "body_name": "Wing"}));
    let n = r["notice"].as_str().unwrap();
    assert!(n.starts_with("Area 1050 cm² projected") && n.contains("AR 9"), "{n}");
    let m = measure(&s, "Wing");
    assert!((m.bbox.max.y + m.bbox.min.y).abs() < 1e-6, "symmetric: {:?}", m.bbox);
    let span = 2.0 * m.bbox.max.y;
    assert!((span - 972.111).abs() < 0.05, "span {span}");
    assert!((m.bbox.min.z + 45.0).abs() < 5.0, "the tips drop 45 mm: {:?}", m.bbox);
    // A 1050 cm² wing about 11 % thick on a ~118 mm mean chord: about 1 litre.
    assert!(m.volume > 0.8e6 && m.volume < 1.2e6, "{}", m.volume);
    assert_eq!(s.model.state().bodies.len(), 1, "one body");
}

/// Winglets turned up take projected span; tubercles and a minimum tip leave the area.
#[test]
fn winglets_and_tubercles_change_the_wing_as_the_notice_says() {
    let mut s = session();
    let mut v = wing_1050();
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
    let r = run(&mut s, "plugin.add", json!({"plugin": "foil-wing", "values": v, "body_name": "Wing"}));
    let n = r["notice"].as_str().unwrap();
    assert!(n.contains("winglets up 15 cm² each"), "{n}");
    let m = measure(&s, "Wing");
    assert!(2.0 * m.bbox.max.y < 972.0, "the winglets take span: {:?}", m.bbox);
}

/// A stabiliser on its arm: span² = AR · area, behind the wing by the arm.
#[test]
fn a_stabiliser_sits_on_its_arm() {
    let mut s = session();
    run(
        &mut s,
        "plugin.add",
        json!({"plugin": "foil-stabiliser", "body_name": "Stab", "values": {
            "profile": "NACA4", "camber": 0, "rootThickness": 9.4, "tipThickness": 7.4, "targetArea": 135, "aspectRatio": 9,
            "chordShape": "SUPERELLIPSE", "superN": 1.4, "endRatio": 0.2, "minTipOn": true, "minTip": 1, "stations": 13,
            "planformAlign": 19, "sweepTip": 15, "sweepExp": 4, "tipDrop": 0, "dropLaw": "POWER", "dropExp": 1,
            "bumpOn": true, "bumpAmp": 1.5, "bumpPitch": 30, "bumpFrom": 15, "bumpTo": 85,
            "incidence": -1.5, "arm": 560, "height": 0, "tipCap": "ROUND_PLAN", "capLength": 8, "capEnd": 25,
            "teThickness": 0.6, "pointsPerSurface": 36
        }}),
    );
    let m = measure(&s, "Stab");
    assert!((m.bbox.max.y - (9.0f64 * 13_500.0).sqrt() / 2.0).abs() < 0.05, "{:?}", m.bbox);
    assert!(m.bbox.min.x > 520.0 && m.bbox.max.x < 620.0, "on its 560 arm: {:?}", m.bbox);
}

#[test]
fn a_wing_follows_its_parameters_and_a_linked_stab_follows_the_wing() {
    let mut s = session();
    run(&mut s, "parameters.add", json!({"name": "area", "expression": "1000", "unit": ""}));
    run(
        &mut s,
        "plugin.add",
        json!({"plugin": "foil-wing", "body_name": "Wing", "values": {"targetArea": "area", "aspectRatio": 8, "stations": 7, "pointsPerSurface": 24}}),
    );
    let r = run(
        &mut s,
        "plugin.add",
        json!({"plugin": "foil-stabiliser", "body_name": "Stab", "values": {"stabLink": "LINKED", "arm": 600, "stations": 7, "pointsPerSurface": 24}}),
    );
    // linked: 15 % of the wing, AR 8 - 1.5
    assert!(r["notice"].as_str().unwrap().starts_with("Area 150 cm²") && r["notice"].as_str().unwrap().contains("AR 6.5"), "{r}");
    let half = |s: &Session, b: &str| measure(s, b).bbox.max.y;
    assert!((half(&s, "Wing") - (8.0f64 * 100_000.0).sqrt() / 2.0).abs() < 0.05);
    assert!((half(&s, "Stab") - (6.5f64 * 15_000.0).sqrt() / 2.0).abs() < 0.05);
    run(&mut s, "parameters.change", json!({"name": "area", "expression": "1200"}));
    assert!((half(&s, "Wing") - (8.0f64 * 120_000.0).sqrt() / 2.0).abs() < 0.05, "the parameter drives the wing");
    assert!((half(&s, "Stab") - (6.5f64 * 18_000.0).sqrt() / 2.0).abs() < 0.05, "the stab follows the wing");
    // Options through plugin.set; values out of range refused with the dialog's own bounds.
    run(&mut s, "plugin.set", json!({"feature": "Wing1", "values": {"chordShape": "LINEAR", "tipRatio": 0.4, "tipCap": "FLAT", "tipDrop": 0}}));
    assert!(measure(&s, "Wing").bbox.min.z > -15.0);
    let e = s.execute("plugin.set", &json!({"feature": "Wing1", "values": {"aspectRatio": 40}})).unwrap_err().to_string();
    assert!(e.contains("Aspect ratio must be within 2…20"), "{e}");
}

#[test]
fn a_foil_set_round_trips_through_a_saved_design() {
    let mut s = session();
    let mut v = wing_1050();
    v["pointsPerSurface"] = json!(20);
    v["stations"] = json!(5);
    v["camber"] = json!("2.5 + 0");
    run(&mut s, "plugin.add", json!({"plugin": "foil-wing", "values": v, "body_name": "Wing"}));
    run(
        &mut s,
        "plugin.add",
        json!({"plugin": "foil-stabiliser", "body_name": "Stab", "values": {"stations": 5, "pointsPerSurface": 20, "profile": "MH18", "invertSection": true}}),
    );
    let dir = std::env::temp_dir().join(format!("foil_plugin_rt_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("set.solidcraft");
    run(&mut s, "file.save_as", json!({"path": path.to_str().unwrap()}));
    let mut t = session();
    run(&mut t, "doc.open", json!({"path": path.to_str().unwrap()}));
    let kinds = |s: &Session| s.doc.features.iter().map(|f| f.kind.clone()).collect::<Vec<_>>();
    assert_eq!(kinds(&s), kinds(&t));
    let (a, b) = (measure(&s, "Wing").volume, measure(&t, "Wing").volume);
    assert!((a - b).abs() < 1e-6 * a, "{a} {b}");
    assert!(t.model.state().body("Stab").is_some());
    assert_eq!(t.doc.plugin_results.len(), 2, "both results recorded for opening without the plugins");
    std::fs::remove_dir_all(&dir).ok();
}

/// What the solve changes on its own reaches the assistant as coded issues with the change that
/// clears them: here the minimum tip raises the end ratio and the cap end.
#[test]
fn a_wing_reports_what_its_solve_changed_as_coded_issues() {
    let mut s = session();
    let mut v = wing_1050();
    for (k, x) in [("stations", json!(7)), ("pointsPerSurface", json!(24)), ("minTipOn", json!(true)), ("minTip", json!(3)), ("endRatio", json!(0.1))]
    {
        v[k] = x;
    }
    let r = run(&mut s, "plugin.add", json!({"plugin": "foil-wing", "values": v, "body_name": "Wing"}));
    assert!((r["values"]["area_cm2"].as_f64().unwrap() - 1050.0).abs() < 1.0, "{r}");
    assert!((r["values"]["aspect_ratio"].as_f64().unwrap() - 9.0).abs() < 0.01, "{r}");
    let raised = r["issues"].as_array().unwrap().iter().find(|i| i["code"] == "tip_ratio_raised").unwrap_or_else(|| panic!("{r}"));
    assert_eq!((raised["severity"].clone(), raised["parameter"].clone()), (json!("info"), json!("endRatio")), "{raised}");
    let fix = raised["suggestion"]["endRatio"].as_f64().unwrap();
    assert!(fix > 0.1, "{raised}");
    // Taking the suggestion clears it.
    let r = run(&mut s, "plugin.set", json!({"feature": "Wing1", "values": {"endRatio": fix}}));
    assert!(!r["issues"].as_array().unwrap().iter().any(|i| i["code"] == "tip_ratio_raised"), "{r}");
}
