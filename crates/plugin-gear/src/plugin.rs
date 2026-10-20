//! The Gear plugin: one involute gear per feature, spur or helical, external, a ring or a
//! straight bevel gear, placed on its own or in mesh with the gear feature before it (which it
//! reads from the context's `published.gear`).

use serde_json::{Map, Value, json};
use solidcraft_plugin::{Display, FallbackPolicy, Field, FieldKind, Host, Issue, Manifest, Plugin, Schema, Severity, Unit, run};

use crate::bevel::{self, Bevel, BevelMesh, Frame};
use crate::involute::{self, Gear, Hand, Kind, Mesh, Piece, Spec};

/// The plugin's id: lowercase letters and hyphens, as plugin folders take it.
pub const ID: &str = "gear";

/// The Gear plugin.
pub struct GearPlugin;

fn number(key: &str, label: &str, group: &str, (min, default, max): (f64, f64, f64), unit: Unit, when: &str, help: &str) -> Field {
    Field {
        key: key.into(),
        label: label.into(),
        group: group.into(),
        collapsed: false,
        kind: FieldKind::Number { min, default, max, unit, integer: false },
        when: when.into(),
        help: help.into(),
        unused: false,
    }
}

fn choice(key: &str, label: &str, group: &str, options: &[(&str, &str)], when: &str, help: &str) -> Field {
    Field {
        key: key.into(),
        label: label.into(),
        group: group.into(),
        collapsed: false,
        kind: FieldKind::Choice { options: options.iter().map(|(i, l)| (i.to_string(), l.to_string())).collect(), default: options[0].0.into() },
        when: when.into(),
        help: help.into(),
        unused: false,
    }
}

fn integer(mut f: Field) -> Field {
    if let FieldKind::Number { integer, .. } = &mut f.kind {
        *integer = true;
    }
    f
}

fn collapsed(mut f: Field) -> Field {
    f.collapsed = true;
    f
}

/// The dialog.
pub fn schema() -> Schema {
    let teeth = "Teeth";
    let helix = "Helix";
    let body = "Body";
    let place = "Placement";
    let cone = "Bevel";
    Schema {
        fields: vec![
            choice(
                "kind",
                "Kind",
                teeth,
                &[("EXTERNAL", "External (spur or helical)"), ("INTERNAL", "Internal (ring gear)"), ("BEVEL", "Bevel (straight)")],
                "",
                "teeth pointing out, a ring with teeth pointing in, or a bevel gear on a cone",
            ),
            integer(number("teeth", "Teeth", teeth, (5.0, 24.0, 300.0), Unit::None, "", "the number of teeth")),
            number(
                "module",
                "Module",
                teeth,
                (0.1, 2.0, 50.0),
                Unit::None,
                "",
                "the normal module in mm (a bevel gear's at its outer end): pitch diameter = teeth · module / cos(helix angle)",
            ),
            number("pressure_angle", "Pressure angle", teeth, (10.0, 20.0, 35.0), Unit::Angle, "", "the normal pressure angle; 20° is the standard"),
            number(
                "profile_shift",
                "Profile shift",
                teeth,
                (-0.8, 0.0, 1.2),
                Unit::None,
                "kind != INTERNAL",
                "the profile shift coefficient x: positive to avoid undercut on few teeth or to stretch the centre distance",
            ),
            number(
                "backlash",
                "Backlash",
                teeth,
                (0.0, 0.0, 2.0),
                Unit::Length,
                "",
                "the pair's circumferential backlash at the pitch circle; each gear thins its teeth by half of it",
            ),
            number("helix_angle", "Helix angle", helix, (0.0, 0.0, 45.0), Unit::Angle, "kind != BEVEL", "0 for a spur gear"),
            choice(
                "hand",
                "Hand",
                helix,
                &[("RIGHT", "Right"), ("LEFT", "Left")],
                "kind != BEVEL",
                "external gears in mesh have opposite hands; a pinion and its ring the same",
            ),
            integer(number(
                "mate_teeth",
                "Mate's teeth",
                cone,
                (5.0, 40.0, 300.0),
                Unit::None,
                "kind == BEVEL && mesh == NONE",
                "the teeth of the bevel gear this one is made to mesh with: the two set the pitch cones",
            )),
            number(
                "shaft_angle",
                "Shaft angle",
                cone,
                (10.0, 90.0, 170.0),
                Unit::Angle,
                "kind == BEVEL && mesh == NONE",
                "the angle between the two gears' axes; 90° for a right-angle drive",
            ),
            number(
                "face_width",
                "Face width",
                body,
                (0.5, 10.0, 1000.0),
                Unit::Length,
                "",
                "the gear's thickness along its axis; a bevel gear's along the pitch cone, at most a third of the cone distance",
            ),
            number(
                "bore",
                "Bore",
                body,
                (0.0, 0.0, 2000.0),
                Unit::Length,
                "kind != INTERNAL",
                "the diameter of the hole through the middle; 0 for none",
            ),
            number("rim", "Rim", body, (0.5, 5.0, 500.0), Unit::Length, "kind == INTERNAL", "the ring's wall outside the root circle"),
            choice(
                "mesh",
                "Mesh with",
                place,
                &[("NONE", "Nothing: place it here"), ("PREVIOUS", "The gear feature before this one")],
                "",
                "a gear in mesh is placed at the working centre distance (a bevel gear on the shared apex) and turned so its teeth fit",
            ),
            number(
                "mesh_angle",
                "Mesh direction",
                place,
                (-360.0, 0.0, 360.0),
                Unit::Angle,
                "mesh == PREVIOUS",
                "the direction from the other gear's centre to this one's (for bevel gears: about the other gear's axis)",
            ),
            number("center_x", "Centre X", place, (-100_000.0, 0.0, 100_000.0), Unit::Length, "mesh == NONE", ""),
            number("center_y", "Centre Y", place, (-100_000.0, 0.0, 100_000.0), Unit::Length, "mesh == NONE", ""),
            number("rotation", "Rotation", place, (-360.0, 0.0, 360.0), Unit::Angle, "mesh == NONE", "the angle of the first tooth's centre line"),
            collapsed(integer(number(
                "flank_points",
                "Points on a flank",
                "Detail",
                (4.0, 8.0, 40.0),
                Unit::None,
                "",
                "how many points each involute flank is fitted through",
            ))),
        ],
    }
}

