//! A plugin feature's dialog, drawn from the plugin's parameter schema (docs/plugins.md): its
//! groups in order (collapsible), each field shown under its condition, its help as the tooltip,
//! numbers as expressions (a parameter may drive them), and the plugin's notice worked out from
//! the values as they are typed. A new feature picks its plugin from the installed ones.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

use serde_json::{Value, json};
use solidcraft_engine::doc::FeatureKind;
use solidcraft_engine::doc::plugin_feature::{PluginParams, number_text};
use solidcraft_plugin::{FieldKind, Schema, Unit};

use crate::SolveApp;
use crate::dialogs::{OP_LABELS, OPS};
use crate::ui_kit;

#[derive(Clone, Debug)]
pub struct PluginDialog {
    pub plugin_id: String,
    /// Picking the plugin (a new feature) or not (editing one).
    pub pick: bool,
    pub schema: Schema,
    /// Number parameters' expressions, flags and choice indices, by key.
    pub exprs: BTreeMap<String, String>,
    pub flags: BTreeMap<String, bool>,
    pub choices: BTreeMap<String, usize>,
    pub operation: usize,
    /// The notice for the values it was worked out from (their hash).
    notice: Option<(u64, Result<String, String>)>,
}

/// The installed plugins' ids and names.
fn installed() -> Vec<(String, String)> {
    solidcraft_plugin::installed().iter().map(|p| p.manifest()).map(|m| (m.id, m.name)).collect()
}

impl PluginDialog {
    /// A new feature of the first installed plugin (or `id`).
    pub fn new(id: Option<&str>) -> Option<PluginDialog> {
        let id = id.map(str::to_string).or_else(|| installed().first().map(|(id, _)| id.clone()))?;
        let plugin = solidcraft_plugin::find(&id)?;
        let schema = plugin.schema();
        let mut d = PluginDialog::from_params(&id, &PluginParams::defaults(&schema), schema, 0);
        d.pick = true;
        Some(d)
    }

    /// The dialog of a feature's stored parameters.
    pub fn from_params(plugin_id: &str, params: &PluginParams, schema: Schema, operation: usize) -> PluginDialog {
        let mut params = params.clone();
        params.conform(&schema);
        let mut d = PluginDialog {
            plugin_id: plugin_id.to_string(),
            pick: false,
            schema,
            exprs: BTreeMap::new(),
            flags: BTreeMap::new(),
            choices: BTreeMap::new(),
            operation,
            notice: None,
        };
        let values = params.to_json();
        for f in &d.schema.fields {
            let v = values.get(&f.key).cloned().unwrap_or_else(|| f.kind.default_value());
            match &f.kind {
                FieldKind::Number { .. } => {
                    let e = match &v {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.as_f64().map(number_text).unwrap_or_default(),
                        _ => String::new(),
                    };
                    d.exprs.insert(f.key.clone(), e);
                }
                FieldKind::Bool { default } => {
                    d.flags.insert(f.key.clone(), v.as_bool().unwrap_or(*default));
                }
                FieldKind::Choice { options, default } => {
                    let id = v.as_str().unwrap_or(default);
                    d.choices.insert(f.key.clone(), options.iter().position(|(o, _)| o == id).unwrap_or(0));
                }
            }
        }
        d
    }

    /// The dialog's title: the plugin's name.
    pub fn title(&self) -> String {
        solidcraft_plugin::find(&self.plugin_id).map(|p| p.manifest().name).unwrap_or_else(|| self.plugin_id.clone()).to_uppercase()
    }

    /// Whether the dialog may rebuild the feature as values change.
    pub fn previews(&self) -> bool {
        solidcraft_plugin::find(&self.plugin_id).is_some_and(|p| p.manifest().preview)
    }

    fn choice_id(&self, key: &str) -> Option<String> {
        let f = self.schema.field(key)?;
        let FieldKind::Choice { options, .. } = &f.kind else { return None };
        options.get(self.choices.get(key).copied().unwrap_or(0)).map(|(id, _)| id.clone())
    }

    /// The parameters as the commands take them.
    pub fn values(&self) -> serde_json::Map<String, Value> {
        let mut m = serde_json::Map::new();
        for f in &self.schema.fields {
            let v = match &f.kind {
                FieldKind::Bool { .. } => json!(self.flags.get(&f.key).copied().unwrap_or(false)),
                FieldKind::Choice { .. } => json!(self.choice_id(&f.key).unwrap_or_default()),
                FieldKind::Number { .. } => json!(self.exprs.get(&f.key).map(|e| e.trim()).unwrap_or("")),
            };
            m.insert(f.key.clone(), v);
        }
        m
    }

    /// `plugin.add`'s parameters.
    pub fn params(&self) -> Value {
        json!({"plugin": self.plugin_id, "values": self.values(), "operation": OPS.get(self.operation).copied().unwrap_or("new")})
    }

