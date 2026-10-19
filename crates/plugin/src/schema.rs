//! A plugin's parameters: what the host's dialog shows, how it checks values and when it shows
//! a field.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Every parameter, in dialog order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    pub fields: Vec<Field>,
}

/// What a number means: it decides the unit the dialog shows and how an expression is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// A plain number (a ratio, a count, a percentage).
    #[default]
    None,
    /// Millimetres.
    Length,
    /// Degrees.
    Angle,
}

/// One parameter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Field {
    /// The key in the parameters (`tooth_count`).
    pub key: String,
    /// The dialog's label.
    pub label: String,
    /// The group it sits in; "" for the fields above the first group.
    #[serde(default)]
    pub group: String,
    /// Whether the group starts collapsed (read from a group's first field).
    #[serde(default)]
    pub collapsed: bool,
    pub kind: FieldKind,
    /// When the dialog shows it: `other == ID`, `other != ID`, a flag's key, `!`, `&&`, `||` and
    /// parentheses over the other fields. Empty: always.
    #[serde(default)]
    pub when: String,
    /// The tooltip.
    #[serde(default)]
    pub help: String,
    /// The plugin keeps the value but does not use it yet (the dialog dims it).
    #[serde(default)]
    pub unused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldKind {
    /// A number; the host stores it as an expression, so a parameter can drive it.
    Number {
        min: f64,
        default: f64,
        max: f64,
        #[serde(default)]
        unit: Unit,
        /// Whole numbers only (rounded).
        #[serde(default)]
        integer: bool,
    },
    Bool {
        default: bool,
    },
    /// One of several options, stored as the option's id.
    Choice {
        /// (id, label).
        options: Vec<(String, String)>,
        default: String,
    },
}

impl FieldKind {
    pub fn is_number(&self) -> bool {
        matches!(self, FieldKind::Number { .. })
    }
    /// The default as JSON (a number, a bool or an option id).
    pub fn default_value(&self) -> Value {
        match self {
            FieldKind::Number { default, .. } => Value::from(*default),
            FieldKind::Bool { default } => Value::Bool(*default),
            FieldKind::Choice { default, .. } => Value::String(default.clone()),
        }
    }
}

impl Schema {
    pub fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.key == key)
    }
    /// The groups in dialog order, each with whether it starts collapsed.
    pub fn groups(&self) -> Vec<(String, bool)> {
        let mut out: Vec<(String, bool)> = Vec::new();
        for f in &self.fields {
            if !out.iter().any(|(g, _)| *g == f.group) {
                out.push((f.group.clone(), f.collapsed));
            }
        }
        out
    }
    /// Every parameter at its default.
    pub fn defaults(&self) -> serde_json::Map<String, Value> {
        self.fields.iter().map(|f| (f.key.clone(), f.kind.default_value())).collect()
    }
    /// Whether the dialog shows `f` with these values (a map of the parameters).
    pub fn shows(&self, f: &Field, values: &serde_json::Map<String, Value>) -> bool {
        visible(&f.when, &|k| values.get(k).cloned().or_else(|| self.field(k).map(|g| g.kind.default_value())))
    }
}

/// A value checked against its field: kind, bounds (with a hair of tolerance) and options. A
/// choice may be given by its label; it comes back as its id. Whole numbers are rounded.
pub fn check(f: &Field, v: &Value) -> Result<Value, String> {
    match (&f.kind, v) {
        (FieldKind::Bool { .. }, Value::Bool(b)) => Ok(Value::Bool(*b)),
        (FieldKind::Choice { options, .. }, Value::String(c)) => match options.iter().find(|(id, label)| id.eq_ignore_ascii_case(c) || label == c) {
            Some((id, _)) => Ok(Value::String(id.clone())),
            None => Err(format!("{}: `{c}` is not one of {}", f.label, options.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>().join(", "))),
        },
        (FieldKind::Number { min, max, unit, integer, .. }, Value::Number(n)) => {
            let x = n.as_f64().unwrap_or(f64::NAN);
            let x = if *integer { x.round() } else { x };
            let tol = 1e-9 * (max - min).abs().max(1.0);
            if !x.is_finite() || x < min - tol || x > max + tol {
                let u = match unit {
                    Unit::Length => " mm",
                    Unit::Angle => " deg",
                    Unit::None => "",
                };
                return Err(format!("{} must be within {min}…{max}{u} (it is {x})", f.label));
            }
            Ok(Value::from(x))
        }
        (_, v) => Err(format!("{}: {v} is the wrong kind of value", f.label)),
    }
}