/// The values the plugin reads, with every default filled in.
struct Params {
    spec: Spec,
    mate_teeth: u32,
    shaft_angle: f64,
    face_width: f64,
    bore: f64,
    rim: f64,
    mesh: bool,
    mesh_angle: f64,
    center: [f64; 2],
    rotation: f64,
    flank_points: usize,
}

fn params(p: &Value) -> Params {
    let n = |k: &str, d: f64| p.get(k).and_then(Value::as_f64).unwrap_or(d);
    let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default();
    let kind = if s("kind") == "INTERNAL" { Kind::Internal } else { Kind::External };
    let is_bevel = s("kind") == "BEVEL";
    Params {
        mate_teeth: n("mate_teeth", 40.0).round().max(1.0) as u32,
        shaft_angle: n("shaft_angle", 90.0).to_radians(),
        spec: Spec {
            kind,
            teeth: n("teeth", 24.0).round().max(1.0) as u32,
            module: n("module", 2.0),
            pressure_angle: n("pressure_angle", 20.0).to_radians(),
            helix_angle: if is_bevel { 0.0 } else { n("helix_angle", 0.0).to_radians() },
            hand: if s("hand") == "LEFT" { Hand::Left } else { Hand::Right },
            profile_shift: if kind == Kind::Internal { 0.0 } else { n("profile_shift", 0.0) },
            backlash: n("backlash", 0.0),
        },
        face_width: n("face_width", 10.0),
        bore: if kind == Kind::Internal { 0.0 } else { n("bore", 0.0) },
        rim: n("rim", 5.0),
        mesh: s("mesh") == "PREVIOUS",
        mesh_angle: n("mesh_angle", 0.0).to_radians(),
        center: [n("center_x", 0.0), n("center_y", 0.0)],
        rotation: n("rotation", 0.0).to_radians(),
        flank_points: n("flank_points", 8.0).round().clamp(3.0, 60.0) as usize,
    }
}

/// The previous gear feature, as it published itself.
struct Previous {
    gear: Gear,
    center: [f64; 2],
    rotation: f64,
    tip: f64,
}

/// What the gear feature before this one published, if there is one.
fn published(ctx: &Value) -> Result<&Value, String> {
    ctx.get("published").and_then(|p| p.get(ID)).ok_or_else(|| "there is no gear feature before this one to mesh with".to_string())
}

fn is_bevel(g: &Value) -> bool {
    g.get("kind").and_then(Value::as_str) == Some("BEVEL")
}

