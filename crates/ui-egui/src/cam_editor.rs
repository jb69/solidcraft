//! The operation editor. Double-clicking an operation in the Browser, or its Edit menu, opens it
//! in a window with two tabs over one draft of the operation:
//! - **Fields** lays the parameters out as inputs: name, type, tool, the kind's own parameters,
//!   stock to leave, geometry, heights and linking. The form is built from the operation's own
//!   saved form and its kind's defaults, so every kind, present and future, gets one.
//! - **JSON** shows the same draft as the text `cam.operation.set` takes, for copying, pasting
//!   and the parameters the form only summarises (picked points, loops).
//!
//! Switching tabs carries the edits across. Apply and OK run `cam.operation.set` with the draft,
//! the same command an assistant runs, as one undo step. When the operation changes elsewhere
//! (an assistant, undo) an untouched draft follows it; an edited one offers to reload.

use std::collections::{BTreeSet, HashMap};

use egui::RichText;
use serde_json::{Map, Value, json};
use solidcraft_engine::cam;

use crate::SolveApp;
use crate::theme::Tokens;

/// The editor's two tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Fields,
    Json,
}

/// Keys every operation has, beside its kind's parameters.
const COMMON: &[&str] = &[
    "id",
    "name",
    "enabled",
    "tool",
    "orientation",
    "geometry",
    "heights",
    "linking",
    "tolerance",
    "stock_to_leave_radial",
    "stock_to_leave_axial",
    "rest_machining",
    "type",
];

/// The five heights, in the order they are met going down.
const HEIGHTS: [(&str, &str); 5] = [("clearance", "Clearance"), ("retract", "Retract"), ("feed", "Feed"), ("top", "Top"), ("bottom", "Bottom")];

/// The open editor.
pub struct CamEditor {
    pub op: u64,
    pub tab: Tab,
    /// The operation as it was opened or last applied, without its `id`.
    base: Value,
    /// The operation as edited.
    pub draft: Value,
    /// The JSON tab's text and, when it does not parse, why.
    pub text: String,
    pub text_error: Option<String>,
    /// The form's inputs.
    inputs: Inputs,
    /// The last Apply's outcome: a refusal (true) or the warnings it gave (false).
    pub message: Option<(String, bool)>,
}

/// The text typed in each field (by the parameter's path), so a value half typed survives the
/// frame, and the fields whose text is not a value.
#[derive(Default)]
struct Inputs {
    text: HashMap<String, String>,
    invalid: BTreeSet<String>,
}

impl CamEditor {
    fn new(op: u64, saved: Value) -> CamEditor {
        CamEditor {
            op,
            tab: Tab::Fields,
            text: pretty(&saved),
            draft: saved.clone(),
            base: saved,
            text_error: None,
            inputs: Inputs::default(),
            message: None,
        }
    }

    /// Takes `v` as the draft, dropping what the fields held.
    fn set_draft(&mut self, v: Value) {
        self.text = pretty(&v);
        self.text_error = None;
        self.draft = v;
        self.inputs = Inputs::default();
    }

    /// Whether the draft differs from the operation.
    pub fn edited(&self) -> bool {
        match self.tab {
            Tab::Fields => self.draft != self.base,
            Tab::Json => serde_json::from_str::<Value>(&self.text).map_or(true, |v| v != self.base),
        }
    }

    /// Why Apply cannot run yet, if it cannot.
    pub fn blocked(&self) -> Option<String> {
        match self.tab {
            Tab::Json => self.text_error.clone(),
            Tab::Fields => self.inputs.invalid.first().map(|p| format!("`{p}` is not a valid value")),
        }
    }
}

