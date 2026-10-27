//! CAM for SolidCraft: toolpaths for 2.5-axis, 3-axis, indexed (3+2) and continuous 4- and
//! 5-axis milling, computed on triangle meshes and planar regions, with stock simulation,
//! machine kinematics and G-code post processors.
//!
//! The crate knows nothing about documents or the modelling kernel: the engine hands it the
//! meshes of the bodies to machine (in the setup's work coordinate system), the stock, the
//! tools and the operations, and gets [`Toolpath`]s back. Units are millimetres, radians and
//! mm/min; Z is the tool axis of a 3-axis setup, which points out of the stock's top.
//!
//! Layout:
//! - [`tool`]: tools (end mills, ball, bull-nose, chamfer, drill), feeds and speeds.
//! - [`frame`]: orthonormal frames (work coordinate systems, tool orientations).
//! - [`toolpath`]: moves with a tool axis, feed and kind; statistics.
//! - [`polyline`], [`offset`]: 2D polygon utilities and offsetting, the heart of 2.5D work.
//! - [`heightmap`], [`dropcutter`]: cutter-location surfaces on meshes, for 3-axis and, in any
//!   frame, for rotary and 5-axis work.
//! - [`linking`]: heights, leads, ramps and helices, and how passes are joined.
//! - [`ops`]: the operations, each a parameter struct and a generator.
//! - [`machine`]: kinematics, inverse kinematics and linearisation for 4/5-axis machines.
//! - [`simulate`]: Z-map stock removal, gouge, rest material and collision checks.
//! - [`post`]: G-code output.
//! - [`setup`]: the manufacturing model a design stores: tools, setups, operations.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod dropcutter;
pub mod frame;
pub mod heightmap;
pub mod linking;
pub mod machine;
pub mod notice;
pub mod ops;
pub mod post;

pub mod offset;

pub mod polyline;
pub mod setup;
pub mod simulate;

pub mod tool;
pub mod toolpath;

pub use frame::Frame;
pub use notice::{Issue, Notice, Severity};
pub use setup::{Manufacture, Operation, Setup, Stock};

pub use tool::{Tool, ToolKind};
pub use toolpath::{Move, MoveKind, Toolpath, ToolpathStats};

/// Errors of toolpath generation, simulation and posting.
#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum CamError {
    /// A parameter is out of range or inconsistent.
    #[error("bad parameter: {0}")]
    Param(String),
    /// The geometry an operation needs is missing or unusable.
    #[error("geometry: {0}")]
    Geometry(String),
    /// The work would exceed a size cap (grid cells, moves, passes).
    #[error("too large: {0}")]
    TooLarge(String),
    /// A machine cannot reach a tool orientation or position.
    #[error("machine: {0}")]
    Machine(String),
    /// Not built yet.
    #[error("not supported yet: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, CamError>;

/// Most moves one toolpath may hold; generation stops with [`CamError::TooLarge`] beyond it.
pub const MAX_MOVES: usize = 4_000_000;
/// Most cells a height map or stock map may hold.
pub const MAX_CELLS: usize = 25_000_000;

/// `x` when it is finite and positive, else an error naming `what`.
pub fn positive(what: &str, x: f64) -> Result<f64> {
    if x.is_finite() && x > 0.0 { Ok(x) } else { Err(CamError::Param(format!("{what} must be a positive number, got {x}"))) }
}

/// `x` when it is finite, else an error naming `what`.
pub fn finite(what: &str, x: f64) -> Result<f64> {
    if x.is_finite() { Ok(x) } else { Err(CamError::Param(format!("{what} must be a finite number"))) }
}

#[cfg(test)]
mod tests;
