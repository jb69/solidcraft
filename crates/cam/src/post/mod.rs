//! Post processors: toolpaths to G-code. A [`Post`] describes a control's dialect (words,
//! codes, formatting, what it supports); [`post_program`] writes a whole setup's program:
//! header, one section per operation (tool change, spindle, coolant, moves, canned cycles
//! where wanted) and the end. Multi-axis moves go through the machine's kinematics
//! ([`crate::machine::post_kinematics`]) to get the rotary words.

use serde::{Deserialize, Serialize};
use solidcraft_geom::Vec3;

use crate::machine::{Kinematics, Machine, post_kinematics};
use crate::notice::{Issue, Notice, r3};
use crate::tool::{Coolant, Tool};
use crate::toolpath::{DrillCycleKind, MoveKind, Toolpath};
use crate::{CamError, Result};

/// A control's dialect.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Post {
    /// Identifier (`generic`, `grbl`, `fiveaxis`).
    pub id: String,
    pub name: String,
    pub description: String,
    /// Decimal places for lengths and angles.
    pub decimals: usize,
    /// Line numbers (`N10`, `N20`, …): 0 for none, else the increment.
    pub line_numbers: u32,
    /// Emit canned drilling cycles (G81/G82/G83/G73/G85) instead of expanded moves.
    pub canned_cycles: bool,
    /// Tool changes (`T1 M6`); off for machines with one tool (routers).
    pub tool_changes: bool,
    /// Tool length offsets (`G43 H1`).
    pub length_offsets: bool,
    /// Coolant codes.
    pub coolant: bool,
    /// Code turning tool-centre-point control on and off (TCP/RTCP), when the machine has it.
    pub tcp_on: String,
    pub tcp_off: String,
    /// Positioning rapids as `G0` (true) or as `G1` at the rapid feed (false).
    pub g0_rapids: bool,
    /// Comments: `(text)` style (true) or `;text` (false).
    pub paren_comments: bool,
    /// Lines placed at the start and the end.
    pub header: Vec<String>,
    pub footer: Vec<String>,
    /// Program end code.
    pub end: String,
    /// Dwell word: `P` seconds (true) or `P` milliseconds (false).
    pub dwell_seconds: bool,
    /// Largest number of axes the control takes.
    pub max_axes: usize,
    /// The tool change line, with `{n}` for the tool number.
    pub tool_change: String,
    /// The line that parks the spindle before a tool change and at the end.
    pub park: String,
    /// Coolant codes for flood and for mist/air (empty: none).
    pub flood_code: String,
    pub mist_code: String,
    /// `%` lines around the program.
    pub percent_lines: bool,
    /// File extension the control's software opens.
    pub extension: String,
}

impl Default for Post {
    fn default() -> Self {
        Post::generic()
    }
}

impl Post {
    /// RS-274 as most controls read it (LinuxCNC, Mach, Fanuc-style): mm, absolute, canned
    /// cycles, tool changes.
    pub fn generic() -> Post {
        Post {
            id: "generic".into(),
            name: "Generic ISO (3-axis)".into(),
            description: "RS-274 G-code for most 3-axis controls: G21 G90 G17, T M6, G43 H, canned drilling cycles, M3/M8".into(),
            decimals: 3,
            line_numbers: 0,
            canned_cycles: true,
            tool_changes: true,
            length_offsets: true,
            coolant: true,
            tcp_on: String::new(),
            tcp_off: String::new(),
            g0_rapids: true,
            paren_comments: true,
            header: vec!["G90 G94 G17 G21".into(), "G54".into()],
            footer: Vec::new(),
            end: "M30".into(),
            dwell_seconds: true,
            max_axes: 3,
            tool_change: "T{n} M6".into(),
            park: "G53 G0 Z0".into(),
            flood_code: "M8".into(),
            mist_code: "M7".into(),
            percent_lines: true,
            extension: "nc".into(),
        }
    }

