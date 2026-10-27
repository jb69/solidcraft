//! End-to-end tests of the `cam.*` commands on small parts.

use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn fails(s: &mut Session, id: &str, p: Value) -> String {
    match s.execute(id, &p) {
        Ok(v) => panic!("{id} {p} succeeded: {v}"),
        Err(e) => e.to_string(),
    }
}

/// A 40 x 30 x 10 block with a 6 mm through hole at (10, 10) and a 16 x 10 x 4 pocket at the
/// right.
fn block() -> Session {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 10, "body_name": "Block"}));
    run(&mut s, "solid.cylinder", json!({"diameter": 6, "height": 12, "base": [10, 10, -1], "operation": "cut", "targets": ["Block"]}));
    run(&mut s, "solid.box", json!({"length": 16, "width": 10, "height": 4, "corner": [20, 10, 6], "operation": "cut", "targets": ["Block"]}));
    s
}

fn add_flat(s: &mut Session, d: f64) -> u64 {
    let t = run(s, "cam.tool.add", json!({"name": format!("{d} mm flat"), "kind": "flat", "diameter": d, "flute_length": 20, "overall_length": 50}));
    t["id"].as_u64().unwrap()
}

fn status(v: &Value) -> &str {
    v["notice"]["status"].as_str().unwrap_or("?")
}

#[test]
fn tools_setups_and_operations_are_stored_in_the_design() {
    let mut s = block();
    let tool = add_flat(&mut s, 6.0);
    let t = run(&mut s, "cam.tool.list", json!({}));
    assert_eq!(t["tools"].as_array().unwrap().len(), 1);
    assert_eq!(t["tools"][0]["kind"], "flat_end");
    // A bull nose needs its corner radius; a bad diameter is refused.
    let e = fails(&mut s, "cam.tool.add", json!({"name": "bull", "kind": "bull", "diameter": 8}));
    assert!(e.contains("corner_radius"), "{e}");
    let e = fails(&mut s, "cam.tool.add", json!({"name": "x", "kind": "flat", "diameter": -2}));
    assert!(e.contains("diameter"), "{e}");
    let setup = run(
        &mut s,
        "cam.setup.create",
        json!({"name": "Top", "stock": {"mode": "relative_box", "x_neg": 2, "x_pos": 2, "y_neg": 2, "y_pos": 2, "z_pos": 1}}),
    );
    let setup_id = setup["id"].as_u64().unwrap();
    assert_eq!(setup["stock_resolved"]["shape"], "box");
    assert!((setup["stock_resolved"]["max"][2].as_f64().unwrap() - 11.0).abs() < 1e-9, "{setup}");
    assert!((setup["stock_resolved"]["min"][0].as_f64().unwrap() + 2.0).abs() < 1e-9, "{setup}");
    let op = run(&mut s, "cam.operation.add", json!({"setup": setup_id, "type": "face", "tool": tool, "stepover": 4, "stepdwon": 1}));
    // The typo is reported, not silently dropped.
    assert_eq!(op["notice"]["issues"][0]["code"], "unknown_parameter", "{op}");
    assert_eq!(op["notice"]["issues"][0]["suggestion"]["parameter"], "stepdown");
    assert_eq!(op["stepover"], 4.0);
    let op_id = op["id"].as_u64().unwrap();
    // Edit: the type can change and the kind's parameters follow.
    let op2 = run(&mut s, "cam.operation.set", json!({"operation": op_id, "type": "pocket3d", "stepdown": 3}));
    assert_eq!(op2["type"], "pocket3d");
    assert_eq!(op2["stepdown"], 3.0);
    assert!(op2.get("angle_deg").is_none(), "{op2}");
    // Unknown type: a suggestion.
    let e = fails(&mut s, "cam.operation.add", json!({"setup": setup_id, "type": "paralel", "tool": tool}));
    assert!(e.contains("parallel"), "{e}");
    // A used tool cannot be removed.
    let e = fails(&mut s, "cam.tool.remove", json!({"tool": tool}));
    assert!(e.contains("used"), "{e}");
    // Round trip through the design's JSON.
    let text = serde_json::to_string(&*s.doc).unwrap();
    let doc: solidcraft_doc::Document = serde_json::from_str(&text).unwrap();
    assert_eq!(doc.manufacture.tools.tools.len(), 1);
    assert_eq!(doc.manufacture.setups[0].operations[0].kind.type_name(), "pocket3d");
    // Undo removes the operation again.
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.undo", json!({}));
    assert!(s.doc.manufacture.setups[0].operations.is_empty());
    let kinds = run(&mut s, "cam.operation.kinds", json!({}));
    assert_eq!(kinds["kinds"].as_array().unwrap().len(), solidcraft_cam::ops::OpKind::NAMES.len());
    assert!(kinds["kinds"].as_array().unwrap().iter().any(|k| k["type"] == "swarf" && k["axes"] == 5));
}