/// The operation as saved, without its `id`.
fn saved(app: &SolveApp, op: u64) -> Option<Value> {
    let (_, o) = app.session.doc.manufacture.operation(op)?;
    let mut v = serde_json::to_value(o).ok()?;
    if let Value::Object(m) = &mut v {
        m.remove("id");
    }
    Some(v)
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

/// Opens the editor on `op` (a second operation replaces the first).
pub fn open(app: &mut SolveApp, op: u64) {
    if let Some(v) = saved(app, op) {
        app.cam_editor = Some(CamEditor::new(op, v));
    }
}

/// Runs `cam.operation.set` with the draft; true when it was taken.
pub fn apply(app: &mut SolveApp, ed: &mut CamEditor) -> bool {
    if ed.tab == Tab::Json {
        match serde_json::from_str::<Value>(&ed.text) {
            Ok(v) => ed.draft = v,
            Err(e) => {
                ed.text_error = Some(e.to_string());
                return false;
            }
        }
    }
    if let Some(why) = ed.blocked() {
        ed.message = Some((why, true));
        return false;
    }
    let mut params = ed.draft.clone();
    if !params.is_object() {
        ed.message = Some(("the operation must be a JSON object".into(), true));
        return false;
    }
    params["operation"] = json!(ed.op);
    if let Value::Object(m) = &mut params {
        m.remove("id");
    }
    match app.run("cam.operation.set", params) {
        Ok(v) => {
            let warnings: Vec<String> =
                v["notice"]["issues"].as_array().into_iter().flatten().filter_map(|i| i["message"].as_str().map(str::to_string)).collect();
            if let Some(now) = saved(app, ed.op) {
                ed.base = now.clone();
                ed.set_draft(now);
            }
            ed.message = (!warnings.is_empty()).then(|| (warnings.join("\n"), false));
            true
        }
        Err(e) => {
            ed.message = Some((e, true));
            false
        }
    }
}

/// Draws the editor when it is open. Called once per frame.
pub fn show(app: &mut SolveApp, ctx: &egui::Context) {
    let Some(mut ed) = app.cam_editor.take() else { return };
    // The operation was deleted (or undone away): nothing left to edit.
    let Some(now) = saved(app, ed.op) else { return };
    let mut changed_elsewhere = false;
    if now != ed.base {
        if ed.edited() {
            changed_elsewhere = true;
        } else {
            ed.base = now.clone();
            ed.set_draft(now.clone());
        }
    }
    let t = Tokens::get();
    let name = ed.base["name"].as_str().unwrap_or("Operation").to_string();
    let title = RichText::new(name).strong().size(t.font_button);
    let tools: Vec<(u64, String)> = app.session.doc.manufacture.tools.tools.iter().map(|tl| (tl.id, format!("T{} {}", tl.number, tl.name))).collect();
    let mut open = true;
    let (mut ok, mut apply_now, mut cancel, mut reload) = (false, false, false, false);
    // Held by its top edge: opening and folding sections grows and shrinks it downwards
    // instead of moving it under the pointer.
    let area = ctx.content_rect();
    crate::frame::window(ctx, title, crate::frame::Width::Wide)
        .id(egui::Id::new("sc_cam_editor"))
        .pivot(egui::Align2::CENTER_TOP)
        .default_pos(egui::pos2(area.center().x, area.top() + t.space_24 * 5.0))
        .min_width(t.space_24 * 18.0)
        .open(&mut open)
        .resizable(true)
        .show(ctx, |ui| {
            // Tabs.
            let mut tab = usize::from(ed.tab == Tab::Json);
            // Room for both labels side by side (narrower, the kit turns segments into a dropdown).
            let width = (t.space_24 * 10.0).min(ui.available_width());
            ui.allocate_ui(egui::vec2(width, crate::ui_kit::row_height()), |ui| {
                crate::ui_kit::segmented(ui, &mut tab, &["Fields", "JSON"], Default::default());
            });
            let want = if tab == 1 { Tab::Json } else { Tab::Fields };
            if want != ed.tab {
                switch(&mut ed, want);
            }
            ui.add_space(t.space_8);
            if changed_elsewhere {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(crate::i18n::tr_label("The operation changed elsewhere since you opened it.")).color(t.warning));
                    if crate::ui_kit::button(ui, "Reload", crate::ui_kit::Button::Normal).clicked() {
                        reload = true;
                    }
                });
            }
            match ed.tab {
                Tab::Fields => {
                    egui::ScrollArea::vertical().max_height(480.0).auto_shrink([false, true]).show(ui, |ui| {
                        let CamEditor { draft, inputs, .. } = &mut ed;
                        fields(ui, draft, inputs, &tools);
                    });
                }
                Tab::Json => {
                    egui::ScrollArea::vertical().max_height(480.0).auto_shrink([false, true]).show(ui, |ui| {
                        let r = ui.add(
                            egui::TextEdit::multiline(&mut ed.text)
                                .id(egui::Id::new("sc_cam_editor_json"))
                                .code_editor()
                                .desired_rows(24)
                                .desired_width(f32::INFINITY),
                        );
                        if r.changed() {
                            ed.text_error = serde_json::from_str::<Value>(&ed.text).err().map(|e| e.to_string());
                        }
                    });
                    if let Some(e) = &ed.text_error {
                        ui.label(RichText::new(e).color(t.error));
                    }
                }
            }
            if let Some(why) = ed.blocked().filter(|_| ed.tab == Tab::Fields) {
                ui.label(RichText::new(why).color(t.error));
            }
            if let Some((m, error)) = &ed.message {
                ui.label(RichText::new(m).color(if *error { t.error } else { t.warning }));
            }
            ui.separator();
            ui.horizontal(|ui| {
                let can = ed.blocked().is_none();
                ui.add_enabled_ui(can, |ui| {
                    if crate::ui_kit::primary_button(ui, "OK").clicked() {
                        ok = true;
                    }
                    if crate::ui_kit::button(ui, "Apply", crate::ui_kit::Button::Normal).clicked() {
                        apply_now = true;
                    }
                });
                if crate::ui_kit::button(ui, "Cancel", crate::ui_kit::Button::Normal).clicked() {
                    cancel = true;
                }
            });
        });
    if reload {
        ed.base = now.clone();
        ed.set_draft(now);
        ed.message = None;
    }
    if ok || apply_now {
        let taken = apply(app, &mut ed);
        if ok && taken {
            return;
        }
    }
    if open && !cancel {
        app.cam_editor = Some(ed);
    }
}

