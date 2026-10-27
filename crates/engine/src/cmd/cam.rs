//! Manufacturing (CAM): tools, setups, operations, toolpath generation, stock simulation and
//! G-code posting, as `cam.*` commands. The design stores the manufacturing model
//! (`Document::manufacture`); toolpaths live in the session (`Session::cam`) and are recomputed
//! by `cam.generate`.
//!
//! Written for an AI assistant first: every command takes plain JSON, answers with the numbers
//! it computed and a `notice` (`status`, `info`, `values`, `issues` with stable codes and
//! suggestions), and `cam.operation.kinds` and `cam.inspect` describe what exists so an agent
//! never has to guess parameter names.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_cam as cam;
use solidcraft_cam::frame::Frame;
use solidcraft_cam::notice::{Issue, Notice, r3};
use solidcraft_cam::ops::{Context, OpKind, ResolvedGeometry};
use solidcraft_cam::setup::{Geometry, Operation, Setup, Stock};
use solidcraft_cam::tool::{Tool, ToolKind};
use solidcraft_cam::toolpath::{MoveKind, Toolpath, ToolpathStats};
use solidcraft_geom::{Aabb2, Aabb3, Mesh, Vec2, Vec3};

use super::CommandSpec;
use crate::{
    EngineError, Result, Session,
    params::{bad, bool_, num, str_, vec3},
};

/// One generated operation: its toolpath in the setup's WCS, its notice and when it was made.
pub struct Generated {
    pub toolpath: Arc<Toolpath>,
    pub notice: Notice,
    pub stats: ToolpathStats,
    /// The operation frame relative to the WCS.
    pub frame: Frame,
    pub setup: u64,
    /// The session's geometry revision the toolpath was generated at (a later one may be stale).
    pub revision: u64,
}

/// What `cam.generate` left in the session, and the simulation preview the viewport draws.
#[derive(Default)]
pub struct CamCache {
    pub operations: BTreeMap<u64, Arc<Generated>>,
    /// The stepping simulator of one setup (built on the first `cam.simulate.show`; rebuilt when
    /// the toolpaths change).
    pub simulator: Option<SimulatorState>,
    /// What the viewport shows: the stock so far, the tool, the path.
    pub preview: Option<Arc<CamPreview>>,
}

pub struct SimulatorState {
    pub setup: u64,
    /// Operation ids and their toolpath pointers, to notice regenerated toolpaths.
    pub stamp: Vec<(u64, usize)>,
    pub sim: Box<cam::simulate::Simulator>,
}

/// The simulation at one moment, in design coordinates.
pub struct CamPreview {
    pub setup: u64,
    pub bodies: Vec<String>,
    pub stock: Arc<Mesh>,
    pub tool: Arc<Mesh>,
    /// Cutting moves already made and still to come, as polylines.
    pub path_done: Vec<Vec<Vec3>>,
    pub path_todo: Vec<Vec<Vec3>>,
    pub tip: Vec3,
    pub axis: Vec3,
    pub progress: f64,
    pub time_s: f64,
    pub total_time_s: f64,
    pub operation: Option<(u64, String)>,
    pub move_index: usize,
    pub total_moves: usize,
    /// Hide the machined bodies while the preview shows (the stock stands in for them).
    pub hide_model: bool,
}

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("cam.tool.add", "Add Tool", tool_add).at("MANUFACTURE", "SETUP")
        .icon("cam")
        .params("name; kind: flat | ball | bull | chamfer | drill | spot | tapered_ball; diameter (mm); corner_radius? (bull); tip_angle_deg? (chamfer, drill, spot; default 90 / 118); tip_radius? and taper_angle_deg? (tapered_ball); number? (T word); flute_length?, overall_length?, shank_diameter?, holder_diameter?, flutes?; feeds?: {spindle_rpm, cutting, plunge, ramp, lead_in, lead_out, retract, clockwise, coolant: flood | mist | air | through_tool | off}: adds a tool to the design's library. Returns the tool with its id"),
    CommandSpec::new("cam.tool.kit", "Tool Kit", tool_kit)
        .at("MANUFACTURE", "SETUP")
        .icon("cam")
        .params("kit?: makera_essential (default); material?: aluminium | wood | plastic (default aluminium); machine?: preset (default the setup's machine, else carvera); setup?: id: adds a maker's tool set with feeds and speeds scaled to the machine (chip loads for the material, held within the spindle and feed limits). Returns the tools with their ids and a starting stepdown per diameter"),
    CommandSpec::new("cam.tool.set", "Edit Tool", tool_set).icon("cam").params("tool: id; any cam.tool.add field: changes a tool. Operations using it need cam.generate again"),
    CommandSpec::new("cam.tool.remove", "Remove Tool", tool_remove).icon("cam").params("tool: id: removes an unused tool"),
    CommandSpec::new("cam.tool.list", "Tools", tool_list).icon("cam").params("lists the tool library").noundo(),
    CommandSpec::new("cam.setup.create", "Create Setup", setup_create).at("MANUFACTURE", "SETUP")
        .icon("cam")
        .params("name?; bodies?: body names to machine (default all); fixtures?: body names to avoid; wcs?: {origin: [x,y,z], z: [x,y,z], x: [x,y,z]} in design coordinates (default the design's origin and axes); stock?: {mode: relative_box, x_neg, x_pos, y_neg, y_pos, z_neg, z_pos} | {mode: fixed_box, min, max} | {mode: relative_cylinder, axis: x|y|z, radial, start, end} | {mode: fixed_cylinder, axis, radius, start, end} (default a box 1 mm around the model, flush at the bottom); machine?: three_axis | four_axis_a | trunnion_ac | trunnion_bc | head_head_bc | carvera | carvera_4axis | carvera_air | carvera_air_4axis | z1 | z1_4axis or a machine object (cam.machine.list); program?: name for the post. Returns the setup with its id and the resolved stock"),
    CommandSpec::new("cam.setup.set", "Edit Setup", setup_set).icon("cam").params("setup: id; any cam.setup.create field: changes a setup"),
    CommandSpec::new("cam.setup.remove", "Remove Setup", setup_remove).icon("cam").params("setup: id: removes a setup and its operations"),
    CommandSpec::new("cam.setup.list", "Setups", setup_list).icon("cam").params("lists the setups with their operations").noundo(),
    CommandSpec::new("cam.machine.set", "Set Machine", machine_set).at("MANUFACTURE", "SETUP")
        .icon("cam")
        .params("setup: id; machine: a preset (cam.machine.list: three_axis … carvera, carvera_air, z1, each with a _4axis variant) or a full machine object; tcp?: bool (the control has tool-centre-point control); rapid_feed?; linearisation_tolerance?; max_rotary_step_deg?; limits?: {A: [min,max], C: [min,max]}: sets the setup's machine"),
    CommandSpec::new("cam.machine.list", "Machines", machine_list).icon("cam").params("lists the machine presets").noundo(),
    CommandSpec::new("cam.operation.add", "Add Operation", operation_add).at("MANUFACTURE", "OPERATIONS")
        .icon("cam")
        .params("setup: id; type: see cam.operation.kinds; tool: id; name?; geometry?: {source: stock} | {source: silhouette} | {source: loops, loops: [[[x,y],…],…]} | {source: sketch, feature} | {source: faces, points: [[x,y,z],…]} | {source: holes, points, diameter?} | {source: chains, bottom: [[x,y,z],…], top: […]} | {source: rails, first, second}; heights?: {clearance|retract|feed|top|bottom: {from: stock_top|stock_bottom|model_top|model_bottom|origin|absolute, offset} or a number (absolute)}; linking?: {ramp: {type: plunge|helix|zigzag|profile, angle_deg, diameter}, lead_in/lead_out: {radius, sweep_deg, distance}, stay_down_distance, minimum_retract}; tolerance? (0.01); stock_to_leave_radial?, stock_to_leave_axial?; rest_machining?; orientation?: {origin, z, x} relative to the WCS for 3+2 work; plus the kind's own parameters at the top level (stepover, stepdown, direction: climb|conventional|both, …). Returns the operation, with issues for unknown parameters"),
    CommandSpec::new("cam.operation.set", "Edit Operation", operation_set).icon("cam").params("operation: id; any cam.operation.add field (type included): changes an operation; cam.generate again afterwards"),
    CommandSpec::new("cam.operation.remove", "Remove Operation", operation_remove).icon("cam").params("operation: id"),
    CommandSpec::new("cam.operation.list", "Operations", operation_list).icon("cam").params("setup?: id: the operations with their status").noundo(),
    CommandSpec::new("cam.operation.kinds", "Operation Kinds", operation_kinds).at("MANUFACTURE", "OPERATIONS").icon("cam").params("type?: one kind: every operation kind with its parameters and defaults, the axes it needs and whether it reads the model").noundo(),
    CommandSpec::new("cam.generate", "Generate Toolpaths", generate).at("MANUFACTURE", "TOOLPATHS")
        .icon("cam")
        .params("setup?: id (default every setup); operation?: id (just that one): computes the toolpaths of the enabled operations in order, simulating the stock as it goes for rest machining. Returns each operation's notice (status, values such as time_s and cutting_length_mm, issues with codes such as no_toolpath, tool_too_large, tool_too_short, below_bottom, tilt_limit) and the setup totals")
        .noundo(),
    CommandSpec::new("cam.toolpath", "Toolpath", toolpath)
        .icon("cam")
        .params("operation: id; offset? (0); limit? (2000 moves); coordinates?: wcs | design (default wcs): the generated moves as [kind, x, y, z, feed] with [ax, ay, az] appended when the tool tilts, plus bounds and stats")
        .noundo(),
    CommandSpec::new("cam.simulate", "Simulate", simulate).at("MANUFACTURE", "TOOLPATHS")
        .icon("cam")
        .params("setup?: id (default the only setup); tolerance? (0.05): sweeps the generated toolpaths through a Z-map of the stock: material removed, rest material above the model, gouges, rapids through stock, shank, holder and fixture collisions, per operation and in total")
        .noundo(),
    CommandSpec::new("cam.post", "Post Process", post).at("MANUFACTURE", "TOOLPATHS")
        .icon("export")
        .params("setup?: id (default the only setup); post?: generic | grbl | fiveaxis | makera (default the machine's own post, else by its axes); path?: write the G-code here (.nc); program?: name; line_numbers?: increment; canned_cycles?: bool: writes the setup's program through the machine's kinematics. Returns the G-code (when under 2 MB), line count and notice")
        .noundo(),
    CommandSpec::new("cam.simulate.show", "Show Simulation", simulate_show).at("MANUFACTURE", "TOOLPATHS")
        .icon("play")
        .params("setup?: id (default the only setup); progress?: 0..1 of the program's time | time_s? | operation?: id with move?: index | step?: moves to advance from where the simulation stands; cell? (mm, default by the stock size); show_model?: bool (default false: the stock stands in for the bodies): sweeps the stock to that moment and shows it in the viewport with the tool where it is and the path so far. Returns where it stands (progress, time_s, operation, move, tip, axis) and the stock volume. cam.simulate.hide ends it")
        .noundo(),
    CommandSpec::new("cam.simulate.hide", "Hide Simulation", simulate_hide).icon("play").params("ends the simulation preview").noundo(),
    CommandSpec::new("cam.post.list", "Post Processors", post_list).icon("export").params("lists the post processors").noundo(),
    CommandSpec::new("cam.notices", "CAM Notices", notices).at("MANUFACTURE", "INSPECT").icon("cam").params("setup?: id: every operation's notice from the last cam.generate, with stale: true when the design changed since").noundo(),
    CommandSpec::new("cam.inspect", "Inspect Manufacturing", inspect).at("MANUFACTURE", "INSPECT").icon("cam").params("the manufacturing model in brief: tools, setups, operations, what is generated and what needs attention").noundo(),
];