    /// Makera Carvera, Carvera Air and Z1 (a Smoothieware-based control, as Makera documents
    /// it): millimetres only, `M6 T` tool changes with the control measuring the tool length
    /// itself (no `G43`), `M7`/`M9` for the air blast, no canned cycles, `;` comments, `M2`
    /// to end. The A word drives the 4th-axis module.
    pub fn makera() -> Post {
        Post {
            id: "makera".into(),
            name: "Makera Carvera / Carvera Air / Z1".into(),
            description: "Makera's control: mm only, M6 T tool changes (the control measures tool length), M7/M9 air, A axis for the 4th-axis module, no canned cycles, ; comments, M2".into(),
            canned_cycles: false,
            tool_changes: true,
            length_offsets: false,
            paren_comments: false,
            header: vec!["G21 G90 G94 G17".into(), "G54".into()],
            end: "M2".into(),
            max_axes: 4,
            tool_change: "M6 T{n}".into(),
            park: "G28".into(),
            flood_code: "M7".into(),
            mist_code: "M7".into(),
            percent_lines: false,
            extension: "cnc".into(),
            ..Post::generic()
        }
    }

    /// Grbl and similar hobby controls: no tool changes or length offsets, no canned cycles,
    /// `;` comments.
    pub fn grbl() -> Post {
        Post {
            id: "grbl".into(),
            name: "Grbl (3-axis router)".into(),
            description: "Grbl-style G-code: no tool changer, no canned cycles, M3 S, M8/M9, ; comments".into(),
            canned_cycles: false,
            tool_changes: false,
            length_offsets: false,
            paren_comments: false,
            header: vec!["G90 G94 G17 G21".into()],
            end: "M30".into(),
            max_axes: 3,
            ..Post::generic()
        }
    }

    /// A 4- or 5-axis control with A/B/C words, TCP through `G43.4` / `G49` (LinuxCNC style).
    pub fn fiveaxis() -> Post {
        Post {
            id: "fiveaxis".into(),
            name: "Generic 5-axis (A/B/C words, G43.4 TCP)".into(),
            description: "RS-274 with rotary words; tool-centre-point control with G43.4 when the machine has it, else machine coordinates linearised by the post".into(),
            tcp_on: "G43.4".into(),
            tcp_off: "G49".into(),
            header: vec!["G90 G94 G17 G21".into(), "G54".into()],
            max_axes: 5,
            ..Post::generic()
        }
    }

    pub fn builtin(id: &str) -> Option<Post> {
        match id {
            "generic" => Some(Post::generic()),
            "grbl" => Some(Post::grbl()),
            "fiveaxis" => Some(Post::fiveaxis()),
            "makera" | "carvera" => Some(Post::makera()),
            _ => None,
        }
    }

    pub const BUILTIN: &'static [&'static str] = &["generic", "grbl", "fiveaxis", "makera"];

    fn comment(&self, text: &str) -> String {
        let clean: String = text.chars().filter(|c| !matches!(c, '(' | ')' | ';' | '\n' | '\r')).collect();
        if self.paren_comments { format!("({clean})") } else { format!(";{clean}") }
    }

    fn num(&self, v: f64) -> String {
        let v = if v.abs() < 0.5 * 10f64.powi(-(self.decimals as i32)) { 0.0 } else { v };
        let s = format!("{:.*}", self.decimals, v);
        // Trim trailing zeros but keep one decimal digit.
        let t = s.trim_end_matches('0');
        if t.ends_with('.') { format!("{t}0") } else { t.to_string() }
    }
}

/// One operation to post.
pub struct PostOperation<'a> {
    pub name: &'a str,
    pub tool: &'a Tool,
    pub toolpath: &'a Toolpath,
}

/// The output of a post.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub gcode: String,
    pub lines: usize,
    pub notice: Notice,
}

/// Modal state while writing.
struct Writer<'a> {
    post: &'a Post,
    out: String,
    lines: usize,
    n: u32,
    last_xyz: Option<Vec3>,
    last_abc: Option<(f64, f64)>,
    last_feed: Option<f64>,
    last_g: Option<&'static str>,
}