/// Moves the edits to the other tab. A JSON text that does not parse keeps the JSON tab.
fn switch(ed: &mut CamEditor, to: Tab) {
    match to {
        Tab::Json => {
            ed.text = pretty(&ed.draft);
            ed.text_error = None;
            ed.tab = Tab::Json;
        }
        Tab::Fields => match serde_json::from_str::<Value>(&ed.text) {
            Ok(v) if v.is_object() => {
                let text = std::mem::take(&mut ed.text);
                ed.set_draft(v);
                ed.text = text;
                ed.tab = Tab::Fields;
            }
            Ok(_) => ed.text_error = Some("the operation must be a JSON object".into()),
            Err(e) => ed.text_error = Some(e.to_string()),
        },
    }
}

/// The Fields tab.
fn fields(ui: &mut egui::Ui, draft: &mut Value, inputs: &mut Inputs, tools: &[(u64, String)]) {
    let Value::Object(op) = draft else { return };
    // Name, type, tool, enabled.
    if let Some(Value::String(name)) = op.get_mut("name") {
        crate::ui_kit::row(ui, "Name", |ui| crate::ui_kit::text_field(ui, name, ""));
    }
    let ty = op.get("type").and_then(Value::as_str).unwrap_or("").to_string();
    let kinds = cam::ops::OpKind::NAMES;
    let mut k = kinds.iter().position(|n| *n == ty).unwrap_or(0);
    let labels: Vec<String> = kinds.iter().map(|n| kind_label(n)).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    crate::ui_kit::row(ui, "Type", |ui| crate::ui_kit::dropdown(ui, "cam_ed_type", &mut k, &refs));
    if let Some(new) = kinds.get(k).filter(|n| **n != ty) {
        change_kind(op, new);
        inputs.text.clear();
        inputs.invalid.clear();
    }
    let tool = op.get("tool").and_then(Value::as_u64).unwrap_or(0);
    let mut ti = tools.iter().position(|x| x.0 == tool).unwrap_or(usize::MAX);
    let tool_labels: Vec<&str> = tools.iter().map(|x| x.1.as_str()).collect();
    crate::ui_kit::row(ui, "Tool", |ui| crate::ui_kit::dropdown(ui, "cam_ed_tool", &mut ti, &tool_labels));
    if let Some((id, _)) = tools.get(ti).filter(|x| x.0 != tool) {
        op.insert("tool".into(), json!(id));
    }
    if let Some(Value::Bool(on)) = op.get_mut("enabled") {
        crate::ui_kit::checkbox(ui, "Enabled", on);
    }
    // The kind's own parameters.
    let ty = op.get("type").and_then(Value::as_str).unwrap_or("").to_string();
    let choices = cam::ops::OpKind::choices(&ty);
    let mut keys: Vec<String> = cam::ops::OpKind::defaults(&ty).and_then(|d| d.as_object().map(|o| o.keys().cloned().collect())).unwrap_or_default();
    keys.extend(op.keys().filter(|k| !COMMON.contains(&k.as_str()) && !keys.contains(k)).cloned().collect::<Vec<_>>());
    keys.retain(|k| k != "type" && op.contains_key(k));
    section(ui, "Parameters", true, |ui| {
        for key in &keys {
            let list = choices.iter().find(|(k, _)| k == key).map(|x| x.1.as_slice());
            if let Some(v) = op.get_mut(key) {
                value(ui, inputs, key, &label_of(key, v), v, list);
            }
        }
    });
    section(ui, "Stock to leave", false, |ui| {
        for (key, label) in [
            ("stock_to_leave_radial", "Radial (mm)"),
            ("stock_to_leave_axial", "Axial (mm)"),
            ("rest_machining", "Rest machining"),
            ("tolerance", "Tolerance (mm)"),
        ] {
            if let Some(v) = op.get_mut(key) {
                value(ui, inputs, key, label, v, None);
            }
        }
    });
    section(ui, "Orientation", false, |ui| {
        let mut tilted = op.contains_key("orientation");
        crate::ui_kit::checkbox(ui, "Tilted tool axis (3+2)", &mut tilted).on_hover_text("orientation");
        if tilted && !op.contains_key("orientation") {
            op.insert("orientation".into(), json!({"origin": [0.0, 0.0, 0.0], "x": [1.0, 0.0, 0.0], "y": [0.0, 1.0, 0.0], "z": [0.0, 0.0, 1.0]}));
        } else if !tilted {
            op.remove("orientation");
            inputs.text.retain(|k, _| !k.starts_with("orientation."));
            inputs.invalid.retain(|k| !k.starts_with("orientation."));
        }
        if let Some(Value::Object(o)) = op.get_mut("orientation") {
            for (key, label) in [("origin", "Origin"), ("x", "X axis"), ("y", "Y axis"), ("z", "Tool axis (Z)")] {
                if let Some(v) = o.get_mut(key) {
                    value(ui, inputs, &format!("orientation.{key}"), label, v, None);
                }
            }
        }
    });
    if let Some(g) = op.get_mut("geometry") {
        section(ui, "Geometry", false, |ui| {
            let sources = cam::ops::variants::<cam::setup::Geometry>(json!({"source": "\u{1}"}));
            tagged(ui, inputs, "geometry", g, "source", &sources);
        });
    }
    if let Some(Value::Object(h)) = op.get_mut("heights") {
        section(ui, "Heights", false, |ui| {
            let from = cam::ops::variants::<cam::linking::HeightRef>(Value::Null);
            for (key, label) in HEIGHTS {
                if let Some(v) = h.get_mut(key) {
                    height(ui, inputs, &format!("heights.{key}"), label, v, &from);
                }
            }
        });
    }
    if let Some(Value::Object(l)) = op.get_mut("linking") {
        section(ui, "Linking", false, |ui| {
            let keys: Vec<String> = l.keys().cloned().collect();
            for key in keys {
                let path = format!("linking.{key}");
                let Some(v) = l.get_mut(&key) else { continue };
                if key == "ramp" {
                    let ramps = cam::ops::variants::<cam::linking::Ramp>(json!({"type": "\u{1}"}));
                    group(ui, "Ramp", |ui| tagged(ui, inputs, &path, v, "type", &ramps));
                } else {
                    let label = label_of(&key, v);
                    value(ui, inputs, &path, &label, v, None);
                }
            }
        });
    }
}

