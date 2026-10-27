//! Machine kinematics: which rotary axes a machine has and where they sit, the inverse
//! kinematics that turn a tool axis direction and tip position into rotary angles and linear
//! positions, the choice between the two solutions, and the linearisation of multi-axis moves
//! for controls without tool-centre-point control.

use serde::{Deserialize, Serialize};
use solidcraft_geom::Vec3;

use crate::frame::rotate;
use crate::toolpath::{Move, Toolpath};
use crate::{CamError, Result};

/// A rotary axis of the machine.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RotaryAxis {
    /// Word in the G-code (`A`, `B` or `C`).
    pub name: char,
    /// Direction of the axis in machine coordinates (also the WCS when the table is level).
    pub direction: Vec3,
    /// A point on the axis (machine coordinates, relative to the WCS origin).
    pub pivot: Vec3,
    pub min_deg: f64,
    pub max_deg: f64,
    /// Whether the axis turns the workpiece (table) rather than the tool (head).
    pub on_table: bool,
}

impl Default for RotaryAxis {
    fn default() -> Self {
        RotaryAxis {
            name: 'A',
            direction: Vec3::new(1.0, 0.0, 0.0),
            pivot: Vec3::new(0.0, 0.0, 0.0),
            min_deg: -120.0,
            max_deg: 120.0,
            on_table: true,
        }
    }
}

impl RotaryAxis {
    pub fn a_table() -> RotaryAxis {
        RotaryAxis { name: 'A', direction: Vec3::new(1.0, 0.0, 0.0), min_deg: -120.0, max_deg: 120.0, on_table: true, ..Default::default() }
    }
    pub fn b_table() -> RotaryAxis {
        RotaryAxis { name: 'B', direction: Vec3::new(0.0, 1.0, 0.0), min_deg: -120.0, max_deg: 120.0, on_table: true, ..Default::default() }
    }
    pub fn c_table() -> RotaryAxis {
        RotaryAxis { name: 'C', direction: Vec3::new(0.0, 0.0, 1.0), min_deg: -99_999.0, max_deg: 99_999.0, on_table: true, ..Default::default() }
    }
    pub fn a_rotary_continuous() -> RotaryAxis {
        RotaryAxis { name: 'A', direction: Vec3::new(1.0, 0.0, 0.0), min_deg: -99_999.0, max_deg: 99_999.0, on_table: true, ..Default::default() }
    }
    pub fn in_range(&self, deg: f64) -> bool {
        deg >= self.min_deg - 1e-9 && deg <= self.max_deg + 1e-9
    }
    /// Whether the axis may turn without limit.
    pub fn is_continuous(&self) -> bool {
        self.max_deg - self.min_deg >= 720.0
    }
}

/// The machine's layout.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Kinematics {
    /// Three linear axes.
    #[default]
    ThreeAxis,
    /// Three linear axes and one rotary axis (on the table for a rotary indexer, else a head).
    FourAxis { rotary: RotaryAxis },
    /// Five axes: `primary` carries `secondary`. On a table-table (trunnion) machine the
    /// primary tilts the trunnion and the secondary is the rotary table on it; on a head-head
    /// machine the primary is the head's first swivel; on a head-table machine the primary is
    /// the head swivel and the secondary the rotary table.
    FiveAxis { primary: RotaryAxis, secondary: RotaryAxis },
}

/// How tools get into the spindle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolChanger {
    /// Unknown or none: the post writes tool changes and the control deals with them.
    #[default]
    Unknown,
    /// Tools are changed by hand; the control pauses and prompts.
    Manual,
    /// An automatic changer with this many slots; tools beyond them are changed by hand.
    Automatic { slots: u32 },
}

/// A machine: kinematics, travel, speeds and the control's capabilities.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Machine {
    pub name: String,
    /// Vendor and model, when the machine is a known product (`Makera`, `Carvera`).
    pub vendor: String,
    pub model: String,
    /// The post that writes this machine's programs (empty: chosen by the axis count).
    pub post: String,
    pub kinematics: Kinematics,
    /// Rapid traverse rate, mm/min, for time estimates.
    pub rapid_feed: f64,
    /// Rotary rapid, deg/min.
    pub rotary_rapid_deg_min: f64,
    /// Linear travel in machine coordinates relative to the WCS origin (checked when set).
    pub travel_min: Option<Vec3>,
    pub travel_max: Option<Vec3>,
    /// The work area as a size (mm): the toolpath's extent is checked against it (0: unknown).
    pub work_area: Vec3,
    /// Clearance under the gantry, mm (0: unknown).
    pub gantry_clearance: f64,
    /// Spindle speed range, rev/min (max 0: unknown), and power, W (0: unknown).
    pub spindle_rpm_min: f64,
    pub spindle_rpm_max: f64,
    pub spindle_power_w: f64,
    /// The largest feed the control moves at, mm/min (0: unknown). Faster feeds are clamped by
    /// the post, with a warning.
    pub max_feed: f64,
    pub tool_changer: ToolChanger,
    /// Shank diameters the collets take, mm (empty: any).
    pub collets: Vec<f64>,
    /// A rotary module's stock capacity, diameter and length (mm), when one is fitted.
    pub rotary_capacity: Option<(f64, f64)>,
    /// The control compensates the rotary axes itself (TCP / RTCP): the post writes workpiece
    /// coordinates and angles. Without it the post computes machine coordinates and linearises.
    pub tcp: bool,
    /// Largest tip deviation allowed when linearising multi-axis moves without TCP.
    pub linearisation_tolerance: f64,
    /// Largest angular change of a rotary axis in one move (degrees).
    pub max_rotary_step_deg: f64,
}