impl Writer<'_> {
    fn line(&mut self, body: &str) {
        if body.is_empty() {
            return;
        }
        if self.post.line_numbers > 0 {
            self.n += self.post.line_numbers;
            self.out.push_str(&format!("N{} ", self.n));
        }
        self.out.push_str(body);
        self.out.push('\n');
        self.lines += 1;
    }

    fn comment(&mut self, text: &str) {
        let c = self.post.comment(text);
        self.line(&c);
    }

    /// A motion line with only the words that changed.
    fn motion(&mut self, g: &'static str, xyz: Vec3, abc: Option<(char, f64, char, f64)>, feed: Option<f64>) {
        let mut words: Vec<String> = Vec::new();
        if self.last_g != Some(g) {
            words.push(g.to_string());
            self.last_g = Some(g);
        }
        let p = self.post;
        let changed = |a: f64, b: Option<f64>| b.is_none_or(|b| p.num(a) != p.num(b));
        if changed(xyz.x, self.last_xyz.map(|l| l.x)) {
            words.push(format!("X{}", p.num(xyz.x)));
        }
        if changed(xyz.y, self.last_xyz.map(|l| l.y)) {
            words.push(format!("Y{}", p.num(xyz.y)));
        }
        if changed(xyz.z, self.last_xyz.map(|l| l.z)) {
            words.push(format!("Z{}", p.num(xyz.z)));
        }
        if let Some((n1, a, n2, b)) = abc {
            if changed(a, self.last_abc.map(|l| l.0)) {
                words.push(format!("{n1}{}", p.num(a)));
            }
            if n2 != ' ' && changed(b, self.last_abc.map(|l| l.1)) {
                words.push(format!("{n2}{}", p.num(b)));
            }
            self.last_abc = Some((a, b));
        }
        if let Some(f) = feed
            && self.last_feed.is_none_or(|l| p.num(l) != p.num(f))
        {
            words.push(format!("F{}", p.num(f)));
            self.last_feed = Some(f);
        }
        self.last_xyz = Some(xyz);
        if words.len() == 1 && words[0] == g {
            return; // nothing moved
        }
        let body = words.join(" ");
        self.line(&body);
    }
}

