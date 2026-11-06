//! Cutting load: what a roughing operation asks of the spindle. The force on each tooth comes
//! from the material's specific cutting force (Kienzle: `kc = kc1.1 · h^-mc` for a mean chip
//! thickness `h`), the operation's engagement (radial width `ae`, axial depth `ap`) and the
//! tool's chip load; power is force times cutting speed, set against the spindle's power at
//! the tool's speed. The numbers are estimates for planning, not measurements: they say how
//! close a cut runs to the machine's limit so the person can choose to push it or not.

use serde::Serialize;

use crate::kits::Material;
use crate::machine::{AxisDrive, Machine};
use crate::notice::Issue;
use crate::ops::OpKind;
use crate::tool::Tool;

/// How a roughing operation engages its tool.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Engagement {
    /// Radial width of cut (mm).
    pub ae: f64,
    /// Axial depth of cut (mm).
    pub ap: f64,
    /// The full width of the tool cuts (a pocket's first ring, a slot).
    pub slotting: bool,
}

/// The estimated load of one engagement.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Load {
    pub ae_mm: f64,
    pub ap_mm: f64,
    pub chip_mm: f64,
    pub removal_cm3_min: f64,
    pub force_mean_n: f64,
    pub force_peak_n: f64,
    pub torque_nm: f64,
    pub power_w: f64,
    /// The spindle's power at the tool's speed (0: not known).
    pub spindle_w: f64,
    /// `power_w / spindle_w` (0: not known).
    pub spindle_share: f64,
    /// The peak resultant force in the feed plane (N): the tangential force on the tooth at its
    /// thickest chip and the radial force pushing the tool off the wall, about 40 % of it in
    /// aluminium.
    pub axis_force_n: f64,
    /// The X/Y drives' force (N, 0: not known) and `axis_force_n` as a share of it.
    pub axis_limit_n: f64,
    pub axis_share: f64,
}

/// The engagement of a roughing operation, from its parameters and their defaults. `None` for
/// finishing, drilling and the other operations whose load is light or set by the stock left.
pub fn engagement(kind: &OpKind, tool: &Tool) -> Option<Engagement> {
    let d = tool.diameter;
    let or = |v: f64, default: f64| if v > 0.0 { v } else { default };
    let (ae, ap, slotting) = match kind {
        OpKind::Adaptive2d { optimal_load, stepdown, .. } | OpKind::Adaptive3d { optimal_load, stepdown, .. } => {
            (or(*optimal_load, d * 0.2), or(*stepdown, d * 0.5), false)
        }
        // A pocket's first ring is a full-width cut.
        OpKind::Pocket2d { stepdown, .. } | OpKind::Pocket3d { stepdown, .. } => (d, or(*stepdown, d * 0.5), true),
        OpKind::Slot { stepdown, .. } => (d, or(*stepdown, d * 0.5), true),
        OpKind::Face { stepover, stepdown, .. } => (or(*stepover, d * 0.4).min(d), or(*stepdown, d * 0.5), false),
        _ => return None,
    };
    // A cut deeper than the flutes is limited by them (and reported as tool_too_short).
    let ap = if tool.flute_length > 0.0 { ap.min(tool.flute_length) } else { ap };
    (ae > 0.0 && ap > 0.0 && d > 0.0).then_some(Engagement { ae: ae.min(d), ap, slotting })
}

