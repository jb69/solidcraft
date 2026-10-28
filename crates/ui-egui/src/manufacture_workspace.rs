//! The Manufacture workspace: the MANUFACTURE ribbon tab with the `cam.*` commands, a panel
//! that shows the manufacturing model (tools, setups, operations, what needs attention) and
//! the machining simulation. Like Render, it is a ribbon tab the design tabs give way to; the
//! viewport, history and selection stay the design's.

use egui::{Color32, RichText};
use serde_json::{Value, json};

use crate::SolveApp;
use crate::theme::Tokens;

pub const TAB: &str = "MANUFACTURE";

pub fn active(app: &SolveApp) -> bool {
    app.ui.tab == TAB
}

/// Enter (`on`) or leave the workspace. Leaving ends a simulation preview.
pub fn enter(app: &mut SolveApp, on: bool) -> Result<(), String> {
    if on {
        if app.session.active_sketch.is_some() || app.tool.is_some() || app.dialog.is_some() || app.file_load.busy() {
            return Err("finish the current modelling operation before entering Manufacture".into());
        }
        if crate::render_workspace::active(app) {
            crate::render_workspace::enter(app, false)?;
        }
        app.ui.tab = TAB.into();
    } else if active(app) {
        if app.session.cam.preview.is_some() {
            let _ = app.run("cam.simulate.hide", json!({}));
        }
        app.ui.tab = "MODEL".into();
    }
    app.cancel_view_animation();
    Ok(())
}

/// A simulation preview started from a script, the CLI or the agent (`cam.simulate.show`)
/// brings the workspace with it, so the simulation never plays over the design tabs.
pub fn sync(app: &mut SolveApp) {
    if !active(app)
        && app.session.cam.preview.is_some()
        && !crate::render_workspace::active(app)
        && app.session.active_sketch.is_none()
        && app.dialog.is_none()
        && app.tool.is_none()
    {
        app.ui.tab = TAB.into();
    }
}

/// A ribbon button for a `cam.*` command. The commands that need parameters have no dialog
/// yet: the button opens the command box with a filled-in call to edit and run. Returns false
/// when the command is not a CAM command (the caller goes on as usual).
pub fn start(app: &mut SolveApp, id: &str) -> bool {
    if !id.starts_with("cam.") {
        return false;
    }
    let setup = selected_setup(app);
    let m = &app.session.doc.manufacture;
    let tool = m.tools.tools.first().map(|t| t.id);
    let template = match id {
        "cam.tool.add" => Some(json!({"name": "6 mm flat", "kind": "flat", "diameter": 6, "flute_length": 20, "overall_length": 50})),
        "cam.setup.create" => Some(
            json!({"name": format!("Setup {}", m.setups.len() + 1), "machine": "carvera", "stock": {"mode": "relative_box", "x_neg": 1, "x_pos": 1, "y_neg": 1, "y_pos": 1, "z_neg": 0, "z_pos": 1}}),
        ),
        "cam.tool.kit" => Some(json!({"kit": "makera_essential", "material": "aluminium"})),
        "cam.machine.set" => Some(json!({"setup": setup, "machine": "carvera"})),
        "cam.operation.add" => {
            let tools: Vec<(u64, bool)> = m.tools.tools.iter().map(|t| (t.id, t.is_laser())).collect();
            let picked = crate::cam_editor::selection_geometry(app, "pocket3d").map(|x| x.0);
            Some(kind_template("pocket3d", setup, tool, &tools, picked))
        }
        "cam.operation.set" => Some(json!({"operation": null})),
        _ => {
            let tools: Vec<(u64, bool)> = m.tools.tools.iter().map(|t| (t.id, t.is_laser())).collect();
            id.strip_prefix("cam.operation.")
                .filter(|k| !matches!(*k, "add" | "remove" | "list" | "kinds"))
                .map(|k| {
                    let picked = crate::cam_editor::selection_geometry(app, if k == "tap" { "drill" } else { k }).map(|x| x.0);
                    kind_template(k, setup, tool, &tools, picked)
                })
        }
    };
    // A new operation opens in the operation editor (Fields and JSON), for the setup picked.
    let new_kind = match id.strip_prefix("cam.operation.") {
        Some("add") => Some("pocket3d"),
        Some("tap") => Some("drill"),
        Some("set" | "remove" | "list" | "kinds" | "move" | "duplicate") | None => None,
        Some(k) => Some(k),
    };
    if let (Some(call), Some(setup), Some(kind)) = (&template, setup, new_kind)
        && crate::cam_editor::open_new(app, setup, kind, call)
    {
        return true;
    }
    if let Some(template) = template {
        crate::shortcut_box::open(app, None);
        app.sbox.text = format!("{id} {}", serde_json::to_string(&template).unwrap_or_default());
        return true;
    }
    if id == "cam.post" {
        let name =
            app.session.doc.manufacture.setups.first().map(|s| s.program.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| "program".into());
        app.pick_save_command(&format!("{name}.nc"), &["nc", "gcode", "ngc", "tap"], "cam.post", json!({}));
        return true;
    }
    if id == "cam.generate" {
        crate::cam_job::start(app, json!({}));
        return true;
    }
    if id == "cam.simulate.show" && app.session.cam.preview.is_some() {
        // Already showing: the panel drives it.
        return true;
    }
    match app.run(id, json!({})) {
        Ok(v) => {
            if let Some(info) = v.get("notice").and_then(|n| n.get("info")).and_then(Value::as_str) {
                app.set_status(info.to_string(), v["notice"]["status"] == "error");
            }
        }
        Err(e) => app.set_status(e, true),
    }
    true
}