/// Writes the program of one setup.
pub fn post_program(post: &Post, machine: &Machine, program_name: &str, ops: &[PostOperation], tolerance: f64) -> Result<Program> {
    machine.validate()?;
    let mut notice = Notice::default();
    let axes_needed = ops.iter().flat_map(|o| o.toolpath.moves.iter()).any(|m| (m.axis - Vec3::new(0.0, 0.0, 1.0)).len() > 1e-6);
    if axes_needed && post.max_axes < 4 {
        return Err(CamError::Machine(format!(
            "the `{}` post writes 3-axis programs but the toolpath tilts the tool; use a multi-axis post",
            post.id
        )));
    }
    if axes_needed && machine.axis_count() < 4 {
        return Err(CamError::Machine(
            "the setup's machine has three axes but the toolpath tilts the tool; set a 4- or 5-axis machine on the setup".into(),
        ));
    }
    let mut w = Writer { post, out: String::new(), lines: 0, n: 0, last_xyz: None, last_abc: None, last_feed: None, last_g: None };
    let name: String = program_name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == ' ').collect();
    if post.percent_lines {
        w.line("%");
    }
    w.comment(&format!("{} - SolidCraft CAM", if name.is_empty() { "program" } else { &name }));
    w.comment(&format!("machine: {}", machine.name));
    let mut listed: Vec<u32> = Vec::new();
    for op in ops {
        if !listed.contains(&op.tool.number) {
            listed.push(op.tool.number);
            w.comment(&format!("T{} {} D={} {}", op.tool.number, op.tool.name, post.num(op.tool.diameter), tool_kind_name(op.tool)));
        }
    }
    for h in &post.header {
        w.line(h);
    }
    let rot = machine.rotaries();
    let words: Vec<char> = rot.iter().map(|r| r.name).collect();
    let mut tcp_active = false;
    let mut prev_tool: Option<u32> = None;
    let mut total_moves = 0usize;
    // What the machine objects to: spindle speeds and feeds past its own are written at the
    // maximum, with a warning each; a shank no collet takes and a toolpath past the work area
    // are reported.
    let mut seen_tools: Vec<u32> = Vec::new();
    for op in ops {
        if !seen_tools.contains(&op.tool.number) {
            seen_tools.push(op.tool.number);
            for issue in machine.check(op.tool, op.toolpath) {
                notice.issue(issue);
            }
        } else {
            for issue in machine.check(op.tool, op.toolpath).into_iter().filter(|i| i.code == "work_area" || i.code == "rotary_capacity") {
                notice.issue(issue);
            }
        }
    }
    if let crate::machine::ToolChanger::Automatic { slots } = machine.tool_changer {
        let beyond: Vec<String> = seen_tools.iter().filter(|n| **n > slots).map(|n| format!("T{n}")).collect();
        if !beyond.is_empty() {
            notice.issue(Issue::info(
                "manual_tool_change",
                format!("{} tool(s) beyond the changer's {slots} slots ({}): the control stops and asks for them by hand", beyond.len(), beyond.join(", ")),
            ));
        }
    } else if machine.tool_changer == crate::machine::ToolChanger::Manual && seen_tools.len() > 1 {
        notice.issue(Issue::info("manual_tool_change", format!("{} tool changes by hand: the control stops and asks for each tool", seen_tools.len() - 1)));
    }
    for op in ops {
        let tp = op.toolpath;
        if tp.is_empty() {
            w.comment(&format!("operation {}: no toolpath", op.name));
            continue;
        }
        w.comment(&format!("operation: {}", op.name));
        let f = &op.tool.feeds;
        if prev_tool != Some(op.tool.number) {
            if prev_tool.is_some() {
                w.line("M5");
                if post.coolant {
                    w.line("M9");
                }
                if !post.park.is_empty() {
                    w.line(&post.park.clone());
                }
                w.last_xyz = None;
                w.last_g = None;
            }
            if post.tool_changes {
                let line = post.tool_change.replace("{n}", &op.tool.number.to_string());
                w.line(&line);
            }
            if post.length_offsets {
                let h = if op.tool.length_offset > 0 { op.tool.length_offset } else { op.tool.number };
                w.line(&format!("G43 H{h}"));
            }
            let (rpm, _) = machine.clamp_rpm(f.spindle_rpm);
            w.line(&format!("S{} {}", (rpm.round() as i64).max(0), if f.clockwise { "M3" } else { "M4" }));
            if post.coolant {
                let code = match f.coolant {
                    Coolant::Flood => Some(post.flood_code.as_str()),
                    Coolant::Mist | Coolant::Air | Coolant::ThroughTool => Some(post.mist_code.as_str()),
                    Coolant::Off => None,
                };
                if let Some(c) = code.filter(|c| !c.is_empty()) {
                    w.line(c);
                }
            }
            prev_tool = Some(op.tool.number);
        }
        let multi = tp.moves.iter().any(|m| (m.axis - Vec3::new(0.0, 0.0, 1.0)).len() > 1e-6);
        if multi && machine.tcp && !post.tcp_on.is_empty() && !tcp_active {
            w.line(&post.tcp_on.clone());
            tcp_active = true;
        }
        if !multi && tcp_active && !post.tcp_off.is_empty() {
            w.line(&post.tcp_off.clone());
            tcp_active = false;
        }
        // Canned cycles: when the post supports them and the operation is drilling.
        let mut canned_done = false;
        if post.canned_cycles && !tp.cycles.is_empty() && !multi {
            let kinds: std::collections::BTreeSet<DrillCycleKind> = tp.cycles.iter().map(|c| c.kind).collect();
            for kind in kinds {
                let code = match kind {
                    DrillCycleKind::Simple => "G81",
                    DrillCycleKind::Dwell => "G82",
                    DrillCycleKind::Peck => "G83",
                    DrillCycleKind::ChipBreak => "G73",
                    DrillCycleKind::FeedOut => "G85",
                };
                let mut first = true;
                for c in tp.cycles.iter().filter(|c| c.kind == kind) {
                    let bottom = c.top.z - c.depth;
                    if first {
                        w.line("G98");
                        let mut words = vec![
                            code.to_string(),
                            format!("X{}", post.num(c.top.x)),
                            format!("Y{}", post.num(c.top.y)),
                            format!("Z{}", post.num(bottom)),
                            format!("R{}", post.num(c.retract_height)),
                        ];
                        if matches!(kind, DrillCycleKind::Peck | DrillCycleKind::ChipBreak) {
                            words.push(format!("Q{}", post.num(c.peck)));
                        }
                        if kind == DrillCycleKind::Dwell {
                            words.push(format!(
                                "P{}",
                                if post.dwell_seconds { post.num(c.dwell_s) } else { ((c.dwell_s * 1000.0).round() as i64).to_string() }
                            ));
                        }
                        words.push(format!("F{}", post.num(c.feed)));
                        let body = words.join(" ");
                        w.line(&body);
                        first = false;
                    } else {
                        w.line(&format!("X{} Y{}", post.num(c.top.x), post.num(c.top.y)));
                    }
                }
                w.line("G80");
            }
            w.last_xyz = None;
            w.last_g = None;
            canned_done = true;
            total_moves += tp.cycles.len();
        }
        if canned_done {
            continue;
        }
        let posted = post_kinematics(machine, tp, tolerance)?;
        for p in &posted {
            let m = &p.m;
            let g: &'static str = match m.kind {
                MoveKind::Rapid if post.g0_rapids => "G0",
                MoveKind::Retract if m.feed <= 0.0 && post.g0_rapids => "G0",
                _ => "G1",
            };
            let feed = if g == "G0" { None } else { Some(machine.clamp_feed(if m.feed > 0.0 { m.feed } else { machine.rapid_feed }).0) };
            let abc = match &machine.kinematics {
                Kinematics::ThreeAxis => None,
                Kinematics::FourAxis { .. } => Some((words.first().copied().unwrap_or('A'), p.pose.primary_deg, ' ', 0.0)),
                Kinematics::FiveAxis { .. } => {
                    Some((words.first().copied().unwrap_or('A'), p.pose.primary_deg, words.get(1).copied().unwrap_or('C'), p.pose.secondary_deg))
                }
            };
            w.motion(g, p.pose.xyz, abc, feed);
        }
        total_moves += posted.len();
        if posted.len() > tp.moves.len() {
            notice.issue(Issue::info(
                "linearised",
                format!("{}: {} moves were split into {} for the control's rotary interpolation", op.name, tp.moves.len(), posted.len()),
            ));
        }
    }
    if tcp_active && !post.tcp_off.is_empty() {
        w.line(&post.tcp_off.clone());
    }
    w.line("M5");
    if post.coolant {
        w.line("M9");
    }
    if !post.park.is_empty() {
        w.line(&post.park.clone());
    }
    for l in &post.footer {
        w.line(l);
    }
    w.line(&post.end.clone());
    if post.percent_lines {
        w.line("%");
    }
    notice.value("lines", w.lines);
    notice.value("moves", total_moves);
    notice.value("operations", ops.len());
    notice.value("bytes", w.out.len());
    notice.info = Some(format!("{} lines, {} operations, {} moves", w.lines, ops.len(), total_moves));
    let _ = r3(0.0);
    Ok(Program { gcode: w.out, lines: w.lines, notice })
}

fn tool_kind_name(t: &Tool) -> &'static str {
    match t.kind {
        crate::tool::ToolKind::FlatEnd => "flat end mill",
        crate::tool::ToolKind::BallEnd => "ball end mill",
        crate::tool::ToolKind::BullNose { .. } => "bull-nose end mill",
        crate::tool::ToolKind::Chamfer { .. } => "chamfer mill",
        crate::tool::ToolKind::Drill { .. } => "drill",
        crate::tool::ToolKind::SpotDrill { .. } => "spot drill",
        crate::tool::ToolKind::TaperedBall { .. } => "tapered ball end mill",
    }
}
