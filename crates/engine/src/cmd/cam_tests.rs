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
        assert!((6.9..=10.2).contains(&r), "{m}");
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
    // Eleven cutters and the Carvera's built-in laser.
    assert_eq!(tools.len(), 12, "{kit}");
    assert_eq!(kit["notice"]["status"], "ok");
    let flat = tools[0]["id"].as_u64().unwrap();
    let drill = tools[8]["id"].as_u64().unwrap();
    let setup = run(&mut s, "cam.setup.create", json!({"name": "Carvera", "machine": "carvera"}))["id"].as_u64().unwrap();
    let st = run(&mut s, "cam.setup.list", json!({}));
    assert_eq!(st["setups"][0]["machine"]["name"], "Makera Carvera", "{st}");
    run(
        &mut s,
        "cam.operation.add",
        json!({"setup": setup, "type": "face", "tool": flat, "name": "Face", "stepover": 2, "stepdown": 0.5, "heights": {"bottom": {"from": "model_top", "offset": 0}}}),
    );
    run(
        &mut s,
        "cam.operation.add",
        json!({"setup": setup, "type": "pocket2d", "tool": flat, "name": "Pocket", "stepover": 1.2, "stepdown": 0.8, "geometry": {"source": "faces", "points": [[28, 15, 6]]}}),
    );
    run(
        &mut s,
        "cam.operation.add",
        json!({"setup": setup, "type": "drill", "tool": drill, "name": "Drill", "cycle": "peck", "peck": 1, "geometry": {"source": "holes", "points": [[13, 10, 5]]}}),
    );
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