/// The setup new operations go into: the one picked in the Browser (or the picked operation's),
/// else the first one.
pub fn selected_setup(app: &SolveApp) -> Option<u64> {
    use crate::browser::cam::CamPick;
    let m = &app.session.doc.manufacture;
    let picked = match app.tree.picked_cam {
        Some(CamPick::Setup(s)) => Some(s),
        Some(CamPick::Operation(o)) => m.operation(o).map(|(st, _)| st.id),
        _ => None,
    };
    picked.filter(|s| m.setup(*s).is_some()).or_else(|| m.setups.first().map(|s| s.id))
}

/// A filled-in `cam.operation.<kind>` call: the kind's parameters at their defaults and the
/// geometry the kind reads (`picked`: from the viewport's selection, when something is selected),
/// for the command box.
fn kind_template(kind: &str, setup: Option<u64>, tool: Option<u64>, tools: &[(u64, bool)], picked: Option<Value>) -> Value {
    use solidcraft_engine::cam::ops::OpKind;
    let (kind_name, extra): (&str, Value) = match kind {
        "tap" => ("drill", json!({"cycle": "tap", "pitch": 1.0})),
        other => (other, json!({})),
    };
    let mut params =
        serde_json::from_value::<OpKind>(json!({"type": kind_name})).ok().and_then(|k| serde_json::to_value(k).ok()).unwrap_or(json!({}));
    if let Value::Object(o) = &mut params {
        o.remove("type");
        if let Value::Object(e) = extra {
            for (k, v) in e {
                o.insert(k, v);
            }
        }
    }
    let geometry = match kind_name {
        "face" => json!({"source": "stock"}),
        "pocket2d" | "adaptive2d" | "chamfer2d" => json!({"source": "faces", "points": [[0, 0, 0]]}),
        "contour2d" | "project" => json!({"source": "silhouette"}),
        "slot" | "trace" | "engrave" => json!({"source": "sketch", "feature": 0}),
        "drill" | "bore" | "circular" | "thread" => json!({"source": "holes", "points": [[0, 0, 0]]}),
        "swarf" => json!({"source": "chains", "bottom": [[0, 0, 0], [10, 0, 0]], "top": [[0, 0, 10], [10, 0, 10]]}),
        "flow" | "morph" => json!({"source": "rails", "first": [[0, 0, 0], [10, 0, 0]], "second": [[0, 10, 0], [10, 10, 0]]}),
        "laser_cut" => json!({"source": "sketch", "feature": 0}),
        "laser_engrave" => json!({"source": "faces", "points": [[0, 0, 0]]}),
        _ => Value::Null,
    };
    // Something selected: the operation works on it (and an area operation machines within it).
    let bounded = serde_json::from_value::<OpKind>(json!({"type": kind_name})).is_ok_and(|k| k.uses_boundary());
    let geometry = match picked {
        Some(g) if !geometry.is_null() || bounded => g,
        _ => geometry,
    };
    // Laser work wants the laser tool, turning a vertical cutter: pick the library's first of
    // the kind when there is one.
    let tool = match kind_name {
        "laser_cut" | "laser_engrave" => tools.iter().find(|t| t.1).map(|t| t.0).or(tool),
        _ => tools.iter().find(|t| !t.1).map(|t| t.0).or(tool),
    };
    let mut call = json!({"setup": setup, "tool": tool, "name": kind_name});
    if let (Value::Object(c), Value::Object(p)) = (&mut call, params) {
        for (k, v) in p {
            c.insert(k, v);
        }
        if !geometry.is_null() {
            c.insert("geometry".into(), geometry);
        }
    }
    call
}