    /// The notice the feature would show with these values, worked out again when they change.
    fn notice(&mut self, app: &SolveApp, editing: Option<u64>) -> Result<String, String> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        Value::Object(self.values()).to_string().hash(&mut h);
        self.plugin_id.hash(&mut h);
        app.session.doc.params.len().hash(&mut h);
        let key = h.finish();
        if let Some((k, n)) = &self.notice
            && *k == key
        {
            return n.clone();
        }
        let s = &app.session;
        let mut params = PluginParams::defaults(&self.schema);
        let n = (|| {
            for (k, v) in self.values() {
                params.set(&self.schema, &k, &v)?;
            }
            let (vals, _) = s.doc.param_values();
            // The context as the feature would see it: what plugin features before it published.
            let published = match editing {
                Some(id) => s.model.state_before(id).plugin_vars.clone(),
                None => s.model.state().plugin_vars.clone(),
            };
            let name = editing.and_then(|id| s.doc.feature(id)).map(|f| f.name.clone()).unwrap_or_default();
            let published: serde_json::Map<String, Value> =
                published.iter().map(|(k, v)| (k.clone(), serde_json::from_str(v).unwrap_or(Value::Null))).collect();
            let ctx = json!({"feature": name, "published": published});
            let d = solidcraft_engine::doc::plugin_feature::describe(&vals, &self.plugin_id, &params, &ctx)?;
            let mut n = d.get("notice").and_then(Value::as_str).unwrap_or_default().to_string();
            for w in d.get("warnings").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                n += &format!("\n⚠ {w}");
            }
            Ok(n)
        })();
        self.notice = Some((key, n.clone()));
        n
    }
}

/// The dialog's rows: the plugin (a new feature), the notice, then each group with the fields
/// the plugin shows in it, then the operation. Each field's label carries its help as the
/// tooltip; fields the plugin does not use yet are dimmed. Returns whether Enter was pressed.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, d: &mut PluginDialog, editing: Option<u64>) -> bool {
    let mut enter = false;
    if d.pick {
        let list = installed();
        let mut i = list.iter().position(|(id, _)| *id == d.plugin_id).unwrap_or(0);
        let before = i;
        let labels: Vec<&str> = list.iter().map(|(_, name)| name.as_str()).collect();
        ui_kit::choice_row(ui, "Plugin", "plugin_pick", &mut i, &labels);
        if i != before
            && let Some(nd) = list.get(i).and_then(|(id, _)| PluginDialog::new(Some(id)))
        {
            *d = PluginDialog { operation: d.operation, ..nd };
        }
    }
    match d.notice(app, editing) {
        Ok(n) if !n.is_empty() => {
            ui_kit::text(ui, &n, ui_kit::Text::Note("plugin_notice"));
        }
        Ok(_) => {}
        Err(e) => {
            ui_kit::text(ui, &e, ui_kit::Text::Error("plugin_notice"));
        }
    }
    let values: serde_json::Map<String, Value> = d.values();
    let schema = d.schema.clone();
    for (group, collapsed) in schema.groups() {
        let fields: Vec<&solidcraft_plugin::Field> = schema.fields.iter().filter(|f| f.group == group && schema.shows(f, &values)).collect();
        if fields.is_empty() {
            continue;
        }
        let mut body = |ui: &mut egui::Ui| {
            for f in fields.iter().copied() {
                let unit = match &f.kind {
                    FieldKind::Number { unit: Unit::Length, .. } => "mm",
                    FieldKind::Number { unit: Unit::Angle, .. } => "deg",
                    _ => "",
                };
                let label = if unit.is_empty() || f.label.contains(unit) { f.label.clone() } else { format!("{} ({unit})", f.label) };
                let help = if f.unused { format!("{}\n\nNot used by the plugin yet: the value is kept.", f.help) } else { f.help.clone() };
                ui.scope(|ui| {
                    if f.unused {
                        ui.multiply_opacity(0.55);
                    }
                    match &f.kind {
                        FieldKind::Bool { .. } => {
                            if let Some(b) = d.flags.get_mut(&f.key) {
                                ui_kit::help_row(ui, &label, help.trim(), |ui| ui_kit::checkbox(ui, "", b));
                            }
                        }
                        FieldKind::Choice { options, .. } => {
                            let labels: Vec<&str> = options.iter().map(|(_, l)| l.as_str()).collect();
                            if let Some(i) = d.choices.get_mut(&f.key) {
                                ui_kit::help_row(ui, &label, help.trim(), |ui| ui_kit::dropdown(ui, &format!("plugin_{}", f.key), i, &labels));
                            }
                        }
                        FieldKind::Number { .. } => {
                            if let Some(e) = d.exprs.get_mut(&f.key) {
                                enter |= ui_kit::help_row(ui, &label, help.trim(), |ui| ui_kit::field(ui, e));
                            }
                        }
                    }
                });
            }
        };
        if group.is_empty() {
            body(ui);
        } else {
            ui_kit::section(ui, ("plugin_group", d.plugin_id.clone(), group.clone()), &group, !collapsed, body);
        }
    }
    ui_kit::choice_row(ui, "Operation", "plugin_operation", &mut d.operation, &OP_LABELS);
    enter
}

/// The dialog of a plugin feature, or `None` when its plugin is not installed.
pub fn for_feature(kind: &FeatureKind, operation: usize) -> Option<PluginDialog> {
    let FeatureKind::Plugin { plugin_id, params, .. } = kind else { return None };
    let plugin = solidcraft_plugin::find(plugin_id)?;
    Some(PluginDialog::from_params(plugin_id, params, plugin.schema(), operation))
}
