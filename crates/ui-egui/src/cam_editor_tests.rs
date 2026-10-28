use serde_json::{Value, json};

use super::*;
use crate::scenario::Harness;

/// A block, a tool, a setup and a pocket operation; returns the operation's id.
fn pocket(h: &mut Harness) -> u64 {
    h.app.run("solid.box", json!({"length": 40, "width": 30, "height": 10, "body_name": "Block"})).unwrap();
    let tool = h.app.run("cam.tool.add", json!({"name": "6 mm flat", "kind": "flat", "diameter": 6})).unwrap()["id"].clone();
    h.app.run("cam.tool.add", json!({"name": "3 mm flat", "kind": "flat", "diameter": 3})).unwrap();
    h.app.run("cam.setup.create", json!({})).unwrap();
    h.app.run("cam.operation.pocket2d", json!({"name": "Pocket", "tool": tool, "stepover": 2, "geometry": {"source": "silhouette"}})).unwrap()["id"]
        .as_u64()
        .unwrap()
}

fn op(h: &Harness, id: u64) -> Value {
    serde_json::to_value(h.app.session.doc.manufacture.operation(id).unwrap().1).unwrap()
}

#[test]
fn fields_tab_shows_every_part_of_the_operation_and_apply_sets_it() {
    let mut h = Harness::new();
    let id = pocket(&mut h);
    crate::browser::cam::edit(&mut h.app, id);
    // The sections open with an animation: let the window settle before clicking.
    h.frames(20);
    for w in ["Fields", "JSON", "Name", "Type", "Tool", "Stepover (mm)", "Direction", "Parameters", "Heights", "Linking", "Geometry", "Apply", "OK"] {
        assert!(h.widget(w).is_some(), "{w} is drawn");
    }
    let undo = h.app.session.undo.len();
    let ed = h.app.cam_editor.as_mut().unwrap();
    assert_eq!(ed.tab, Tab::Fields);
    assert!(!ed.edited());
    ed.draft["stepover"] = json!(1.25);
    ed.draft["heights"]["bottom"] = json!({"from": "model_top", "offset": -3.0});
    h.frames(1);
    h.step(&json!({"widget": "Apply"})).unwrap();
    let now = op(&h, id);
    assert_eq!(now["stepover"], 1.25);
    assert_eq!(now["heights"]["bottom"]["offset"], -3.0);
    assert_eq!(h.app.session.undo.len(), undo + 1, "one undo step");
    let ed = h.app.cam_editor.as_ref().expect("Apply keeps the editor open");
    assert!(!ed.edited(), "the draft is the operation again");
}

#[test]
fn json_tab_carries_the_draft_both_ways_and_ok_applies_it() {
    let mut h = Harness::new();
    let id = pocket(&mut h);
    crate::browser::cam::edit(&mut h.app, id);
    // The sections open with an animation: let the window settle before clicking.
    h.frames(20);
    h.app.cam_editor.as_mut().unwrap().draft["stepover"] = json!(1.5);
    h.step(&json!({"widget": "JSON"})).unwrap();
    h.frames(20);
    let ed = h.app.cam_editor.as_mut().unwrap();
    assert_eq!(ed.tab, Tab::Json);
    let v: Value = serde_json::from_str(&ed.text).unwrap();
    assert_eq!(v["stepover"], 1.5, "the field edit is in the text");
    // A text that does not parse keeps the JSON tab and blocks Apply.
    ed.text = "{ not json".into();
    switch(ed, Tab::Fields);
    assert_eq!(ed.tab, Tab::Json);
    assert!(ed.blocked().is_some());
    // A valid text comes back to the fields.
    let mut v2 = v.clone();
    v2["stepdown"] = json!(0.75);
    ed.text = serde_json::to_string_pretty(&v2).unwrap();
    switch(ed, Tab::Fields);
    assert_eq!(ed.tab, Tab::Fields);
    assert_eq!(ed.draft["stepdown"], 0.75);
    // OK from the JSON tab applies its text and closes.
    switch(ed, Tab::Json);
    v2["stepdown"] = json!(0.5);
    ed.text = serde_json::to_string_pretty(&v2).unwrap();
    h.frames(1);
    h.step(&json!({"widget": "OK"})).unwrap();
    assert!(h.app.cam_editor.is_none(), "OK closes");
    let now = op(&h, id);
    assert_eq!(now["stepdown"], 0.5);
    assert_eq!(now["stepover"], 1.5);
}

