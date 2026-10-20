//! Involute gear geometry: the sizes of one gear, the outline of its teeth, and how two gears
//! mesh. Standard proportions (ISO 53 basic rack: addendum 1·m, dedendum 1.25·m), transverse
//! quantities for helical gears (ISO 21771). The root is a circle joined to the flanks by radial
//! lines below the base circle, as most CAD gear generators draw it; the trochoid a hob would cut
//! there is not drawn, and an undercut is reported instead.

use std::f64::consts::PI;

/// Whether the teeth point outward (a pinion, a spur or helical gear) or inward (a ring gear).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    External,
    Internal,
}

/// Which way a helical gear's teeth wind; a right-hand helix turns anticlockwise as it rises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Right,
    Left,
}

/// What a gear is asked to be. Lengths in mm, angles in radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spec {
    pub kind: Kind,
    pub teeth: u32,
    /// Normal module.
    pub module: f64,
    /// Normal pressure angle.
    pub pressure_angle: f64,
    /// 0 for a spur gear.
    pub helix_angle: f64,
    pub hand: Hand,
    /// Profile shift coefficient (external gears; a ring gear has none).
    pub profile_shift: f64,
    /// Circumferential backlash at the pitch circle that this gear takes up (half of a pair's).
    pub backlash: f64,
}

/// The sizes that follow from a [`Spec`], in the transverse plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gear {
    pub spec: Spec,
    /// Transverse module m / cos β.
    pub module_t: f64,
    /// Transverse pressure angle.
    pub alpha_t: f64,
    pub pitch_radius: f64,
    pub base_radius: f64,
    /// Where the teeth end: the tip of an external gear, the tooth tips (inner circle) of a ring.
    pub tip_radius: f64,
    /// The root circle: inside an external gear's teeth, outside a ring's.
    pub root_radius: f64,
    /// Arc thickness at the pitch circle of what is drawn as a tooth: an external gear's tooth,
    /// a ring's tooth space.
    pub drawn_thickness: f64,
    /// The drawn shape's outer and inner radii (for a ring: its root and its tooth tips).
    pub drawn_outer: f64,
    pub drawn_inner: f64,
}

/// inv α = tan α − α.
pub fn inv(a: f64) -> f64 {
    a.tan() - a
}

/// The α whose involute function is `v` (Newton from a good start; v ≥ 0).
pub fn inv_inverse(v: f64) -> f64 {
    let mut a = (3.0 * v).cbrt().min(1.5);
    for _ in 0..50 {
        let f = inv(a) - v;
        let d = a.tan().powi(2);
        if d <= 0.0 {
            break;
        }
        let step = f / d;
        a -= step;
        if step.abs() < 1e-15 {
            break;
        }
    }
    a
}

impl Gear {
    pub fn new(spec: Spec) -> Gear {
        let m = spec.module;
        let cb = spec.helix_angle.cos();
        let module_t = m / cb;
        let alpha_t = (spec.pressure_angle.tan() / cb).atan();
        let pitch_radius = spec.teeth as f64 * module_t / 2.0;
        let base_radius = pitch_radius * alpha_t.cos();
        let x = if spec.kind == Kind::Internal { 0.0 } else { spec.profile_shift };
        // Arc thickness at the pitch circle (ISO 21771: s_t = m_t (π/2 + 2 x tan α_n)).
        let tooth = module_t * (PI / 2.0 + 2.0 * x * spec.pressure_angle.tan());
        match spec.kind {
            Kind::External => {
                let tip_radius = pitch_radius + m * (1.0 + x);
                let root_radius = pitch_radius - m * (1.25 - x);
                Gear {
                    spec,
                    module_t,
                    alpha_t,
                    pitch_radius,
                    base_radius,
                    tip_radius,
                    root_radius,
                    drawn_thickness: tooth - spec.backlash / 2.0,
                    drawn_outer: tip_radius,
                    drawn_inner: root_radius,
                }
            }
            Kind::Internal => {
                // A ring's tooth space is an external tooth of the same pitch circle: draw that
                // and cut it from the rim.
                let tip_radius = pitch_radius - m;
                let root_radius = pitch_radius + 1.25 * m;
                Gear {
                    spec,
                    module_t,
                    alpha_t,
                    pitch_radius,
                    base_radius,
                    tip_radius,
                    root_radius,
                    drawn_thickness: module_t * PI / 2.0 + spec.backlash / 2.0,
                    drawn_outer: root_radius,
                    drawn_inner: tip_radius,
                }
            }
        }
    }

