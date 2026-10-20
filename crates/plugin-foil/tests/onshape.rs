//! The foil plugins in a design, against the Onshape tool's own results for the same dialog
//! values (the test copies of the Foil designer and the B-Series generator, 2026-10-19), and as
//! features: parameters driving them, a linked stabiliser following its wing, save and reopen.

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

/// Bounds and volume against Onshape's.
fn check_against(s: &Session, body: &str, vol: f64, min: [f64; 3], max: [f64; 3], vol_tol: f64, tol: f64) {
    let m = measure(s, body);
    println!("{body}: volume {} ({:+.4} %) bbox {:?}", m.volume, 100.0 * (m.volume / vol - 1.0), m.bbox);
    assert!((m.volume - vol).abs() / vol < vol_tol, "{body} volume {} vs Onshape {vol}", m.volume);
    let (a, b) = ([m.bbox.min.x, m.bbox.min.y, m.bbox.min.z], [m.bbox.max.x, m.bbox.max.y, m.bbox.max.z]);
    for i in 0..3 {
        assert!((a[i] - min[i]).abs() < tol && (b[i] - max[i]).abs() < tol, "{body} bounds {a:?} {b:?} vs Onshape {min:?} {max:?}");
    }
}

/// The Silk V2 1050 of the brief (designs/jason/silk-1050-usurf-135-tuned.md, no winglets or
/// tubercles).
fn silk_1050() -> Value {
    json!({
        "targetArea": 1050, "aspectRatio": 9, "profile": "NACA4", "camber": 2.5, "camberPos": 40,
        "rootThickness": 12, "tipThickness": 10, "tipProfile": "NACA4", "tipCamber": 1.0, "tipCamberPos": 50, "blendFrom": 40,
        "chordShape": "SUPERELLIPSE", "superN": 1.4, "endRatio": 0.26, "planformAlign": 78, "sweepTip": 70, "sweepExp": 4,
        "tipDrop": 45, "dropLaw": "SCURVE", "dropFrom": 3, "dropTo": 91, "tipCap": "ROUND_PLAN", "capLength": 18, "capEnd": 25,
        "stations": 13, "teThickness": 0.8, "pointsPerSurface": 50
    })
}

/// Onshape: 1,013,913.8 mm³, (-37.657, -486.056, -47.598) to (152.090, 486.056, 12.798), and the
/// notice word for word up to its roll note.
#[test]
fn silk_1050_as_onshape_builds_it() {
    let mut s = session();
    let r = run(&mut s, "plugin.add", json!({"plugin": "foil-wing", "values": silk_1050(), "body_name": "Wing"}));
    let n = r["notice"].as_str().unwrap();
    assert!(n.starts_with("Area 1050 cm² projected (1057 developed) · span 972 mm · AR 9 · root 151 mm · end 47 mm · MAC 118 mm"), "{n}");
    check_against(&s, "Wing", 1_013_913.8, [-37.6575, -486.0556, -47.5982], [152.0899, 486.0556, 12.7978], 5e-4, 0.05);
}

/// The tuned Silk 1050 in full: winglets up (15 cm², 25°, 60 mm), tubercles 5 mm at 62 mm from 18
/// to 82 %, Min tip 1 mm. Onshape: 1,013,172.4 mm³, (-37.657, -484.619, -47.598) to (152.109,
/// 484.619, 12.798).
#[test]
fn silk_1050_winglets_and_tubercles_as_onshape_builds_them() {
    let mut s = session();
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
    let r = run(&mut s, "plugin.add", json!({"plugin": "foil-wing", "values": v, "body_name": "Wing"}));
    assert!(
        r["notice"].as_str().unwrap().contains(
            "winglets up 15 cm² each (14.4 projected), 9 mm high at 25°, projected span 969 mm · tip section NACA4 1% camber from 40% span"
        ),
        "{r}"
    );
    check_against(&s, "Wing", 1_013_172.44, [-37.6575, -484.619, -47.5982], [152.109, 484.619, 12.7978], 5e-4, 0.05);
}

/// The U Surf 135 on its 560 arm: tubercles 1.5 mm at 30 mm from 15 to 85 %, Min tip 1 mm
/// (Onshape raised Cap end to 45 %). Onshape: 39,324.3 mm³, (546.155, -174.284, -2.652) to
/// (601.542, 174.284, 2.854).
#[test]
fn u_surf_135_as_onshape_builds_it() {
    let mut s = session();
    let r = run(
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
    let n = r["notice"].as_str().unwrap();
    assert!(
        n.starts_with(
            "Area 135 cm² projected · span 349 mm · AR 9 · root 55 mm · end 15 mm · MAC 43 mm · Cap end raised to 45 % for a 0.5 mm cap end"
        ),
        "{n}"
    );
    check_against(&s, "Stab", 39_324.26, [546.155, -174.2843, -2.6522], [601.5423, 174.2843, 2.854], 2e-3, 0.05);
}

/// The "Propeller 140 x 3 on the 63 mm hub (the eFoil set)" of the B-Series generator's test
/// studio, root fillet 0: Onshape gives 112,441.3 mm³ and extents 123.379 x 117.414 x 45.000 mm
/// (it is moved by a Transform there, so the extents are compared).
#[test]
fn efoil_propeller_as_onshape_builds_it() {
    let mut s = session();
    let t = std::time::Instant::now();
    let r = run(
        &mut s,
        "plugin.add",
        json!({"plugin": "foil-propeller", "body_name": "Prop", "values": {
            "diameter": 140, "pdRatio": 0.9, "ear": 0.3, "hubUpstream": 63, "hubDownstream": 63, "hubLength": 30, "noseLength": 15,
            "mountType": "BOLTED", "pilotDia": 12, "tipPitchChange": -10, "skewAngle": 7, "rake": 5, "taperExponent": 1, "sectionCount": 12
        }}),
    );
    println!("built in {:?}: {}", t.elapsed(), r["notice"]);
    let m = measure(&s, "Prop");
    let e = m.bbox.max - m.bbox.min;
    println!("volume {} extents {e:?}", m.volume);
    assert!((m.volume - 112_441.3).abs() / 112_441.3 < 1e-3, "{}", m.volume);
    // The bounds come from the body's mesh, which can fall a little short of a blade's round tip.
    assert!((e.x - 123.379).abs() < 0.1 && (e.y - 117.414).abs() < 0.1 && (e.z - 45.0).abs() < 0.01, "{e:?}");
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
    let mut v = silk_1050();
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
