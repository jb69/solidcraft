//! The operation and tool editor. Double-clicking an operation or a tool in the Browser, or its
//! Edit menu, opens it in a window with two tabs over one draft of it:
//! - **Fields** lays the parameters out as inputs: name, type, tool, the kind's own parameters,
//!   stock to leave, geometry, heights and linking. The form is built from the operation's own
//!   saved form and its kind's defaults, so every kind, present and future, gets one. A tool's
//!   form has its number, kind (with the kind's own sizes), size, and feeds and speeds.
//! - **JSON** shows the same draft as the text `cam.operation.set` (`cam.tool.set`) takes, for
//!   reading, copying and pasting.
//!
//! Switching tabs carries the edits across. Apply and OK run the set command with the draft, the
//! same command an assistant runs, as one undo step. When the operation or tool changes
//! elsewhere (an assistant, undo) an untouched draft follows it; an edited one offers to reload.

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
    "boundary",
    "type",
];

/// What the editor edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Operation(u64),
    /// An operation not added yet, for this setup: OK and Apply add it (`cam.operation.add`).
    NewOperation(u64),
    Tool(u64),
    Setup(u64),
}

impl Target {
    /// The command that sets it, and the parameter naming it.
    fn command(self) -> (&'static str, &'static str, u64) {
        match self {
            Target::Operation(id) => ("cam.operation.set", "operation", id),
            Target::NewOperation(setup) => ("cam.operation.add", "setup", setup),
            Target::Tool(id) => ("cam.tool.set", "tool", id),
            Target::Setup(id) => ("cam.setup.set", "setup", id),
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Target::Operation(_) | Target::NewOperation(_) => "operation",
            Target::Tool(_) => "tool",
            Target::Setup(_) => "setup",
        }
    }
}

/// A tool kind's own sizes, beside the sizes every tool has.
const TOOL_KIND_KEYS: &[&str] = &["corner_radius", "tip_angle_deg", "wavelength_nm", "tip_radius", "taper_angle_deg"];

/// The five heights, in the order they are met going down.
const HEIGHTS: [(&str, &str); 5] = [("clearance", "Clearance"), ("retract", "Retract"), ("feed", "Feed"), ("top", "Top"), ("bottom", "Bottom")];

/// The open editor.
pub struct CamEditor {
    pub target: Target,
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
    /// What the next Apply adds to `cam.setup.set`: `flip` (turn the setup over), `origin` (a
    /// coordinate or a construction point) or `origin_at` (a point of the stock or model).
    pub place: Option<Value>,
    /// Placing a setup's origin: the choices in the form, and picking on the model.
    pub origin: OriginForm,
}

/// Placing a setup's origin: which point of the stock or model, which construction point, and
/// picking on the model (the snap under the pointer, and the selection when picking began, so
/// the click that picks is the one that changes it).
#[derive(Default)]
pub struct OriginForm {
    of: usize,
    x: usize,
    y: usize,
    z: usize,
    point: usize,
    pub picking: bool,
    before: Option<Vec<solidcraft_engine::Sel>>,
    hover: Option<(solidcraft_engine::Sel, Option<crate::dialogs_assembly::SnapPt>)>,
}

/// The text typed in each field (by the parameter's path), so a value half typed survives the
/// frame, and the fields whose text is not a value.
#[derive(Default)]
struct Inputs {
    text: HashMap<String, String>,
    invalid: BTreeSet<String>,
}

