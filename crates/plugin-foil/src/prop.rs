//! The B-Series Propeller, as the FeatureScript's `buildBlade` and `bSeriesProp` build it: the
//! Wageningen B-series chord, generator-line and thickness tables (V1, V2) interpolated along
//! the radius, the other section profiles, pitch with its tip change, skew, rake, a rounded
//! leading edge and tip dome, sections wrapped onto cylinders; the hub as a revolved outline
//! with nose, cone or dish tail, pocket and mount. Lengths in mm, angles in degrees. The shaft is
//! the Z axis, the blades are built pointing along +Y and the finished propeller is turned aft up
//! (180° about X), as the FeatureScript does.

use std::f64::consts::PI;

use solidcraft_plugin::{Issue, Severity};

use crate::schema::Values;
use crate::tables_gen as t;

const PITCH_REF_STATION: f64 = 0.7;
const NOSE_LENGTH_RATIO: f64 = 1.0;
const NOSE_ANGLES: [f64; 6] = [0.0, 10.0, 25.0, 45.0, 67.5, 90.0];
const TIP_BLEND_START: f64 = 0.9;
const TIP_ROUND_SPAN: f64 = 0.02;
const TIP_ROUND_FLOOR: f64 = 0.45;
const TIP_ROUND_SECTIONS: usize = 3;
const TIP_BLEND_EXPONENT: f64 = 0.7;

fn row_at(row: &[f64], p: f64) -> f64 {
    let ps = t::PROP_P_STATIONS;
    for j in 0..ps.len() - 1 {
        if p <= ps[j] && p >= ps[j + 1] {
            let f = (ps[j] - p) / (ps[j] - ps[j + 1]);
            return row[j] + f * (row[j + 1] - row[j]);
        }
    }
    row[row.len() - 1]
}

fn v1_row(i: usize, p: f64) -> f64 {
    t::PROP_V1_TABLE.get(i).map(|r| row_at(r, p)).unwrap_or(0.0)
}

fn v2_row(i: usize, p: f64) -> f64 {
    t::PROP_V2_TABLE.get(i).map(|r| row_at(r, p)).unwrap_or(1.0 - p * p)
}

fn bracket(x: f64) -> (usize, f64) {
    let ts = t::PROP_TABLE_STATIONS;
    for i in (0..ts.len() - 1).rev() {
        if x >= ts[i] {
            return (i, (x - ts[i]) / (ts[i + 1] - ts[i]));
        }
    }
    (0, 0.0)
}

fn table_slope(table: &[f64], i: usize) -> f64 {
    let ts = t::PROP_TABLE_STATIONS;
    let last = table.len() - 1;
    if i == 0 {
        return (table[1] - table[0]) / (ts[1] - ts[0]);
    }
    if i == last {
        return (table[last] - table[last - 1]) / (ts[last] - ts[last - 1]);
    }
    let d0 = (table[i] - table[i - 1]) / (ts[i] - ts[i - 1]);
    let d1 = (table[i + 1] - table[i]) / (ts[i + 1] - ts[i]);
    if d0 * d1 <= 0.0 {
        return 0.0;
    }
    let m = (d0 + d1) / 2.0;
    let limit = 3.0 * d0.abs().min(d1.abs());
    if m.abs() < limit { m } else { limit.copysign(m) }
}

/// Monotone cubic Hermite through a table along the radius (`blendTable`).
pub fn blend_table(table: &[f64], x: f64) -> f64 {
    let ts = t::PROP_TABLE_STATIONS;
    let (i, tt) = bracket(x);
    let h = ts[i + 1] - ts[i];
    let (m0, m1) = (table_slope(table, i), table_slope(table, i + 1));
    let (t2, t3) = (tt * tt, tt * tt * tt);
    (2.0 * t3 - 3.0 * t2 + 1.0) * table[i] + (t3 - 2.0 * t2 + tt) * h * m0 + (-2.0 * t3 + 3.0 * t2) * table[i + 1] + (t3 - t2) * h * m1
}