#[test]
fn every_operation_kind_has_a_command_and_the_new_kinds_cut() {
    let mut s = block();
    let flat = add_flat(&mut s, 3.0);
    let v = run(&mut s, "cam.tool.add", json!({"name": "60° V", "kind": "chamfer", "tip_angle_deg": 60, "diameter": 6}))["id"].as_u64().unwrap();
    let setup = run(&mut s, "cam.setup.create", json!({"name": "Top"}))["id"].as_u64().unwrap();
    // A command per kind, with setup and tool defaulting to the only setup and the first tool.
    for kind in solidcraft_cam::ops::OpKind::NAMES {
        let spec = crate::find_command(&format!("cam.operation.{kind}")).unwrap_or_else(|| panic!("cam.operation.{kind}"));
        assert_eq!(spec.tab, "MANUFACTURE", "{kind}");
    }
    let kinds = run(&mut s, "cam.operation.kinds", json!({}));
    let names: Vec<&str> = kinds["kinds"].as_array().unwrap().iter().map(|k| k["type"].as_str().unwrap()).collect();
    for k in ["slot", "trace", "engrave", "circular", "thread", "adaptive3d", "morph"] {
        assert!(names.contains(&k), "{names:?}");
    }
    // A slot around a rectangle, 4 mm wide with a 3 mm tool, 2 mm deep.
    let rect = json!([[[5, 5], [35, 5], [35, 25], [5, 25]]]);
    let slot = run(
        &mut s,
        "cam.operation.slot",
        json!({"name": "Slot", "width": 4, "stepdown": 1, "geometry": {"source": "loops", "loops": rect}, "heights": {"bottom": {"from": "model_top", "offset": -2}}}),
    );
    assert_eq!(slot["tool"], flat, "{slot}");
    assert_eq!(slot["setup"], setup);
    let trace = run(
        &mut s,
        "cam.operation.trace",
        json!({"name": "Trace", "geometry": {"source": "loops", "loops": rect}, "heights": {"bottom": {"from": "model_top", "offset": -0.5}}}),
    );
    let engrave =
        run(&mut s, "cam.operation.engrave", json!({"name": "Engrave", "tool": v, "width": 1.0, "geometry": {"source": "loops", "loops": rect}}));
    let circular =
        run(&mut s, "cam.operation.circular", json!({"name": "Circular", "stepdown": 3, "geometry": {"source": "holes", "points": [[13, 10, 5]]}}));
    let thread = run(
        &mut s,
        "cam.operation.thread",
        json!({"name": "Thread", "pitch": 1.0, "passes": 2, "geometry": {"source": "holes", "points": [[13, 10, 5]]}, "heights": {"bottom": {"from": "model_top", "offset": -6}}}),
    );
    let tap = run(&mut s, "cam.operation.tap", json!({"name": "Tap", "pitch": 1.0, "geometry": {"source": "holes", "points": [[13, 10, 5]]}}));
    let adaptive = run(&mut s, "cam.operation.adaptive3d", json!({"name": "Adaptive 3D", "stepdown": 2, "optimal_load": 1.0}));
    let morph = run(
        &mut s,
        "cam.operation.morph",
        json!({"name": "Morph", "stepover": 2, "geometry": {"source": "rails", "first": [[2, 2, 10], [38, 2, 10]], "second": [[2, 28, 10], [38, 28, 10]]}}),
    );
    let g = run(&mut s, "cam.generate", json!({}));
    let ops = g["setups"][0]["operations"].as_array().unwrap();
    for o in ops {
        assert_ne!(o["status"], "error", "{o}");
        assert!(o["stats"]["moves"].as_u64().unwrap() > 0, "{o}");
    }
    let by_id = |id: &Value| ops.iter().find(|o| &o["id"] == id).unwrap().clone();
    // The slot has three offset passes per level (centre and the two walls) at three levels
    // (from the stock top, 1 mm above the part, 2 mm into it at 1 mm a level).
    let sl = by_id(&slot["id"]);
    assert_eq!(sl["notice"]["info"].as_str().unwrap().split(':').next(), Some("slot"), "{sl}");
    assert_eq!(sl["notice"]["values"]["levels"], 3, "{sl}");
    assert_eq!(sl["notice"]["values"]["passes"], 9, "{sl}");
    assert!((sl["stats"]["min_z"].as_f64().unwrap() - 8.0).abs() < 1e-6);
    let tr = by_id(&trace["id"]);
    assert!((tr["stats"]["min_z"].as_f64().unwrap() - 9.5).abs() < 1e-6, "{tr}");
    // Engraving a 1 mm wide groove with a 60° V: depth = 0.5 / tan 30° = 0.866.
    let en = by_id(&engrave["id"]);
    assert!((en["stats"]["min_z"].as_f64().unwrap() - (10.0 - 0.866)).abs() < 0.01, "{en}");
    // Circular milling of the 6 mm hole with a 3 mm tool: two levels, rings at the wall.
    let ci = by_id(&circular["id"]);
    assert_eq!(ci["notice"]["values"]["holes"], 1, "{ci}");
    assert!(ci["notice"]["values"]["rings"].as_u64().unwrap() >= 4, "{ci}");
    // The thread climbs 7 mm (from the stock top, 1 mm above the part) at 1 mm per turn, twice.
    let th = by_id(&thread["id"]);
    assert!((th["notice"]["values"]["turns"].as_f64().unwrap() - 7.0).abs() < 1e-6, "{th}");
    assert_eq!(th["notice"]["values"]["passes"], 2);
    let tp = run(&mut s, "cam.toolpath", json!({"operation": thread["id"], "limit": 100000}));
    let r = (tp["bounds"]["max"][0].as_f64().unwrap() - 13.0).abs();
    assert!((r - 1.5).abs() < 0.05, "thread radius {r}: {}", tp["bounds"]);
    let ta = by_id(&tap["id"]);
    assert_eq!(ta["notice"]["values"]["cycle"], "tap");
    assert!((ta["notice"]["values"]["tap_feed_mm_min"].as_f64().unwrap() - 10000.0).abs() < 1e-6, "{ta}");
    let ad = by_id(&adaptive["id"]);
    assert!(ad["notice"]["values"]["rings"].as_u64().unwrap() > 0, "{ad}");
    let mo = by_id(&morph["id"]);
    assert!(mo["notice"]["values"]["passes"].as_u64().unwrap() >= 13, "{mo}");
    // On the top face between the rails, down to the part's bottom where it crosses the hole.
    let tp = run(&mut s, "cam.toolpath", json!({"operation": morph["id"], "limit": 100000}));
    let zs: Vec<f64> = tp["moves"].as_array().unwrap().iter().filter(|m| m[0] == "cut").map(|m| m[3].as_f64().unwrap()).collect();
    assert!(zs.iter().all(|z| *z >= -1e-6 && *z <= 10.0 + 1e-6), "{zs:?}");
    assert!(zs.iter().filter(|z| (**z - 10.0).abs() < 1e-6).count() > zs.len() / 2, "mostly on the top face");
    // The makera post writes G84 as an expanded cycle and the generic one as a canned cycle.
    let p = run(&mut s, "cam.post", json!({"post": "generic"}));
    assert!(p["gcode"].as_str().unwrap().contains("G84"), "{}", &p["gcode"].as_str().unwrap()[..300]);
}