/// Evaluate a show condition: `a == ID`, `a != ID`, a bare flag, `!`, `&&`, `||` and parentheses.
/// `get` gives a field's value. A condition that cannot be read shows the field.
pub fn visible(when: &str, get: &dyn Fn(&str) -> Option<Value>) -> bool {
    if when.trim().is_empty() {
        return true;
    }
    let toks = tokens(when);
    let mut p = Parser { toks: &toks, i: 0, get };
    match p.or() {
        Some(b) if p.i == toks.len() => b,
        _ => true,
    }
}

fn tokens(s: &str) -> Vec<String> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&ch) = c.get(i) {
        if ch.is_whitespace() {
            i += 1;
        } else if ch.is_alphanumeric() || ch == '_' || ch == '.' {
            let st = i;
            while c.get(i).is_some_and(|x| x.is_alphanumeric() || *x == '_' || *x == '.') {
                i += 1;
            }
            out.push(c[st..i].iter().collect());
        } else if let Some(&next) = c.get(i + 1)
            && matches!((ch, next), ('=', '=') | ('!', '=') | ('&', '&') | ('|', '|'))
        {
            out.push([ch, next].iter().collect());
            i += 2;
        } else {
            out.push(ch.to_string());
            i += 1;
        }
    }
    out
}

struct Parser<'a> {
    toks: &'a [String],
    i: usize,
    get: &'a dyn Fn(&str) -> Option<Value>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&str> {
        self.toks.get(self.i).map(String::as_str)
    }
    fn or(&mut self) -> Option<bool> {
        let mut v = self.and()?;
        while self.peek() == Some("||") {
            self.i += 1;
            let r = self.and()?;
            v = v || r;
        }
        Some(v)
    }
    fn and(&mut self) -> Option<bool> {
        let mut v = self.not()?;
        while self.peek() == Some("&&") {
            self.i += 1;
            let r = self.not()?;
            v = v && r;
        }
        Some(v)
    }
    fn not(&mut self) -> Option<bool> {
        if self.peek() == Some("!") {
            self.i += 1;
            return self.not().map(|b| !b);
        }
        if self.peek() == Some("(") {
            self.i += 1;
            let v = self.or()?;
            if self.peek() != Some(")") {
                return None;
            }
            self.i += 1;
            return Some(v);
        }
        let name = self.peek()?.to_string();
        self.i += 1;
        let value = (self.get)(&name);
        match self.peek() {
            Some(op @ ("==" | "!=")) => {
                let eq = op == "==";
                self.i += 1;
                let rhs = self.peek()?.to_string();
                self.i += 1;
                let same = match value {
                    Some(Value::String(s)) => s == rhs,
                    Some(Value::Bool(b)) => (if b { "true" } else { "false" }) == rhs,
                    Some(Value::Number(n)) => rhs.parse::<f64>().ok().zip(n.as_f64()).is_some_and(|(a, b)| a == b),
                    _ => false,
                };
                Some(if eq { same } else { !same })
            }
            _ => match name.as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => Some(matches!(value, Some(Value::Bool(true)))),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn conditions() {
        let v = json!({"shape": "DOUBLE", "tips": true, "n": 3});
        let get = |k: &str| v.get(k).cloned();
        assert!(visible("", &get));
        assert!(visible("shape == DOUBLE", &get));
        assert!(!visible("shape != DOUBLE", &get));
        assert!(visible("tips && (shape == LINEAR || shape == DOUBLE)", &get));
        assert!(!visible("!tips", &get));
        assert!(visible("n == 3", &get));
        // unreadable: shown
        assert!(visible("shape ==", &get));
    }

    #[test]
    fn checks() {
        let f = Field {
            key: "r".into(),
            label: "Radius".into(),
            group: String::new(),
            collapsed: false,
            kind: FieldKind::Number { min: 1.0, default: 5.0, max: 10.0, unit: Unit::Length, integer: false },
            when: String::new(),
            help: String::new(),
            unused: false,
        };
        assert_eq!(check(&f, &json!(3)).unwrap(), json!(3.0));
        assert_eq!(check(&f, &json!(11)).unwrap_err(), "Radius must be within 1…10 mm (it is 11)");
        let c = Field { kind: FieldKind::Choice { options: vec![("A".into(), "Alpha".into())], default: "A".into() }, ..f };
        assert_eq!(check(&c, &json!("Alpha")).unwrap(), json!("A"));
        assert!(check(&c, &json!("B")).is_err());
    }
}
