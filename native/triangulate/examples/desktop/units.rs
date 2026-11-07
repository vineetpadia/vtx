//! Length units describe rendered coordinates, after any importer normalization.
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Serialize)]
pub struct Units {
    pub label: String,
    pub source: String,
    pub status: String,
    pub meters_per_model_unit: Option<f64>,
    pub evidence: Value,
    pub note: String,
}
impl Units {
    pub fn unknown(reason: &str) -> Self {
        Self {
            label: "model units".into(),
            source: "unspecified".into(),
            status: "unknown".into(),
            meters_per_model_unit: None,
            evidence: Value::Null,
            note: reason.into(),
        }
    }
    pub fn known(label: &str, factor: f64, source: &str, evidence: Value) -> Self {
        Self {
            label: label.into(),
            source: source.into(),
            status: "known".into(),
            meters_per_model_unit: Some(factor),
            evidence,
            note:
                "Units describe imported/rendered coordinates; reporting does not rescale geometry."
                    .into(),
        }
    }
    pub fn overridden(&self, label: &str) -> Self {
        if label == "model" {
            return self.clone();
        }
        let factor = length_factor(label).expect("validated unit override");
        let mut units = Self::known(label, factor, "user_override", json!({"detected":self}));
        units.note = "User supplied unit label; coordinates are not rescaled. The user is responsible for matching imported coordinate values.".into();
        if self
            .meters_per_model_unit
            .is_some_and(|f| !same_scale(f, factor))
        {
            units
                .note
                .push_str(" WARNING: override conflicts with detected coordinate units.");
        }
        units
    }
}
pub fn length_factor(label: &str) -> Option<f64> {
    Some(match label {
        "mm" => 0.001,
        "cm" => 0.01,
        "m" => 1.0,
        "in" => 0.0254,
        "ft" => 0.3048,
        _ => return None,
    })
}
fn same_scale(a: f64, b: f64) -> bool {
    (a - b).abs() <= a.abs().max(b.abs()) * 1e-10
}
fn scale_label(scale: f64) -> String {
    for (label, factor) in [
        ("mm", 0.001),
        ("cm", 0.01),
        ("m", 1.0),
        ("in", 0.0254),
        ("ft", 0.3048),
        ("um", 1e-6),
        ("nm", 1e-9),
        ("km", 1000.0),
    ] {
        if same_scale(scale, factor) {
            return label.into();
        }
    }
    format!("unit ({:.6e} m)", scale)
}