#[test]
fn a_refused_apply_keeps_the_draft_and_says_why() {
    let mut h = Harness::new();
    let id = pocket(&mut h);
    crate::browser::cam::edit(&mut h.app, id);
    h.frames(1);
    let mut ed = h.app.cam_editor.take().unwrap();
    ed.draft["tool"] = json!(999);
    assert!(!apply(&mut h.app, &mut ed));
    assert!(ed.message.as_ref().is_some_and(|m| m.1), "{:?}", ed.message);
    assert_eq!(ed.draft["tool"], 999, "the draft stays for fixing");
}

#[test]
fn an_untouched_draft_follows_changes_made_elsewhere() {
    let mut h = Harness::new();
    let id = pocket(&mut h);
    crate::browser::cam::edit(&mut h.app, id);
    h.frames(1);
    // An assistant sets the JSON directly.
    h.app.run("cam.operation.set", json!({"operation": id, "stepover": 2.75})).unwrap();
    h.frames(1);
    assert_eq!(h.app.cam_editor.as_ref().unwrap().draft["stepover"], 2.75);
    // An edited draft is kept, with Reload offered.
    h.app.cam_editor.as_mut().unwrap().draft["stepover"] = json!(1.0);
    h.app.run("cam.operation.set", json!({"operation": id, "stepover": 3.0})).unwrap();
    h.frames(20);
    assert_eq!(h.app.cam_editor.as_ref().unwrap().draft["stepover"], 1.0);
    assert!(h.widget("Reload").is_some());
    h.step(&json!({"widget": "Reload"})).unwrap();
    h.frames(1);
    assert_eq!(h.app.cam_editor.as_ref().unwrap().draft["stepover"], 3.0);
    // Deleting the operation closes the editor.
    h.app.run("cam.operation.remove", json!({"operation": id})).unwrap();
    h.frames(1);
    assert!(h.app.cam_editor.is_none());
}

#[test]
fn changing_the_kind_keeps_the_common_parts() {
    let mut h = Harness::new();
    let id = pocket(&mut h);
    let mut v = op(&h, id);
    let Value::Object(m) = &mut v else { panic!("an object") };
    change_kind(m, "drill");
    assert_eq!(m["type"], "drill");
    assert!(m.contains_key("cycle") && !m.contains_key("stepover"), "{m:?}");
    assert_eq!(m["name"], "Pocket");
    assert!(m.contains_key("heights") && m.contains_key("linking"));
}

#[test]
fn values_read_and_type_as_people_write_them() {
    assert_eq!(label_of("stay_down_distance", &json!(0.0)), "Stay down distance (mm)");
    assert_eq!(label_of("angle_deg", &json!(2.0)), "Angle (°)");
    assert_eq!(label_of("passes", &json!(3)), "Passes");
    assert_eq!(kind_label("pocket2d"), "Pocket 2D");
    assert_eq!(kind_label("steep_shallow"), "Steep shallow");
    assert_eq!(number_text(&json!(0.30000000000000004)), "0.3");
    assert_eq!(parse_number("2,5", false), Some(json!(2.5)));
    assert_eq!(parse_number("4 mm", false), Some(json!(4.0)));
    assert_eq!(parse_number("2.5", true), None, "a count is whole");
    assert_eq!(parse_point("1, 2, 3"), Some(json!([1.0, 2.0, 3.0])));
    assert_eq!(parse_point(""), Some(Value::Null));
    assert_eq!(parse_point("1"), None);
    assert_eq!(new_item("geometry.points", None), json!([0.0, 0.0, 0.0]));
    assert_eq!(new_item("geometry.loops", None), json!([]));
    assert_eq!(new_item("geometry.loops.0", None), json!([0.0, 0.0]));
    assert_eq!(new_item("geometry.loops", Some(&json!([[1.0, 1.0], [2.0, 1.0]]))), json!([]));
    assert_eq!(new_item("geometry.points", Some(&json!([4.0, 5.0, 6.0]))), json!([4.0, 5.0, 6.0]));
}