#[test]
fn laser_programs_post_in_makera_laser_mode() {
    let mut s = block();
    let kit = run(&mut s, "cam.tool.kit", json!({"material": "wood", "machine": "carvera"}));
    let tools = kit["tools"].as_array().unwrap();
    let laser = tools.iter().find(|t| t["kind"] == "laser").unwrap_or_else(|| panic!("{kit}"))["id"].as_u64().unwrap();
    let flat = tools[0]["id"].as_u64().unwrap();
    run(&mut s, "cam.setup.create", json!({"name": "Carvera", "machine": "carvera"}));
    let rect = json!([[[5, 5], [35, 5], [35, 25], [5, 25]]]);
    run(
        &mut s,
        "cam.operation.face",
        json!({"tool": flat, "stepover": 2, "stepdown": 0.5, "heights": {"bottom": {"from": "model_top", "offset": 0}}}),
    );
    let engrave = run(
        &mut s,
        "cam.operation.laser_engrave",
        json!({"tool": laser, "power": 0.4, "speed": 2500, "line_interval": 0.5, "geometry": {"source": "loops", "loops": rect}}),
    );
    let cut = run(
        &mut s,
        "cam.operation.laser_cut",
        json!({"tool": laser, "power": 1.0, "speed": 250, "passes": 2, "geometry": {"source": "loops", "loops": rect}}),
    );
    // A cutter cannot run a laser operation.
    let e = fails(&mut s, "cam.operation.laser_cut", json!({"tool": flat, "geometry": {"source": "loops", "loops": rect}}));
    let _ = e;
    let g = run(&mut s, "cam.generate", json!({}));
    let ops = g["setups"][0]["operations"].as_array().unwrap();
    let by = |id: &Value| ops.iter().find(|o| &o["id"] == id).unwrap().clone();
    let en = by(&engrave["id"]);
    assert_eq!(en["status"], "ok", "{en}");
    // 20 mm tall region at 0.5 mm: about 40 lines.
    let lines = en["notice"]["values"]["lines"].as_u64().unwrap();
    assert!((38..=42).contains(&lines), "{en}");
    let cu = by(&cut["id"]);
    assert_eq!(cu["notice"]["values"]["passes"], 2, "{cu}");
    // The stock simulation leaves laser work alone.
    let sim = run(&mut s, "cam.simulate", json!({}));
    assert_eq!(sim["notice"]["values"]["gouge_cells"], 0, "{}", sim["notice"]);
    let post = run(&mut s, "cam.post", json!({}));
    let code = post["gcode"].as_str().unwrap();
    let on = code.find("\nM321\n").unwrap_or_else(|| panic!("{code}"));
    let off = code.find("\nM322\n").unwrap_or_else(|| panic!("{code}"));
    assert!(on < off && code[..on].contains("M6 T1"), "milling first, then the laser");
    let laser_part = &code[on..off];
    assert!(laser_part.contains("\nG0 Z0\n"), "focus at the work's Z0");
    assert!(laser_part.contains("S0.4") && laser_part.contains("S1.0"), "power as a fraction");
    assert!(laser_part.contains("F2500.0") && laser_part.contains("F250.0"));
    for l in laser_part.lines().filter(|l| l.starts_with("G1") || l.starts_with("G0 X")) {
        assert!(!l.contains('Z'), "no Z in laser mode: {l}");
    }
    // A laser program on a machine without a laser module is an error.
    run(&mut s, "cam.machine.set", json!({"machine": "three_axis", "setup": g["setups"][0]["id"]}));
    let g = run(&mut s, "cam.generate", json!({}));
    let en = g["setups"][0]["operations"].as_array().unwrap().iter().find(|o| o["id"] == engrave["id"]).unwrap().clone();
    assert!(en["notice"]["issues"].as_array().unwrap().iter().any(|i| i["code"] == "no_laser"), "{en}");
}

