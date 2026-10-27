//! Cutting tools and their feeds and speeds.

use serde::{Deserialize, Serialize};

use crate::{CamError, Result, positive};

/// The cutting geometry of a tool.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolKind {
    /// Square end mill.
    FlatEnd,
    /// Ball-nose end mill (corner radius = half the diameter).
    BallEnd,
    /// Bull-nose (radiused corner) end mill.
    BullNose {
        /// Corner radius, less than half the diameter.
        corner_radius: f64,
    },
    /// Chamfer mill: a cone whose tip angle is the full included angle (90° for a 45° chamfer).
    Chamfer {
        /// Full included angle of the cone, in degrees.
        tip_angle_deg: f64,
    },
    /// Twist drill with a conical tip.
    Drill {
        /// Full included tip angle, in degrees (118 or 135 are common).
        tip_angle_deg: f64,
    },
    /// Spot drill or centre drill (a short drill, used for spotting only).
    SpotDrill { tip_angle_deg: f64 },
    /// Tapered ball end mill (for deep 5-axis finishing).
    TaperedBall {
        /// Radius of the ball tip; the shank widens above it at the taper angle.
        tip_radius: f64,
        /// Half angle of the taper, in degrees.
        taper_angle_deg: f64,
    },
}

/// Feeds and speeds for one tool. Feeds are mm/min; spindle speed is rev/min.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Feeds {
    pub spindle_rpm: f64,
    /// Spindle direction: true for clockwise (M3).
    pub clockwise: bool,
    pub cutting: f64,
    pub plunge: f64,
    pub ramp: f64,
    pub lead_in: f64,
    pub lead_out: f64,
    /// Feed for the retract moves (0: rapid).
    pub retract: f64,
    pub coolant: Coolant,
}

