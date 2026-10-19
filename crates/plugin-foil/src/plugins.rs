//! The Foil wing, Foil stabiliser and B-Series Propeller as SolidCraft plugins: their dialogs
//! from the FeatureScript's (every field, its group, bounds, default, condition and help), and
//! their geometry built through the host's commands only (`solid.loft_points`, `solid.mirror`,
//! sketches and revolves, cylinders and boxes, `solid.move`, `solid.combine`).

use std::f64::consts::PI;
use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_plugin::{Display, FallbackPolicy, Field, FieldKind, Host, Manifest, Plugin, Schema, Unit, run};

use crate::prop::{self, BladeSection, Cut};
use crate::schema::{self, Kind, Tool, Val, Values};
use crate::wing::{self, Surface, WingLink};

pub const WING: &str = "foil.wing";
pub const STABILISER: &str = "foil.stabiliser";
pub const PROPELLER: &str = "foil.propeller";

/// Register the three plugins (once; an app calls it at start-up).
pub fn register() {
    for p in [Arc::new(FoilPlugin(Tool::Wing)) as Arc<dyn Plugin>, Arc::new(FoilPlugin(Tool::Stabiliser)), Arc::new(FoilPlugin(Tool::Propeller))] {
        let _ = solidcraft_plugin::register(p);
    }
}

/// One of the three tools as a plugin.
pub struct FoilPlugin(pub Tool);

/// The tool's dialog as the plugin contract's schema.
pub fn schema_of(tool: Tool) -> Schema {
    let fields = tool
        .fields()
        .iter()
        .map(|f| {
            let kind = match f.kind {
                Kind::Real { min, default, max } => FieldKind::Number { min, default, max, unit: Unit::None, integer: false },
                Kind::Length { min, default, max } => FieldKind::Number { min, default, max, unit: Unit::Length, integer: false },
                Kind::Angle { min, default, max } => FieldKind::Number { min, default, max, unit: Unit::Angle, integer: false },
                Kind::Int { min, default, max } => FieldKind::Number { min, default, max, unit: Unit::None, integer: true },
                Kind::Bool { default } => FieldKind::Bool { default },
                Kind::Enum { options, default } => FieldKind::Choice {
                    options: options.iter().map(|(id, l)| (id.to_string(), l.to_string())).collect(),
                    default: default.to_string(),
                },
            };
            Field {
                key: f.key.to_string(),
                label: f.label.to_string(),
                group: f.group.to_string(),
                collapsed: f.collapsed,
                kind,
                when: f.when.to_string(),
                help: schema::plain_help(f.help),
                unused: schema::not_built(tool, f),
            }
        })
        .collect();
    Schema { fields }
}

/// The plugin's parameters (numbers already evaluated by the host) as the laws take them.
pub fn values_of(tool: Tool, p: &Value) -> Result<Values, String> {
    let mut v = Values::defaults(tool);
    for f in tool.fields() {
        let Some(x) = p.get(f.key) else { continue };
        let val = match (&f.kind, x) {
            (Kind::Bool { .. }, Value::Bool(b)) => Val::Flag(*b),
            (Kind::Enum { .. }, Value::String(s)) => Val::Choice(s.clone()),
            (k, Value::Number(n)) if k.is_number() => Val::Num(n.as_f64().unwrap_or(0.0)),
            _ => return Err(format!("{}: {x} is the wrong kind of value", f.label)),
        };
        v.set(f.key, val)?;
    }
    Ok(v)
}

fn link_of(ctx: &Value) -> Option<WingLink> {
    let w = ctx.get("published")?.get(WING)?;
    let n = |k: &str| w.get(k).and_then(Value::as_f64);
    Some(WingLink {
        area: n("area")?,
        aspect: n("aspect")?,
        root_thickness: n("root_thickness")?,
        tip_thickness: n("tip_thickness")?,
        incidence: n("incidence")?,
    })
}

/// A wing or stabiliser surface from its values (the stab on its arm, following the wing above
/// it when linked).
fn surface(tool: Tool, p: &Value, ctx: &Value) -> Result<Surface, String> {
    let v = values_of(tool, p)?;
    if tool == Tool::Stabiliser {
        let link = if v.choice("stabLink") == "LINKED" { link_of(ctx) } else { None };
        return wing::build(&v, [v.num("arm"), 0.0, v.num("height")], link);
    }
    wing::build(&v, [0.0; 3], None)
}