impl Default for Machine {
    fn default() -> Self {
        Machine {
            name: "3-axis mill".into(),
            vendor: String::new(),
            model: String::new(),
            post: String::new(),
            kinematics: Kinematics::ThreeAxis,
            rapid_feed: 5000.0,
            rotary_rapid_deg_min: 3600.0,
            travel_min: None,
            travel_max: None,
            work_area: Vec3::new(0.0, 0.0, 0.0),
            gantry_clearance: 0.0,
            spindle_rpm_min: 0.0,
            spindle_rpm_max: 0.0,
            spindle_power_w: 0.0,
            max_feed: 0.0,
            tool_changer: ToolChanger::Unknown,
            collets: Vec::new(),
            rotary_capacity: None,
            tcp: false,
            linearisation_tolerance: 0.01,
            max_rotary_step_deg: 5.0,
        }
    }
}

impl Machine {
    pub fn three_axis() -> Machine {
        Machine::default()
    }
    /// A 4-axis mill with a continuous A rotary along X.
    pub fn four_axis_a() -> Machine {
        Machine {
            name: "4-axis mill (A)".into(),
            kinematics: Kinematics::FourAxis { rotary: RotaryAxis::a_rotary_continuous() },
            ..Default::default()
        }
    }
    /// A trunnion machine: A tilts about X, carrying a C rotary table.
    pub fn trunnion_ac() -> Machine {
        Machine {
            name: "5-axis trunnion (A/C)".into(),
            kinematics: Kinematics::FiveAxis { primary: RotaryAxis::a_table(), secondary: RotaryAxis::c_table() },
            ..Default::default()
        }
    }
    /// A trunnion machine: B tilts about Y, carrying a C rotary table.
    pub fn trunnion_bc() -> Machine {
        Machine {
            name: "5-axis trunnion (B/C)".into(),
            kinematics: Kinematics::FiveAxis { primary: RotaryAxis::b_table(), secondary: RotaryAxis::c_table() },
            ..Default::default()
        }
    }
    /// A head-head machine: the spindle swivels about B then C (both on the head).
    pub fn head_head_bc() -> Machine {
        let mut b = RotaryAxis::b_table();
        b.on_table = false;
        let mut c = RotaryAxis::c_table();
        c.on_table = false;
        Machine {
            name: "5-axis head-head (B/C)".into(),
            kinematics: Kinematics::FiveAxis { primary: c, secondary: b },
            tcp: true,
            ..Default::default()
        }
    }

    /// Makera Carvera: 360 × 240 × 140 mm, 200 W spindle to 15 000 rev/min, 6-slot automatic
    /// tool changer, 1/8" collet (1/4", 6, 4 and 3 mm optional), 6 000 mm/min, 120 mm under the
    /// gantry (Makera's published specifications).
    pub fn makera_carvera() -> Machine {
        Machine {
            name: "Makera Carvera".into(),
            vendor: "Makera".into(),
            model: "Carvera".into(),
            post: "makera".into(),
            rapid_feed: 6000.0,
            max_feed: 6000.0,
            work_area: Vec3::new(360.0, 240.0, 140.0),
            gantry_clearance: 120.0,
            spindle_rpm_min: 0.0,
            spindle_rpm_max: 15000.0,
            spindle_power_w: 200.0,
            tool_changer: ToolChanger::Automatic { slots: 6 },
            collets: vec![3.175, 6.35, 6.0, 4.0, 3.0],
            ..Default::default()
        }
    }

    /// The Carvera with its 4th-axis module: a continuous A rotary along X taking stock up to
    /// Ø92 × 240 mm.
    pub fn makera_carvera_4axis() -> Machine {
        Machine {
            name: "Makera Carvera + 4th axis".into(),
            rotary_rapid_deg_min: 2300.0,
            kinematics: Kinematics::FourAxis { rotary: RotaryAxis::a_rotary_continuous() },
            rotary_capacity: Some((92.0, 240.0)),
            ..Machine::makera_carvera()
        }
    }

