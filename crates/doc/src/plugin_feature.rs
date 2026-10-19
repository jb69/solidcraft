//! The `Plugin` feature (docs/plugins.md): one timeline row whose geometry a plugin builds from
//! the values stored here, on every rebuild.
//!
//! The values are the plugin's parameters as its schema names them. Numbers are expressions and
//! feature inputs (each gets a `dN` name, so a parameter can drive it); flags and choices are
//! stored as they are. `inputs` keeps each number's label and unit with the feature, so the
//! design walks its expressions, and opens, without the plugin. The plugin runs through a
//! [`PluginRunner`] the engine registers: it evaluates the plugin on a scratch design and hands
//! back the bodies left there. What a plugin publishes is passed to the plugin features after it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value as Json;
use solidcraft_kernel::Body;
use solidcraft_plugin::{FallbackPolicy, FieldKind, Plugin, Schema, Unit};

use crate::expr::{Kind, Value};
use crate::walk::{ExprItem, ExprItemMut, ExprRole, Walk};
use crate::{DocError, Expr, Feature, FeatureKind, MeshData, Result};

/// A number parameter: its key, label and unit, kept with the feature.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginInput {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub unit: Unit,
}

/// A plugin feature's parameters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginParams {
    /// The number parameters, in the plugin's order.
    pub inputs: Vec<PluginInput>,
    /// Their expressions, by key.
    pub exprs: BTreeMap<String, Expr>,
    /// The other parameters (flags, choice ids, anything a plugin keeps), by key.
    pub values: BTreeMap<String, Json>,
}

/// The expression kind of a unit.
pub fn kind_of(u: Unit) -> Kind {
    match u {
        Unit::Length => Kind::Length,
        Unit::Angle => Kind::Angle,
        Unit::None => Kind::Unitless,
    }
}

/// A number as an expression: as short as it reads.
pub fn number_text(x: f64) -> String {
    let s = format!("{x}");
    if s.contains('e') { format!("{x:.9}") } else { s }
}

impl PluginParams {
    /// Every parameter of `schema` at its default.
    pub fn defaults(schema: &Schema) -> PluginParams {
        let mut p = PluginParams::default();
        p.conform(schema);
        p
    }

    /// Bring the stored parameters to `schema`: inputs in its order with its labels, a default for
    /// every parameter missing. Values the schema no longer has are kept (an older plugin may read
    /// them).
    pub fn conform(&mut self, schema: &Schema) {
        self.inputs = schema
            .fields
            .iter()
            .filter_map(|f| match f.kind {
                FieldKind::Number { unit, .. } => Some(PluginInput { key: f.key.clone(), label: f.label.clone(), unit }),
                _ => None,
            })
            .collect();
        for f in &schema.fields {
            match &f.kind {
                FieldKind::Number { default, .. } => {
                    self.exprs.entry(f.key.clone()).or_insert_with(|| Expr::new(number_text(*default)));
                }
                k => {
                    self.values.entry(f.key.clone()).or_insert_with(|| k.default_value());
                }
            }
        }
    }

    /// Set one parameter from JSON, checked against the schema: a number or an expression for a
    /// number, a bool for a flag, an option id or label for a choice.
    pub fn set(&mut self, schema: &Schema, key: &str, v: &Json) -> std::result::Result<(), String> {
        let f = schema.field(key).ok_or_else(|| format!("no parameter `{key}`"))?;
        match (&f.kind, v) {
            (FieldKind::Number { .. }, Json::String(s)) => {
                if s.trim().is_empty() {
                    return Err(format!("{}: empty expression", f.label));
                }
                self.exprs.insert(f.key.clone(), Expr::new(s.trim()));
            }
            (FieldKind::Number { .. }, Json::Number(_)) => {
                let x = solidcraft_plugin::check(f, v)?;
                self.exprs.insert(f.key.clone(), Expr::new(number_text(x.as_f64().unwrap_or(0.0))));
            }
            _ => {
                let x = solidcraft_plugin::check(f, v)?;
                self.values.insert(f.key.clone(), x);
            }
        }
        Ok(())
    }

    /// The parameters as JSON, numbers as their expressions (what the dialog edits).
    pub fn to_json(&self) -> serde_json::Map<String, Json> {
        let mut m: serde_json::Map<String, Json> = self.values.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (k, e) in &self.exprs {
            m.insert(k.clone(), Json::String(e.as_str().to_string()));
        }
        m
    }

