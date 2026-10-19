//! Plugin features (docs/plugins.md): list the installed plugins, make a plugin's feature, change
//! its parameters, and read its dialog (schema, values, what the dialog shows, the notice).

use serde_json::{Value, json};
use solidcraft_doc::FeatureKind;
use solidcraft_doc::plugin_feature::{PluginParams, PluginProvenance};
use solidcraft_plugin::{FieldKind, Plugin, Unit};

use super::{CommandSpec, not_in_sketch};
use crate::params::{bad, str_, string_list};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("plugin.list", "Installed Plugins", list)
        .icon("list")
        .noundo()
        .params("→ plugins: the plugins compiled into this app, each {id, name, version, description, display, fallback, preview}"),
    CommandSpec::new("plugin.add", "Plugin Feature", add).at("SOLID", "CREATE").icon("include").enabled(not_in_sketch).params(
        "plugin: id (plugin.list); values?: {parameter: value} (numbers or expressions, true/false, option ids; the rest take the \
         plugin's defaults; plugin.fields lists them); name?, body_name?, operation?: new|join|cut|intersect, targets?: [body] \
         → a feature the plugin rebuilds from its values on every change, its notice, values and coded issues ({severity, code, message, parameter?, value?, limit?, suggestion?})",
    ),
    CommandSpec::new("plugin.set", "Set Plugin Values", set)
        .icon("edit")
        .params("feature: id|name of a plugin feature, values: {parameter: value} → the feature rebuilt, its notice, values and issues"),
    CommandSpec::new("plugin.fields", "Plugin Parameters", fields).icon("inspect").noundo().params(
        "plugin?: id, or feature?: id|name → the plugin's parameters in dialog order (key, label, group, kind, unit, min, default, \
         max, options, when: the condition the dialog shows it under, help, unused), with a feature also each value, whether the \
         dialog shows it, and the notice",
    ),
];

/// Refuse parameters a command does not take (a misspelt `values` would otherwise make the
/// defaults without a word).
fn known_keys(p: &Value, cmd: &str, keys: &[&str]) -> Result<()> {
    if let Some(m) = p.as_object()
        && let Some(k) = m.keys().find(|k| !keys.contains(&k.as_str()))
    {
        return Err(bad(cmd, format!("unknown parameter `{k}` (takes {})", keys.join(", "))));
    }
    Ok(())
}

fn plugin_of(id: &str, cmd: &str) -> Result<std::sync::Arc<dyn Plugin>> {
    solidcraft_plugin::find(id).ok_or_else(|| {
        let ids: Vec<String> = solidcraft_plugin::installed().iter().map(|p| p.manifest().id).collect();
        bad(cmd, format!("no plugin `{id}` is installed (installed: {})", if ids.is_empty() { "none".to_string() } else { ids.join(", ") }))
    })
}

fn set_values(params: &mut PluginParams, plugin: &dyn Plugin, values: Option<&Value>, cmd: &str) -> Result<()> {
    let schema = plugin.schema();
    match values {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Object(m)) => {
            for (k, v) in m {
                params.set(&schema, k, v).map_err(|e| bad(cmd, e))?;
            }
            Ok(())
        }
        Some(_) => Err(bad(cmd, "`values` must be an object of parameters")),
    }
}

/// What the feature's last evaluation said: its `values` and `issues` (empty when its plugin did
/// not build it).
fn output(s: &Session, id: u64) -> (Value, Value) {
    let out = s.model.result(id).and_then(|r| r.state.plugin_output(id)).unwrap_or(Value::Null);
    let values = out.get("values").filter(|v| v.is_object()).cloned().unwrap_or_else(|| json!({}));
    (values, json!(solidcraft_doc::plugin_feature::issues(&out)))
}

/// The feature's notice and warnings, from the plugin's `describe` (or its last evaluation).
pub(crate) fn notice(s: &Session, id: u64) -> Option<String> {
    let f = s.doc.feature(id)?;
    let FeatureKind::Plugin { plugin_id, params, .. } = &f.kind else { return None };
    let (vals, _) = s.doc.param_values();
    let before = s.model.state_before(id);
    let ctx = solidcraft_doc::plugin_feature::context(f, &before.plugin_vars);
    let d = solidcraft_doc::plugin_feature::describe(&vals, plugin_id, params, &ctx).ok()?;
    let mut n = d.get("notice").and_then(Value::as_str).unwrap_or_default().to_string();
    for w in d.get("warnings").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        n += &format!(" | {w}");
    }
    for i in solidcraft_doc::plugin_feature::issues(&d) {
        if let Some(m) = i.get("message").and_then(Value::as_str) {
            n += &format!(" | {m}");
        }
    }
    (!n.is_empty()).then_some(n)
}