#[test]
fn block_is_faced_roughed_profiled_and_drilled() {
    let mut s = block();
    let flat = add_flat(&mut s, 6.0);
    let drill = run(
        &mut s,
        "cam.tool.add",
        json!({"name": "6 mm drill", "kind": "drill", "diameter": 6, "flute_length": 30, "overall_length": 60, "feeds": {"plunge": 150}}),
    )["id"]
        .as_u64()
        .unwrap();
    let setup = run(
        &mut s,
        "cam.setup.create",
        json!({"name": "Top", "stock": {"mode": "relative_box", "x_neg": 1, "x_pos": 1, "y_neg": 1, "y_pos": 1, "z_pos": 1}}),
    )["id"]
        .as_u64()
        .unwrap();
    let face = run(&mut s, "cam.operation.add", json!({"setup": setup, "type": "face", "tool": flat, "name": "Face", "stepover": 4, "stepdown": 1, "direction": "both", "heights": {"bottom": {"from": "model_top", "offset": 0}}}))["id"].as_u64().unwrap();
    let rough = run(&mut s, "cam.operation.add", json!({"setup": setup, "type": "pocket3d", "tool": flat, "name": "Rough", "stepover": 3, "stepdown": 2, "stock_to_leave_radial": 0.2, "stock_to_leave_axial": 0.1}))["id"].as_u64().unwrap();
    // The pocket floor, picked by a point on it: a 2D pocket down to that face.
    let pocket = run(&mut s, "cam.operation.add", json!({"setup": setup, "type": "pocket2d", "tool": flat, "name": "Pocket", "stepover": 2, "stepdown": 2, "geometry": {"source": "faces", "points": [[28, 15, 6]]}}))["id"].as_u64().unwrap();
    let contour = run(&mut s, "cam.operation.add", json!({"setup": setup, "type": "contour2d", "tool": flat, "name": "Outside", "side": "outside", "stepdown": 5, "geometry": {"source": "silhouette"}}))["id"].as_u64().unwrap();
    let hole = run(&mut s, "cam.operation.add", json!({"setup": setup, "type": "drill", "tool": drill, "name": "Drill", "cycle": "peck", "peck": 2, "break_through": 1, "geometry": {"source": "holes", "points": [[13, 10, 5]]}}))["id"].as_u64().unwrap();
    let g = run(&mut s, "cam.generate", json!({"setup": setup}));
    let ops = g["setups"][0]["operations"].as_array().unwrap().clone();
    assert_eq!(ops.len(), 5, "{g}");
    for o in &ops {
        assert_ne!(o["status"], "error", "{o}");
        assert!(o["stats"]["moves"].as_u64().unwrap() > 0, "{o}");
    }
    assert_eq!(g["status"], "ok", "{g}");
    // Face: cuts at z = 10 (the model top) over the whole stock.
    let tp = run(&mut s, "cam.toolpath", json!({"operation": face, "limit": 100000}));
    assert!((tp["bounds"]["min"][2].as_f64().unwrap() - 10.0).abs() < 1e-6, "{}", tp["bounds"]);
    assert!(tp["bounds"]["max"][0].as_f64().unwrap() >= 41.0);
    // Rough: stays 0.1 above the model's floors (pocket floor at 6 → 6.1) and never below 0.
    let tp = run(&mut s, "cam.toolpath", json!({"operation": rough, "limit": 200000}));
    let min_z = tp["bounds"]["min"][2].as_f64().unwrap();
    assert!(min_z >= 0.0 - 1e-6, "{min_z}");
    let cuts_in_pocket: Vec<f64> = tp["moves"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| {
            m[0] == "cut"
                && m[1].as_f64().unwrap() > 24.0
                && m[1].as_f64().unwrap() < 32.0
                && m[2].as_f64().unwrap() > 14.0
                && m[2].as_f64().unwrap() < 16.0
        })
        .map(|m| m[3].as_f64().unwrap())
        .collect();
    assert!(!cuts_in_pocket.is_empty());
    assert!(cuts_in_pocket.iter().all(|z| *z >= 6.1 - 1e-6), "{cuts_in_pocket:?}");
    // Pocket: the bottom is the picked face.
    let n = run(&mut s, "cam.notices", json!({"setup": setup}));
    let pocket_notice = n["notices"].as_array().unwrap().iter().find(|x| x["operation"] == pocket).unwrap().clone();
    assert!((pocket_notice["values"]["bottom_from_face_mm"].as_f64().unwrap() - 6.0).abs() < 1e-6, "{pocket_notice}");
    assert_eq!(pocket_notice["status"], "ok", "{pocket_notice}");
    let tp = run(&mut s, "cam.toolpath", json!({"operation": pocket, "limit": 100000}));
    assert!((tp["bounds"]["min"][2].as_f64().unwrap() - 6.0).abs() < 1e-6, "{}", tp["bounds"]);
    // The tool centre stays 3 mm inside the 16 x 10 pocket.
    assert!(tp["bounds"]["min"][0].as_f64().unwrap() >= 23.0 - 1e-6 && tp["bounds"]["max"][0].as_f64().unwrap() <= 33.0 + 1e-6, "{}", tp["bounds"]);
    // Contour: 3 mm outside the 40 x 30 outline.
    let tp = run(&mut s, "cam.toolpath", json!({"operation": contour, "limit": 100000}));
    let cuts: Vec<&Value> = tp["moves"].as_array().unwrap().iter().filter(|m| m[0] == "cut").collect();
    let min_x = cuts.iter().map(|m| m[1].as_f64().unwrap()).fold(f64::INFINITY, f64::min);
    let max_x = cuts.iter().map(|m| m[1].as_f64().unwrap()).fold(f64::NEG_INFINITY, f64::max);
    assert!((min_x + 3.0).abs() < 0.2 && (max_x - 43.0).abs() < 0.2, "{min_x} {max_x}");
    // Drill: the hole was found as a 6 mm cylinder at (10, 10); the cycle bottoms below the part.
    let tp = run(&mut s, "cam.toolpath", json!({"operation": hole}));
    assert_eq!(tp["cycles"], 1, "{tp}");
    let plunges: Vec<&Value> = tp["moves"].as_array().unwrap().iter().filter(|m| m[0] == "plunge").collect();
    assert!(plunges.iter().all(|m| (m[1].as_f64().unwrap() - 10.0).abs() < 1e-6 && (m[2].as_f64().unwrap() - 10.0).abs() < 1e-6), "{plunges:?}");
    assert!(tp["bounds"]["min"][2].as_f64().unwrap() < -1.0);
    // Simulation: no gouges, no rapids through stock, most of the stock around the part gone.
    let sim = run(&mut s, "cam.simulate", json!({"setup": setup}));
    assert_eq!(sim["total"]["gouge_cells"], 0, "{}", sim["notice"]);
    assert_eq!(sim["total"]["rapid_collisions"], 0, "{}", sim["notice"]);
    assert!(sim["total"]["removed_volume_mm3"].as_f64().unwrap() > 1500.0, "{}", sim["notice"]);
    assert_eq!(sim["notice"]["status"], "ok", "{}", sim["notice"]);
    // Post: one program, two tools, canned peck cycle, ends with M30.
    let post = run(&mut s, "cam.post", json!({"setup": setup}));
    let g = post["gcode"].as_str().unwrap();
    assert!(g.contains("T1 M6") && g.contains("T2 M6"), "{}", &g[..600]);
    assert!(g.contains("G83"), "{}", &g[g.len() - 800..]);
    assert!(g.trim_end().ends_with("M30\n%"), "{}", &g[g.len() - 100..]);
    assert_eq!(post["notice"]["status"], "ok", "{}", post["notice"]);
    // Changing the design marks the toolpaths stale.
    run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [0, 0, 10], "operation": "join", "targets": ["Block"]}));
    let n = run(&mut s, "cam.notices", json!({}));
    assert!(n["notices"].as_array().unwrap().iter().all(|x| x["stale"] == true), "{n}");
    let i = run(&mut s, "cam.inspect", json!({}));
    assert_eq!(i["attention"].as_array().unwrap().len(), 5, "{}", i["attention"]);
}