fn previous(ctx: &Value) -> Result<Previous, String> {
    let g = published(ctx)?;
    if is_bevel(g) {
        return Err("a spur, helical or ring gear cannot mesh with a bevel gear".into());
    }
    previous_of(g).ok_or_else(|| "the gear feature before this one published nothing to mesh with".into())
}

fn previous_of(g: &Value) -> Option<Previous> {
    let n = |k: &str| g.get(k).and_then(Value::as_f64);
    let spec = Spec {
        kind: if g.get("kind")?.as_str()? == "INTERNAL" { Kind::Internal } else { Kind::External },
        teeth: n("teeth")? as u32,
        module: n("module")?,
        pressure_angle: n("pressure_angle_deg")?.to_radians(),
        helix_angle: n("helix_angle_deg")?.to_radians(),
        hand: if g.get("hand")?.as_str()? == "LEFT" { Hand::Left } else { Hand::Right },
        profile_shift: n("profile_shift")?,
        backlash: n("backlash")?,
    };
    let c = g.get("center")?.as_array()?;
    Some(Previous {
        gear: Gear::new(spec),
        center: [c.first()?.as_f64()?, c.get(1)?.as_f64()?],
        rotation: n("rotation_deg")?.to_radians(),
        tip: n("tip_radius")?,
    })
}

/// Everything about the gear except its geometry: what `describe` gives and `evaluate` builds from.
struct Plan {
    gear: Gear,
    p: Params,
    center: [f64; 2],
    rotation: f64,
    /// The drawn tooth's outer radius (its tip, cut back where it would come to a point).
    drawn_outer: f64,
    /// The gear's actual tip radius (external) or tooth-tip radius (ring).
    tip: f64,
    mesh: Option<(Mesh, u32)>,
    issues: Vec<Issue>,
}

fn round_up(v: f64, step: f64) -> f64 {
    (v / step).ceil() * step
}

/// The drawn tooth's outer radius: its tip, cut back to 0.1·m thick where the flanks would meet
/// (with a `pointed_tip` issue for a tooth, `warn`).
fn tip_cut_back(gear: &Gear, warn: bool, issues: &mut Vec<Issue>) -> f64 {
    let s = gear.spec;
    let m = s.module;
    let t = gear.thickness_at(gear.drawn_outer);
    if t >= 0.25 * m {
        return gear.drawn_outer;
    }
    if warn {
        issues.push(
            Issue::new(
                Severity::Warning,
                "pointed_tip",
                format!("the teeth are {:.2} mm thick at the tip (less than a quarter of the module)", t.max(0.0)),
            )
            .on("profile_shift", s.profile_shift)
            .limit(0.25 * m)
            .suggest("profile_shift", ((s.profile_shift - 0.2) * 100.0).round() / 100.0),
        );
    }
    if t < 0.1 * m { gear.radius_for_thickness(0.1 * m, gear.drawn_outer) } else { gear.drawn_outer }
}

