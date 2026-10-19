//! Hydrofoil commands: a lofted front wing from NACA 4-digit sections.
//!
//! `foil.wing` is a one-shot command: it computes the sections in Rust and adds ordinary
//! features (one sketch per station on an XZ plane offset along +Y, a loft through them and a
//! mirror), so the result is an editable timeline. The station planes' offsets are expressions
//! in `span`, so a wing whose `span` names a parameter follows that parameter; the sections
//! themselves are fit splines of literal points.

use serde_json::{Value, json};
use solidcraft_doc::expr::Kind;

use super::{CommandSpec, not_in_sketch};
use crate::params::{bad, bool_, expr, num, str_};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("foil.wing", "Foil Wing from Stations", wing).at("SOLID", "CREATE").icon("loft").enabled(not_in_sketch).params(
    "span: expr (tip to tip, mm; a parameter name keeps the station planes following it); \
     root_chord, tip_chord: mm (the chord at fraction 0 and 1, interpolated for a station without `chord`); \
     stations?: [{fraction: 0…1 (of the half span, increasing), chord?: mm, camber?: % of chord, camber_pos?: % of chord, thickness?: % of chord, \
     sweep?: mm the pivot moves aft at this station, drop?: mm it moves down, twist?: degrees nose up}] \
     (default: root and tip); camber?, camber_pos?, thickness?: the station defaults (3, 40, 12); \
     te_thickness?: mm (default 0.6, at most 3 % of the chord); sweep?: degrees the pivot line sweeps back (default 0); pivot?: chord fraction on the sweep line (default 0.25); \
     twist?: degrees nose up for every station (incidence); x_offset?, z_offset?: mm the whole wing moves (its root pivot is at the origin); \
     points?: per surface (default 50); mirror?: bool (default true: one body across XZ); name?, body_name?",
)];

/// The two surfaces of a NACA 4-digit section, leading edge to trailing edge, in chord units:
/// `m` camber, `p` its position, `t` thickness (fractions of the chord), `te` the trailing-edge
/// thickness (fraction of the chord), `n` points per surface on a cosine spacing.
pub fn naca4(m: f64, p: f64, t: f64, te: f64, n: usize) -> (Vec<[f64; 2]>, Vec<[f64; 2]>) {
    let mut upper = Vec::with_capacity(n);
    let mut lower = Vec::with_capacity(n);
    for i in 0..n {
        let x = 0.5 * (1.0 - (std::f64::consts::PI * i as f64 / (n - 1) as f64).cos());
        let (yc, dyc) = if m.abs() < 1e-12 {
            (0.0, 0.0)
        } else if x < p {
            (m / (p * p) * (2.0 * p * x - x * x), 2.0 * m / (p * p) * (p - x))
        } else {
            let q = 1.0 - p;
            (m / (q * q) * ((1.0 - 2.0 * p) + 2.0 * p * x - x * x), 2.0 * m / (q * q) * (p - x))
        };
        let yt = 5.0 * t * (0.2969 * x.sqrt() - 0.1260 * x - 0.3516 * x * x + 0.2843 * x * x * x - 0.1036 * x * x * x * x) + 0.5 * te * x;
        let k = 1.0 / (1.0 + dyc * dyc).sqrt();
        let s = dyc * k;
        upper.push([x - yt * s, yc + yt * k]);
        lower.push([x + yt * s, yc - yt * k]);
    }
    (upper, lower)
}

/// One closed loop of a section (trailing edge over the top to the leading edge and back
/// under), in mm, with no repeated points: the input of a periodic fit spline.
pub fn section_loop(chord: f64, m: f64, p: f64, t: f64, te_mm: f64, n: usize) -> Vec<[f64; 2]> {
    let (upper, lower) = naca4(m, p, t, te_mm / chord, n);
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(2 * n);
    for q in upper.iter().rev().chain(lower.iter()) {
        let q = [q[0] * chord, q[1] * chord];
        let same = |a: &[f64; 2]| (a[0] - q[0]).abs() < 1e-9 && (a[1] - q[1]).abs() < 1e-9;
        if out.last().is_some_and(same) || (out.len() > 1 && out.first().is_some_and(same)) {
            continue;
        }
        out.push(q);
    }
    out
}