fn v1_at(x: f64, p: f64) -> f64 {
    let (i, f) = bracket(x);
    let lo = v1_row(i, p);
    lo + f * (v1_row(i + 1, p) - lo)
}

fn v2_at(x: f64, p: f64) -> f64 {
    let (i, f) = bracket(x);
    let lo = v2_row(i, p);
    lo + f * (v2_row(i + 1, p) - lo)
}

fn max_thickness_position(profile: &str) -> f64 {
    match profile {
        "NACA_66" => 0.45,
        "NACA_4DIGIT" | "CLARK_Y" => 0.30,
        "YS_920" => 0.343,
        "E817" => 0.329,
        _ => 0.50,
    }
}

/// (mean-line ordinate, half thickness) at chord station p (`sectionOrdinates`).
fn ordinates(profile: &str, x: f64, p: f64, t_max: f64, t_edge: f64, camber_h: f64) -> [f64; 2] {
    if profile == "B_SERIES" {
        let (v1, v2) = (v1_at(x, p), v2_at(x, p));
        let body = v2 * (t_max - t_edge);
        return [v1 * (t_max - t_edge) + body / 2.0 + t_edge / 2.0, body / 2.0];
    }
    let (thick, camber) = match profile {
        "NACA_16" => (t::PROP_NACA16_THICKNESS, t::PROP_NACA16_CAMBER),
        "NACA_66" => (t::PROP_NACA66_THICKNESS, t::PROP_NACA66_CAMBER),
        "NACA_4DIGIT" => (t::PROP_NACA4_THICKNESS, t::PROP_NACA16_CAMBER),
        "CLARK_Y" => (t::PROP_CLARKY_THICKNESS, t::PROP_CLARKY_CAMBER),
        "ELLIPTICAL" => (t::PROP_ELLIPSE_THICKNESS, t::PROP_NACA16_CAMBER),
        "YS_920" => (t::PROP_YS920_THICKNESS, t::PROP_YS920_CAMBER),
        "E817" => (t::PROP_E817_THICKNESS, t::PROP_E817_CAMBER),
        _ => (t::PROP_ARC_SHAPE, t::PROP_ARC_SHAPE),
    };
    let half = row_at(thick, p) * (t_max - t_edge) / 2.0;
    if profile == "OGIVAL" {
        return [half + t_edge / 2.0, half];
    }
    [camber_h * row_at(camber, p) + t_max / 2.0, half]
}

/// One blade section on its cylinder: radius, the angle its generator line sits at (rad), the
/// hand, and the closed outline in developed coordinates (u along the cylinder, w along the
/// shaft), back from the leading edge to the trailing edge then face back to the leading edge.
#[derive(Clone, Debug, PartialEq)]
pub struct BladeSection {
    pub x: f64,
    pub r: f64,
    pub theta0: f64,
    pub hand: f64,
    pub back: Vec<[f64; 2]>,
    pub face: Vec<[f64; 2]>,
    pub chord: f64,
    pub thickness: f64,
    pub pitch_angle: f64,
}

impl Cut {
    /// The tool grown by `d` all round (a revolve's outline offset outward, kept off the axis's
    /// far side).
    pub fn grown(&self, d: f64) -> Cut {
        match self {
            Cut::Cylinder { x, y, z0, z1, r } => Cut::Cylinder { x: *x, y: *y, z0: z0 - d, z1: z1 + d, r: r + d },
            Cut::Box { lo, hi } => Cut::Box { lo: lo.map(|v| v - d), hi: hi.map(|v| v + d) },
            Cut::Bolt { x, y, z0, z1, r, head, head_top } => {
                Cut::Bolt { x: *x, y: *y, z0: z0 - d, z1: z1 + d, r: r + d, head: if *head > 0.0 { head + d } else { 0.0 }, head_top: head_top + d }
            }
            Cut::Revolve(outline) => {
                let n = outline.len();
                // Signed area: positive when counter-clockwise, so the outward normal of an edge
                // (a, b) is (dy, -dx) then.
                let area: f64 = (0..n).map(|i| outline[i][0] * outline[(i + 1) % n][1] - outline[(i + 1) % n][0] * outline[i][1]).sum();
                let sign = if area >= 0.0 { 1.0 } else { -1.0 };
                let normal = |a: [f64; 2], b: [f64; 2]| {
                    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                    let l = dx.hypot(dy).max(1e-12);
                    [sign * dy / l, -sign * dx / l]
                };
                let out = (0..n)
                    .map(|i| {
                        let (p, a, b) = (outline[(i + n - 1) % n], outline[i], outline[(i + 1) % n]);
                        let (n1, n2) = (normal(p, a), normal(a, b));
                        let m = [n1[0] + n2[0], n1[1] + n2[1]];
                        let k = (m[0] * n2[0] + m[1] * n2[1]).max(0.2);
                        let q = [a[0] + d * m[0] / k, a[1] + d * m[1] / k];
                        // A point on the axis stays on it.
                        [if a[0] <= 1e-9 { 0.0 } else { q[0].max(0.0) }, q[1]]
                    })
                    .collect();
                Cut::Revolve(out)
            }
        }
    }

