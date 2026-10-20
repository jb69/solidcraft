//! Straight bevel gears. The teeth taper to the cone apex (standard tapered depth: every line of
//! a flank, the tip and the root runs through the apex), so the gear is the cone over one planar
//! outline, cut off at the outer and inner ends of the face. The outline is the tooth profile of
//! Tredgold's virtual spur gear on the back cone (z / cos δ teeth of the outer module) wrapped onto
//! the back cone and projected from the apex onto the plane of the outer pitch circle; that
//! projection keeps every flank line through the apex, so the lofted flanks are the flanks the
//! back-cone profile defines. The outer end is then trimmed to the back cone, which puts the tip
//! edge on the standard tip circle d + 2·h_a·cos δ.
//!
//! A gear is built in its own frame: axis +Z (from the back towards the apex), the outer pitch
//! circle in the plane Z = 0, the apex at (0, 0, H).

use std::f64::consts::PI;

use crate::involute::{Gear, Kind, Piece, Spec};

/// A straight bevel gear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bevel {
    pub spec: Spec,
    /// The teeth of the gear it is made to mesh with.
    pub mate_teeth: u32,
    pub shaft_angle: f64,
    /// Pitch cone angle δ: tan δ = sin Σ / (z_mate / z + cos Σ).
    pub delta: f64,
    /// Outer pitch radius z·m/2.
    pub pitch_radius: f64,
    /// Outer cone distance R = r / sin δ.
    pub cone_distance: f64,
    /// The apex's height above the outer pitch circle, R·cos δ.
    pub apex_height: f64,
    /// Along the pitch cone.
    pub face_width: f64,
    /// Tredgold's virtual spur gear on the back cone (radius r / cos δ).
    pub virtual_gear: Gear,
    /// The virtual tooth's outer radius (its tip, cut back where it would come to a point).
    pub virtual_outer: f64,
}

impl Bevel {
    pub fn new(spec: Spec, mate_teeth: u32, shaft_angle: f64, face_width: f64) -> Bevel {
        let (z, zm) = (spec.teeth as f64, mate_teeth as f64);
        let delta = shaft_angle.sin().atan2(zm / z + shaft_angle.cos());
        let m = spec.module;
        let pitch_radius = z * m / 2.0;
        let cone_distance = pitch_radius / delta.sin();
        let rv = pitch_radius / delta.cos();
        let x = spec.profile_shift;
        let a = spec.pressure_angle;
        let virtual_gear = Gear {
            spec: Spec { kind: Kind::External, helix_angle: 0.0, ..spec },
            module_t: m,
            alpha_t: a,
            pitch_radius: rv,
            base_radius: rv * a.cos(),
            tip_radius: rv + m * (1.0 + x),
            root_radius: rv - m * (1.25 - x),
            drawn_thickness: m * (PI / 2.0 + 2.0 * x * a.tan()) - spec.backlash / 2.0,
            drawn_outer: rv + m * (1.0 + x),
            drawn_inner: rv - m * (1.25 - x),
        };
        Bevel {
            spec,
            mate_teeth,
            shaft_angle,
            delta,
            pitch_radius,
            cone_distance,
            apex_height: cone_distance * delta.cos(),
            face_width,
            virtual_gear,
            virtual_outer: virtual_gear.drawn_outer,
        }
    }

    /// Virtual number of teeth z / cos δ.
    pub fn virtual_teeth(&self) -> f64 {
        self.spec.teeth as f64 / self.delta.cos()
    }

    /// The angular pitch about the gear's axis.
    pub fn tau(&self) -> f64 {
        2.0 * PI / self.spec.teeth as f64
    }

    /// Outer tip radius: r + h_a·cos δ (on the back cone).
    pub fn tip_radius(&self) -> f64 {
        self.pitch_radius + (self.virtual_outer - self.virtual_gear.pitch_radius) * self.delta.cos()
    }

    /// Outer root radius: r − h_f·cos δ.
    pub fn root_radius(&self) -> f64 {
        self.pitch_radius - (self.virtual_gear.pitch_radius - self.virtual_gear.root_radius) * self.delta.cos()
    }