    /// Makera Carvera Air: 300 × 200 × 130 mm, 200 W spindle to 13 000 rev/min, lever quick
    /// tool changer (by hand), 1/8" collet (1/4", 6 and 4 mm optional), 4 000 mm/min, 120 mm under
    /// the gantry.
    pub fn makera_carvera_air() -> Machine {
        Machine {
            name: "Makera Carvera Air".into(),
            vendor: "Makera".into(),
            model: "Carvera Air".into(),
            post: "makera".into(),
            rapid_feed: 4000.0,
            max_feed: 4000.0,
            work_area: Vec3::new(300.0, 200.0, 130.0),
            gantry_clearance: 120.0,
            spindle_rpm_max: 13000.0,
            spindle_power_w: 200.0,
            tool_changer: ToolChanger::Manual,
            collets: vec![3.175, 6.35, 6.0, 4.0],
            ..Default::default()
        }
    }

    /// The Carvera Air with its 4th-axis module (Ø92 × 200 mm).
    pub fn makera_carvera_air_4axis() -> Machine {
        Machine {
            name: "Makera Carvera Air + 4th axis".into(),
            rotary_rapid_deg_min: 2300.0,
            kinematics: Kinematics::FourAxis { rotary: RotaryAxis::a_rotary_continuous() },
            rotary_capacity: Some((92.0, 200.0)),
            ..Machine::makera_carvera_air()
        }
    }

    /// Makera Z1: 200 × 200 × 100 mm, 150 W spindle to 13 000 rev/min, quick tool change by
    /// hand, 1/8" collet (1/4", 6, 4 and 3 mm optional), 1 200 mm/min, 115 mm under the gantry.
    pub fn makera_z1() -> Machine {
        Machine {
            name: "Makera Z1".into(),
            vendor: "Makera".into(),
            model: "Z1".into(),
            post: "makera".into(),
            rapid_feed: 1200.0,
            max_feed: 1200.0,
            work_area: Vec3::new(200.0, 200.0, 100.0),
            gantry_clearance: 115.0,
            spindle_rpm_max: 13000.0,
            spindle_power_w: 150.0,
            tool_changer: ToolChanger::Manual,
            collets: vec![3.175, 6.35, 6.0, 4.0, 3.0],
            ..Default::default()
        }
    }

    /// The Z1 with its 4th-axis module (Ø80 × 150 mm).
    pub fn makera_z1_4axis() -> Machine {
        Machine {
            name: "Makera Z1 + 4th axis".into(),
            rotary_rapid_deg_min: 2300.0,
            kinematics: Kinematics::FourAxis { rotary: RotaryAxis::a_rotary_continuous() },
            rotary_capacity: Some((80.0, 150.0)),
            ..Machine::makera_z1()
        }
    }

