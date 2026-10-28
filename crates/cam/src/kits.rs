//! Tool kits: the tools a machine's maker sells for it, with feeds and speeds scaled to the
//! machine's spindle and feed limits. The numbers are starting points for light desktop
//! machines (chip loads of a few hundredths of a millimetre), not a feeds-and-speeds engine.

use serde::{Deserialize, Serialize};

use crate::machine::Machine;
use crate::tool::{Coolant, Feeds, Tool, ToolKind};

/// The material the feeds are for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Material {
    #[default]
    Aluminium,
    Wood,
    Plastic,
}

impl Material {
    pub fn parse(s: &str) -> Option<Material> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "aluminium" | "aluminum" | "al" | "metal" | "brass" => Material::Aluminium,
            "wood" | "mdf" | "plywood" | "hardwood" => Material::Wood,
            "plastic" | "acrylic" | "delrin" | "pom" | "hdpe" | "pla" => Material::Plastic,
            _ => return None,
        })
    }

    /// Chip load per tooth (mm) for a 3.175 mm cutter: single-flute flat, two-flute ball.
    fn chip_load(self) -> (f64, f64) {
        match self {
            Material::Aluminium => (0.025, 0.012),
            Material::Wood => (0.06, 0.035),
            Material::Plastic => (0.05, 0.03),
        }
    }

    /// Spindle speed asked for before the machine's limit.
    fn rpm(self) -> f64 {
        match self {
            Material::Aluminium => 12000.0,
            Material::Wood => 15000.0,
            Material::Plastic => 12000.0,
        }
    }

    /// Axial depth of cut to start with (mm), per millimetre of tool diameter.
    pub fn stepdown_per_diameter(self) -> f64 {
        match self {
            Material::Aluminium => 0.25,
            Material::Wood => 1.0,
            Material::Plastic => 0.75,
        }
    }
}

/// Every kit by id.
pub const KITS: &[&str] = &["makera_essential"];

/// Makera's 1/8" shank bit set as sold for the Carvera, Carvera Air and Z1: single-flute flat
/// end mills, two-flute ball end mills, a 90° chamfer bit, a 30° engraving bit and twist
/// drills, in 3.175, 2 and 1 mm. Feeds come from chip loads for `material`, scaled down for
/// the smaller cutters, with the speed and feed held within `machine`'s limits.
pub fn makera_essential(machine: &Machine, material: Material) -> Vec<Tool> {
    let (cl_flat, cl_ball) = material.chip_load();
    let (rpm, _) = machine.clamp_rpm(material.rpm());
    let (drill_rpm, _) = machine.clamp_rpm(6000.0);
    let cap = |f: f64| if machine.max_feed > 0.0 { f.min(machine.max_feed * 0.8) } else { f };
    let scale = |d: f64| (d / 3.175).clamp(0.3, 1.0);
    let feeds = |cutting: f64, plunge: f64, rpm: f64| Feeds {
        spindle_rpm: rpm,
        cutting: cap(cutting).round(),
        plunge: cap(plunge).round().max(20.0),
        ramp: cap(cutting * 0.5).round(),
        lead_in: cap(cutting).round(),
        lead_out: cap(cutting).round(),
        retract: 0.0,
        coolant: Coolant::Air,
        clockwise: true,
    };
    let mut out = Vec::new();
    let mut push = |name: &str, kind: ToolKind, d: f64, flute: f64, flutes: u32, f: Feeds| {
        let mut t = Tool::new(0, name, kind, d);
        t.flute_length = flute;
        t.overall_length = 38.0;
        t.shank_diameter = 3.175;
        t.flutes = flutes;
        t.feeds = f;
        out.push(t);
    };
    for (d, flute) in [(3.175, 12.0), (2.0, 8.0), (1.0, 4.0)] {
        let cutting = cl_flat * scale(d) * rpm;
        push(&format!("{d} mm single-flute flat end mill"), ToolKind::FlatEnd, d, flute, 1, feeds(cutting, cutting / 3.0, rpm));
    }
    for (d, flute) in [(3.175, 12.0), (2.0, 8.0), (1.0, 4.0)] {
        let cutting = cl_ball * scale(d) * 2.0 * rpm;
        push(&format!("{d} mm ball end mill"), ToolKind::BallEnd, d, flute, 2, feeds(cutting, cutting / 3.0, rpm));
    }
    let v_cut = cl_flat * 0.6 * 2.0 * rpm;
    push("90° chamfer / V bit", ToolKind::Chamfer { tip_angle_deg: 90.0 }, 3.175, 4.0, 2, feeds(v_cut, v_cut / 3.0, rpm));
    let e_cut = cl_flat * 0.4 * rpm;
    push("30° engraving bit", ToolKind::Chamfer { tip_angle_deg: 30.0 }, 3.175, 6.0, 1, feeds(e_cut, e_cut / 3.0, rpm));
    for (d, flute) in [(3.175, 20.0), (2.0, 15.0), (1.0, 10.0)] {
        // Twist drills: feed per revolution of about a hundredth of the diameter.
        let per_rev = match material {
            Material::Aluminium => 0.012 * d,
            Material::Wood => 0.03 * d,
            Material::Plastic => 0.02 * d,
        };
        let plunge = per_rev * drill_rpm;
        push(&format!("{d} mm drill"), ToolKind::Drill { tip_angle_deg: 118.0 }, d, flute, 2, feeds(plunge, plunge, drill_rpm));
    }
    if let Some(laser) = machine.laser {
        let mut t = Tool::new(0, &format!("{} W {} nm laser", laser.power_w, laser.wavelength_nm), ToolKind::Laser { wavelength_nm: laser.wavelength_nm }, laser.spot_mm.max(0.01));
        t.flute_length = 1.0;
        t.overall_length = 1.0;
        t.shank_diameter = 0.0;
        t.flutes = 0;
        t.feeds = Feeds { spindle_rpm: 1.0, cutting: 300.0, plunge: 300.0, ramp: 300.0, lead_in: 300.0, lead_out: 300.0, retract: 0.0, coolant: Coolant::Air, clockwise: true };
        out.push(t);
    }
    out
}

/// A kit by id.
pub fn kit(id: &str, machine: &Machine, material: Material) -> Option<Vec<Tool>> {
    match id.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
        "makera_essential" | "makera" | "carvera" | "essential" => Some(makera_essential(machine, material)),
        _ => None,
    }
}