    /// The scale of the inner end about the apex, (R − b)/R.
    pub fn inner_scale(&self) -> f64 {
        (self.cone_distance - self.face_width) / self.cone_distance
    }

    /// The inner end's height above the outer pitch circle, b·cos δ.
    pub fn inner_height(&self) -> f64 {
        self.apex_height * (1.0 - self.inner_scale())
    }

    /// A point of the virtual gear (radius `rho` from the back cone's apex, `phi` from the tooth's
    /// centre line in the development) on a tooth centred at azimuth `c`: wrapped onto the back
    /// cone and projected from the apex onto the plane Z = 0, then scaled by `scale` about the axis
    /// (the section at that fraction of the cone distance, projected the same way).
    fn point(&self, rho: f64, phi: f64, c: f64, scale: f64) -> [f64; 2] {
        let (sd, cd) = self.delta.sin_cos();
        let rv = self.virtual_gear.pitch_radius;
        let r = rho * cd;
        let h = (rho - rv) * sd;
        let f = scale * self.apex_height / (self.apex_height - h);
        let psi = c + phi / cd;
        [r * f * psi.cos(), r * f * psi.sin()]
    }

    /// The radius at which the virtual gear's circle `rho` lies in the projected outline at
    /// `scale`.
    pub fn projected_radius(&self, rho: f64, scale: f64) -> f64 {
        self.point(rho, 0.0, 0.0, scale)[0]
    }

    /// The closed outline of the teeth at `scale` (1 at the outer end), tooth 0 centred at
    /// azimuth `rotation`, anticlockwise.
    pub fn outline(&self, rotation: f64, scale: f64, flank_points: usize) -> Vec<Piece> {
        let g = &self.virtual_gear;
        let rs = g.flank_start();
        let rf = g.drawn_inner;
        let ra = self.virtual_outer;
        let (ths, tha) = (g.half_angle(rs), g.half_angle(ra));
        let t_of = |r: f64| ((r / g.base_radius).powi(2) - 1.0).max(0.0).sqrt();
        let (t0, t1) = (t_of(rs), t_of(ra));
        let n = flank_points.max(3);
        let radii: Vec<f64> = (0..n)
            .map(|i| match i {
                0 => rs,
                i if i == n - 1 => ra,
                i => g.base_radius * (1.0 + (t0 + (t1 - t0) * i as f64 / (n - 1) as f64).powi(2)).sqrt(),
            })
            .collect();
        let origin = [0.0, 0.0];
        let mut out = Vec::new();
        for k in 0..self.spec.teeth {
            let c = rotation + k as f64 * self.tau();
            let p = |rho: f64, phi: f64| self.point(rho, phi, c, scale);
            if rf < rs {
                out.push(Piece::Line(p(rf, -ths), p(rs, -ths)));
            }
            out.push(Piece::Spline(radii.iter().map(|&r| p(r, -g.half_angle(r))).collect()));
            out.push(Piece::Arc { center: origin, start: p(ra, -tha), end: p(ra, tha) });
            out.push(Piece::Spline(radii.iter().rev().map(|&r| p(r, g.half_angle(r))).collect()));
            if rf < rs {
                out.push(Piece::Line(p(rs, ths), p(rf, ths)));
            }
            out.push(Piece::Arc { center: origin, start: p(rf, ths), end: self.point(rf, -ths, c + self.tau(), scale) });
        }
        out
    }
}

/// Where a gear sits: its apex, its axis (from the back towards the apex) and the direction of
/// azimuth 0 about it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub apex: [f64; 3],
    pub axis: [f64; 3],
    pub reference: [f64; 3],
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn unit(a: [f64; 3]) -> [f64; 3] {
    let n = dot(a, a).sqrt();
    if n > 0.0 { mul(a, 1.0 / n) } else { a }
}