/// The surface's loft: each station's section as its plane places it (the foil spline and the
/// straight trailing edge), mirrored and joined when the dialog says so.
fn build_surface(host: &mut dyn Host, s: &Surface) -> Result<(), String> {
    let sections: Vec<Value> = s
        .stations
        .iter()
        .map(|st| {
            let pts: Vec<[f64; 3]> = st.points.iter().map(|q| st.frame.world(*q)).collect();
            let (Some(a), Some(b)) = (pts.last().copied(), pts.first().copied()) else { return json!([]) };
            json!([pts, [a, b]])
        })
        .collect();
    run(host, "solid.loft_points", json!({"sections": sections, "body_name": "Surface"}))?;
    if s.mirror {
        run(host, "solid.mirror", json!({"bodies": ["Surface"], "plane": "XZ", "combine": true}))?;
    }
    Ok(())
}

fn publish(s: &Surface) -> Value {
    json!({"area": s.area_proj, "aspect": s.aspect, "root_thickness": s.root_thickness, "tip_thickness": s.tip_thickness, "incidence": s.incidence,
        "span": s.span, "root_chord": s.root_chord, "mac": s.mac})
}

impl Plugin for FoilPlugin {
    fn manifest(&self) -> Manifest {
        let (id, name, icon, about, preview) = match self.0 {
            Tool::Wing => (WING, "Foil Wing", "loft", "A hydrofoil front wing from its planform, section and tip (the Onshape Foil wing).", true),
            Tool::Stabiliser => {
                (STABILISER, "Foil Stabiliser", "loft", "A hydrofoil stabiliser on its arm, independent or following the wing above it.", true)
            }
            Tool::Propeller => (PROPELLER, "B-Series Propeller", "revolve", "A Wageningen B-series propeller on its hub, with its mount.", false),
        };
        Manifest {
            id: id.into(),
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            api_version: solidcraft_plugin::API_VERSION,
            description: about.into(),
            display: Display {
                label: match self.0 {
                    Tool::Wing => "Wing",
                    Tool::Stabiliser => "Stabiliser",
                    Tool::Propeller => "Propeller",
                }
                .into(),
                icon_id: icon.into(),
                tooltip: about.into(),
                type_name: self.0.title().into(),
            },
            fallback: FallbackPolicy::RecordedMesh,
            preview,
        }
    }

    fn schema(&self) -> Schema {
        schema_of(self.0)
    }

    fn evaluate(&self, p: &Value, ctx: &Value, host: &mut dyn Host) -> Result<Value, String> {
        match self.0 {
            Tool::Wing | Tool::Stabiliser => {
                let s = surface(self.0, p, ctx)?;
                build_surface(host, &s)?;
                let mut out = json!({"notice": s.notice});
                if self.0 == Tool::Wing {
                    out["publish"] = publish(&s);
                }
                Ok(out)
            }
            Tool::Propeller => {
                let pr = prop::build(&values_of(Tool::Propeller, p)?)?;
                build_propeller(host, &pr)?;
                Ok(json!({"notice": pr.notice, "warnings": pr.warnings}))
            }
        }
    }

    fn describe(&self, p: &Value, ctx: &Value) -> Result<Value, String> {
        match self.0 {
            Tool::Wing | Tool::Stabiliser => {
                let v = values_of(self.0, p)?;
                let s = surface(self.0, p, ctx)?;
                let mut notice = s.notice;
                if self.0 == Tool::Stabiliser && v.choice("stabLink") == "LINKED" && link_of(ctx).is_none() {
                    notice += " | linked, but no Foil wing above it: its own values are used";
                }
                Ok(json!({"notice": notice}))
            }
            Tool::Propeller => {
                let pr = prop::build(&values_of(Tool::Propeller, p)?)?;
                Ok(json!({"notice": pr.notice, "warnings": pr.warnings}))
            }
        }
    }
}

// ---- the propeller through commands

/// Points along each blade surface per section.
const BLADE_POINTS: usize = 28;
/// Points across the trailing edge per section (every section needs as many).
const TE_POINTS: usize = 4;