fn plan(params_in: &Value, ctx: &Value) -> Result<Plan, String> {
    let p = params(params_in);
    let s = p.spec;
    if s.kind == Kind::Internal && s.teeth < 12 {
        return Err("a ring gear needs at least 12 teeth".into());
    }
    let gear = Gear::new(s);
    let m = s.module;
    let mut issues = Vec::new();

    // The root: two teeth may not meet there.
    let gap = gear.tau() - 2.0 * gear.half_angle(gear.flank_start());
    if gap <= 1e-3 {
        return Err(format!("the teeth overlap at the root: {} teeth of module {m} need less profile shift or more teeth", s.teeth));
    }
    let drawn_outer = tip_cut_back(&gear, s.kind == Kind::External, &mut issues);
    if s.kind == Kind::External && (s.teeth as f64) < gear.min_teeth() - 1e-9 {
        let x_min = 1.0 - s.teeth as f64 * gear.alpha_t.sin().powi(2) / (2.0 * s.helix_angle.cos());
        issues.push(
            Issue::new(
                Severity::Warning,
                "undercut",
                format!(
                    "{} teeth are undercut when cut with a rack (fewer than {:.1}); a profile shift of {:.2} avoids it",
                    s.teeth,
                    gear.min_teeth(),
                    x_min
                ),
            )
            .on("profile_shift", s.profile_shift)
            .limit(round_up(x_min, 0.01))
            .suggest("profile_shift", round_up(x_min, 0.01)),
        );
    }
    let tip = if s.kind == Kind::External { drawn_outer } else { gear.tip_radius };
    if s.kind == Kind::External && p.bore > 0.0 && p.bore / 2.0 > gear.root_radius - m {
        let most = ((2.0 * (gear.root_radius - m)) * 10.0).floor() / 10.0;
        return Err(format!("the bore Ø{} leaves less than one module of rim under the teeth (at most Ø{most})", p.bore));
    }

    let (center, rotation, mesh) = if p.mesh {
        let prev = previous(ctx)?;
        let mesh = involute::mesh(&prev.gear, prev.center, prev.rotation, prev.tip, &gear, tip, p.mesh_angle)?;
        let (k1, k2) = (prev.gear.spec.kind, s.kind);
        if s.helix_angle > 0.0 {
            let want_same = k1 != k2;
            if (prev.gear.spec.hand == s.hand) != want_same {
                let other = if s.hand == Hand::Right { "LEFT" } else { "RIGHT" };
                issues.push(
                    Issue::new(
                        Severity::Warning,
                        "helix_hand",
                        if want_same {
                            "a pinion and its ring need the same hand of helix"
                        } else {
                            "external helical gears in mesh need opposite hands"
                        },
                    )
                    .on("hand", if s.hand == Hand::Right { "RIGHT" } else { "LEFT" })
                    .suggest("hand", other),
                );
            }
        }
        if k1 != k2 {
            let (zr, zp) = if k1 == Kind::Internal { (prev.gear.spec.teeth, s.teeth) } else { (s.teeth, prev.gear.spec.teeth) };
            if zr < zp + 10 {
                issues.push(
                    Issue::new(
                        Severity::Warning,
                        "ring_interference",
                        format!("a {zr}-tooth ring and a {zp}-tooth pinion are closer than 10 teeth apart and may interfere"),
                    )
                    .on("teeth", s.teeth)
                    .limit(10),
                );
            }
        }
        if mesh.contact_ratio < 1.2 {
            issues.push(
                Issue::new(
                    Severity::Warning,
                    "low_contact_ratio",
                    format!("the contact ratio is {:.2}: less than 1.2 runs rough", mesh.contact_ratio),
                )
                .on("teeth", s.teeth)
                .limit(1.2),
            );
        }
        (mesh.center, mesh.rotation, Some((mesh, prev.gear.spec.teeth)))
    } else {
        (p.center, p.rotation, None)
    };
    Ok(Plan { gear, p, center, rotation, drawn_outer, tip, mesh, issues })
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// The notice, values, issues and what the feature publishes.
fn report(pl: &Plan) -> Value {
    let g = &pl.gear;
    let s = g.spec;
    let kind = match (s.kind, s.helix_angle > 0.0) {
        (Kind::Internal, false) => "Ring gear".to_string(),
        (Kind::Internal, true) => {
            format!("Helical ring gear ({} hand {:.0}°)", if s.hand == Hand::Right { "right" } else { "left" }, s.helix_angle.to_degrees())
        }
        (Kind::External, false) => "Spur gear".to_string(),
        (Kind::External, true) => {
            format!("Helical gear ({} hand {:.0}°)", if s.hand == Hand::Right { "right" } else { "left" }, s.helix_angle.to_degrees())
        }
    };
    let (tip_d, root_d) = (2.0 * pl.tip, 2.0 * g.root_radius);
    let mut notice =
        format!("{kind}, {} teeth, module {}: pitch Ø {:.2}, tip Ø {:.2}, root Ø {:.2} mm", s.teeth, s.module, 2.0 * g.pitch_radius, tip_d, root_d);
    let mut values = Map::new();
    values.insert("pitch_diameter_mm".into(), json!(r3(2.0 * g.pitch_radius)));
    values.insert("tip_diameter_mm".into(), json!(r3(tip_d)));
    values.insert("root_diameter_mm".into(), json!(r3(root_d)));
    values.insert("base_diameter_mm".into(), json!(r3(2.0 * g.base_radius)));
    values.insert(
        "tooth_thickness_mm".into(),
        json!(r3(if s.kind == Kind::External { g.drawn_thickness } else { g.module_t * std::f64::consts::PI - g.drawn_thickness })),
    );
    if s.helix_angle > 0.0 {
        values.insert("transverse_module".into(), json!(r3(g.module_t)));
        values.insert("twist_deg".into(), json!(r3(g.twist(pl.p.face_width).to_degrees())));
    }
    if let Some((m, z1)) = pl.mesh {
        notice +=
            &format!("; meshes with the {z1}-tooth gear at {:.2} mm (ratio {:.3}, contact ratio {:.2})", m.center_distance, m.ratio, m.contact_ratio);
        values.insert("center_distance_mm".into(), json!(r3(m.center_distance)));
        values.insert("ratio".into(), json!(r3(m.ratio)));
        values.insert("contact_ratio".into(), json!(r3(m.contact_ratio)));
        values.insert("working_pressure_angle_deg".into(), json!(r3(m.alpha_w.to_degrees())));
    }
    values.insert("center_mm".into(), json!([r3(pl.center[0]), r3(pl.center[1])]));
    values.insert("rotation_deg".into(), json!(r3(pl.rotation.to_degrees())));
    let publish = json!({
        "kind": if s.kind == Kind::Internal { "INTERNAL" } else { "EXTERNAL" },
        "teeth": s.teeth,
        "module": s.module,
        "pressure_angle_deg": s.pressure_angle.to_degrees(),
        "helix_angle_deg": s.helix_angle.to_degrees(),
        "hand": if s.hand == Hand::Right { "RIGHT" } else { "LEFT" },
        "profile_shift": s.profile_shift,
        "backlash": s.backlash,
        "center": pl.center,
        "rotation_deg": pl.rotation.to_degrees(),
        "tip_radius": pl.tip,
        "pitch_radius": g.pitch_radius,
    });
    json!({"notice": notice, "values": values, "issues": pl.issues, "publish": publish})
}

fn xy(p: [f64; 2]) -> Value {
    json!([p[0], p[1]])
}

/// Draw an outline in the sketch being edited.
fn draw(host: &mut dyn Host, pieces: Vec<Piece>) -> Result<(), String> {
    for piece in pieces {
        match piece {
            Piece::Line(a, b) => run(host, "sketch.line", json!({"points": [xy(a), xy(b)], "connect": false, "infer": false}))?,
            Piece::Arc { center, start, end } => {
                run(host, "sketch.arc.center_point", json!({"center": xy(center), "start": xy(start), "end": xy(end), "connect": false}))?
            }
            Piece::Spline(points) => run(host, "sketch.spline.fit_point", json!({"points": points.into_iter().map(xy).collect::<Vec<_>>()}))?,
        };
    }
    Ok(())
}

/// The gear on a scratch design: one sketch (the tooth outline and the bore or rim circle) and
/// one extrude, or a sweep with twist for a helical gear.
fn build(pl: &Plan, host: &mut dyn Host) -> Result<(), String> {
    let g = &pl.gear;
    let c = pl.center;
    // A ring's tooth spaces are drawn as an external gear's teeth: turned half a pitch.
    let drawn_rotation = if g.spec.kind == Kind::Internal { pl.rotation + g.tau() / 2.0 } else { pl.rotation };
    let sk = run(host, "sketch.create", json!({"plane": "XY"}))?;
    let sketch = sk.get("name").cloned().unwrap_or(json!("Sketch1"));
    draw(host, involute::outline(g, c, drawn_rotation, pl.drawn_outer, pl.p.flank_points))?;
    // The material: between the bore and the teeth, or between the rim and the tooth spaces.
    let inside = match g.spec.kind {
        Kind::External if pl.p.bore > 0.0 => {
            run(host, "sketch.circle.center", json!({"center": xy(c), "radius": pl.p.bore / 2.0}))?;
            (pl.p.bore / 2.0 + g.root_radius) / 2.0
        }
        Kind::External => 0.0,
        Kind::Internal => {
            let rim = g.root_radius + pl.p.rim;
            run(host, "sketch.circle.center", json!({"center": xy(c), "radius": rim}))?;
            (rim + g.root_radius) / 2.0
        }
    };
    run(host, "sketch.finish", json!({}))?;
    let profiles = json!([{"point": [c[0] + inside, c[1]]}]);
    let width = pl.p.face_width;
    if g.spec.helix_angle > 0.0 {
        let path = run(host, "sketch.create", json!({"plane": {"origin": [c[0], c[1], 0], "x_dir": [1, 0, 0], "y_dir": [0, 0, 1]}}))?;
        let path_name = path.get("name").cloned().unwrap_or(json!("Sketch2"));
        let line = run(host, "sketch.line", json!({"points": [[0, 0], [0, width]], "connect": false, "infer": false}))?;
        let curve = line.pointer("/created/curves/0").cloned().unwrap_or(json!("l1"));
        run(host, "sketch.finish", json!({}))?;
        run(
            host,
            "solid.sweep",
            json!({"sketch": sketch, "profiles": profiles, "path_sketch": path_name, "path": [curve],
                   "twist": format!("{} deg", g.twist(width).to_degrees()), "body_name": "Gear"}),
        )?;
    } else {
        run(host, "solid.extrude", json!({"sketch": sketch, "profiles": profiles, "distance": width, "body_name": "Gear"}))?;
    }
    Ok(())
}

/// A bevel gear feature: the gear, where it sits and what it says.
struct BevelPlan {
    bevel: Bevel,
    p: Params,
    frame: Frame,
    rotation: f64,
    mesh: Option<(BevelMesh, u32)>,
    issues: Vec<Issue>,
}

/// The bevel gear before this one, as it published itself.
struct PreviousBevel {
    bevel: Bevel,
    frame: Frame,
    rotation: f64,
}

fn vec3(v: &Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?])
}