    /// Every preset by id, in the order shown to people.
    pub const PRESETS: &'static [&'static str] = &[
        "three_axis",
        "four_axis_a",
        "trunnion_ac",
        "trunnion_bc",
        "head_head_bc",
        "carvera",
        "carvera_4axis",
        "carvera_air",
        "carvera_air_4axis",
        "z1",
        "z1_4axis",
    ];

    /// A preset by id or alias.
    pub fn preset(id: &str) -> Option<Machine> {
        Some(match id.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "three_axis" | "3axis" | "3_axis" | "mill3" => Machine::three_axis(),
            "four_axis_a" | "4axis" | "4_axis" | "rotary_a" => Machine::four_axis_a(),
            "trunnion_ac" | "5axis" | "5_axis" | "table_table_ac" => Machine::trunnion_ac(),
            "trunnion_bc" | "table_table_bc" => Machine::trunnion_bc(),
            "head_head_bc" | "head_head" => Machine::head_head_bc(),
            "carvera" | "makera_carvera" => Machine::makera_carvera(),
            "carvera_4axis" | "makera_carvera_4axis" | "carvera_4th_axis" => Machine::makera_carvera_4axis(),
            "carvera_air" | "makera_carvera_air" => Machine::makera_carvera_air(),
            "carvera_air_4axis" | "makera_carvera_air_4axis" | "carvera_air_4th_axis" => Machine::makera_carvera_air_4axis(),
            "z1" | "makera_z1" => Machine::makera_z1(),
            "z1_4axis" | "makera_z1_4axis" | "z1_4th_axis" => Machine::makera_z1_4axis(),
            _ => return None,
        })
    }

    /// The spindle speed the machine can give for `rpm`, and whether it had to be changed.
    pub fn clamp_rpm(&self, rpm: f64) -> (f64, bool) {
        let mut v = rpm;
        if self.spindle_rpm_max > 0.0 {
            v = v.min(self.spindle_rpm_max);
        }
        if self.spindle_rpm_min > 0.0 {
            v = v.max(self.spindle_rpm_min);
        }
        (v, (v - rpm).abs() > 1e-9)
    }

    /// The feed the machine can move at for `feed`, and whether it had to be changed.
    pub fn clamp_feed(&self, feed: f64) -> (f64, bool) {
        if self.max_feed > 0.0 && feed > self.max_feed { (self.max_feed, true) } else { (feed, false) }
    }

    /// What the machine would object to in a tool and its toolpath: a spindle speed or feed
    /// past the machine's, a shank no collet takes, a toolpath wider than the work area.
    pub fn check(&self, tool: &crate::tool::Tool, toolpath: &Toolpath) -> Vec<crate::notice::Issue> {
        use crate::notice::Issue;
        let mut out = Vec::new();
        let who = if self.model.is_empty() { self.name.clone() } else { self.model.clone() };
        let f = &tool.feeds;
        if self.spindle_rpm_max > 0.0 && f.spindle_rpm > self.spindle_rpm_max + 1e-9 {
            out.push(
                Issue::warning(
                    "spindle_speed",
                    format!("T{} {}: {} rev/min is past the {}'s {} rev/min; the post writes the maximum", tool.number, tool.name, f.spindle_rpm.round(), who, self.spindle_rpm_max.round()),
                )
                .parameter("feeds.spindle_rpm", f.spindle_rpm)
                .limit(self.spindle_rpm_max)
                .suggest(serde_json::json!({"command": "cam.tool.set", "tool": tool.id, "feeds": {"spindle_rpm": self.spindle_rpm_max}})),
            );
        }
        if self.max_feed > 0.0 {
            for (what, v) in [("cutting", f.cutting), ("plunge", f.plunge), ("ramp", f.ramp), ("lead_in", f.lead_in), ("lead_out", f.lead_out)] {
                if v > self.max_feed + 1e-9 {
                    out.push(
                        Issue::warning(
                            "feed_limit",
                            format!("T{} {}: the {what} feed {} mm/min is past the {}'s {} mm/min; the post writes the maximum", tool.number, tool.name, v.round(), who, self.max_feed.round()),
                        )
                        .parameter(&format!("feeds.{what}"), v)
                        .limit(self.max_feed)
                        .suggest(serde_json::json!({"command": "cam.tool.set", "tool": tool.id, "feeds": {what: self.max_feed}})),
                    );
                }
            }
        }
        if !self.collets.is_empty() {
            let shank = if tool.shank_diameter > 0.0 { tool.shank_diameter } else { tool.diameter };
            if !self.collets.iter().any(|c| (c - shank).abs() < 0.06) {
                let list: Vec<String> = self.collets.iter().map(|c| format!("{c}")).collect();
                out.push(
                    Issue::warning("collet", format!("T{} {}: no {} collet takes a {shank} mm shank (collets: {} mm)", tool.number, tool.name, who, list.join(", ")))
                        .parameter("shank_diameter", shank)
                        .limit(serde_json::json!(self.collets)),
                );
            }
        }
        if let Some((lo, hi)) = toolpath.bounds() {
            let ext = hi - lo;
            for (axis, size, limit) in [("X", ext.x, self.work_area.x), ("Y", ext.y, self.work_area.y), ("Z", ext.z, self.work_area.z)] {
                if limit > 0.0 && size > limit + 1e-6 {
                    out.push(
                        Issue::error("work_area", format!("the toolpath spans {:.1} mm in {axis}; the {}'s work area is {limit} mm", size, who))
                            .parameter(axis, size)
                            .limit(limit),
                    );
                }
            }
            if let Some((dia, len)) = self.rotary_capacity
                && let Kinematics::FourAxis { rotary } = &self.kinematics
            {
                // Radius from the rotary axis and length along it.
                let axis = rotary.direction.normalized().unwrap_or(Vec3::new(1.0, 0.0, 0.0));
                let mut r_max: f64 = 0.0;
                let (mut a_min, mut a_max) = (f64::INFINITY, f64::NEG_INFINITY);
                for m in &toolpath.moves {
                    if m.kind == crate::toolpath::MoveKind::Rapid {
                        continue;
                    }
                    let d = m.to - rotary.pivot;
                    let along = d.dot(axis);
                    let radial = (d - axis * along).len();
                    r_max = r_max.max(radial);
                    a_min = a_min.min(along);
                    a_max = a_max.max(along);
                }
                if r_max * 2.0 > dia + 1e-6 {
                    out.push(
                        Issue::warning("rotary_capacity", format!("cuts reach {:.1} mm from the rotary axis; the {}'s 4th-axis module takes Ø{dia} mm stock", r_max, who))
                            .limit(dia),
                    );
                }
                if a_max.is_finite() && a_max - a_min > len + 1e-6 {
                    out.push(
                        Issue::warning("rotary_capacity", format!("cuts span {:.1} mm along the rotary axis; the {}'s 4th-axis module takes {len} mm", a_max - a_min, who))
                            .limit(len),
                    );
                }
            }
        }
        out
    }

    pub fn axis_count(&self) -> usize {
        match self.kinematics {
            Kinematics::ThreeAxis => 3,
            Kinematics::FourAxis { .. } => 4,
            Kinematics::FiveAxis { .. } => 5,
        }
    }

    pub fn rotaries(&self) -> Vec<RotaryAxis> {
        match &self.kinematics {
            Kinematics::ThreeAxis => Vec::new(),
            Kinematics::FourAxis { rotary } => vec![*rotary],
            Kinematics::FiveAxis { primary, secondary } => vec![*primary, *secondary],
        }
    }

    pub fn validate(&self) -> Result<()> {
        for r in self.rotaries() {
            if r.direction.normalized().is_none() || !r.pivot.is_finite() {
                return Err(CamError::Param(format!("rotary axis {} needs a direction and a finite pivot", r.name)));
            }
            if !(r.min_deg.is_finite() && r.max_deg.is_finite() && r.min_deg < r.max_deg) {
                return Err(CamError::Param(format!("rotary axis {} limits must be finite with min below max", r.name)));
            }
            if !matches!(r.name, 'A' | 'B' | 'C') {
                return Err(CamError::Param("rotary axis names are A, B or C".into()));
            }
        }
        if let Kinematics::FiveAxis { primary, secondary } = &self.kinematics {
            if primary.name == secondary.name {
                return Err(CamError::Param("the two rotary axes need different names".into()));
            }
            if primary.direction.normalized().zip(secondary.direction.normalized()).is_some_and(|(a, b)| a.cross(b).len() < 1e-6) {
                return Err(CamError::Param("the two rotary axes must not be parallel".into()));
            }
        }
        crate::positive("rapid feed", self.rapid_feed)?;
        crate::positive("linearisation tolerance", self.linearisation_tolerance)?;
        Ok(())
    }
}