/// A collapsible section of the form.
fn section(ui: &mut egui::Ui, title: &str, open: bool, add: impl FnOnce(&mut egui::Ui)) {
    let t = Tokens::get();
    egui::CollapsingHeader::new(RichText::new(crate::i18n::tr_label(title)).strong().color(t.text))
        .id_salt(("cam_ed_section", title))
        .default_open(open)
        .show(ui, add);
}

/// A nested group (a lead, the ramp) inside a section.
fn group(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(crate::i18n::tr_label(title)).id_salt(("cam_ed_group", title)).default_open(true).show(ui, add);
}

/// One parameter: a checkbox, a number, a choice, a text, a point, or a group of them.
fn value(ui: &mut egui::Ui, inputs: &mut Inputs, path: &str, label: &str, v: &mut Value, choices: Option<&[String]>) {
    match v {
        Value::Bool(b) => {
            crate::ui_kit::checkbox(ui, label, b).on_hover_text(path);
        }
        Value::Number(_) => {
            let integer = v.is_u64() || v.is_i64();
            let shown = number_text(v);
            if let Some(n) = typed(ui, inputs, path, label, &shown, "", |s| parse_number(s, integer)) {
                *v = n;
            }
        }
        Value::String(s) => match choices {
            Some(list) if !list.is_empty() => {
                let mut i = list.iter().position(|c| c == s).unwrap_or(0);
                let labels: Vec<String> = list.iter().map(|c| kind_label(c)).collect();
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                crate::ui_kit::row(ui, label, |ui| crate::ui_kit::dropdown(ui, &format!("cam_ed:{path}"), &mut i, &refs)).on_hover_text(path);
                if let Some(c) = list.get(i).filter(|c| *c != s) {
                    *s = c.clone();
                }
            }
            _ => {
                crate::ui_kit::row(ui, label, |ui| crate::ui_kit::text_field(ui, s, ""));
            }
        },
        // Not set: a point typed as "x, y" sets it, an empty field leaves it automatic.
        Value::Null => {
            if let Some(p) = typed(ui, inputs, path, label, "", "auto", parse_point) {
                *v = p;
            }
        }
        Value::Object(o) => {
            group(ui, label, |ui| {
                let keys: Vec<String> = o.keys().cloned().collect();
                for key in keys {
                    let p = format!("{path}.{key}");
                    if let Some(x) = o.get_mut(&key) {
                        let l = label_of(&key, x);
                        value(ui, inputs, &p, &l, x, None);
                    }
                }
            });
        }
        Value::Array(a) if is_point(a) => {
            let shown = point_text(a);
            let hint = if a.len() == 2 { "x, y" } else { "x, y, z" };
            if let Some(p) = typed(ui, inputs, path, label, &shown, hint, parse_point) {
                *v = p;
            }
        }
        Value::Array(a) => list(ui, inputs, path, label, a),
    }
}

