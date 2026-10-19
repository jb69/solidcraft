//! The Foil wing, Foil stabiliser and B-Series Propeller dialogs: every field of the Onshape
//! feature in its group and order, shown under the conditions the FeatureScript shows it, its
//! description as the tooltip, and the feature's notice (area, span, chords…) worked out live
//! from the values as they are typed.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

use egui::{RichText, vec2};
use serde_json::{Value, json};
use solidcraft_engine::doc::FeatureKind;
use solidcraft_engine::doc::foil::{FieldKind, FoilParams, Tool, Val, schema};

use crate::SolveApp;
use crate::dialogs::{OP_LABELS, OPS, check, combo, field, row_label};

#[derive(Clone, Debug)]
pub struct FoilDialog {
    pub tool: Tool,
    /// Number fields' expressions, flags and enum option indices, by FeatureScript key.
    pub exprs: BTreeMap<&'static str, String>,
    pub flags: BTreeMap<&'static str, bool>,
    pub choices: BTreeMap<&'static str, usize>,
    pub operation: usize,
    /// The notice for the values it was worked out from (their hash).
    notice: Option<(u64, Result<String, String>)>,
}

/// The command that makes the feature.
pub fn command(tool: Tool) -> &'static str {
    match tool {
        Tool::Wing => "foil.wing_feature",
        Tool::Stabiliser => "foil.stabiliser_feature",
        Tool::Propeller => "foil.propeller_feature",
    }
}

pub fn tool_of_command(cmd: &str) -> Option<Tool> {
    match cmd {
        "foil.wing_feature" => Some(Tool::Wing),
        "foil.stabiliser_feature" => Some(Tool::Stabiliser),
        "foil.propeller_feature" => Some(Tool::Propeller),
        _ => None,
    }
}

pub fn title(tool: Tool) -> &'static str {
    match tool {
        Tool::Wing => "FOIL WING",
        Tool::Stabiliser => "FOIL STABILISER",
        Tool::Propeller => "B-SERIES PROPELLER",
    }
}

impl FoilDialog {
    pub fn new(tool: Tool) -> FoilDialog {
        FoilDialog::from_params(&FoilParams::defaults(tool), 0)
    }

    /// The dialog of a stored feature's values.
    pub fn from_params(p: &FoilParams, operation: usize) -> FoilDialog {
        let mut d = FoilDialog { tool: p.tool, exprs: BTreeMap::new(), flags: BTreeMap::new(), choices: BTreeMap::new(), operation, notice: None };
        for f in p.tool.fields() {
            match f.kind {
                FieldKind::Bool { default } => {
                    d.flags.insert(f.key, p.flags.get(f.key).copied().unwrap_or(default));
                }
                FieldKind::Enum { options, default } => {
                    let id = p.choices.get(f.key).map(String::as_str).unwrap_or(default);
                    d.choices.insert(f.key, options.iter().position(|(o, _)| *o == id).unwrap_or(0));
                }
                _ => {
                    d.exprs.insert(f.key, p.exprs.get(f.key).map(|e| e.as_str().to_string()).unwrap_or_default());
                }
            }
        }
        d
    }