/// A machine pose: rotary angles in degrees (primary, secondary) and the linear position the
/// machine's XYZ axes take (machine coordinates relative to the WCS origin, or workpiece
/// coordinates in TCP mode).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub xyz: Vec3,
    pub primary_deg: f64,
    pub secondary_deg: f64,
}

/// Tool axis direction (unit, in workpiece coordinates) produced by rotary angles. Rotations
/// about a table axis turn the workpiece, so in workpiece coordinates the tool axis turns the
/// opposite way; rotations about a head axis turn the tool.
pub fn forward_axis(k: &Kinematics, primary_deg: f64, secondary_deg: f64) -> Vec3 {
    let z = Vec3::new(0.0, 0.0, 1.0);
    match k {
        Kinematics::ThreeAxis => z,
        Kinematics::FourAxis { rotary } => {
            let d = rotary.direction.normalized().unwrap_or(Vec3::new(1.0, 0.0, 0.0));
            let sign = if rotary.on_table { -1.0 } else { 1.0 };
            rotate(z, d, sign * primary_deg.to_radians())
        }
        Kinematics::FiveAxis { primary, secondary } => {
            let dp = primary.direction.normalized().unwrap_or(Vec3::new(1.0, 0.0, 0.0));
            let ds = secondary.direction.normalized().unwrap_or(Vec3::new(0.0, 0.0, 1.0));
            // The primary carries the secondary. Workpiece-to-machine: p_m = R_p(a) R_s(c) p_w for
            // table axes. The tool axis is e_z in machine coordinates, so in workpiece
            // coordinates t_w = R_s(-c) R_p(-a) e_z. For head axes the tool turns: t = R_p(a) R_s(c) e_z.
            match (primary.on_table, secondary.on_table) {
                (true, true) => {
                    let t = rotate(z, dp, -primary_deg.to_radians());
                    rotate(t, ds, -secondary_deg.to_radians())
                }
                (false, false) => {
                    let t = rotate(z, ds, secondary_deg.to_radians());
                    rotate(t, dp, primary_deg.to_radians())
                }
                (false, true) => {
                    // Head primary tilts the tool; table secondary turns the workpiece.
                    let t = rotate(z, dp, primary_deg.to_radians());
                    rotate(t, ds, -secondary_deg.to_radians())
                }
                (true, false) => {
                    let t = rotate(z, ds, secondary_deg.to_radians());
                    rotate(t, dp, -primary_deg.to_radians())
                }
            }
        }
    }
}

