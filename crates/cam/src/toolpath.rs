//! Toolpaths: the tool tip positions and tool axis directions an operation produces, with the
//! kind and feed of every move. Positions are in the setup's work coordinate system; a 3-axis
//! move has axis `(0, 0, 1)`.

use serde::{Deserialize, Serialize};
use solidcraft_geom::Vec3;

use crate::{CamError, MAX_MOVES, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MoveKind {
    /// Positioning move at the machine's rapid rate, above the clearance height or along a
    /// checked link.
    Rapid,
    /// Cutting at the cutting feed.
    Cut,
    /// Straight descent into the material.
    Plunge,
    /// Ramp or helix descent.
    Ramp,
    /// Lead-in or lead-out arc or line.
    Lead,
    /// Positioning at a feed: between passes, below the clearance height but off the material.
    Link,
    /// Retract at a feed (or at the rapid rate when the feed is 0).
    Retract,
}

/// One straight move of the tool tip to `to`, with the tool pointing along `axis`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Move {
    pub kind: MoveKind,
    pub to: Vec3,
    pub axis: Vec3,
    /// mm/min; 0 for a rapid.
    pub feed: f64,
}

impl Move {
    pub fn new(kind: MoveKind, to: Vec3, feed: f64) -> Move {
        Move { kind, to, axis: Vec3::new(0.0, 0.0, 1.0), feed }
    }
    pub fn with_axis(mut self, axis: Vec3) -> Move {
        self.axis = axis;
        self
    }
    pub fn is_cutting(&self) -> bool {
        matches!(self.kind, MoveKind::Cut | MoveKind::Plunge | MoveKind::Ramp | MoveKind::Lead)
    }
}

/// A toolpath: a sequence of moves. The first move positions the tool from wherever it was.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Toolpath {
    pub moves: Vec<Move>,
    /// Drilling cycles the post may emit as canned cycles (also expanded into `moves`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cycles: Vec<DrillCycle>,
}

/// A drilling cycle at one hole, for posts that emit canned cycles.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrillCycle {
    pub kind: DrillCycleKind,
    /// Position of the hole top (tool tip at the start of the cut).
    pub top: Vec3,
    pub axis: Vec3,
    pub depth: f64,
    pub retract_height: f64,
    pub peck: f64,
    pub dwell_s: f64,
    pub feed: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrillCycleKind {
    /// G81: feed in, rapid out.
    #[default]
    Simple,
    /// G82: feed in, dwell, rapid out.
    Dwell,
    /// G83: peck with full retract.
    Peck,
    /// G73: chip-break peck.
    ChipBreak,
    /// G85: feed in, feed out (boring/reaming).
    FeedOut,
}

/// Measures of a toolpath.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolpathStats {
    pub moves: usize,
    pub cutting_length: f64,
    pub rapid_length: f64,
    pub link_length: f64,
    /// Estimated machining time in seconds (rapids at `rapid_feed`).
    pub time_s: f64,
    pub min_z: f64,
    pub max_z: f64,
    /// Largest angle between the tool axis and Z, in degrees (0 for 3-axis work).
    pub max_tilt_deg: f64,
}

impl Toolpath {
    pub fn new() -> Toolpath {
        Toolpath::default()
    }

    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    pub fn len(&self) -> usize {
        self.moves.len()
    }

    pub fn last(&self) -> Option<&Move> {
        self.moves.last()
    }

    pub fn last_position(&self) -> Option<Vec3> {
        self.moves.last().map(|m| m.to)
    }

    /// Appends a move, refusing non-finite positions and paths past [`MAX_MOVES`]. A move that
    /// repeats the previous position and axis is dropped.
    pub fn push(&mut self, m: Move) -> Result<()> {
        if !(m.to.is_finite() && m.axis.is_finite() && m.feed.is_finite()) {
            return Err(CamError::Geometry("toolpath move is not finite".into()));
        }
        if let Some(last) = self.moves.last()
            && last.to.dist(m.to) < 1e-9
            && (last.axis - m.axis).len() < 1e-9
        {
            // Keep the more conservative kind so a zero-length rapid does not hide a cut.
            return Ok(());
        }
        if self.moves.len() >= MAX_MOVES {
            return Err(CamError::TooLarge(format!("toolpath exceeds {MAX_MOVES} moves")));
        }
        self.moves.push(m);
        Ok(())
    }

    pub fn rapid(&mut self, to: Vec3) -> Result<()> {
        self.push(Move::new(MoveKind::Rapid, to, 0.0))
    }

    pub fn cut(&mut self, to: Vec3, feed: f64) -> Result<()> {
        self.push(Move::new(MoveKind::Cut, to, feed))
    }

    pub fn extend(&mut self, other: &Toolpath) -> Result<()> {
        for m in &other.moves {
            self.push(*m)?;
        }
        self.cycles.extend(other.cycles.iter().copied());
        Ok(())
    }

    /// Statistics with rapids at `rapid_feed` mm/min and `start` as the position before the
    /// first move (the machine's home or the previous operation's end).
    pub fn stats(&self, rapid_feed: f64, start: Option<Vec3>) -> ToolpathStats {
        let mut st = ToolpathStats { min_z: f64::INFINITY, max_z: f64::NEG_INFINITY, moves: self.moves.len(), ..Default::default() };
        let mut prev = start;
        let rapid = if rapid_feed.is_finite() && rapid_feed > 0.0 { rapid_feed } else { 5000.0 };
        for m in &self.moves {
            let d = prev.map_or(0.0, |p| p.dist(m.to));
            let feed = if m.feed > 0.0 { m.feed } else { rapid };
            match m.kind {
                MoveKind::Rapid => st.rapid_length += d,
                MoveKind::Link | MoveKind::Retract => st.link_length += d,
                _ => st.cutting_length += d,
            }
            st.time_s += d / feed * 60.0;
            st.min_z = st.min_z.min(m.to.z);
            st.max_z = st.max_z.max(m.to.z);
            let tilt = crate::frame::angle_between(m.axis, Vec3::new(0.0, 0.0, 1.0)).to_degrees();
            if tilt.is_finite() {
                st.max_tilt_deg = st.max_tilt_deg.max(tilt);
            }
            prev = Some(m.to);
        }
        if self.moves.is_empty() {
            st.min_z = 0.0;
            st.max_z = 0.0;
        }
        st
    }

    /// The toolpath with every point and axis mapped through `f` (positions) and `g` (axes).
    pub fn transformed(&self, f: impl Fn(Vec3) -> Vec3, g: impl Fn(Vec3) -> Vec3) -> Toolpath {
        Toolpath {
            moves: self.moves.iter().map(|m| Move { to: f(m.to), axis: g(m.axis), ..*m }).collect(),
            cycles: self.cycles.iter().map(|c| DrillCycle { top: f(c.top), axis: g(c.axis), ..*c }).collect(),
        }
    }

    /// Bounding box of the tip positions.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut it = self.moves.iter();
        let first = it.next()?.to;
        Some(it.fold((first, first), |(lo, hi), m| (lo.min(m.to), hi.max(m.to))))
    }
}
