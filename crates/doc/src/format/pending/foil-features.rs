//! A design can hold the foil features (`FoilWing`, `FoilStabiliser`, `Propeller`: the Onshape
//! Foil wing, Foil stabiliser and B-Series Propeller, `docs/foil-features.md`), each with
//! `values`, every field of its dialog by the FeatureScript's name (numbers as expressions,
//! flags, option ids), an `operation` and optional `targets`. A value missing from a saved
//! feature takes the tool's default; a field the build does not know is dropped. Older designs
//! need no transformation; the last release refuses the newer format number instead of failing
//! on a feature type it does not know.