#[test]
fn rotary_part_posts_with_a_words() {
    let mut s = Session::default();
    // A shaft along X with a flat milled on it.
    run(&mut s, "solid.cylinder", json!({"diameter": 20, "height": 60, "base": [-30, 0, 0], "axis": "X", "body_name": "Shaft"}));
    run(&mut s, "solid.box", json!({"length": 20, "width": 30, "height": 5, "corner": [-10, -15, 7], "operation": "cut", "targets": ["Shaft"]}));
    let ball = run(&mut s, "cam.tool.add", json!({"name": "4 mm ball", "kind": "ball", "diameter": 4}))["id"].as_u64().unwrap();
    let setup = run(
        &mut s,
        "cam.setup.create",
        json!({"name": "Rotary", "machine": "four_axis_a", "stock": {"mode": "relative_cylinder", "axis": "x", "radial": 1, "start": 0, "end": 0}}),
    )["id"]
        .as_u64()
        .unwrap();
    let op = run(
        &mut s,
        "cam.operation.add",
        json!({"setup": setup, "type": "rotary", "tool": ball, "along_axis": true, "stepover": 15, "tolerance": 0.05}),
    );
    assert_eq!(status(&op), "ok", "{op}");
    let g = run(&mut s, "cam.generate", json!({"setup": setup}));
    assert_ne!(g["setups"][0]["operations"][0]["status"], "error", "{g}");
    let tp = run(&mut s, "cam.toolpath", json!({"operation": op["id"], "limit": 50000}));
    // Every cut has a radial tool axis and sits near the 10 mm surface + 2 mm ball... at the tip
    // radius 10 (the ball touches the cylinder), 7 on the flat.
    let cuts: Vec<&Value> = tp["moves"].as_array().unwrap().iter().filter(|m| m[0] == "cut").collect();
    assert!(cuts.len() > 20, "{}", cuts.len());
    for m in &cuts {
        let r = (m[2].as_f64().unwrap().powi(2) + m[3].as_f64().unwrap().powi(2)).sqrt();
        assert!(r <= 10.0 + 0.2 && r >= 6.9, "{m}");
        assert!(m.get(5).is_some(), "tilted move without axis: {m}");
    }
    assert!(tp["stats"]["max_tilt_deg"].as_f64().unwrap() > 90.0);
    let post = run(&mut s, "cam.post", json!({"setup": setup}));
    assert_eq!(post["post"], "fiveaxis");
    let g = post["gcode"].as_str().unwrap();
    assert!(g.lines().any(|l| l.contains(" A") || l.starts_with('A')), "{}", &g[..800]);
    // Every posted point lies on the machine's XZ plane (Y = 0 after the table turns).
    for l in g.lines().filter(|l| l.starts_with("G1") || l.starts_with("G0")) {
        if let Some(y) = l.split_whitespace().find(|w| w.starts_with('Y')) {
            let v: f64 = y[1..].parse().unwrap();
            assert!(v.abs() < 1e-3, "{l}");
        }
    }
    // The 3-axis post refuses it with a clear message.
    let e = fails(&mut s, "cam.post", json!({"setup": setup, "post": "generic"}));
    assert!(e.contains("multi-axis post"), "{e}");
}

