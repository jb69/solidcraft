//! A design can hold plugin features (`Plugin`: `plugin_id`, `api_version`, `params` with the
//! number parameters' `inputs` and every parameter's value, `display`, `provenance`, `fallback`,
//! `operation`, optional `targets`; docs/plugins.md) and, at the document's top level,
//! `plugin_results`: the last result of each such feature whose fallback is a recorded mesh, by
//! feature id. Older designs need no transformation; the last release refuses the newer format
//! number instead of failing on a feature type it does not know.