/// Both inverse-kinematics solutions (primary, secondary in degrees) for a tool axis `t`
/// (unit, workpiece coordinates). For a 4-axis machine the second element repeats the first and
/// the axis must lie in the plane the rotary can reach (within `tol` radians).
pub fn inverse_angles(k: &Kinematics, t: Vec3, tol: f64) -> Result<[(f64, f64); 2]> {
    let t = t.normalized().ok_or_else(|| CamError::Machine("tool axis must not be zero".into()))?;
    let z = Vec3::new(0.0, 0.0, 1.0);
    match k {
        Kinematics::ThreeAxis => {
            if t.cross(z).len() > tol {
                return Err(CamError::Machine("a 3-axis machine cannot tilt the tool".into()));
            }
            Ok([(0.0, 0.0), (0.0, 0.0)])
        }
        Kinematics::FourAxis { rotary } => {
            let d = rotary.direction.normalized().unwrap_or(Vec3::new(1.0, 0.0, 0.0));
            if t.dot(d).abs() > tol {
                return Err(CamError::Machine(format!("tool axis is out of the plane the {} axis can reach", rotary.name)));
            }
            // Angle from z to t about d.
            let tp = (t - d * t.dot(d)).normalized().unwrap_or(z);
            let zp = (z - d * z.dot(d)).normalized().unwrap_or(z);
            let ang = zp.cross(tp).dot(d).atan2(zp.dot(tp)).to_degrees();
            let sign = if rotary.on_table { -1.0 } else { 1.0 };
            let a = sign * ang;
            Ok([(a, 0.0), (a, 0.0)])
        }
        Kinematics::FiveAxis { primary, secondary } => {
            // Solve numerically but exactly for the two classic cases through the general
            // relation: find (a, c) with forward_axis(a, c) = t. The secondary is the axis
            // nearest the tool at rest (usually parallel to z); the primary tilts away from it.
            solve_two_axes(k, primary, secondary, t, tol)
        }
    }
}

fn solve_two_axes(k: &Kinematics, primary: &RotaryAxis, secondary: &RotaryAxis, t: Vec3, tol: f64) -> Result<[(f64, f64); 2]> {
    let z = Vec3::new(0.0, 0.0, 1.0);
    let dp = primary.direction.normalized().unwrap_or(Vec3::new(1.0, 0.0, 0.0));
    let ds = secondary.direction.normalized().unwrap_or(z);
    // Which axis moves the tool axis "out of" z? With the secondary parallel to z (a table-table
    // or head-table machine), the tilt comes from the primary alone: the component of t along ds
    // is set by the primary's angle, the secondary then rotates t around ds.
    // General approach: the primary angle a must bring e_z (turned by the primary) to the cone
    // about ds with the same ds-component as t; the secondary angle then matches the azimuth.
    let solve_with = |tilt_axis: Vec3, spin_axis: Vec3, tilt_first: bool| -> Option<Vec<(f64, f64)>> {
        // We need R(tilt, a) applied to z (or in the other order) to have spin-component equal to t·spin.
        // Component of R(tilt, a) z along spin: with z = z_par + z_perp relative to tilt axis:
        // R z = z_par + z_perp cos a + (tilt × z) sin a. Its dot with spin is of the form
        // A + B cos a + C sin a = target.
        let z_par = tilt_axis * z.dot(tilt_axis);
        let z_perp = z - z_par;
        let cross = tilt_axis.cross(z);
        let aa = z_par.dot(spin_axis);
        let bb = z_perp.dot(spin_axis);
        let cc = cross.dot(spin_axis);
        let target = t.dot(spin_axis);
        // B cos a + C sin a = target - A → R cos(a - φ) = rhs.
        let rr = (bb * bb + cc * cc).sqrt();
        let rhs = target - aa;
        if rr < 1e-12 {
            return None;
        }
        let ratio = rhs / rr;
        if ratio.abs() > 1.0 + 1e-9 {
            return None;
        }
        let phi = cc.atan2(bb);
        let delta = ratio.clamp(-1.0, 1.0).acos();
        let mut sols = Vec::new();
        for a in [phi + delta, phi - delta] {
            let tilted = rotate(z, tilt_axis, a);
            // Spin angle: rotate `tilted` about spin_axis to reach t.
            let tp = (tilted - spin_axis * tilted.dot(spin_axis)).normalized();
            let tt = (t - spin_axis * t.dot(spin_axis)).normalized();
            let c = match (tp, tt) {
                (Some(tp), Some(tt)) => tp.cross(tt).dot(spin_axis).atan2(tp.dot(tt)),
                _ => 0.0,
            };
            let _ = tilt_first;
            sols.push((a, c));
        }
        Some(sols)
    };
    // Candidate interpretations; keep those whose forward kinematics reproduce t.
    let mut out: Vec<(f64, f64)> = Vec::new();
    let candidates: Vec<(f64, f64)> = [solve_with(dp, ds, true), solve_with(ds, dp, false)]
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|(a, c)| {
            // Try the angles as (primary, secondary) and swapped, with both signs for table axes.
            let mut v = Vec::new();
            for (p, s) in [(a, c), (c, a)] {
                for sp in [1.0, -1.0] {
                    for ss in [1.0, -1.0] {
                        v.push((sp * p.to_degrees(), ss * s.to_degrees()));
                    }
                }
            }
            v
        })
        .collect();
    for (p, s) in candidates {
        let f = forward_axis(k, p, s);
        if (f - t).len() <= tol.max(1e-7) && !out.iter().any(|(a, b)| (a - p).abs() < 1e-6 && (b - s).abs() < 1e-6) {
            out.push((normalize_deg(p), normalize_deg(s)));
        }
    }
    match out.len() {
        0 => Err(CamError::Machine(format!(
            "tool axis ({:.4}, {:.4}, {:.4}) cannot be reached by {} and {}",
            t.x, t.y, t.z, primary.name, secondary.name
        ))),
        1 => Ok([out[0], out[0]]),
        _ => Ok([out[0], out[1]]),
    }
}