// Parse only the STEP unit graph, without relying on names such as 'INCH'.
// Other STEP data is deliberately left to the existing geometry importer.
#[derive(Debug)]
struct Component {
    name: String,
    args: Vec<String>,
}
type Records = BTreeMap<usize, Vec<Component>>;
fn fields(value: &str) -> Option<Vec<String>> {
    let mut depth = 0_i32;
    let mut quoted = false;
    let mut start = 0;
    let mut out = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                if quoted && bytes.get(i + 1) == Some(&b'\'') {
                    i += 2;
                    continue;
                }
                quoted = !quoted;
            }
            b'(' if !quoted => depth += 1,
            b')' if !quoted => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
            }
            b',' if !quoted && depth == 0 => {
                out.push(value[start..i].trim().into());
                start = i + 1;
            }
            _ => (),
        }
        i += 1;
    }
    if quoted || depth != 0 {
        return None;
    }
    if !value.is_empty() {
        out.push(value[start..].trim().into());
    }
    Some(out)
}
fn components(body: &str) -> Option<Vec<Component>> {
    let body = body.trim();
    let body = if body.starts_with('(') {
        body.strip_prefix('(')?.strip_suffix(')')?
    } else {
        body
    };
    let mut rest = body;
    let mut out = Vec::new();
    while !rest.trim().is_empty() {
        rest = rest.trim();
        let open = rest.find('(')?;
        let name = &rest[..open];
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return None;
        }
        let mut depth = 1;
        let mut quoted = false;
        let bytes = rest.as_bytes();
        let mut i = open + 1;
        while i < bytes.len() && depth > 0 {
            match bytes[i] {
                b'\'' => {
                    if quoted && bytes.get(i + 1) == Some(&b'\'') {
                        i += 2;
                        continue;
                    }
                    quoted = !quoted;
                }
                b'(' if !quoted => depth += 1,
                b')' if !quoted => depth -= 1,
                _ => (),
            }
            i += 1;
        }
        if depth != 0 || quoted {
            return None;
        }
        out.push(Component {
            name: name.into(),
            args: fields(&rest[open + 1..i - 1])?,
        });
        rest = &rest[i..];
    }
    Some(out)
}
fn reference(value: &str) -> Option<usize> {
    value.trim().strip_prefix('#')?.parse().ok()
}
fn component<'a>(records: &'a Records, id: usize, name: &str) -> Option<&'a Component> {
    records.get(&id)?.iter().find(|c| c.name == name)
}
fn prefix(value: &str) -> Option<f64> {
    Some(match value {
        "$" => 1.0,
        ".EXA." => 1e18,
        ".PETA." => 1e15,
        ".TERA." => 1e12,
        ".GIGA." => 1e9,
        ".MEGA." => 1e6,
        ".KILO." => 1e3,
        ".HECTO." => 1e2,
        ".DECA." => 1e1,
        ".DECI." => 1e-1,
        ".CENTI." => 1e-2,
        ".MILLI." => 1e-3,
        ".MICRO." => 1e-6,
        ".NANO." => 1e-9,
        ".PICO." => 1e-12,
        ".FEMTO." => 1e-15,
        ".ATTO." => 1e-18,
        _ => return None,
    })
}
fn scale(records: &Records, id: usize, visiting: &mut HashSet<usize>) -> Option<f64> {
    if visiting.len() > 64 || !visiting.insert(id) {
        return None;
    }
    let result = (|| {
        if let Some(si) = component(records, id, "SI_UNIT") {
            if si.args.len() == 2 && si.args[1] == ".METRE." {
                return prefix(&si.args[0]);
            }
            return None;
        }
        // Require explicit LENGTH_UNIT: a conversion-based angle is not a length.
        component(records, id, "LENGTH_UNIT")?;
        let conversion = component(records, id, "CONVERSION_BASED_UNIT")?;
        let measure_id = reference(conversion.args.last()?)?;
        let measure = component(records, measure_id, "LENGTH_MEASURE_WITH_UNIT")
            .or_else(|| component(records, measure_id, "MEASURE_WITH_UNIT"))?;
        if measure.args.len() != 2 {
            return None;
        }
        let value = &measure.args[0];
        let numeric = if let Some(inner) = value
            .strip_prefix("LENGTH_MEASURE(")
            .and_then(|s| s.strip_suffix(')'))
        {
            inner
        } else {
            value
        };
        let multiplier = numeric.replace(['D', 'd'], "E").parse::<f64>().ok()?;
        let result = multiplier * scale(records, reference(&measure.args[1])?, visiting)?;
        (result.is_finite() && result > 0.0).then_some(result)
    })();
    visiting.remove(&id);
    result
}