fn engine_err(cmd: &str, e: cam::CamError) -> EngineError {
    match e {
        cam::CamError::Param(m) => bad(cmd, m),
        other => EngineError::Other(format!("{cmd}: {other}")),
    }
}

fn id_param(p: &Value, k: &str, cmd: &str) -> Result<u64> {
    p.get(k).and_then(Value::as_u64).ok_or_else(|| bad(cmd, format!("`{k}` must be an id")))
}

/// The `setup` parameter, or the design's only setup when it has just one (the toolbar's
/// buttons run without parameters).
fn setup_param(s: &Session, p: &Value, cmd: &str) -> Result<u64> {
    if let Some(id) = p.get("setup").and_then(Value::as_u64) {
        return Ok(id);
    }
    match s.doc.manufacture.setups.as_slice() {
        [] => Err(bad(cmd, "no setup yet: cam.setup.create first")),
        [only] => Ok(only.id),
        many => Err(bad(cmd, format!("`setup` must be an id (there are {} setups: see cam.setup.list)", many.len()))),
    }
}

fn merge_json(base: &mut Value, over: &Value, skip: &[&str]) {
    if let (Value::Object(b), Value::Object(o)) = (base, over) {
        for (k, v) in o {
            if !skip.contains(&k.as_str()) {
                b.insert(k.clone(), v.clone());
            }
        }
    }
}

fn tool_json(t: &Tool, uses: usize) -> Value {
    let mut v = serde_json::to_value(t).unwrap_or(Value::Null);
    v["uses"] = json!(uses);
    v
}

/// Reads the tool kind from `kind` and the related fields.
fn tool_kind(p: &Value, cmd: &str, current: Option<ToolKind>) -> Result<ToolKind> {
    let kind = match str_(p, "kind") {
        Some(k) => k.to_ascii_lowercase(),
        None => return current.ok_or_else(|| bad(cmd, "`kind` is required: flat | ball | bull | chamfer | drill | spot | tapered_ball")),
    };
    let tip = |default: f64| num(p, "tip_angle_deg").unwrap_or(default);
    Ok(match kind.as_str() {
        "flat" | "flat_end" | "endmill" | "end_mill" | "square" => ToolKind::FlatEnd,
        "ball" | "ball_end" | "ballnose" | "ball_nose" => ToolKind::BallEnd,
        "bull" | "bull_nose" | "bullnose" | "radius" => {
            ToolKind::BullNose { corner_radius: num(p, "corner_radius").ok_or_else(|| bad(cmd, "a bull-nose tool needs `corner_radius`"))? }
        }
        "chamfer" => ToolKind::Chamfer { tip_angle_deg: tip(90.0) },
        "drill" => ToolKind::Drill { tip_angle_deg: tip(118.0) },
        "spot" | "spot_drill" | "center_drill" | "centre_drill" => ToolKind::SpotDrill { tip_angle_deg: tip(90.0) },
        "tapered_ball" | "taper_ball" | "tapered" => ToolKind::TaperedBall {
            tip_radius: num(p, "tip_radius").ok_or_else(|| bad(cmd, "a tapered ball tool needs `tip_radius`"))?,
            taper_angle_deg: num(p, "taper_angle_deg").unwrap_or(5.0),
        },
        other => return Err(bad(cmd, format!("unknown tool kind `{other}`: flat | ball | bull | chamfer | drill | spot | tapered_ball"))),
    })
}

fn tool_add(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.tool.add";
    let name = str_(p, "name").filter(|n| !n.trim().is_empty()).ok_or_else(|| bad(cmd, "`name` is required"))?.trim().to_string();
    let diameter = num(p, "diameter").ok_or_else(|| bad(cmd, "`diameter` (mm) is required"))?;
    let kind = tool_kind(p, cmd, None)?;
    if s.doc.manufacture.tools.tools.len() >= 1000 {
        return Err(bad(cmd, "a design holds at most 1000 tools"));
    }
    let id = s.doc_mut().manufacture.next_id();
    let mut tool = Tool::new(id, &name, kind, diameter);
    tool.number = p.get("number").and_then(Value::as_u64).map_or(s.doc.manufacture.tools.tools.len() as u32 + 1, |n| n.min(9999) as u32).max(1);
    let mut v = serde_json::to_value(&tool).map_err(|e| EngineError::Other(e.to_string()))?;
    merge_json(&mut v, p, &["id", "kind", "name", "diameter", "number", "corner_radius", "tip_angle_deg", "tip_radius", "taper_angle_deg"]);
    let tool: Tool = serde_json::from_value(v).map_err(|e| bad(cmd, format!("tool fields: {e}")))?;
    tool.validate().map_err(|e| engine_err(cmd, e))?;
    let out = tool_json(&tool, 0);
    s.doc_mut().manufacture.tools.tools.push(tool);
    Ok(out)
}

fn tool_kit(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.tool.kit";
    let kit_id = str_(p, "kit").unwrap_or("makera_essential");
    let material = match str_(p, "material") {
        Some(m) => cam::kits::Material::parse(m).ok_or_else(|| bad(cmd, format!("unknown material `{m}`: aluminium | wood | plastic")))?,
        None => cam::kits::Material::Aluminium,
    };
    let machine = match p.get("machine") {
        Some(v) if !v.is_null() => machine_param(v, cmd, None)?,
        _ => {
            let setup = p.get("setup").and_then(Value::as_u64).and_then(|id| s.doc.manufacture.setup(id)).or_else(|| s.doc.manufacture.setups.first());
            setup.map(|st| st.machine.clone()).unwrap_or_else(cam::machine::Machine::makera_carvera)
        }
    };
    let tools = cam::kits::kit(kit_id, &machine, material).ok_or_else(|| bad(cmd, format!("unknown kit `{kit_id}`: {}", cam::kits::KITS.join(" | "))))?;
    if s.doc.manufacture.tools.tools.len() + tools.len() > 1000 {
        return Err(bad(cmd, "a design holds at most 1000 tools"));
    }
    let mut number = s.doc.manufacture.tools.tools.iter().map(|t| t.number).max().unwrap_or(0);
    let mut out = Vec::new();
    for mut tool in tools {
        tool.validate().map_err(|e| engine_err(cmd, e))?;
        tool.id = s.doc_mut().manufacture.next_id();
        number += 1;
        tool.number = number;
        out.push(tool_json(&tool, 0));
        s.doc_mut().manufacture.tools.tools.push(tool);
    }
    let mut notice = Notice::new(format!("{} tools added for the {} ({:?})", out.len(), machine.name, material));
    notice.value("spindle_rpm_max", machine.spindle_rpm_max);
    notice.value("max_feed_mm_min", machine.max_feed);
    notice.value("stepdown_per_diameter", material.stepdown_per_diameter());
    notice.issue(Issue::info(
        "starting_point",
        format!("feeds are starting points for a {} W spindle: begin with a stepdown of {} × the tool diameter and a stepover of half of it, then listen to the cut", machine.spindle_power_w, material.stepdown_per_diameter()),
    ));
    Ok(json!({"tools": out, "machine": machine.name, "material": material, "notice": notice.to_json()}))
}

fn tool_set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.tool.set";
    let id = id_param(p, "tool", cmd)?;
    let current = s.doc.manufacture.tools.get(id).cloned().ok_or_else(|| bad(cmd, format!("no tool {id}")))?;
    let kind = if p.get("kind").is_some() || p.get("corner_radius").is_some() || p.get("tip_angle_deg").is_some() {
        let mut merged = serde_json::to_value(&current).unwrap_or(Value::Null);
        merge_json(&mut merged, p, &["tool"]);
        tool_kind(&merged, cmd, Some(current.kind))?
    } else {
        current.kind
    };
    let mut v = serde_json::to_value(&Tool { kind, ..current.clone() }).map_err(|e| EngineError::Other(e.to_string()))?;
    merge_json(&mut v, p, &["id", "tool", "kind", "corner_radius", "tip_angle_deg", "tip_radius", "taper_angle_deg"]);
    let tool: Tool = serde_json::from_value(v).map_err(|e| bad(cmd, format!("tool fields: {e}")))?;
    tool.validate().map_err(|e| engine_err(cmd, e))?;
    let uses = s.doc.manufacture.tool_uses(id);
    let out = tool_json(&tool, uses);
    if let Some(t) = s.doc_mut().manufacture.tools.get_mut(id) {
        *t = tool;
    }
    Ok(out)
}

fn tool_remove(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.tool.remove";
    let id = id_param(p, "tool", cmd)?;
    let uses = s.doc.manufacture.tool_uses(id);
    if uses > 0 {
        return Err(bad(cmd, format!("tool {id} is used by {uses} operation(s); change or remove them first")));
    }
    s.doc_mut().manufacture.tools.remove(id).ok_or_else(|| bad(cmd, format!("no tool {id}")))?;
    Ok(json!({"removed": id}))
}

fn tool_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let m = &s.doc.manufacture;
    Ok(json!({"tools": m.tools.tools.iter().map(|t| tool_json(t, m.tool_uses(t.id))).collect::<Vec<_>>()}))
}

fn frame_param(p: &Value, k: &str, cmd: &str, default: Frame) -> Result<Frame> {
    let Some(f) = p.get(k) else { return Ok(default) };
    if f.is_null() {
        return Ok(default);
    }
    let origin = f.get("origin").and_then(vec3).unwrap_or(default.origin);
    let z = f.get("z").or_else(|| f.get("z_axis")).and_then(vec3).unwrap_or(default.z);
    let x =
        f.get("x").or_else(|| f.get("x_axis")).and_then(vec3).unwrap_or_else(|| if (z - default.z).len() < 1e-9 { default.x } else { z.any_perp() });
    Frame::new(origin, z, x).map_err(|e| engine_err(cmd, e))
}

fn machine_param(v: &Value, cmd: &str, current: Option<&cam::machine::Machine>) -> Result<cam::machine::Machine> {
    let mut m = match v {
        Value::String(name) => cam::machine::Machine::preset(name)
            .ok_or_else(|| bad(cmd, format!("unknown machine preset `{name}`: {}", cam::machine::Machine::PRESETS.join(" | "))))?,
        Value::Object(_) => {
            let mut base = serde_json::to_value(current.cloned().unwrap_or_default()).unwrap_or(Value::Null);
            if let Some(Value::String(preset)) = v.get("preset").or_else(|| v.get("kind_preset")) {
                base = serde_json::to_value(machine_param(&Value::String(preset.clone()), cmd, None)?).unwrap_or(Value::Null);
            }
            merge_json(&mut base, v, &["preset", "kind_preset", "limits"]);
            serde_json::from_value(base).map_err(|e| bad(cmd, format!("machine: {e}")))?
        }
        Value::Null => current.cloned().unwrap_or_default(),
        _ => return Err(bad(cmd, "`machine` is a preset name or an object")),
    };
    if let Some(Value::Object(limits)) = v.get("limits") {
        for r in match &mut m.kinematics {
            cam::machine::Kinematics::ThreeAxis => vec![],
            cam::machine::Kinematics::FourAxis { rotary } => vec![rotary],
            cam::machine::Kinematics::FiveAxis { primary, secondary } => vec![primary, secondary],
        } {
            if let Some(Value::Array(lim)) = limits.get(&r.name.to_string())
                && lim.len() == 2
                && let (Some(lo), Some(hi)) = (lim[0].as_f64(), lim[1].as_f64())
            {
                r.min_deg = lo;
                r.max_deg = hi;
            }
        }
    }
    m.validate().map_err(|e| engine_err(cmd, e))?;
    Ok(m)
}