#[test]
fn turning_on_the_rotary_axis_faces_grooves_and_threads() {
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"diameter": 20, "height": 60, "base": [-30, 0, 0], "axis": "X", "body_name": "Shaft"}));
    let flat = add_flat(&mut s, 3.0);
    let v = run(&mut s, "cam.tool.add", json!({"name": "60° V", "kind": "chamfer", "tip_angle_deg": 60, "diameter": 6}))["id"].as_u64().unwrap();
    let setup = run(
        &mut s,
        "cam.setup.create",
        json!({"name": "Rotary", "machine": "carvera_4axis", "stock": {"mode": "fixed_cylinder", "axis": "x", "radius": 11, "start": -30, "end": 33}}),
    )["id"]
        .as_u64()
        .unwrap();
    let face = run(&mut s, "cam.operation.turn_face", json!({"tool": flat, "stepdown": 1}));
    let groove = run(&mut s, "cam.operation.turn_groove", json!({"tool": flat, "position": 0, "width": 4, "depth": 2, "stepdown": 1}));
    let thread = run(&mut s, "cam.operation.turn_thread", json!({"tool": v, "pitch": 1.5, "passes": 3, "start": -25, "length": 15}));
    let profile = run(&mut s, "cam.operation.turn_profile", json!({"tool": flat, "stepover": 30}));
    let kinds = run(&mut s, "cam.operation.kinds", json!({"type": "turn_thread"}));
    assert_eq!(kinds["kinds"][0]["axes"], 4);
    let g = run(&mut s, "cam.generate", json!({"setup": setup}));
    let ops = g["setups"][0]["operations"].as_array().unwrap();
    for o in ops {
        assert_ne!(o["status"], "error", "{o}");
    }
    let by = |id: &Value| ops.iter().find(|o| &o["id"] == id).unwrap().clone();
    // Facing back 3 mm of stock at 1 mm a level.
    let fa = by(&face["id"]);
    assert_eq!(fa["notice"]["values"]["levels"], 3, "{fa}");
    assert!((fa["notice"]["values"]["face_x_mm"].as_f64().unwrap() - 30.0).abs() < 1e-6, "{fa}");
    // The groove's floor is 2 mm below the 11 mm stock radius.
    let gr = by(&groove["id"]);
    assert!((gr["notice"]["values"]["radius_mm"].as_f64().unwrap() - 9.0).abs() < 1e-6, "{gr}");
    // Ten turns of 1.5 mm over 15 mm, three passes, each cut point at the pass radius.
    let th = by(&thread["id"]);
    assert!((th["notice"]["values"]["turns"].as_f64().unwrap() - 10.0).abs() < 1e-9, "{th}");
    assert_eq!(th["notice"]["values"]["passes"], 3);
    let depth = th["notice"]["values"]["thread_depth_mm"].as_f64().unwrap();
    assert!((depth - 0.6134 * 1.5).abs() < 1e-3, "{th}");
    let tp = run(&mut s, "cam.toolpath", json!({"operation": thread["id"], "limit": 200000}));
    let cuts: Vec<&Value> = tp["moves"].as_array().unwrap().iter().filter(|m| m[0] == "cut").collect();
    assert!(cuts.len() > 300, "{}", cuts.len());
    let rmin = cuts.iter().map(|m| (m[2].as_f64().unwrap().powi(2) + m[3].as_f64().unwrap().powi(2)).sqrt()).fold(f64::INFINITY, f64::min);
    assert!((rmin - (11.0 - depth)).abs() < 0.01, "deepest pass at {rmin}");
    let xs: Vec<f64> = cuts.iter().map(|m| m[1].as_f64().unwrap()).collect();
    assert!(xs.iter().all(|x| *x >= -25.0 - 1e-6 && *x <= -10.0 + 1e-6));
    assert_ne!(by(&profile["id"])["status"], "error");
    // The makera post writes the turns as A words with the tool vertical.
    let post = run(&mut s, "cam.post", json!({"setup": setup}));
    assert_eq!(post["post"], "makera");
    let code = post["gcode"].as_str().unwrap();
    assert!(code.lines().filter(|l| l.contains(" A")).count() > 300, "{}", &code[..600]);
}