    /// The angular pitch.
    pub fn tau(&self) -> f64 {
        2.0 * PI / self.spec.teeth as f64
    }

    /// Half the angle the drawn tooth takes up at radius `r` (≥ the base circle).
    pub fn half_angle(&self, r: f64) -> f64 {
        let a = (self.base_radius / r).min(1.0).acos();
        self.drawn_thickness / (2.0 * self.pitch_radius) + inv(self.alpha_t) - inv(a)
    }

    /// Where the drawn flank starts: the base circle, or the inner circle when that is outside it.
    pub fn flank_start(&self) -> f64 {
        self.drawn_inner.max(self.base_radius)
    }

    /// The drawn tooth's arc thickness at radius `r`.
    pub fn thickness_at(&self, r: f64) -> f64 {
        2.0 * r * self.half_angle(r)
    }

    /// The radius at which the drawn tooth is `t` thick (between the flank start and `limit`),
    /// for a tip that would otherwise come to a point.
    pub fn radius_for_thickness(&self, t: f64, limit: f64) -> f64 {
        let (mut lo, mut hi) = (self.flank_start(), limit);
        for _ in 0..100 {
            let mid = 0.5 * (lo + hi);
            if self.thickness_at(mid) > t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Twist over a face `width` (radians, anticlockwise positive): `width · tan β / r`.
    pub fn twist(&self, width: f64) -> f64 {
        let t = width * self.spec.helix_angle.tan() / self.pitch_radius;
        if self.spec.hand == Hand::Right { t } else { -t }
    }

    /// The fewest teeth an external gear has without undercut: 2 cos β (1 − x) / sin² α_t.
    pub fn min_teeth(&self) -> f64 {
        2.0 * self.spec.helix_angle.cos() * (1.0 - self.spec.profile_shift) / self.alpha_t.sin().powi(2)
    }
}

/// One piece of a tooth outline in the sketch plane.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Line([f64; 2], [f64; 2]),
    /// Anticlockwise about `center` from `start` to `end`.
    Arc {
        center: [f64; 2],
        start: [f64; 2],
        end: [f64; 2],
    },
    /// A fit-point spline through these points.
    Spline(Vec<[f64; 2]>),
}

/// The closed outline of the drawn teeth (an external gear, or the tooth spaces of a ring),
/// anticlockwise, about `center`, tooth 0 centred at `rotation`, the drawn tooth's tips at `outer`
/// (≤ [`Gear::drawn_outer`], less when a tip is cut back). `flank_points` per flank.
pub fn outline(g: &Gear, center: [f64; 2], rotation: f64, outer: f64, flank_points: usize) -> Vec<Piece> {
    let at = |r: f64, a: f64| [center[0] + r * a.cos(), center[1] + r * a.sin()];
    let rs = g.flank_start();
    let rf = g.drawn_inner;
    let (ths, tha) = (g.half_angle(rs), g.half_angle(outer));
    // Flank samples even in the involute's roll parameter t = tan α_r: closer near the base circle,
    // where the flank bends most.
    let t_of = |r: f64| ((r / g.base_radius).powi(2) - 1.0).max(0.0).sqrt();
    let (t0, t1) = (t_of(rs), t_of(outer));
    let n = flank_points.max(3);
    let radii: Vec<f64> = (0..n).map(|i| t0 + (t1 - t0) * i as f64 / (n - 1) as f64).map(|t| g.base_radius * (1.0 + t * t).sqrt()).collect();
    let radii: Vec<f64> = radii
        .iter()
        .enumerate()
        .map(|(i, r)| {
            if i == 0 {
                rs
            } else if i == n - 1 {
                outer
            } else {
                *r
            }
        })
        .collect();
    let mut out = Vec::new();
    for k in 0..g.spec.teeth {
        let c = rotation + k as f64 * g.tau();
        if rf < rs {
            out.push(Piece::Line(at(rf, c - ths), at(rs, c - ths)));
        }
        out.push(Piece::Spline(radii.iter().map(|&r| at(r, c - g.half_angle(r))).collect()));
        out.push(Piece::Arc { center, start: at(outer, c - tha), end: at(outer, c + tha) });
        out.push(Piece::Spline(radii.iter().rev().map(|&r| at(r, c + g.half_angle(r))).collect()));
        if rf < rs {
            out.push(Piece::Line(at(rs, c + ths), at(rf, c + ths)));
        }
        out.push(Piece::Arc { center, start: at(rf, c + ths), end: at(rf, c + g.tau() - ths) });
    }
    out
}

/// Two gears in mesh: gear 1 placed already, gear 2 placed by it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mesh {
    /// Operating centre distance.
    pub center_distance: f64,
    /// Operating transverse pressure angle.
    pub alpha_w: f64,
    /// Gear 2's centre.
    pub center: [f64; 2],
    /// Gear 2's rotation: the angle of its tooth 0's centre.
    pub rotation: f64,
    /// Transverse contact ratio.
    pub contact_ratio: f64,
    /// Speed ratio z2 / z1.
    pub ratio: f64,
}

