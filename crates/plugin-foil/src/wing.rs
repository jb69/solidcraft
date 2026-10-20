//! The Foil wing and Foil stabiliser surfaces, as the FeatureScript's `buildFoil` builds them:
//! the area-first planform solve, the superellipse / linear / double-taper chord with its
//! plan-rounded tip, alignment line, power-law sweep, drop law and gull dihedral, winglets,
//! leading-edge tubercles, washout, thickness law, tip cap and the sections at the loft's
//! stations. Lengths in mm, angles in degrees, X downstream, Y along the span, Z up; the root
//! pivot sits at `base`.

use solidcraft_plugin::{Issue, Severity};
use std::f64::consts::PI;

use crate::schema::Values;
use crate::section::{MeanLine, Profile, SectionDef};

/// The chord law.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Superellipse,
    Linear,
    Double,
}

/// The planform law (`pl` in the FeatureScript).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Planform {
    pub shape: Shape,
    pub ratio: f64,
    pub n: f64,
    pub brk: f64,
    pub mid: f64,
    pub blend: f64,
    /// Plan-view tip rounding starts here (1 = none) and ends at `cap_end` of the chord.
    pub cap_eta: f64,
    pub cap_end: f64,
    /// Root tail: `tail` × c0 added at the centreline, fading out at `tail_eta`.
    pub tail: f64,
    pub tail_eta: f64,
    pub drop: DropLaw,
    /// Leading-edge tubercles: amplitude and pitch in mm, from and to as fractions of the half span.
    pub bump: Option<[f64; 4]>,
}

/// The tip drop's shape along the span (`dropShape`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropLaw {
    pub exp: f64,
    pub from: f64,
    pub to: f64,
    pub smooth: bool,
}

impl DropLaw {
    pub fn shape(&self, eta: f64) -> f64 {
        if eta <= self.from {
            return 0.0;
        }
        if eta >= self.to {
            return 1.0;
        }
        let u = (eta - self.from) / (self.to - self.from);
        if self.smooth { u * u * (3.0 - 2.0 * u) } else { u.powf(self.exp) }
    }
    pub fn slope(&self, eta: f64) -> f64 {
        if eta <= self.from || eta >= self.to {
            return 0.0;
        }
        let w = self.to - self.from;
        let u = (eta - self.from) / w;
        if self.smooth {
            6.0 * u * (1.0 - u) / w
        } else if self.exp == 1.0 {
            1.0 / w
        } else {
            self.exp * u.powf(self.exp - 1.0) / w
        }
    }
}

/// Scale so the truncated superellipse equals `ratio` at eta = 1.
pub fn se_scale(ratio: f64, n: f64) -> f64 {
    (1.0 - ratio.powf(n)).powf(1.0 / n)
}

