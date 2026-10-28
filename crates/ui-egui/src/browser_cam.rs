//! The Manufacture folder of the browser: each setup with its operations in order, as
//! parametric steps (each depends on the ones before it: rest machining sees their stock, so
//! editing, suppressing or moving one makes it and every later one stale), and the tool library.
//! Rows show each operation's state; the eye suppresses; double-click edits; the menu
//! regenerates, simulates to the operation, reorders, duplicates and deletes.

use egui::Color32;
use serde_json::{Value, json};

use super::{Actions, Row, draw_row, is_open, toggle};
use crate::SolveApp;
use crate::context_menu::{Item, Target};
use crate::theme::Tokens;
use solidcraft_engine::cam;

/// An operation's state for its row: `ok`, `warning`, `error`, `stale`, `not generated` or
/// `suppressed`.
pub(crate) fn op_state(app: &SolveApp, op: u64) -> &'static str {
    if let Some(job) = crate::cam_job::state(app, op) {
        return job;
    }
    let s = &app.session;
    let Some((st, o)) = s.doc.manufacture.operation(op) else { return "not generated" };
    if !o.enabled {
        return "suppressed";
    }
    let Some(g) = s.cam.operations.get(&op) else { return "not generated" };
    let sig = solidcraft_engine::cmd::cam::signatures(s, st).into_iter().find(|(id, _)| *id == op).map_or(0, |x| x.1);
    if sig != g.signature {
        return "stale";
    }
    g.notice.status()
}

fn state_color(t: &Tokens, state: &str) -> Option<Color32> {
    match state {
        "error" => Some(t.error),
        "warning" | "stale" => Some(t.warning),
        "generating" => Some(t.accent),
        _ => None,
    }
}

/// While an operation (or any of a setup's) is being generated: a spinning arc at the row's
/// right end, and frames kept coming; a queued one gets a still, faint ring.
fn row_activity(ui: &egui::Ui, rect: Option<egui::Rect>, generating: bool, queued: bool) {
    let Some(r) = rect else { return };
    if !generating && !queued {
        return;
    }
    let t = Tokens::get();
    let c = egui::pos2(r.right() - 12.0, r.center().y);
    if generating {
        let time = ui.input(|i| i.time);
        crate::cam_job::paint_spinner(ui.painter(), c, 5.0, time, t.accent);
        ui.ctx().request_repaint();
    } else {
        ui.painter().circle_stroke(c, 4.0, egui::Stroke::new(1.2, t.text_dim.gamma_multiply(0.6)));
    }
}

