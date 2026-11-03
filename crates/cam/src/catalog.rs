//! Machines documented as data. Each known machine is a file (`crates/cam/machines/*.json`,
//! described in `docs/cam-machines.md`): the machine's numbers in the shape a setup stores, its
//! ids and aliases, the options it can be fitted with (a 4th-axis module: only the fields that
//! change), the tool kit that suits it, and where each number came from. The analysis, the
//! plan and every check read a machine's limits from here, and a new machine is added by
//! writing a file, not code. Files are checked as hostile input: every number finite and in
//! range, every list capped.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::machine::{Kinematics, Machine, ToolChanger};
use crate::{CamError, Result};

/// The format tag every machine file starts with.
pub const FORMAT: &str = "solidcraft-machine/1";

/// The files SolidCraft ships, in the order shown to people.
const BUILTIN: &[&str] = &[
    include_str!("../machines/generic-3-axis-mill.json"),
    include_str!("../machines/generic-5-axis-trunnion-ac.json"),
    include_str!("../machines/generic-5-axis-trunnion-bc.json"),
    include_str!("../machines/generic-5-axis-head-head-bc.json"),
    include_str!("../machines/makera-carvera.json"),
    include_str!("../machines/makera-carvera-air.json"),
    include_str!("../machines/makera-z1.json"),
];

/// Caps on a file: it is read from anywhere an assistant or a person can put one.
const MAX_FILE_BYTES: usize = 256 * 1024;
const MAX_LIST: usize = 32;
const MAX_TEXT: usize = 2000;
const MAX_ID: usize = 64;
/// The largest length (mm), speed (rev/min, mm/min) or power (W) a machine file may give.
const MAX_SIZE_MM: f64 = 100_000.0;
const MAX_RPM: f64 = 200_000.0;
const MAX_FEED: f64 = 1_000_000.0;
const MAX_POWER_W: f64 = 1_000_000.0;

/// One machine file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineFile {
    /// [`FORMAT`].
    pub format: String,
    /// The id a setup names (`carvera`): lower case letters, digits and `_`.
    pub id: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// One sentence on what the machine is.
    #[serde(default)]
    pub summary: String,
    /// Every number, in the shape a setup stores ([`Machine`]); a field left out takes its
    /// default.
    pub machine: Machine,
    /// What it can be fitted with: each a machine of its own, the base with these fields changed.
    #[serde(default)]
    pub options: Vec<MachineOption>,
    /// The tool kit that suits it (`cam.tool.kit`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kit: Option<String>,
    /// Where the numbers came from.
    #[serde(default)]
    pub sources: Vec<Source>,
    /// What a programmer should know about it, one point each.
    #[serde(default)]
    pub notes: Vec<String>,
}

/// An option a machine can be fitted with (a 4th-axis module, a laser).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineOption {
    pub id: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub summary: String,
    /// The fields that change from the base machine.
    #[serde(default)]
    pub machine: Value,
}

/// Where some of a machine's numbers came from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// The machine fields it gives (`work_area`, `spindle_rpm_max`…).
    #[serde(default)]
    pub fields: Vec<String>,
    /// The document: the maker's specification page, a manual, a measurement.
    pub from: String,
    /// When it was last checked against that document (`2026-10-25`); `None`: not yet.
    #[serde(default)]
    pub checked: Option<String>,
}

/// One machine the catalog knows: the base or one of its options.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub aliases: Vec<String>,
    pub summary: String,
    pub machine: Machine,
    /// The base machine's id, for an option.
    pub base: Option<String>,
    pub kit: Option<String>,
    pub sources: Vec<Source>,
    pub notes: Vec<String>,
}

impl MachineFile {
    /// Reads a machine file and checks it.
    pub fn parse(text: &str) -> Result<MachineFile> {
        if text.len() > MAX_FILE_BYTES {
            return Err(CamError::TooLarge(format!("a machine file is at most {} KB", MAX_FILE_BYTES / 1024)));
        }
        let file: MachineFile = serde_json::from_str(text).map_err(|e| CamError::Param(format!("machine file: {e}")))?;
        file.check()?;
        Ok(file)
    }

    /// Reads a machine file given as JSON.
    pub fn from_value(v: &Value) -> Result<MachineFile> {
        let text = serde_json::to_string(v).map_err(|e| CamError::Param(format!("machine file: {e}")))?;
        MachineFile::parse(&text)
    }

    /// Every machine in the file: the base, then each option.
    pub fn entries(&self) -> Result<Vec<Entry>> {
        let mut out = vec![Entry {
            id: self.id.clone(),
            aliases: self.aliases.clone(),
            summary: self.summary.clone(),
            machine: self.machine.clone(),
            base: None,
            kit: self.kit.clone(),
            sources: self.sources.clone(),
            notes: self.notes.clone(),
        }];
        for o in &self.options {
            out.push(Entry {
                id: o.id.clone(),
                aliases: o.aliases.clone(),
                summary: o.summary.clone(),
                machine: self.option_machine(o)?,
                base: Some(self.id.clone()),
                kit: self.kit.clone(),
                sources: self.sources.clone(),
                notes: self.notes.clone(),
            });
        }
        Ok(out)
    }