fn list(_s: &mut Session, _p: &Value) -> Result<Value> {
    let plugins: Vec<Value> = solidcraft_plugin::installed()
        .iter()
        .map(|p| {
            let m = p.manifest();
            json!({"id": m.id, "name": m.name, "version": m.version, "description": m.description, "display": m.display,
                "fallback": m.fallback, "preview": m.preview})
        })
        .collect();
    Ok(json!({"plugins": plugins}))
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "plugin.add";
    known_keys(p, cmd, &["plugin", "values", "name", "body_name", "body_names", "operation", "targets", "participants"])?;
    let id = str_(p, "plugin").ok_or_else(|| bad(cmd, "`plugin` must name an installed plugin (plugin.list)"))?;
    let plugin = plugin_of(id, cmd)?;
    let m = plugin.manifest();
    let mut params = PluginParams::defaults(&plugin.schema());
    set_values(&mut params, plugin.as_ref(), p.get("values"), cmd)?;
    let operation = match str_(p, "operation") {
        None => solidcraft_doc::Operation::NewBody,
        Some(o) => solidcraft_doc::Operation::parse(o).ok_or_else(|| bad(cmd, "`operation` must be new, join, cut or intersect"))?,
    };
    let targets = string_list(p, "targets").into_iter().map(Into::into).collect();
    let kind = FeatureKind::Plugin {
        plugin_id: m.id.clone(),
        api_version: m.api_version,
        params,
        display: m.display.clone(),
        provenance: PluginProvenance { version: m.version.clone() },
        fallback: m.fallback,
        operation,
        targets,
    };
    let mut p = p.clone();
    if let Some(o) = p.as_object_mut() {
        for k in ["plugin", "values", "operation", "targets"] {
            o.remove(k);
        }
        // Named after the plugin's label, numbered as built-in features are.
        if !o.contains_key("name") {
            let base: String = m.display.label.chars().filter(|c| !c.is_whitespace()).collect();
            let base = if base.is_empty() { "Plugin".to_string() } else { base };
            let name = (1..).map(|n| format!("{base}{n}")).find(|n| s.doc.find_feature(n).is_none()).unwrap_or(base);
            o.insert("name".into(), json!(name));
        }
    }
    let mut r = super::features::add_feature(s, &p, kind)?;
    if let Some(id) = r.get("feature").and_then(Value::as_u64)
        && let Some(o) = r.as_object_mut()
    {
        o.insert("notice".into(), json!(notice(s, id)));
        let (values, issues) = output(s, id);
        o.insert("values".into(), values);
        o.insert("issues".into(), issues);
    }
    Ok(r)
}

fn plugin_feature(s: &Session, p: &Value, cmd: &str) -> Result<(u64, String, PluginParams)> {
    let key = match p.get("feature") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(x)) => x.clone(),
        _ => return Err(bad(cmd, "`feature` must be an id or name")),
    };
    let f = s.doc.find_feature(&key).ok_or_else(|| bad(cmd, format!("no feature `{key}`")))?;
    match &f.kind {
        FeatureKind::Plugin { plugin_id, params, .. } => Ok((f.id, plugin_id.clone(), params.clone())),
        _ => Err(bad(cmd, format!("`{key}` is not a plugin feature"))),
    }
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "plugin.set";
    known_keys(p, cmd, &["feature", "values"])?;
    let (id, plugin_id, mut params) = plugin_feature(s, p, cmd)?;
    let plugin = plugin_of(&plugin_id, cmd)?;
    params.conform(&plugin.schema());
    let Some(values) = p.get("values").filter(|v| v.is_object()) else { return Err(bad(cmd, "`values` must be an object of parameters")) };
    set_values(&mut params, plugin.as_ref(), Some(values), cmd)?;
    if let Some(f) = s.doc_mut().feature_mut(id)
        && let FeatureKind::Plugin { params: slot, .. } = &mut f.kind
    {
        *slot = params;
    }
    // New inputs (a plugin that gained parameters) get their `dN` names.
    s.doc_mut().name_feature_inputs(id);
    s.refresh();
    if let Some(e) = s.model.result(id).and_then(|r| r.error.clone()) {
        return Err(EngineError::Other(e));
    }
    let warning = s.model.result(id).and_then(|r| r.warning.clone());
    let (values, issues) = output(s, id);
    Ok(json!({"feature": id, "notice": notice(s, id), "values": values, "issues": issues, "warning": warning,
        "recomputed": s.model.last_recomputed}))
}

fn fields(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "plugin.fields";
    known_keys(p, cmd, &["plugin", "feature"])?;
    let (plugin_id, current) = if p.get("feature").is_some() {
        let (id, plugin_id, params) = plugin_feature(s, p, cmd)?;
        (plugin_id, Some((id, params)))
    } else {
        (str_(p, "plugin").ok_or_else(|| bad(cmd, "give `plugin` or `feature`"))?.to_string(), None)
    };
    let plugin = plugin_of(&plugin_id, cmd)?;
    let schema = plugin.schema();
    let values = current.as_ref().map(|(_, params)| {
        let mut params = params.clone();
        params.conform(&schema);
        params.to_json()
    });
    let rows: Vec<Value> = schema
        .fields
        .iter()
        .map(|f| {
            let mut row = json!({"key": f.key, "label": f.label, "group": f.group, "when": f.when, "help": f.help, "unused": f.unused});
            if let Some(o) = row.as_object_mut() {
                match &f.kind {
                    FieldKind::Number { min, default, max, unit, integer } => {
                        o.insert("kind".into(), json!(if *integer { "integer" } else { "number" }));
                        o.insert("min".into(), json!(min));
                        o.insert("default".into(), json!(default));
                        o.insert("max".into(), json!(max));
                        o.insert(
                            "unit".into(),
                            json!(match unit {
                                Unit::Length => "mm",
                                Unit::Angle => "deg",
                                Unit::None => "",
                            }),
                        );
                    }
                    FieldKind::Bool { default } => {
                        o.insert("kind".into(), json!("bool"));
                        o.insert("default".into(), json!(default));
                    }
                    FieldKind::Choice { options, default } => {
                        o.insert("kind".into(), json!("choice"));
                        o.insert("default".into(), json!(default));
                        o.insert("options".into(), json!(options.iter().map(|(id, label)| json!({"id": id, "label": label})).collect::<Vec<_>>()));
                    }
                }
                if let Some(v) = &values {
                    o.insert("value".into(), v.get(&f.key).cloned().unwrap_or(Value::Null));
                    o.insert("shown".into(), json!(schema.shows(f, v)));
                }
            }
            row
        })
        .collect();
    let notice = current.as_ref().and_then(|(id, _)| notice(s, *id));
    let m = plugin.manifest();
    Ok(json!({"plugin": m.id, "name": m.name, "fields": rows, "notice": notice}))
}