/// The rotation that takes +Z to `axis` (about their common perpendicular, through the origin):
/// its axis and angle, as `solid.move` takes them.
pub fn rotation_to(axis: [f64; 3]) -> ([f64; 3], f64) {
    let z = [0.0, 0.0, 1.0];
    let c = dot(z, unit(axis)).clamp(-1.0, 1.0);
    let n = cross(z, axis);
    if dot(n, n).sqrt() < 1e-12 {
        return ([1.0, 0.0, 0.0], if c > 0.0 { 0.0 } else { PI });
    }
    (unit(n), c.acos())
}

/// `v` turned by `angle` about the unit `axis` (Rodrigues).
pub fn rotate(v: [f64; 3], axis: [f64; 3], angle: f64) -> [f64; 3] {
    let (s, c) = angle.sin_cos();
    add(add(mul(v, c), mul(cross(axis, v), s)), mul(axis, dot(axis, v) * (1.0 - c)))
}

/// Two bevel gears in mesh: gear 2's frame and rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BevelMesh {
    pub frame: Frame,
    /// Gear 2's tooth 0 azimuth in its frame.
    pub rotation: f64,
    pub ratio: f64,
    /// Transverse contact ratio of the virtual spur gears.
    pub contact_ratio: f64,
}

/// Where gear 2 goes to mesh with gear 1 (in `f1`, tooth 0 at azimuth `r1`), its axis turned
/// from gear 1's towards azimuth `angle` about gear 1's axis by the shaft angle, the apexes
/// together.
pub fn mesh(b1: &Bevel, f1: &Frame, r1: f64, b2: &Bevel, angle: f64) -> Result<BevelMesh, String> {
    let (s1, s2) = (b1.spec, b2.spec);
    if (s1.module - s2.module).abs() > 1e-9 {
        return Err(format!("module {} cannot mesh with module {}", s2.module, s1.module));
    }
    if (s1.pressure_angle - s2.pressure_angle).abs() > 1e-9 {
        return Err(format!("pressure angle {:.1}° cannot mesh with {:.1}°", s2.pressure_angle.to_degrees(), s1.pressure_angle.to_degrees()));
    }
    if b1.mate_teeth != s2.teeth {
        return Err(format!("the bevel gear before this one is made for a {}-tooth mate, this one has {} teeth", b1.mate_teeth, s2.teeth));
    }
    let u1 = unit(f1.axis);
    let v1 = unit(f1.reference);
    let a1 = mul(u1, -1.0);
    let h = add(mul(v1, angle.cos()), mul(cross(u1, v1), angle.sin()));
    let a2 = add(mul(a1, b1.shaft_angle.cos()), mul(h, b1.shaft_angle.sin()));
    let u2 = mul(a2, -1.0);
    let (n, turn) = rotation_to(u2);
    let v2 = rotate([1.0, 0.0, 0.0], n, turn);
    // The pitch element both cones share, and gear 2's azimuth of it.
    let e = add(mul(a1, b1.delta.cos()), mul(h, b1.delta.sin()));
    let e2 = add(e, mul(u2, -dot(e, u2)));
    let psi2 = dot(e2, cross(u2, v2)).atan2(dot(e2, v2));
    // Gear 1 turned by ε from a tooth on the pitch element; gear 2 then has a space there, turned
    // by ε·ω2/ω1, the ratio that matches the two surfaces' speeds on the pitch element.
    let eps = r1 - angle;
    let (w1, w2) = (cross(u1, e), cross(u2, e));
    let speed = dot(w1, w2) / dot(w2, w2);
    let rotation = psi2 + b2.tau() / 2.0 + eps * speed;
    let (g1, g2) = (&b1.virtual_gear, &b2.virtual_gear);
    let reach = |g: &Gear, tip: f64| (tip.powi(2) - g.base_radius.powi(2)).max(0.0).sqrt();
    let a = s1.pressure_angle;
    let contact_ratio =
        (reach(g1, b1.virtual_outer) + reach(g2, b2.virtual_outer) - (g1.pitch_radius + g2.pitch_radius) * a.sin()) / (PI * s1.module * a.cos());
    Ok(BevelMesh { frame: Frame { apex: f1.apex, axis: u2, reference: v2 }, rotation, ratio: s2.teeth as f64 / s1.teeth as f64, contact_ratio })
}