impl Planform {
    /// Normalised chord g(eta) (`chordFactor`).
    pub fn chord(&self, eta: f64) -> f64 {
        let g = match self.shape {
            Shape::Linear => 1.0 - (1.0 - self.ratio) * eta,
            Shape::Double => {
                let (brk, mid, d) = (self.brk, self.mid, self.blend);
                if d > 0.0 && (eta - brk).abs() < d {
                    let t = (eta - (brk - d)) / (2.0 * d);
                    let p0 = 1.0 - (1.0 - mid) * (brk - d) / brk;
                    let p2 = mid - (mid - self.ratio) * d / (1.0 - brk);
                    (1.0 - t) * (1.0 - t) * p0 + 2.0 * t * (1.0 - t) * mid + t * t * p2
                } else if eta < brk {
                    1.0 - (1.0 - mid) * eta / brk
                } else {
                    mid - (mid - self.ratio) * (eta - brk) / (1.0 - brk)
                }
            }
            Shape::Superellipse => {
                let v = 1.0 - (se_scale(self.ratio, self.n) * eta).powf(self.n);
                if v <= 0.0 { 0.0 } else { v.powf(1.0 / self.n) }
            }
        };
        g * self.cap_plan(eta) + self.tail_factor(eta)
    }
    /// Plan-view tip rounding multiplier (`capPlanFactor`).
    pub fn cap_plan(&self, eta: f64) -> f64 {
        if self.cap_eta >= 1.0 || eta <= self.cap_eta {
            return 1.0;
        }
        let u = ((eta - self.cap_eta) / (1.0 - self.cap_eta)).min(1.0);
        self.cap_end + (1.0 - self.cap_end) * (1.0 - u * u).max(0.0).sqrt()
    }
    /// The root tail's share of g (`tailFactor`).
    pub fn tail_factor(&self, eta: f64) -> f64 {
        if self.tail <= 0.0 || eta >= self.tail_eta {
            return 0.0;
        }
        let u = eta / self.tail_eta;
        self.tail * (2.0 * u * u * u - 3.0 * u * u + 1.0)
    }
    /// How far the leading edge is forward of the smooth outline at eta, mm (`bumpOffset`).
    pub fn bump(&self, eta: f64, h: f64) -> f64 {
        let Some([amp, pitch, from, to]) = self.bump else { return 0.0 };
        let (y, y0, y1) = (eta * h, from * h, to * h);
        if y < y0 || y > y1 || pitch <= 0.0 {
            return 0.0;
        }
        let ramp = pitch / 2.0;
        let mut w: f64 = 1.0;
        if y - y0 < ramp {
            w = 0.5 * (1.0 - (PI * (y - y0) / ramp).cos());
        }
        if y1 - y < ramp {
            w = w.min(0.5 * (1.0 - (PI * (y1 - y) / ramp).cos()));
        }
        0.5 * amp * (2.0 * PI * (y - y0) / pitch).cos() * w
    }
    /// Simpson integrals of g over the half span (`shapeIntegral`): kind 0 g, 1 g², 2 the
    /// developed g (k = tip drop / h, td = tan of the gull dihedral).
    pub fn integral(&self, kind: u8, k: f64, td: f64) -> f64 {
        const N: usize = 400;
        let mut sum = 0.0;
        for i in 0..=N {
            let eta = i as f64 / N as f64;
            let w = if i == 0 || i == N {
                1.0
            } else if i % 2 == 1 {
                4.0
            } else {
                2.0
            };
            let g = self.chord(eta);
            let v = match kind {
                1 => g * g,
                2 => {
                    let ps = self.drop.slope(eta);
                    let s = -k * ps + td * (1.0 - ps);
                    g * (1.0 + s * s).sqrt()
                }
                _ => g,
            };
            sum += w * v;
        }
        sum / (3.0 * N as f64)
    }
    /// Integral of g from eta0 to 1 (`shapeIntegralFrom`).
    pub fn integral_from(&self, eta0: f64) -> f64 {
        const N: usize = 200;
        let mut sum = 0.0;
        for i in 0..=N {
            let w = if i == 0 || i == N {
                1.0
            } else if i % 2 == 1 {
                4.0
            } else {
                2.0
            };
            sum += w * self.chord(eta0 + (1.0 - eta0) * i as f64 / N as f64);
        }
        sum * (1.0 - eta0) / (3.0 * N as f64)
    }
}

/// A section plane: `origin + u·x + v·y` (mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin: [f64; 3],
    pub x: [f64; 3],
    pub y: [f64; 3],
}

impl Frame {
    pub fn world(&self, p: [f64; 2]) -> [f64; 3] {
        [0, 1, 2].map(|i| self.origin[i] + p[0] * self.x[i] + p[1] * self.y[i])
    }
}

/// One loft station: its place along the half span, chord and section in its plane.
#[derive(Clone, Debug, PartialEq)]
pub struct Station {
    pub eta: f64,
    pub chord: f64,
    pub frame: Frame,
    /// Upper trailing edge → leading edge → lower trailing edge, placed and twisted, mm in the plane.
    pub points: Vec<[f64; 2]>,
}

