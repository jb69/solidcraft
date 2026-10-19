//! A design can hold a `LoftPoints` feature (Loft Through Points: `sections`, by section, by
//! edge, the points a fit spline passes through; `operation`, optional `targets`). Older designs
//! need no transformation; the last release refuses the newer format number instead of failing
//! on a feature type it does not know.