/// The area of a closed polygon (shoelace), for the report.
fn polygon_area(pts: &[[f64; 2]]) -> f64 {
    let mut a = 0.0;
    for (i, q) in pts.iter().enumerate() {
        let r = pts[(i + 1) % pts.len()];
        a += q[0] * r[1] - r[0] * q[1];
    }
    a.abs() / 2.0
}

struct Station {
    fraction: f64,
    chord: f64,
    camber: f64,
    camber_pos: f64,
    thickness: f64,
    /// The pivot's shift aft and down at this station (mm) and the section's twist (radians, nose up).
    sweep: f64,
    drop: f64,
    twist: f64,
}

/// A millimetre or degree value from `key`, or 0; within ±`limit`.
fn shift(v: &Value, key: &str, limit: f64, cmd: &str, what: &str) -> Result<f64> {
    let x = match v.get(key) {
        None | Some(Value::Null) => 0.0,
        Some(_) => num(v, key).ok_or_else(|| bad(cmd, format!("{what} `{key}` must be a number")))?,
    };
    if x.abs() > limit {
        return Err(bad(cmd, format!("{what} `{key}` is out of range (within ±{limit})")));
    }
    Ok(x)
}

/// A percentage of the chord from `key`, or `default`; camber may be 0, the rest must be positive.
fn percent(v: &Value, key: &str, default: f64, cmd: &str, what: &str) -> Result<f64> {
    let x = match v.get(key) {
        None | Some(Value::Null) => default,
        Some(_) => num(v, key).ok_or_else(|| bad(cmd, format!("{what} `{key}` must be a number (% of chord)")))?,
    };
    let ok = if key == "camber" {
        (0.0..=20.0).contains(&x)
    } else if key == "camber_pos" {
        x > 0.0 && x < 100.0
    } else {
        x > 0.0 && x <= 50.0
    };
    if !ok {
        return Err(bad(cmd, format!("{what} `{key}` is out of range ({x} % of chord)")));
    }
    Ok(x)
}

fn stations(p: &Value, cmd: &str, root: f64, tip: f64) -> Result<Vec<Station>> {
    let camber = percent(p, "camber", 3.0, cmd, "the default")?;
    let camber_pos = percent(p, "camber_pos", 40.0, cmd, "the default")?;
    let thickness = percent(p, "thickness", 12.0, cmd, "the default")?;
    let given: Vec<Value> = match p.get("stations") {
        None | Some(Value::Null) => vec![json!({"fraction": 0}), json!({"fraction": 1})],
        Some(Value::Array(a)) => a.clone(),
        Some(_) => return Err(bad(cmd, "`stations` must be a list of {fraction, chord?, camber?, camber_pos?, thickness?}")),
    };
    if given.len() < 2 || given.len() > 50 {
        return Err(bad(cmd, "a wing needs 2…50 stations"));
    }
    let mut out = Vec::with_capacity(given.len());
    for (i, st) in given.iter().enumerate() {
        let what = format!("station {}", i + 1);
        let fraction = num(st, "fraction").ok_or_else(|| bad(cmd, format!("{what} needs `fraction` (0…1 of the half span)")))?;
        if !(0.0..=1.0).contains(&fraction) {
            return Err(bad(cmd, format!("{what} `fraction` must be between 0 and 1")));
        }
        if i == 0 && fraction != 0.0 {
            return Err(bad(cmd, "the first station is the root: `fraction` 0"));
        }
        if let Some(prev) = out.last().map(|q: &Station| q.fraction)
            && fraction <= prev + 1e-9
        {
            return Err(bad(cmd, format!("{what} `fraction` must increase along the span")));
        }
        let chord = match st.get("chord") {
            None | Some(Value::Null) => root + (tip - root) * fraction,
            Some(_) => num(st, "chord").ok_or_else(|| bad(cmd, format!("{what} `chord` must be a number (mm)")))?,
        };
        if !(chord > 0.0 && chord < 1e5) {
            return Err(bad(cmd, format!("{what} `chord` must be positive")));
        }
        out.push(Station {
            fraction,
            chord,
            camber: percent(st, "camber", camber, cmd, &what)?,
            camber_pos: percent(st, "camber_pos", camber_pos, cmd, &what)?,
            thickness: percent(st, "thickness", thickness, cmd, &what)?,
            sweep: shift(st, "sweep", 1e4, cmd, &what)?,
            drop: shift(st, "drop", 1e4, cmd, &what)?,
            twist: shift(st, "twist", 45.0, cmd, &what)?.to_radians(),
        });
    }
    Ok(out)
}