/// The most rows a list shows; the rest are in the JSON tab.
const LIST_ROWS: usize = 500;

/// A list (picked points, a loop's points, the loops): a row per item with a remove button,
/// and Add, which repeats the last item (or starts a point) for editing.
fn list(ui: &mut egui::Ui, inputs: &mut Inputs, path: &str, label: &str, a: &mut Vec<Value>) {
    let t = Tokens::get();
    let mut remove = None;
    let mut add = false;
    let title = format!("{label} ({})", a.len());
    egui::CollapsingHeader::new(title).id_salt(("cam_ed_list", path)).default_open(a.len() <= 12).show(ui, |ui| {
        for (i, x) in a.iter_mut().enumerate().take(LIST_ROWS) {
            let p = format!("{path}.{i}");
            let what = match x {
                Value::Array(p) if is_point(p) => "Point",
                Value::Array(_) => "Loop",
                _ => "Item",
            };
            let l = format!("{what} {}", i + 1);
            ui.horizontal(|ui| {
                if crate::ui_kit::button(ui, "✕", crate::ui_kit::Button::Small).on_hover_text(crate::i18n::tr_label("Remove")).clicked() {
                    remove = Some(i);
                }
                ui.vertical(|ui| value(ui, inputs, &p, &l, x, None));
            });
        }
        if a.len() > LIST_ROWS {
            ui.label(RichText::new(format!("{} more in the JSON tab", a.len() - LIST_ROWS)).color(t.text_dim));
        }
        if crate::ui_kit::button(ui, "Add", crate::ui_kit::Button::Small).clicked() {
            add = true;
        }
    });
    if remove.is_some() || add {
        if let Some(i) = remove.filter(|i| *i < a.len()) {
            a.remove(i);
        }
        if add {
            a.push(new_item(path, a.last()));
        }
        // Rows moved: their typed text belongs to other items now.
        let prefix = format!("{path}.");
        inputs.text.retain(|k, _| !k.starts_with(&prefix));
        inputs.invalid.retain(|k| !k.starts_with(&prefix));
    }
}

/// The item Add appends: a copy of the last, else an empty loop for `loops` or a point.
fn new_item(path: &str, last: Option<&Value>) -> Value {
    match last {
        Some(Value::Array(p)) if !is_point(p) => json!([]),
        Some(x) => x.clone(),
        None if path.ends_with("loops") => json!([]),
        None if path.contains("loops.") => json!([0.0, 0.0]),
        None => json!([0.0, 0.0, 0.0]),
    }
}

