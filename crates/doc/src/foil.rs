//! The foil features (`FoilWing`, `FoilStabiliser`, `Propeller`): their dialog values as stored
//! in the design, and their bodies, rebuilt from those values on every evaluation by the laws in
//! `solidcraft-foil` (a port of the Onshape Foil design tool and B-Series propeller).
//!
//! Every number of the dialog is an expression and a feature input (so it gets a `dN` name and
//! a parameter can drive it); flags and enum options are stored as they are. A design keeps every
//! value, the defaults included, as a flat map from the FeatureScript's field names.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
pub use solidcraft_foil::{Field, Kind as FieldKind, Tool, Val, Values, prop, schema, section, wing};
use solidcraft_geom::{Loop2, Plane, Region2, Vec2, Vec3};
use solidcraft_kernel::{self as kernel, Body, BoolOp};

use crate::expr::{Kind, Value};
use crate::walk::{ExprItem, ExprItemMut, ExprRole, Walk};
use crate::{DocError, Document, Expr, Feature, FeatureKind, Result};

/// A foil feature's dialog values.
#[derive(Clone, Debug, PartialEq)]
pub struct FoilParams {
    pub tool: Tool,
    /// Every number field's expression.
    pub exprs: BTreeMap<String, Expr>,
    pub flags: BTreeMap<String, bool>,
    /// Enum fields: the option's id (`SUPERELLIPSE`).
    pub choices: BTreeMap<String, String>,
}

/// A default number as an expression: the FeatureScript's default, plain.
fn number_text(x: f64) -> String {
    let s = format!("{x}");
    if s.contains('e') { format!("{x:.6}") } else { s }
}

/// The expression kind of a field.
pub fn expr_kind(k: &FieldKind) -> Kind {
    match k {
        FieldKind::Length { .. } => Kind::Length,
        FieldKind::Angle { .. } => Kind::Angle,
        _ => Kind::Unitless,
    }
}

impl FoilParams {
    /// The FeatureScript's defaults.
    pub fn defaults(tool: Tool) -> FoilParams {
        let mut p = FoilParams { tool, exprs: BTreeMap::new(), flags: BTreeMap::new(), choices: BTreeMap::new() };
        p.fill_defaults();
        p
    }

    /// Give every field without a value its default (a design from an older build of the tool).
    pub fn fill_defaults(&mut self) {
        for f in self.tool.fields() {
            match f.kind {
                FieldKind::Bool { default } => {
                    self.flags.entry(f.key.to_string()).or_insert(default);
                }
                FieldKind::Enum { default, .. } => {
                    self.choices.entry(f.key.to_string()).or_insert_with(|| default.to_string());
                }
                k => {
                    let d = k.bounds().map(|(_, d, _)| d).unwrap_or(0.0);
                    self.exprs.entry(f.key.to_string()).or_insert_with(|| Expr::new(number_text(d)));
                }
            }
        }
    }

    /// Set one field from JSON: a number or an expression for a number field, a boolean for a
    /// flag, an option id or label for an enum. Unknown keys and wrong kinds are refused.
    pub fn set_json(&mut self, key: &str, v: &serde_json::Value) -> std::result::Result<(), String> {
        let f = self.tool.field(key).ok_or_else(|| format!("{} has no field `{key}`", self.tool.title()))?;
        match (&f.kind, v) {
            (FieldKind::Bool { .. }, serde_json::Value::Bool(b)) => {
                self.flags.insert(f.key.to_string(), *b);
            }
            (FieldKind::Enum { .. }, serde_json::Value::String(s)) => {
                let Ok(Val::Choice(id)) = solidcraft_foil::schema::check(f, Val::Choice(s.clone())) else {
                    return Err(solidcraft_foil::schema::check(f, Val::Choice(s.clone())).err().unwrap_or_default());
                };
                self.choices.insert(f.key.to_string(), id);
            }
            (k, serde_json::Value::Number(n)) if k.is_number() => {
                let x = n.as_f64().ok_or_else(|| format!("{}: not a number", f.label))?;
                solidcraft_foil::schema::check(f, Val::Num(x))?;
                self.exprs.insert(f.key.to_string(), Expr::new(number_text(x)));
            }
            (k, serde_json::Value::String(s)) if k.is_number() => {
                if s.trim().is_empty() {
                    return Err(format!("{}: empty expression", f.label));
                }
                self.exprs.insert(f.key.to_string(), Expr::new(s.trim()));
            }
            _ => return Err(format!("{}: {v} is the wrong kind of value", f.label)),
        }
        Ok(())
    }