fn wing(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "foil.wing";
    let span = expr(p, "span").ok_or_else(|| bad(cmd, "`span` must be a length (mm) or an expression"))?;
    let span_mm = s.doc.eval(&span, Kind::Length).map_err(|e| bad(cmd, format!("span: {e}")))?;
    if !(span_mm > 0.0 && span_mm < 1e5) {
        return Err(bad(cmd, "`span` must be positive"));
    }
    let span_literal = solidcraft_doc::expr::is_literal(&span);
    let chord_of = |key: &str| -> Result<f64> {
        let c = num(p, key).ok_or_else(|| bad(cmd, format!("`{key}` must be a number (mm)")))?;
        if c > 0.0 && c < 1e5 { Ok(c) } else { Err(bad(cmd, format!("`{key}` must be positive"))) }
    };
    let (root, tip) = (chord_of("root_chord")?, chord_of("tip_chord")?);
    let stations = stations(p, cmd, root, tip)?;
    let te = num(p, "te_thickness").unwrap_or(0.6);
    if !(0.0..=20.0).contains(&te) {
        return Err(bad(cmd, "`te_thickness` must be 0…20 mm"));
    }
    let sweep = num(p, "sweep").unwrap_or(0.0);
    if sweep.abs() >= 80.0 {
        return Err(bad(cmd, "`sweep` must be within ±80 degrees"));
    }
    let pivot = num(p, "pivot").unwrap_or(0.25);
    if !(0.0..=1.0).contains(&pivot) {
        return Err(bad(cmd, "`pivot` must be a chord fraction 0…1"));
    }
    let n = num(p, "points").map(|v| v as usize).unwrap_or(50);
    if !(8..=200).contains(&n) {
        return Err(bad(cmd, "`points` must be 8…200 per surface"));
    }
    let incidence = shift(p, "twist", 45.0, cmd, "the wing")?.to_radians();
    let x_offset = shift(p, "x_offset", 1e5, cmd, "the wing")?;
    let z_offset = shift(p, "z_offset", 1e5, cmd, "the wing")?;
    let mirror = bool_(p, "mirror").unwrap_or(true);
    let name = str_(p, "name").map(str::trim).filter(|t| !t.is_empty()).unwrap_or("Wing").to_string();
    let body = str_(p, "body_name").map(str::trim).filter(|t| !t.is_empty()).unwrap_or(&name).to_string();
    for key in ["name", "body_name"] {
        let taken = str_(p, key).unwrap_or(&name);
        if s.doc.find_feature(taken).is_some() {
            return Err(bad(cmd, format!("a feature named `{taken}` already exists; give the wing another `name`")));
        }
    }

    let run = |s: &mut Session, id: &str, params: Value| -> Result<Value> {
        crate::find_command(id).ok_or_else(|| bad(cmd, format!("`{id}` is unavailable")))?.run_nested(s, &params)
    };
    let slope = sweep.to_radians().tan();
    let mut sketches = Vec::new();
    let mut report = Vec::new();
    let mut areas = Vec::new();
    for (i, st) in stations.iter().enumerate() {
        let y = st.fraction * span_mm / 2.0;
        // The station plane: XZ offset along +Y, seen from behind so the sketch's y axis is +Z
        // (chord along +X, thickness up).
        let offset = if span_literal { format!("{}", y) } else { format!("{} * ({})", st.fraction / 2.0, span.as_str()) };
        let sketch_name = format!("{name} S{i}");
        let made = run(s, "sketch.create", json!({"plane": "XZ", "offset": offset, "flip": "x", "name": sketch_name}))?;
        // The station's pivot, and its section turned about it (nose up for a positive twist).
        let (px, pz) = (x_offset + slope * y + st.sweep, z_offset - st.drop);
        let x0 = px - pivot * st.chord;
        // A trailing edge no thicker than 3 % of the chord, so a tip of a few millimetres keeps its shape.
        let te_local = te.min(0.03 * st.chord);
        let pts = section_loop(st.chord, st.camber / 100.0, st.camber_pos / 100.0, st.thickness / 100.0, te_local, n);
        areas.push(polygon_area(&pts));
        let (sin, cos) = (incidence + st.twist).sin_cos();
        let points: Vec<Value> = pts
            .iter()
            .map(|q| {
                let (u, v) = (q[0] - pivot * st.chord, q[1]);
                json!([px + u * cos + v * sin, pz - u * sin + v * cos])
            })
            .collect();
        run(s, "sketch.spline.fit_point", json!({"points": points, "periodic": true}))?;
        run(s, "sketch.finish", json!({}))?;
        sketches.push(made.get("sketch").cloned().unwrap_or(Value::Null));
        report.push(
            json!({"sketch": made.get("sketch"), "name": made.get("name"), "y": y, "chord": st.chord, "pivot": [px, pz], "leading_edge_x": x0, "area_mm2": areas[i]}),
        );
    }
    let sections: Vec<Value> = sketches.iter().map(|id| json!({"sketch": id})).collect();
    let loft = run(s, "solid.loft", json!({"sections": sections, "operation": "new", "name": name, "body_name": body}))?;
    let mirrored = if mirror {
        // Mirrored as a body (a mirror of features keeps two bodies, whatever `combine` says).
        Some(run(s, "solid.mirror", json!({"bodies": [body], "plane": "XZ", "combine": true, "name": format!("{name} mirror")}))?)
    } else {
        None
    };
    // The volume a straight-sided loft would have, for a check against `measure`.
    let mut half = 0.0;
    for (i, w) in stations.windows(2).enumerate() {
        half += 0.5 * (areas[i] + areas[i + 1]) * (w[1].fraction - w[0].fraction) * span_mm / 2.0;
    }
    Ok(json!({
        "feature": loft.get("feature"),
        "name": loft.get("name"),
        "body": body,
        "mirror": mirrored.as_ref().and_then(|m| m.get("feature")),
        "stations": report,
        "half_volume_ruled_mm3": half,
        "note": "the smooth loft bulges a little between stations: measure gives the true volume",
    }))
}