/// A text field holding `shown` until typed in; returns the parsed value once the text is valid.
fn typed(
    ui: &mut egui::Ui,
    inputs: &mut Inputs,
    path: &str,
    label: &str,
    shown: &str,
    hint: &str,
    parse: impl Fn(&str) -> Option<Value>,
) -> Option<Value> {
    let text = inputs.text.entry(path.to_string()).or_insert_with(|| shown.to_string());
    let changed = crate::ui_kit::row(ui, label, |ui| {
        let r = ui.scope(|ui| crate::ui_kit::text_field(ui, text, hint).1);
        r.response.on_hover_text(path);
        r.inner
    });
    if !changed {
        return None;
    }
    match parse(text) {
        Some(v) => {
            inputs.invalid.remove(path);
            Some(v)
        }
        None => {
            inputs.invalid.insert(path.to_string());
            None
        }
    }
}

/// A height: what it is measured from, and the offset from there.
fn height(ui: &mut egui::Ui, inputs: &mut Inputs, path: &str, label: &str, v: &mut Value, from: &[String]) {
    let Value::Object(h) = v else { return };
    let t = Tokens::get();
    let current = h.get("from").and_then(Value::as_str).unwrap_or("").to_string();
    let mut i = from.iter().position(|c| *c == current).unwrap_or(0);
    let labels: Vec<String> = from.iter().map(|c| kind_label(c)).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let offset = h.get("offset").map(number_text).unwrap_or_default();
    let text = inputs.text.entry(format!("{path}.offset")).or_insert(offset);
    let changed = crate::ui_kit::row(ui, label, |ui| {
        let w = ((ui.available_width() - t.space_8) / 2.0).max(0.0);
        ui.spacing_mut().item_spacing.x = t.space_8;
        ui.allocate_ui(egui::vec2(w, crate::ui_kit::row_height()), |ui| crate::ui_kit::dropdown(ui, &format!("cam_ed:{path}"), &mut i, &refs));
        ui.allocate_ui(egui::vec2(w, crate::ui_kit::row_height()), |ui| crate::ui_kit::text_field(ui, text, "offset mm").1).inner
    });
    if let Some(c) = from.get(i).filter(|c| **c != current) {
        h.insert("from".into(), json!(c));
    }
    if changed {
        let key = format!("{path}.offset");
        let parsed = inputs.text.get(&key).and_then(|s| parse_number(s, false));
        match parsed {
            Some(n) => {
                inputs.invalid.remove(&key);
                h.insert("offset".into(), n);
            }
            None => {
                inputs.invalid.insert(key);
            }
        }
    }
}

/// A choice that brings its own parameters (the ramp's kind, the geometry's source): a dropdown
/// of the choices, then the chosen one's parameters.
fn tagged(ui: &mut egui::Ui, inputs: &mut Inputs, path: &str, v: &mut Value, tag: &str, choices: &[String]) {
    let Value::Object(o) = v else { return };
    let current = o.get(tag).and_then(Value::as_str).unwrap_or("").to_string();
    let mut i = choices.iter().position(|c| *c == current).unwrap_or(0);
    let labels: Vec<String> = choices.iter().map(|c| kind_label(c)).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let label = if tag == "source" { "Source" } else { "Kind" };
    crate::ui_kit::row(ui, label, |ui| crate::ui_kit::dropdown(ui, &format!("cam_ed:{path}.{tag}"), &mut i, &refs));
    if let Some(c) = choices.get(i).filter(|c| **c != current) {
        let mut new = tagged_default(tag, c);
        if let Value::Object(n) = &mut new {
            for (k, x) in n.iter_mut() {
                if let Some(old) = o.get(k).filter(|old| std::mem::discriminant(*old) == std::mem::discriminant(&*x)) {
                    *x = old.clone();
                }
            }
            n.insert(tag.into(), json!(c));
            *o = std::mem::take(n);
        }
        inputs.text.retain(|k, _| !k.starts_with(&format!("{path}.")));
        inputs.invalid.retain(|k| !k.starts_with(&format!("{path}.")));
    }
    let keys: Vec<String> = o.keys().filter(|k| *k != tag).cloned().collect();
    for key in keys {
        let p = format!("{path}.{key}");
        if let Some(x) = o.get_mut(&key) {
            let l = label_of(&key, x);
            value(ui, inputs, &p, &l, x, None);
        }
    }
}