/// The natural cubic spline through `p` by chord length, sampled at `m` points closer together
/// towards the start (the leading edge) on a half-cosine spacing of its length: every blade
/// section gets the same count, whatever its rounded nose took.
fn resample(p: &[[f64; 2]], m: usize) -> Vec<[f64; 2]> {
    let n = p.len();
    if n < 3 || m < 2 {
        return p.to_vec();
    }
    let h: Vec<f64> = p.windows(2).map(|w| ((w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).max(1e-9)).collect();
    // Second derivatives, natural ends (tridiagonal solve), each coordinate.
    let mm = n - 2;
    let mut sec = vec![[0.0; 2]; n];
    for c in 0..2 {
        let (mut diag, mut up, mut low, mut rhs) = (vec![0.0; mm], vec![0.0; mm], vec![0.0; mm], vec![0.0; mm]);
        for i in 0..mm {
            diag[i] = 2.0 * (h[i] + h[i + 1]);
            low[i] = h[i];
            up[i] = h[i + 1];
            rhs[i] = 6.0 * ((p[i + 2][c] - p[i + 1][c]) / h[i + 1] - (p[i + 1][c] - p[i][c]) / h[i]);
        }
        for i in 1..mm {
            let w = low[i] / diag[i - 1];
            diag[i] -= w * up[i - 1];
            rhs[i] -= w * rhs[i - 1];
        }
        let mut sol = vec![0.0; mm];
        for i in (0..mm).rev() {
            let next = if i + 1 < mm { sol[i + 1] * up[i] } else { 0.0 };
            sol[i] = (rhs[i] - next) / diag[i];
        }
        for i in 0..mm {
            sec[i + 1][c] = sol[i];
        }
    }
    // The spline densely, with its length along it.
    let at = |i: usize, t: f64| -> [f64; 2] {
        let hi = h[i];
        let (a, b) = (1.0 - t, t);
        [0, 1].map(|c| a * p[i][c] + b * p[i + 1][c] + ((a * a * a - a) * sec[i][c] + (b * b * b - b) * sec[i + 1][c]) * hi * hi / 6.0)
    };
    let mut dense = Vec::new();
    for i in 0..n - 1 {
        for k in 0..16 {
            dense.push(at(i, k as f64 / 16.0));
        }
    }
    dense.push(p[n - 1]);
    let mut len = vec![0.0];
    for w in dense.windows(2) {
        len.push(len[len.len() - 1] + (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]));
    }
    let total = len[len.len() - 1];
    let mut out = Vec::with_capacity(m);
    let mut j = 0;
    for i in 0..m {
        let target = total * (1.0 - (PI / 2.0 * i as f64 / (m - 1) as f64).cos());
        while j + 2 < len.len() && len[j + 1] < target {
            j += 1;
        }
        let f = if len[j + 1] > len[j] { ((target - len[j]) / (len[j + 1] - len[j])).clamp(0.0, 1.0) } else { 0.0 };
        out.push([dense[j][0] + (dense[j + 1][0] - dense[j][0]) * f, dense[j][1] + (dense[j + 1][1] - dense[j][1]) * f]);
    }
    out
}

/// A blade section's loft section: back (leading to trailing edge), the trailing edge, face (back
/// to the leading edge), each point wrapped onto the section's cylinder. The root section is
/// flattened onto the plane through its ends (it lies inside the hub), the tip's onto its
/// tangent plane (it spans a few degrees): they are the loft's planar caps.
fn blade_section(s: &BladeSection, flat: Option<bool>) -> Value {
    let n0 = [-s.hand * s.theta0.sin(), s.theta0.cos()];
    let t0 = [-s.hand * s.theta0.cos(), -s.theta0.sin()];
    let r_flat = match flat {
        Some(true) => s.r * s.back.iter().chain(&s.face).map(|q| (q[0] / s.r).cos()).fold(1.0, f64::min),
        _ => s.r,
    };
    let place = |q: [f64; 2]| -> [f64; 3] {
        let a = q[0] / s.r;
        let (radial, tangential) = if flat.is_some() { (r_flat, s.r * a.sin()) } else { (s.r * a.cos(), s.r * a.sin()) };
        [n0[0] * radial + t0[0] * tangential, n0[1] * radial + t0[1] * tangential, q[1]]
    };
    let back = resample(&s.back, BLADE_POINTS);
    let mut face = resample(&s.face, BLADE_POINTS);
    face.reverse();
    let (Some(&te_b), Some(&te_f)) = (back.last(), face.first()) else { return json!([]) };
    let te: Vec<[f64; 3]> = (0..TE_POINTS)
        .map(|k| k as f64 / (TE_POINTS - 1) as f64)
        .map(|t| place([te_b[0] + (te_f[0] - te_b[0]) * t, te_b[1] + (te_f[1] - te_b[1]) * t]))
        .collect();
    let back: Vec<[f64; 3]> = back.into_iter().map(place).collect();
    let face: Vec<[f64; 3]> = face.into_iter().map(place).collect();
    json!([back, te, face])
}