fn previous_bevel(ctx: &Value) -> Result<PreviousBevel, String> {
    let g = published(ctx)?;
    if !is_bevel(g) {
        return Err("a bevel gear meshes only with a bevel gear".into());
    }
    let read = || -> Option<PreviousBevel> {
        let n = |k: &str| g.get(k).and_then(Value::as_f64);
        let spec = Spec {
            kind: Kind::External,
            teeth: n("teeth")? as u32,
            module: n("module")?,
            pressure_angle: n("pressure_angle_deg")?.to_radians(),
            helix_angle: 0.0,
            hand: Hand::Right,
            profile_shift: n("profile_shift")?,
            backlash: n("backlash")?,
        };
        let mut bevel = Bevel::new(spec, n("mate_teeth")? as u32, n("shaft_angle_deg")?.to_radians(), n("face_width")?);
        bevel.virtual_outer = tip_cut_back(&bevel.virtual_gear, false, &mut Vec::new());
        let frame = Frame { apex: vec3(g.get("apex")?)?, axis: vec3(g.get("axis")?)?, reference: vec3(g.get("reference")?)? };
        Some(PreviousBevel { bevel, frame, rotation: n("rotation_deg")?.to_radians() })
    };
    read().ok_or_else(|| "the bevel gear before this one published nothing to mesh with".into())
}