/// Angle wrapped into (-180, 180].
pub fn normalize_deg(a: f64) -> f64 {
    let mut a = a % 360.0;
    if a <= -180.0 {
        a += 360.0;
    }
    if a > 180.0 {
        a -= 360.0;
    }
    a
}

/// The angle equivalent to `target` nearest `prev` (for continuous axes: unwrap).
pub fn nearest_turn(prev: f64, target: f64) -> f64 {
    let mut t = target;
    while t - prev > 180.0 {
        t -= 360.0;
    }
    while prev - t > 180.0 {
        t += 360.0;
    }
    t
}

/// The linear position of the machine for a tip at `tip` (workpiece) with rotary angles, for a
/// control without TCP: table axes turn the workpiece about their pivots (the tip in machine
/// coordinates is the workpiece point rotated), head axes swing the tool about their pivots
/// (the spindle's reference point is the tip moved back along the tool by the pivot offset).
pub fn machine_xyz(k: &Kinematics, tip: Vec3, primary_deg: f64, secondary_deg: f64, tool_axis: Vec3) -> Vec3 {
    match k {
        Kinematics::ThreeAxis => tip,
        Kinematics::FourAxis { rotary } => {
            if rotary.on_table {
                rotate_about(tip, rotary, primary_deg)
            } else {
                // Head pivot: the pivot lies on the tool axis at distance |pivot.z| from the tip
                // at rest; the programmed point is the pivot's position.
                let l = rotary.pivot.len();
                tip + tool_axis * l - Vec3::new(0.0, 0.0, l)
            }
        }
        Kinematics::FiveAxis { primary, secondary } => {
            let mut p = tip;
            // The secondary sits on the primary: apply the secondary's rotation first, then the primary's.
            if secondary.on_table {
                p = rotate_about(p, secondary, secondary_deg);
            }
            if primary.on_table {
                p = rotate_about(p, primary, primary_deg);
            }
            // Head axes: the programmed point is the head's pivot.
            let head_len: f64 = [primary, secondary].iter().filter(|r| !r.on_table).map(|r| r.pivot.len()).sum();
            if head_len > 0.0 {
                p = p + tool_axis * head_len - Vec3::new(0.0, 0.0, head_len);
            }
            p
        }
    }
}

fn rotate_about(p: Vec3, r: &RotaryAxis, deg: f64) -> Vec3 {
    let d = r.direction.normalized().unwrap_or(Vec3::new(1.0, 0.0, 0.0));
    r.pivot + rotate(p - r.pivot, d, deg.to_radians())
}

/// One posted multi-axis point: workpiece tip and axis, chosen rotary solution and the machine
/// position.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Posted {
    pub m: Move,
    pub pose: Pose,
}

/// Converts a toolpath to machine poses: picks for each move the rotary solution nearest the
/// previous one that is within the axis limits (unwrapping continuous axes), computes the
/// machine XYZ (or keeps the workpiece XYZ in TCP mode), and without TCP subdivides moves whose
/// tip would stray more than the linearisation tolerance as the control interpolates the axes.
pub fn post_kinematics(machine: &Machine, tp: &Toolpath, tol: f64) -> Result<Vec<Posted>> {
    machine.validate()?;
    let k = &machine.kinematics;
    let rot = machine.rotaries();
    let mut out: Vec<Posted> = Vec::with_capacity(tp.moves.len());
    let mut prev: Option<(Pose, Vec3)> = None;
    for m in &tp.moves {
        let sols = inverse_angles(k, m.axis, tol.max(1e-6))?;
        let mut best: Option<(f64, f64, f64)> = None;
        for (p, s) in sols {
            // Equivalent angles for continuous axes.
            let (p, s) = match prev {
                Some((pp, _)) => {
                    let p = if rot.first().is_some_and(|r| r.is_continuous()) { nearest_turn(pp.primary_deg, p) } else { p };
                    let s = if rot.get(1).is_some_and(|r| r.is_continuous()) { nearest_turn(pp.secondary_deg, s) } else { s };
                    (p, s)
                }
                None => (p, s),
            };
            let ok_p = rot.first().is_none_or(|r| r.in_range(p));
            let ok_s = rot.get(1).is_none_or(|r| r.in_range(s));
            if !(ok_p && ok_s) {
                continue;
            }
            let cost = prev.map_or(p.abs() + s.abs(), |(pp, _)| (pp.primary_deg - p).abs() + (pp.secondary_deg - s).abs());
            if best.is_none_or(|(c, _, _)| cost < c) {
                best = Some((cost, p, s));
            }
        }
        let Some((_, p, s)) = best else {
            return Err(CamError::Machine(format!("tool axis ({:.3}, {:.3}, {:.3}) is outside the rotary limits", m.axis.x, m.axis.y, m.axis.z)));
        };
        let pose_of = |tip: Vec3, axis: Vec3, p: f64, s: f64| Pose {
            xyz: if machine.tcp { tip } else { machine_xyz(k, tip, p, s, axis) },
            primary_deg: p,
            secondary_deg: s,
        };
        let pose = pose_of(m.to, m.axis, p, s);
        if let Some((pp, ptip)) = prev
            && !machine.tcp
            && ((pp.primary_deg - p).abs() > 1e-9 || (pp.secondary_deg - s).abs() > 1e-9)
            && m.is_cutting()
        {
            // Linearise: subdivide until the midpoint error is within tolerance.
            let steps = linearisation_steps(
                k,
                machine.linearisation_tolerance.max(1e-6),
                ptip,
                m.to,
                pp,
                pose,
                m.axis,
                prev.map(|x| forward_axis(k, x.0.primary_deg, x.0.secondary_deg)).unwrap_or(m.axis),
            );
            let max_step = machine.max_rotary_step_deg.max(0.01);
            let ang_steps = (((pp.primary_deg - p).abs().max((pp.secondary_deg - s).abs())) / max_step).ceil() as usize;
            let n = steps.max(ang_steps).clamp(1, 10_000);
            for i in 1..n {
                let t = i as f64 / n as f64;
                let tip = ptip.lerp(m.to, t);
                let pi = pp.primary_deg + (p - pp.primary_deg) * t;
                let si = pp.secondary_deg + (s - pp.secondary_deg) * t;
                let axis = forward_axis(k, pi, si);
                out.push(Posted { m: Move { to: tip, axis, ..*m }, pose: pose_of(tip, axis, pi, si) });
            }
        }
        out.push(Posted { m: *m, pose });
        prev = Some((pose, m.to));
    }
    Ok(out)
}