#[test]
fn swarf_and_indexed_orientation() {
    let mut s = block();
    let flat = add_flat(&mut s, 6.0);
    let setup = run(&mut s, "cam.setup.create", json!({"name": "5x", "machine": "trunnion_ac"}))["id"].as_u64().unwrap();
    // Swarf along the block's front wall, which is vertical: the tool axis is z.
    let op = run(
        &mut s,
        "cam.operation.add",
        json!({"setup": setup, "type": "swarf", "tool": flat, "left": false, "geometry": {"source": "chains", "bottom": [[0, 0, 0], [40, 0, 0]], "top": [[0, 0, 10], [40, 0, 10]]}}),
    );
    assert_eq!(status(&op), "ok", "{op}");
    // An indexed contour from the side: tool along -y, looking at the front face.
    let side = run(
        &mut s,
        "cam.operation.add",
        json!({"setup": setup, "type": "contour2d", "tool": flat, "side": "outside", "orientation": {"origin": [0, 0, 0], "z": [0, -1, 0], "x": [1, 0, 0]}, "geometry": {"source": "silhouette"}, "heights": {"bottom": {"from": "model_top", "offset": -2}}}),
    );
    let g = run(&mut s, "cam.generate", json!({"setup": setup}));
    for o in g["setups"][0]["operations"].as_array().unwrap() {
        assert_ne!(o["status"], "error", "{o}");
    }
    let tp = run(&mut s, "cam.toolpath", json!({"operation": op["id"]}));
    let cuts: Vec<&Value> = tp["moves"].as_array().unwrap().iter().filter(|m| m[0] == "cut").collect();
    assert!(!cuts.is_empty());
    // Right of +x travel on a vertical wall: the tool centre sits at y = -3.
    for m in &cuts {
        assert!((m[2].as_f64().unwrap() + 3.0).abs() < 1e-6, "{m}");
    }
    let tp = run(&mut s, "cam.toolpath", json!({"operation": side["id"]}));
    // Indexed: every move's axis is -y in the WCS.
    for m in tp["moves"].as_array().unwrap() {
        let a = &m[5];
        assert!((a[1].as_f64().unwrap() + 1.0).abs() < 1e-9, "{m}");
    }
    let post = run(&mut s, "cam.post", json!({"setup": setup}));
    assert_eq!(post["notice"]["status"], "ok", "{}", post["notice"]);
    assert!(post["gcode"].as_str().unwrap().contains("A"));
}

