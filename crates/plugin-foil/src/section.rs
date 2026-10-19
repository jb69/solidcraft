//! Foil sections, as the FeatureScript builds them (`naca4SectionML`, `tableSection`,
//! `buildSection`, `buildSectionAt`): chord-normalised points from the upper trailing edge round
//! the leading edge to the lower trailing edge, `2n − 1` of them on a cosine spacing, with the
//! trailing edge opened to `te` (a fraction of the chord) by a linear ramp.

use std::f64::consts::PI;

use crate::tables_gen as t;

/// The camber line's shape (build 246).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MeanLine {
    /// Two parabolas meeting at the camber position.
    Naca4,
    /// NACA 6-series a-line: `a` and the peak camber of its cli = 1 line (to scale it to Camber %).
    A6 { a: f64, peak: f64 },
    /// NACA 4-digit with a reflex of `reflex` (fraction of chord) from `from` (fraction) aft.
    Reflex { reflex: f64, from: f64 },
}

impl MeanLine {
    /// `meanLineOf`: from the dialog's Mean line, Load to a (fraction), Reflex % and Reflex from %.
    pub fn of(kind: &str, a: f64, reflex_pct: f64, reflex_from_pct: f64) -> MeanLine {
        match kind {
            "A6" => {
                let a = a.clamp(0.01, 0.999);
                let peak = (1..400).map(|i| a6_raw(i as f64 / 400.0, a)[0]).fold(0.0, f64::max);
                MeanLine::A6 { a, peak }
            }
            "REFLEX" => MeanLine::Reflex { reflex: reflex_pct / 100.0, from: (reflex_from_pct / 100.0).clamp(0.05, 0.95) },
            _ => MeanLine::Naca4,
        }
    }
}

/// NACA 6-series a-line for cli = 1: `[yc, dyc/dx]` at x (Abbott and von Doenhoff, eq. 4.26).
pub fn a6_raw(x_in: f64, a: f64) -> [f64; 2] {
    let x = x_in.clamp(1e-9, 1.0 - 1e-9);
    let g = -1.0 / (1.0 - a) * (a * a * (0.5 * a.ln() - 0.25) + 0.25);
    let h = 1.0 / (1.0 - a) * (0.5 * (1.0 - a).powi(2) * (1.0 - a).ln() - 0.25 * (1.0 - a).powi(2)) + g;
    let k = 1.0 / (2.0 * PI * (a + 1.0));
    let am = a - x;
    let l = if am.abs() > 1e-12 { am * am * am.abs().ln() } else { 0.0 };
    let d = if am.abs() > 1e-12 { am * am.abs().ln() } else { 0.0 };
    let y = k
        * (1.0 / (1.0 - a) * (0.5 * l - 0.5 * (1.0 - x).powi(2) * (1.0 - x).ln() + 0.25 * (1.0 - x).powi(2) - 0.25 * am * am) - x * x.ln() + g
            - h * x);
    let dy = k * (((1.0 - x) * (1.0 - x).ln() - d) / (1.0 - a) - x.ln() - 1.0 - h);
    [y, dy]
}

/// `[yc, dyc/dx]` of the camber line for maximum camber `m`, position `p` (fractions) and mean line `ml`.
pub fn camber_line(m: f64, p: f64, x: f64, ml: MeanLine) -> [f64; 2] {
    if m == 0.0 {
        return [0.0, 0.0];
    }
    if let MeanLine::A6 { a, peak } = ml {
        let r = a6_raw(x, a);
        let f = m / peak;
        return [r[0] * f, r[1] * f];
    }
    let (mut yc, mut dyc) = if x < p {
        (m / (p * p) * (2.0 * p * x - x * x), 2.0 * m / (p * p) * (p - x))
    } else {
        let q = 1.0 - p;
        (m / (q * q) * ((1.0 - 2.0 * p) + 2.0 * p * x - x * x), 2.0 * m / (q * q) * (p - x))
    };
    if let MeanLine::Reflex { reflex, from } = ml
        && reflex != 0.0
        && x > from
    {
        let s = (x - from) / (1.0 - from);
        let k = 27.0 / 4.0 * reflex;
        yc -= k * s * s * (1.0 - s);
        dyc -= k * (2.0 * s - 3.0 * s * s) / (1.0 - from);
    }
    [yc, dyc]
}