    /// The values as the laws take them: every expression evaluated (lengths in mm, angles in
    /// degrees) and checked against the field's bounds.
    pub fn values(&self, vals: &BTreeMap<String, Value>) -> Result<Values> {
        let mut v = Values::defaults(self.tool);
        let title = self.tool.title();
        for f in self.tool.fields() {
            let x = match f.kind {
                FieldKind::Bool { default } => Val::Flag(self.flags.get(f.key).copied().unwrap_or(default)),
                FieldKind::Enum { default, .. } => Val::Choice(self.choices.get(f.key).cloned().unwrap_or_else(|| default.to_string())),
                k => {
                    let e = self.exprs.get(f.key).map(Expr::as_str).unwrap_or("0");
                    let ek = expr_kind(&k);
                    let x = crate::eval::val(vals, e, ek).map_err(|err| DocError::Invalid(format!("{title}: {}: {err}", f.label)))?;
                    Val::Num(if ek == Kind::Angle { x.to_degrees() } else { x })
                }
            };
            v.set(f.key, x).map_err(|e| DocError::Invalid(format!("{title}: {e}")))?;
        }
        Ok(v)
    }

    /// The values as JSON (numbers as expressions), in dialog order.
    pub fn to_json(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        for f in self.tool.fields() {
            let v = match f.kind {
                FieldKind::Bool { default } => serde_json::Value::Bool(self.flags.get(f.key).copied().unwrap_or(default)),
                FieldKind::Enum { default, .. } => serde_json::Value::String(self.choices.get(f.key).cloned().unwrap_or_else(|| default.to_string())),
                _ => serde_json::Value::String(self.exprs.get(f.key).map(|e| e.as_str().to_string()).unwrap_or_default()),
            };
            m.insert(f.key.to_string(), v);
        }
        serde_json::Value::Object(m)
    }

    fn from_json(tool: Tool, map: BTreeMap<String, serde_json::Value>) -> FoilParams {
        let mut p = FoilParams { tool, exprs: BTreeMap::new(), flags: BTreeMap::new(), choices: BTreeMap::new() };
        for (k, v) in &map {
            // Kept as written: a value out of range or an option this build does not know shows
            // as the feature's error when it is evaluated. A field this build does not know is
            // dropped; a missing one takes its default.
            let Some(f) = tool.field(k) else { continue };
            match (&f.kind, v) {
                (FieldKind::Bool { .. }, serde_json::Value::Bool(b)) => {
                    p.flags.insert(f.key.to_string(), *b);
                }
                (FieldKind::Enum { .. }, serde_json::Value::String(s)) => {
                    let id = match solidcraft_foil::schema::check(f, Val::Choice(s.clone())) {
                        Ok(Val::Choice(id)) => id,
                        _ => s.clone(),
                    };
                    p.choices.insert(f.key.to_string(), id);
                }
                (k, serde_json::Value::String(s)) if k.is_number() => {
                    p.exprs.insert(f.key.to_string(), Expr::new(s.clone()));
                }
                (k, serde_json::Value::Number(n)) if k.is_number() => {
                    p.exprs.insert(f.key.to_string(), Expr::new(n.as_f64().map(number_text).unwrap_or_else(|| n.to_string())));
                }
                _ => {}
            }
        }
        p.fill_defaults();
        p
    }
}

impl Serialize for FoilParams {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        self.to_json().serialize(s)
    }
}

fn read<'de, D: Deserializer<'de>>(d: D, tool: Tool) -> std::result::Result<FoilParams, D::Error> {
    Ok(FoilParams::from_json(tool, BTreeMap::<String, serde_json::Value>::deserialize(d)?))
}

pub(crate) fn read_wing<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<FoilParams, D::Error> {
    read(d, Tool::Wing)
}
pub(crate) fn read_stab<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<FoilParams, D::Error> {
    read(d, Tool::Stabiliser)
}
pub(crate) fn read_prop<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<FoilParams, D::Error> {
    read(d, Tool::Propeller)
}