impl CamEditor {
    fn new(target: Target, saved: Value) -> CamEditor {
        CamEditor {
            target,
            tab: Tab::Fields,
            text: pretty(&saved),
            draft: saved.clone(),
            base: saved,
            text_error: None,
            inputs: Inputs::default(),
            message: None,
            place: None,
            origin: OriginForm::default(),
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

/// The operation geometry the viewport's selection makes for an operation of kind `ty`, and a few
/// words saying what it is: picked faces (holes for drilling kinds) or a sketch's profiles.
pub fn selection_geometry(app: &SolveApp, ty: &str) -> Option<(Value, String)> {
    use solidcraft_engine::Sel;
    let sel = &app.session.selection;
    let points: Vec<Value> = sel.iter().filter_map(|s| if let Sel::Face { point, .. } = s { Some(json!([point.x, point.y, point.z])) } else { None }).collect();
    if !points.is_empty() {
        let n = points.len();
        let holes = matches!(ty, "drill" | "bore" | "circular" | "thread");
        let (g, what) = if holes {
            (json!({"source": "holes", "points": points, "diameter": 0.0}), if n == 1 { "1 hole".to_string() } else { format!("{n} holes") })
        } else {
            (json!({"source": "faces", "points": points}), if n == 1 { "1 face".to_string() } else { format!("{n} faces") })
        };
        return Some((g, what));
    }
    sel.iter().find_map(|s| if let Sel::Profile { sketch, .. } = s { Some(*sketch) } else { None }).map(|feature| {
        let name = app.session.model.state().sketch(feature).map(|k| k.name.clone()).unwrap_or_else(|| format!("sketch {feature}"));
        (json!({"source": "sketch", "feature": feature}), format!("the profiles of {name}"))
    })
}

/// The operation or tool as saved, without its `id`.
fn saved(app: &SolveApp, target: Target) -> Option<Value> {
    let m = &app.session.doc.manufacture;
    let mut v = match target {
        Target::Operation(op) => serde_json::to_value(m.operation(op)?.1).ok()?,
        Target::Tool(tool) => serde_json::to_value(m.tools.get(tool)?).ok()?,
        Target::Setup(setup) => serde_json::to_value(m.setup(setup)?).ok()?,
        Target::NewOperation(_) => return None,
    };
    if let Value::Object(m) = &mut v {
        m.remove("id");
        // A setup's operations are edited one by one.
        if matches!(target, Target::Setup(_)) {
            m.remove("operations");
        }
    }
    Some(v)
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

/// Opens the editor on an operation or a tool (opening another replaces it).
pub fn open(app: &mut SolveApp, target: Target) {
    if let Some(v) = saved(app, target) {
        app.cam_editor = Some(CamEditor::new(target, v));
    }
}

/// Opens the editor on a new operation for `setup`, from a `cam.operation.<kind>` call (its
/// kind's parameters, tool, name and geometry): the whole operation is in the form before it is
/// added. False when the call does not make an operation.
pub fn open_new(app: &mut SolveApp, setup: u64, kind: &str, call: &Value) -> bool {
    let mut v = call.clone();
    let Value::Object(o) = &mut v else { return false };
    o.remove("setup");
    o.entry("type").or_insert_with(|| json!(kind));
    o.insert("id".into(), json!(0));
    // Through the saved form, so every field the operation has is in the draft.
    let Some(mut full) = serde_json::from_value::<cam::setup::Operation>(v).ok().and_then(|op| serde_json::to_value(op).ok()) else {
        return false;
    };
    if let Value::Object(m) = &mut full {
        m.remove("id");
    }
    app.cam_editor = Some(CamEditor::new(Target::NewOperation(setup), full));
    true
}

/// Runs the set command with the draft; true when it was taken.
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
        ed.message = Some((format!("the {} must be a JSON object", ed.target.noun()), true));
        return false;
    }
    let (command, key, id) = ed.target.command();
    params[key] = json!(id);
    if let Some(Value::Object(extra)) = ed.place.take() {
        for (k, v) in extra {
            params[k.as_str()] = v;
        }
    }
    if let Value::Object(m) = &mut params {
        m.remove("id");
    }
    match app.run(command, params) {
        Ok(v) => {
            // Added: from now on the editor edits the operation it made.
            if let (Target::NewOperation(_), Some(id)) = (ed.target, v["id"].as_u64()) {
                ed.target = Target::Operation(id);
            }
            let warnings: Vec<String> =
                v["notice"]["issues"].as_array().into_iter().flatten().filter_map(|i| i["message"].as_str().map(str::to_string)).collect();
            if let Some(now) = saved(app, ed.target) {
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
    // Deleted (or undone away): nothing left to edit.
    let now = match ed.target {
        Target::NewOperation(_) => ed.base.clone(),
        target => match saved(app, target) {
            Some(v) => v,
            None => return,
        },
    };
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
    let name = match (ed.base["name"].as_str(), ed.target) {
        (Some(n), Target::Tool(_)) => format!("T{} {n}", ed.base["number"].as_u64().unwrap_or(0)),
        (Some(n), Target::Operation(_)) => n.to_string(),
        (Some(n), Target::NewOperation(_)) => format!("New operation: {n}"),
        (Some(n), Target::Setup(_)) => format!("Setup: {n}"),
        (None, _) => kind_label(ed.target.noun()),
    };
    let title = RichText::new(name).strong().size(t.font_button);
    let uses = match ed.target {
        Target::Tool(id) => app.session.doc.manufacture.tool_uses(id),
        _ => 0,
    };
    let setups: Vec<(u64, String)> = app.session.doc.manufacture.setups.iter().map(|x| (x.id, x.name.clone())).collect();
    // The boundary the last generation used, from its notice.
    let used: Option<String> = match ed.target {
        Target::Operation(id) => {
            app.session.cam.operations.get(&id).and_then(|g| g.notice.issues.iter().find(|i| i.code == "boundary").map(|i| i.message.clone()))
        }
        _ => None,
    };
    let tools: Vec<(u64, String)> = app.session.doc.manufacture.tools.tools.iter().map(|tl| (tl.id, format!("T{} {}", tl.number, tl.name))).collect();
    // Points the designer placed (construction points), for the setup's origin.
    let points: Vec<String> = match ed.target {
        Target::Setup(_) => app
            .session
            .model
            .state()
            .construct
            .iter()
            .filter(|c| matches!(c.geom, solidcraft_engine::doc::construct::ConstructGeom::Point(_)))
            .map(|c| c.name.clone())
            .collect(),
        _ => Vec::new(),
    };
    let picked = match ed.target {
        Target::Operation(_) | Target::NewOperation(_) => selection_geometry(app, ed.draft["type"].as_str().unwrap_or("")),
        _ => None,
    };
    let mut open = true;
    let (mut ok, mut apply_now, mut cancel, mut reload) = (false, false, false, false);
    // The form scrolls within most of the window's height.
    let tall = (ctx.content_rect().height() * 0.72).max(320.0);
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
                    let text = format!("The {} changed elsewhere since you opened it.", ed.target.noun());
                    ui.label(RichText::new(text).color(t.warning));
                    if crate::ui_kit::button(ui, "Reload", crate::ui_kit::Button::Normal).clicked() {
                        reload = true;
                    }
                });
            }
            match ed.tab {
                Tab::Fields => {
                    egui::ScrollArea::vertical().max_height(tall).auto_shrink([false, true]).show(ui, |ui| {
                        let CamEditor { draft, inputs, target, place, origin, .. } = &mut ed;
                        match target {
                            Target::Operation(_) => fields(ui, draft, inputs, &tools, used.as_deref(), picked.as_ref()),
                            Target::NewOperation(setup) => {
                                // Which setup it goes into (the one picked in the Browser to start with).
                                let mut k = setups.iter().position(|x| x.0 == *setup).unwrap_or(0);
                                let names: Vec<&str> = setups.iter().map(|x| x.1.as_str()).collect();
                                crate::ui_kit::row(ui, "Setup", |ui| crate::ui_kit::dropdown(ui, "cam_ed_setup", &mut k, &names));
                                if let Some(x) = setups.get(k) {
                                    *setup = x.0;
                                }
                                fields(ui, draft, inputs, &tools, None, picked.as_ref());
                            }
                            Target::Tool(_) => tool_fields(ui, draft, inputs, uses),
                            Target::Setup(id) => {
                                if let Some(extra) = setup_fields(ui, draft, inputs, *id, &setups, origin, &points) {
                                    *place = Some(extra);
                                    apply_now = true;
                                }
                            }
                        }
                    });
                }
                Tab::Json => {
                    egui::ScrollArea::vertical().max_height(tall).auto_shrink([false, true]).show(ui, |ui| {
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
    // Picking the origin on the model: the click that changes the selection places it, at the
    // snap of what it picked (a coordinate).
    if ed.origin.picking {
        let sel = &app.session.selection;
        match &ed.origin.before {
            None => ed.origin.before = Some(sel.clone()),
            Some(before) if before != sel => {
                if let Some(snap) = sel.last().and_then(|x| crate::dialogs_assembly::snap_of(&app.session, x)) {
                    ed.place = Some(json!({"origin": [snap.at.x, snap.at.y, snap.at.z]}));
                    ed.origin.picking = false;
                    apply_now = true;
                } else {
                    ed.origin.before = Some(sel.clone());
                }
            }
            Some(_) => {}
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ed.origin.picking = false;
        }
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
            Ok(_) => ed.text_error = Some(format!("the {} must be a JSON object", ed.target.noun())),
            Err(e) => ed.text_error = Some(e.to_string()),
        },
    }
}

/// The Fields tab.
/// `picked`: the geometry the viewport's selection would give, for Use Selected.
fn fields(ui: &mut egui::Ui, draft: &mut Value, inputs: &mut Inputs, tools: &[(u64, String)], used: Option<&str>, picked: Option<&(Value, String)>) {
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
    let ty = op.get("type").and_then(Value::as_str).unwrap_or("").to_string();
    // What the operation works on, and a way to take it from the viewport's selection, at the
    // top where it is seen first.
    let bounded = serde_json::from_value::<cam::ops::OpKind>(json!({"type": ty})).is_ok_and(|k| k.uses_boundary());
    let picks = op.get("geometry").and_then(geometry_picks);
    let wants_selection = op.get("boundary").and_then(|b| b.get("source")).and_then(Value::as_str) == Some("selection");
    let t = Tokens::get();
    match &picks {
        Some(w) => {
            ui.label(RichText::new(format!("Selection: {w}")).color(t.text_dim));
        }
        None if wants_selection => {
            ui.label(RichText::new("Nothing is picked: select faces or a sketch in the viewport, then Use Selected").color(t.warning));
        }
        None => {}
    }
    if let Some((g, what)) = picked
        && crate::ui_kit::button(ui, &format!("Use Selected ({what})"), crate::ui_kit::Button::Normal)
            .on_hover_text("Work on what is selected in the viewport: sets the geometry (and an area operation's boundary) to the selection")
            .clicked()
    {
        op.insert("geometry".into(), g.clone());
        inputs.text.retain(|k, _| !k.starts_with("geometry."));
        inputs.invalid.retain(|k| !k.starts_with("geometry."));
        if bounded {
            let b = op.entry("boundary").or_insert_with(|| serde_json::to_value(cam::boundary::Boundary::default()).unwrap_or(Value::Null));
            if let Value::Object(b) = b {
                b.insert("source".into(), json!("selection"));
            }
            inputs.text.remove("boundary.source");
        }
    }
    // The kind's own parameters.
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
    // Where the operation may machine (area operations).
    if bounded {
        if !op.contains_key("boundary") {
            op.insert("boundary".into(), serde_json::to_value(cam::boundary::Boundary::default()).unwrap_or(Value::Null));
        }
        if let Some(Value::Object(b)) = op.get_mut("boundary") {
            section(ui, "Boundary", true, |ui| {
                let t = Tokens::get();
                let sources = cam::ops::variants::<cam::boundary::Source>(Value::Null);
                let contain = cam::ops::variants::<cam::boundary::Containment>(Value::Null);
                for (key, label) in [
                    ("source", "Machine within"),
                    ("containment", "Tool"),
                    ("offset", "Additional offset (mm)"),
                    ("clip_to_stock", "Stay within the stock"),
                    ("avoid_fixtures", "Keep clear of fixtures"),
                    ("fixture_clearance", "Fixture clearance (mm)"),
                ] {
                    let choices = match key {
                        "source" => Some(sources.as_slice()),
                        "containment" => Some(contain.as_slice()),
                        _ => None,
                    };
                    if let Some(v) = b.get_mut(key) {
                        value(ui, inputs, &format!("boundary.{key}"), label, v, choices);
                    }
                }
                let hint = "Auto: roughing clears around the part's outline, wall finishing reaches the walls from outside, surface finishing keeps to the part, and a selection bounds the operation to what was picked.";
                ui.label(RichText::new(crate::i18n::tr_label(hint)).color(t.text_dim));
                if let Some(u) = used {
                    ui.label(RichText::new(format!("Last generated: {u}")).color(t.text_dim));
                }
            });
        }
    }
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

/// The Fields tab of a tool.
fn tool_fields(ui: &mut egui::Ui, draft: &mut Value, inputs: &mut Inputs, uses: usize) {
    let Value::Object(tl) = draft else { return };
    let t = Tokens::get();
    if uses > 0 {
        let s = if uses == 1 { "" } else { "s" };
        let text = format!("Used by {uses} operation{s}: a change makes their toolpaths stale.");
        ui.label(RichText::new(text).color(t.text_dim));
    }
    if let Some(Value::String(name)) = tl.get_mut("name") {
        crate::ui_kit::row(ui, "Name", |ui| crate::ui_kit::text_field(ui, name, ""));
    }
    for (key, label) in [("number", "Tool number (T)"), ("length_offset", "Length offset (H)")] {
        if let Some(v) = tl.get_mut(key) {
            value(ui, inputs, key, label, v, None);
        }
    }
    // The kind, then its own sizes.
    let kinds = cam::ops::variants::<cam::tool::ToolKind>(json!({"kind": "\u{1}"}));
    let current = tl.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
    let mut k = kinds.iter().position(|c| *c == current).unwrap_or(0);
    let labels: Vec<String> = kinds.iter().map(|c| kind_label(c)).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    crate::ui_kit::row(ui, "Kind", |ui| crate::ui_kit::dropdown(ui, "cam_ed_tool_kind", &mut k, &refs));
    if let Some(new) = kinds.get(k).filter(|c| **c != current) {
        let diameter = tl.get("diameter").and_then(Value::as_f64).unwrap_or(3.0);
        tl.retain(|key, _| !TOOL_KIND_KEYS.contains(&key.as_str()));
        tl.insert("kind".into(), json!(new));
        if let Value::Object(d) = tool_kind_default(new, diameter) {
            tl.extend(d);
        }
        inputs.text.retain(|key, _| !TOOL_KIND_KEYS.contains(&key.as_str()));
        inputs.invalid.retain(|key| !TOOL_KIND_KEYS.contains(&key.as_str()));
    }
    for key in TOOL_KIND_KEYS {
        if let Some(v) = tl.get_mut(*key) {
            let label = label_of(key, v);
            value(ui, inputs, key, &label, v, None);
        }
    }
    section(ui, "Size", true, |ui| {
        for (key, label) in [
            ("diameter", "Diameter (mm)"),
            ("flute_length", "Flute length (mm)"),
            ("shank_diameter", "Shank diameter (mm)"),
            ("overall_length", "Stick-out (mm)"),
            ("holder_diameter", "Holder diameter (mm)"),
            ("flutes", "Flutes"),
        ] {
            if let Some(v) = tl.get_mut(key) {
                value(ui, inputs, key, label, v, None);
            }
        }
    });
    if let Some(Value::Object(f)) = tl.get_mut("feeds") {
        section(ui, "Feeds and speeds", true, |ui| {
            let coolants = cam::ops::variants::<cam::tool::Coolant>(Value::Null);
            let keys: Vec<String> = f.keys().cloned().collect();
            for key in keys {
                let path = format!("feeds.{key}");
                let Some(v) = f.get_mut(&key) else { continue };
                let label = match key.as_str() {
                    "spindle_rpm" => "Spindle speed (rev/min)".to_string(),
                    "clockwise" => "Clockwise (M3)".to_string(),
                    "coolant" => "Coolant".to_string(),
                    _ if v.is_number() => format!("{} feed (mm/min)", kind_label(&key)),
                    _ => kind_label(&key),
                };
                let choices = (key == "coolant").then_some(coolants.as_slice());
                value(ui, inputs, &path, &label, v, choices);
            }
        });
    }
    // Anything newer than this form.
    let known = [
        "name",
        "number",
        "length_offset",
        "kind",
        "diameter",
        "flute_length",
        "shank_diameter",
        "overall_length",
        "holder_diameter",
        "flutes",
        "feeds",
    ];
    let rest: Vec<String> = tl.keys().filter(|k| !known.contains(&k.as_str()) && !TOOL_KIND_KEYS.contains(&k.as_str())).cloned().collect();
    if !rest.is_empty() {
        section(ui, "Other", false, |ui| {
            for key in rest {
                if let Some(v) = tl.get_mut(&key) {
                    let label = label_of(&key, v);
                    value(ui, inputs, &key, &label, v, None);
                }
            }
        });
    }
}

/// The Fields tab of a setup; returns an axis when a Turn Over button was pressed.
/// The setup's form; returns what to add to `cam.setup.set` when a button applies at once
/// (turn over, place the origin).
fn setup_fields(
    ui: &mut egui::Ui,
    draft: &mut Value,
    inputs: &mut Inputs,
    id: u64,
    setups: &[(u64, String)],
    of: &mut OriginForm,
    points: &[String],
) -> Option<Value> {
    let Value::Object(st) = draft else { return None };
    let t = Tokens::get();
    let mut flip: Option<Value> = None;
    if let Some(Value::String(name)) = st.get_mut("name") {
        crate::ui_kit::row(ui, "Name", |ui| crate::ui_kit::text_field(ui, name, ""));
    }
    if let Some(Value::String(program)) = st.get_mut("program") {
        crate::ui_kit::row(ui, "Program", |ui| crate::ui_kit::text_field(ui, program, "the setup's name"));
    }
    // The stock, and where it comes from.
    let here = setups.iter().position(|x| x.0 == id).unwrap_or(0);
    if let Some(stock) = st.get_mut("stock") {
        section(ui, "Stock", true, |ui| {
            let modes = cam::ops::variants::<cam::setup::Stock>(json!({"mode": "\u{1}"}));
            let axes = cam::ops::variants::<cam::setup::Axis>(Value::Null);
            tagged_with(ui, inputs, "stock", stock, "mode", &modes, &[("axis", axes)]);
            if stock["mode"] == "from_setup" {
                let from = stock["setup"].as_u64().unwrap_or(0);
                let source = if from == 0 { here.checked_sub(1).and_then(|i| setups.get(i)) } else { setups.iter().take(here).find(|x| x.0 == from) };
                let text = match source {
                    Some((_, n)) => format!("Starts from what `{n}` leaves after its operations (setup 0: the one before)."),
                    None => "There is no earlier setup to take the stock from.".to_string(),
                };
                ui.label(RichText::new(text).color(if source.is_some() { t.text_dim } else { t.error }));
            }
        });
    }
    // The origin and axes every operation of the setup is measured from.
    if let Some(Value::Object(w)) = st.get_mut("wcs") {
        section(ui, "Origin and axes", true, |ui| {
            for (key, label) in [("origin", "Origin"), ("x", "X axis"), ("y", "Y axis"), ("z", "Tool axis (Z)")] {
                if let Some(v) = w.get_mut(key) {
                    value(ui, inputs, &format!("wcs.{key}"), label, v, None);
                }
            }
            ui.label(
                RichText::new("Turn the part over to machine its other side; the origin moves to the stock's front-left top corner.")
                    .color(t.text_dim),
            );
            ui.horizontal(|ui| {
                if crate::ui_kit::button(ui, "Turn Over About X", crate::ui_kit::Button::Normal).clicked() {
                    flip = Some(json!({"flip": "x"}));
                }
                if crate::ui_kit::button(ui, "Turn Over About Y", crate::ui_kit::Button::Normal).clicked() {
                    flip = Some(json!({"flip": "y"}));
                }
            });
            // Put the origin somewhere: a point of the stock or the model, a point the designer
            // placed, or picked on the model (it snaps as the pointer moves).
            if let Some(extra) = origin_rows(ui, of, points) {
                flip = Some(extra);
            }
        });
    }
    // The machine: a preset, then its properties.
    if let Some(machine) = st.get_mut("machine") {
        section(ui, "Machine", false, |ui| {
            let presets = cam::machine::Machine::PRESETS;
            let mut labels: Vec<String> = vec![crate::i18n::tr_label("Keep this machine").to_string()];
            labels.extend(presets.iter().map(|p| kind_label(p)));
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let mut k = 0usize;
            crate::ui_kit::row(ui, "Preset", |ui| crate::ui_kit::dropdown(ui, "cam_ed_machine_preset", &mut k, &refs));
            if let Some(m) = k.checked_sub(1).and_then(|i| presets.get(i)).and_then(|p| cam::machine::Machine::preset(p)) {
                if let Ok(v) = serde_json::to_value(m) {
                    *machine = v;
                }
                inputs.text.retain(|key, _| !key.starts_with("machine."));
                inputs.invalid.retain(|key| !key.starts_with("machine."));
            }
            let Value::Object(mo) = machine else { return };
            let changers = cam::ops::variants::<cam::machine::ToolChanger>(json!({"kind": "\u{1}"}));
            let keys: Vec<String> = mo.keys().cloned().collect();
            for key in keys {
                let path = format!("machine.{key}");
                let Some(v) = mo.get_mut(&key) else { continue };
                match key.as_str() {
                    "tool_changer" => group(ui, "Tool changer", |ui| tagged(ui, inputs, &path, v, "kind", &changers)),
                    "laser" => {
                        let mut on = !v.is_null();
                        crate::ui_kit::checkbox(ui, "Laser module", &mut on).on_hover_text(path.as_str());
                        if on && v.is_null() {
                            *v = json!({"power_w": 2.5, "wavelength_nm": 445.0, "builtin": false, "spot_mm": 0.1});
                        } else if !on {
                            *v = Value::Null;
                        }
                        if v.is_object() {
                            value(ui, inputs, &path, "Laser", v, None);
                        }
                    }
                    _ => {
                        let label = match key.as_str() {
                            "spindle_rpm_min" => "Spindle min (rev/min)".to_string(),
                            "spindle_rpm_max" => "Spindle max (rev/min)".to_string(),
                            "spindle_power_w" => "Spindle power (W)".to_string(),
                            "max_feed" | "rapid_feed" => format!("{} (mm/min)", kind_label(&key)),
                            "rotary_rapid_deg_min" => "Rotary rapid (°/min)".to_string(),
                            "work_area" => "Work area (mm)".to_string(),
                            "collets" => "Collets (mm)".to_string(),
                            "rotary_capacity" => "Rotary capacity (mm)".to_string(),
                            _ => label_of(&key, v),
                        };
                        value(ui, inputs, &path, &label, v, None);
                    }
                }
            }
        });
    }
    // What is machined and what is avoided.
    section(ui, "Bodies", false, |ui| {
        for (key, label) in [("bodies", "Bodies to machine (none: all)"), ("fixtures", "Fixtures to avoid")] {
            if let Some(v) = st.get_mut(key) {
                value(ui, inputs, key, label, v, None);
            }
        }
    });
    flip
}

/// The sizes a tool kind starts with (a tool of `diameter` mm).
fn tool_kind_default(kind: &str, diameter: f64) -> Value {
    match kind {
        "bull_nose" => json!({"corner_radius": (diameter * 0.1).max(0.1)}),
        "chamfer" | "spot_drill" => json!({"tip_angle_deg": 90.0}),
        "drill" => json!({"tip_angle_deg": 118.0}),
        "laser" => json!({"wavelength_nm": 445.0}),
        "tapered_ball" => json!({"tip_radius": (diameter * 0.25).max(0.1), "taper_angle_deg": 5.0}),
        _ => json!({}),
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
    if path.ends_with("bodies") || path.ends_with("fixtures") || path.ends_with("collets") {
        return match last {
            Some(x) => x.clone(),
            None if path.ends_with("collets") => json!(3.175),
            None => json!(""),
        };
    }
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
    tagged_with(ui, inputs, path, v, tag, choices, &[]);
}

/// [`tagged`], with fixed choices for some of the chosen kind's fields.
fn tagged_with(ui: &mut egui::Ui, inputs: &mut Inputs, path: &str, v: &mut Value, tag: &str, choices: &[String], fields: &[(&str, Vec<String>)]) {
    let Value::Object(o) = v else { return };
    let current = o.get(tag).and_then(Value::as_str).unwrap_or("").to_string();
    let mut i = choices.iter().position(|c| *c == current).unwrap_or(0);
    let labels: Vec<String> = choices.iter().map(|c| kind_label(c)).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let label = match tag {
        "source" => "Source",
        "mode" => "Stock",
        _ => "Kind",
    };
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
            let list = fields.iter().find(|(k, _)| *k == key).map(|x| x.1.as_slice());
            value(ui, inputs, &p, &l, x, list);
        }
    }
}

/// What an operation's geometry picks, in a few words (`None`: nothing, it is the stock or the
/// model's silhouette).
fn geometry_picks(g: &Value) -> Option<String> {
    let count = |k: &str| g.get(k).and_then(Value::as_array).map_or(0, Vec::len);
    let n = match g.get("source").and_then(Value::as_str)? {
        "faces" => (count("points"), "face"),
        "holes" => (count("points"), "hole"),
        "loops" => (count("loops"), "loop"),
        "sketch" => return Some(format!("sketch {}", g["feature"].as_u64().unwrap_or(0))),
        _ => return None,
    };
    match n.0 {
        0 => None,
        1 => Some(format!("1 {}", n.1)),
        c => Some(format!("{c} {}s", n.1)),
    }
}

/// The origin's rows in the setup form; returns what to add to `cam.setup.set` when one of its
/// buttons is pressed.
fn origin_rows(ui: &mut egui::Ui, of: &mut OriginForm, points: &[String]) -> Option<Value> {
    let t = Tokens::get();
    let mut out = None;
    ui.add_space(t.space_8);
    ui.label(RichText::new("Place the origin").strong());
    crate::ui_kit::row(ui, "On", |ui| crate::ui_kit::dropdown(ui, "cam_ed_origin_of", &mut of.of, &["Stock", "Model"]));
    crate::ui_kit::row(ui, "X", |ui| crate::ui_kit::dropdown(ui, "cam_ed_origin_x", &mut of.x, &["Left", "Center", "Right"]));
    crate::ui_kit::row(ui, "Y", |ui| crate::ui_kit::dropdown(ui, "cam_ed_origin_y", &mut of.y, &["Front", "Center", "Back"]));
    crate::ui_kit::row(ui, "Z", |ui| crate::ui_kit::dropdown(ui, "cam_ed_origin_z", &mut of.z, &["Top", "Middle", "Bottom"]));
    let on = ["stock", "model"].get(of.of).copied().unwrap_or("stock");
    if crate::ui_kit::button(ui, "Place Origin", crate::ui_kit::Button::Normal)
        .on_hover_text("Moves the origin to that point of the stock's or the model's box, in the setup's axes")
        .clicked()
    {
        fn pick(i: usize, names: [&'static str; 3]) -> &'static str {
            names.get(i).copied().unwrap_or(names[0])
        }
        out = Some(json!({"origin_at": {
            "of": on,
            "x": pick(of.x, ["left", "center", "right"]),
            "y": pick(of.y, ["front", "center", "back"]),
            "z": pick(of.z, ["top", "middle", "bottom"]),
        }}));
    }
    // Points the designer placed for it.
    if !points.is_empty() {
        let refs: Vec<&str> = points.iter().map(String::as_str).collect();
        of.point = of.point.min(points.len() - 1);
        crate::ui_kit::row(ui, "Design point", |ui| crate::ui_kit::dropdown(ui, "cam_ed_origin_point", &mut of.point, &refs));
        if crate::ui_kit::button(ui, "Use Point", crate::ui_kit::Button::Normal).clicked()
            && let Some(name) = points.get(of.point)
        {
            out = Some(json!({"origin": {"point": name}}));
        }
    }
    // Picked on the model: it snaps to vertices, edge midpoints, circle and face centres.
    let label = if of.picking { "Stop Picking" } else { "Pick on Model" };
    if crate::ui_kit::button(ui, label, crate::ui_kit::Button::Normal).clicked() {
        of.picking = !of.picking;
        of.before = None;
        of.hover = None;
    }
    if of.picking {
        ui.label(
            RichText::new("Move over the model: the origin snaps to vertices, edge midpoints and circle and face centres. Click to place it; Esc stops.")
                .color(t.warning),
        );
    }
    out
}

/// While a setup's origin is being picked: the snap under the pointer, drawn over the model
/// (called by the viewport after the model is drawn).
pub fn overlay(app: &mut SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(mut ed) = app.cam_editor.take() else { return };
    if ed.origin.picking {
        let under = app.viewport.hover.as_ref().and_then(crate::viewport::hit_sel);
        match under {
            Some(sel) => {
                let fresh = !matches!(&ed.origin.hover, Some((s, _)) if *s == sel);
                if fresh {
                    ed.origin.hover = Some((sel.clone(), crate::dialogs_assembly::snap_of(&app.session, &sel)));
                }
                if let Some((_, Some(s))) = ed.origin.hover.as_ref() {
                    let t = Tokens::get();
                    crate::dialogs_assembly::glyph(painter, proj, s, t.accent, app.cam.half_height(), Some(crate::dialogs_assembly::snap_name(s.kind)));
                }
            }
            None => ed.origin.hover = None,
        }
    }
    app.cam_editor = Some(ed);
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
        ("mode", "relative_box") => json!({"x_neg": 1.0, "x_pos": 1.0, "y_neg": 1.0, "y_pos": 1.0, "z_neg": 0.0, "z_pos": 1.0}),
        ("mode", "fixed_box") => json!({"min": [0.0, 0.0, 0.0], "max": [100.0, 100.0, 20.0]}),
        ("mode", "relative_cylinder") => json!({"axis": "x", "radial": 1.0, "start": 0.0, "end": 0.0}),
        ("mode", "fixed_cylinder") => json!({"axis": "x", "radius": 10.0, "start": 0.0, "end": 50.0}),
        ("mode", "from_setup") => json!({"setup": 0}),
        ("kind", "automatic") => json!({"slots": 6}),
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
    let bare = ["_deg", "_s", "_mm", "_nm"].iter().find_map(|u| key.strip_suffix(u)).unwrap_or(key);
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
    } else if key.ends_with("_nm") {
        "nm"
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
