//! Hydrofoil and propeller design in SolidCraft: the Onshape Foil design tool's Foil wing and
//! Foil stabiliser features and the B-Series Propeller feature, ported from their FeatureScript.
//!
//! - [`schema`]: every dialog field (generated from the FeatureScript preconditions), values and
//!   the conditions the dialogs show fields under;
//! - [`section`]: NACA 4-digit sections on three mean lines, the tabulated profiles, the tip
//!   section blend;
//! - [`wing`]: the planform solve and the loft stations of a wing or stabiliser;
//! - [`prop`]: the B-series blade sections and the hub.
//!
//! This crate has the laws only; `solidcraft-doc` turns their sections into solids.

pub mod prop;
pub mod schema;
mod schema_gen;
pub mod section;
mod tables_gen;
pub mod wing;

pub use schema::{Field, Kind, Tool, Val, Values};