/// Every number field is an input, in dialog order, whether the dialog shows it or not, so the
/// inputs' `dN` names never move when an option changes.
impl Walk for FoilParams {
    fn walk_exprs<'a>(&'a self, f: &mut dyn FnMut(ExprItem<'a>)) {
        for field in self.tool.fields() {
            if let Some(e) = self.exprs.get(field.key) {
                f((field.label, e, expr_kind(&field.kind), ExprRole::Input));
            }
        }
    }
    fn walk_exprs_mut<'a>(&'a mut self, f: &mut dyn FnMut(ExprItemMut<'a>)) {
        let tool = self.tool;
        let mut refs: std::collections::HashMap<&str, &'a mut Expr> = self.exprs.iter_mut().map(|(k, e)| (k.as_str(), e)).collect();
        for field in tool.fields() {
            if let Some(e) = refs.remove(field.key) {
                f((field.label, e, expr_kind(&field.kind), ExprRole::Input));
            }
        }
    }
}

// ---- geometry

fn v3(a: [f64; 3]) -> Vec3 {
    Vec3::new(a[0], a[1], a[2])
}

/// Column-major 4×4: a linear map `l` (rows of the 3×3) and no translation.
fn linear(l: [[f64; 3]; 3]) -> [[f64; 4]; 4] {
    [[l[0][0], l[1][0], l[2][0], 0.0], [l[0][1], l[1][1], l[2][1], 0.0], [l[0][2], l[1][2], l[2][2], 0.0], [0.0, 0.0, 0.0, 1.0]]
}

/// An open fit spline through `pts` as cubic Béziers (the FeatureScript's `skFitSpline`).
fn fit(pts: &[Vec2]) -> Result<Vec<[Vec2; 4]>> {
    solidcraft_sketch::fit_beziers_mode(pts, false).map_err(|e| DocError::Invalid(format!("section spline: {e}")))
}

fn union(a: Body, b: &Body) -> Result<Body> {
    kernel::boolean(&a, b, BoolOp::Union)?.ok_or_else(|| DocError::Invalid("the join came out empty".into()))
}

fn cut(a: Body, b: &Body) -> Result<Body> {
    kernel::boolean(&a, b, BoolOp::Cut)?.ok_or_else(|| DocError::Invalid("the cut left nothing".into()))
}

/// Cut `a` by every tool: those clear of each other as one tool of several pieces, in one cut.
fn cut_all(mut a: Body, tools: &[Body]) -> Result<Body> {
    if tools.len() > 1
        && let Ok(Some(all)) = kernel::union_apart(tools)
    {
        return cut(a, &all);
    }
    for t in tools {
        a = cut(a, t)?;
    }
    Ok(a)
}

/// A straight piece as a cubic Bézier.
fn line3(a: Vec3, b: Vec3) -> [Vec3; 4] {
    [a, a + (b - a) * (1.0 / 3.0), a + (b - a) * (2.0 / 3.0), b]
}

/// The cubic through `q0..q3` at t = 0, 1/3, 2/3, 1.
fn cubic_through(q: [Vec3; 4]) -> [Vec3; 4] {
    let r1 = q[1] * 27.0 - q[0] * 8.0 - q[3];
    let r2 = q[2] * 27.0 - q[0] - q[3] * 8.0;
    [q[0], (r1 * 2.0 - r2) * (1.0 / 18.0), (r2 * 2.0 - r1) * (1.0 / 18.0), q[3]]
}

fn bezier_point(b: &[Vec2; 4], t: f64) -> Vec2 {
    let u = 1.0 - t;
    b[0] * (u * u * u) + b[1] * (3.0 * u * u * t) + b[2] * (3.0 * u * t * t) + b[3] * (t * t * t)
}

/// A wing or stabiliser section in its plane as the loft takes it: the foil spline (upper
/// trailing edge round the nose to the lower) and the straight trailing edge.
fn foil_section(frame: &wing::Frame, pts: &[[f64; 2]]) -> Result<kernel::BezierSection> {
    let p: Vec<Vec2> = pts.iter().map(|q| Vec2::new(q[0], q[1])).collect();
    let place = |q: Vec2| v3(frame.origin) + v3(frame.x) * q.x + v3(frame.y) * q.y;
    let curve: Vec<[Vec3; 4]> = fit(&p)?.iter().map(|b| b.map(place)).collect();
    let (Some(a), Some(b)) = (curve.last().map(|b| b[3]), curve.first().map(|b| b[0])) else { return Err(DocError::Invalid("empty section".into())) };
    Ok(vec![curve, vec![line3(a, b)]])
}