    /// The base machine with the option's fields over it.
    fn option_machine(&self, o: &MachineOption) -> Result<Machine> {
        let mut v = serde_json::to_value(&self.machine).map_err(|e| CamError::Param(format!("machine file: {e}")))?;
        match (&mut v, &o.machine) {
            (Value::Object(base), Value::Object(changes)) => {
                for (k, x) in changes {
                    base.insert(k.clone(), x.clone());
                }
            }
            (_, Value::Null) => {}
            _ => return Err(CamError::Param(format!("option `{}`: `machine` lists the fields that change, as an object", o.id))),
        }
        serde_json::from_value(v).map_err(|e| CamError::Param(format!("option `{}`: {e}", o.id)))
    }

    fn check(&self) -> Result<()> {
        if self.format != FORMAT {
            return Err(CamError::Param(format!("machine file: `format` is `{FORMAT}`, not `{}`", self.format)));
        }
        id_ok(&self.id)?;
        let mut ids = vec![self.id.as_str()];
        for a in &self.aliases {
            id_ok(a)?;
            ids.push(a);
        }
        text_ok("summary", &self.summary)?;
        for (what, n) in [("aliases", self.aliases.len()), ("options", self.options.len()), ("sources", self.sources.len()), ("notes", self.notes.len())] {
            if n > MAX_LIST {
                return Err(CamError::TooLarge(format!("machine file: at most {MAX_LIST} {what}")));
            }
        }
        for n in &self.notes {
            text_ok("note", n)?;
        }
        for s in &self.sources {
            text_ok("source", &s.from)?;
            if s.fields.len() > MAX_LIST * 2 {
                return Err(CamError::TooLarge("machine file: a source lists too many fields".into()));
            }
            if let Some(c) = &s.checked {
                text_ok("checked", c)?;
            }
        }
        if let Some(k) = &self.kit {
            id_ok(k)?;
        }
        machine_ok(&self.id, &self.machine)?;
        for o in &self.options {
            id_ok(&o.id)?;
            ids.push(&o.id);
            for a in &o.aliases {
                id_ok(a)?;
                ids.push(a);
            }
            if o.aliases.len() > MAX_LIST {
                return Err(CamError::TooLarge(format!("option `{}`: at most {MAX_LIST} aliases", o.id)));
            }
            text_ok("summary", &o.summary)?;
            machine_ok(&o.id, &self.option_machine(o)?)?;
        }
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        if let Some(w) = sorted.windows(2).find(|w| w[0] == w[1]) {
            return Err(CamError::Param(format!("machine file: the id `{}` is given twice", w[0])));
        }
        Ok(())
    }
}

/// A machine id: 1 to 64 lower-case letters, digits and `_`.
fn id_ok(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > MAX_ID || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        return Err(CamError::Param(format!("machine id `{id}`: 1 to {MAX_ID} lower-case letters, digits and `_`")));
    }
    Ok(())
}

fn text_ok(what: &str, s: &str) -> Result<()> {
    if s.len() > MAX_TEXT {
        return Err(CamError::TooLarge(format!("machine file: a {what} is at most {MAX_TEXT} characters")));
    }
    Ok(())
}

