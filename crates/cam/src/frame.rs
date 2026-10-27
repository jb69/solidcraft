//! Orthonormal frames: a work coordinate system, a tool orientation for indexed (3+2) work,
//! the radial frame of a rotary pass. A frame maps local coordinates (x along `x`, y along `y`,
//! z along the tool axis `z`) to world coordinates and back.

use serde::{Deserialize, Serialize};
use solidcraft_geom::Vec3;

use crate::{CamError, Result};

/// A right-handed orthonormal frame. `z` is the tool axis (pointing from the tip up the tool).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub origin: Vec3,
    pub x: Vec3,
    pub y: Vec3,
    pub z: Vec3,
}

impl Default for Frame {
    fn default() -> Self {
        Frame::IDENTITY
    }
}

impl Frame {
    pub const IDENTITY: Frame = Frame {
        origin: Vec3 { x: 0.0, y: 0.0, z: 0.0 },
        x: Vec3 { x: 1.0, y: 0.0, z: 0.0 },
        y: Vec3 { x: 0.0, y: 1.0, z: 0.0 },
        z: Vec3 { x: 0.0, y: 0.0, z: 1.0 },
    };

    /// A frame from its origin, tool axis and a hint for the x direction (orthogonalised
    /// against `z`; any direction not parallel to `z`). Fails on zero or parallel vectors.
    pub fn new(origin: Vec3, z: Vec3, x_hint: Vec3) -> Result<Frame> {
        if !(origin.is_finite() && z.is_finite() && x_hint.is_finite()) {
            return Err(CamError::Param("frame vectors must be finite".into()));
        }
        let z = z.normalized().ok_or_else(|| CamError::Param("frame axis must not be zero".into()))?;
        let x =
            (x_hint - z * x_hint.dot(z)).normalized().ok_or_else(|| CamError::Param("frame x direction must not be parallel to its axis".into()))?;
        let y = z.cross(x);
        Ok(Frame { origin, x, y, z })
    }

    /// A frame with the given tool axis and any x direction.
    pub fn from_axis(origin: Vec3, z: Vec3) -> Result<Frame> {
        let zn = z.normalized().ok_or_else(|| CamError::Param("frame axis must not be zero".into()))?;
        Frame::new(origin, zn, zn.any_perp())
    }

    pub fn to_world(&self, p: Vec3) -> Vec3 {
        self.origin + self.x * p.x + self.y * p.y + self.z * p.z
    }

    pub fn dir_to_world(&self, d: Vec3) -> Vec3 {
        self.x * d.x + self.y * d.y + self.z * d.z
    }

    pub fn to_local(&self, p: Vec3) -> Vec3 {
        let d = p - self.origin;
        Vec3::new(d.dot(self.x), d.dot(self.y), d.dot(self.z))
    }

    pub fn dir_to_local(&self, d: Vec3) -> Vec3 {
        Vec3::new(d.dot(self.x), d.dot(self.y), d.dot(self.z))
    }

    /// The frame moved along its own axes.
    pub fn translated_local(&self, d: Vec3) -> Frame {
        Frame { origin: self.to_world(d), ..*self }
    }

    /// This frame expressed in `outer`'s local coordinates (`outer.to_world(result.to_world(p)) == self.to_world(p)`
    /// when `self` is a world frame).
    pub fn in_frame(&self, outer: &Frame) -> Frame {
        Frame { origin: outer.to_local(self.origin), x: outer.dir_to_local(self.x), y: outer.dir_to_local(self.y), z: outer.dir_to_local(self.z) }
    }

    /// The frame rotated by `angle` about the axis through its origin along `axis` (world).
    pub fn rotated(&self, axis: Vec3, angle: f64) -> Result<Frame> {
        let a = axis.normalized().ok_or_else(|| CamError::Param("rotation axis must not be zero".into()))?;
        let rot = |v: Vec3| rotate(v, a, angle);
        Ok(Frame { origin: self.origin, x: rot(self.x), y: rot(self.y), z: rot(self.z) })
    }

    /// Whether the axes are unit and orthogonal, within `tol`.
    pub fn is_orthonormal(&self, tol: f64) -> bool {
        (self.x.len() - 1.0).abs() < tol
            && (self.y.len() - 1.0).abs() < tol
            && (self.z.len() - 1.0).abs() < tol
            && self.x.dot(self.y).abs() < tol
            && self.y.dot(self.z).abs() < tol
            && self.z.dot(self.x).abs() < tol
            && (self.x.cross(self.y) - self.z).len() < tol
    }
}

/// `v` rotated by `angle` about the unit vector `axis` (Rodrigues).
pub fn rotate(v: Vec3, axis: Vec3, angle: f64) -> Vec3 {
    let (s, c) = angle.sin_cos();
    v * c + axis.cross(v) * s + axis * (axis.dot(v) * (1.0 - c))
}

/// The angle between two directions, in radians (0 for parallel, π for opposite).
pub fn angle_between(a: Vec3, b: Vec3) -> f64 {
    let d = a.dot(b) / (a.len() * b.len());
    if d.is_finite() { d.clamp(-1.0, 1.0).acos() } else { 0.0 }
}