/// A wing or stabiliser surface as a solid: the loft through its stations, mirrored and joined.
pub fn surface_body(s: &wing::Surface) -> Result<Body> {
    let secs = s.stations.iter().map(|st| foil_section(&st.frame, &st.points)).collect::<Result<Vec<_>>>()?;
    let half = kernel::loft_beziers(&secs)?;
    if !s.mirror {
        return Ok(half);
    }
    let other = kernel::transform_matrix(&half, linear([[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]]))?;
    union(half, &other)
}

/// The values of the last foil wing before feature `at` (a linked stabiliser follows it), or the
/// last one of all.
fn wing_link(doc: &Document, vals: &BTreeMap<String, Value>, at: Option<u64>) -> Result<Option<wing::WingLink>> {
    let before = doc.features.iter().take_while(|g| Some(g.id) != at);
    let Some(w) =
        before.filter(|g| !g.suppressed).filter_map(|g| if let FeatureKind::FoilWing { values, .. } = &g.kind { Some(values) } else { None }).last()
    else {
        return Ok(None);
    };
    Ok(Some(wing::build(&w.values(vals)?, [0.0; 3], None).map_err(|e| DocError::Invalid(format!("the wing above: {e}")))?.link()))
}

/// What a foil feature builds: its bodies, its notice (the Onshape feature's summary line) and
/// its warnings.
pub struct Built {
    pub bodies: Vec<Body>,
    pub notice: String,
    pub warnings: Vec<String>,
}

/// The laws' result of a foil feature without its solids: the notice and warnings, or the error
/// the feature would show. Quick enough for a dialog to call as values change.
pub fn summary(doc: &Document, vals: &BTreeMap<String, Value>, f: &Feature) -> Result<(String, Vec<String>)> {
    summary_of(doc, vals, &f.kind, Some(f.id))
}

/// [`summary`] of a foil feature's kind not (yet) in the design: `at` is the feature it stands in
/// for (a linked stabiliser follows the last wing before it), `None` for one added at the end.
pub fn summary_of(doc: &Document, vals: &BTreeMap<String, Value>, kind: &FeatureKind, at: Option<u64>) -> Result<(String, Vec<String>)> {
    match kind {
        FeatureKind::FoilWing { values, .. } | FeatureKind::FoilStabiliser { values, .. } => {
            let s = surface(doc, vals, at, values)?;
            Ok((s.notice, Vec::new()))
        }
        FeatureKind::Propeller { values, .. } => {
            let p = prop::build(&values.values(vals)?).map_err(|e| DocError::Invalid(format!("B-Series Propeller: {e}")))?;
            Ok((p.notice, p.warnings))
        }
        _ => Err(DocError::Invalid("not a foil feature".into())),
    }
}

fn surface(doc: &Document, vals: &BTreeMap<String, Value>, at: Option<u64>, values: &FoilParams) -> Result<wing::Surface> {
    let v = values.values(vals)?;
    let title = values.tool.title();
    if values.tool == Tool::Stabiliser {
        let link = if v.choice("stabLink") == "LINKED" { wing_link(doc, vals, at)? } else { None };
        return wing::build(&v, [v.num("arm"), 0.0, v.num("height")], link).map_err(|e| DocError::Invalid(format!("{title}: {e}")));
    }
    wing::build(&v, [0.0; 3], None).map_err(|e| DocError::Invalid(format!("{title}: {e}")))
}

/// What a foil feature builds.
pub fn build(doc: &Document, vals: &BTreeMap<String, Value>, f: &Feature) -> Result<Built> {
    match &f.kind {
        FeatureKind::FoilWing { values, .. } | FeatureKind::FoilStabiliser { values, .. } => {
            let s = surface(doc, vals, Some(f.id), values)?;
            Ok(Built { bodies: vec![surface_body(&s)?], notice: s.notice, warnings: Vec::new() })
        }
        FeatureKind::Propeller { values, .. } => {
            let p = prop::build(&values.values(vals)?).map_err(|e| DocError::Invalid(format!("B-Series Propeller: {e}")))?;
            Ok(Built { bodies: vec![prop_body(&p)?], notice: p.notice.clone(), warnings: p.warnings.clone() })
        }
        _ => Err(DocError::Invalid(format!("{} is not a foil feature", f.name))),
    }
}