/// Number of pieces a move needs so that the tip, as the control interpolates linear and rotary
/// axes together, stays within `tol` of the straight workpiece path.
#[allow(clippy::too_many_arguments)]
fn linearisation_steps(k: &Kinematics, tol: f64, tip0: Vec3, tip1: Vec3, pose0: Pose, pose1: Pose, _axis1: Vec3, _axis0: Vec3) -> usize {
    let mut n = 1usize;
    while n < 4096 {
        // Worst deviation at the midpoints of each piece.
        let mut worst = 0.0f64;
        for i in 0..n {
            let t0 = i as f64 / n as f64;
            let t1 = (i + 1) as f64 / n as f64;
            let tm = (t0 + t1) * 0.5;
            // Machine interpolates joints linearly between the piece ends.
            let xyz0 = pose0.xyz.lerp(pose1.xyz, t0);
            let xyz1 = pose0.xyz.lerp(pose1.xyz, t1);
            let p0 = pose0.primary_deg + (pose1.primary_deg - pose0.primary_deg) * t0;
            let p1 = pose0.primary_deg + (pose1.primary_deg - pose0.primary_deg) * t1;
            let s0 = pose0.secondary_deg + (pose1.secondary_deg - pose0.secondary_deg) * t0;
            let s1 = pose0.secondary_deg + (pose1.secondary_deg - pose0.secondary_deg) * t1;
            let xyz_m = xyz0.lerp(xyz1, 0.5);
            let pm = (p0 + p1) * 0.5;
            let sm = (s0 + s1) * 0.5;
            // Where does that machine pose put the tip in the workpiece? Invert the table
            // rotations (the inverse of machine_xyz).
            let tip_m = workpiece_tip(k, xyz_m, pm, sm);
            let want = tip0.lerp(tip1, tm);
            worst = worst.max(tip_m.dist(want));
        }
        if worst <= tol {
            break;
        }
        n *= 2;
    }
    n
}

/// Inverse of [`machine_xyz`]: the workpiece tip for a machine position and angles.
pub fn workpiece_tip(k: &Kinematics, xyz: Vec3, primary_deg: f64, secondary_deg: f64) -> Vec3 {
    let axis = forward_axis(k, primary_deg, secondary_deg);
    match k {
        Kinematics::ThreeAxis => xyz,
        Kinematics::FourAxis { rotary } => {
            if rotary.on_table {
                rotate_about(xyz, rotary, -primary_deg)
            } else {
                let l = rotary.pivot.len();
                xyz - axis * l + Vec3::new(0.0, 0.0, l)
            }
        }
        Kinematics::FiveAxis { primary, secondary } => {
            let mut p = xyz;
            let head_len: f64 = [primary, secondary].iter().filter(|r| !r.on_table).map(|r| r.pivot.len()).sum();
            if head_len > 0.0 {
                p = p - axis * head_len + Vec3::new(0.0, 0.0, head_len);
            }
            if primary.on_table {
                p = rotate_about(p, primary, -primary_deg);
            }
            if secondary.on_table {
                p = rotate_about(p, secondary, -secondary_deg);
            }
            p
        }
    }
}