#[test]
fn hostile_cam_params_never_panic() {
    let mut s = block();
    let flat = add_flat(&mut s, 6.0);
    let setup = run(&mut s, "cam.setup.create", json!({}))["id"].as_u64().unwrap();
    let junk = [
        json!(null),
        json!(-1),
        json!(1e300),
        json!(f64::MAX),
        json!("x"),
        json!([]),
        json!({}),
        json!([1, 2]),
        json!([[0, 0], [1, 1]]),
        json!({"from": "nowhere"}),
    ];
    let cmds = [
        "cam.tool.add",
        "cam.tool.set",
        "cam.setup.create",
        "cam.setup.set",
        "cam.machine.set",
        "cam.operation.add",
        "cam.operation.set",
        "cam.toolpath",
    ];
    let keys = [
        "tool",
        "setup",
        "operation",
        "type",
        "diameter",
        "kind",
        "stepover",
        "stepdown",
        "geometry",
        "heights",
        "linking",
        "machine",
        "stock",
        "wcs",
        "orientation",
        "tolerance",
        "post",
        "path",
        "limit",
        "offset",
        "corner_radius",
        "points",
        "loops",
        "feeds",
    ];
    for cmd in cmds {
        for k in keys {
            for j in &junk {
                let mut p = json!({"setup": setup, "tool": flat, "type": "pocket2d", "name": "t", "kind": "flat", "diameter": 6});
                p[k] = j.clone();
                let r = s.execute(cmd, &p);
                if let Err(crate::EngineError::Internal(m, _)) = &r {
                    panic!("{cmd} {k}={j}: {m}");
                }
            }
        }
    }
    for cmd in ["cam.generate", "cam.simulate", "cam.post"] {
        for p in [
            json!({}),
            json!({"setup": -1}),
            json!({"setup": "x", "operation": 1e300}),
            json!({"setup": setup, "post": "nope", "path": "", "tolerance": -1}),
        ] {
            if let Err(crate::EngineError::Internal(m, _)) = s.execute(cmd, &p) {
                panic!("{cmd} {p}: {m}");
            }
        }
    }
    // Operations with absurd parameters generate without panicking.
    for params in [
        json!({"stepover": 1e-9, "stepdown": 1e-9}),
        json!({"stepover": 1e9}),
        json!({"tolerance": 10}),
        json!({"heights": {"bottom": -1000}}),
        json!({"type": "parallel", "angle_deg": 1e9}),
        json!({"type": "rotary", "stepover": 0.001}),
    ] {
        let mut p = json!({"setup": setup, "tool": flat, "type": "pocket3d"});
        for (k, v) in params.as_object().unwrap() {
            p[k] = v.clone();
        }
        if let Ok(op) = s.execute("cam.operation.add", &p) {
            let r = s.execute("cam.generate", &json!({"operation": op["id"]}));
            if let Err(crate::EngineError::Internal(m, _)) = &r {
                panic!("{p}: {m}");
            }
            let _ = s.execute("cam.operation.remove", &json!({"operation": op["id"]}));
        }
    }
}