/// The built surface: its stations (root first) and what the notice reports.
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    pub stations: Vec<Station>,
    pub mirror: bool,
    pub span: f64,
    pub root_chord: f64,
    pub tip_chord: f64,
    pub area_proj: f64,
    pub area_dev: f64,
    pub mac: f64,
    pub aspect: f64,
    pub incidence: f64,
    pub root_thickness: f64,
    pub tip_thickness: f64,
    pub notice: String,
    /// What the solve changed on its own (the notice says it too), coded for programs.
    pub issues: Vec<Issue>,
}

/// What a linked stabiliser reads from the wing above it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WingLink {
    /// Projected area, mm².
    pub area: f64,
    pub aspect: f64,
    pub root_thickness: f64,
    pub tip_thickness: f64,
    pub incidence: f64,
}

impl Surface {
    pub fn link(&self) -> WingLink {
        WingLink {
            area: self.area_proj,
            aspect: self.aspect,
            root_thickness: self.root_thickness,
            tip_thickness: self.tip_thickness,
            incidence: self.incidence,
        }
    }
}

/// The definition, with what the FeatureScript adds or overrides (a linked stab's values).
struct Def<'a> {
    v: &'a Values,
    area: f64,
    aspect: f64,
    root_t: f64,
    tip_t: f64,
    incidence: f64,
    area_mode: bool,
    developed: bool,
}

impl Def<'_> {
    fn n(&self, k: &str) -> f64 {
        self.v.num(k)
    }
}

/// `pivotFraction`.
pub fn pivot_fraction(pivot: &str) -> f64 {
    match pivot {
        "LEADING_EDGE" => 0.0,
        "TRAILING_EDGE" => 1.0,
        _ => 0.25,
    }
}

/// `solvePlanform`: (G, G2, span, c0, pl with its cap start).
fn solve(d: &Def, mut pl: Planform, plan_cap_len: f64) -> (f64, f64, f64, f64, Planform) {
    let cap_eta = |span: f64| if plan_cap_len > 0.0 { (1.0 - plan_cap_len / (span / 2.0)).max(0.5) } else { 1.0 };
    let (span, c0);
    if d.area_mode {
        let target = d.area * 100.0;
        let mut s_proj = target;
        let mut sp = (d.aspect * s_proj).sqrt();
        pl.cap_eta = cap_eta(sp);
        if d.developed {
            for _ in 0..12 {
                let b = (d.aspect * s_proj).sqrt();
                pl.cap_eta = cap_eta(b);
                let r = pl.integral(2, d.n("tipDrop") / (b / 2.0), d.n("rootDihedral").to_radians().tan()) / pl.integral(0, 0.0, 0.0);
                s_proj = target / r;
            }
            sp = (d.aspect * s_proj).sqrt();
            pl.cap_eta = cap_eta(sp);
        }
        span = sp;
        c0 = s_proj / (span * pl.integral(0, 0.0, 0.0));
    } else {
        span = d.n("span");
        c0 = d.n("rootChord");
        pl.cap_eta = cap_eta(span);
    }
    (pl.integral(0, 0.0, 0.0), pl.integral(1, 0.0, 0.0), span, c0, pl)
}

/// The winglet path at developed distance s beyond the bend start (`wingletPath`): (y, z, phi rad).
fn winglet_path(s: f64, s0: f64, z0: f64, phi0: f64, sign: f64, ang: f64, r: f64) -> (f64, f64, f64) {
    let ds = s - s0;
    let arc = r * ang;
    let (mut y, mut z) = (s0, z0);
    const N: usize = 24;
    for q in 0..N {
        let sm = ds * (q as f64 + 0.5) / N as f64;
        let ph = phi0 + sign * ang * (sm / arc).min(1.0);
        y += ds / N as f64 * ph.cos();
        z += ds / N as f64 * ph.sin();
    }
    (y, z, phi0 + sign * ang * (ds / arc).min(1.0))
}

/// The keys `foilStationEtas` reads.
struct StationKeys {
    h: f64,
    count: usize,
    tip_on: bool,
    blend_from: f64,
    winglet: Option<(f64, f64, f64)>,
    cap: Option<f64>,
    has_drop: bool,
}

