//! The Gear plugin: one involute gear per feature, spur or helical, external or a ring, placed
//! on its own or in mesh with the gear feature before it (which it reads from the context's
//! `published.gear`).

use serde_json::{Map, Value, json};
use solidcraft_plugin::{Display, FallbackPolicy, Field, FieldKind, Host, Issue, Manifest, Plugin, Schema, Severity, Unit, run};

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
    Schema {
        fields: vec![
            choice(
                "kind",
                "Kind",
                teeth,
                &[("EXTERNAL", "External (spur or helical)"), ("INTERNAL", "Internal (ring gear)")],
                "",
                "teeth pointing out, or a ring with teeth pointing in",
            ),
            integer(number("teeth", "Teeth", teeth, (5.0, 24.0, 300.0), Unit::None, "", "the number of teeth")),
            number(
                "module",
                "Module",
                teeth,
                (0.1, 2.0, 50.0),
                Unit::None,
                "",
                "the normal module in mm: pitch diameter = teeth · module / cos(helix angle)",
            ),
            number("pressure_angle", "Pressure angle", teeth, (10.0, 20.0, 35.0), Unit::Angle, "", "the normal pressure angle; 20° is the standard"),
            number(
                "profile_shift",
                "Profile shift",
                teeth,
                (-0.8, 0.0, 1.2),
                Unit::None,
                "kind == EXTERNAL",
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
            number("helix_angle", "Helix angle", helix, (0.0, 0.0, 45.0), Unit::Angle, "", "0 for a spur gear"),
            choice(
                "hand",
                "Hand",
                helix,
                &[("RIGHT", "Right"), ("LEFT", "Left")],
                "",
                "external gears in mesh have opposite hands; a pinion and its ring the same",
            ),
            number("face_width", "Face width", body, (0.5, 10.0, 1000.0), Unit::Length, "", "the gear's thickness along its axis"),
            number(
                "bore",
                "Bore",
                body,
                (0.0, 0.0, 2000.0),
                Unit::Length,
                "kind == EXTERNAL",
                "the diameter of the hole through the middle; 0 for none",
            ),
            number("rim", "Rim", body, (0.5, 5.0, 500.0), Unit::Length, "kind == INTERNAL", "the ring's wall outside the root circle"),
            choice(
                "mesh",
                "Mesh with",
                place,
                &[("NONE", "Nothing: place it here"), ("PREVIOUS", "The gear feature before this one")],
                "",
                "a gear in mesh is placed at the working centre distance and turned so its teeth fit",
            ),
            number(
                "mesh_angle",
                "Mesh direction",
                place,
                (-360.0, 0.0, 360.0),
                Unit::Angle,
                "mesh == PREVIOUS",
                "the direction from the other gear's centre to this one's",
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
    Params {
        spec: Spec {
            kind,
            teeth: n("teeth", 24.0).round().max(1.0) as u32,
            module: n("module", 2.0),
            pressure_angle: n("pressure_angle", 20.0).to_radians(),
            helix_angle: n("helix_angle", 0.0).to_radians(),
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

fn previous(ctx: &Value) -> Option<Previous> {
    let g = ctx.get("published")?.get(ID)?;
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
    // The tip: cut back where the flanks would meet.
    let mut drawn_outer = gear.drawn_outer;
    if gear.thickness_at(drawn_outer) < 0.25 * m {
        let t = gear.thickness_at(gear.drawn_outer);
        if gear.thickness_at(gear.drawn_outer) < 0.1 * m {
            drawn_outer = gear.radius_for_thickness(0.1 * m, gear.drawn_outer);
        }
        if s.kind == Kind::External {
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
    }
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
        let prev = previous(ctx).ok_or("there is no gear feature before this one to mesh with")?;
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

/// The gear on a scratch design: one sketch (the tooth outline and the bore or rim circle) and
/// one extrude, or a sweep with twist for a helical gear.
fn build(pl: &Plan, host: &mut dyn Host) -> Result<(), String> {
    let g = &pl.gear;
    let c = pl.center;
    // A ring's tooth spaces are drawn as an external gear's teeth: turned half a pitch.
    let drawn_rotation = if g.spec.kind == Kind::Internal { pl.rotation + g.tau() / 2.0 } else { pl.rotation };
    let sk = run(host, "sketch.create", json!({"plane": "XY"}))?;
    let sketch = sk.get("name").cloned().unwrap_or(json!("Sketch1"));
    for piece in involute::outline(g, c, drawn_rotation, pl.drawn_outer, pl.p.flank_points) {
        match piece {
            Piece::Line(a, b) => run(host, "sketch.line", json!({"points": [xy(a), xy(b)], "connect": false, "infer": false}))?,
            Piece::Arc { center, start, end } => {
                run(host, "sketch.arc.center_point", json!({"center": xy(center), "start": xy(start), "end": xy(end), "connect": false}))?
            }
            Piece::Spline(points) => run(host, "sketch.spline.fit_point", json!({"points": points.into_iter().map(xy).collect::<Vec<_>>()}))?,
        };
    }
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

impl Plugin for GearPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Gear".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: solidcraft_plugin::API_VERSION,
            description: "An involute gear: spur or helical, external or a ring, on its own or in mesh with the gear before it.".into(),
            display: Display { label: "Gear".into(), icon_id: "gear".into(), tooltip: "Involute gear".into(), type_name: "GearFeature".into() },
            fallback: FallbackPolicy::RecordedMesh,
            preview: true,
        }
    }
    fn schema(&self) -> Schema {
        schema()
    }
    fn evaluate(&self, params: &Value, ctx: &Value, host: &mut dyn Host) -> Result<Value, String> {
        let pl = plan(params, ctx)?;
        build(&pl, host)?;
        Ok(report(&pl))
    }
    fn describe(&self, params: &Value, ctx: &Value) -> Result<Value, String> {
        Ok(report(&plan(params, ctx)?))
    }
}