// ---- The parametric foil features: one timeline feature per Onshape feature, every dialog
// value stored on it and the geometry rebuilt from them on each evaluation.

use solidcraft_doc::FeatureKind;
use solidcraft_doc::foil::{FoilParams, Tool};

const VALUES_DOC: &str = "values?: {field: value} (any field of the Onshape dialog by its FeatureScript name, e.g. targetArea, aspectRatio, chordShape; \
     numbers or expressions (a parameter name drives it), true/false, option ids such as SUPERELLIPSE; the rest keep the tool's defaults; \
     foil.fields lists every field with its group, bounds, default, options and when the dialog shows it)";

pub static FEATURE_COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("foil.wing_feature", "Foil Wing", wing_feature).at("SOLID", "CREATE").icon("loft").enabled(not_in_sketch).params(
        "values?: the Foil wing dialog's values (see foil.fields tool=wing); name?, body_name? (default Wing), operation?: new|join|cut|intersect, targets?: [body] \
         → a parametric feature rebuilt from its values on every edit (timeline.edit set {values: {...}} or foil.set change them)",
    ),
    CommandSpec::new("foil.stabiliser_feature", "Foil Stabiliser", stab_feature).at("SOLID", "CREATE").icon("loft").enabled(not_in_sketch).params(
        "values?: the Foil stabiliser dialog's values (arm, height, stabLink LINKED follows the last Foil wing above it; see foil.fields tool=stabiliser); \
         name?, body_name? (default Stabiliser), operation?, targets?",
    ),
    CommandSpec::new("foil.propeller_feature", "B-Series Propeller", prop_feature).at("SOLID", "CREATE").icon("revolve").enabled(not_in_sketch).params(
        "values?: the B-Series Propeller dialog's values (diameter, pdRatio, ear, bladeCount, rotation, hub, mount, rake, skew…; see foil.fields tool=propeller); \
         name?, body_name? (default Propeller), operation?, targets?",
    ),
    CommandSpec::new("foil.set", "Set Foil Values", set_values).icon("edit").params(
        "feature: id|name of a Foil wing, Foil stabiliser or B-Series Propeller feature, values: {field: value} (as foil.wing_feature) \
         → the feature rebuilt; its notice (area, span, AR, chords…)",
    ),
    CommandSpec::new("foil.fields", "Foil Dialog Fields", fields).icon("inspect").noundo().params(
        "tool?: wing|stabiliser|propeller, or feature?: id|name (then each field's current value and whether the dialog shows it, and the notice) \
         → every field in dialog order: key, label, group, kind, unit, min, default, max, options, when (the FeatureScript's show condition), help, built",
    ),
];