/// The loft's stations as etas in [0, 1] (`foilStationEtas`).
fn station_etas(pl: &Planform, k: &StationKeys) -> Vec<f64> {
    let h = k.h;
    let n = k.count.max(2);
    let mut es: Vec<f64> = Vec::new();
    if pl.shape == Shape::Superellipse {
        let sc = se_scale(pl.ratio, pl.n);
        let phi_end = sc.powf(pl.n / 2.0).min(1.0).asin();
        for i in 0..n {
            let phi = phi_end * i as f64 / (n - 1) as f64;
            es.push((phi.sin().powf(2.0 / pl.n) / sc).min(1.0));
        }
    } else {
        for i in 0..n {
            es.push((PI / 2.0 * i as f64 / (n - 1) as f64).sin());
        }
    }
    if pl.shape == Shape::Double {
        let d = if pl.blend > 0.0 { pl.blend } else { 0.03 };
        for e in [pl.brk - d, pl.brk - d / 2.0, pl.brk, pl.brk + d / 2.0, pl.brk + d] {
            es.push(e.clamp(0.0, 1.0));
        }
    }
    if pl.tail > 0.0 {
        for e in [pl.tail_eta / 3.0, 2.0 * pl.tail_eta / 3.0, pl.tail_eta] {
            es.push(e.min(1.0));
        }
    }
    if let Some([_, pitch, from, to]) = pl.bump {
        let mut yb = from * h;
        let y_end = (to * h).min(h);
        while yb <= y_end && pitch > 0.0 {
            es.push(yb / h);
            yb += pitch / 6.0;
        }
    }
    if k.tip_on {
        for j in 0..=6 {
            es.push(k.blend_from + (1.0 - k.blend_from) * j as f64 / 6.0);
        }
    }
    if let Some((w_eta, w_r, w_ang)) = k.winglet {
        let arc = w_r * w_ang;
        for j in 0..=4 {
            es.push(((w_eta * h + arc * j as f64 / 4.0) / h).min(1.0));
        }
    }
    if let Some(cap) = k.cap
        && cap > 0.0
    {
        let start = h - cap;
        for j in 0..=4 {
            es.push((start + cap * (PI / 2.0 * j as f64 / 4.0).sin()) / h);
        }
    }
    if k.has_drop {
        let (db, dt) = (pl.drop.from, pl.drop.to);
        let mut drop_es = vec![db + 0.02, db + 0.05, dt - 0.05, dt - 0.02];
        if dt < 1.0 {
            drop_es.extend([dt + 0.02, dt + 0.05]);
        }
        for j in 0..=12 {
            drop_es.push(db + (dt - db) * j as f64 / 12.0);
        }
        for e in drop_es {
            let ec = e.clamp(0.0, 1.0);
            if !es.iter().any(|x| (x - ec).abs() < 1.5 / h) {
                es.push(ec);
            }
        }
    }
    es.sort_by(f64::total_cmp);
    let mut out: Vec<f64> = Vec::new();
    for e in es {
        let ec = e.clamp(0.0, 1.0);
        if out.last().is_none_or(|l| ec - l > 0.3 / h) {
            out.push(ec);
        }
    }
    if let Some(f) = out.first_mut() {
        *f = 0.0;
    }
    if let Some(l) = out.last_mut() {
        *l = 1.0;
    }
    out
}

/// The FeatureScript's `fmt`: rounded to `d` decimals, printed without trailing zeros.
fn fmt(x: f64, d: i32) -> String {
    let k = 10f64.powi(d);
    format!("{}", (x * k).round() / k)
}