fn stock_param(v: Option<&Value>, cmd: &str, current: Stock) -> Result<Stock> {
    match v {
        None | Some(Value::Null) => Ok(current),
        Some(v) => {
            let mut base = serde_json::to_value(&current).unwrap_or(Value::Null);
            if v.get("mode").is_some() && v.get("mode") != base.get("mode") {
                base = json!({});
            }
            merge_json(&mut base, v, &[]);
            serde_json::from_value(base)
                .map_err(|e| bad(cmd, format!("stock: {e} (modes: relative_box, fixed_box, relative_cylinder, fixed_cylinder)")))
        }
    }
}

fn setup_json(s: &Session, st: &Setup) -> Value {
    let bounds = setup_model_bounds(s, st);
    let stock = bounds.as_ref().and_then(|b| st.stock.resolve(b).ok());
    let ops: Vec<Value> = st.operations.iter().map(|o| operation_brief(s, st, o)).collect();
    json!({
        "id": st.id,
        "name": st.name,
        "bodies": st.bodies,
        "fixtures": st.fixtures,
        "wcs": st.wcs,
        "stock": st.stock,
        "stock_resolved": stock,
        "model_bounds_wcs": bounds.map(|b| json!({"min": b.min, "max": b.max})),
        "machine": {"name": st.machine.name, "axes": st.machine.axis_count(), "tcp": st.machine.tcp},
        "program": st.program,
        "operations": ops,
    })
}

fn setup_create(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.setup.create";
    if s.doc.manufacture.setups.len() >= 100 {
        return Err(bad(cmd, "a design holds at most 100 setups"));
    }
    let id = s.doc_mut().manufacture.next_id();
    let name = str_(p, "name")
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map_or_else(|| format!("Setup {}", s.doc.manufacture.setups.len() + 1), str::to_string);
    let bodies = crate::params::string_list(p, "bodies");
    let fixtures = crate::params::string_list(p, "fixtures");
    for b in bodies.iter().chain(&fixtures) {
        if s.model.state().body(b).is_none() {
            return Err(bad(cmd, format!("no body `{b}`")));
        }
    }
    let wcs = frame_param(p, "wcs", cmd, Frame::IDENTITY)?;
    let stock = stock_param(p.get("stock"), cmd, Stock::default())?;
    let machine = match p.get("machine") {
        Some(v) => machine_param(v, cmd, None)?,
        None => cam::machine::Machine::three_axis(),
    };
    let setup = Setup {
        id,
        name,
        bodies,
        fixtures,
        wcs,
        stock,
        machine,
        operations: Vec::new(),
        program: str_(p, "program").unwrap_or_default().to_string(),
    };
    let out = setup_json(s, &setup);
    s.doc_mut().manufacture.setups.push(setup);
    Ok(out)
}

fn setup_set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.setup.set";
    let id = id_param(p, "setup", cmd)?;
    let mut st = s.doc.manufacture.setup(id).cloned().ok_or_else(|| bad(cmd, format!("no setup {id}")))?;
    if let Some(n) = str_(p, "name") {
        st.name = n.trim().to_string();
    }
    if p.get("bodies").is_some() {
        st.bodies = crate::params::string_list(p, "bodies");
    }
    if p.get("fixtures").is_some() {
        st.fixtures = crate::params::string_list(p, "fixtures");
    }
    for b in st.bodies.iter().chain(&st.fixtures) {
        if s.model.state().body(b).is_none() {
            return Err(bad(cmd, format!("no body `{b}`")));
        }
    }
    st.wcs = frame_param(p, "wcs", cmd, st.wcs)?;
    st.stock = stock_param(p.get("stock"), cmd, st.stock.clone())?;
    if let Some(m) = p.get("machine") {
        st.machine = machine_param(m, cmd, Some(&st.machine))?;
    }
    if let Some(prog) = str_(p, "program") {
        st.program = prog.to_string();
    }
    let out = setup_json(s, &st);
    if let Some(slot) = s.doc_mut().manufacture.setup_mut(id) {
        *slot = st;
    }
    s.cam.operations.retain(|_, g| g.setup != id);
    Ok(out)
}

fn setup_remove(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.setup.remove";
    let id = id_param(p, "setup", cmd)?;
    let m = &mut s.doc_mut().manufacture;
    let i = m.setups.iter().position(|x| x.id == id).ok_or_else(|| bad(cmd, format!("no setup {id}")))?;
    let removed = m.setups.remove(i);
    s.cam.operations.retain(|_, g| g.setup != id);
    Ok(json!({"removed": id, "operations_removed": removed.operations.len()}))
}

fn setup_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let setups: Vec<Value> = s.doc.manufacture.setups.iter().map(|st| setup_json(s, st)).collect();
    Ok(json!({"setups": setups}))
}

fn machine_set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.machine.set";
    let id = id_param(p, "setup", cmd)?;
    let st = s.doc.manufacture.setup(id).cloned().ok_or_else(|| bad(cmd, format!("no setup {id}")))?;
    let mut m = match p.get("machine") {
        Some(v) => machine_param(v, cmd, Some(&st.machine))?,
        None => st.machine.clone(),
    };
    if let Some(t) = bool_(p, "tcp") {
        m.tcp = t;
    }
    if let Some(f) = num(p, "rapid_feed") {
        m.rapid_feed = f;
    }
    if let Some(t) = num(p, "linearisation_tolerance") {
        m.linearisation_tolerance = t;
    }
    if let Some(a) = num(p, "max_rotary_step_deg") {
        m.max_rotary_step_deg = a;
    }
    if p.get("limits").is_some() {
        let mut v = serde_json::to_value(&m).unwrap_or(Value::Null);
        v["limits"] = p["limits"].clone();
        m = machine_param(&v, cmd, Some(&m))?;
    }
    m.validate().map_err(|e| engine_err(cmd, e))?;
    let out = serde_json::to_value(&m).unwrap_or(Value::Null);
    if let Some(slot) = s.doc_mut().manufacture.setup_mut(id) {
        slot.machine = m;
    }
    Ok(out)
}

fn machine_list(_s: &mut Session, _p: &Value) -> Result<Value> {
    let presets: Vec<(&str, cam::machine::Machine)> = cam::machine::Machine::PRESETS.iter().filter_map(|id| cam::machine::Machine::preset(id).map(|m| (*id, m))).collect();
    Ok(
        json!({"machines": presets.iter().map(|(id, m)| json!({"preset": id, "name": m.name, "vendor": m.vendor, "model": m.model, "axes": m.axis_count(), "tcp": m.tcp, "post": m.post, "work_area_mm": m.work_area, "spindle_rpm_max": m.spindle_rpm_max, "spindle_power_w": m.spindle_power_w, "max_feed": m.max_feed, "tool_changer": m.tool_changer, "collets_mm": m.collets, "rotary_capacity_mm": m.rotary_capacity, "rotaries": m.rotaries().iter().map(|r| json!({"name": r.name.to_string(), "direction": r.direction, "on_table": r.on_table, "min_deg": r.min_deg, "max_deg": r.max_deg})).collect::<Vec<_>>(), "machine": m})).collect::<Vec<_>>(), "kits": cam::kits::KITS}),
    )
}

/// The common operation fields (not kind parameters).
const COMMON_KEYS: &[&str] = &[
    "setup",
    "operation",
    "type",
    "tool",
    "name",
    "enabled",
    "geometry",
    "heights",
    "linking",
    "tolerance",
    "stock_to_leave_radial",
    "stock_to_leave_axial",
    "rest_machining",
    "orientation",
    "id",
];