impl Default for Feeds {
    fn default() -> Self {
        Feeds {
            spindle_rpm: 10_000.0,
            clockwise: true,
            cutting: 1000.0,
            plunge: 300.0,
            ramp: 600.0,
            lead_in: 1000.0,
            lead_out: 1000.0,
            retract: 0.0,
            coolant: Coolant::Flood,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coolant {
    #[default]
    Flood,
    Mist,
    Air,
    ThroughTool,
    Off,
}

/// A tool in the library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub id: u64,
    pub name: String,
    /// Tool number in the machine's magazine (T word).
    pub number: u32,
    /// Length offset register (H word); 0 means the tool number.
    #[serde(default)]
    pub length_offset: u32,
    #[serde(flatten)]
    pub kind: ToolKind,
    pub diameter: f64,
    /// Length of the cutting flutes, measured from the tip.
    pub flute_length: f64,
    /// Diameter of the shank above the flutes (the plain part); defaults to the diameter.
    #[serde(default)]
    pub shank_diameter: f64,
    /// Length from the tip to the holder face (the stick-out).
    pub overall_length: f64,
    /// Holder diameter at the holder face (for collision checks); 0 means no holder modelled.
    #[serde(default)]
    pub holder_diameter: f64,
    #[serde(default)]
    pub flutes: u32,
    #[serde(default)]
    pub feeds: Feeds,
}

impl Tool {
    /// A tool with sensible defaults for its size: a 10 mm flat end mill is
    /// `Tool::new(1, "10 mm flat", ToolKind::FlatEnd, 10.0)`.
    pub fn new(id: u64, name: &str, kind: ToolKind, diameter: f64) -> Tool {
        Tool {
            id,
            name: name.to_string(),
            number: u32::try_from(id).unwrap_or(1).max(1),
            length_offset: 0,
            kind,
            diameter,
            flute_length: diameter * 3.0,
            shank_diameter: diameter,
            overall_length: diameter * 6.0,
            holder_diameter: 0.0,
            flutes: 2,
            feeds: Feeds::default(),
        }
    }

    pub fn radius(&self) -> f64 {
        self.diameter * 0.5
    }

    /// Corner radius of the profile: 0 for a flat tool, the radius for a ball.
    pub fn corner_radius(&self) -> f64 {
        match self.kind {
            ToolKind::FlatEnd | ToolKind::Chamfer { .. } | ToolKind::Drill { .. } | ToolKind::SpotDrill { .. } => 0.0,
            ToolKind::BallEnd => self.radius(),
            ToolKind::BullNose { corner_radius } => corner_radius.clamp(0.0, self.radius()),
            ToolKind::TaperedBall { tip_radius, .. } => tip_radius.clamp(0.0, self.radius()),
        }
    }

    /// Radius of the tip (the flat part of the bottom): the radius minus the corner radius.
    pub fn flat_radius(&self) -> f64 {
        (self.radius() - self.corner_radius()).max(0.0)
    }

    /// Height of the cutting profile above the tip at horizontal distance `rho` from the axis
    /// (`rho ≤ radius`). This is the "inverse tool offset": a surface point at `(rho, z)` from the
    /// axis is touched when the tip is at `z - profile(rho)`.
    pub fn profile(&self, rho: f64) -> f64 {
        let r = self.radius();
        let rho = rho.clamp(0.0, r);
        match self.kind {
            ToolKind::Chamfer { tip_angle_deg } | ToolKind::Drill { tip_angle_deg } | ToolKind::SpotDrill { tip_angle_deg } => {
                let half = (tip_angle_deg.clamp(1.0, 179.0) * 0.5).to_radians();
                rho / half.tan()
            }
            ToolKind::TaperedBall { tip_radius, taper_angle_deg } => {
                let rc = tip_radius.clamp(1e-9, r);
                let a = taper_angle_deg.clamp(0.0, 89.0).to_radians();
                // Ball up to the tangent point with the cone, then the cone.
                let tangent_rho = rc * a.cos();
                if rho <= tangent_rho {
                    rc - (rc * rc - rho * rho).max(0.0).sqrt()
                } else {
                    let z_t = rc - rc * a.sin();
                    z_t + (rho - tangent_rho) / a.tan().max(1e-9)
                }
            }
            _ => {
                let rc = self.corner_radius();
                let a = r - rc;
                if rho <= a || rc <= 0.0 {
                    0.0
                } else {
                    let d = rho - a;
                    rc - (rc * rc - d * d).max(0.0).sqrt()
                }
            }
        }
    }

    /// Whether the profile is a flat disc, a torus and a cylinder (end mills), which the exact
    /// drop cutter handles; cones are handled through their profile only.
    pub fn is_end_mill(&self) -> bool {
        matches!(self.kind, ToolKind::FlatEnd | ToolKind::BallEnd | ToolKind::BullNose { .. })
    }

    /// Checks the tool's numbers.
    pub fn validate(&self) -> Result<()> {
        positive("tool diameter", self.diameter)?;
        positive("flute length", self.flute_length)?;
        positive("overall length", self.overall_length)?;
        if self.overall_length < self.flute_length {
            return Err(CamError::Param("overall length must be at least the flute length".into()));
        }
        if self.shank_diameter < 0.0 || !self.shank_diameter.is_finite() || self.holder_diameter < 0.0 || !self.holder_diameter.is_finite() {
            return Err(CamError::Param("shank and holder diameters must be finite and not negative".into()));
        }
        match self.kind {
            ToolKind::BullNose { corner_radius } => {
                if !(corner_radius.is_finite() && corner_radius > 0.0 && corner_radius < self.radius()) {
                    return Err(CamError::Param("corner radius must be between 0 and the tool radius".into()));
                }
            }
            ToolKind::Chamfer { tip_angle_deg } | ToolKind::Drill { tip_angle_deg } | ToolKind::SpotDrill { tip_angle_deg } => {
                if !(tip_angle_deg.is_finite() && tip_angle_deg > 0.0 && tip_angle_deg < 180.0) {
                    return Err(CamError::Param("tip angle must be between 0 and 180 degrees".into()));
                }
            }
            ToolKind::TaperedBall { tip_radius, taper_angle_deg } => {
                if !(tip_radius.is_finite() && tip_radius > 0.0 && tip_radius <= self.radius()) {
                    return Err(CamError::Param("tip radius must be between 0 and the tool radius".into()));
                }
                if !(taper_angle_deg.is_finite() && (0.0..90.0).contains(&taper_angle_deg)) {
                    return Err(CamError::Param("taper angle must be between 0 and 90 degrees".into()));
                }
            }
            ToolKind::FlatEnd | ToolKind::BallEnd => {}
        }
        let f = &self.feeds;
        for (what, v) in [
            ("spindle speed", f.spindle_rpm),
            ("cutting feed", f.cutting),
            ("plunge feed", f.plunge),
            ("ramp feed", f.ramp),
            ("lead-in feed", f.lead_in),
            ("lead-out feed", f.lead_out),
        ] {
            positive(what, v)?;
        }
        if !(f.retract.is_finite() && f.retract >= 0.0) {
            return Err(CamError::Param("retract feed must be finite and not negative".into()));
        }
        Ok(())
    }

    /// The tip's effective cutting radius at height `h` above the tip (the shank above the
    /// flutes, the holder above the stick-out), for collision checks.
    pub fn radius_at(&self, h: f64) -> f64 {
        if h < 0.0 {
            return 0.0;
        }
        if h <= self.flute_length {
            // Invert the profile: the widest the tool is at this height.
            let r = self.radius();
            let rc = self.corner_radius();
            match self.kind {
                ToolKind::Chamfer { tip_angle_deg } | ToolKind::Drill { tip_angle_deg } | ToolKind::SpotDrill { tip_angle_deg } => {
                    let half = (tip_angle_deg.clamp(1.0, 179.0) * 0.5).to_radians();
                    (h * half.tan()).min(r)
                }
                ToolKind::TaperedBall { tip_radius, taper_angle_deg } => {
                    let rc = tip_radius.clamp(1e-9, r);
                    let a = taper_angle_deg.clamp(0.0, 89.0).to_radians();
                    if h <= rc { (rc * rc - (rc - h) * (rc - h)).max(0.0).sqrt() } else { (rc + (h - rc) * a.tan()).min(r) }
                }
                _ => {
                    if h >= rc || rc <= 0.0 {
                        r
                    } else {
                        (r - rc) + (rc * rc - (rc - h) * (rc - h)).max(0.0).sqrt()
                    }
                }
            }
        } else if h <= self.overall_length {
            if self.shank_diameter > 0.0 { self.shank_diameter * 0.5 } else { self.radius() }
        } else {
            self.holder_diameter * 0.5
        }
    }
}

/// A library: tools by id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolLibrary {
    pub tools: Vec<Tool>,
}

impl ToolLibrary {
    pub fn get(&self, id: u64) -> Option<&Tool> {
        self.tools.iter().find(|t| t.id == id)
    }
    pub fn get_mut(&mut self, id: u64) -> Option<&mut Tool> {
        self.tools.iter_mut().find(|t| t.id == id)
    }
    pub fn remove(&mut self, id: u64) -> Option<Tool> {
        let i = self.tools.iter().position(|t| t.id == id)?;
        Some(self.tools.remove(i))
    }
}