/// A cut of `target` by `c` (grown by `grow` all round).
fn cut(host: &mut dyn Host, c: &Cut, target: &str, grow: f64) -> Result<(), String> {
    let c = if grow > 0.0 { c.grown(grow) } else { c.clone() };
    let op = json!({"operation": "cut", "targets": [target]});
    let with = |mut v: Value| {
        if let (Value::Object(m), Value::Object(o)) = (&mut v, &op) {
            m.extend(o.clone());
        }
        v
    };
    match &c {
        Cut::Cylinder { x, y, z0, z1, r } => {
            run(host, "solid.cylinder", with(json!({"radius": r, "height": z1 - z0, "base": [x, y, z0]})))?;
        }
        Cut::Box { lo, hi } => {
            run(host, "solid.box", with(json!({"corner": lo, "length": hi[0] - lo[0], "width": hi[1] - lo[1], "height": hi[2] - lo[2]})))?;
        }
        Cut::Revolve(outline) => revolve(host, [0.0, 0.0], outline, Some(target))?,
        Cut::Bolt { x, y, z0, z1, r, head, head_top } => {
            let outline: Vec<[f64; 2]> = if head > r {
                vec![[0.0, *z0], [*head, *z0], [*head, *head_top], [*r, *head_top], [*r, *z1], [0.0, *z1]]
            } else {
                vec![[0.0, *z0], [*r, *z0], [*r, *z1], [0.0, *z1]]
            };
            revolve(host, [*x, *y], &outline, Some(target))?;
        }
    }
    Ok(())
}

/// A solid of revolution about the vertical line through `at` from an (r, z) outline: a new body
/// "Hub", or a cut of `cut_target`.
fn revolve(host: &mut dyn Host, at: [f64; 2], outline: &[[f64; 2]], cut_target: Option<&str>) -> Result<(), String> {
    let sk = run(host, "sketch.create", json!({"plane": {"origin": [at[0], at[1], 0], "x_dir": [1, 0, 0], "y_dir": [0, 0, 1]}}))?;
    run(host, "sketch.line", json!({"points": outline, "closed": true, "infer": false, "connect": false}))?;
    run(host, "sketch.finish", json!({}))?;
    let mut p = json!({"sketch": sk.get("sketch").cloned().unwrap_or(Value::Null), "axis": "y", "angle": "360 deg"});
    match cut_target {
        Some(t) => {
            p["operation"] = json!("cut");
            p["targets"] = json!([t]);
        }
        None => {
            p["operation"] = json!("new");
            p["body_name"] = json!("Hub");
        }
    }
    run(host, "solid.revolve", p)?;
    Ok(())
}

/// The propeller: the hub with its mount cut, one blade trimmed to the hub's ends, copied round,
/// each copy cut by the mount tools that reach it (grown, so their faces lie in the hub's
/// material) and joined; turned aft up. The FeatureScript joins first and cuts after; cutting
/// while the bodies are simple gives the same solid much faster.
fn build_propeller(host: &mut dyn Host, p: &prop::Prop) -> Result<(), String> {
    revolve(host, [0.0, 0.0], &p.hub, None)?;
    for c in &p.cuts {
        cut(host, c, "Hub", 0.0)?;
    }
    let last = p.sections.len().saturating_sub(1);
    let sections: Vec<Value> = p
        .sections
        .iter()
        .enumerate()
        .map(|(i, s)| {
            blade_section(
                s,
                if i == 0 {
                    Some(true)
                } else if i == last {
                    Some(false)
                } else {
                    None
                },
            )
        })
        .collect();
    run(host, "solid.loft_points", json!({"sections": sections, "body_name": "Blade"}))?;
    for t in &p.blade_trims {
        cut(host, t, "Blade", 0.0)?;
    }
    let root = p.sections.first().map(|s| s.r * s.back.iter().chain(&s.face).map(|q| (q[0] / s.r).cos()).fold(1.0, f64::min)).unwrap_or(0.0);
    let mut blades = vec!["Blade".to_string()];
    for k in 1..p.blades {
        let r = run(
            host,
            "solid.move",
            json!({"bodies": ["Blade"], "axis": [0, 0, 1], "angle": format!("{} deg", 360.0 * k as f64 / p.blades as f64), "copy": true}),
        )?;
        let name =
            r.get("created_bodies").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str).ok_or("the blade copy has no name")?;
        blades.push(name.to_string());
    }
    for b in &blades {
        for c in p.cuts.iter().filter(|c| c.reach() > root - 0.3) {
            cut(host, c, b, 0.3)?;
        }
        run(host, "solid.combine", json!({"target": "Hub", "tools": [b], "operation": "join"}))?;
    }
    run(host, "solid.move", json!({"bodies": ["Hub"], "axis": [1, 0, 0], "angle": "180 deg"}))?;
    Ok(())
}