    fn choice_id(&self, key: &str) -> Option<&'static str> {
        let f = self.tool.field(key)?;
        let FieldKind::Enum { options, .. } = f.kind else { return None };
        options.get(self.choices.get(key).copied().unwrap_or(0)).map(|(id, _)| *id)
    }

    /// Whether Onshape would show field `f` now.
    fn shows(&self, f: &schema::Field) -> bool {
        schema::visible(f.when, &|k| {
            if let Some(b) = self.flags.get(k) {
                return Some(Val::Flag(*b));
            }
            self.choice_id(k).map(|c| Val::Choice(c.to_string()))
        })
    }

    /// The values as the command takes them.
    pub fn values(&self) -> Value {
        let mut m = serde_json::Map::new();
        for f in self.tool.fields() {
            let v = match f.kind {
                FieldKind::Bool { .. } => json!(self.flags.get(f.key).copied().unwrap_or(false)),
                FieldKind::Enum { .. } => json!(self.choice_id(f.key).unwrap_or("")),
                _ => json!(self.exprs.get(f.key).map(|e| e.trim()).unwrap_or("")),
            };
            m.insert(f.key.to_string(), v);
        }
        Value::Object(m)
    }

    /// The command's parameters.
    pub fn params(&self) -> Value {
        json!({"values": self.values(), "operation": OPS.get(self.operation).copied().unwrap_or("new")})
    }

    fn params_kind(&self) -> Result<FeatureKind, String> {
        let mut p = FoilParams::defaults(self.tool);
        if let Value::Object(m) = self.values() {
            for (k, v) in &m {
                p.set_json(k, v)?;
            }
        }
        let (operation, targets) = (Default::default(), Vec::new());
        Ok(match self.tool {
            Tool::Wing => FeatureKind::FoilWing { values: p, operation, targets },
            Tool::Stabiliser => FeatureKind::FoilStabiliser { values: p, operation, targets },
            Tool::Propeller => FeatureKind::Propeller { values: p, operation, targets },
        })
    }

    /// The notice the feature would show with these values, worked out again when they change.
    fn notice(&mut self, app: &SolveApp, editing: Option<u64>) -> Result<String, String> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.values().to_string().hash(&mut h);
        app.session.doc.params.len().hash(&mut h);
        let key = h.finish();
        if let Some((k, n)) = &self.notice
            && *k == key
        {
            return n.clone();
        }
        let s = &app.session;
        let n = self.params_kind().and_then(|kind| {
            let (vals, _) = s.doc.param_values();
            solidcraft_engine::doc::foil::summary_of(&s.doc, &vals, &kind, editing)
                .map(|(n, w)| if w.is_empty() { n } else { format!("{n}\n⚠ {}", w.join("\n⚠ ")) })
                .map_err(|e| e.to_string())
        });
        self.notice = Some((key, n.clone()));
        n
    }
}

/// The dialog's rows: the notice, then each group with the fields Onshape shows in it.
/// Returns whether Enter was pressed in a field.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, d: &mut FoilDialog, editing: Option<u64>) -> bool {
    let t = crate::theme::Tokens::get();
    let mut enter = false;
    match d.notice(app, editing) {
        Ok(n) => {
            ui.add(egui::Label::new(RichText::new(n).small().color(t.text_dim)).wrap());
        }
        Err(e) => {
            ui.add(egui::Label::new(RichText::new(e).small().color(t.error)).wrap());
        }
    }
    ui.add_space(4.0);
    let tool = d.tool;
    for (group, collapsed) in tool.groups() {
        let fields: Vec<&'static schema::Field> = tool.fields().iter().filter(|f| f.group == group && d.shows(f)).collect();
        if fields.is_empty() {
            continue;
        }
        let mut body = |ui: &mut egui::Ui| {
            egui::Grid::new(("foil_grid", tool.title(), group)).num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
                for f in fields.iter().copied() {
                    let unbuilt = schema::not_built(tool, f);
                    let help = schema::plain_help(f.help);
                    let label = if f.kind.unit().is_empty() || f.label.contains(f.kind.unit()) {
                        f.label.to_string()
                    } else {
                        format!("{} ({})", f.label, f.kind.unit())
                    };
                    let r = ui
                        .scope(|ui| {
                            if unbuilt {
                                ui.visuals_mut().override_text_color = Some(t.text_dim);
                            }
                            row_label(ui, &label);
                        })
                        .response;
                    let tip = if unbuilt { format!("{help}\n\nNot built in SolidCraft yet: the value is kept for when it is.") } else { help };
                    if !tip.trim().is_empty() {
                        r.on_hover_text(tip.clone());
                    }
                    match f.kind {
                        FieldKind::Bool { .. } => {
                            if let Some(b) = d.flags.get_mut(f.key) {
                                check(ui, b);
                            }
                        }
                        FieldKind::Enum { options, .. } => {
                            let labels: Vec<&str> = options.iter().map(|(_, l)| *l).collect();
                            if let Some(i) = d.choices.get_mut(f.key) {
                                combo(ui, &format!("foil_{}", f.key), &labels, i);
                            }
                        }
                        _ => {
                            if let Some(e) = d.exprs.get_mut(f.key) {
                                enter |= field(ui, e);
                            }
                        }
                    }
                    ui.end_row();
                }
            });
        };
        if group.is_empty() {
            body(ui);
        } else {
            egui::CollapsingHeader::new(RichText::new(group).strong())
                .id_salt(("foil_group", tool.title(), group))
                .default_open(!collapsed)
                .show(ui, body);
        }
    }
    ui.add_space(4.0);
    egui::Grid::new(("foil_op", tool.title())).num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
        row_label(ui, "Operation");
        combo(ui, "foil_operation", &OP_LABELS, &mut d.operation);
        ui.end_row();
    });
    enter
}