/// Builds an operation from params over `base` (an existing operation's JSON, or the defaults).
fn operation_from(p: &Value, cmd: &str, base: Value, setup: &Setup, tools: &cam::tool::ToolLibrary) -> Result<(Operation, Vec<Issue>)> {
    let mut v = base;
    // Kind: `type` plus every non-common key.
    let ty = str_(p, "type")
        .map(str::to_string)
        .or_else(|| v.get("type").and_then(Value::as_str).map(str::to_string))
        .ok_or_else(|| bad(cmd, "`type` is required (see cam.operation.kinds)"))?;
    let ty = ty.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    let ty = match ty.as_str() {
        "pocket" | "2d_pocket" | "pocket_2d" => "pocket2d".to_string(),
        "contour" | "2d_contour" | "contour_2d" | "profile" | "slot" | "trace" => "contour2d".to_string(),
        "adaptive" | "2d_adaptive" | "adaptive_2d" => "adaptive2d".to_string(),
        "chamfer" | "2d_chamfer" => "chamfer2d".to_string(),
        "3d_pocket" | "pocket_3d" | "adaptive_clearing" | "3d_adaptive" | "roughing" | "rough" => "pocket3d".to_string(),
        "3d_contour" | "contour_3d" | "waterline" => "contour3d".to_string(),
        "morph_spiral" | "morphedspiral" => "morphed_spiral".to_string(),
        "steep_and_shallow" | "steepshallow" => "steep_shallow".to_string(),
        "multiaxis_contour" | "multi_axis" | "5axis_contour" => "multi_axis_contour".to_string(),
        "rotary_parallel" | "wrap" | "4axis" => "rotary".to_string(),
        other => other.to_string(),
    };
    if !OpKind::NAMES.contains(&ty.as_str()) {
        let near = OpKind::NAMES.iter().min_by_key(|n| edit_distance(n, &ty)).copied().unwrap_or("pocket2d");
        return Err(bad(cmd, format!("unknown operation type `{ty}`; did you mean `{near}`? (cam.operation.kinds lists them)")));
    }
    // Start the kind from its defaults when the type changes, else from the existing fields.
    if v.get("type").and_then(Value::as_str) != Some(ty.as_str()) {
        let defaults: OpKind = serde_json::from_value(json!({"type": ty})).map_err(|e| bad(cmd, format!("{ty}: {e}")))?;
        let dv = serde_json::to_value(&defaults).unwrap_or(Value::Null);
        // Drop the old kind's parameters.
        if let (Value::Object(vo), Value::Object(d)) = (&mut v, &dv) {
            let old_kind_keys: Vec<String> = vo.keys().filter(|k| !COMMON_KEYS.contains(&k.as_str())).cloned().collect();
            for k in old_kind_keys {
                vo.remove(&k);
            }
            for (k, val) in d {
                vo.insert(k.clone(), val.clone());
            }
        }
    }
    let mut issues = Vec::new();
    // Known keys of this kind.
    let probe: OpKind = serde_json::from_value(json!({"type": ty})).map_err(|e| bad(cmd, format!("{ty}: {e}")))?;
    let known: Vec<String> = serde_json::to_value(&probe).ok().and_then(|k| k.as_object().map(|o| o.keys().cloned().collect())).unwrap_or_default();
    if let Value::Object(po) = p {
        for (k, val) in po {
            if COMMON_KEYS.contains(&k.as_str()) {
                continue;
            }
            if known.iter().any(|kk| kk == k) {
                v[k] = val.clone();
            } else {
                let near = known.iter().filter(|kk| *kk != "type").min_by_key(|kk| edit_distance(kk, k)).cloned();
                let mut issue = Issue::warning("unknown_parameter", format!("`{k}` is not a parameter of {ty}; it was ignored"));
                if let Some(n) = near {
                    issue = issue.suggest(json!({"parameter": n}));
                }
                issues.push(issue);
            }
        }
    }
    v["type"] = json!(ty);
    // Common fields.
    if let Some(t) = p.get("tool") {
        v["tool"] = t.clone();
    }
    if let Some(n) = str_(p, "name") {
        v["name"] = json!(n.trim());
    }
    if let Some(e) = bool_(p, "enabled") {
        v["enabled"] = json!(e);
    }
    if let Some(g) = p.get("geometry") {
        v["geometry"] = geometry_param(g, cmd)?;
    }
    if let Some(h) = p.get("heights") {
        v["heights"] = heights_param(h, v.get("heights").cloned().unwrap_or(Value::Null), cmd)?;
    }
    if let Some(l) = p.get("linking") {
        let mut base = v.get("linking").cloned().unwrap_or_else(|| serde_json::to_value(cam::linking::Linking::default()).unwrap_or(Value::Null));
        merge_json(&mut base, l, &[]);
        v["linking"] = base;
    }
    for k in ["tolerance", "stock_to_leave_radial", "stock_to_leave_axial", "rest_machining"] {
        if let Some(x) = p.get(k) {
            v[k] = x.clone();
        }
    }
    if let Some(o) = p.get("orientation") {
        v["orientation"] =
            if o.is_null() { Value::Null } else { serde_json::to_value(frame_param(p, "orientation", cmd, Frame::IDENTITY)?).unwrap_or(Value::Null) };
    }
    let op: Operation = serde_json::from_value(v).map_err(|e| bad(cmd, format!("operation fields: {e}")))?;
    let tool = tools.get(op.tool).ok_or_else(|| bad(cmd, format!("no tool {} (cam.tool.list)", op.tool)))?;
    if !(op.tolerance.is_finite() && op.tolerance > 0.0 && op.tolerance <= 10.0) {
        return Err(bad(cmd, "`tolerance` must be between 0 and 10 mm"));
    }
    if op.kind.axes() > setup.machine.axis_count() {
        issues.push(
            Issue::warning(
                "machine_axes",
                format!(
                    "{} needs {} axes but the setup's machine `{}` has {}; set a machine with cam.machine.set before posting",
                    op.kind.type_name(),
                    op.kind.axes(),
                    setup.machine.name,
                    setup.machine.axis_count()
                ),
            )
            .suggest(json!({"command": "cam.machine.set", "machine": if op.kind.axes() == 4 { "four_axis_a" } else { "trunnion_ac" }})),
        );
    }
    match (&op.kind, &tool.kind) {
        (OpKind::Drill { .. }, ToolKind::Drill { .. } | ToolKind::SpotDrill { .. }) => {}
        (OpKind::Drill { .. }, _) => issues.push(Issue::info("tool_kind", "drilling with an end mill: fine for spotting or short holes")),
        (OpKind::Chamfer2d { .. }, ToolKind::Chamfer { .. }) => {}
        (OpKind::Chamfer2d { .. }, _) => issues.push(Issue::error("tool_kind", "2D chamfer needs a chamfer mill")),
        (
            OpKind::Parallel { .. } | OpKind::Scallop { .. } | OpKind::Pencil { .. } | OpKind::MultiAxisContour { .. } | OpKind::Flow { .. },
            ToolKind::FlatEnd,
        ) => issues.push(Issue::info("tool_kind", "a flat end mill on a sloped surface leaves steps; a ball or bull-nose tool follows it")),
        _ => {}
    }
    Ok((op, issues))
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn geometry_param(g: &Value, cmd: &str) -> Result<Value> {
    let mut g = g.clone();
    if let Some(Value::String(src)) = g.get("source").cloned() {
        let norm = match src.as_str() {
            "stock" | "all" => "stock",
            "silhouette" | "model" | "outline" => "silhouette",
            "loops" | "chains_2d" | "polygons" => "loops",
            "sketch" => "sketch",
            "faces" | "face" => "faces",
            "holes" | "hole" | "points" => "holes",
            "chains" | "swarf" => "chains",
            "rails" | "flow" => "rails",
            other => {
                return Err(bad(
                    cmd,
                    format!("unknown geometry source `{other}`: stock | silhouette | loops | sketch | faces | holes | chains | rails"),
                ));
            }
        };
        g["source"] = json!(norm);
    }
    let geometry: Geometry = serde_json::from_value(g).map_err(|e| bad(cmd, format!("geometry: {e}")))?;
    if let Geometry::Loops { loops } = &geometry {
        for l in loops {
            if l.len() < 3 || l.len() > 100_000 || l.iter().any(|p| !p.is_finite()) {
                return Err(bad(cmd, "each loop needs 3 to 100000 finite [x, y] points"));
            }
        }
    }
    Ok(serde_json::to_value(&geometry).unwrap_or(Value::Null))
}

fn heights_param(h: &Value, base: Value, cmd: &str) -> Result<Value> {
    let mut out = if base.is_null() { serde_json::to_value(cam::linking::Heights::default()).unwrap_or(Value::Null) } else { base };
    let Value::Object(ho) = h else { return Err(bad(cmd, "`heights` is an object of clearance, retract, feed, top, bottom")) };
    for (k, v) in ho {
        if !["clearance", "retract", "feed", "top", "bottom"].contains(&k.as_str()) {
            return Err(bad(cmd, format!("unknown height `{k}`: clearance | retract | feed | top | bottom")));
        }
        match v {
            Value::Number(n) => out[k] = json!({"from": "absolute", "offset": n}),
            Value::Object(o) => {
                let from = o.get("from").and_then(Value::as_str).unwrap_or("stock_top");
                let offset = o.get("offset").and_then(Value::as_f64).unwrap_or(0.0);
                let _: cam::linking::HeightRef = serde_json::from_value(json!(from)).map_err(|_| bad(cmd, format!("height `{k}`: unknown reference `{from}`: stock_top | stock_bottom | model_top | model_bottom | origin | absolute | top_height")))?;
                out[k] = json!({"from": from, "offset": offset});
            }
            _ => return Err(bad(cmd, format!("height `{k}` is a number (absolute) or {{from, offset}}"))),
        }
    }
    Ok(out)
}

fn operation_brief(s: &Session, st: &Setup, o: &Operation) -> Value {
    let made = s.cam.operations.get(&o.id);
    let tool = s.doc.manufacture.tools.get(o.tool);
    json!({
        "id": o.id,
        "name": o.name,
        "type": o.kind.type_name(),
        "enabled": o.enabled,
        "tool": o.tool,
        "tool_name": tool.map(|t| t.name.clone()),
        "axes": o.kind.axes(),
        "setup": st.id,
        "generated": made.is_some(),
        "stale": made.is_some_and(|g| g.revision != s.geometry_revision),
        "status": made.map_or("not generated", |g| g.notice.status()),
    })
}

fn operation_json(s: &Session, st: &Setup, o: &Operation, issues: &[Issue]) -> Value {
    let mut v = serde_json::to_value(o).unwrap_or(Value::Null);
    v["setup"] = json!(st.id);
    v["axes"] = json!(o.kind.axes());
    v["needs_model"] = json!(o.kind.needs_model());
    let mut n = Notice::default();
    for i in issues {
        n.issue(i.clone());
    }
    if let Some(g) = s.cam.operations.get(&o.id) {
        v["generated"] = json!({"stale": g.revision != s.geometry_revision, "notice": g.notice.to_json()});
    }
    v["notice"] = n.to_json();
    v
}

fn operation_add(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.operation.add";
    let setup_id = id_param(p, "setup", cmd)?;
    let st = s.doc.manufacture.setup(setup_id).cloned().ok_or_else(|| bad(cmd, format!("no setup {setup_id} (cam.setup.create)")))?;
    if st.operations.len() >= 500 {
        return Err(bad(cmd, "a setup holds at most 500 operations"));
    }
    let ty = str_(p, "type").unwrap_or("pocket2d");
    let base = json!({
        "id": 0, "name": format!("{} {}", ty, st.operations.len() + 1), "enabled": true, "tool": p.get("tool").cloned().unwrap_or(Value::Null),
        "geometry": {"source": "stock"}, "heights": cam::linking::Heights::default(), "linking": cam::linking::Linking::default(),
        "tolerance": 0.01, "stock_to_leave_radial": 0.0, "stock_to_leave_axial": 0.0, "rest_machining": false, "type": "__new__",
    });
    let (mut op, issues) = operation_from(p, cmd, base, &st, &s.doc.manufacture.tools)?;
    op.id = s.doc_mut().manufacture.next_id();
    let out = operation_json(s, &st, &op, &issues);
    if let Some(slot) = s.doc_mut().manufacture.setup_mut(setup_id) {
        slot.operations.push(op);
    }
    Ok(out)
}

fn operation_set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.operation.set";
    let id = id_param(p, "operation", cmd)?;
    let (st, current) =
        s.doc.manufacture.operation(id).map(|(st, o)| (st.clone(), o.clone())).ok_or_else(|| bad(cmd, format!("no operation {id}")))?;
    let base = serde_json::to_value(&current).map_err(|e| EngineError::Other(e.to_string()))?;
    let (mut op, issues) = operation_from(p, cmd, base, &st, &s.doc.manufacture.tools)?;
    op.id = id;
    s.cam.operations.remove(&id);
    let out = operation_json(s, &st, &op, &issues);
    if let Some(slot) = s.doc_mut().manufacture.operation_mut(id) {
        *slot = op;
    }
    Ok(out)
}

fn operation_remove(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.operation.remove";
    let id = id_param(p, "operation", cmd)?;
    let m = &mut s.doc_mut().manufacture;
    for st in &mut m.setups {
        if let Some(i) = st.operations.iter().position(|o| o.id == id) {
            st.operations.remove(i);
            s.cam.operations.remove(&id);
            return Ok(json!({"removed": id}));
        }
    }
    Err(bad(cmd, format!("no operation {id}")))
}

fn operation_list(s: &mut Session, p: &Value) -> Result<Value> {
    let setup = p.get("setup").and_then(Value::as_u64);
    let ops: Vec<Value> = s
        .doc
        .manufacture
        .setups
        .iter()
        .filter(|st| setup.is_none_or(|id| st.id == id))
        .flat_map(|st| st.operations.iter().map(move |o| (st, o)))
        .map(|(st, o)| operation_brief(s, st, o))
        .collect();
    Ok(json!({"operations": ops}))
}

fn operation_kinds(_s: &mut Session, p: &Value) -> Result<Value> {
    let only = str_(p, "type");
    let mut kinds = Vec::new();
    for name in OpKind::NAMES {
        if only.is_some_and(|o| o != *name) {
            continue;
        }
        let Ok(k) = serde_json::from_value::<OpKind>(json!({"type": name})) else { continue };
        let mut params = serde_json::to_value(&k).unwrap_or(Value::Null);
        if let Value::Object(o) = &mut params {
            o.remove("type");
        }
        kinds.push(json!({
            "type": name,
            "axes": k.axes(),
            "needs_model": k.needs_model(),
            "parameters": params,
            "description": kind_description(name),
        }));
    }
    Ok(json!({
        "kinds": kinds,
        "common": {
            "tool": "tool id", "name": "string", "enabled": true,
            "geometry": "{source: stock | silhouette | loops | sketch | faces | holes | chains | rails, …}",
            "heights": "{clearance, retract, feed, top, bottom: {from, offset} | absolute number}",
            "linking": cam::linking::Linking::default(),
            "tolerance": 0.01, "stock_to_leave_radial": 0.0, "stock_to_leave_axial": 0.0, "rest_machining": false,
            "orientation": "{origin, z, x} relative to the WCS (3+2 work)",
        },
        "notes": ["a parameter at 0 means its default (40 % of the diameter for a roughing stepover, 10 % for finishing, half the diameter for a stepdown)", "direction: climb | conventional | both (both = zigzag, no retract between passes)"],
    }))
}