#[test]
fn operations_are_parametric_steps_each_depending_on_the_ones_before() {
    let mut s = block();
    let flat = add_flat(&mut s, 6.0);
    let small = add_flat(&mut s, 3.0);
    let setup = run(&mut s, "cam.setup.create", json!({"name": "Top"}))["id"].as_u64().unwrap();
    let face =
        run(&mut s, "cam.operation.face", json!({"tool": flat, "stepover": 4, "heights": {"bottom": {"from": "model_top", "offset": 0}}}))["id"]
            .as_u64()
            .unwrap();
    let rough = run(&mut s, "cam.operation.pocket3d", json!({"tool": flat, "stepover": 3, "stepdown": 3}))["id"].as_u64().unwrap();
    let rest =
        run(&mut s, "cam.operation.pocket3d", json!({"tool": small, "stepover": 1.5, "stepdown": 3, "rest_machining": true}))["id"].as_u64().unwrap();
    let g = run(&mut s, "cam.generate", json!({}));
    assert_eq!(g["regenerated"], 3, "{g}");
    let stale = |s: &mut Session| -> Vec<bool> {
        run(s, "cam.notices", json!({}))["notices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["stale"].as_bool().unwrap_or_else(|| panic!("{n}")))
            .collect()
    };
    assert_eq!(stale(&mut s), vec![false, false, false]);
    // Nothing changed: generating again reuses every toolpath.
    let g = run(&mut s, "cam.generate", json!({}));
    assert_eq!((g["regenerated"].as_u64(), g["reused"].as_u64()), (Some(0), Some(3)), "{g}");
    // Regenerating one operation by itself makes every operation below it stale (their rest
    // machining read its old toolpath) and leaves those above it as they were.
    let g = run(&mut s, "cam.generate", json!({"operation": rough}));
    assert_eq!(g["regenerated"], 1, "{g}");
    assert_eq!(stale(&mut s), vec![false, false, true]);
    // The next Generate redoes only the stale one.
    let g = run(&mut s, "cam.generate", json!({}));
    assert_eq!((g["regenerated"].as_u64(), g["reused"].as_u64()), (Some(1), Some(2)), "{g}");
    assert_eq!(stale(&mut s), vec![false, false, false]);
    // From the first operation, everything below goes stale.
    run(&mut s, "cam.generate", json!({"operation": face}));
    assert_eq!(stale(&mut s), vec![false, true, true]);
    let g = run(&mut s, "cam.generate", json!({}));
    assert_eq!((g["regenerated"].as_u64(), g["reused"].as_u64()), (Some(2), Some(1)), "{g}");
    // all: true regenerates every operation, and each one below a regenerated one stays current.
    let g = run(&mut s, "cam.generate", json!({"all": true}));
    assert_eq!(g["regenerated"], 3, "{g}");
    assert_eq!(stale(&mut s), vec![false, false, false]);
    // Editing the second operation leaves the first as it was and makes it and the third stale;
    // posting now says so.
    run(&mut s, "cam.operation.set", json!({"operation": rough, "stepdown": 2}));
    assert_eq!(stale(&mut s), vec![false, true, true]);
    let post = run(&mut s, "cam.post", json!({}));
    assert!(post["notice"]["issues"].as_array().unwrap().iter().any(|i| i["code"] == "stale"), "{}", post["notice"]);
    let g = run(&mut s, "cam.generate", json!({}));
    assert_eq!((g["regenerated"].as_u64(), g["reused"].as_u64()), (Some(2), Some(1)), "{g}");
    assert_eq!(stale(&mut s), vec![false, false, false]);
    // A tool change reaches only the operations from the first that uses it.
    run(&mut s, "cam.tool.set", json!({"tool": small, "feeds": {"cutting": 500}}));
    assert_eq!(stale(&mut s), vec![false, false, true]);
    // Suppressing the first operation changes everything after it.
    run(&mut s, "cam.generate", json!({}));
    run(&mut s, "cam.operation.set", json!({"operation": face, "enabled": false}));
    let n = run(&mut s, "cam.notices", json!({}));
    let rows = n["notices"].as_array().unwrap();
    assert!(rows[1]["stale"] == true && rows[2]["stale"] == true, "{n}");
    run(&mut s, "cam.operation.set", json!({"operation": face, "enabled": true}));
    run(&mut s, "cam.generate", json!({}));
    // Reordering: the moved operation and those after its old and new places go stale.
    let m = run(&mut s, "cam.operation.move", json!({"operation": rest, "before": rough}));
    assert_eq!(m["order"], json!([face, rest, rough]), "{m}");
    assert_eq!(stale(&mut s), vec![false, true, true]);
    let m = run(&mut s, "cam.operation.move", json!({"operation": rest, "down": true}));
    assert_eq!(m["order"], json!([face, rough, rest]));
    // The model changing makes every operation stale.
    run(&mut s, "cam.generate", json!({}));
    run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [0, 0, 10], "operation": "join", "targets": ["Block"]}));
    assert_eq!(stale(&mut s), vec![true, true, true]);
    // Duplicating puts the copy right after the original.
    let d = run(&mut s, "cam.operation.duplicate", json!({"operation": face}));
    assert_eq!(d["index"], 1);
    let ops = run(&mut s, "cam.operation.list", json!({"setup": setup}));
    assert!(ops["operations"][1]["name"].as_str().unwrap().ends_with("(copy)"), "{ops}");
    // all: true regenerates every operation.
    let g = run(&mut s, "cam.generate", json!({"all": true}));
    assert_eq!(g["regenerated"], 4, "{g}");
}

