//! Conservative native ASCII DXF linework reader. Unsupported content falls back
//! for the entire drawing; this path never silently drops unsupported entities.
use super::drawing::{DrawPart, Drawing, EntityMetadata, Primitive};
use serde_json::json;
use std::sync::Arc;
use std::{collections::HashMap, fs, path::Path, time::Instant};
type Tag<'a> = (i32, &'a str);
fn value<'a>(tags: &[Tag<'a>], code: i32) -> Option<&'a str> {
    tags.iter().find(|t| t.0 == code).map(|t| t.1)
}
fn number(tags: &[Tag<'_>], code: i32, default: f64) -> Option<f64> {
    let n = value(tags, code)
        .map(fast_float::parse::<f64, _>)
        .transpose()
        .ok()?
        .unwrap_or(default);
    n.is_finite().then_some(n)
}
fn integer(tags: &[Tag<'_>], code: i32, default: i32) -> Option<i32> {
    value(tags, code)
        .map(str::parse)
        .transpose()
        .ok()
        .map(|n| n.unwrap_or(default))
}
fn color(aci: i32) -> Option<[u8; 4]> {
    Some(match aci {
        1 => [255, 0, 0, 255],
        2 => [255, 255, 0, 255],
        3 => [0, 255, 0, 255],
        4 => [0, 255, 255, 255],
        5 => [0, 0, 255, 255],
        6 => [255, 0, 255, 255],
        7 => [255, 255, 255, 255],
        8 => [128, 128, 128, 255],
        9 => [192, 192, 192, 255],
        _ => return None,
    })
}
fn true_color(n: i32) -> [u8; 4] {
    [(n >> 16) as u8, (n >> 8) as u8, n as u8, 255]
}
fn append_arc(
    out: &mut Vec<[f32; 3]>,
    center: [f64; 2],
    radius: f64,
    start: f64,
    sweep: f64,
) -> Option<()> {
    if radius <= 0.0 || !radius.is_finite() || !sweep.is_finite() {
        return None;
    }
    let step = (2.0 * (1.0 - (0.01 / radius).min(1.0)).acos()).min(std::f64::consts::TAU / 128.0);
    let count = (sweep.abs() / step).ceil().max(1.0);
    if !count.is_finite() || count > 1_000_000.0 {
        return None;
    }
    let point = |angle: f64| {
        [
            (center[0] + radius * angle.cos()) as f32,
            (center[1] + radius * angle.sin()) as f32,
            0.0,
        ]
    };
    let mut last = point(start);
    for i in 1..=count as usize {
        let next = point(start + sweep * i as f64 / count);
        out.extend_from_slice(&[last, next]);
        last = next;
    }
    Some(())
}
fn poly_segment(out: &mut Vec<[f32; 3]>, a: [f64; 3], b: [f64; 3]) -> Option<()> {
    if a[0] == b[0] && a[1] == b[1] {
        return None;
    }
    if a[2] == 0.0 {
        out.extend_from_slice(&[
            [a[0] as f32, a[1] as f32, 0.0],
            [b[0] as f32, b[1] as f32, 0.0],
        ]);
        return Some(());
    }
    if a[2].abs() < 1e-8 {
        return None;
    }
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let chord = dx.hypot(dy);
    if chord == 0.0 {
        return None;
    }
    let offset = (1.0 - a[2] * a[2]) / (4.0 * a[2]);
    let center = [
        (a[0] + b[0]) * 0.5 - dy * offset,
        (a[1] + b[1]) * 0.5 + dx * offset,
    ];
    let radius = (a[0] - center[0]).hypot(a[1] - center[1]);
    append_arc(
        out,
        center,
        radius,
        (a[1] - center[1]).atan2(a[0] - center[0]),
        4.0 * a[2].atan(),
    )
}
#[derive(Clone)]
struct Layer {
    visible: bool,
    color: [u8; 4],
    name: Arc<str>,
}

pub fn load(path: &Path) -> Option<Drawing> {
    if std::env::var_os("VTX_CAD_FORCE_FALLBACK").is_some() {
        return None;
    }
    let start = Instant::now();
    let bytes = fs::read(path).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    if text.starts_with("AutoCAD Binary DXF") {
        return None;
    }
    let mut lines = text.lines();
    let mut tags = Vec::new();
    while let Some(code) = lines.next() {
        let val = lines.next()?;
        tags.push((code.trim().parse::<i32>().ok()?, val.trim()));
    }
    if tags.last() != Some(&(0, "EOF")) {
        return None;
    }
    let mut insunits = 0;
    let mut version = "unknown";
    for pair in tags.windows(2) {
        match pair[0] {
            (9, "$INSUNITS") => insunits = pair[1].1.parse().ok()?,
            (9, "$ACADVER") => version = pair[1].1,
            _ => (),
        }
    }
    let mut layers = HashMap::new();
    layers.insert(
        "0",
        Layer {
            visible: true,
            color: color(7)?,
            name: Arc::from("0"),
        },
    );
    let mut section = "";
    let mut records = Vec::new();
    let mut i = 0;
    let mut saw_entities = false;
    while i < tags.len() {
        if tags[i].0 != 0 {
            i += 1;
            continue;
        }
        let kind = tags[i].1;
        let begin = i + 1;
        i += 1;
        while i < tags.len() && tags[i].0 != 0 {
            i += 1;
        }
        let data = &tags[begin..i];
        if kind == "SECTION" {
            section = value(data, 2)?;
            if section == "ENTITIES" {
                saw_entities = true;
            }
            continue;
        }
        if kind == "ENDSEC" {
            section = "";
            continue;
        }
        if section == "TABLES" && kind == "LAYER" {
            if value(data, 6).is_some_and(|s| !s.eq_ignore_ascii_case("CONTINUOUS"))
                || value(data, 440).is_some()
            {
                return None;
            }
            let aci = integer(data, 62, 7)?;
            let rgba = if let Some(tc) = value(data, 420) {
                true_color(tc.parse().ok()?)
            } else {
                color(aci.abs())?
            };
            // Frozen/off layers follow the same visible-modelspace policy as the fallback.
            layers.insert(
                value(data, 2)?,
                Layer {
                    visible: aci >= 0 && (integer(data, 70, 0)? & 1) == 0,
                    color: rgba,
                    name: Arc::from(value(data, 2)?),
                },
            );
        }
        if section == "ENTITIES" {
            records.push((kind, data));
        }
    }
    if !saw_entities {
        return None;
    }
    // Reject unsupported visible content before allocating/tessellating any entities.
    for &(kind, data) in &records {
        if !matches!(kind, "LINE" | "CIRCLE" | "ARC" | "LWPOLYLINE") {
            let ignored = integer(data, 67, 0)? != 0
                || value(data, 410).is_some_and(|s| s != "Model")
                || integer(data, 60, 0)? != 0
                || !layers.get(value(data, 8).unwrap_or("0"))?.visible;
            if !ignored {
                return None;
            }
        }
    }
    let names: HashMap<_, Arc<str>> = ["LINE", "CIRCLE", "ARC", "LWPOLYLINE"]
        .iter()
        .map(|&s| (s, Arc::from(s)))
        .collect();
    let mut parts = Vec::with_capacity(records.len());
    for (kind, data) in records {
        if integer(data, 67, 0)? != 0
            || value(data, 410).is_some_and(|s| s != "Model")
            || integer(data, 60, 0)? != 0
        {
            continue;
        }
        let layer_name = value(data, 8).unwrap_or("0");
        let layer = layers.get(layer_name)?;
        if !layer.visible {
            continue;
        }
        // Non-default OCS/extrusion, widths, transparency and thickness need the full frontend.
        if number(data, 210, 0.0)? != 0.0
            || number(data, 220, 0.0)? != 0.0
            || number(data, 230, 1.0)? != 1.0
            || number(data, 39, 0.0)? != 0.0
            || value(data, 440).is_some()
        {
            return None;
        }
        if value(data, 6).is_some_and(|s| {
            !s.eq_ignore_ascii_case("BYLAYER") && !s.eq_ignore_ascii_case("CONTINUOUS")
        }) {
            return None;
        }

        let rgba = if let Some(tc) = value(data, 420) {
            true_color(tc.parse().ok()?)
        } else {
            match integer(data, 62, 256)? {
                256 => layer.color,
                n => color(n)?,
            }
        };
        let mut positions = Vec::new();
        let primitive = match kind {
            "LINE" => {
                positions.extend_from_slice(&[
                    [
                        number(data, 10, f64::NAN)? as f32,
                        number(data, 20, f64::NAN)? as f32,
                        0.0,
                    ],
                    [
                        number(data, 11, f64::NAN)? as f32,
                        number(data, 21, f64::NAN)? as f32,
                        0.0,
                    ],
                ]);
                if positions[0] == positions[1] {
                    return None;
                }
                Primitive::Lines
            }
            "POINT" => return None, // PDMODE can turn a point into a symbol.
            "CIRCLE" | "ARC" => {
                let center = [number(data, 10, f64::NAN)?, number(data, 20, f64::NAN)?];
                let radius = number(data, 40, f64::NAN)?;
                let (start, sweep) = if kind == "CIRCLE" {
                    (0.0, std::f64::consts::TAU)
                } else {
                    let start = number(data, 50, f64::NAN)?.to_radians();
                    let end = number(data, 51, f64::NAN)?.to_radians();
                    let sweep = (end - start).rem_euclid(std::f64::consts::TAU);
                    if sweep == 0.0 {
                        return None;
                    }
                    (start, sweep)
                };
                append_arc(&mut positions, center, radius, start, sweep)?;
                Primitive::Lines
            }
            "LWPOLYLINE" => {
                if number(data, 43, 0.0)? != 0.0
                    || data
                        .iter()
                        .any(|(c, v)| (*c == 40 || *c == 41) && v.parse::<f64>().ok() != Some(0.0))
                {
                    return None;
                }
                let mut vertices = Vec::new();
                let mut current = None;
                for &(code, val) in data {
                    match code {
                        10 => {
                            if let Some(p) = current.take() {
                                vertices.push(p);
                            }
                            current = Some([val.parse::<f64>().ok()?, f64::NAN, 0.0]);
                        }
                        20 => current.as_mut()?[1] = val.parse().ok()?,
                        42 => current.as_mut()?[2] = val.parse().ok()?,
                        _ => (),
                    }
                }
                if let Some(p) = current {
                    vertices.push(p);
                }
                if vertices.len() < 2
                    || vertices.iter().flatten().any(|v| !v.is_finite())
                    || integer(data, 90, -1)? != vertices.len() as i32
                {
                    return None;
                }
                for pair in vertices.windows(2) {
                    poly_segment(&mut positions, pair[0], pair[1])?;
                }
                if integer(data, 70, 0)? & 1 != 0 {
                    poly_segment(&mut positions, *vertices.last()?, vertices[0])?;
                }
                Primitive::Lines
            }
            // A single unsupported visible entity sends the entire drawing through ezdxf.
            _ => return None,
        };
        if positions.iter().flatten().any(|v| !v.is_finite()) {
            return None;
        }
        let handle = value(data, 5)?.to_string();
        parts.push(DrawPart {
            positions,
            kind: primitive,
            color: rgba,
            metadata: EntityMetadata {
                handle,
                entity_type: names.get(kind)?.clone(),
                layer: layer.name.clone(),
                block_path: Vec::new(),
                text: None,
            },
        });
    }
    if parts.is_empty() {
        return None;
    }
    let units = match insunits {
        1 => Some(("Inches", 0.0254)),
        2 => Some(("Feet", 0.3048)),
        4 => Some(("Millimeters", 0.001)),
        5 => Some(("Centimeters", 0.01)),
        6 => Some(("Meters", 1.0)),
        0 => None,
        _ => return None,
    };
    Some(Drawing {
        parts,
        source_info: json!({"kind":"drawing_2d","format":"dxf","projection":"modelspace projected onto XY; Z set to zero",
        "reader":"native_ascii_dxf","dxf_version":version,"insunits":insunits,"units_label":units.map_or("Unitless",|u|u.0),"meters_per_unit":units.map(|u|u.1),
        "conversion":null,"warnings":[],"native_parse_ms":start.elapsed().as_secs_f64()*1000.0,
        "limitations":["2D modelspace only; paperspace layouts are not imported","Native linework path; analytic curves tessellated to <=0.01 source-unit sagitta before float32 conversion","No whole-document DXF auditor on the native path; unsupported content falls back to ezdxf"]}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bulge_endpoints_and_sagitta() {
        let mut p = Vec::new();
        poly_segment(&mut p, [0., 0., 1.], [2., 0., 0.]).unwrap();
        assert!((p[0][0]).abs() < 1e-6);
        assert!((p.last().unwrap()[0] - 2.).abs() < 1e-6);
        for point in p {
            assert!(((point[0] - 1.).hypot(point[1]) - 1.).abs() < 1e-5);
        }
    }
    #[test]
    fn layers_handles_and_fallback() {
        let file = std::env::temp_dir().join(format!("vtx-fast-dxf-{}.dxf", std::process::id()));
        let text="0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nLINE\n5\nA1\n8\n0\n10\n1\n20\n2\n11\n3\n21\n4\n0\nENDSEC\n0\nEOF\n";
        fs::write(&file, text).unwrap();
        let drawing = load(&file).unwrap();
        assert_eq!(drawing.parts[0].positions, vec![[1., 2., 0.], [3., 4., 0.]]);
        assert_eq!(drawing.parts[0].metadata.handle, "A1");
        for unsupported in ["6\nDASHED\n", "39\n1\n", "210\n1\n", "440\n127\n"] {
            fs::write(
                &file,
                text.replace("10\n1", &format!("{}10\n1", unsupported)),
            )
            .unwrap();
            assert!(load(&file).is_none(), "unsupported: {}", unsupported);
        }
        fs::write(&file, text.replace("11\n3\n21\n4", "11\n1\n21\n2")).unwrap();
        assert!(load(&file).is_none());
        fs::write(&file, text.replace("LINE\n5", "TEXT\n5")).unwrap();
        assert!(load(&file).is_none());
        fs::remove_file(file).unwrap();
    }
}