    /// The parameters as the plugin takes them: every expression evaluated (lengths in mm,
    /// angles in degrees) and, when the plugin is here, checked against its schema.
    pub fn evaluated(&self, vals: &BTreeMap<String, Value>, schema: Option<&Schema>, what: &str) -> Result<Json> {
        let mut m: serde_json::Map<String, Json> = self.values.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for input in &self.inputs {
            let e = self.exprs.get(&input.key).map(Expr::as_str).unwrap_or("0");
            let k = kind_of(input.unit);
            let x = crate::eval::val(vals, e, k).map_err(|err| DocError::Invalid(format!("{what}: {}: {err}", input.label)))?;
            let x = if k == Kind::Angle { x.to_degrees() } else { x };
            m.insert(input.key.clone(), Json::from(x));
        }
        if let Some(schema) = schema {
            for f in &schema.fields {
                let v = m.get(&f.key).cloned().unwrap_or_else(|| f.kind.default_value());
                let v = solidcraft_plugin::check(f, &v).map_err(|e| DocError::Invalid(format!("{what}: {e}")))?;
                m.insert(f.key.clone(), v);
            }
        }
        Ok(Json::Object(m))
    }
}

#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    inputs: Vec<PluginInput>,
    #[serde(default)]
    values: serde_json::Map<String, Json>,
}

impl Serialize for PluginParams {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        Stored { inputs: self.inputs.clone(), values: self.to_json() }.serialize(s)
    }
}

impl<'de> Deserialize<'de> for PluginParams {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<PluginParams, D::Error> {
        let st = Stored::deserialize(d)?;
        let numbers: BTreeSet<&str> = st.inputs.iter().map(|i| i.key.as_str()).collect();
        let mut p = PluginParams { inputs: st.inputs.clone(), ..Default::default() };
        for (k, v) in st.values {
            match v {
                Json::String(s) if numbers.contains(k.as_str()) => {
                    p.exprs.insert(k, Expr::new(s));
                }
                Json::Number(n) if numbers.contains(k.as_str()) => {
                    p.exprs.insert(k, Expr::new(n.as_f64().map(number_text).unwrap_or_else(|| n.to_string())));
                }
                v => {
                    p.values.insert(k, v);
                }
            }
        }
        // A number input without a value is 0 until the plugin's default fills it (conform).
        for i in &st.inputs {
            p.exprs.entry(i.key.clone()).or_insert_with(|| Expr::new("0"));
        }
        Ok(p)
    }
}

/// Labels live as long as the program: an input's label is handed out as `&'static str`, and
/// there are only as many as there are distinct labels.
fn intern(label: &str) -> &'static str {
    static LABELS: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let m = LABELS.get_or_init(Default::default);
    let Ok(mut m) = m.lock() else { return "Plugin parameter" };
    if let Some(l) = m.get(label) {
        return l;
    }
    let l: &'static str = Box::leak(label.to_string().into_boxed_str());
    m.insert(label.to_string(), l);
    l
}

/// The number parameters are inputs, in the plugin's order, whether the dialog shows them or not,
/// so their `dN` names stay put when an option changes.
impl Walk for PluginParams {
    fn walk_exprs<'a>(&'a self, f: &mut dyn FnMut(ExprItem<'a>)) {
        for i in &self.inputs {
            if let Some(e) = self.exprs.get(&i.key) {
                f((intern(&i.label), e, kind_of(i.unit), ExprRole::Input));
            }
        }
    }
    fn walk_exprs_mut<'a>(&'a mut self, f: &mut dyn FnMut(ExprItemMut<'a>)) {
        let inputs = self.inputs.clone();
        let mut refs: HashMap<&str, &'a mut Expr> = self.exprs.iter_mut().map(|(k, e)| (k.as_str(), e)).collect();
        for i in &inputs {
            if let Some(e) = refs.remove(i.key.as_str()) {
                f((intern(&i.label), e, kind_of(i.unit), ExprRole::Input));
            }
        }
    }
}

/// The plugin's version and contract a feature was made with.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginProvenance {
    #[serde(default)]
    pub version: String,
}

impl Walk for PluginProvenance {}
impl Walk for solidcraft_plugin::Display {}
impl Walk for FallbackPolicy {}

/// Evaluates a plugin on a scratch design: the engine registers one ([`set_runner`]).
pub trait PluginRunner: Send + Sync {
    /// Run `evaluate` and return the bodies it left and its JSON result.
    fn evaluate(&self, plugin: &Arc<dyn Plugin>, params: &Json, ctx: &Json) -> std::result::Result<(Vec<Body>, Json), String>;
}

static RUNNER: OnceLock<Arc<dyn PluginRunner>> = OnceLock::new();