#[test]
fn carvera_program_uses_the_kit_the_checks_and_the_makera_post() {
    let mut s = block();
    let kit = run(&mut s, "cam.tool.kit", json!({"material": "aluminium", "machine": "carvera"}));
    let tools = kit["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 11, "{kit}");
    assert_eq!(kit["notice"]["status"], "ok");
    let flat = tools[0]["id"].as_u64().unwrap();
    let drill = tools[8]["id"].as_u64().unwrap();
    let setup = run(&mut s, "cam.setup.create", json!({"name": "Carvera", "machine": "carvera"}))["id"].as_u64().unwrap();
    let st = run(&mut s, "cam.setup.list", json!({}));
    assert_eq!(st["setups"][0]["machine"]["name"], "Makera Carvera", "{st}");
    run(&mut s, "cam.operation.add", json!({"setup": setup, "type": "face", "tool": flat, "name": "Face", "stepover": 2, "stepdown": 0.5, "heights": {"bottom": {"from": "model_top", "offset": 0}}}));
    run(&mut s, "cam.operation.add", json!({"setup": setup, "type": "pocket2d", "tool": flat, "name": "Pocket", "stepover": 1.2, "stepdown": 0.8, "geometry": {"source": "faces", "points": [[28, 15, 6]]}}));
    run(&mut s, "cam.operation.add", json!({"setup": setup, "type": "drill", "tool": drill, "name": "Drill", "cycle": "peck", "peck": 1, "geometry": {"source": "holes", "points": [[13, 10, 5]]}}));
    let g = run(&mut s, "cam.generate", json!({"setup": setup}));
    // The block's hole is 6 mm and the kit's drill 3.175 mm: that warning is the only one.
    assert_eq!(g["status"], "warning", "{g}");
    for o in g["setups"][0]["operations"].as_array().unwrap() {
        for i in o["notice"]["issues"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            assert_eq!(i["code"], "tool_diameter_mismatch", "{o}");
        }
    }
    let post = run(&mut s, "cam.post", json!({}));
    assert_eq!(post["post"], "makera", "{post}");
    let code = post["gcode"].as_str().unwrap();
    assert!(code.contains("M6 T1") && code.contains("M6 T9") && !code.contains("G43") && !code.contains("G83"), "{}", &code[..400]);
    assert!(code.trim_end().ends_with("M2"));
    // T9 is beyond the six slots: the control asks for it by hand.
    assert!(post["notice"]["issues"].as_array().unwrap().iter().any(|i| i["code"] == "manual_tool_change"), "{}", post["notice"]);
    // The machine list carries the Makera presets with their numbers.
    let ml = run(&mut s, "cam.machine.list", json!({}));
    let z1 = ml["machines"].as_array().unwrap().iter().find(|m| m["preset"] == "z1").unwrap();
    assert_eq!(z1["spindle_rpm_max"], 13000.0);
    assert_eq!(z1["work_area_mm"][0], 200.0);
    // A tool asked to spin faster than the Z1 can is flagged by generate on a Z1 setup.
    run(&mut s, "cam.tool.set", json!({"tool": flat, "feeds": {"spindle_rpm": 15000}}));
    run(&mut s, "cam.machine.set", json!({"setup": setup, "machine": "z1"}));
    let g = run(&mut s, "cam.generate", json!({"setup": setup}));
    let face = &g["setups"][0]["operations"][0];
    assert!(face["notice"]["issues"].as_array().unwrap().iter().any(|i| i["code"] == "spindle_speed" && i["limit"] == 13000.0), "{face}");
}