fn bevel_plan(params_in: &Value, ctx: &Value) -> Result<BevelPlan, String> {
    let p = params(params_in);
    let s = Spec { kind: Kind::External, helix_angle: 0.0, ..p.spec };
    let prev = if p.mesh { Some(previous_bevel(ctx)?) } else { None };
    let (mate, sigma) = prev.as_ref().map(|q| (q.bevel.spec.teeth, q.bevel.shaft_angle)).unwrap_or((p.mate_teeth, p.shaft_angle));
    let mut bevel = Bevel::new(s, mate, sigma, p.face_width);
    let m = s.module;
    let r = bevel.cone_distance;
    let mut issues = Vec::new();
    if p.face_width >= 0.9 * r {
        return Err(format!("the face width {} mm is too long for a cone distance of {r:.2} mm (keep it to a third)", p.face_width));
    }
    if p.face_width > r / 3.0 + 1e-9 {
        let most = (r / 3.0 * 10.0).floor() / 10.0;
        issues.push(
            Issue::new(
                Severity::Warning,
                "face_too_wide",
                format!("the face is wider than a third of the cone distance {r:.2} mm: the inner teeth get too small"),
            )
            .on("face_width", p.face_width)
            .limit(most)
            .suggest("face_width", most),
        );
    }
    let g = bevel.virtual_gear;
    let zv = bevel.virtual_teeth();
    if 2.0 * std::f64::consts::PI / zv - 2.0 * g.half_angle(g.flank_start()) <= 1e-3 {
        return Err(format!("the teeth overlap at the root: {} teeth of module {m} need less profile shift or more teeth", s.teeth));
    }
    bevel.virtual_outer = tip_cut_back(&g, true, &mut issues);
    if zv < g.min_teeth() - 1e-9 {
        let x_min = 1.0 - zv * s.pressure_angle.sin().powi(2) / 2.0;
        issues.push(
            Issue::new(
                Severity::Warning,
                "undercut",
                format!(
                    "{} teeth on this cone ({zv:.1} virtual teeth, fewer than {:.1}) are undercut; a profile shift of {x_min:.2} avoids it",
                    s.teeth,
                    g.min_teeth()
                ),
            )
            .on("profile_shift", s.profile_shift)
            .limit(round_up(x_min, 0.01))
            .suggest("profile_shift", round_up(x_min, 0.01)),
        );
    }
    if p.bore > 0.0 {
        let k = bevel.inner_scale();
        let root_inner = bevel.projected_radius(g.drawn_inner, k);
        if p.bore / 2.0 > root_inner - m * k {
            let most = ((2.0 * (root_inner - m * k)) * 10.0).floor() / 10.0;
            return Err(format!("the bore Ø{} leaves less than one module of rim under the teeth at the inner end (at most Ø{most})", p.bore));
        }
    }
    let (frame, rotation, mesh) = match prev {
        Some(q) => {
            let mesh = bevel::mesh(&q.bevel, &q.frame, q.rotation, &bevel, p.mesh_angle)?;
            if mesh.contact_ratio < 1.2 {
                issues.push(
                    Issue::new(
                        Severity::Warning,
                        "low_contact_ratio",
                        format!("the contact ratio is {:.2}: less than 1.2 runs rough", mesh.contact_ratio),
                    )
                    .on("teeth", s.teeth)
                    .limit(1.2),
                );
            }
            let x1 = q.bevel.spec.profile_shift;
            if (x1 + s.profile_shift).abs() > 1e-9 {
                issues.push(
                    Issue::new(
                        Severity::Warning,
                        "bevel_shift_sum",
                        "bevel gears on their standard cones mesh only when their profile shifts cancel (x2 = −x1)",
                    )
                    .on("profile_shift", s.profile_shift)
                    .suggest("profile_shift", -x1),
                );
            }
            (mesh.frame, mesh.rotation, Some((mesh, q.bevel.spec.teeth)))
        }
        None => {
            let frame = Frame { apex: [p.center[0], p.center[1], bevel.apex_height], axis: [0.0, 0.0, 1.0], reference: [1.0, 0.0, 0.0] };
            (frame, p.rotation, None)
        }
    };
    Ok(BevelPlan { bevel, p, frame, rotation, mesh, issues })
}