/// Where gear 2 goes to mesh with gear 1 (centre `c1`, tooth 0 at `r1`, tips cut back to `tip1`)
/// in the direction `angle` from gear 1's centre. `Err` when the two cannot mesh.
pub fn mesh(g1: &Gear, c1: [f64; 2], r1: f64, tip1: f64, g2: &Gear, tip2: f64, angle: f64) -> Result<Mesh, String> {
    let (s1, s2) = (g1.spec, g2.spec);
    if (s1.module - s2.module).abs() > 1e-9 {
        return Err(format!("module {} cannot mesh with module {}", s2.module, s1.module));
    }
    if (s1.pressure_angle - s2.pressure_angle).abs() > 1e-9 {
        return Err(format!("pressure angle {:.1}° cannot mesh with {:.1}°", s2.pressure_angle.to_degrees(), s1.pressure_angle.to_degrees()));
    }
    if (s1.helix_angle - s2.helix_angle).abs() > 1e-9 {
        return Err(format!("helix angle {:.1}° cannot mesh with {:.1}°", s2.helix_angle.to_degrees(), s1.helix_angle.to_degrees()));
    }
    let (z1, z2) = (s1.teeth as f64, s2.teeth as f64);
    let tan_a = s1.pressure_angle.tan();
    let a_t = g1.alpha_t;
    let pb = PI * g1.module_t * a_t.cos();
    let reach = |g: &Gear, tip: f64| (tip.powi(2) - g.base_radius.powi(2)).max(0.0).sqrt();
    // ψ: the direction of the pitch point from each centre; `same`: whether gear 2 turns the
    // same way as gear 1 (an internal pair).
    let (alpha_w, a_w, psi1, psi2, same) = match (s1.kind, s2.kind) {
        (Kind::External, Kind::External) => {
            let aw = inv_inverse(inv(a_t) + 2.0 * tan_a * (s1.profile_shift + s2.profile_shift) / (z1 + z2));
            (aw, (z1 + z2) * g1.module_t * a_t.cos() / (2.0 * aw.cos()), angle, angle + PI, false)
        }
        (Kind::Internal, Kind::Internal) => return Err("two ring gears cannot mesh".into()),
        (ring, _) => {
            let (zr, zp, xp) = if ring == Kind::Internal { (z1, z2, s2.profile_shift) } else { (z2, z1, s1.profile_shift) };
            if zr <= zp {
                return Err(format!("a {zr}-tooth ring cannot hold a {zp}-tooth pinion"));
            }
            let aw = inv_inverse((inv(a_t) - 2.0 * tan_a * xp / (zr - zp)).max(0.0));
            let a = (zr - zp) * g1.module_t * a_t.cos() / (2.0 * aw.cos());
            // Ring first: the pitch point lies beyond the pinion; pinion first: opposite the ring's offset.
            if ring == Kind::Internal { (aw, a, angle, angle, true) } else { (aw, a, angle + PI, angle + PI, true) }
        }
    };
    let center = [c1[0] + a_w * angle.cos(), c1[1] + a_w * angle.sin()];
    // Gear 1 turned by ε from a tooth on the pitch point; gear 2 then has a space there, turned
    // by ε z1/z2 the other way (or the same way, inside a ring).
    let eps = r1 - psi1;
    let aligned = psi2 + g2.tau() / 2.0;
    let rotation = if same { aligned + eps * z1 / z2 } else { aligned - eps * z1 / z2 };
    let contact_ratio = match (s1.kind, s2.kind) {
        (Kind::External, Kind::External) => (reach(g1, tip1) + reach(g2, tip2) - a_w * alpha_w.sin()) / pb,
        (Kind::Internal, _) => (reach(g2, tip2) - reach(g1, tip1) + a_w * alpha_w.sin()) / pb,
        _ => (reach(g1, tip1) - reach(g2, tip2) + a_w * alpha_w.sin()) / pb,
    };
    Ok(Mesh { center_distance: a_w, alpha_w, center, rotation, contact_ratio, ratio: z2 / z1 })
}
