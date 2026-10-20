//! Hydrofoil and propeller plugins for SolidCraft: the Foil wing and Foil stabiliser of Jason
//! Bird's Foil design tool (build 246) and his B-Series Propeller generator (build 99), Onshape
//! FeatureScripts ported by their author's request as parametric plugins on the plugin contract
//! (docs/plugins.md, docs/foil-plugins.md).
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
