//! The Foil wing and Foil stabiliser as SolidCraft plugins: their dialogs from the
//! FeatureScript's (every field, its group, bounds, default, condition and help), and their
//! geometry built through the host's commands only (`solid.loft_points`, `solid.mirror`).

use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_plugin::{Display, FallbackPolicy, Field, FieldKind, Host, Issue, Manifest, Plugin, Schema, Severity, Unit, run};

use crate::schema::{self, Kind, Tool, Val, Values};
use crate::wing::{self, Surface, WingLink};

pub const WING: &str = "foil-wing";
pub const STABILISER: &str = "foil-stabiliser";

/// Register the two plugins (once; an app calls it at start-up).
pub fn register() {
    for p in [Arc::new(FoilPlugin(Tool::Wing)) as Arc<dyn Plugin>, Arc::new(FoilPlugin(Tool::Stabiliser))] {
        let _ = solidcraft_plugin::register(p);
    }
}

/// One of the tools as a plugin.
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

/// What a wing or stabiliser measures, keys naming their units.
fn values(s: &Surface) -> Value {
    json!({"area_cm2": s.area_proj / 100.0, "area_developed_cm2": s.area_dev / 100.0, "span_mm": s.span, "aspect_ratio": s.aspect,
        "root_chord_mm": s.root_chord, "tip_chord_mm": s.tip_chord, "mac_mm": s.mac, "incidence_deg": s.incidence,
        "root_thickness_pct": s.root_thickness, "tip_thickness_pct": s.tip_thickness, "stations": s.stations.len()})
}

/// The surface's issues, and a stabiliser's link that found no wing above it.
fn issues(tool: Tool, p: &Value, ctx: &Value, s: &Surface) -> Result<Vec<Issue>, String> {
    let mut out = s.issues.clone();
    if tool == Tool::Stabiliser && values_of(tool, p)?.choice("stabLink") == "LINKED" && link_of(ctx).is_none() {
        out.push(
            Issue::new(Severity::Warning, "stab_link_without_wing", "linked, but no Foil wing above it: its own values are used")
                .on("stabLink", "LINKED")
                .suggest("stabLink", "INDEPENDENT"),
        );
    }
    Ok(out)
}

fn publish(s: &Surface) -> Value {
    json!({"area": s.area_proj, "aspect": s.aspect, "root_thickness": s.root_thickness, "tip_thickness": s.tip_thickness, "incidence": s.incidence,
        "span": s.span, "root_chord": s.root_chord, "mac": s.mac})
}

impl Plugin for FoilPlugin {
    fn manifest(&self) -> Manifest {
        let (id, name, icon, about, preview) = match self.0 {
            Tool::Wing => {
                (WING, "Foil Wing", "loft", "A hydrofoil front wing from its planform, section and tip (the Foil design tool's Foil wing).", true)
            }
            Tool::Stabiliser => {
                (STABILISER, "Foil Stabiliser", "loft", "A hydrofoil stabiliser on its arm, independent or following the wing above it.", true)
            }
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
                let mut out = json!({"notice": s.notice, "values": values(&s), "issues": issues(self.0, p, ctx, &s)?});
                if self.0 == Tool::Wing {
                    out["publish"] = publish(&s);
                }
                Ok(out)
            }
        }
    }

    fn describe(&self, p: &Value, ctx: &Value) -> Result<Value, String> {
        match self.0 {
            Tool::Wing | Tool::Stabiliser => {
                let s = surface(self.0, p, ctx)?;
                Ok(json!({"notice": s.notice, "values": values(&s), "issues": issues(self.0, p, ctx, &s)?}))
            }
        }
    }
}
