//! Hydrofoil plugins for SolidCraft: the Foil wing and Foil stabiliser of Jason Bird's Foil design
//! tool (an Onshape FeatureScript, build 246), ported by its author's request as parametric
//! plugins on the plugin contract (docs/plugins.md, docs/foil-plugins.md).
//!
//! - [`plugins`]: the plugins (`foil-wing`, `foil-stabiliser`); [`register`] installs them in an
//!   app.
//! - [`schema`]: every dialog field (generated from the FeatureScript preconditions), values and
//!   the conditions the dialogs show fields under;
//! - [`section`]: NACA 4-digit sections on three mean lines, the tabulated profiles, the tip
//!   section blend;
//! - [`wing`]: the planform solve and the loft stations of a wing or stabiliser.

pub mod plugins;
pub mod schema;
mod schema_gen;
pub mod section;
mod tables_gen;
pub mod wing;

pub use plugins::register;
pub use schema::{Field, Kind, Tool, Val, Values};
