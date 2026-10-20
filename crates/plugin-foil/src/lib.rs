//! Hydrofoil and propeller plugins for SolidCraft: the Onshape Foil design tool's Foil wing and
//! Foil stabiliser and the B-Series Propeller, ported from their FeatureScript (Jason Bird's tool,
//! foil design build 246, propeller build 99) and checked against it. Fork-only: a port of
//! FeatureScript checked against Onshape cannot go upstream (CONTRIBUTING, rule 1).
//!
//! - [`plugins`]: the three plugins (`foil-wing`, `foil-stabiliser`, `foil-propeller`) on the
//!   plugin contract; [`register`] installs them in an app.
//! - [`schema`]: every dialog field (generated from the FeatureScript preconditions), values and
//!   the conditions the dialogs show fields under;
//! - [`section`]: NACA 4-digit sections on three mean lines, the tabulated profiles, the tip
//!   section blend;
//! - [`wing`]: the planform solve and the loft stations of a wing or stabiliser;
//! - [`prop`]: the B-series blade sections and the hub.

pub mod plugins;
pub mod prop;
pub mod schema;
mod schema_gen;
pub mod section;
mod tables_gen;
pub mod wing;

pub use plugins::register;
pub use schema::{Field, Kind, Tool, Val, Values};
