//! A design saves its manufacturing model under `manufacture`: the tool library, setups (WCS,
//! stock, which may come from an earlier setup, machine) and their operations, each with its
//! machining boundary. An older design has none and gets an empty one; the last release refuses
//! a design saved with it (the newer format number). Toolpaths are not saved: `cam.generate`
//! recomputes them.