fn tool_of(name: &str) -> Option<Tool> {
    match name.to_ascii_lowercase().as_str() {
        "wing" | "foil wing" | "foilwing" => Some(Tool::Wing),
        "stab" | "stabiliser" | "stabilizer" | "foil stabiliser" | "foilstabiliser" => Some(Tool::Stabiliser),
        "prop" | "propeller" | "b-series propeller" => Some(Tool::Propeller),
        _ => None,
    }
}

/// Refuse parameters a foil command does not take: a misspelt `values` would otherwise build the
/// tool's defaults without a word.
fn known_keys(p: &Value, cmd: &str, keys: &[&str]) -> Result<()> {
    if let Some(m) = p.as_object()
        && let Some(k) = m.keys().find(|k| !keys.contains(&k.as_str()))
    {
        return Err(bad(cmd, format!("unknown parameter `{k}` (takes {})", keys.join(", "))));
    }
    Ok(())
}

fn params_from(tool: Tool, p: &Value, cmd: &str) -> Result<FoilParams> {
    known_keys(p, cmd, &["values", "name", "body_name", "body_names", "operation", "targets", "participants"])?;
    let mut fp = FoilParams::defaults(tool);
    match p.get("values") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            for (k, v) in m {
                fp.set_json(k, v).map_err(|e| bad(cmd, e))?;
            }
        }
        Some(_) => return Err(bad(cmd, format!("`values` must be an object of fields; {VALUES_DOC}"))),
    }
    Ok(fp)
}

fn add(s: &mut Session, p: &Value, tool: Tool, cmd: &str, body: &str) -> Result<Value> {
    let values = params_from(tool, p, cmd)?;
    let operation = match str_(p, "operation") {
        None => solidcraft_doc::Operation::NewBody,
        Some(o) => solidcraft_doc::Operation::parse(o).ok_or_else(|| bad(cmd, "`operation` must be new, join, cut or intersect"))?,
    };
    let targets = crate::params::string_list(p, "targets").into_iter().map(Into::into).collect();
    let kind = match tool {
        Tool::Wing => FeatureKind::FoilWing { values, operation, targets },
        Tool::Stabiliser => FeatureKind::FoilStabiliser { values, operation, targets },
        Tool::Propeller => FeatureKind::Propeller { values, operation, targets },
    };
    let mut p = p.clone();
    if let Some(o) = p.as_object_mut() {
        o.remove("targets");
        if !o.contains_key("body_name") && operation == solidcraft_doc::Operation::NewBody {
            o.insert("body_name".into(), json!(body));
        }
    }
    let mut r = super::features::add_feature(s, &p, kind)?;
    if let Some(id) = r.get("feature").and_then(Value::as_u64)
        && let Some(o) = r.as_object_mut()
    {
        o.insert("notice".into(), json!(notice(s, id)));
    }
    Ok(r)
}

/// The feature's summary line, as the Onshape feature's notice gives it.
fn notice(s: &Session, id: u64) -> Option<String> {
    let f = s.doc.feature(id)?;
    let (vals, _) = s.doc.param_values();
    solidcraft_doc::foil::summary(&s.doc, &vals, f).ok().map(|(n, w)| if w.is_empty() { n } else { format!("{n} | {}", w.join(" | ")) })
}

fn wing_feature(s: &mut Session, p: &Value) -> Result<Value> {
    add(s, p, Tool::Wing, "foil.wing_feature", "Wing")
}

fn stab_feature(s: &mut Session, p: &Value) -> Result<Value> {
    add(s, p, Tool::Stabiliser, "foil.stabiliser_feature", "Stabiliser")
}

fn prop_feature(s: &mut Session, p: &Value) -> Result<Value> {
    add(s, p, Tool::Propeller, "foil.propeller_feature", "Propeller")
}