fn kind_description(name: &str) -> &'static str {
    match name {
        "face" => "clears the top of the stock with parallel passes (geometry: stock or loops)",
        "pocket2d" => "contour-parallel pocketing of closed loops; inner loops are islands (geometry: sketch, faces or loops)",
        "adaptive2d" => "engagement-controlled clearing: inside-out rings at the optimal load with trochoids where the cut would be full width",
        "contour2d" => "profiling along loops with the tool outside, inside or on them; roughing and finishing passes, tabs",
        "chamfer2d" => "a chamfer along loops with a chamfer mill",
        "drill" => "drilling cycles at holes (geometry: holes with points or cylindrical faces)",
        "bore" => "helical boring of round holes with an end mill",
        "pocket3d" => "Z-level roughing between the stock and the model surface, with flat-area levels and rest machining",
        "parallel" => "parallel finishing passes following the surface",
        "contour3d" => "waterline finishing: the surface's level contours at each stepdown",
        "scallop" => "constant stepover along the surface",
        "pencil" => "cleans inside corners where the tool touches two faces",
        "spiral" => "an outward spiral from a centre, on the surface",
        "radial" => "rays from a centre, on the surface",
        "morphed_spiral" => "a spiral morphed from the boundary's shape, on the surface",
        "project" => "projects loops or chains onto the surface",
        "horizontal" => "finds flat areas and clears each at its level",
        "steep_shallow" => "waterlines on steep areas, parallel passes on shallow ones",
        "rotary" => "continuous 4-axis: the tool points at the operation frame's x axis and sweeps the surface around it",
        "swarf" => "5-axis: the side of the tool along a wall between a bottom and a top chain (geometry: chains)",
        "multi_axis_contour" => "5-axis: parallel passes with the tool axis on the surface normal, leaned by lead and tilt",
        "flow" => "5-axis: passes morphed between two rails, surface-normal (geometry: rails)",
        "thread" => "thread milling (not built yet)",
        _ => "",
    }
}

/// The meshes of the setup's bodies (names, or every solid body not a fixture), tessellated to
/// `tol`, in design coordinates.
fn setup_meshes(s: &Session, st: &Setup, tol: f64, fixtures: bool) -> Result<Vec<(String, Arc<Mesh>)>> {
    let state = s.model.state();
    let mut out = Vec::new();
    for b in &state.bodies {
        let wanted = if fixtures {
            st.fixtures.iter().any(|n| n == &b.name)
        } else if st.bodies.is_empty() {
            !st.fixtures.iter().any(|n| n == &b.name)
        } else {
            st.bodies.iter().any(|n| n == &b.name)
        };
        if !wanted {
            continue;
        }
        let mesh = match b.body.tessellate(tol.max(0.002)) {
            Ok(m) => Arc::new(m),
            Err(_) => b.mesh(),
        };
        out.push((b.name.clone(), mesh));
    }
    Ok(out)
}

fn transform_mesh(m: &Mesh, f: impl Fn(Vec3) -> Vec3) -> Mesh {
    Mesh { positions: m.positions.iter().map(|p| f(*p)).collect(), triangles: m.triangles.clone(), ..Default::default() }
}

/// The setup's model bounds in its WCS.
fn setup_model_bounds(s: &Session, st: &Setup) -> Option<Aabb3> {
    let meshes = setup_meshes(s, st, 0.05, false).ok()?;
    let mut b = Aabb3::default();
    for (_, m) in &meshes {
        for p in &m.positions {
            b.add(st.wcs.to_local(*p));
        }
    }
    (!b.is_empty()).then_some(b)
}

/// Joins edge polylines into closed loops by their ends.
fn chain_loops(polys: &[Vec<Vec2>], tol: f64) -> Vec<Vec<Vec2>> {
    let mut used = vec![false; polys.len()];
    let mut loops = Vec::new();
    for start in 0..polys.len() {
        if used[start] || polys[start].len() < 2 {
            continue;
        }
        used[start] = true;
        let mut chain = polys[start].clone();
        let mut guard = 0;
        loop {
            guard += 1;
            let Some(&end) = chain.last() else { break };
            let Some(&first) = chain.first() else { break };
            if chain.len() > 2 && end.dist(first) <= tol {
                chain.pop();
                break;
            }
            if guard > polys.len() + 2 {
                break;
            }
            let mut next: Option<(usize, bool)> = None;
            for (k, pl) in polys.iter().enumerate() {
                if used[k] || pl.len() < 2 {
                    continue;
                }
                if pl[0].dist(end) <= tol {
                    next = Some((k, false));
                    break;
                }
                if pl[pl.len() - 1].dist(end) <= tol {
                    next = Some((k, true));
                    break;
                }
            }
            match next {
                Some((k, rev)) => {
                    used[k] = true;
                    let mut pl = polys[k].clone();
                    if rev {
                        pl.reverse();
                    }
                    chain.extend(pl.into_iter().skip(1));
                }
                None => break,
            }
        }
        if chain.len() >= 3 {
            loops.push(chain);
        }
    }
    loops
}

/// Resolves the operation's geometry selection into the operation frame.
fn resolve_geometry(
    s: &Session,
    st: &Setup,
    op: &Operation,
    to_op: &dyn Fn(Vec3) -> Vec3,
    dir_to_op: &dyn Fn(Vec3) -> Vec3,
    model: Option<&cam::dropcutter::DropMesh>,
    notice: &mut Notice,
) -> Result<ResolvedGeometry> {
    let cmd = "cam.generate";
    let tol = op.tolerance;
    let mut g = ResolvedGeometry::default();
    let to_xy = |p: Vec3| {
        let q = to_op(p);
        Vec2::new(q.x, q.y)
    };
    match &op.geometry {
        Geometry::Stock => {}
        Geometry::Silhouette => {
            let Some(m) = model else { return Err(bad(cmd, "silhouette geometry needs model bodies in the setup")) };
            let b = m.bounds;
            let mut b2 = Aabb2::default();
            b2.add(Vec2::new(b.min.x - 1.0, b.min.y - 1.0));
            b2.add(Vec2::new(b.max.x + 1.0, b.max.y + 1.0));
            let cell = (tol * 4.0).max(0.05).max(b2.diagonal() / 3000.0);
            let probe = Tool::new(0, "probe", ToolKind::FlatEnd, 1e-3);
            let floor = b.min.z - 1.0;
            let map = cam::heightmap::HeightMap::from_fn(b2, cell, |p| m.drop_z(&probe, p, floor).map_or(floor, |c| c.z))
                .map_err(|e| engine_err(cmd, e))?;
            // The outer loops only: a through hole is no part of the silhouette's profile.
            let loops: Vec<Vec<Vec2>> = map.contours(floor + 0.5).iter().map(|l| cam::polyline::simplify_closed(l, tol)).collect();
            g.loops = loops.iter().enumerate().filter(|(i, _)| cam::polyline::nesting_depth(&loops, *i) == 0).map(|(_, l)| l.clone()).collect();
        }
        Geometry::Loops { loops } => {
            g.loops = cam::polyline::normalize_set(loops);
        }
        Geometry::Sketch { feature } => {
            let state = s.model.state();
            let sk = state.sketch(*feature).ok_or_else(|| bad(cmd, format!("no sketch feature {feature}")))?;
            if sk.profiles.is_empty() {
                return Err(bad(cmd, format!("sketch {} has no closed profiles", sk.name)));
            }
            let mut zs = Vec::new();
            let mut loops = Vec::new();
            for prof in &sk.profiles {
                let outer = prof.region.outer.polyline(tol).into_vec();
                let mut l: Vec<Vec2> = Vec::new();
                for q in outer {
                    let w = sk.plane.to_world(q);
                    let o = to_op(w);
                    zs.push(o.z);
                    l.push(Vec2::new(o.x, o.y));
                }
                loops.push(l);
                for h in &prof.region.holes {
                    let pts: Vec<Vec2> = h.polyline(tol).into_vec().into_iter().map(|q| to_xy(sk.plane.to_world(q))).collect();
                    loops.push(pts);
                }
            }
            g.loops = cam::polyline::normalize_set(&loops);
            if let (Some(lo), Some(hi)) = (zs.iter().copied().reduce(f64::min), zs.iter().copied().reduce(f64::max)) {
                if hi - lo > tol * 10.0 {
                    notice.issue(Issue::warning(
                        "sketch_not_level",
                        format!("sketch {} is not perpendicular to the tool axis (z spans {:.2} mm)", sk.name, hi - lo),
                    ));
                }
                g.face_z = Some((lo + hi) * 0.5);
            }
            let n = dir_to_op(sk.plane.normal());
            if n.z.abs() < 0.99 {
                notice.issue(Issue::warning(
                    "sketch_not_level",
                    format!("sketch {} is tilted {:.1}° from the tool axis", sk.name, n.z.abs().clamp(-1.0, 1.0).acos().to_degrees()),
                ));
            }
        }
        Geometry::Faces { points } => {
            let state = s.model.state();
            let mut loops: Vec<Vec<Vec2>> = Vec::new();
            let mut zs = Vec::new();
            for pt in points {
                let picked = super::face::face_at_state(&state, *pt, None)
                    .map_err(|names| bad(cmd, format!("the point {pt:?} is equally near faces of {}", names.join(", "))))?;
                let Some((mesh, face, _, body_name)) = picked else { return Err(bad(cmd, format!("no face near {pt:?}"))) };
                let body = state.body(&body_name).ok_or_else(|| bad(cmd, format!("no body {body_name}")))?;
                match solidcraft_kernel::face_surface(&body.body, face as usize) {
                    Some(solidcraft_kernel::FaceSurface::Cylinder { origin, axis, radius }) => {
                        // A hole: its top centre along the tool axis.
                        let pts: Vec<Vec3> = mesh
                            .triangles
                            .iter()
                            .zip(&mesh.tri_face)
                            .filter(|(_, f)| **f == face)
                            .flat_map(|(t, _)| t.iter().filter_map(|i| mesh.positions.get(*i as usize).copied()))
                            .collect();
                        let ax = axis.vec();
                        let tmax = pts.iter().map(|p| (*p - origin).dot(ax)).fold(f64::NEG_INFINITY, f64::max);
                        let tmin = pts.iter().map(|p| (*p - origin).dot(ax)).fold(f64::INFINITY, f64::min);
                        let a_op = dir_to_op(ax);
                        let top = if a_op.z >= 0.0 { origin + ax * tmax } else { origin + ax * tmin };
                        if a_op.z.abs() < 0.999 {
                            notice.issue(Issue::warning("hole_not_aligned", format!("the hole at {pt:?} is not along the tool axis")));
                        }
                        g.holes.push(to_op(top));
                        if g.hole_diameter > 0.0 && (g.hole_diameter - 2.0 * radius).abs() > tol {
                            notice.issue(Issue::warning(
                                "hole_diameters_differ",
                                format!("holes of {:.3} and {:.3} mm are selected together", g.hole_diameter, 2.0 * radius),
                            ));
                        }
                        g.hole_diameter = 2.0 * radius;
                    }
                    Some(solidcraft_kernel::FaceSurface::Plane { normal, point }) => {
                        let n = dir_to_op(normal.vec());
                        if n.z.abs() < 0.99 {
                            notice.issue(Issue::warning(
                                "face_not_level",
                                format!(
                                    "the face at {pt:?} is tilted {:.1}° from the tool axis; its loops are projected",
                                    n.z.abs().clamp(-1.0, 1.0).acos().to_degrees()
                                ),
                            ));
                        }
                        zs.push(to_op(point).z);
                        let edges: Vec<Vec<Vec2>> = mesh
                            .face_edges(face)
                            .into_iter()
                            .filter_map(|e| mesh.edges.get(e))
                            .map(|pl| pl.iter().map(|p| to_xy(*p)).collect())
                            .collect();
                        let mut face_loops = chain_loops(&edges, tol.max(1e-4) * 10.0);
                        for l in &mut face_loops {
                            *l = cam::polyline::clean(l, tol * 0.1);
                        }
                        loops.extend(face_loops.into_iter().filter(|l| l.len() >= 3));
                    }
                    _ => {
                        // Any other face: its boundary loops as the outline.
                        let edges: Vec<Vec<Vec2>> = mesh
                            .face_edges(face)
                            .into_iter()
                            .filter_map(|e| mesh.edges.get(e))
                            .map(|pl| pl.iter().map(|p| to_xy(*p)).collect())
                            .collect();
                        loops.extend(chain_loops(&edges, tol * 10.0).into_iter().filter(|l| l.len() >= 3));
                        notice.issue(Issue::info("face_not_planar", format!("the face at {pt:?} is not planar; its outline is used")));
                    }
                }
            }
            if !loops.is_empty() {
                g.loops = cam::polyline::normalize_set(&loops);
            }
            if !zs.is_empty() {
                g.face_z = Some(zs.iter().sum::<f64>() / zs.len() as f64);
            }
        }
        Geometry::Holes { points, diameter } => {
            let state = s.model.state();
            for pt in points {
                let mut top = to_op(*pt);
                let mut d = *diameter;
                if let Ok(Some((mesh, face, dist, body_name))) = super::face::face_at_state(&state, *pt, None)
                    && dist < 1.0
                    && let Some(body) = state.body(&body_name)
                    && let Some(solidcraft_kernel::FaceSurface::Cylinder { origin, axis, radius }) =
                        solidcraft_kernel::face_surface(&body.body, face as usize)
                {
                    let pts: Vec<Vec3> = mesh
                        .triangles
                        .iter()
                        .zip(&mesh.tri_face)
                        .filter(|(_, f)| **f == face)
                        .flat_map(|(t, _)| t.iter().filter_map(|i| mesh.positions.get(*i as usize).copied()))
                        .collect();
                    let ax = axis.vec();
                    let tmax = pts.iter().map(|p| (*p - origin).dot(ax)).fold(f64::NEG_INFINITY, f64::max);
                    let tmin = pts.iter().map(|p| (*p - origin).dot(ax)).fold(f64::INFINITY, f64::min);
                    let a_op = dir_to_op(ax);
                    top = to_op(if a_op.z >= 0.0 { origin + ax * tmax } else { origin + ax * tmin });
                    if d <= 0.0 {
                        d = 2.0 * radius;
                    }
                }
                g.holes.push(top);
                g.hole_diameter = d;
            }
        }
        Geometry::Chains { bottom, top } => {
            g.chain_a = bottom.iter().map(|p| to_op(*p)).collect();
            g.chain_b = top.iter().map(|p| to_op(*p)).collect();
        }
        Geometry::Rails { first, second } => {
            g.chain_a = first.iter().map(|p| to_op(*p)).collect();
            g.chain_b = second.iter().map(|p| to_op(*p)).collect();
        }
    }
    let _ = st;
    Ok(g)
}

