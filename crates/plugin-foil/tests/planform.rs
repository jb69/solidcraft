//! The planform laws against the Python port of the same FeatureScript (dialog_to_stations.py)
//! and the FeatureScript's own numbers.

use solidcraft_plugin_foil::{Tool, Val, Values, wing};

fn set(v: &mut Values, kv: &[(&str, Val)]) {
    for (k, x) in kv {
        v.set(k, x.clone()).unwrap_or_else(|e| panic!("{k}: {e}"));
    }
}

fn n(x: f64) -> Val {
    Val::Num(x)
}
fn c(s: &str) -> Val {
    Val::Choice(s.into())
}

/// The Silk V2 1050 as the brief gives it (no winglet, no tubercles).
pub fn silk_1050() -> Values {
    let mut v = Values::defaults(Tool::Wing);
    set(
        &mut v,
        &[
            ("targetArea", n(1050.0)),
            ("aspectRatio", n(9.0)),
            ("camber", n(2.5)),
            ("camberPos", n(40.0)),
            ("rootThickness", n(12.0)),
            ("tipThickness", n(10.0)),
            ("tipProfile", c("NACA4")),
            ("tipCamber", n(1.0)),
            ("tipCamberPos", n(50.0)),
            ("blendFrom", n(40.0)),
            ("chordShape", c("SUPERELLIPSE")),
            ("superN", n(1.4)),
            ("endRatio", n(0.26)),
            ("planformAlign", n(78.0)),
            ("sweepTip", n(70.0)),
            ("sweepExp", n(4.0)),
            ("tipDrop", n(45.0)),
            ("dropLaw", c("SCURVE")),
            ("dropFrom", n(3.0)),
            ("dropTo", n(91.0)),
            ("tipCap", c("ROUND_PLAN")),
            ("capLength", n(18.0)),
            ("capEnd", n(25.0)),
            ("stations", n(13.0)),
            ("teThickness", n(0.8)),
            ("pointsPerSurface", n(50.0)),
        ],
    );
    v
}

#[test]
fn silk_1050_planform_matches_the_python_port() {
    let s = wing::build(&silk_1050(), [0.0; 3], None).expect("builds");
    println!("{}", s.notice);
    // dialog_to_stations.py: span 972.111, c0 150.555 (its root-chord integral is 4000 midpoints,
    // the FeatureScript's 400 Simpson intervals)
    assert!((s.span - 972.111).abs() < 0.01, "span {}", s.span);
    assert!((s.root_chord - 150.555).abs() < 0.01, "root chord {}", s.root_chord);
    assert!((s.area_proj - 105_000.0).abs() < 1.0, "area {}", s.area_proj);
    // (eta, chord, sweep x, drop) from the Python port at the superellipse stations
    let python = [
        (0.190485, 141.493, 4.895, 3.944),
        (0.481883, 116.068, 22.053, 23.411),
        (0.773517, 79.342, 62.803, 42.088),
        (0.935016, 52.345, 105.554, 45.0),
        (0.962967, 46.895, 115.133, 45.0),
        (1.0, 9.786, 136.387, 45.0),
    ];
    for (eta, chord, x, drop) in python {
        let st = s.stations.iter().find(|st| (st.eta - eta).abs() < 2e-6).unwrap_or_else(|| panic!("no station at {eta}"));
        assert!((st.chord - chord).abs() < 0.01, "chord at {eta}: {} vs {chord}", st.chord);
        assert!((st.frame.origin[0] - x).abs() < 0.01, "sweep at {eta}: {} vs {x}", st.frame.origin[0]);
        assert!((st.frame.origin[2] + drop).abs() < 0.01, "drop at {eta}: {} vs {drop}", st.frame.origin[2]);
    }
}
