//! The dialogs of the foil features: every field of the Onshape feature, in its order, with its
//! group, bounds, default, tooltip and the condition under which Onshape shows it. The tables
//! themselves are generated from the FeatureScript (`schema_gen.rs`, `gen/gen_schema.py`).

use std::collections::BTreeMap;

/// What a field holds. Numbers are in the FeatureScript's units: lengths in mm, angles in
/// degrees, the rest plain (percentages as numbers of percent).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Real {
        min: f64,
        default: f64,
        max: f64,
    },
    Length {
        min: f64,
        default: f64,
        max: f64,
    },
    Angle {
        min: f64,
        default: f64,
        max: f64,
    },
    Int {
        min: f64,
        default: f64,
        max: f64,
    },
    Bool {
        default: bool,
    },
    /// Options as (id, label); the default is an id.
    Enum {
        options: &'static [(&'static str, &'static str)],
        default: &'static str,
    },
}

impl Kind {
    /// The bounds and default of a number field.
    pub fn bounds(&self) -> Option<(f64, f64, f64)> {
        match *self {
            Kind::Real { min, default, max }
            | Kind::Length { min, default, max }
            | Kind::Angle { min, default, max }
            | Kind::Int { min, default, max } => Some((min, default, max)),
            _ => None,
        }
    }
    pub fn is_number(&self) -> bool {
        self.bounds().is_some()
    }
    /// The unit shown after a number (`mm`, `deg`) or none.
    pub fn unit(&self) -> &'static str {
        match self {
            Kind::Length { .. } => "mm",
            Kind::Angle { .. } => "deg",
            _ => "",
        }
    }
}

/// One dialog field.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    /// The FeatureScript's `definition` key (`targetArea`).
    pub key: &'static str,
    /// The dialog label (`Area cm^2`).
    pub label: &'static str,
    /// The group it sits in ("" for the fields above the first group).
    pub group: &'static str,
    /// Whether Onshape shows the group collapsed at first.
    pub collapsed: bool,
    pub kind: Kind,
    /// When Onshape shows it, in the FeatureScript's terms (`profile == NACA4 && meanLine == A6`);
    /// empty = always.
    pub when: &'static str,
    /// The FeatureScript's description (tooltip), HTML as written there.
    pub help: &'static str,
}

/// Which feature a dialog belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Wing,
    Stabiliser,
    Propeller,
}

impl Tool {
    pub fn fields(self) -> &'static [Field] {
        match self {
            Tool::Wing => crate::schema_gen::WING,
            Tool::Stabiliser => crate::schema_gen::STAB,
            Tool::Propeller => crate::schema_gen::PROP,
        }
    }
    pub fn field(self, key: &str) -> Option<&'static Field> {
        self.fields().iter().find(|f| f.key == key)
    }
    /// The groups in dialog order, each with whether it starts collapsed.
    pub fn groups(self) -> Vec<(&'static str, bool)> {
        let mut out: Vec<(&'static str, bool)> = Vec::new();
        for f in self.fields() {
            if !out.iter().any(|(g, _)| *g == f.group) {
                out.push((f.group, f.collapsed));
            }
        }
        out
    }
    /// The Onshape feature's name.
    pub fn title(self) -> &'static str {
        match self {
            Tool::Wing => "Foil wing",
            Tool::Stabiliser => "Foil stabiliser",
            Tool::Propeller => "B-Series Propeller",
        }
    }
}

/// A field's value: a number in the field's units, a flag or an enum option id.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Num(f64),
    Flag(bool),
    Choice(String),
}

/// The values of one feature, every field present (defaults where none was given).
#[derive(Clone, Debug, PartialEq)]
pub struct Values {
    pub tool: Tool,
    map: BTreeMap<&'static str, Val>,
}

impl Values {
    /// The FeatureScript's defaults.
    pub fn defaults(tool: Tool) -> Values {
        let map = tool.fields().iter().map(|f| (f.key, default_of(&f.kind))).collect();
        Values { tool, map }
    }
    /// Set one value; refused for an unknown key, the wrong kind, a number outside the field's
    /// bounds or an option the enum does not have.
    pub fn set(&mut self, key: &str, v: Val) -> Result<(), String> {
        let f = self.tool.field(key).ok_or_else(|| format!("{} has no field `{key}`", self.tool.title()))?;
        let v = check(f, v)?;
        self.map.insert(f.key, v);
        Ok(())
    }
    /// A number (0 for a missing or non-number key: the schema makes that a programming error).
    pub fn num(&self, key: &str) -> f64 {
        match self.map.get(key) {
            Some(Val::Num(x)) => *x,
            _ => 0.0,
        }
    }
    pub fn flag(&self, key: &str) -> bool {
        matches!(self.map.get(key), Some(Val::Flag(true)))
    }
    pub fn choice(&self, key: &str) -> &str {
        match self.map.get(key) {
            Some(Val::Choice(s)) => s,
            _ => "",
        }
    }
    pub fn get(&self, key: &str) -> Option<&Val> {
        self.map.get(key)
    }
    /// Whether Onshape shows field `f` with these values.
    pub fn shows(&self, f: &Field) -> bool {
        visible(f.when, &|k| self.map.get(k).cloned())
    }
}