#[test]
fn toolpaths_show_and_hide_on_the_workpiece() {
    let mut s = block();
    let flat = add_flat(&mut s, 6.0);
    let setup = run(&mut s, "cam.setup.create", json!({}))["id"].as_u64().unwrap();
    let a = run(&mut s, "cam.operation.face", json!({"tool": flat, "stepover": 4, "heights": {"bottom": {"from": "model_top", "offset": 0}}}))["id"]
        .as_u64()
        .unwrap();
    let b = run(&mut s, "cam.operation.contour2d", json!({"tool": flat, "side": "outside", "geometry": {"source": "silhouette"}}))["id"]
        .as_u64()
        .unwrap();
    // Nothing drawn before generating; both after.
    assert!(super::cam::shown_toolpaths(&s).is_empty());
    run(&mut s, "cam.generate", json!({}));
    let shown = |s: &Session| super::cam::shown_toolpaths(s).into_iter().map(|x| x.0).collect::<Vec<_>>();
    assert_eq!(shown(&s), vec![a, b]);
    // The eye hides one; the design is not changed and nothing goes on the undo stack.
    let undo = s.undo.len();
    let doc = s.doc.to_json();
    run(&mut s, "cam.toolpath.visibility", json!({"operation": a, "visible": false}));
    assert_eq!(shown(&s), vec![b]);
    assert_eq!((s.undo.len(), s.doc.to_json()), (undo, doc));
    // A setup's eye hides all of its operations; all: true shows every one again.
    run(&mut s, "cam.toolpath.visibility", json!({"setup": setup, "visible": false}));
    assert!(shown(&s).is_empty());
    run(&mut s, "cam.toolpath.visibility", json!({"all": true, "visible": true}));
    assert_eq!(shown(&s), vec![a, b]);
    // The simulation draws each operation's path only while its eye is on.
    run(&mut s, "cam.simulate.show", json!({"progress": 0.5}));
    let preview = s.cam.preview.clone().unwrap();
    let ops_in = |lines: &[(u64, Vec<solidcraft_geom::Vec3>)]| lines.iter().map(|(op, _)| *op).collect::<std::collections::BTreeSet<_>>();
    let mut all_ops = ops_in(&preview.path_done);
    all_ops.extend(ops_in(&preview.path_todo));
    assert_eq!(all_ops, [a, b].into_iter().collect(), "both operations have simulation paths");
    let cam = solidcraft_render::Camera::default();
    let lines_with_all = crate::view::scene(&s, &cam).lines.len();
    run(&mut s, "cam.toolpath.visibility", json!({"operation": a, "visible": false}));
    let lines_without_a = crate::view::scene(&s, &cam).lines.len();
    let a_lines = preview.path_done.iter().chain(&preview.path_todo).filter(|(op, _)| *op == a).count();
    assert_eq!(lines_with_all - lines_without_a, a_lines, "hiding the face drops exactly its simulation lines");
    run(&mut s, "cam.toolpath.visibility", json!({"operation": a, "visible": true}));
    run(&mut s, "cam.simulate.hide", json!({}));
    // A suppressed operation draws nothing, whatever its eye.
    run(&mut s, "cam.operation.set", json!({"operation": a, "enabled": false}));
    assert_eq!(shown(&s), vec![b]);
    // Bad requests are refused; a deleted operation leaves the hidden set.
    fails(&mut s, "cam.toolpath.visibility", json!({"visible": false}));
    fails(&mut s, "cam.toolpath.visibility", json!({"operation": 9999, "visible": false}));
    run(&mut s, "cam.toolpath.visibility", json!({"operation": b, "visible": false}));
    run(&mut s, "cam.operation.remove", json!({"operation": b}));
    assert!(s.cam.hidden.is_empty());
}