fn bevel_report(pl: &BevelPlan) -> Value {
    let b = &pl.bevel;
    let s = b.spec;
    let mut notice = format!(
        "Bevel gear, {} teeth, module {}, for a {}-tooth mate at {:.0}°: pitch cone {:.2}°, pitch Ø {:.2}, tip Ø {:.2} mm, cone distance {:.2} mm",
        s.teeth,
        s.module,
        b.mate_teeth,
        b.shaft_angle.to_degrees(),
        b.delta.to_degrees(),
        2.0 * b.pitch_radius,
        2.0 * b.tip_radius(),
        b.cone_distance
    );
    let mut values = Map::new();
    values.insert("pitch_diameter_mm".into(), json!(r3(2.0 * b.pitch_radius)));
    values.insert("tip_diameter_mm".into(), json!(r3(2.0 * b.tip_radius())));
    values.insert("root_diameter_mm".into(), json!(r3(2.0 * b.root_radius())));
    values.insert("pitch_cone_angle_deg".into(), json!(r3(b.delta.to_degrees())));
    values.insert("cone_distance_mm".into(), json!(r3(b.cone_distance)));
    values.insert("virtual_teeth".into(), json!(r3(b.virtual_teeth())));
    values.insert("height_mm".into(), json!(r3(b.inner_height())));
    if let Some((m, z1)) = pl.mesh {
        notice += &format!("; meshes with the {z1}-tooth bevel gear (ratio {:.3}, contact ratio {:.2})", m.ratio, m.contact_ratio);
        values.insert("ratio".into(), json!(r3(m.ratio)));
        values.insert("contact_ratio".into(), json!(r3(m.contact_ratio)));
    }
    values.insert("apex_mm".into(), json!(pl.frame.apex.map(r3)));
    values.insert("axis".into(), json!(pl.frame.axis.map(r3)));
    let publish = json!({
        "kind": "BEVEL",
        "teeth": s.teeth,
        "mate_teeth": b.mate_teeth,
        "shaft_angle_deg": b.shaft_angle.to_degrees(),
        "module": s.module,
        "pressure_angle_deg": s.pressure_angle.to_degrees(),
        "profile_shift": s.profile_shift,
        "backlash": s.backlash,
        "face_width": b.face_width,
        "apex": pl.frame.apex,
        "axis": pl.frame.axis,
        "reference": pl.frame.reference,
        "rotation_deg": pl.rotation.to_degrees(),
    });
    json!({"notice": notice, "values": values, "issues": pl.issues, "publish": publish})
}