/// Points along a fit spline through `pts`, `m` of them, closer together towards the start (the
/// leading edge) on a half-cosine spacing of the spline's length: every blade section gets the
/// same count, whatever its nose took.
fn resample(pts: &[Vec2], m: usize) -> Result<Vec<Vec2>> {
    let segs = fit(pts)?;
    // The spline densely, with its length along it.
    let mut dense = Vec::new();
    for b in &segs {
        for k in 0..16 {
            dense.push(bezier_point(b, k as f64 / 16.0));
        }
    }
    if let Some(b) = segs.last() {
        dense.push(b[3]);
    }
    let mut len = vec![0.0];
    for w in dense.windows(2) {
        len.push(len.last().copied().unwrap_or(0.0) + w[0].dist(w[1]));
    }
    let total = len.last().copied().unwrap_or(0.0);
    let mut out = Vec::with_capacity(m);
    let mut j = 0;
    for i in 0..m {
        let target = total * (1.0 - (std::f64::consts::FRAC_PI_2 * i as f64 / (m - 1) as f64).cos());
        while j + 2 < len.len() && len[j + 1] < target {
            j += 1;
        }
        let (l0, l1) = (len[j], len[j + 1]);
        let f = if l1 > l0 { ((target - l0) / (l1 - l0)).clamp(0.0, 1.0) } else { 0.0 };
        out.push(dense[j] + (dense[j + 1] - dense[j]) * f);
    }
    Ok(out)
}

/// Points along each blade surface per section.
const BLADE_POINTS: usize = 28;

/// One blade: the loft through its sections, each wrapped onto its cylinder (every piece of the
/// spline through four points on it); the root and tip sections are flattened onto a plane, so
/// they cap the blade: the root's through its ends (it lies inside the hub, where Onshape caps it
/// on its cylinder: a lone blade has up to a few per cent more volume here), the tip's tangent to
/// its cylinder (it spans a few degrees).
pub fn blade_body(sections: &[prop::BladeSection]) -> Result<Body> {
    let last = sections.len().saturating_sub(1);
    let mut secs = Vec::with_capacity(sections.len());
    for (i, s) in sections.iter().enumerate() {
        let n0 = Vec3::new(-s.hand * s.theta0.sin(), s.theta0.cos(), 0.0);
        let t0 = Vec3::new(-s.hand * s.theta0.cos(), -s.theta0.sin(), 0.0);
        let flat = i == 0 || i == last;
        // The root's plane through its ends (inside the cylinder: the hub covers it), the tip's
        // the tangent plane (the tip reaches the diameter).
        let r_flat = if i == 0 { s.r * s.back.iter().chain(&s.face).map(|q| (q[0] / s.r).cos()).fold(1.0, f64::min) } else { s.r };
        // A developed point (u round the cylinder, w along the shaft) in 3D.
        let place = |q: Vec2| -> Vec3 {
            let a = q.x / s.r;
            if flat { n0 * r_flat + t0 * (s.r * a.sin()) + Vec3::Z * q.y } else { n0 * (s.r * a.cos()) + t0 * (s.r * a.sin()) + Vec3::Z * q.y }
        };
        let wrap = |b: &[Vec2; 4]| -> [Vec3; 4] {
            if flat { b.map(place) } else { cubic_through([0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0].map(|t| place(bezier_point(b, t)))) }
        };
        let dev = |l: &[[f64; 2]]| -> Vec<Vec2> { l.iter().map(|q| Vec2::new(q[0], q[1])).collect() };
        let points = std::env::var("FOIL_BLADE_POINTS").ok().and_then(|v| v.parse().ok()).unwrap_or(BLADE_POINTS);
        let back = resample(&dev(&s.back), points)?;
        let mut face = resample(&dev(&s.face), points)?;
        face.reverse();
        let back: Vec<[Vec3; 4]> = fit(&back)?.iter().map(wrap).collect();
        let face: Vec<[Vec3; 4]> = fit(&face)?.iter().map(wrap).collect();
        let (Some(te_b), Some(te_f)) = (back.last().map(|b| b[3]), face.first().map(|b| b[0])) else {
            return Err(DocError::Invalid("empty blade section".into()));
        };
        // The trailing edge on the cylinder too: its middle points wrapped.
        let te = if flat {
            line3(te_b, te_f)
        } else {
            let (a, b) = (dev(&s.back).last().copied(), dev(&s.face).last().copied());
            let (Some(a), Some(b)) = (a, b) else { return Err(DocError::Invalid("empty blade section".into())) };
            let mut q = [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0].map(|t| place(a + (b - a) * t));
            q[0] = te_b;
            q[3] = te_f;
            cubic_through(q)
        };
        secs.push(vec![back, vec![te], face]);
    }
    Ok(kernel::loft_beziers(&secs)?)
}