/// Generates one operation. `rest` is the stock map (WCS) after the previous operations.
fn generate_one(
    s: &Session,
    st: &Setup,
    op: &Operation,
    meshes: &[(String, Arc<Mesh>)],
    fixtures: &[(String, Arc<Mesh>)],
    model_bounds_wcs: Option<Aabb3>,
    rest: Option<&cam::simulate::StockMap>,
) -> Result<Generated> {
    let cmd = "cam.generate";
    let tool =
        s.doc.manufacture.tools.get(op.tool).ok_or_else(|| bad(cmd, format!("operation {} uses tool {} which does not exist", op.id, op.tool)))?;
    let orient = op.orientation.unwrap_or(Frame::IDENTITY);
    let to_op = |p: Vec3| orient.to_local(st.wcs.to_local(p));
    let dir_to_op = |d: Vec3| orient.dir_to_local(st.wcs.dir_to_local(d));
    let mut notice = Notice::default();
    let model_mesh = if meshes.is_empty() {
        None
    } else {
        Some(cam::dropcutter::DropMesh::from_meshes(meshes.iter().map(|(_, m)| transform_mesh(m, to_op)).collect::<Vec<_>>().iter()))
    };
    let fixture_mesh = if fixtures.is_empty() {
        None
    } else {
        Some(cam::dropcutter::DropMesh::from_meshes(fixtures.iter().map(|(_, m)| transform_mesh(m, to_op)).collect::<Vec<_>>().iter()))
    };
    let model_bounds_op = model_mesh.as_ref().map_or(Aabb3::default(), |m| m.bounds);
    let stock = st.stock.resolve(&model_bounds_wcs.unwrap_or_default()).map_err(|e| engine_err(cmd, e))?;
    let stock_z = stock.z_range_in(&orient);
    let model_z = if model_bounds_op.is_empty() { stock_z } else { (model_bounds_op.min.z, model_bounds_op.max.z) };
    let geometry = resolve_geometry(s, st, op, &to_op, &dir_to_op, model_mesh.as_ref(), &mut notice)?;
    let mut resolved = op.heights.resolve(stock_z, model_z).map_err(|e| engine_err(cmd, e))?;
    if let Some(fz) = geometry.face_z
        && matches!(op.kind, OpKind::Pocket2d { .. } | OpKind::Adaptive2d { .. } | OpKind::Contour2d { .. })
        && op.heights.bottom == cam::linking::Heights::default().bottom
    {
        // The floor of a picked face is where the cut ends; the selection's top is the stock top.
        resolved.bottom = fz;
        notice.value("bottom_from_face_mm", r3(fz));
    }
    let rest_map = match rest {
        Some(r) if op.rest_machining => {
            if op.orientation.is_some() {
                notice.issue(Issue::info(
                    "rest_orientation",
                    "rest machining ignores the stock left by earlier operations when the operation is oriented away from the WCS",
                ));
                None
            } else {
                Some(&r.map)
            }
        }
        _ => None,
    };
    let ctx = Context {
        tool,
        model: model_mesh.as_ref(),
        fixtures: fixture_mesh.as_ref(),
        model_bounds: model_bounds_op,
        stock,
        frame: orient,
        heights: resolved,
        geometry,
        tolerance: op.tolerance,
        stock_to_leave_radial: op.stock_to_leave_radial,
        stock_to_leave_axial: op.stock_to_leave_axial,
        rest: rest_map,
    };
    let (tp, n) = match cam::ops::generate(op, &ctx) {
        Ok(x) => x,
        Err(e) => {
            let mut n = Notice::new(format!("{}: {e}", op.kind.type_name()));
            n.issue(Issue::error("generation_failed", e.to_string()));
            (Toolpath::new(), n)
        }
    };
    notice.merge(n);
    // To the WCS.
    let tp_wcs = tp.transformed(|p| orient.to_world(p), |a| orient.dir_to_world(a));
    let stats = tp_wcs.stats(st.machine.rapid_feed, None);
    if op.kind.axes() > st.machine.axis_count() {
        notice.issue(
            Issue::warning(
                "machine_axes",
                format!("the toolpath tilts the tool but the setup's machine `{}` has {} axes", st.machine.name, st.machine.axis_count()),
            )
            .suggest(json!({"command": "cam.machine.set", "machine": if op.kind.axes() == 4 { "four_axis_a" } else { "trunnion_ac" }})),
        );
    }
    Ok(Generated { toolpath: Arc::new(tp_wcs), notice, stats, frame: orient, setup: st.id, revision: s.geometry_revision })
}

fn generate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.generate";
    let only_setup = p.get("setup").and_then(Value::as_u64);
    let only_op = p.get("operation").and_then(Value::as_u64);
    if let Some(id) = only_setup
        && s.doc.manufacture.setup(id).is_none()
    {
        return Err(bad(cmd, format!("no setup {id}")));
    }
    if let Some(id) = only_op
        && s.doc.manufacture.operation(id).is_none()
    {
        return Err(bad(cmd, format!("no operation {id}")));
    }
    let setups: Vec<Setup> = s
        .doc
        .manufacture
        .setups
        .iter()
        .filter(|st| only_setup.is_none_or(|id| st.id == id) && only_op.is_none_or(|id| st.operations.iter().any(|o| o.id == id)))
        .cloned()
        .collect();
    if setups.is_empty() {
        return Err(bad(cmd, "no setup to generate (cam.setup.create first)"));
    }
    let mut out_setups = Vec::new();
    let mut total_time = 0.0;
    let mut worst = "ok";
    for st in &setups {
        let tol = st.operations.iter().filter(|o| o.enabled).map(|o| o.tolerance).fold(0.05, f64::min).max(0.002);
        let meshes = setup_meshes(s, st, tol, false)?;
        let fixtures = setup_meshes(s, st, tol, true)?;
        let model_bounds = setup_model_bounds(s, st);
        let mut setup_notice = Notice::default();
        if meshes.is_empty() {
            setup_notice
                .issue(Issue::warning("no_bodies", "the setup has no bodies: only 2D operations on explicit loops and swarf on chains can run"));
        }
        // Progressive stock for rest machining (3-axis in the WCS).
        let stock_shape = model_bounds.as_ref().and_then(|b| st.stock.resolve(b).ok());
        let mut stock_map =
            stock_shape.as_ref().and_then(|sh| cam::simulate::stock_cell(sh, 0.1).ok().and_then(|c| cam::simulate::StockMap::new(sh, c).ok()));
        let mut ops_out = Vec::new();
        for op in &st.operations {
            if only_op.is_some_and(|id| id != op.id) {
                continue;
            }
            if !op.enabled {
                ops_out.push(json!({"id": op.id, "name": op.name, "type": op.kind.type_name(), "status": "disabled"}));
                continue;
            }
            let mut g = match generate_one(s, st, op, &meshes, &fixtures, model_bounds, stock_map.as_ref()) {
                Ok(g) => g,
                Err(e) => {
                    let mut n = Notice::new(e.to_string());
                    n.issue(Issue::error("generation_failed", e.to_string()));
                    Generated {
                        toolpath: Arc::new(Toolpath::new()),
                        notice: n,
                        stats: ToolpathStats::default(),
                        frame: op.orientation.unwrap_or(Frame::IDENTITY),
                        setup: st.id,
                        revision: s.geometry_revision,
                    }
                }
            };
            // What the machine objects to (speeds, feeds, collets, work area).
            if let Some(tool) = s.doc.manufacture.tools.get(op.tool) {
                for issue in st.machine.check(tool, &g.toolpath) {
                    g.notice.issue(issue);
                }
            }
            // Sweep the stock when the operation is in the WCS.
            if let Some(sm) = stock_map.as_mut()
                && op.orientation.is_none()
                && let Some(tool) = s.doc.manufacture.tools.get(op.tool)
            {
                let _ = cam::simulate::simulate(sm, tool, &g.toolpath, None, None, None, None, 0.05);
            }
            total_time += g.stats.time_s;
            if g.notice.status() == "error" || (g.notice.status() == "warning" && worst == "ok") {
                worst = g.notice.status();
            }
            ops_out.push(json!({
                "id": op.id, "name": op.name, "type": op.kind.type_name(), "tool": op.tool,
                "status": g.notice.status(), "notice": g.notice.to_json(), "stats": g.stats,
            }));
            s.cam.operations.insert(op.id, Arc::new(g));
        }
        setup_notice.value("time_s", r3(ops_out.iter().filter_map(|o| o["stats"]["time_s"].as_f64()).sum::<f64>()));
        if let Some(sm) = &stock_map {
            setup_notice.value("remaining_stock_mm3", r3(sm.volume()));
        }
        out_setups.push(json!({"id": st.id, "name": st.name, "operations": ops_out, "notice": setup_notice.to_json()}));
    }
    Ok(
        json!({"setups": out_setups, "status": worst, "time_s": r3(total_time), "revision": s.geometry_revision, "hint": "cam.notices lists every operation's report; cam.simulate checks the stock; cam.post writes the G-code"}),
    )
}