/// Set the runner plugin features evaluate through (once; later calls are ignored).
pub fn set_runner(r: Arc<dyn PluginRunner>) {
    let _ = RUNNER.set(r);
}

/// What a plugin feature built.
pub struct PluginBuilt {
    pub bodies: Vec<Body>,
    /// The plugin's result (`notice`, `warnings`, `publish`).
    pub result: Json,
    pub warnings: Vec<String>,
}

/// The context a plugin feature evaluates in: the feature's name and what earlier plugin
/// features published.
pub fn context(f: &Feature, published: &BTreeMap<String, String>) -> Json {
    let published: serde_json::Map<String, Json> =
        published.iter().map(|(k, v)| (k.clone(), serde_json::from_str(v).unwrap_or(Json::Null))).collect();
    serde_json::json!({"feature": f.name, "published": published})
}

fn recorded_bodies(recorded: &[MeshData]) -> Result<Vec<Body>> {
    recorded
        .iter()
        .map(|m| {
            let pos: Vec<solidcraft_geom::Vec3> =
                m.positions.as_chunks::<3>().0.iter().map(|&[x, y, z]| solidcraft_geom::Vec3::new(x, y, z)).collect();
            let tri: Vec<[u32; 3]> = m.triangles.as_chunks::<3>().0.to_vec();
            Ok(solidcraft_kernel::mesh_body(&pos, &tri)?)
        })
        .collect()
}

/// Build a plugin feature: through its plugin, or by its fallback when the plugin is missing.
pub fn build(doc: &crate::Document, vals: &BTreeMap<String, Value>, f: &Feature, published: &BTreeMap<String, String>) -> Result<PluginBuilt> {
    let FeatureKind::Plugin { plugin_id, params, display, fallback, .. } = &f.kind else {
        return Err(DocError::Invalid(format!("{} is not a plugin feature", f.name)));
    };
    let what = if display.label.is_empty() { plugin_id.as_str() } else { display.label.as_str() };
    let runner = RUNNER.get();
    let plugin = solidcraft_plugin::find(plugin_id);
    let (Some(plugin), Some(runner)) = (plugin, runner) else {
        let missing = format!("the plugin `{plugin_id}` is not installed");
        let recorded = doc.plugin_results.get(&f.id).map(Vec::as_slice).unwrap_or_default();
        return match fallback {
            FallbackPolicy::RecordedMesh if !recorded.is_empty() => Ok(PluginBuilt {
                bodies: recorded_bodies(recorded)?,
                result: Json::Null,
                warnings: vec![format!("{missing}: its last result is shown as a mesh; changing it needs the plugin")],
            }),
            FallbackPolicy::RecordedMesh => Err(DocError::Invalid(format!("{missing}, and the design holds no recorded result"))),
            FallbackPolicy::SuppressWithWarning => {
                Ok(PluginBuilt { bodies: Vec::new(), result: Json::Null, warnings: vec![format!("{missing}: the feature is left out")] })
            }
            FallbackPolicy::ErrorOnRebuild => Err(DocError::Invalid(missing)),
        };
    };
    let schema = plugin.schema();
    let p = params.evaluated(vals, Some(&schema), what)?;
    let ctx = context(f, published);
    let (bodies, result) = runner.evaluate(&plugin, &p, &ctx).map_err(|e| DocError::Invalid(format!("{what}: {e}")))?;
    let warnings = result
        .get("warnings")
        .and_then(Json::as_array)
        .map(|w| w.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    Ok(PluginBuilt { bodies, result, warnings })
}

/// What a plugin feature's result depends on beyond the feature itself: the installed plugin
/// and its version (nothing when it is missing), for the evaluation's fingerprint.
pub fn installed_version(f: &Feature) -> Option<String> {
    let FeatureKind::Plugin { plugin_id, .. } = &f.kind else { return None };
    Some(solidcraft_plugin::find(plugin_id).map(|p| p.manifest().version).unwrap_or_else(|| "missing".into()))
}

/// What a plugin feature would say (its `describe`), for a dialog: the notice and warnings, or
/// the error the feature would show.
pub fn describe(vals: &BTreeMap<String, Value>, plugin_id: &str, params: &PluginParams, ctx: &Json) -> std::result::Result<Json, String> {
    let plugin = solidcraft_plugin::find(plugin_id).ok_or_else(|| format!("the plugin `{plugin_id}` is not installed"))?;
    let schema = plugin.schema();
    let p = params.evaluated(vals, Some(&schema), &plugin.manifest().name).map_err(|e| e.to_string())?;
    plugin.describe(&p, ctx)
}