/// A bevel gear on a scratch design: the outer and inner outlines lofted (every line of the loft
/// runs to the apex), the outer end trimmed to the back cone and the bore cut, in one revolve,
/// then the gear moved from its own frame to where it sits.
fn build_bevel(pl: &BevelPlan, host: &mut dyn Host) -> Result<(), String> {
    let b = &pl.bevel;
    let (k, zi) = (b.inner_scale(), b.inner_height());
    let fp = pl.p.flank_points;
    let outer = run(host, "sketch.create", json!({"plane": "XY"}))?;
    let outer = outer.get("name").cloned().unwrap_or(json!("Sketch1"));
    draw(host, b.outline(pl.rotation, 1.0, fp))?;
    run(host, "sketch.finish", json!({}))?;
    let inner = run(host, "sketch.create", json!({"plane": {"origin": [0, 0, zi], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}}))?;
    let inner = inner.get("name").cloned().unwrap_or(json!("Sketch2"));
    draw(host, b.outline(pl.rotation, k, fp))?;
    run(host, "sketch.finish", json!({}))?;
    run(host, "solid.loft", json!({"sections": [{"sketch": outer}, {"sketch": inner}], "body_name": "Gear"}))?;

    // The trim, in the plane through the axis: below the back cone (through the outer pitch
    // circle, square to the pitch cone) beyond the pitch circle, and the bore.
    let rp = b.pitch_radius;
    let td = b.delta.tan();
    let bore = pl.p.bore / 2.0;
    let depth = (0.5 * (rp - bore) * td).clamp(0.1, 5.0);
    let r_low = rp - depth / td;
    let r_far = 1.5 * b.projected_radius(b.virtual_outer, 1.0);
    let trim = run(host, "sketch.create", json!({"plane": {"origin": [0, 0, 0], "x_dir": [1, 0, 0], "y_dir": [0, 0, 1]}}))?;
    let trim = trim.get("name").cloned().unwrap_or(json!("Sketch3"));
    run(
        host,
        "sketch.line",
        json!({"points": [[r_low, -depth], [r_far, (r_far - rp) * td], [r_far, -depth]], "closed": true, "connect": false, "infer": false}),
    )?;
    if bore > 0.0 {
        run(
            host,
            "sketch.line",
            json!({"points": [[0, -depth], [bore, -depth], [bore, zi + depth], [0, zi + depth]], "closed": true, "connect": false, "infer": false}),
        )?;
    }
    run(host, "sketch.finish", json!({}))?;
    run(host, "solid.revolve", json!({"sketch": trim, "axis": "Z", "operation": "cut", "targets": ["Gear"]}))?;

    // From its own frame (apex at (0, 0, H), axis +Z) to the plan's.
    let f = &pl.frame;
    let (n, turn) = bevel::rotation_to(f.axis);
    if turn.abs() < 1e-12 {
        let t = [f.apex[0], f.apex[1], f.apex[2] - b.apex_height];
        if t.iter().any(|v| v.abs() > 1e-12) {
            run(host, "solid.move", json!({"bodies": ["Gear"], "translate": t}))?;
        }
    } else {
        run(host, "solid.move", json!({"bodies": ["Gear"], "translate": [0, 0, -b.apex_height]}))?;
        run(host, "solid.move", json!({"bodies": ["Gear"], "axis": n, "angle": format!("{} deg", turn.to_degrees())}))?;
        run(host, "solid.move", json!({"bodies": ["Gear"], "translate": f.apex}))?;
    }
    Ok(())
}

impl Plugin for GearPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Gear".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: solidcraft_plugin::API_VERSION,
            description:
                "An involute gear: spur or helical, external, a ring or a straight bevel gear, on its own or in mesh with the gear before it.".into(),
            display: Display { label: "Gear".into(), icon_id: "gear".into(), tooltip: "Involute gear".into(), type_name: "GearFeature".into() },
            fallback: FallbackPolicy::RecordedMesh,
            preview: true,
        }
    }
    fn schema(&self) -> Schema {
        schema()
    }
    fn evaluate(&self, params: &Value, ctx: &Value, host: &mut dyn Host) -> Result<Value, String> {
        if params.get("kind").and_then(Value::as_str) == Some("BEVEL") {
            let pl = bevel_plan(params, ctx)?;
            build_bevel(&pl, host)?;
            return Ok(bevel_report(&pl));
        }
        let pl = plan(params, ctx)?;
        build(&pl, host)?;
        Ok(report(&pl))
    }
    fn describe(&self, params: &Value, ctx: &Value) -> Result<Value, String> {
        if params.get("kind").and_then(Value::as_str) == Some("BEVEL") {
            return Ok(bevel_report(&bevel_plan(params, ctx)?));
        }
        Ok(report(&plan(params, ctx)?))
    }
}