/// The parameters a ramp kind or geometry source starts with.
fn tagged_default(tag: &str, choice: &str) -> Value {
    match (tag, choice) {
        ("type", "helix") => json!({"diameter": 0.0, "angle_deg": 2.0}),
        ("type", "zigzag" | "profile") => json!({"angle_deg": 2.0}),
        ("source", "loops") => json!({"loops": []}),
        ("source", "sketch") => json!({"feature": 0}),
        ("source", "faces") => json!({"points": []}),
        ("source", "holes") => json!({"points": [], "diameter": 0.0}),
        ("source", "chains") => json!({"bottom": [], "top": []}),
        ("source", "rails") => json!({"first": [], "second": []}),
        _ => json!({}),
    }
}

/// Changes the operation's kind: the old kind's parameters go, the new one's come at their
/// defaults; name, tool, geometry, heights and linking stay.
fn change_kind(op: &mut Map<String, Value>, ty: &str) {
    let Some(Value::Object(d)) = cam::ops::OpKind::defaults(ty) else { return };
    op.retain(|k, _| COMMON.contains(&k.as_str()));
    for (k, v) in d {
        op.insert(k, v);
    }
}

/// A key as a label, with its unit: `stay_down_distance` is "Stay down distance (mm)".
fn label_of(key: &str, v: &Value) -> String {
    let unit = unit_of(key, v);
    let bare = key.strip_suffix("_deg").or_else(|| key.strip_suffix("_s")).or_else(|| key.strip_suffix("_mm")).unwrap_or(key);
    let words = kind_label(bare);
    if unit.is_empty() { words } else { format!("{words} ({unit})") }
}

/// The unit a parameter is in, from its name: lengths are mm, `_deg` degrees, `_s` seconds.
fn unit_of(key: &str, v: &Value) -> &'static str {
    if !v.is_f64() {
        return "";
    }
    if key.ends_with("_deg") {
        "°"
    } else if key.ends_with("_s") {
        "s"
    } else if key == "power" {
        "0–1"
    } else if key == "speed" {
        "mm/min"
    } else {
        "mm"
    }
}

/// `steep_shallow` as "Steep shallow", `pocket2d` as "Pocket 2D".
fn kind_label(name: &str) -> String {
    let spaced = name.replace('_', " ").replace("2d", " 2D").replace("3d", " 3D");
    let mut c = spaced.trim().chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn number_text(v: &Value) -> String {
    match v.as_f64() {
        Some(x) if v.is_f64() => {
            let s = format!("{x:.6}");
            let s = s.trim_end_matches('0').trim_end_matches('.');
            if s == "-0" { "0".into() } else { s.to_string() }
        }
        _ => v.to_string(),
    }
}

/// A typed number: a whole number for a count, else a decimal.
fn parse_number(s: &str, integer: bool) -> Option<Value> {
    let x = crate::prefs::parse_number(s.trim().trim_end_matches("mm").trim_end_matches('°'))?;
    if integer { (x.fract() == 0.0 && (0.0..=1e9).contains(&x)).then(|| json!(x as u64)) } else { Some(json!(x)) }
}

/// A point as saved: `[x, y]` or `[x, y, z]`.
fn is_point(a: &[Value]) -> bool {
    (2..=3).contains(&a.len()) && a.iter().all(Value::is_number)
}

fn point_text(a: &[Value]) -> String {
    a.iter().map(number_text).collect::<Vec<_>>().join(", ")
}

/// "x, y" or "x, y, z" (spaces or semicolons also separate); an empty text is not set.
fn parse_point(s: &str) -> Option<Value> {
    let s = s.trim();
    if s.is_empty() {
        return Some(Value::Null);
    }
    let parts: Vec<f64> = s.split([',', ';', ' ']).filter(|p| !p.is_empty()).map(crate::prefs::parse_number).collect::<Option<_>>()?;
    match parts.as_slice() {
        [x, y] => Some(json!([x, y])),
        [x, y, z] => Some(json!([x, y, z])),
        _ => None,
    }
}

#[cfg(test)]
#[path = "cam_editor_tests.rs"]
mod tests;