/// Input has already been comment/whitespace-normalized by StepFile::strip_flatten.
pub fn step_units(flat: &[u8]) -> Units {
    let text = String::from_utf8_lossy(flat);
    let mut records = Records::new();
    let mut malformed_context = false;
    let mut start = 0;
    let mut quoted = false;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            if quoted && bytes.get(i + 1) == Some(&b'\'') {
                i += 2;
                continue;
            }
            quoted = !quoted;
        }
        if bytes[i] == b';' && !quoted {
            let record = &text[start..i];
            start = i + 1;
            if let Some((id, body)) = record.strip_prefix('#').and_then(|s| s.split_once('=')) {
                if body.contains("UNIT") {
                    if let (Ok(id), Some(parts)) = (id.parse(), components(body)) {
                        records.insert(id, parts);
                    } else if body.contains("GLOBAL_UNIT_ASSIGNED_CONTEXT") {
                        malformed_context = true;
                    }
                }
            }
        }
        i += 1;
    }
    let mut evidence = Vec::new();
    let mut scales = Vec::new();
    let mut unresolved = malformed_context;
    for (&id, parts) in &records {
        for context in parts
            .iter()
            .filter(|c| c.name == "GLOBAL_UNIT_ASSIGNED_CONTEXT")
        {
            let refs = context
                .args
                .last()
                .and_then(|s| s.strip_prefix('(')?.strip_suffix(')'))
                .and_then(fields);
            let mut lengths = Vec::new();
            if let Some(refs) = refs {
                for unit in refs {
                    let Some(unit_id) = reference(&unit) else {
                        unresolved = true;
                        continue;
                    };
                    if let Some(factor) = scale(&records, unit_id, &mut HashSet::new()) {
                        lengths.push((unit_id, factor));
                    } else if !records.contains_key(&unit_id)
                        || component(&records, unit_id, "LENGTH_UNIT").is_some()
                    {
                        unresolved = true;
                    }
                }
            }
            if lengths.len() != 1 {
                unresolved = true;
            }
            for (unit_id, factor) in lengths {
                evidence.push(
                    json!({"context_id":id,"unit_id":unit_id,"meters_per_model_unit":factor}),
                );
                scales.push(factor);
            }
        }
    }
    if scales.is_empty() {
        return Units::unknown(
            "No unambiguous context-assigned STEP length unit could be resolved.",
        );
    }
    let conflicting = scales.iter().any(|&f| !same_scale(f, scales[0]));
    if unresolved || conflicting {
        let mut units=Units::unknown("STEP contexts have conflicting or unresolved length units. The geometry importer does not normalize mixed-unit contexts; physical conversion is disabled.");
        units.status = "ambiguous".into();
        units.source = "step_context".into();
        units.evidence = json!(evidence);
        return units;
    }
    Units::known(
        &scale_label(scales[0]),
        scales[0],
        "step_context",
        json!({"contexts":evidence,"coordinates_rescaled":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn read(data: &str) -> Units {
        step_units(data.as_bytes())
    }
    const MM:&str="#1=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));#2=SI_UNIT($,.RADIAN.);#3=GLOBAL_UNIT_ASSIGNED_CONTEXT('','',(#1,#2));";
    #[test]
    fn context_units_and_prefixes() {
        let units = read(MM);
        assert_eq!(units.label, "mm");
        assert_eq!(units.meters_per_model_unit, Some(0.001));
        assert_eq!(read(&MM.replace(".MILLI.", ".MICRO.")).label, "um");
        assert_eq!(
            read("#1=SI_UNIT(.MILLI.,.METRE.);").meters_per_model_unit,
            None
        );
    }
    #[test]
    fn conversion_graph_not_unit_name() {
        let input=format!("{}#4=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#1);#5=(CONVERSION_BASED_UNIT('name;with''quote',#4)LENGTH_UNIT()NAMED_UNIT(*));#6=GLOBAL_UNIT_ASSIGNED_CONTEXT('','',(#5));",MM.split("#3=").next().unwrap());
        assert_eq!(read(&input).label, "in");
        assert_eq!(read(&input.replace("25.4", "304.8")).label, "ft");
        assert_eq!(
            read(&input.replace("25.4", "-25.4")).meters_per_model_unit,
            None
        );
        assert_eq!(
            read(&input.replace(
                "#4=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#1)",
                "#4=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#5)"
            ))
            .meters_per_model_unit,
            None
        );
    }
    #[test]
    fn refuses_conflicting_missing_and_dangling_units() {
        let input=format!("{}#4=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT($,.METRE.));#5=GLOBAL_UNIT_ASSIGNED_CONTEXT('','',(#4));",MM);
        assert_eq!(read(&input).status, "ambiguous");
        assert_eq!(
            read(&format!(
                "{}#5=GLOBAL_UNIT_ASSIGNED_CONTEXT('','',(#99));",
                MM
            ))
            .status,
            "ambiguous"
        );
        assert_eq!(
            read(&MM.replace(".METRE.", ".SECOND.")).meters_per_model_unit,
            None
        );
    }
    #[test]
    fn override_records_conflict() {
        let u = read(MM).overridden("in");
        assert_eq!(u.source, "user_override");
        assert!(u.note.contains("conflicts"));
        assert_eq!(u.meters_per_model_unit, Some(0.0254));
    }
}