#[test]
fn picked_points_are_rows_to_edit_add_and_remove() {
    let mut h = Harness::new();
    h.app.run("solid.box", json!({"length": 60, "width": 40, "height": 15})).unwrap();
    h.app.run("cam.tool.add", json!({"name": "2 mm drill", "kind": "drill", "diameter": 2})).unwrap();
    h.app.run("cam.setup.create", json!({})).unwrap();
    let id = h
        .app
        .run("cam.operation.drill", json!({"geometry": {"source": "holes", "points": [[10, 10, 15], [50, 10, 15]], "diameter": 2}}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    crate::browser::cam::edit(&mut h.app, id);
    // Fold the parameters away so the geometry section is in view, then open it.
    h.frames(20);
    h.step(&json!({"widget": "Parameters"})).unwrap();
    h.frames(20);
    h.step(&json!({"widget": "Geometry"})).unwrap();
    h.frames(20);
    assert!(h.widget("Points (2)").is_some(), "the points are a list");
    assert!(h.widget("10, 10, 15").is_some(), "each point is a field");
    h.step(&json!({"widget": "Add"})).unwrap();
    h.frames(2);
    assert_eq!(h.app.cam_editor.as_ref().unwrap().draft["geometry"]["points"].as_array().map(Vec::len), Some(3));
    h.step(&json!({"widget": "Apply"})).unwrap();
    assert_eq!(op(&h, id)["geometry"]["points"][2], json!([50.0, 10.0, 15.0]), "Add repeats the last point");
}

fn tool(h: &Harness, id: u64) -> Value {
    serde_json::to_value(h.app.session.doc.manufacture.tools.get(id).unwrap()).unwrap()
}

#[test]
fn tools_open_in_the_same_editor_and_apply_through_tool_set() {
    let mut h = Harness::new();
    let op = pocket(&mut h);
    let id = h.app.session.doc.manufacture.operation(op).unwrap().1.tool;
    crate::browser::cam::edit_tool(&mut h.app, id);
    h.frames(20);
    let ed = h.app.cam_editor.as_ref().unwrap();
    assert_eq!(ed.target, Target::Tool(id));
    for w in ["Fields", "JSON", "Name", "Tool number (T)", "Kind", "Size", "Diameter (mm)", "Feeds and speeds", "Spindle speed (rev/min)", "Coolant"]
    {
        assert!(h.widget(w).is_some(), "{w} is drawn");
    }
    assert!(h.widget("Used by 1 operation: a change makes their toolpaths stale.").is_some());
    let undo = h.app.session.undo.len();
    let ed = h.app.cam_editor.as_mut().unwrap();
    ed.draft["feeds"]["spindle_rpm"] = json!(12000.0);
    ed.draft["number"] = json!(4);
    h.frames(1);
    h.step(&json!({"widget": "Apply"})).unwrap();
    let now = tool(&h, id);
    assert_eq!(now["feeds"]["spindle_rpm"], 12000.0);
    assert_eq!(now["number"], 4);
    assert_eq!(h.app.session.undo.len(), undo + 1, "one undo step");
    // A new kind with its own size, from the JSON draft.
    let ed = h.app.cam_editor.as_mut().unwrap();
    switch(ed, Tab::Json);
    let mut v: Value = serde_json::from_str(&ed.text).unwrap();
    v["kind"] = json!("bull_nose");
    v["corner_radius"] = json!(0.5);
    ed.text = serde_json::to_string_pretty(&v).unwrap();
    h.frames(1);
    h.step(&json!({"widget": "OK"})).unwrap();
    assert!(h.app.cam_editor.is_none(), "OK closes");
    let now = tool(&h, id);
    assert_eq!((now["kind"].as_str(), now["corner_radius"].as_f64()), (Some("bull_nose"), Some(0.5)));
}

#[test]
fn a_tool_kind_brings_its_own_sizes() {
    assert_eq!(tool_kind_default("drill", 3.0), json!({"tip_angle_deg": 118.0}));
    assert_eq!(tool_kind_default("bull_nose", 6.0)["corner_radius"], json!(0.6000000000000001));
    assert_eq!(tool_kind_default("flat_end", 6.0), json!({}));
    let kinds = cam::ops::variants::<cam::tool::ToolKind>(json!({"kind": "\u{1}"}));
    for k in ["flat_end", "ball_end", "bull_nose", "chamfer", "drill", "laser", "tapered_ball"] {
        assert!(kinds.iter().any(|x| x == k), "{k} in {kinds:?}");
    }
    assert_eq!(label_of("wavelength_nm", &json!(445.0)), "Wavelength (nm)");
}

#[test]
fn setups_open_with_stock_origin_and_machine_and_turn_over() {
    let mut h = Harness::new();
    pocket(&mut h);
    let top = h.app.session.doc.manufacture.setups[0].id;
    let bottom = h.app.run("cam.setup.create", json!({"name": "Bottom"})).unwrap()["id"].as_u64().unwrap();
    crate::browser::cam::edit_setup(&mut h.app, bottom);
    h.frames(20);
    assert_eq!(h.app.cam_editor.as_ref().unwrap().target, Target::Setup(bottom));
    for w in ["Fields", "JSON", "Name", "Stock", "Origin and axes", "Turn Over About X", "Machine", "Bodies"] {
        assert!(h.widget(w).is_some(), "{w} is drawn");
    }
    let top_name = h.app.session.doc.manufacture.setups[0].name.clone();
    assert!(h.widget(&format!("Starts from what `{top_name}` leaves after its operations (setup 0: the one before).")).is_some());
    assert!(!h.app.cam_editor.as_ref().unwrap().draft.as_object().unwrap().contains_key("operations"), "operations are edited on their own");
    // Turning over applies at once: the tool axis points down and the origin is the corner.
    h.step(&json!({"widget": "Turn Over About X"})).unwrap();
    h.frames(2);
    let st = h.app.session.doc.manufacture.setup(bottom).unwrap().clone();
    assert!((st.wcs.z.z + 1.0).abs() < 1e-9, "{:?}", st.wcs);
    assert_eq!(h.app.cam_editor.as_ref().unwrap().draft["wcs"]["z"], json!([0.0, 0.0, -1.0]));
    // A machine preset from the draft, applied through cam.setup.set.
    let ed = h.app.cam_editor.as_mut().unwrap();
    ed.draft["machine"] = serde_json::to_value(cam::machine::Machine::preset("carvera").unwrap()).unwrap();
    h.frames(1);
    h.step(&json!({"widget": "Apply"})).unwrap();
    assert_eq!(h.app.session.doc.manufacture.setup(bottom).unwrap().machine.post, "makera");
    assert_eq!(h.app.session.doc.manufacture.setups[0].id, top, "the first setup is untouched");
}

#[test]
fn area_operations_show_their_boundary() {
    let mut h = Harness::new();
    pocket(&mut h);
    let tool = h.app.session.doc.manufacture.tools.tools[0].id;
    let id = h.app.run("cam.operation.adaptive3d", json!({"tool": tool})).unwrap()["id"].as_u64().unwrap();
    crate::browser::cam::edit(&mut h.app, id);
    h.frames(20);
    for w in ["Boundary", "Machine within", "Tool", "Additional offset (mm)", "Stay within the stock", "Keep clear of fixtures"] {
        assert!(h.widget(w).is_some(), "{w} is drawn");
    }
    let ed = h.app.cam_editor.as_mut().unwrap();
    assert_eq!(ed.draft["boundary"]["source"], "auto");
    ed.draft["boundary"]["source"] = json!("bounding_box");
    ed.draft["boundary"]["containment"] = json!("center");
    h.frames(1);
    h.step(&json!({"widget": "Apply"})).unwrap();
    let b = h.app.session.doc.manufacture.operation(id).unwrap().1.boundary;
    assert_eq!((b.source, b.containment), (cam::boundary::Source::BoundingBox, cam::boundary::Containment::Center));
}

#[test]
fn a_selection_boundary_takes_what_is_selected_in_the_viewport() {
    let mut h = Harness::new();
    pocket(&mut h);
    let tool = h.app.session.doc.manufacture.tools.tools[0].id;
    let id = h.app.run("cam.operation.pocket3d", json!({"tool": tool, "boundary": "selection"})).unwrap()["id"].as_u64().unwrap();
    crate::browser::cam::edit(&mut h.app, id);
    h.frames(20);
    // Nothing picked yet: the editor says so.
    assert!(h.widget("Nothing is picked: select faces or a sketch in the viewport, then Use Selected").is_some());
    // The top face picked in the viewport: Use Selected makes it the geometry and the boundary.
    h.app.run("select.set", json!({"items": [{"type": "face", "body": "Block", "index": 0, "point": [20.0, 15.0, 10.0]}]})).unwrap();
    h.frames(2);
    h.step(&json!({"widget": "Use Selected (1 face)"})).unwrap();
    h.frames(2);
    let ed = h.app.cam_editor.as_mut().unwrap();
    assert_eq!(ed.draft["geometry"], json!({"source": "faces", "points": [[20.0, 15.0, 10.0]]}));
    assert_eq!(ed.draft["boundary"]["source"], "selection");
    h.step(&json!({"widget": "Apply"})).unwrap();
    let saved = op(&h, id);
    assert_eq!(saved["geometry"]["source"], "faces");
    assert!(h.widget("Selection: 1 face").is_some());
    // A new operation opens in the same editor, its geometry from the selection, for the setup
    // picked in the Browser; OK adds it there.
    let second = h.app.run("cam.setup.create", json!({"name": "Second"})).unwrap()["id"].as_u64().unwrap();
    h.app.tree.picked_cam = Some(crate::browser::cam::CamPick::Setup(second));
    h.app.start("cam.operation.scallop");
    h.frames(20);
    let ed = h.app.cam_editor.as_ref().expect("the editor opens on the new operation");
    assert_eq!(ed.target, Target::NewOperation(second));
    assert_eq!(ed.draft["type"], "scallop");
    assert_eq!(ed.draft["geometry"], json!({"source": "faces", "points": [[20.0, 15.0, 10.0]]}));
    for w in ["Fields", "JSON", "Setup", "Boundary", "Selection: 1 face"] {
        assert!(h.widget(w).is_some(), "{w} is drawn");
    }
    h.step(&json!({"widget": "OK"})).unwrap();
    assert!(h.app.cam_editor.is_none(), "OK adds it and closes");
    let st = h.app.session.doc.manufacture.setup(second).unwrap();
    assert_eq!(st.operations.len(), 1, "added to the picked setup");
    assert_eq!(st.operations[0].kind.type_name(), "scallop");
    // Drilling kinds take picked faces as holes.
    h.app.start("cam.operation.drill");
    assert_eq!(h.app.cam_editor.as_ref().unwrap().draft["geometry"]["source"], "holes");
}

#[test]
fn operations_drag_to_reorder_and_into_another_setup() {
    let mut h = Harness::new();
    let first = pocket(&mut h);
    let tool = h.app.session.doc.manufacture.tools.tools[0].id;
    let second = h.app.run("cam.operation.face", json!({"name": "Face", "tool": tool, "stepover": 4})).unwrap()["id"].as_u64().unwrap();
    let other = h.app.run("cam.setup.create", json!({"name": "Bottom"})).unwrap()["id"].as_u64().unwrap();
    h.app.ui.tab = crate::manufacture_workspace::TAB.into();
    h.settle();
    let ids = |h: &Harness, k: usize| -> Vec<u64> { h.app.session.doc.manufacture.setups[k].operations.iter().map(|o| o.id).collect() };
    assert_eq!(ids(&h, 0), vec![first, second]);
    // Onto the first: before it.
    h.step(&json!({"drag": [{"handle": "row:2. Face"}, {"handle": "row:1. Pocket"}]})).unwrap();
    h.settle();
    assert_eq!(ids(&h, 0), vec![second, first], "reordered by dragging");
    // Onto the other setup: at its end.
    h.step(&json!({"drag": [{"handle": "row:2. Pocket"}, {"handle": "row:Bottom"}]})).unwrap();
    h.settle();
    assert_eq!((ids(&h, 0), ids(&h, 1)), (vec![second], vec![first]), "moved into the other setup");
    assert_eq!(h.app.session.doc.manufacture.setups[1].id, other);
    // The tree: the machine holds its setups and the tools.
    for w in ["row:Bottom", "row:Tools"] {
        assert!(crate::scenario::handle_at(w).is_some() || h.widget(&w[4..]).is_some(), "{w}");
    }
}