fn foil_feature(s: &Session, p: &Value, cmd: &str) -> Result<(u64, FoilParams)> {
    let key = match p.get("feature") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(x)) => x.clone(),
        _ => return Err(bad(cmd, "`feature` must be an id or name")),
    };
    let f = s.doc.find_feature(&key).ok_or_else(|| bad(cmd, format!("no feature `{key}`")))?;
    match &f.kind {
        FeatureKind::FoilWing { values, .. } | FeatureKind::FoilStabiliser { values, .. } | FeatureKind::Propeller { values, .. } => {
            Ok((f.id, values.clone()))
        }
        _ => Err(bad(cmd, format!("`{key}` is not a Foil wing, Foil stabiliser or B-Series Propeller feature"))),
    }
}

fn set_values(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "foil.set";
    known_keys(p, cmd, &["feature", "values"])?;
    let (id, mut values) = foil_feature(s, p, cmd)?;
    let Some(Value::Object(m)) = p.get("values") else { return Err(bad(cmd, format!("`values` must be an object of fields; {VALUES_DOC}"))) };
    for (k, v) in m {
        values.set_json(k, v).map_err(|e| bad(cmd, e))?;
    }
    if let Some(f) = s.doc_mut().feature_mut(id) {
        match &mut f.kind {
            FeatureKind::FoilWing { values: slot, .. }
            | FeatureKind::FoilStabiliser { values: slot, .. }
            | FeatureKind::Propeller { values: slot, .. } => {
                *slot = values;
            }
            _ => {}
        }
    }
    s.refresh();
    if let Some(e) = s.model.result(id).and_then(|r| r.error.clone()) {
        return Err(EngineError::Other(e));
    }
    let warning = s.model.result(id).and_then(|r| r.warning.clone());
    Ok(json!({"feature": id, "notice": notice(s, id), "warning": warning, "recomputed": s.model.last_recomputed}))
}

fn fields(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "foil.fields";
    known_keys(p, cmd, &["tool", "feature"])?;
    let (tool, current) = if p.get("feature").is_some() {
        let (id, values) = foil_feature(s, p, cmd)?;
        (values.tool, Some((id, values)))
    } else {
        let t = str_(p, "tool").unwrap_or("wing");
        (tool_of(t).ok_or_else(|| bad(cmd, "`tool` must be wing, stabiliser or propeller"))?, None)
    };
    let shown = current.as_ref().and_then(|(_, v)| v.values(&s.doc.param_values().0).ok());
    let mut out = Vec::new();
    for f in tool.fields() {
        let mut row = json!({
            "key": f.key, "label": f.label, "group": f.group,
            "kind": match f.kind { solidcraft_doc::foil::FieldKind::Bool { .. } => "bool", solidcraft_doc::foil::FieldKind::Enum { .. } => "enum",
                solidcraft_doc::foil::FieldKind::Int { .. } => "integer", solidcraft_doc::foil::FieldKind::Length { .. } => "length",
                solidcraft_doc::foil::FieldKind::Angle { .. } => "angle", _ => "number" },
            "when": f.when, "help": solidcraft_doc::foil::schema::plain_help(f.help),
            "built": !solidcraft_doc::foil::schema::not_built(tool, f),
        });
        if let Some(o) = row.as_object_mut() {
            if let Some((lo, d, hi)) = f.kind.bounds() {
                o.insert("min".into(), json!(lo));
                o.insert("default".into(), json!(d));
                o.insert("max".into(), json!(hi));
                if !f.kind.unit().is_empty() {
                    o.insert("unit".into(), json!(f.kind.unit()));
                }
            }
            match f.kind {
                solidcraft_doc::foil::FieldKind::Bool { default } => {
                    o.insert("default".into(), json!(default));
                }
                solidcraft_doc::foil::FieldKind::Enum { options, default } => {
                    o.insert("default".into(), json!(default));
                    o.insert("options".into(), json!(options.iter().map(|(id, label)| json!({"id": id, "label": label})).collect::<Vec<_>>()));
                }
                _ => {}
            }
            if let Some((_, v)) = &current {
                o.insert("value".into(), v.to_json().get(f.key).cloned().unwrap_or(Value::Null));
            }
            if let Some(vals) = &shown {
                o.insert("shown".into(), json!(vals.shows(f)));
            }
        }
        out.push(row);
    }
    let notice = current.as_ref().and_then(|(id, _)| notice(s, *id));
    Ok(json!({"tool": tool.title(), "fields": out, "notice": notice}))
}