fn toolpath(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.toolpath";
    let id = id_param(p, "operation", cmd)?;
    let g = s.cam.operations.get(&id).cloned().ok_or_else(|| bad(cmd, format!("operation {id} has no toolpath; run cam.generate")))?;
    let offset = p.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(2000).clamp(1, 200_000) as usize;
    let design = str_(p, "coordinates") == Some("design");
    let wcs = s.doc.manufacture.setup(g.setup).map(|st| st.wcs).unwrap_or(Frame::IDENTITY);
    let z = Vec3::new(0.0, 0.0, 1.0);
    let tilts = g.toolpath.moves.iter().any(|m| (m.axis - z).len() > 1e-6);
    let moves: Vec<Value> = g
        .toolpath
        .moves
        .iter()
        .skip(offset)
        .take(limit)
        .map(|m| {
            let (to, axis) = if design { (wcs.to_world(m.to), wcs.dir_to_world(m.axis)) } else { (m.to, m.axis) };
            let kind = match m.kind {
                MoveKind::Rapid => "rapid",
                MoveKind::Cut => "cut",
                MoveKind::Plunge => "plunge",
                MoveKind::Ramp => "ramp",
                MoveKind::Lead => "lead",
                MoveKind::Link => "link",
                MoveKind::Retract => "retract",
            };
            let mut v = json!([kind, r3(to.x), r3(to.y), r3(to.z), r3(m.feed)]);
            if tilts && let Value::Array(a) = &mut v {
                a.push(json!([r3(axis.x), r3(axis.y), r3(axis.z)]));
            }
            v
        })
        .collect();
    let bounds = g.toolpath.bounds().map(|(lo, hi)| json!({"min": lo, "max": hi}));
    Ok(json!({
        "operation": id, "coordinates": if design { "design" } else { "wcs" }, "total": g.toolpath.len(), "offset": offset, "returned": moves.len(),
        "format": "[kind, x, y, z, feed_mm_min] plus [ax, ay, az] on every move when the toolpath tilts the tool",
        "moves": moves, "bounds": bounds, "stats": g.stats, "cycles": g.toolpath.cycles.len(), "stale": g.revision != s.geometry_revision,
    }))
}

fn simulate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.simulate";
    let id = setup_param(s, p, cmd)?;
    let st = s.doc.manufacture.setup(id).cloned().ok_or_else(|| bad(cmd, format!("no setup {id}")))?;
    let tol = num(p, "tolerance").unwrap_or(0.05).clamp(0.005, 5.0);
    let meshes = setup_meshes(s, &st, tol, false)?;
    let fixtures = setup_meshes(s, &st, tol, true)?;
    let bounds = setup_model_bounds(s, &st).ok_or_else(|| bad(cmd, "the setup has no bodies to simulate against"))?;
    let shape = st.stock.resolve(&bounds).map_err(|e| engine_err(cmd, e))?;
    let cell = cam::simulate::stock_cell(&shape, tol).map_err(|e| engine_err(cmd, e))?;
    let mut stock = cam::simulate::StockMap::new(&shape, cell).map_err(|e| engine_err(cmd, e))?;
    let to_wcs = |q: Vec3| st.wcs.to_local(q);
    let model = cam::dropcutter::DropMesh::from_meshes(meshes.iter().map(|(_, m)| transform_mesh(m, to_wcs)).collect::<Vec<_>>().iter());
    let fixture = if fixtures.is_empty() {
        None
    } else {
        Some(cam::dropcutter::DropMesh::from_meshes(fixtures.iter().map(|(_, m)| transform_mesh(m, to_wcs)).collect::<Vec<_>>().iter()))
    };
    let model_map = cam::simulate::model_map(&model, &stock.map, shape.bounds().min.z - 1.0);
    let initial = stock.volume();
    let mut ops_out = Vec::new();
    let mut total = cam::simulate::SimulationReport::default();
    let mut missing = Vec::new();
    for op in st.operations.iter().filter(|o| o.enabled) {
        let Some(g) = s.cam.operations.get(&op.id).cloned() else {
            missing.push(op.id);
            continue;
        };
        let Some(tool) = s.doc.manufacture.tools.get(op.tool) else { continue };
        let rep = cam::simulate::simulate(&mut stock, tool, &g.toolpath, None, Some(&model_map), Some(&model), fixture.as_ref(), tol)
            .map_err(|e| engine_err(cmd, e))?;
        total.removed_volume_mm3 += rep.removed_volume_mm3;
        total.rapid_collisions += rep.rapid_collisions;
        total.shank_collisions += rep.shank_collisions;
        total.holder_collisions += rep.holder_collisions;
        total.fixture_collisions += rep.fixture_collisions;
        total.swept_moves += rep.swept_moves;
        total.checked_moves += rep.checked_moves;
        total.gouge_cells = total.gouge_cells.max(rep.gouge_cells);
        total.gouge_max_mm = total.gouge_max_mm.max(rep.gouge_max_mm);
        ops_out.push(json!({"id": op.id, "name": op.name, "type": op.kind.type_name(), "report": rep, "notice": rep.notice().to_json(), "stale": g.revision != s.geometry_revision}));
    }
    // Final rest and gouges from the finished stock.
    let final_rep = cam::simulate::simulate(
        &mut stock,
        &Tool::new(0, "none", ToolKind::FlatEnd, 1e-3),
        &Toolpath::new(),
        None,
        Some(&model_map),
        None,
        None,
        tol,
    )
    .map_err(|e| engine_err(cmd, e))?;
    total.remaining_volume_mm3 = stock.volume();
    total.rest_volume_mm3 = final_rep.rest_volume_mm3;
    total.rest_max_mm = final_rep.rest_max_mm;
    total.gouge_cells = final_rep.gouge_cells;
    total.gouge_max_mm = final_rep.gouge_max_mm;
    let mut notice = total.notice();
    notice.value("initial_stock_mm3", r3(initial));
    notice.value("cell_mm", r3(cell));
    if !missing.is_empty() {
        notice.issue(
            Issue::warning("not_generated", format!("{} operation(s) have no toolpath yet; run cam.generate", missing.len()))
                .suggest(json!({"command": "cam.generate", "setup": id})),
        );
    }
    let model_volume = meshes.iter().filter_map(|(_, m)| m.measure().volume.is_finite().then_some(m.measure().volume)).sum::<f64>();
    notice.value("model_volume_mm3", r3(model_volume));
    Ok(json!({"setup": id, "operations": ops_out, "total": total, "notice": notice.to_json(), "missing": missing}))
}

fn post(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.post";
    let id = setup_param(s, p, cmd)?;
    let st = s.doc.manufacture.setup(id).cloned().ok_or_else(|| bad(cmd, format!("no setup {id}")))?;
    let default_post = if !st.machine.post.is_empty() {
        st.machine.post.as_str()
    } else if st.machine.axis_count() > 3 {
        "fiveaxis"
    } else {
        "generic"
    };
    let post_id = str_(p, "post").unwrap_or(default_post);
    let mut post =
        cam::post::Post::builtin(post_id).ok_or_else(|| bad(cmd, format!("unknown post `{post_id}`: {}", cam::post::Post::BUILTIN.join(" | "))))?;
    if let Some(n) = p.get("line_numbers").and_then(Value::as_u64) {
        post.line_numbers = n.min(1000) as u32;
    }
    if let Some(c) = bool_(p, "canned_cycles") {
        post.canned_cycles = c;
    }
    let program = str_(p, "program").map(str::to_string).unwrap_or_else(|| if st.program.is_empty() { st.name.clone() } else { st.program.clone() });
    let mut missing = Vec::new();
    let mut held: Vec<(String, Tool, Arc<Toolpath>)> = Vec::new();
    for op in st.operations.iter().filter(|o| o.enabled) {
        let Some(g) = s.cam.operations.get(&op.id).cloned() else {
            missing.push(op.id);
            continue;
        };
        let tool = s.doc.manufacture.tools.get(op.tool).cloned().ok_or_else(|| bad(cmd, format!("operation {} uses a missing tool", op.id)))?;
        held.push((op.name.clone(), tool, g.toolpath.clone()));
    }
    if held.is_empty() {
        return Err(bad(cmd, "nothing to post: run cam.generate first"));
    }
    let ops: Vec<cam::post::PostOperation> = held.iter().map(|(n, t, tp)| cam::post::PostOperation { name: n, tool: t, toolpath: tp }).collect();
    let tol = st.operations.iter().map(|o| o.tolerance).fold(0.01, f64::min);
    let prog = cam::post::post_program(&post, &st.machine, &program, &ops, tol).map_err(|e| engine_err(cmd, e))?;
    let mut notice = prog.notice.clone();
    if !missing.is_empty() {
        notice.issue(
            Issue::warning("not_generated", format!("{} operation(s) were left out: no toolpath yet", missing.len()))
                .suggest(json!({"command": "cam.generate", "setup": id})),
        );
    }
    let mut written = None;
    if let Some(path) = str_(p, "path") {
        let path = path.trim();
        if path.is_empty() {
            return Err(bad(cmd, "`path` must not be empty"));
        }
        solidcraft_io::vfs::write(path, prog.gcode.as_bytes()).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        written = Some(path.to_string());
        s.echo(format!("wrote {path}"));
    }
    let gcode = if prog.gcode.len() <= 2_000_000 { Some(prog.gcode.clone()) } else { None };
    Ok(
        json!({"setup": id, "post": post.id, "machine": st.machine.name, "program": program, "lines": prog.lines, "bytes": prog.gcode.len(), "path": written, "gcode": gcode, "notice": notice.to_json(), "missing": missing}),
    )
}

/// The setup's program for the simulator: enabled, generated operations in order.
fn sim_program(s: &Session, st: &Setup) -> Result<(Vec<cam::simulate::SimOperation>, Vec<(u64, usize)>, Vec<u64>)> {
    let mut ops = Vec::new();
    let mut stamp = Vec::new();
    let mut missing = Vec::new();
    for op in st.operations.iter().filter(|o| o.enabled) {
        let Some(g) = s.cam.operations.get(&op.id) else {
            missing.push(op.id);
            continue;
        };
        let Some(tool) = s.doc.manufacture.tools.get(op.tool).cloned() else { continue };
        stamp.push((op.id, Arc::as_ptr(&g.toolpath) as usize));
        ops.push(cam::simulate::SimOperation { id: op.id, name: op.name.clone(), tool, toolpath: g.toolpath.clone() });
    }
    Ok((ops, stamp, missing))
}