    /// How far the tool reaches from the shaft axis.
    pub fn reach(&self) -> f64 {
        match self {
            Cut::Revolve(outline) => outline.iter().map(|q| q[0]).fold(0.0, f64::max),
            Cut::Cylinder { x, y, r, .. } => x.hypot(*y) + r,
            Cut::Bolt { x, y, r, head, .. } => x.hypot(*y) + r.max(*head),
            Cut::Box { lo, hi } => {
                [lo[0].abs(), hi[0].abs()].into_iter().fold(0.0, f64::max).hypot([lo[1].abs(), hi[1].abs()].into_iter().fold(0.0, f64::max))
            }
        }
    }
}

impl BladeSection {
    /// A developed point on the cylinder, before the propeller is turned aft up.
    pub fn wrap(&self, u: f64, w: f64) -> [f64; 3] {
        let th = self.theta0 + u / self.r;
        [-self.hand * self.r * th.sin(), self.r * th.cos(), w]
    }
}

/// A tool the hub is cut with.
#[derive(Clone, Debug, PartialEq)]
pub enum Cut {
    /// A solid of revolution about Z from an (r, z) outline.
    Revolve(Vec<[f64; 2]>),
    /// A cylinder along Z: centre (x, y), from z0 to z1, radius.
    Cylinder { x: f64, y: f64, z0: f64, z1: f64, r: f64 },
    /// An axis-aligned box from one corner to the other.
    Box { lo: [f64; 3], hi: [f64; 3] },
    /// A bolt hole along Z at (x, y) from z0 to z1, radius r, with a head recess of radius
    /// `head` from z0 up to `head_top` (none when `head` is 0).
    Bolt { x: f64, y: f64, z0: f64, z1: f64, r: f64, head: f64, head_top: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prop {
    pub sections: Vec<BladeSection>,
    pub blades: usize,
    /// The hub with its nose and cone tail, as an (r, z) outline revolved about Z.
    pub hub: Vec<[f64; 2]>,
    /// What keeps the blades' roots inside the hub: everything of a blade inside these cylinders
    /// beyond the hub's end faces is cut away.
    pub blade_trims: Vec<Cut>,
    /// Cuts of the joined propeller (tail dish or pocket, bore, keyway, bolts).
    pub cuts: Vec<Cut>,
    pub diameter: f64,
    pub notice: String,
    /// What the build changed or could not do, coded for programs.
    pub issues: Vec<Issue>,
}

fn mm(x: f64) -> String {
    format!("{}", (x * 1000.0).round() / 1000.0)
}

/// The blade's sections (`buildBlade`); `hub_diameter` is the hub's diameter at the blade root plane.
pub fn blade(v: &Values, hub_diameter: f64, issues: &mut Vec<Issue>) -> Result<Vec<BladeSection>, String> {
    let d = v.num("diameter");
    let r_tip = d / 2.0;
    let x_hub = hub_diameter / d;
    let pitch = v.num("pdRatio") * d;
    let z = v.num("bladeCount").round() as usize;
    let (t_root, t_tip, t_edge) = (v.num("rootThickness"), v.num("tipThickness"), v.num("edgeThickness"));
    let hand = if v.choice("rotation") == "CCW" { -1.0 } else { 1.0 };
    let (k, ac, bc) = if z <= 3 { (t::PROP_K_Z3, t::PROP_AC_Z3, t::PROP_BC_Z3) } else { (t::PROP_K_Z4, t::PROP_AC_Z4, t::PROP_BC_Z4) };
    let ear = v.num("ear");
    let round_tip = v.flag("roundTip");
    let x_end = if round_tip { 1.0 - TIP_ROUND_SPAN } else { 1.0 };
    let count = (v.num("sectionCount").round() as usize).max(3);
    let mut stations = Vec::new();
    let mut scales = Vec::new();
    for i in 0..count {
        let u = i as f64 / (count - 1) as f64;
        stations.push(0.2 + (x_end - 0.2) * (0.5 * u + 0.5 * (u * PI / 2.0).sin()));
        scales.push(1.0);
    }
    let base_gap = (stations[count - 1] - stations[count - 2]) * r_tip;
    if base_gap < 0.4 {
        return Err(format!("Sections are crowding at the tip ({} mm apart): lower the section count", mm(base_gap)));
    }
    if round_tip {
        for k in 1..=TIP_ROUND_SECTIONS {
            let u = k as f64 / TIP_ROUND_SECTIONS as f64;
            stations.push(x_end + TIP_ROUND_SPAN * u);
            scales.push(TIP_ROUND_FLOOR.max((1.0 - u * u).max(0.0).sqrt()));
        }
    }
    if x_hub < 0.22 {
        return Err("Hub diameter is under 0.22 x D: the blade starts at r/R = 0.2 and would not bury into the hub".into());
    }
    if x_hub > 0.6 {
        return Err("Hub diameter is over 0.6 x D".into());
    }
    if t_tip < t_edge {
        return Err("Tip thickness must be at least the edge thickness".into());
    }
    if t_root <= t_tip {
        return Err("Root thickness must exceed tip thickness".into());
    }
    let chord_scale = d * ear / z as f64;
    if v.num("tipChord") >= 0.5 * blend_table(k, TIP_BLEND_START) * chord_scale {
        return Err(format!("Tip chord must be under half the chord where the tip blend starts (r/R = {TIP_BLEND_START})"));
    }
    let phi_hub = (pitch / (2.0 * PI * hub_diameter / 2.0)).atan();
    let chord_hub = blend_table(k, x_hub) * chord_scale;
    let circ = PI * hub_diameter;
    let used = z as f64 * chord_hub * phi_hub.cos();
    let ear_note = format!(
        " [EAR {ear} over {z} blades: a {} mm chord at the hub, {} % of the {} mm hub circumference]",
        mm(chord_hub),
        (100.0 * chord_hub * phi_hub.cos() / circ).round(),
        mm(circ)
    );
    if used > 1.3 * circ {
        return Err(format!(
            "At EAR {ear} with {z} blades the blades overlap badly where they leave the hub ({} mm of chord on a {} mm circumference). Lower EAR, raise the hub diameter, or use fewer blades.",
            mm(used),
            mm(circ)
        ));
    }
    if used > 0.95 * circ {
        issues.push(
            Issue::new(
                Severity::Warning,
                "blades_touch_at_hub",
                format!("Blades will touch or overlap at the hub when patterned: {} mm of chord on a {} mm circumference", mm(used), mm(circ)),
            )
            .on("ear", ear)
            .limit((ear * 0.95 * circ / used * 1000.0).floor() / 1000.0)
            .suggest("ear", (ear * 0.95 * circ / used * 1000.0).floor() / 1000.0),
        );
    }
    if NOSE_LENGTH_RATIO * t_edge > 0.25 * v.num("tipChord") {
        return Err("Edge thickness is too large for the tip chord: the rounded nose would swallow the tip section".into());
    }
    let profile = v.choice("sectionProfile");
    let mut out = Vec::new();
    for (i, (&x, &scale)) in stations.iter().zip(&scales).enumerate() {
        let r = r_tip * x;
        let n = (x - x_hub) / (1.0 - x_hub);
        let t_max = (t_tip + (t_root - t_tip) * (1.0 - n).powf(v.num("taperExponent"))) * scale;
        let t_edge_l = t_edge * scale;
        let nose = NOSE_LENGTH_RATIO * t_edge_l;
        let mut c = blend_table(k, x) * chord_scale;
        if x > TIP_BLEND_START {
            let c_start = blend_table(k, TIP_BLEND_START) * chord_scale;
            let c_trend = c_start + (x - TIP_BLEND_START) * table_slope(k, 7) * chord_scale;
            let m = (x - TIP_BLEND_START) / (1.0 - TIP_BLEND_START);
            c = v.num("tipChord") + (c_trend - v.num("tipChord")) * (1.0 - m * m).max(0.0).powf(TIP_BLEND_EXPONENT);
        }
        c *= scale;
        let a = blend_table(ac, x) * c;
        let mut b = blend_table(bc, x) * c;
        if profile != "B_SERIES" {
            b = max_thickness_position(profile) * c;
        }
        let camber_h = v.num("camberPercent") / 100.0 * c;
        if nose > 0.4 * b {
            return Err(format!(
                "Edge thickness is too large for the chord at r/R = {}: the nose would swallow the section. Raise tip chord or lower edge thickness.",
                (x * 100.0).round() / 100.0
            ));
        }
        let s_max = a - b;
        let pitch_blend = if x > PITCH_REF_STATION { ((x - PITCH_REF_STATION) / (1.0 - PITCH_REF_STATION)).powi(2) } else { 0.0 };
        let local_pitch = pitch * (1.0 + v.num("tipPitchChange") / 100.0 * pitch_blend);
        let phi = (local_pitch / (2.0 * PI * r)).atan();
        if c * phi.cos() / r > 4.5 {
            return Err(format!("Section at r/R = {x} wraps more than ~260 deg round the shaft: reduce EAR or raise the blade count.{ear_note}"));
        }
        let theta0 = -v.num("skewAngle").to_radians() * n;
        let z0 = -r * v.num("rake").to_radians().tan();
        // Developed coordinates: u = s cos(phi) - y sin(phi) round the cylinder, w = z0 + s sin(phi) + y cos(phi).
        let dev = |s: f64, y: f64| [s * phi.cos() - y * phi.sin(), z0 + s * phi.sin() + y * phi.cos()];
        let (mut back, mut face) = (Vec::new(), Vec::new());
        for (kk, ang) in NOSE_ANGLES.iter().enumerate() {
            let psi = ang.to_radians();
            let setback = nose * (1.0 - psi.cos());
            let half_edge = (t_edge_l / 2.0) * psi.sin();
            let p = 1.0 - setback / b;
            let s = s_max + p * b;
            let o = ordinates(profile, x, p, t_max, t_edge_l, camber_h);
            back.push(dev(s, o[0] + o[1] + half_edge));
            if kk == 0 {
                face.push(back[0]);
            } else {
                face.push(dev(s, o[0] - o[1] - half_edge));
            }
        }
        for &p in t::PROP_P_STATIONS {
            if p > 0.0 && p * b > b - 1.4 * nose {
                continue;
            }
            let s = s_max + if p >= 0.0 { p * b } else { p * (c - b) };
            let o = ordinates(profile, x, p, t_max, t_edge_l, camber_h);
            face.push(dev(s, o[0] - o[1] - t_edge_l / 2.0));
            back.push(dev(s, o[0] + o[1] + t_edge_l / 2.0));
        }
        let wrap = c * phi.cos() / r;
        if t_max > 0.85 * c {
            return Err(format!(
                "Section {i} at r/R {} is {} mm thick on a {} mm chord and cannot close: lower Root thick. or Taper exponent, or raise EAR or Diameter.{ear_note}",
                (x * 100.0).round() / 100.0,
                mm(t_max),
                mm(c)
            ));
        }
        if wrap > 160.0 * PI / 180.0 {
            return Err(format!(
                "Section {i} at r/R {} wraps {} deg round its {} mm radius and folds onto itself: lower EAR, raise Diameter or the hub, or use more blades.{ear_note}",
                (x * 100.0).round() / 100.0,
                (wrap * 180.0 / PI).round(),
                mm(r)
            ));
        }
        out.push(BladeSection { x, r, theta0, hand, back, face, chord: c, thickness: t_max, pitch_angle: phi.to_degrees() });
    }
    Ok(out)
}

/// The whole propeller (`bSeriesProp`, fixed blades).
pub fn build(v: &Values) -> Result<Prop, String> {
    let mut issues = Vec::new();
    let ru = v.num("hubUpstream") / 2.0;
    let rd = v.num("hubDownstream") / 2.0;
    let z_top = v.num("hubShift") + v.num("hubLength") / 2.0;
    let z_bot = v.num("hubShift") - v.num("hubLength") / 2.0;
    let fz = z_top / (z_top - z_bot);
    let mut r_blade = ru + (rd - ru) * fz;
    if z_top < 0.0 || z_bot > 0.0 {
        r_blade = ru.min(rd);
        issues.push(
            Issue::new(
                Severity::Warning,
                "blade_root_outside_hub",
                "The blade root plane (z = 0) lies outside the hub: reduce Axial shift or lengthen the hub. Building to the smaller hub diameter.",
            )
            .on("hubShift", v.num("hubShift"))
            .suggest("hubShift", 0.0),
        );
    }
    let sections = blade(v, 2.0 * r_blade, &mut issues)?;
    let d = v.num("diameter");
    let z = v.num("bladeCount").round() as usize;
    if v.choice("hubType") == "FOLDING" {
        issues.push(
            Issue::new(Severity::Warning, "folding_hub_not_built", "Folding blades are not built in SolidCraft yet: built with fixed blades")
                .on("hubType", "FOLDING")
                .suggest("hubType", "FIXED"),
        );
    }
    // The hub outline (r, z), with a nose and a cone tail.
    let nose = v.num("noseLength");
    let mut hub = vec![[0.0, z_bot], [rd, z_bot], [ru, z_top], [0.0, z_top]];
    if nose > 0.0 {
        hub = vec![[0.0, z_bot], [rd, z_bot], [ru, z_top], [ru / 5.0, z_top + nose], [0.0, z_top + nose]];
    }
    let tail_len = if v.choice("tailStyle") == "CONE" { v.num("tailLength") } else { 0.0 };
    if tail_len > 0.0 {
        let mut h2 = vec![[0.0, z_bot - tail_len], [rd / 5.0, z_bot - tail_len]];
        h2.extend_from_slice(&hub[1..]);
        hub = h2;
    }
    let trim_len = 0.6 * d;
    // The FeatureScript trims from 0.001 mm beyond the end faces; these start 0.05 mm inside them,
    // so the trimmed root ends inside the hub instead of a hair off its face (a boolean of two
    // nearly coincident faces is slow and unreliable here), at the cost of a 0.05 mm step where
    // the blade leaves the hub's edge.
    let blade_trims = vec![
        Cut::Cylinder { x: 0.0, y: 0.0, z0: z_top - 0.05, z1: z_top + trim_len, r: ru + 0.5 },
        Cut::Cylinder { x: 0.0, y: 0.0, z0: z_bot - trim_len, z1: z_bot + 0.05, r: rd + 0.5 },
    ];
    // Tail dish or pocket, then the mount.
    let mut cuts = Vec::new();
    let z_lo = z_bot - tail_len - 1.0;
    let z_hi = z_top + nose + 1.0;
    let pocket_r = v.num("pocketDia") / 2.0;
    let z_floor = z_top - if pocket_r > 0.0 { v.num("pocketFloor") } else { 0.0 };
    if v.choice("tailStyle") == "DISH" {
        let rise = (rd - pocket_r) * v.num("dishAngle").to_radians().tan();
        let mut z_dish = z_bot + rise;
        let z_dish_max = z_top - v.num("pocketFloor").max(2.0);
        if z_dish > z_dish_max {
            let fits = ((z_dish_max - z_bot) / (rd - pocket_r)).atan().to_degrees().floor();
            issues.push(
                Issue::new(
                    Severity::Warning,
                    "dish_flattened",
                    format!("The dish would rise {} mm and break through the upstream face: it is flattened", mm(rise)),
                )
                .on("dishAngle", v.num("dishAngle"))
                .limit(fits)
                .suggest("dishAngle", fits),
            );
            z_dish = z_dish_max;
        }
        let z_out = z_bot - 2.0 * (z_dish - z_bot) / (rd - pocket_r);
        let mut pts = vec![[0.0, z_bot - 3.0], [rd + 2.0, z_bot - 3.0], [rd + 2.0, z_out], [pocket_r, z_dish]];
        if pocket_r > 0.0 && z_floor > z_dish {
            pts.extend([[pocket_r, z_floor], [0.0, z_floor]]);
        } else if pocket_r > 0.0 {
            pts.push([0.0, z_dish]);
        }
        cuts.push(Cut::Revolve(pts));
    } else if pocket_r > 0.0 {
        cuts.push(Cut::Revolve(vec![[0.0, z_lo], [pocket_r, z_lo], [pocket_r, z_floor], [0.0, z_floor]]));
    }
    match v.choice("mountType") {
        mt @ ("BORE" | "KEYED") => {
            let rb = v.num("boreDia") / 2.0;
            cuts.push(Cut::Cylinder { x: 0.0, y: 0.0, z0: z_lo, z1: z_hi, r: rb });
            if mt == "KEYED" {
                let kw = v.num("keyWidth");
                cuts.push(Cut::Box { lo: [-kw / 2.0, 0.0, z_lo], hi: [kw / 2.0, rb + v.num("keyDepth"), z_hi] });
            }
        }
        "BOLTED" => {
            if v.num("pilotDia") > 0.0 {
                cuts.push(Cut::Cylinder { x: 0.0, y: 0.0, z0: z_lo, z1: z_hi, r: v.num("pilotDia") / 2.0 });
            }
            let (hole, head) = match v.choice("boltSize") {
                "M3" => (3.4, 6.5),
                "M4" => (4.5, 8.0),
                "M5" => (5.5, 9.5),
                "M6" => (6.6, 11.0),
                "M8" => (9.0, 14.0),
                _ => (v.num("boltDia"), v.num("headDia")),
            };
            let recess = v.flag("recessHeads") && head > hole;
            let count = v.num("boltCount").round() as usize;
            let z_head = z_top - v.num("boltFlange");
            for k in 0..count {
                let ang = 2.0 * PI * k as f64 / count as f64;
                let (cx, cy) = (v.num("boltPcd") / 2.0 * ang.cos(), v.num("boltPcd") / 2.0 * ang.sin());
                let head = if recess && z_head > z_lo { head / 2.0 } else { 0.0 };
                cuts.push(Cut::Bolt { x: cx, y: cy, z0: z_lo, z1: z_hi, r: hole / 2.0, head, head_top: z_head });
            }
            if recess && z_head <= z_lo {
                issues.push(
                    Issue::new(
                        Severity::Warning,
                        "bolt_head_recess_dropped",
                        "The bolt flange is thicker than the hub, so there is no room for the head recess: plain through holes cut instead",
                    )
                    .on("boltFlange", v.num("boltFlange")),
                );
            }
        }
        _ => {}
    }
    let root = sections.first();
    let notice = format!(
        "Propeller {} mm x {}, P/D {} (pitch {} mm), EAR {}, {}{}",
        mm(d),
        z,
        v.num("pdRatio"),
        mm(v.num("pdRatio") * d),
        v.num("ear"),
        if v.choice("rotation") == "CCW" { "left-hand" } else { "right-hand" },
        root.map(|s| format!(", root section chord {} mm, {} mm thick", mm(s.chord), mm(s.thickness))).unwrap_or_default()
    );
    Ok(Prop { sections, blades: z, hub, blade_trims, cuts, diameter: d, notice, issues })
}