fn cosine_x(i: usize, n: usize) -> f64 {
    0.5 * (1.0 - (PI * i as f64 / (n - 1) as f64).cos())
}

/// Upper reversed then lower without its leading-edge point: TE upper → LE → TE lower.
fn join(upper: &[[f64; 2]], lower: &[[f64; 2]]) -> Vec<[f64; 2]> {
    upper.iter().rev().chain(lower.iter().skip(1)).copied().collect()
}

/// NACA 4-digit thickness on mean line `ml` (`naca4SectionML`).
pub fn naca4(m: f64, p: f64, t: f64, te: f64, n: usize, ml: MeanLine) -> Vec<[f64; 2]> {
    let (mut upper, mut lower) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for i in 0..n {
        let x = cosine_x(i, n);
        let [yc, dyc] = camber_line(m, p, x, ml);
        let yt = 5.0 * t * (0.2969 * x.sqrt() - 0.1260 * x - 0.3516 * x * x + 0.2843 * x.powi(3) - 0.1036 * x.powi(4)) + 0.5 * te * x;
        let k = 1.0 / (1.0 + dyc * dyc).sqrt();
        let s = dyc * k;
        upper.push([x - yt * s, yc + yt * k]);
        lower.push([x + yt * s, yc - yt * k]);
    }
    join(&upper, &lower)
}

/// Linear interpolation in a sorted `(x, y)` table, clamped at the ends (`tableInterp`).
pub fn table_interp(table: &[[f64; 2]], x: f64) -> f64 {
    let (Some(first), Some(last)) = (table.first(), table.last()) else { return 0.0 };
    if x <= first[0] {
        return first[1];
    }
    if x >= last[0] {
        return last[1];
    }
    for w in table.windows(2) {
        let (a, b) = (w[0], w[1]);
        if x >= a[0] && x <= b[0] && b[0] > a[0] {
            return a[1] + (x - a[0]) / (b[0] - a[0]) * (b[1] - a[1]);
        }
    }
    last[1]
}

/// The table's greatest thickness on a fine cosine sweep (`tableMaxThickness`).
pub fn table_max_thickness(upper: &[[f64; 2]], lower: &[[f64; 2]]) -> f64 {
    (0..=200).map(|i| cosine_x(i, 201)).map(|x| table_interp(upper, x) - table_interp(lower, x)).fold(0.0, f64::max)
}

/// A digitised section scaled to thickness `t` (`tableSection`): about its camber line, or with
/// `keep_lower` about its (flat) lower surface.
pub fn table_section(upper: &[[f64; 2]], lower: &[[f64; 2]], t: f64, te: f64, n: usize, keep_lower: bool) -> Vec<[f64; 2]> {
    let scale = t / table_max_thickness(upper, lower);
    let (mut up, mut lo) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for i in 0..n {
        let x = cosine_x(i, n);
        let (yu, yl) = (table_interp(upper, x), table_interp(lower, x));
        if keep_lower {
            up.push([x, yu * scale + te * x]);
            lo.push([x, yl * scale]);
            continue;
        }
        let yc = 0.5 * (yu + yl);
        let yt = 0.5 * (yu - yl) * scale + 0.5 * te * x;
        up.push([x, yc + yt]);
        lo.push([x, yc - yt]);
    }
    join(&up, &lo)
}

/// A table profile's upper and lower surfaces, and whether it scales about its lower surface.
pub fn table_of(profile: &str) -> Option<(&'static [[f64; 2]], &'static [[f64; 2]], bool)> {
    Some(match profile {
        "E817" => (t::E817_UPPER, t::E817_LOWER, false),
        "E818" => (t::E818_UPPER, t::E818_LOWER, false),
        "NACA63412" => (t::NACA63412_UPPER, t::NACA63412_LOWER, false),
        "CLARKY" => (t::CLARKY_UPPER, t::CLARKY_LOWER, true),
        "MRC16" => (t::MRC16_UPPER, t::MRC16_LOWER, false),
        "MH18" => (t::MH18_UPPER, t::MH18_LOWER, false),
        "H105" => (t::H105_UPPER, t::H105_LOWER, false),
        _ => return None,
    })
}