/// Build a wing or stabiliser from its dialog values (`buildFoil`). `base` is the root pivot
/// (the stab's arm and height); `link` the wing a linked stabiliser follows.
pub fn build(v: &Values, base: [f64; 3], link: Option<WingLink>) -> Result<Surface, String> {
    let mut d = Def {
        v,
        area: v.num("targetArea"),
        aspect: v.num("aspectRatio"),
        root_t: v.num("rootThickness"),
        tip_t: v.num("tipThickness"),
        incidence: v.num("incidence"),
        area_mode: v.choice("planformMode") != "SPAN_CHORD",
        developed: v.choice("areaDefinition") == "DEVELOPED",
    };
    let mut linked_note = "";
    if v.choice("stabLink") == "LINKED" {
        match link {
            Some(w) => {
                d.area = w.area / 100.0 * v.num("linkAreaRatio");
                d.aspect = w.aspect + v.num("linkAROffset");
                d.root_t = w.root_thickness + v.num("linkThicknessOffset");
                d.tip_t = w.tip_thickness + v.num("linkThicknessOffset");
                d.incidence = w.incidence - v.num("linkDecalage");
                d.area_mode = true;
                d.developed = false;
            }
            None => linked_note = " | linked, but no Foil wing above it: its own values are used",
        }
    }
    let invert = v.flag("invertSection") && v.choice("profile") != "NACA4";

    // ---- planform solve
    let shape = match v.choice("chordShape") {
        "LINEAR" => Shape::Linear,
        "DOUBLE" => Shape::Double,
        _ => Shape::Superellipse,
    };
    let is_double = shape == Shape::Double;
    let n = if shape == Shape::Superellipse { v.num("superN") } else { 2.0 };
    let ratio_in = if shape == Shape::Superellipse { v.num("endRatio") } else { v.num("tipRatio") };
    let brk = if is_double { v.num("breakPct") / 100.0 } else { 0.3 };
    let mid = if is_double { v.num("midRatio") } else { 0.6 };
    let blend = if is_double { (v.num("blendPct") / 100.0).min(0.95 * brk).min(0.95 * (1.0 - brk)) } else { 0.0 };
    if is_double && ratio_in > mid {
        return Err(format!("Tip ratio ({}) exceeds Mid ratio ({}): the outer taper would widen towards the tip", fmt(ratio_in, 2), fmt(mid, 2)));
    }
    let cap_on = v.choice("tipCap") != "FLAT";
    let plan_round = v.choice("tipCap") == "ROUND_PLAN";
    let cap_len = if cap_on { v.num("capLength") } else { 0.0 };
    let tail_on = v.flag("rootTailOn") && v.tool.field("rootTailLen").is_some();
    let tail_eta = if tail_on { v.num("rootTailSpan") / 100.0 } else { 0.3 };
    let drop_from = v.num("dropFrom") / 100.0;
    let drop_to = (v.num("dropTo") / 100.0).max(drop_from + 0.05);
    let drop = DropLaw { exp: v.num("dropExp"), from: drop_from, to: drop_to, smooth: v.choice("dropLaw") == "SCURVE" };
    let bump = v.flag("bumpOn").then(|| [v.num("bumpAmp"), v.num("bumpPitch"), v.num("bumpFrom") / 100.0, v.num("bumpTo") / 100.0]);
    let min_tip_on = v.flag("minTipOn");
    let mut ratio = ratio_in;
    let mut tip_raised = false;
    let mut tail = 0.0;
    let mut solved = None;
    for _pass in 0..8 {
        let pl0 = Planform { shape, ratio, n, brk, mid, blend, cap_eta: 1.0, cap_end: v.num("capEnd") / 100.0, tail, tail_eta, drop, bump: None };
        let (g, g2, span, c0, mut pl) = solve(&d, pl0, if plan_round { cap_len } else { 0.0 });
        pl.bump = bump;
        solved = Some((g, g2, span, c0, pl));
        let mut settled = true;
        if tail_on {
            let tail_new = v.num("rootTailLen") / c0;
            settled = (tail_new - tail).abs() < 1e-4;
            tail = tail_new;
        }
        if !min_tip_on && settled {
            break;
        }
        if !min_tip_on {
            continue;
        }
        let tip_chord = c0 * pl.chord(pl.cap_eta);
        let need = v.num("minTip") / (d.tip_t / 100.0);
        if tip_chord >= need - 1e-6 {
            if settled {
                break;
            }
            continue;
        }
        ratio = (if is_double { mid } else { 0.95 }).min(ratio * need / tip_chord);
        tip_raised = true;
    }
    let Some((g, g2, span, c0, pl)) = solved else { return Err("the planform did not solve".into()) };
    if !(span > 0.0 && c0 > 0.0 && span.is_finite() && c0.is_finite()) {
        return Err("the planform did not solve (span or root chord not positive)".into());
    }
    let h = span / 2.0;
    let tip_drop = v.num("tipDrop");
    let dihedral = v.num("rootDihedral").to_radians();
    let drop_k = tip_drop / h;
    let area_proj = span * c0 * g;
    let area_dev = span * c0 * pl.integral(2, drop_k, dihedral.tan());
    let mac = span * c0 * c0 * g2 / area_proj;
    let aspect = span * span / area_proj;
    let tip_chord = c0 * pl.chord(pl.cap_eta);

    // ---- validation
    let te = v.num("teThickness");
    if te > 0.03 * c0 {
        return Err(format!("Trailing edge thickness exceeds 3% of the root chord ({} mm)", c0.round()));
    }
    if cap_on && cap_len > 0.3 * h {
        return Err("Tip cap is longer than 30% of the half span".into());
    }

    // ---- winglet
    let winglet = v.choice("winglet");
    let winglet_on = !winglet.is_empty() && winglet != "OFF";
    let (mut w_eta, mut w_sign, mut w_ang, mut w_r, mut w_clamped) = (1.0, 0.0, 0.0, 0.0, false);
    let (mut w_s0, mut w_z0, mut w_phi0) = (h, 0.0, 0.0);
    if winglet_on {
        w_sign = if winglet == "UP" { 1.0 } else { -1.0 };
        w_ang = v.num("wingletAngle").to_radians();
        let target = v.num("wingletArea") * 100.0;
        let (mut lo, mut hi) = (0.3, 1.0);
        for _ in 0..40 {
            let em = 0.5 * (lo + hi);
            if pl.integral_from(em) * c0 * h > target {
                lo = em;
            } else {
                hi = em;
            }
        }
        w_eta = 0.5 * (lo + hi);
        if w_eta < 0.3 + 1e-4 {
            return Err(format!(
                "Winglet area {} cm² would start the bend inside 30 % of the half span - reduce it (the outer 70 % holds {} cm²)",
                fmt(v.num("wingletArea"), 1),
                fmt(pl.integral_from(0.3) * c0 * h / 100.0, 1)
            ));
        }
        w_s0 = w_eta * h;
        let ps0 = drop.slope(w_eta);
        let f0 = drop.shape(w_eta);
        w_z0 = -tip_drop * f0 + h * dihedral.tan() * (w_eta - f0);
        w_phi0 = ((-tip_drop * ps0 + h * dihedral.tan() * (1.0 - ps0)) / h).atan();
        w_r = v.num("wingletRadius");
        let w_len = h - w_s0;
        if w_r * w_ang > w_len {
            w_r = w_len / w_ang;
            w_clamped = true;
        }
    }

    // ---- sections
    let mean_line = MeanLine::of(v.choice("meanLine"), v.num("meanA"), v.num("reflex"), v.num("reflexFrom"));
    let root = Profile { profile: v.choice("profile"), camber: v.num("camber"), camber_pos: v.num("camberPos"), mean_line, invert };
    let tip_profile = v.choice("tipProfile");
    let tip_on = !tip_profile.is_empty() && tip_profile != "SAME";
    let tip = tip_on.then(|| Profile {
        profile: tip_profile,
        camber: if tip_profile == "NACA4" { v.num("tipCamber") } else { 0.0 },
        camber_pos: if tip_profile == "NACA4" { v.num("tipCamberPos") } else { 40.0 },
        mean_line,
        invert,
    });
    let sec = SectionDef { root, tip, blend_from: if tip_on { v.num("blendFrom") / 100.0 } else { 0.0 } };
    let cap_start = h - cap_len;
    let mut cap_end_t = v.num("capEnd") / 100.0;
    let mut cap_end_raised = false;
    if cap_on && min_tip_on {
        let t_cs = d.tip_t / 100.0 * c0 * pl.chord(pl.cap_eta);
        if t_cs > 0.0 && t_cs * cap_end_t < 0.5 {
            cap_end_t = (0.5 / t_cs).min(1.0);
            cap_end_raised = true;
        }
    }
    let keys = StationKeys {
        h,
        count: v.num("stations") as usize,
        tip_on,
        blend_from: sec.blend_from,
        winglet: winglet_on.then_some((w_eta, w_r, w_ang)),
        cap: cap_on.then_some(cap_len),
        has_drop: tip_drop.abs() > 1e-3 || dihedral.abs() > 1e-6,
    };
    let pivot = pivot_fraction(v.choice("pivot"));
    let align = v.num("planformAlign") / 100.0;
    let points = v.num("pointsPerSurface") as usize;
    let mut stations = Vec::new();
    for eta in station_etas(&pl, &keys) {
        let y = eta * h;
        let chord = c0 * pl.chord(eta);
        let tail_add = c0 * pl.tail_factor(eta);
        let mut thick = (d.root_t + (d.tip_t - d.root_t) * eta.powf(v.num("thicknessExp"))) / 100.0;
        thick *= (chord - tail_add) / chord;
        if cap_on && y > cap_start {
            let dd = ((y - cap_start) / cap_len).min(1.0);
            thick = thick * (cap_end_t + (1.0 - cap_end_t) * (1.0 - dd * dd).max(0.0).sqrt()) / if plan_round { pl.cap_plan(eta) } else { 1.0 };
        }
        let twist = (d.incidence - v.num("washout") * eta.powf(v.num("washoutExp"))).to_radians();
        let cap_f = pl.cap_plan(eta);
        let chord_base = (chord - tail_add) / cap_f;
        let align_shift = (pivot - align) * (chord_base - c0) + pivot * tail_add + (0.5 - pivot) * (chord_base - (chord - tail_add));
        let x_off = v.num("sweepTip") * eta.powf(v.num("sweepExp")) + align_shift;
        let f_drop = drop.shape(eta);
        let mut z_off = -tip_drop * f_drop + h * dihedral.tan() * (eta - f_drop);
        let mut y_pos = y;
        let mut phi = 0.0;
        if winglet_on && y > w_s0 {
            let (wy, wz, wp) = winglet_path(y, w_s0, w_z0, w_phi0, w_sign, w_ang, w_r);
            y_pos = wy;
            z_off = wz;
            phi = wp;
        }
        let bump = pl.bump(eta, h);
        let chord_b = chord + bump;
        thick = thick * chord / chord_b;
        let x_off_b = x_off - (1.0 - pivot) * bump;
        let te_local = te.min(0.03 * chord_b);
        let section = sec.at(thick, te_local / chord_b, points, sec.weight(eta));
        let (s, c) = twist.sin_cos();
        let placed: Vec<[f64; 2]> = section
            .iter()
            .map(|q| {
                let xs = q[0] - pivot;
                [(xs * c + q[1] * s) * chord_b, (-xs * s + q[1] * c) * chord_b]
            })
            .collect();
        let frame = Frame { origin: [base[0] + x_off_b, base[1] + y_pos, base[2] + z_off], x: [1.0, 0.0, 0.0], y: [0.0, -phi.sin(), phi.cos()] };
        stations.push(Station { eta, chord: chord_b, frame, points: placed });
    }

    // ---- notice (planformSummary)
    let mut msg = format!("Area {} cm² projected", (area_proj / 100.0).round());
    if tip_drop.abs() > 0.0 || dihedral.abs() > 0.0 {
        msg += &format!(" ({} developed)", (area_dev / 100.0).round());
    }
    msg += &format!(
        " · span {} mm · AR {} · root {} mm · {} {} mm · MAC {} mm",
        span.round(),
        (aspect * 100.0).round() / 100.0,
        (c0 * (1.0 + tail)).round(),
        if shape == Shape::Superellipse { "end" } else { "tip" },
        tip_chord.round(),
        mac.round()
    );
    if tip_raised {
        msg += &format!(
            " · {} raised to {} for a {} mm tip",
            if shape == Shape::Superellipse { "End ratio" } else { "Tip ratio" },
            fmt(ratio, 3),
            fmt(v.num("minTip"), 1)
        );
    }
    if cap_end_raised {
        msg += &format!(" · Cap end raised to {} % for a 0.5 mm cap end", fmt(cap_end_t * 100.0, 0));
    }
    if winglet_on {
        let (ty, tz, _) = winglet_path(h, w_s0, w_z0, w_phi0, w_sign, w_ang, w_r);
        // One winglet's projected area, mm².
        const NW: usize = 40;
        let proj: f64 = (0..NW)
            .map(|q| {
                let sm = w_s0 + (h - w_s0) * (q as f64 + 0.5) / NW as f64;
                let (_, _, phi) = winglet_path(sm, w_s0, w_z0, w_phi0, w_sign, w_ang, w_r);
                c0 * pl.chord(sm / h) * phi.cos() * (h - w_s0) / NW as f64
            })
            .sum();
        msg += &format!(
            " | winglets {} {} cm² each ({} projected), {} mm high at {}°, projected span {} mm{}",
            if winglet == "UP" { "up" } else { "down" },
            fmt(v.num("wingletArea"), 1),
            fmt(proj / 100.0, 1),
            (tz - w_z0).abs().round(),
            fmt(v.num("wingletAngle"), 0),
            (2.0 * ty).round(),
            if w_clamped { format!(" - bend radius reduced to {} mm so the arc fits the winglet", w_r.round()) } else { String::new() }
        );
    }
    if tip_on {
        msg += &format!(
            " · tip section {}{} from {}% span",
            tip_profile,
            if tip_profile == "NACA4" { format!(" {}% camber", fmt(v.num("tipCamber"), 1)) } else { String::new() },
            fmt(v.num("blendFrom"), 0)
        );
    }
    if !v.flag("mirror") {
        msg += " (full-span values, half built)";
    }
    msg += &format!(" · {} stations", stations.len());
    msg += linked_note;
    let mut issues = Vec::new();
    if tip_raised {
        let key = if shape == Shape::Superellipse { "endRatio" } else { "tipRatio" };
        let raised = (ratio * 1000.0).ceil() / 1000.0;
        issues.push(
            Issue::new(
                Severity::Info,
                "tip_ratio_raised",
                format!("{key} raised from {} to {} for a {} mm tip", fmt(ratio_in, 3), fmt(raised, 3), fmt(v.num("minTip"), 1)),
            )
            .on(key, ratio_in)
            .limit(raised)
            .suggest(key, raised),
        );
    }
    if cap_end_raised {
        let raised = (cap_end_t * 100.0).ceil();
        issues.push(
            Issue::new(
                Severity::Info,
                "cap_end_raised",
                format!("Cap end raised from {} % to {} % for a 0.5 mm cap end", fmt(v.num("capEnd"), 0), raised),
            )
            .on("capEnd", v.num("capEnd"))
            .limit(raised)
            .suggest("capEnd", raised),
        );
    }
    if w_clamped {
        let fits = w_r.floor();
        issues.push(
            Issue::new(Severity::Info, "winglet_radius_reduced", format!("winglet bend radius reduced to {} mm so the arc fits the winglet", fits))
                .on("wingletRadius", v.num("wingletRadius"))
                .limit(fits)
                .suggest("wingletRadius", fits),
        );
    }
    Ok(Surface {
        stations,
        mirror: v.flag("mirror"),
        span,
        root_chord: c0 * (1.0 + tail),
        tip_chord,
        area_proj,
        area_dev,
        mac,
        aspect,
        incidence: d.incidence,
        root_thickness: d.root_t,
        tip_thickness: d.tip_t,
        notice: msg,
        issues,
    })
}