pub fn default_of(k: &Kind) -> Val {
    match *k {
        Kind::Bool { default } => Val::Flag(default),
        Kind::Enum { default, .. } => Val::Choice(default.to_string()),
        Kind::Real { default, .. } | Kind::Length { default, .. } | Kind::Angle { default, .. } | Kind::Int { default, .. } => Val::Num(default),
    }
}

/// A value checked against its field: kind, bounds (as Onshape checks them, to a hair) and options.
pub fn check(f: &Field, v: Val) -> Result<Val, String> {
    match (&f.kind, v) {
        (Kind::Bool { .. }, Val::Flag(b)) => Ok(Val::Flag(b)),
        (Kind::Enum { options, .. }, Val::Choice(c)) => match options.iter().find(|(id, label)| id.eq_ignore_ascii_case(&c) || *label == c) {
            Some((id, _)) => Ok(Val::Choice(id.to_string())),
            None => Err(format!("{}: `{c}` is not one of {}", f.label, options.iter().map(|(id, _)| *id).collect::<Vec<_>>().join(", "))),
        },
        (k, Val::Num(x)) if k.is_number() => {
            let (lo, _, hi) = k.bounds().unwrap_or((f64::MIN, 0.0, f64::MAX));
            let x = if matches!(k, Kind::Int { .. }) { x.round() } else { x };
            let tol = 1e-9 * (hi - lo).abs().max(1.0);
            if !x.is_finite() || x < lo - tol || x > hi + tol {
                return Err(format!("{} must be within {lo}…{hi}{} (it is {x})", f.label, unit_suffix(k)));
            }
            Ok(Val::Num(x))
        }
        (_, v) => Err(format!("{}: {v:?} is the wrong kind of value", f.label)),
    }
}

fn unit_suffix(k: &Kind) -> &'static str {
    match k {
        Kind::Length { .. } => " mm",
        Kind::Angle { .. } => " deg",
        _ => "",
    }
}

/// Evaluate a FeatureScript visibility condition: `a == X`, `a != X`, a bare boolean field, `!`,
/// `&&`, `||` and parentheses. An unreadable condition shows the field.
pub fn visible(when: &str, get: &dyn Fn(&str) -> Option<Val>) -> bool {
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
    let mut out = Vec::new();
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch.is_alphanumeric() || ch == '_' {
            let st = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            out.push(c[st..i].iter().collect());
        } else if i + 1 < c.len() && matches!((ch, c[i + 1]), ('=', '=') | ('!', '=') | ('&', '&') | ('|', '|')) {
            out.push(c[i..i + 2].iter().collect());
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
    get: &'a dyn Fn(&str) -> Option<Val>,
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
        match self.peek() {
            Some(op @ ("==" | "!=")) => {
                let eq = op == "==";
                self.i += 1;
                let rhs = self.peek()?.to_string();
                self.i += 1;
                let lhs = match (self.get)(&name) {
                    Some(Val::Choice(c)) => c == rhs,
                    Some(Val::Flag(b)) => (if b { "true" } else { "false" }) == rhs,
                    _ => false,
                };
                Some(if eq { lhs } else { !lhs })
            }
            _ => match name.as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => Some(matches!((self.get)(&name), Some(Val::Flag(true)))),
            },
        }
    }
}

/// The help text without its HTML: line breaks for `<br>`, other tags dropped.
pub fn plain_help(html: &str) -> String {
    let s = html.replace("<br>", "\n").replace("<br/>", "\n").replace("<br />", "\n");
    let mut out = String::with_capacity(s.len());
    let mut tag = false;
    for ch in s.chars() {
        match ch {
            '<' => tag = true,
            '>' if tag => tag = false,
            _ if !tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// Groups and fields not built yet in SolidCraft (they stay in the dialog with their values).
pub fn not_built(tool: Tool, f: &Field) -> bool {
    match tool {
        Tool::Wing => matches!(f.group, "Fuselage joint" | "Section polar") || f.key == "logInputs",
        Tool::Stabiliser => (matches!(f.group, "Mount pad" | "Mount bolts" | "Section polar") && f.key != "invertSection") || f.key == "logInputs",
        Tool::Propeller => {
            matches!(f.group, "Folding" | "Operating check")
                || matches!(f.key, "rootFillet" | "hubCore" | "coreSkin" | "keepSections" | "debugPrint" | "sectionsOnly")
        }
    }
}