#[test]
fn regenerate_chain_brings_dependencies_and_dependents_current() {
    let mut s = block();
    let flat = add_flat(&mut s, 6.0);
    let small = add_flat(&mut s, 3.0);
    run(&mut s, "cam.setup.create", json!({}));
    let face =
        run(&mut s, "cam.operation.face", json!({"tool": flat, "stepover": 4, "heights": {"bottom": {"from": "model_top", "offset": 0}}}))["id"]
            .as_u64()
            .unwrap();
    let rough = run(&mut s, "cam.operation.pocket3d", json!({"tool": flat, "stepover": 3, "stepdown": 3}))["id"].as_u64().unwrap();
    let rest =
        run(&mut s, "cam.operation.pocket3d", json!({"tool": small, "stepover": 1.5, "stepdown": 3, "rest_machining": true}))["id"].as_u64().unwrap();
    let contour = run(&mut s, "cam.operation.contour2d", json!({"tool": flat, "side": "outside", "geometry": {"source": "silhouette"}}))["id"]
        .as_u64()
        .unwrap();
    let stale = |s: &mut Session| -> Vec<bool> {
        run(s, "cam.notices", json!({}))["notices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["stale"].as_bool().unwrap_or_else(|| panic!("{n}")))
            .collect()
    };
    // Nothing generated yet: the chain through the third regenerates everything (its
    // dependencies above it, it, and the one below).
    let g = run(&mut s, "cam.generate", json!({"operation": rest, "chain": true}));
    assert_eq!(g["regenerated"], 4, "{g}");
    assert_eq!(stale(&mut s), vec![false, false, false, false]);
    // The first edited: the chain through the third redoes the first (a stale dependency), the
    // second and third (below a regenerated one) and the fourth.
    run(&mut s, "cam.operation.set", json!({"operation": face, "stepover": 3}));
    let g = run(&mut s, "cam.generate", json!({"operation": rest, "chain": true}));
    assert_eq!((g["regenerated"].as_u64(), g["reused"].as_u64()), (Some(4), Some(0)), "{g}");
    // Everything current: the chain through the third keeps the two above it and regenerates
    // it and the one below.
    let g = run(&mut s, "cam.generate", json!({"operation": rest, "chain": true}));
    assert_eq!((g["regenerated"].as_u64(), g["reused"].as_u64()), (Some(2), Some(2)), "{g}");
    let ids: Vec<u64> =
        g["setups"][0]["operations"].as_array().unwrap().iter().filter(|o| o["reused"] != true).map(|o| o["id"].as_u64().unwrap()).collect();
    assert_eq!(ids, vec![rest, contour]);
    assert_eq!(stale(&mut s), vec![false, false, false, false]);
    let _ = rough;
}

#[test]
fn a_second_setup_turns_the_part_over_and_starts_from_what_the_first_left() {
    let mut s = block();
    // A 10 x 10 x 2 pocket in the bottom face too.
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 3, "corner": [5, 15, -1], "operation": "cut", "targets": ["Block"]}));
    let tool = add_flat(&mut s, 6.0);
    let small = add_flat(&mut s, 3.0);
    let top = run(
        &mut s,
        "cam.setup.create",
        json!({"name": "Top", "stock": {"mode": "relative_box", "x_neg": 2, "x_pos": 2, "y_neg": 2, "y_pos": 2, "z_pos": 1}}),
    );
    let top_id = top["id"].as_u64().unwrap();
    assert!(top["stock_from"].is_null());
    let face = run(
        &mut s,
        "cam.operation.face",
        json!({"setup": top_id, "tool": tool, "stepover": 4, "heights": {"bottom": {"from": "model_top", "offset": 0}}}),
    )["id"]
        .as_u64()
        .unwrap();
    run(&mut s, "cam.operation.pocket3d", json!({"setup": top_id, "tool": small, "stepover": 1.5, "stepdown": 3}));
    // The bottom: turned over about x, its stock what the top setup leaves (the default for a
    // later setup), its origin at the turned-over stock's front-left top corner.
    let bottom = run(&mut s, "cam.setup.create", json!({"name": "Bottom", "flip": "x"}));
    let bottom_id = bottom["id"].as_u64().unwrap();
    assert_eq!(bottom["stock"]["mode"], "from_setup");
    assert_eq!(bottom["stock_from"]["setup"], top_id);
    let r = &bottom["stock_resolved"];
    let num = |v: &Value| v.as_f64().unwrap();
    assert!(num(&r["min"][0]).abs() < 1e-9 && num(&r["min"][1]).abs() < 1e-9 && num(&r["max"][2]).abs() < 1e-9, "origin at the corner: {r}");
    assert!((num(&r["min"][2]) + 11.0).abs() < 1e-9, "the whole stock below the origin: {r}");
    assert!((num(&bottom["wcs"]["z"][2]) + 1.0).abs() < 1e-9, "tool axis turned over: {}", bottom["wcs"]);
    let rest = run(
        &mut s,
        "cam.operation.pocket3d",
        json!({"setup": bottom_id, "tool": small, "stepover": 1.5, "stepdown": 3, "rest_machining": true}),
    )["id"]
        .as_u64()
        .unwrap();
    let g = run(&mut s, "cam.generate", json!({}));
    assert_eq!(g["regenerated"], 3, "{g}");
    for st in g["setups"].as_array().unwrap() {
        for op in st["operations"].as_array().unwrap() {
            assert_ne!(op["status"], "error", "{op}");
        }
        assert!(st["notice"]["issues"].as_array().is_none_or(|i| i.iter().all(|x| x["code"] != "stock_from_setup")), "{st}");
    }
    // The bottom's stock is what the top left, and machining it does not gouge the part.
    let top_sim = run(&mut s, "cam.simulate", json!({"setup": top_id}));
    let bottom_sim = run(&mut s, "cam.simulate", json!({"setup": bottom_id}));
    let left = num(&top_sim["total"]["remaining_volume_mm3"]);
    let start = num(&bottom_sim["notice"]["values"]["initial_stock_mm3"]);
    assert!((start - left).abs() < left * 0.02, "carried over {start} vs left {left}");
    assert!(start < num(&top_sim["notice"]["values"]["initial_stock_mm3"]) - 500.0, "the top's cuts are gone from the bottom's stock");
    assert_eq!(bottom_sim["notice"]["values"]["stock_from_setup"], top_id);
    assert_eq!(bottom_sim["total"]["gouge_cells"], 0, "{}", bottom_sim["notice"]);
    // Parametric across setups: changing the top makes the bottom stale; generating brings both current.
    run(&mut s, "cam.operation.set", json!({"operation": face, "stepover": 3}));
    let n = run(&mut s, "cam.notices", json!({}));
    let stale = |n: &Value, id: u64| n["notices"].as_array().unwrap().iter().find(|o| o["operation"] == id).map(|o| o["stale"].clone());
    assert_eq!(stale(&n, rest), Some(json!(true)), "{n}");
    let g = run(&mut s, "cam.generate", json!({}));
    assert_eq!(g["regenerated"], 3, "the top's chain and the bottom after it: {g}");
    let n = run(&mut s, "cam.notices", json!({}));
    assert_eq!(stale(&n, rest), Some(json!(false)), "{n}");
    // Rest machining on the bottom never cuts more than without it (the top cleared the sides).
    let with_rest = s.cam.operations.get(&rest).unwrap().stats.time_s;
    run(&mut s, "cam.operation.set", json!({"operation": rest, "rest_machining": false}));
    run(&mut s, "cam.generate", json!({"setup": bottom_id}));
    let without = s.cam.operations.get(&rest).unwrap().stats.time_s;
    assert!(with_rest <= without + 1e-6, "rest {with_rest} s, without {without} s");
    // A turned-over setup with stock of its own also has its origin at the stock's corner.
    let own = run(
        &mut s,
        "cam.setup.create",
        json!({"name": "Own", "flip": "y", "stock": {"mode": "relative_box", "x_neg": 2, "x_pos": 2, "y_neg": 2, "y_pos": 2, "z_neg": 1}}),
    );
    let r = &own["stock_resolved"];
    assert!(num(&r["min"][0]).abs() < 1e-6 && num(&r["min"][1]).abs() < 1e-6 && num(&r["max"][2]).abs() < 1e-6, "{r}");
    // Stock comes only from an earlier setup.
    let e = fails(&mut s, "cam.setup.set", json!({"setup": bottom_id, "stock": {"mode": "from_setup", "setup": bottom_id}}));
    assert!(e.contains("not an earlier setup"), "{e}");
    let e = fails(&mut s, "cam.setup.set", json!({"setup": top_id, "stock": {"mode": "from_setup"}}));
    assert!(e.contains("is the first"), "{e}");
    // Removing the top hands its stock to the bottom, which is then the first setup.
    run(&mut s, "cam.setup.remove", json!({"setup": top_id}));
    let list = run(&mut s, "cam.setup.list", json!({}));
    assert_eq!(list["setups"][0]["stock"]["mode"], "relative_box", "{list}");
}