fn simulate_show(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cam.simulate.show";
    let id = setup_param(s, p, cmd)?;
    let st = s.doc.manufacture.setup(id).cloned().ok_or_else(|| bad(cmd, format!("no setup {id}")))?;
    let (ops, stamp, missing) = sim_program(s, &st)?;
    if ops.is_empty() {
        return Err(bad(cmd, "nothing to simulate: run cam.generate first"));
    }
    let bounds = setup_model_bounds(s, &st);
    let shape = st.stock.resolve(&bounds.unwrap_or_default()).map_err(|e| engine_err(cmd, e))?;
    // Rebuild the simulator when it is for another setup, or the toolpaths changed.
    let rebuild = match &s.cam.simulator {
        Some(ss) => ss.setup != id || ss.stamp != stamp,
        None => true,
    };
    if rebuild {
        let b = shape.bounds();
        let size = b.size();
        let cell = match num(p, "cell") {
            Some(c) if c > 0.0 => c.max(0.02),
            _ => {
                // About 160k cells: smooth enough to look at, quick enough to step.
                let mut c = ((size.x * size.y) / 160_000.0).sqrt().max(0.05);
                let _ = &mut c;
                c
            }
        };
        if (size.x / cell + 2.0) * (size.y / cell + 2.0) > 4_000_000.0 {
            return Err(bad(cmd, "the simulation grid would exceed 4 million cells; use a larger `cell`"));
        }
        let sim = cam::simulate::Simulator::new(shape, ops, cell, st.machine.rapid_feed).map_err(|e| engine_err(cmd, e))?;
        s.cam.simulator = Some(SimulatorState { setup: id, stamp, sim: Box::new(sim) });
    }
    let Some(state) = s.cam.simulator.as_mut() else { return Err(EngineError::Other("simulator missing".into())) };
    let sim = &mut state.sim;
    // Where to go.
    let target = if let Some(step) = p.get("step").and_then(Value::as_i64) {
        let mut c = sim.cursor;
        if step >= 0 {
            let mut n = step as usize;
            while n > 0 {
                let len = sim.operations.get(c.operation).map_or(0, |o| o.toolpath.len());
                if c.move_index + 1 < len {
                    c.move_index += 1;
                } else if c.operation + 1 < sim.operations.len() {
                    c.operation += 1;
                    c.move_index = 0;
                } else {
                    break;
                }
                n -= 1;
            }
            c.fraction = 1.0;
        } else {
            let mut n = step.unsigned_abs() as usize;
            while n > 0 {
                if c.move_index > 0 {
                    c.move_index -= 1;
                } else if c.operation > 0 {
                    c.operation -= 1;
                    c.move_index = sim.operations.get(c.operation).map_or(0, |o| o.toolpath.len().saturating_sub(1));
                } else {
                    break;
                }
                n -= 1;
            }
            c.fraction = 1.0;
        }
        c
    } else if let Some(op_id) = p.get("operation").and_then(Value::as_u64) {
        let oi = sim
            .operations
            .iter()
            .position(|o| o.id == op_id)
            .ok_or_else(|| bad(cmd, format!("operation {op_id} is not in the simulation (disabled or not generated)")))?;
        let mv = p.get("move").and_then(Value::as_u64).map_or(usize::MAX, |m| m as usize);
        let len = sim.operations[oi].toolpath.len();
        cam::simulate::Cursor { operation: oi, move_index: mv.min(len.saturating_sub(1)), fraction: 1.0 }
    } else if let Some(t) = num(p, "time_s") {
        sim.cursor_at_fraction(if sim.total_time_s > 0.0 { t / sim.total_time_s } else { 1.0 })
    } else {
        let f = num(p, "progress").unwrap_or(1.0).clamp(0.0, 1.0);
        sim.cursor_at_fraction(f)
    };
    sim.seek(target).map_err(|e| engine_err(cmd, e))?;
    // Meshes and paths in design coordinates.
    let wcs = st.wcs;
    let to_design = |p: Vec3| wcs.to_world(p);
    let stock_mesh = transform_mesh_full(&sim.stock_mesh(1), &to_design, &|d| wcs.dir_to_world(d));
    let tool_mesh = transform_mesh_full(&sim.tool_mesh(), &to_design, &|d| wcs.dir_to_world(d));
    let cursor = sim.cursor;
    let mut done: Vec<Vec<Vec3>> = Vec::new();
    let mut todo: Vec<Vec<Vec3>> = Vec::new();
    let mut budget = 400_000usize;
    for (oi, op) in sim.operations.iter().enumerate() {
        let mut cur: Vec<Vec3> = Vec::new();
        let mut prev: Option<Vec3> = None;
        for (mi, m) in op.toolpath.moves.iter().enumerate() {
            let before = oi < cursor.operation || (oi == cursor.operation && mi <= cursor.move_index);
            let target_list: &mut Vec<Vec<Vec3>> = if before { &mut done } else { &mut todo };
            if m.is_cutting() || m.kind == MoveKind::Link {
                if cur.is_empty()
                    && let Some(pp) = prev
                {
                    cur.push(to_design(pp));
                }
                cur.push(to_design(m.to));
                budget = budget.saturating_sub(1);
            } else if !cur.is_empty() {
                target_list.push(std::mem::take(&mut cur));
            }
            if oi == cursor.operation && mi == cursor.move_index && !cur.is_empty() {
                done.push(std::mem::take(&mut cur));
            }
            prev = Some(m.to);
            if budget == 0 {
                break;
            }
        }
        if !cur.is_empty() {
            let before = oi < cursor.operation;
            if before { done.push(cur) } else { todo.push(cur) }
        }
    }
    let op_now = sim.operations.get(cursor.operation).map(|o| (o.id, o.name.clone()));
    let tip = to_design(sim.tip);
    let axis = wcs.dir_to_world(sim.axis);
    let preview = CamPreview {
        setup: id,
        bodies: if st.bodies.is_empty() {
            s.model.state().bodies.iter().filter(|b| !st.fixtures.contains(&b.name)).map(|b| b.name.clone()).collect()
        } else {
            st.bodies.clone()
        },
        stock: Arc::new(stock_mesh),
        tool: Arc::new(tool_mesh),
        path_done: done,
        path_todo: todo,
        tip,
        axis,
        progress: sim.progress(),
        time_s: sim.time_s,
        total_time_s: sim.total_time_s,
        operation: op_now.clone(),
        move_index: cursor.move_index,
        total_moves: sim.total_moves,
        hide_model: !bool_(p, "show_model").unwrap_or(false),
    };
    let stock_volume = sim.stock.volume();
    let triangles = preview.stock.triangles.len();
    s.cam.preview = Some(Arc::new(preview));
    let mut notice = Notice::new(format!(
        "simulation at {:.0} % ({:.0} s of {:.0} s), {}",
        sim.progress() * 100.0,
        sim.time_s,
        sim.total_time_s,
        op_now.as_ref().map_or(String::from("no operation"), |(_, n)| n.clone())
    ));
    notice.value("progress", r3(sim.progress()));
    notice.value("time_s", r3(sim.time_s));
    notice.value("total_time_s", r3(sim.total_time_s));
    notice.value("stock_volume_mm3", r3(stock_volume));
    notice.value("cell_mm", r3(sim.stock.map.cell));
    notice.value("stock_triangles", triangles);
    if !missing.is_empty() {
        notice.issue(
            Issue::warning("not_generated", format!("{} operation(s) are not in the simulation: no toolpath yet", missing.len()))
                .suggest(json!({"command": "cam.generate", "setup": id})),
        );
    }
    Ok(json!({
        "setup": id, "progress": r3(sim.progress()), "time_s": r3(sim.time_s), "total_time_s": r3(sim.total_time_s),
        "operation": op_now.as_ref().map(|(i, _)| *i), "operation_name": op_now.as_ref().map(|(_, n)| n.clone()),
        "move": cursor.move_index, "moves_in_operation": sim.operations.get(cursor.operation).map_or(0, |o| o.toolpath.len()), "total_moves": sim.total_moves,
        "tip": tip, "axis": axis, "stock_volume_mm3": r3(stock_volume), "stock_triangles": triangles,
        "notice": notice.to_json(), "hint": "screenshot shows the stock and the tool; cam.simulate.show again with a new progress, or step: n, moves on",
    }))
}

fn transform_mesh_full(m: &Mesh, f: &dyn Fn(Vec3) -> Vec3, g: &dyn Fn(Vec3) -> Vec3) -> Mesh {
    Mesh {
        positions: m.positions.iter().map(|p| f(*p)).collect(),
        normals: m.normals.iter().map(|n| g(*n)).collect(),
        triangles: m.triangles.clone(),
        tri_face: m.tri_face.clone(),
        ..Default::default()
    }
}

fn simulate_hide(s: &mut Session, _p: &Value) -> Result<Value> {
    let was = s.cam.preview.take().is_some();
    s.cam.simulator = None;
    Ok(json!({"hidden": was}))
}

fn post_list(_s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(
        json!({"posts": cam::post::Post::BUILTIN.iter().filter_map(|id| cam::post::Post::builtin(id)).map(|p| json!({"id": p.id, "name": p.name, "description": p.description, "max_axes": p.max_axes, "canned_cycles": p.canned_cycles, "tool_changes": p.tool_changes})).collect::<Vec<_>>()}),
    )
}

fn notices(s: &mut Session, p: &Value) -> Result<Value> {
    let setup = p.get("setup").and_then(Value::as_u64);
    let mut out = Vec::new();
    for st in s.doc.manufacture.setups.iter().filter(|st| setup.is_none_or(|id| st.id == id)) {
        for op in &st.operations {
            let v = match s.cam.operations.get(&op.id) {
                Some(g) => {
                    json!({"operation": op.id, "name": op.name, "type": op.kind.type_name(), "setup": st.id, "status": g.notice.status(), "stale": g.revision != s.geometry_revision, "info": g.notice.info, "values": g.notice.values, "issues": g.notice.issues})
                }
                None => {
                    json!({"operation": op.id, "name": op.name, "type": op.kind.type_name(), "setup": st.id, "status": if op.enabled { "not generated" } else { "disabled" }})
                }
            };
            out.push(v);
        }
    }
    Ok(json!({"notices": out}))
}

fn inspect(s: &mut Session, _p: &Value) -> Result<Value> {
    let m = &s.doc.manufacture;
    let mut attention = Vec::new();
    for st in &m.setups {
        for op in &st.operations {
            match s.cam.operations.get(&op.id) {
                None if op.enabled => attention.push(json!({"operation": op.id, "name": op.name, "why": "not generated", "command": "cam.generate"})),
                Some(g) if g.revision != s.geometry_revision => attention.push(json!({"operation": op.id, "name": op.name, "why": "the design changed since it was generated", "command": "cam.generate"})),
                Some(g) if g.notice.status() != "ok" => attention.push(json!({"operation": op.id, "name": op.name, "why": g.notice.status(), "issues": g.notice.issues.iter().map(|i| i.code.clone()).collect::<Vec<_>>()})),
                _ => {}
            }
        }
    }
    let bodies: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    Ok(json!({
        "tools": m.tools.tools.iter().map(|t| json!({"id": t.id, "name": t.name, "number": t.number, "kind": t.kind, "diameter": t.diameter, "uses": m.tool_uses(t.id)})).collect::<Vec<_>>(),
        "setups": m.setups.iter().map(|st| setup_json(s, st)).collect::<Vec<_>>(),
        "bodies": bodies,
        "attention": attention,
        "workflow": ["cam.tool.add", "cam.setup.create", "cam.operation.add (cam.operation.kinds lists the kinds)", "cam.generate", "cam.simulate", "cam.post"],
    }))
}