/// One section's family: a profile id (`NACA4` or a table), its camber and position in percent
/// (NACA 4-digit only), its mean line and whether it is mounted upside down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Profile<'a> {
    pub profile: &'a str,
    pub camber: f64,
    pub camber_pos: f64,
    pub mean_line: MeanLine,
    pub invert: bool,
}

impl Profile<'_> {
    /// `buildSection`.
    pub fn build(&self, t: f64, te: f64, n: usize) -> Vec<[f64; 2]> {
        let pts = match table_of(self.profile) {
            Some((u, l, keep)) => table_section(u, l, t, te, n, keep),
            None => naca4(self.camber / 100.0, self.camber_pos / 100.0, t, te, n, self.mean_line),
        };
        if self.invert { pts.into_iter().map(|[x, y]| [x, -y]).collect() } else { pts }
    }
}

/// The root section and, from `blend_from` (fraction of the half span), a tip section blended in
/// (`sectionDefFromDefinition`, `sectionWeightAt`, `buildSectionAt`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SectionDef<'a> {
    pub root: Profile<'a>,
    pub tip: Option<Profile<'a>>,
    pub blend_from: f64,
}

impl SectionDef<'_> {
    /// The tip section's share at `eta`: 0 out to the blend start, then a smoothstep to 1.
    pub fn weight(&self, eta: f64) -> f64 {
        if self.tip.is_none() || eta <= self.blend_from {
            return 0.0;
        }
        let u = ((eta - self.blend_from) / (1.0 - self.blend_from).max(1e-6)).min(1.0);
        u * u * (3.0 - 2.0 * u)
    }
    /// The section at a station: the root's, the tip's or a point-by-point blend.
    pub fn at(&self, t: f64, te: f64, n: usize, w: f64) -> Vec<[f64; 2]> {
        let tip = match self.tip {
            Some(tip) if w > 0.0 => tip,
            _ => return self.root.build(t, te, n),
        };
        let tp = tip.build(t, te, n);
        if w >= 1.0 {
            return tp;
        }
        let rp = self.root.build(t, te, n);
        rp.iter().zip(&tp).map(|(a, b)| [a[0] * (1.0 - w) + b[0] * w, a[1] * (1.0 - w) + b[1] * w]).collect()
    }
}

/// Section area (shoelace) of a closed point list, chord units squared.
pub fn area(pts: &[[f64; 2]]) -> f64 {
    let n = pts.len();
    (0..n).map(|i| pts[i][0] * pts[(i + 1) % n][1] - pts[(i + 1) % n][0] * pts[i][1]).sum::<f64>().abs() / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naca_2412_thickness_and_camber() {
        let pts = naca4(0.02, 0.4, 0.12, 0.0, 81, MeanLine::Naca4);
        assert_eq!(pts.len(), 161);
        // the leading edge is the middle point, the trailing edge both ends
        assert!(pts[80][0].abs() < 1e-12 && pts[80][1].abs() < 1e-12);
        assert!((pts[0][0] - 1.0).abs() < 1e-3);
        let t = table_max_thickness(&pts[..81].iter().rev().copied().collect::<Vec<_>>(), &pts[80..]);
        assert!((t - 0.12).abs() < 2e-3, "{t}");
    }

    #[test]
    fn a6_line_scales_to_the_camber() {
        let ml = MeanLine::of("A6", 0.8, 0.0, 70.0);
        let peak = (1..200).map(|i| camber_line(0.03, 0.4, i as f64 / 200.0, ml)[0]).fold(0.0, f64::max);
        assert!((peak - 0.03).abs() < 2e-4, "{peak}");
    }

    #[test]
    fn tables_scale_to_their_thickness() {
        for p in ["E817", "E818", "NACA63412", "CLARKY", "MRC16", "MH18", "H105"] {
            let (u, l, keep) = table_of(p).expect("table");
            let pts = table_section(u, l, 0.1, 0.0, 60, keep);
            let up: Vec<[f64; 2]> = pts[..60].iter().rev().copied().collect();
            let lo: Vec<[f64; 2]> = pts[59..].to_vec();
            let t = table_max_thickness(&up, &lo);
            assert!((t - 0.1).abs() < 2e-3, "{p}: {t}");
        }
    }
}