/// Every number of a machine finite and in range.
fn machine_ok(id: &str, m: &Machine) -> Result<()> {
    let bad = |what: &str, v: f64, max: f64| -> Result<()> {
        if v.is_finite() && (0.0..=max).contains(&v) {
            Ok(())
        } else {
            Err(CamError::Param(format!("machine `{id}`: {what} is {v}; it is 0 (not set) up to {max}")))
        }
    };
    if m.name.trim().is_empty() || m.name.len() > 120 {
        return Err(CamError::Param(format!("machine `{id}`: a name of 1 to 120 characters")));
    }
    for (what, v) in [("work_area x", m.work_area.x), ("work_area y", m.work_area.y), ("work_area z", m.work_area.z), ("gantry_clearance", m.gantry_clearance)] {
        bad(what, v, MAX_SIZE_MM)?;
    }
    bad("spindle_rpm_min", m.spindle_rpm_min, MAX_RPM)?;
    bad("spindle_rpm_max", m.spindle_rpm_max, MAX_RPM)?;
    if m.spindle_rpm_max > 0.0 && m.spindle_rpm_min > m.spindle_rpm_max {
        return Err(CamError::Param(format!("machine `{id}`: spindle_rpm_min is above spindle_rpm_max")));
    }
    bad("spindle_power_w", m.spindle_power_w, MAX_POWER_W)?;
    bad("max_feed", m.max_feed, MAX_FEED)?;
    bad("rapid_feed", m.rapid_feed, MAX_FEED)?;
    bad("rotary_rapid_deg_min", m.rotary_rapid_deg_min, MAX_FEED)?;
    if m.collets.len() > MAX_LIST {
        return Err(CamError::TooLarge(format!("machine `{id}`: at most {MAX_LIST} collets")));
    }
    for c in &m.collets {
        if !(c.is_finite() && *c > 0.0 && *c <= 100.0) {
            return Err(CamError::Param(format!("machine `{id}`: a collet size is {c} mm; it is above 0 and at most 100")));
        }
    }
    if let ToolChanger::Automatic { slots } = m.tool_changer
        && !(1..=1000).contains(&slots)
    {
        return Err(CamError::Param(format!("machine `{id}`: a tool changer has 1 to 1000 slots")));
    }
    if let Some((d, l)) = m.rotary_capacity {
        bad("rotary_capacity diameter", d, MAX_SIZE_MM)?;
        bad("rotary_capacity length", l, MAX_SIZE_MM)?;
    }
    if let Some(l) = &m.laser {
        bad("laser power_w", l.power_w, MAX_POWER_W)?;
        bad("laser wavelength_nm", l.wavelength_nm, 100_000.0)?;
        bad("laser spot_mm", l.spot_mm, 100.0)?;
    }
    if let (Some(lo), Some(hi)) = (m.travel_min, m.travel_max)
        && !(lo.is_finite() && hi.is_finite() && lo.x <= hi.x && lo.y <= hi.y && lo.z <= hi.z)
    {
        return Err(CamError::Param(format!("machine `{id}`: travel_min must be finite and below travel_max")));
    }
    if matches!(m.kinematics, Kinematics::FourAxis { .. } | Kinematics::FiveAxis { .. }) || m.tcp {
        bad("max_rotary_step_deg", m.max_rotary_step_deg, 360.0)?;
    }
    m.validate().map_err(|e| CamError::Param(format!("machine `{id}`: {e}")))
}

/// The machines SolidCraft ships, read once.
pub fn builtin() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        // A shipped file that does not read is a bug the tests catch; at run time it is left
        // out rather than taking the others with it.
        BUILTIN.iter().filter_map(|t| MachineFile::parse(t).ok()).filter_map(|f| f.entries().ok()).flatten().collect()
    })
}

/// A machine by id or alias among `entries` (case, spaces and `-` don't matter).
pub fn find<'a>(entries: &'a [Entry], id: &str) -> Option<&'a Entry> {
    let key = id.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    entries.iter().find(|e| e.id == key || e.aliases.iter().any(|a| *a == key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_machine_file_reads_and_checks() {
        for t in BUILTIN {
            let f = MachineFile::parse(t).unwrap_or_else(|e| panic!("{e}: {}", &t[..t.len().min(80)]));
            assert!(f.entries().unwrap().iter().all(|e| !e.machine.name.is_empty()));
        }
        assert_eq!(builtin().len(), 11, "the four generic machines, three Makera machines and their four options");
    }

    #[test]
    fn options_change_only_their_fields() {
        let air4 = find(builtin(), "carvera-air 4th axis").unwrap();
        let air = find(builtin(), "carvera_air").unwrap();
        assert_eq!(air4.base.as_deref(), Some("carvera_air"));
        assert_eq!(air4.machine.work_area, air.machine.work_area);
        assert_eq!(air4.machine.rotary_capacity, Some((92.0, 200.0)));
        assert!(matches!(air4.machine.kinematics, Kinematics::FourAxis { .. }));
    }

    #[test]
    fn hostile_machine_files_are_refused() {
        let good: Value = serde_json::from_str(include_str!("../machines/makera-z1.json")).unwrap();
        let cases: Vec<(&str, Value)> = vec![
            ("format", serde_json::json!("other/2")),
            ("id", serde_json::json!("Not An Id!")),
            ("machine", serde_json::json!({"name": "x", "work_area": [1e300, 0, 0]})),
            ("machine", serde_json::json!({"name": "x", "spindle_rpm_min": 9000, "spindle_rpm_max": 100})),
            ("machine", serde_json::json!({"name": "x", "collets": [-3]})),
            ("machine", serde_json::json!({"name": "x", "tool_changer": {"kind": "automatic", "slots": 0}})),
            ("machine", serde_json::json!({"name": ""})),
            ("aliases", serde_json::json!(vec!["a"; 40])),
            ("aliases", serde_json::json!(["z1"])),
            ("options", serde_json::json!([{"id": "o", "machine": [1, 2]}])),
            ("options", serde_json::json!([{"id": "o", "machine": {"max_feed": -1}}])),
            ("unknown_field", serde_json::json!(1)),
        ];
        for (k, v) in cases {
            let mut f = good.clone();
            f[k] = v.clone();
            assert!(MachineFile::from_value(&f).is_err(), "{k} = {v} was accepted");
        }
        assert!(MachineFile::parse(&" ".repeat(MAX_FILE_BYTES + 1)).is_err());
        assert!(MachineFile::parse("not json").is_err());
    }
}