/// The Manufacture folder (only when the design has tools or setups).
pub(super) fn rows(app: &mut SolveApp, ui: &mut egui::Ui, depth: usize, acts: &mut Actions) {
    if app.session.doc.manufacture.is_empty() {
        return;
    }
    let t = Tokens::get();
    let key = "manufacture";
    let open = is_open(app, key, true);
    let r = draw_row(ui, ui.id().with(key), &Row { depth, fold: Some(open), icon: "cam", label: "Manufacture", ..Default::default() });
    row_activity(ui, r.rect, app.cam_job.as_ref().is_some_and(|j| j.current.is_some()), false);
    if r.fold || r.clicked {
        toggle(app, key, true);
    }
    if r.double {
        let _ = crate::manufacture_workspace::enter(app, true);
    }
    if !open {
        return;
    }
    let m = &app.session.doc.manufacture;
    let tool_of = |id: u64| m.tools.get(id).map_or_else(|| format!("tool {id} (missing)"), |t| format!("T{} {}", t.number, t.name));
    let setups: Vec<(u64, String, String, String, Vec<(u64, String, &'static str, String)>)> = m
        .setups
        .iter()
        .enumerate()
        .map(|(i, st)| {
            // Where the stock comes from, for the row's hover text.
            let stock = match st.stock {
                cam::setup::Stock::FromSetup { setup } => {
                    let from =
                        if setup == 0 { i.checked_sub(1).and_then(|k| m.setups.get(k)) } else { m.setups.iter().take(i).find(|x| x.id == setup) };
                    from.map_or_else(|| "stock: from an earlier setup (none)".to_string(), |f| format!("stock: what `{}` leaves", f.name))
                }
                _ => "stock: its own".to_string(),
            };
            let ops = st.operations.iter().map(|o| (o.id, o.name.clone(), o.kind.type_name(), tool_of(o.tool))).collect();
            (st.id, st.name.clone(), st.machine.name.clone(), stock, ops)
        })
        .collect();
    for (sid, name, machine, stock, ops) in setups {
        let skey = format!("cam-setup-{sid}");
        let sopen = is_open(app, &skey, true);
        let label = format!("{name} · {machine}");
        let picked = app.tree.picked_cam == Some(CamPick::Setup(sid));
        let any_shown = ops.iter().any(|(oid, ..)| !app.session.cam.hidden.contains(oid));
        let r = draw_row(
            ui,
            ui.id().with(("cam-setup", sid)),
            &Row {
                depth: depth + 1,
                fold: Some(sopen),
                eye: (!ops.is_empty()).then_some(any_shown),
                icon: "folder",
                label: &label,
                name: true,
                selected: picked,
                dim: !any_shown && !ops.is_empty(),
                ..Default::default()
            },
        );
        if r.eye {
            let _ = app.run("cam.toolpath.visibility", json!({"setup": sid, "visible": !any_shown}));
        }
        let busy = ops.iter().any(|(oid, ..)| crate::cam_job::state(app, *oid) == Some("generating"));
        row_activity(ui, r.rect, busy, false);
        if r.fold {
            toggle(app, &skey, true);
        }
        if let Some(resp) = &r.resp {
            resp.clone().on_hover_text(stock);
        }
        if r.clicked {
            app.tree.picked_cam = Some(CamPick::Setup(sid));
        }
        if r.double {
            edit_setup(app, sid);
        }
        if let Some(p) = r.secondary {
            app.tree.picked_cam = Some(CamPick::Setup(sid));
            acts.menu = Some((p, Target::CamSetup { id: sid }));
        }
        if !sopen {
            continue;
        }
        // The chain: each operation numbered in order.
        for (k, (oid, oname, kind, tool)) in ops.iter().enumerate() {
            let state = op_state(app, *oid);
            let hidden = app.session.cam.hidden.contains(oid);
            let label = format!("{}. {oname}", k + 1);
            let picked = app.tree.picked_cam == Some(CamPick::Operation(*oid));
            let r = draw_row(
                ui,
                ui.id().with(("cam-op", *oid)),
                &Row {
                    depth: depth + 2,
                    // The eye shows or hides the toolpath on the workpiece; Suppress is in the menu.
                    eye: Some(!hidden),
                    icon: "cam",
                    label: &label,
                    name: true,
                    selected: picked,
                    dim: state == "suppressed",
                    color: state_color(&t, state),
                    ..Default::default()
                },
            );
            row_activity(ui, r.rect, state == "generating", state == "queued");
            if let Some(resp) = &r.resp {
                resp.clone().on_hover_text(format!("{kind} with {tool}: {state}"));
            }
            if r.eye {
                let _ = app.run("cam.toolpath.visibility", json!({"operation": oid, "visible": hidden}));
            }
            if r.clicked {
                app.tree.picked_cam = Some(CamPick::Operation(*oid));
            }
            if r.double {
                edit(app, *oid);
            }
            if let Some(p) = r.secondary {
                app.tree.picked_cam = Some(CamPick::Operation(*oid));
                acts.menu = Some((p, Target::CamOperation { id: *oid }));
            }
        }
    }
    // The tool library.
    let tkey = "cam-tools";
    let topen = is_open(app, tkey, false);
    let tools: Vec<(u64, String)> = app.session.doc.manufacture.tools.tools.iter().map(|t| (t.id, format!("T{} {}", t.number, t.name))).collect();
    let r = draw_row(ui, ui.id().with(tkey), &Row { depth: depth + 1, fold: Some(topen), icon: "folder", label: "Tools", ..Default::default() });
    if r.fold || r.clicked {
        toggle(app, tkey, false);
    }
    if topen {
        for (id, label) in tools {
            let picked = app.tree.picked_cam == Some(CamPick::Tool(id));
            let r = draw_row(
                ui,
                ui.id().with(("cam-tool", id)),
                &Row { depth: depth + 2, icon: "cam", label: &label, name: true, selected: picked, ..Default::default() },
            );
            if r.clicked {
                app.tree.picked_cam = Some(CamPick::Tool(id));
            }
            if r.double {
                edit_tool(app, id);
            }
            if let Some(p) = r.secondary {
                app.tree.picked_cam = Some(CamPick::Tool(id));
                acts.menu = Some((p, Target::CamTool { id }));
            }
        }
    }
}

/// What is picked in the Manufacture folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CamPick {
    Setup(u64),
    Operation(u64),
    Tool(u64),
}

/// Opens the command box with `cam.operation.set` and the operation's current parameters.
/// Opens the operation editor (Fields and JSON tabs) on `op`.
pub(crate) fn edit(app: &mut SolveApp, op: u64) {
    crate::cam_editor::open(app, crate::cam_editor::Target::Operation(op));
}

/// Opens the same editor on a setup: its stock, origin and axes, and machine.
pub(crate) fn edit_setup(app: &mut SolveApp, setup: u64) {
    crate::cam_editor::open(app, crate::cam_editor::Target::Setup(setup));
}

/// Opens the same editor on a tool.
pub(crate) fn edit_tool(app: &mut SolveApp, tool: u64) {
    crate::cam_editor::open(app, crate::cam_editor::Target::Tool(tool));
}

/// The menu of an operation.
pub fn operation_items(app: &SolveApp, id: u64) -> Vec<Item> {
    let state = op_state(app, id);
    let (setup, index, count) = app
        .session
        .doc
        .manufacture
        .setups
        .iter()
        .find_map(|st| st.operations.iter().position(|o| o.id == id).map(|i| (st.id, i, st.operations.len())))
        .unwrap_or((0, 0, 0));
    let suppressed = state == "suppressed";
    vec![
        Item::action("ui.camEdit", "Edit", "edit").with(json!({"operation": id})),
        Item::action("cam.generate", "Regenerate", "cam").with(json!({"operation": id})),
        Item::action("cam.generate", "Regenerate Chain", "cam").with(json!({"operation": id, "chain": true})),
        Item::action("cam.simulate.show", "Simulate to Here", "play").with(json!({"setup": setup, "operation": id, "move": 1_000_000_000u64})),
        Item::sep(),
        Item::action("cam.operation.move", "Move Up", "").with(json!({"operation": id, "up": true})).on(index > 0),
        Item::action("cam.operation.move", "Move Down", "").with(json!({"operation": id, "down": true})).on(index + 1 < count),
        Item::action("cam.operation.duplicate", "Duplicate", "copy").with(json!({"operation": id})),
        Item::sep(),
        Item::action("cam.toolpath.visibility", "Show/Hide Toolpath", "eye")
            .with(json!({"operation": id, "visible": app.session.cam.hidden.contains(&id)})),
        Item::action("cam.operation.set", if suppressed { "Unsuppress" } else { "Suppress" }, "suppress")
            .with(json!({"operation": id, "enabled": suppressed})),
        Item::action("cam.operation.remove", "Delete", "delete").with(json!({"operation": id})),
    ]
}

/// The menu of a setup.
pub fn setup_items(_app: &SolveApp, id: u64) -> Vec<Item> {
    vec![
        Item::action("ui.camEditSetup", "Edit", "edit").with(json!({"setup": id})),
        Item::action("cam.generate", "Generate", "cam").with(json!({"setup": id})),
        Item::action("cam.generate", "Regenerate All", "cam").with(json!({"setup": id, "all": true})),
        Item::action("cam.simulate.show", "Simulate", "play").with(json!({"setup": id, "progress": 0.0})),
        Item::action("ui.camPost", "Post Process", "export").with(json!({"setup": id})),
        Item::sep(),
        Item::action("cam.setup.remove", "Delete", "delete").with(json!({"setup": id})),
    ]
}

/// The menu of a tool.
pub fn tool_items(app: &SolveApp, id: u64) -> Vec<Item> {
    let used = app.session.doc.manufacture.tool_uses(id) > 0;
    vec![
        Item::action("ui.camEditTool", "Edit", "edit").with(json!({"tool": id})),
        Item::action("cam.tool.remove", "Delete", "delete").with(json!({"tool": id})).on(!used),
    ]
}

/// The `ui.cam*` menu actions.
pub fn run_action(app: &mut SolveApp, id: &str, p: &Value) {
    let num = |k: &str| p.get(k).and_then(Value::as_u64);
    match id {
        "ui.camEdit" => {
            if let Some(op) = num("operation") {
                edit(app, op);
            }
        }
        "ui.camEditSetup" => {
            if let Some(st) = num("setup") {
                edit_setup(app, st);
            }
        }
        "ui.camEditTool" => {
            if let Some(t) = num("tool") {
                edit_tool(app, t);
            }
        }
        "ui.camPost" => {
            let name = num("setup")
                .and_then(|s| app.session.doc.manufacture.setup(s))
                .map(|st| if st.program.is_empty() { st.name.clone() } else { st.program.clone() })
                .unwrap_or_else(|| "program".into());
            let ext = num("setup")
                .and_then(|s| app.session.doc.manufacture.setup(s))
                .map(|st| if st.machine.post == "makera" { "cnc" } else { "nc" })
                .unwrap_or("nc");
            app.pick_save_command(&format!("{name}.{ext}"), &["cnc", "nc", "gcode", "ngc", "tap"], "cam.post", p.clone());
        }
        _ => {}
    }
}