fn status_color(t: &Tokens, status: &str) -> Color32 {
    match status {
        "ok" => t.agent_ok,
        "warning" => t.warning,
        "error" => t.error,
        _ => t.text_dim,
    }
}

/// The right-hand panel: the manufacturing model as it stands.
pub fn panel(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let mut run: Option<&'static str> = None;
    egui::Panel::right("sc_manufacture")
        .exact_size(285.0)
        .resizable(false)
        .frame(egui::Frame::NONE.fill(t.panel).inner_margin(10))
        .show(ui, |ui| {
            crate::ui_kit::scope(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading(crate::i18n::tr_label("Manufacture"));
                    let s = &app.session;
                    let m = &s.doc.manufacture;
                    if m.tools.tools.is_empty() && m.setups.is_empty() {
                        ui.add_space(t.space_8);
                        ui.label(RichText::new(crate::i18n::tr_label("No tools or setups yet.")).color(t.text_dim));
                        ui.small(crate::i18n::tr_label(
                            "Add a tool and create a setup, add operations, then generate, simulate and post. Every button opens its command filled in; the command box, scripts and the agent run the same cam.* commands.",
                        ));
                        return;
                    }
                    ui.add_space(t.space_8);
                    ui.label(RichText::new(crate::i18n::tr_label("Tools")).strong());
                    for tool in &m.tools.tools {
                        let kind = serde_json::to_value(tool.kind).ok().and_then(|v| v["kind"].as_str().map(str::to_string)).unwrap_or_default();
                        ui.label(format!("T{}  {}  Ø{} {}", tool.number, tool.name, tool.diameter, kind.replace('_', " ")));
                    }
                    for st in &m.setups {
                        ui.add_space(t.space_8);
                        ui.label(RichText::new(format!("{}  ·  {} ({} axes)", st.name, st.machine.name, st.machine.axis_count())).strong());
                        if st.operations.is_empty() {
                            ui.label(RichText::new(crate::i18n::tr_label("No operations.")).color(t.text_dim));
                        }
                        for op in &st.operations {
                            let made = s.cam.operations.get(&op.id);
                            let state = crate::browser::cam::op_state(app, op.id);
                            let (status, stale) = match made {
                                Some(g) => (g.notice.status(), state == "stale"),
                                None if op.enabled => ("not generated", false),
                                None => ("disabled", false),
                            };
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("●").color(status_color(&t, status)));
                                ui.label(format!("{}  {}", op.name, op.kind.type_name()));
                                let word = if stale { "stale" } else { status };
                                ui.label(RichText::new(crate::i18n::tr_label(word)).color(t.text_dim).small());
                            });
                            if let Some(g) = made {
                                for issue in g.notice.issues.iter().filter(|i| i.severity != solidcraft_engine::cam::notice::Severity::Info) {
                                    ui.indent(("issue", op.id, &issue.code), |ui| {
                                        ui.label(RichText::new(&issue.message).color(status_color(&t, &format!("{:?}", issue.severity).to_lowercase())).small());
                                    });
                                }
                            }
                        }
                    }
                    ui.add_space(t.space_12);
                    ui.horizontal_wrapped(|ui| {
                        for (label, id) in [("Generate", "cam.generate"), ("Simulate", "cam.simulate.show"), ("Post", "cam.post")] {
                            if crate::ui_kit::button(ui, crate::i18n::tr_label(label), crate::ui_kit::Button::Small).clicked() {
                                run = Some(id);
                            }
                        }
                    });
                    if let Some(p) = &s.cam.preview {
                        ui.add_space(t.space_8);
                        ui.small(format!("{}: {:.0} %", crate::i18n::tr_label("Simulation"), p.progress * 100.0));
                    }
                });
            });
        });
    if let Some(id) = run {
        app.start(id);
    }
}