/// A solid of revolution about Z from an (r, z) outline.
fn revolved(outline: &[[f64; 2]]) -> Result<Body> {
    let plane = Plane { origin: Vec3::ZERO, x: Vec3::X, y: Vec3::Z };
    let pts: Vec<Vec2> = outline.iter().map(|q| Vec2::new(q[0], q[1])).collect();
    let region = Region2 { outer: Loop2::polygon(&pts).ccw(), holes: Vec::new() };
    kernel::revolve(&plane, &[region], Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0), std::f64::consts::TAU)?
        .into_iter()
        .next()
        .ok_or_else(|| DocError::Invalid("the revolve made nothing".into()))
}

fn cut_tool(c: &prop::Cut) -> Result<Body> {
    Ok(match c {
        prop::Cut::Revolve(outline) => revolved(outline)?,
        prop::Cut::Cylinder { x, y, z0, z1, r } => kernel::cylinder(Vec3::new(*x, *y, *z0), Vec3::Z, *r, z1 - z0)?,
        prop::Cut::Box { lo, hi } => kernel::box_solid(v3(*lo), v3(*hi))?,
        prop::Cut::Bolt { x, y, z0, z1, r, head, head_top } => {
            // A stepped hole: one revolve about its own axis.
            let outline: Vec<[f64; 2]> = if *head > *r {
                vec![[0.0, *z0], [*head, *z0], [*head, *head_top], [*r, *head_top], [*r, *z1], [0.0, *z1]]
            } else {
                vec![[0.0, *z0], [*r, *z0], [*r, *z1], [0.0, *z1]]
            };
            let b = revolved(&outline)?;
            kernel::transform_matrix(&b, [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [*x, *y, 0.0, 1.0]])?
        }
    })
}