/// The load of `e` with `tool`'s feeds in `material`, against `machine`'s spindle.
pub fn estimate(tool: &Tool, e: Engagement, material: Material, machine: &Machine) -> Option<Load> {
    let d = tool.diameter;
    let rpm = tool.feeds.spindle_rpm;
    let feed = tool.feeds.cutting;
    let z = f64::from(tool.flutes.max(1));
    if !(d > 0.0 && rpm > 0.0 && feed > 0.0) {
        return None;
    }
    let (kc11, mc) = material.cutting_force();
    let fz = feed / (rpm * z);
    // Engagement angle and chip thickness over it.
    let phi = (1.0 - 2.0 * e.ae / d).clamp(-1.0, 1.0).acos();
    if phi <= 0.0 {
        return None;
    }
    let h_mean = fz * (1.0 - phi.cos()) / phi;
    let h_max = fz * if phi > std::f64::consts::FRAC_PI_2 { 1.0 } else { phi.sin() };
    if !(h_mean > 0.0 && h_max > 0.0) {
        return None;
    }
    let kc_mean = kc11 * h_mean.powf(-mc);
    let kc_peak = kc11 * h_max.powf(-mc);
    // mm³/s removed; power = kc · removal rate.
    let removal = e.ae * e.ap * feed / 60.0;
    let power = kc_mean * removal / 1000.0;
    let vc = std::f64::consts::PI * d * rpm / 60_000.0; // m/s
    let force_mean = power / vc;
    let force_peak = kc_peak * e.ap * h_max;
    let torque = force_mean * d / 2000.0;
    let axis_force = force_peak * (1.0 + RADIAL_RATIO * RADIAL_RATIO).sqrt();
    let axis_limit = machine.axis_force_n;
    let axis_share = if axis_limit > 0.0 { axis_force / axis_limit } else { 0.0 };
    let spindle = machine.spindle_power_at(rpm);
    let share = if spindle > 0.0 { power / spindle } else { 0.0 };
    let r = |v: f64, k: f64| (v * k).round() / k;
    Some(Load {
        ae_mm: r(e.ae, 1000.0),
        ap_mm: r(e.ap, 1000.0),
        chip_mm: r(h_mean, 10_000.0),
        removal_cm3_min: r(removal * 0.06, 100.0),
        force_mean_n: r(force_mean, 10.0),
        force_peak_n: r(force_peak, 10.0),
        torque_nm: r(torque, 1000.0),
        power_w: r(power, 10.0),
        spindle_w: r(spindle, 10.0),
        spindle_share: r(share, 1000.0),
        axis_force_n: r(axis_force, 10.0),
        axis_limit_n: r(axis_limit, 10.0),
        axis_share: r(axis_share, 1000.0),
    })
}

/// The radial force on a tooth as a share of its tangential force (aluminium, sharp tool).
const RADIAL_RATIO: f64 = 0.4;

/// The share of the spindle's power above which a cut is reported as running close to it.
pub const CLOSE_SHARE: f64 = 0.8;

/// What the load says about the cut: over the spindle's power (a warning: the person may push
/// it, knowing), close to it (info), with the radial width that brings it to [`CLOSE_SHARE`].
pub fn issues(load: &Load, e: Engagement, tool: &Tool, material: Material, drive: AxisDrive) -> Vec<Issue> {
    let mut out = Vec::new();
    let name = format!("T{} {}", tool.number, tool.name);
    let over = match drive {
        AxisDrive::OpenLoopStepper => {
            "its open-loop steppers can lose steps without noticing, and the rest of the program is cut out of place: cut lighter or feed slower"
        }
        AxisDrive::ClosedLoopStepper | AxisDrive::Servo => {
            "the drives may stop the machine with a position error or let it chatter: cut lighter or feed slower, or keep it if you know the machine takes it"
        }
        AxisDrive::Unknown => "they may lose position or chatter: cut lighter or feed slower, or keep it if you know the machine takes it",
    };
    if load.axis_limit_n > 0.0 && load.axis_share > CLOSE_SHARE {
        let pct = (load.axis_share * 100.0).round();
        let msg = format!(
            "{name}: the cut pushes the tool with up to {} N, {pct} % of the {} N the X and Y drives take: {}",
            load.axis_force_n,
            load.axis_limit_n,
            if load.axis_share > 1.0 { over } else { "little margin" }
        );
        let issue = if load.axis_share > 1.0 { Issue::warning("axis_overload", msg) } else { Issue::info("axis_margin", msg) };
        out.push(issue.parameter("axis_share", load.axis_share).limit(1.0));
    }
    if load.spindle_w <= 0.0 {
        return out;
    }
    let pct = (load.spindle_share * 100.0).round();
    let what = if e.slotting { "a full-width cut" } else { "the cut" };
    let m = material.name();
    if load.spindle_share > 1.0 {
        out.push(
            Issue::warning(
                "spindle_overload",
                format!(
                    "{name}: {what} {} mm wide and {} mm deep in {m} takes about {} W, {pct} % of the spindle's {} W at this speed; it may stall or chatter: cut lighter, or keep it if you know the machine takes it",
                    load.ae_mm, load.ap_mm, load.power_w, load.spindle_w
                ),
            )
            .parameter("spindle_share", load.spindle_share)
            .limit(1.0),
        );
    } else if load.spindle_share > CLOSE_SHARE {
        out.push(
            Issue::info(
                "spindle_margin",
                format!(
                    "{name}: {what} takes about {} W, {pct} % of the spindle's {} W: little margin for harder stock or a dull tool",
                    load.power_w, load.spindle_w
                ),
            )
            .parameter("spindle_share", load.spindle_share)
            .limit(1.0),
        );
    }
    out
}
