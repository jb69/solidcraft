//! What an operation reports about itself, written for an AI assistant first: a one-line
//! `info`, structured `values` (keys name their unit: `time_s`, `length_mm`, `max_tilt_deg`;
//! counts have none) and `issues`, one object per problem with a stable `code`, so an agent can
//! check numbers and act on problems without parsing sentences. The same shape as a feature
//! notice in the design feedback block.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// One problem an agent can act on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    pub severity: Severity,
    /// Stable code: `gouge`, `rapid_collision`, `no_toolpath`, `tilt_limit`, `tool_too_short`…
    pub code: String,
    pub message: String,
    /// The parameter out of range, when one is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<Value>,
    /// The change that clears it: `{parameter: value}` or what to do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<Value>,
}

impl Issue {
    pub fn new(severity: Severity, code: &str, message: impl Into<String>) -> Issue {
        Issue { severity, code: code.to_string(), message: message.into(), parameter: None, value: None, limit: None, suggestion: None }
    }
    pub fn error(code: &str, message: impl Into<String>) -> Issue {
        Issue::new(Severity::Error, code, message)
    }
    pub fn warning(code: &str, message: impl Into<String>) -> Issue {
        Issue::new(Severity::Warning, code, message)
    }
    pub fn info(code: &str, message: impl Into<String>) -> Issue {
        Issue::new(Severity::Info, code, message)
    }
    pub fn parameter(mut self, name: &str, value: impl Into<Value>) -> Issue {
        self.parameter = Some(name.to_string());
        self.value = Some(value.into());
        self
    }
    pub fn limit(mut self, limit: impl Into<Value>) -> Issue {
        self.limit = Some(limit.into());
        self
    }
    pub fn suggest(mut self, suggestion: Value) -> Issue {
        self.suggestion = Some(suggestion);
        self
    }
}

/// An operation's or setup's report.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Notice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub values: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<Issue>,
}

impl Notice {
    pub fn new(info: impl Into<String>) -> Notice {
        Notice { info: Some(info.into()), ..Default::default() }
    }
    pub fn value(&mut self, key: &str, v: impl Into<Value>) -> &mut Notice {
        self.values.insert(key.to_string(), v.into());
        self
    }
    pub fn issue(&mut self, i: Issue) -> &mut Notice {
        self.issues.push(i);
        self
    }
    pub fn merge(&mut self, other: Notice) {
        if self.info.is_none() {
            self.info = other.info;
        }
        for (k, v) in other.values {
            self.values.entry(k).or_insert(v);
        }
        self.issues.extend(other.issues);
    }
    /// `error`, `warning` or `ok` by the worst issue.
    pub fn status(&self) -> &'static str {
        if self.issues.iter().any(|i| i.severity == Severity::Error) {
            "error"
        } else if self.issues.iter().any(|i| i.severity == Severity::Warning) {
            "warning"
        } else {
            "ok"
        }
    }
    pub fn has_errors(&self) -> bool {
        self.status() == "error"
    }
    pub fn to_json(&self) -> Value {
        let mut v = json!({"status": self.status()});
        if let Some(info) = &self.info {
            v["info"] = json!(info);
        }
        if !self.values.is_empty() {
            v["values"] = Value::Object(self.values.clone());
        }
        if !self.issues.is_empty() {
            v["issues"] = json!(self.issues);
        }
        v
    }
}

/// Rounds a number for a notice value (3 decimals).
pub fn r3(x: f64) -> f64 {
    if x.is_finite() { (x * 1000.0).round() / 1000.0 } else { 0.0 }
}