/// The propeller: the hub with its nose, tail and mount cut; one blade trimmed to the hub's ends
/// (and cut by any mount tool reaching its root), copied round and joined; turned aft up. The
/// FeatureScript joins first and cuts after; here the cuts are made while the bodies are simple
/// (the same solid, much faster: every boolean after the blades are on meets their surfaces).
pub fn prop_body(p: &prop::Prop) -> Result<Body> {
    let t = std::time::Instant::now();
    let timing = std::env::var_os("FOIL_TIMING").is_some();
    let lap = |what: &str| {
        if timing {
            eprintln!("propeller: {what} at {:?}", t.elapsed());
        }
    };
    let mut hub = revolved(&p.hub)?;
    hub = cut_all(hub, &p.cuts.iter().map(cut_tool).collect::<Result<Vec<_>>>()?)?;
    lap("hub");
    let mut blade = blade_body(&p.sections)?;
    for c in &p.blade_trims {
        blade = cut(blade, &cut_tool(c)?)?;
    }
    lap("blade trimmed");
    // The blade's root comes no nearer the axis than its flattened root section.
    let root = p.sections.first().map(|s| s.r * s.back.iter().chain(&s.face).map(|q| (q[0] / s.r).cos()).fold(1.0, f64::min)).unwrap_or(0.0);
    // Grown by 0.3 mm, so the blade's cut faces lie inside the hub's material and the holes are
    // the hub's own faces: coincident or nearly coincident faces from the same tool defeat the
    // join or slow it to minutes.
    // The mount's tools that reach the blades' roots, grown: each copy is cut where it sits (the
    // bolt circle need not repeat with the blades).
    let reaching: Vec<Body> = p.cuts.iter().filter(|c| c.reach() > root - 0.3).map(|c| cut_tool(&c.grown(0.3))).collect::<Result<_>>()?;
    let mut body = hub;
    for k in 0..p.blades {
        let a = std::f64::consts::TAU * k as f64 / p.blades as f64;
        let (s, c) = a.sin_cos();
        let copy = if k == 0 { blade.clone() } else { kernel::transform_matrix(&blade, linear([[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]))? };
        let copy = cut_all(copy, &reaching)?;
        lap("blade cut");
        body = union(body, &copy)?;
        lap("blade joined");
    }
    Ok(kernel::transform_matrix(&body, linear([[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]]))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The "Propeller 140 x 3 on the 63 mm hub (the eFoil set)" of the B-Series generator's test
    /// studio, root fillet 0: Onshape gives 112,441.3 mm³, 123.38 x 117.41 x 45.00 mm.
    pub(crate) fn efoil_prop() -> Values {
        let mut v = Values::defaults(Tool::Propeller);
        for (k, x) in [
            ("diameter", 140.0),
            ("pdRatio", 0.9),
            ("ear", 0.3),
            ("hubUpstream", 63.0),
            ("hubDownstream", 63.0),
            ("hubLength", 30.0),
            ("noseLength", 15.0),
            ("pilotDia", 12.0),
            ("tipPitchChange", -10.0),
            ("skewAngle", 7.0),
            ("rake", 5.0),
            ("taperExponent", 1.0),
            ("sectionCount", 12.0),
        ] {
            v.set(k, Val::Num(x)).unwrap();
        }
        v.set("mountType", Val::Choice("BOLTED".into())).unwrap();
        v
    }

    #[test]
    fn efoil_propeller_against_onshape() {
        let p = prop::build(&efoil_prop()).unwrap();
        let t = std::time::Instant::now();
        let b = prop_body(&p).unwrap();
        let m = kernel::measure(&b).unwrap();
        let e = m.bbox.max - m.bbox.min;
        println!("propeller {:?}: volume {} extents {:?}", t.elapsed(), m.volume, e);
        assert!((m.volume - 112_441.3).abs() / 112_441.3 < 1e-3, "{}", m.volume);
        // The bounds come from the body's mesh, which can fall a little short of a blade's round
        // tip (0.02 to 0.08 mm from run to run here).
        assert!((e.x - 123.379).abs() < 0.1 && (e.y - 117.414).abs() < 0.1 && (e.z - 45.0).abs() < 0.01, "{e:?}");
    }

    /// The generator's "Blade test, valid" (the Universal B-Series Blade Engine): Onshape gives
    /// 5,716.6 mm³ and bounds (-13.75, -69.63, -9.82) to (20.21, -12.02, 20.51), aft up.
    #[test]
    fn blade_against_onshape() {
        let mut v = efoil_prop();
        v.set("pdRatio", Val::Num(0.8)).unwrap();
        v.set("rake", Val::Num(15.0)).unwrap();
        let mut w = Vec::new();
        let sections = prop::blade(&v, 52.0, &mut w).unwrap();
        let b = blade_body(&sections).unwrap();
        let b = kernel::transform_matrix(&b, linear([[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]])).unwrap();
        let m = kernel::measure(&b).unwrap();
        println!("blade volume {} bbox {:?}", m.volume, m.bbox);
        // The root cap: Onshape's lies on the cylinder r = 0.2R, ours on the plane through the
        // section's ends, up to 2.4 mm further in (a stub the hub covers on a propeller).
        assert!(m.volume > 5716.6 && (m.volume - 5716.6) / 5716.6 < 0.04, "{}", m.volume);
        for (a, b) in [(m.bbox.min.x, -13.7475), (m.bbox.min.y, -69.6295), (m.bbox.min.z, -9.8200), (m.bbox.max.x, 20.2106), (m.bbox.max.z, 20.5108)]
        {
            assert!((a - b).abs() < 0.1, "{a} vs {b}: {:?}", m.bbox);
        }
        assert!(m.bbox.max.y > -12.02 && m.bbox.max.y < -11.0, "the root: {:?}", m.bbox);
    }
}
