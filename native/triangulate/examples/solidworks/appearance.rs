//! Saved display appearances. Layout reference: cadmpeg's sldprt.md, section 8
//! (CC BY 4.0): https://github.com/cadmpeg/cadmpeg/blob/main/docs/formats/sldprt.md
//! Definitions alone are not assignments: require a part-property relation or a
//! direct instance in the metadata belonging to a validated display table.
use super::{word, Blocks};
use memchr::memmem;
use std::collections::HashMap;
use std::convert::TryInto;

pub type Color = [u8; 4];

/// Feature colors require a unique Keywords owner, a matching saved feature
/// identity/timestamp, and the enabled VisualStates attribute relation.
pub fn feature_colors(blocks: &Blocks<'_>) -> HashMap<u32, Color> {
    let parse = || -> Option<HashMap<u32, Color>> {
        let state = blocks.get("ThirdPtyStore/VisualStates")?.expand().ok()?;
        let scopes = classes(&state);
        if scopes
            .iter()
            .filter(|c| c.name == b"visStateKeyData_c")
            .count()
            != 1
        {
            return None;
        }
        let keywords = blocks.get("swXmlContents/KeyWords")?.expand().ok()?;
        let start = keywords.iter().position(|b| *b == b'<')?;
        let mut xml = quick_xml::Reader::from_reader(&keywords[start..]);
        let mut owners = HashMap::<u32, usize>::new();
        loop {
            match xml.read_event().ok()? {
                quick_xml::events::Event::Start(e) | quick_xml::events::Event::Empty(e) => {
                    if matches!(
                        e.name().as_ref(),
                        b"Keywords" | b"Configuration" | b"Dimension"
                    ) {
                        continue;
                    }
                    for a in e.attributes() {
                        let a = a.ok()?;
                        if a.key.as_ref() == b"id" {
                            if let Some(id) = std::str::from_utf8(&a.value)
                                .ok()
                                .and_then(|s| s.parse().ok())
                            {
                                *owners.entry(id).or_default() += 1;
                            }
                        }
                    }
                }
                quick_xml::events::Event::Eof => break,
                _ => (),
            }
        }
        let resolved: Vec<_> = blocks
            .iter()
            .filter(|(name, _)| name.ends_with("-ResolvedFeatures"))
            .collect();
        if resolved.len() != 1 {
            return None;
        }
        let resolved = resolved[0].1.expand().ok()?;
        let tag = tag(&state)?;
        let mut output = HashMap::<u32, Option<Color>>::new();
        for at in memmem::find_iter(&state, &tag) {
            if at < 30
                || state[at - 5] != 0x80
                || !(1..=254).contains(&state[at - 6])
                || state[at - 4..at] != [1, 0, 0, 0]
            {
                continue;
            }
            let record = at - 30;
            let version = word(&state[record..]);
            let source = word(&state[record + 4..]);
            let stamp = word(&state[record + 8..]);
            if !(9000..=40000).contains(&version)
                || version % 1000 != 0
                || source == 0
                || stamp == 0
                || stamp == u32::MAX
                || owners.get(&source) != Some(&1)
            {
                continue;
            }
            if state[record + 12..record + 24] != [1, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0] {
                continue;
            }
            if scopes
                .iter()
                .rev()
                .find(|c| c.end <= record)
                .is_none_or(|c| c.name != b"moCompFeature_c")
            {
                continue;
            }
            if memmem::find(&resolved, &state[record..record + 12]).is_none() {
                continue;
            }
            if let Some(color) = payload(&state[at + 2..]) {
                output
                    .entry(source)
                    .and_modify(|previous| {
                        if *previous != Some(color) {
                            *previous = None;
                        }
                    })
                    .or_insert(Some(color));
            }
        }
        Some(
            output
                .into_iter()
                .filter_map(|(source, color)| color.map(|color| (source, color)))
                .collect(),
        )
    };
    parse().unwrap_or_default()
}

pub fn feature_color(references: &[String], colors: &HashMap<u32, Color>) -> Option<Color> {
    let mut sources = references
        .iter()
        .map(|r| r.split(',').nth(1)?.parse::<u32>().ok());
    let first = sources.next()??;
    if sources.any(|s| s != Some(first)) {
        return None;
    }
    colors.get(&first).copied()
}

pub fn configuration_matches(blocks: &Blocks<'_>, configuration: &str) -> bool {
    let key = || -> Option<String> {
        let bytes = blocks
            .get("ThirdPtyStore/VisualStates")
            .or_else(|| blocks.get("ThirdPtyStore/VisualStates__ZLB"))?
            .expand()
            .ok()?;
        let classes = classes(&bytes);
        let keys: Vec<_> = classes
            .iter()
            .enumerate()
            .filter(|(_, class)| class.name == b"visStateKeyData_c")
            .collect();
        if keys.len() != 1 {
            return None;
        }
        let (index, class) = keys[0];
        let end = classes
            .get(index + 1)
            .map_or(bytes.len(), |next| next.start);
        let data = &bytes[class.end..end];
        let at = memmem::find(data, b"\xff\xfe\xff")?;
        string(&mut &data[at..])
    };
    key().is_some_and(|key| {
        key.starts_with(&format!("{configuration}_"))
            || key.starts_with(&format!("<{configuration}>_"))
    })
}

pub struct SourceAppearance {
    pub color: Option<Color>,
    pub surfaces: HashMap<String, Color>,
    pub visible_mesh: Option<super::Mesh>,
}

impl SourceAppearance {
    pub fn from_mesh(mesh: &super::Mesh) -> Self {
        let mut surfaces = HashMap::new();
        let painted: HashMap<_, _> = mesh
            .face_colors
            .iter()
            .map(|(range, color)| (range.start, (range.end, *color)))
            .collect();
        for (range, references) in &mesh.face_references {
            let color = painted
                .get(&range.start)
                .filter(|(end, _)| *end == range.end)
                .map(|(_, color)| *color)
                .or(mesh.color);
            for reference in references {
                // One persistent reference must identify exactly one table in
                // the source. Duplicated/conflicting owners remain unresolved.
                surfaces
                    .entry(reference.clone())
                    .and_modify(|value| *value = None)
                    .or_insert(color);
            }
        }
        Self {
            color: mesh.color,
            visible_mesh: Some(mesh.clone()),
            surfaces: surfaces
                .into_iter()
                .filter_map(|(key, value)| value.map(|color| (key, color)))
                .collect(),
        }
    }

    pub fn matched_color(&self, references: &[String]) -> Option<Color> {
        let first = *self.surfaces.get(references.first()?)?;
        references
            .iter()
            .all(|reference| self.surfaces.get(reference) == Some(&first))
            .then_some(first)
    }
}

pub fn surface_references(bytes: &[u8]) -> Vec<String> {
    let mut references = Vec::new();
    for at in memmem::find_iter(bytes, b"\xff\xfe\xff") {
        let Some(text) = string(&mut &bytes[at..]) else {
            continue;
        };
        for line in text.lines().map(str::trim) {
            if !line.ends_with(',') {
                continue;
            }
            let fields: Vec<_> = line
                .trim_end_matches(',')
                .split(',')
                .map(str::trim)
                .collect();
            if fields.len() < 3
                || !fields[0].starts_with("mo")
                || !fields[0].ends_with("SurfIdRep_c")
            {
                continue;
            }
            let numbers: Option<Vec<u32>> = fields[1..]
                .iter()
                .map(|field| {
                    field
                        .parse::<u32>()
                        .ok()
                        .or_else(|| field.parse::<i32>().ok().map(|n| n as u32))
                })
                .collect();
            let Some(numbers) = numbers else {
                continue;
            };
            let key = format!(
                "{},{}",
                fields[0],
                numbers
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            );
            if !references.contains(&key) {
                references.push(key);
            }
        }
    }
    references
}

struct Class<'a> {
    start: usize,
    end: usize,
    name: &'a [u8],
}

fn classes(bytes: &[u8]) -> Vec<Class<'_>> {
    memmem::find_iter(bytes, b"\xff\xff")
        .filter_map(|start| {
            let header = bytes.get(start..start + 6)?;
            let size = u16::from_le_bytes([header[4], header[5]]) as usize;
            if !(3..=128).contains(&size) {
                return None;
            }
            let end = start + 6 + size;
            let name = bytes.get(start + 6..end)?;
            (name.ends_with(b"_c") && name.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_'))
                .then_some(Class { start, end, name })
        })
        .collect()
}

pub(super) fn string(bytes: &mut &[u8]) -> Option<String> {
    *bytes = bytes.strip_prefix(b"\xff\xfe\xff")?;
    let (&size, rest) = bytes.split_first()?;
    *bytes = rest;
    let size = if size == 255 {
        let n = bytes.get(..2)?;
        *bytes = &bytes[2..];
        usize::from(u16::from_le_bytes([n[0], n[1]]))
    } else {
        usize::from(size)
    };
    if size > 4096 {
        return None;
    }
    let (text, rest) = bytes.split_at_checked(size * 2)?;
    *bytes = rest;
    String::from_utf16(
        &text
            .chunks_exact(2)
            .map(|w| u16::from_le_bytes([w[0], w[1]]))
            .collect::<Vec<_>>(),
    )
    .ok()
}

/// Match imported-body appearance IDs through owned XT body names, then use
/// validated DisplayLists body groups. Body order alone never binds a color.
pub fn imported_body_colors(
    blocks: &Blocks<'_>,
    display: &[u8],
    tables: &[(usize, usize)],
) -> Vec<Option<Color>> {
    let empty = || vec![None; tables.len()];
    let parse = || -> Option<Vec<Option<Color>>> {
        let state = blocks.get("ThirdPtyStore/VisualStates")?.expand().ok()?;
        let scopes = classes(&state);
        if scopes
            .iter()
            .filter(|c| c.name == b"visStateKeyData_c")
            .count()
            != 1
        {
            return None;
        }
        let body = scopes.iter().find(|c| c.name == b"moCompSolidBody_c")?;
        let identities = super::bodies::imported_identities(blocks);
        if identities.is_empty() {
            return None;
        }
        let id_set: std::collections::HashSet<_> = identities.values().copied().collect();
        let prefixes: Vec<_> = (body.end..state.len().saturating_sub(16))
            .filter_map(|p| {
                let bytes = state.get(p..p + 17)?;
                if bytes[0] == 0 || bytes[1] != 0x80 || bytes[2..9] != [0; 7] {
                    return None;
                }
                let id = word(&bytes[9..]);
                (id == word(&bytes[13..]) && id_set.contains(&id)).then_some((p, id))
            })
            .collect();
        if prefixes.first()?.0 != body.end {
            return None;
        }
        let mut reference = None;
        for (p, _) in prefixes.iter().skip(1) {
            let bytes = state.get(p - 2..*p)?;
            if bytes[0] == 0 || bytes[1] != 0x80 {
                return None;
            }
            if reference.is_some_and(|r| r != bytes) {
                return None;
            }
            reference = Some(bytes);
        }
        let tag = tag(&state)?;
        let mut colors = HashMap::<u32, Option<Color>>::new();
        let mut contexts = Vec::new();
        let mut opaque = Vec::new();
        if let Some((i, c)) = scopes
            .iter()
            .enumerate()
            .find(|(_, c)| c.name == b"moVisualProperties_c")
        {
            opaque.push(c.end..scopes.get(i + 1).map_or(state.len(), |c| c.start));
        }
        for (i, (p, id)) in prefixes.iter().enumerate() {
            let limit = prefixes.get(i + 1).map_or(state.len(), |next| next.0 - 2);
            let end = scopes
                .iter()
                .find(|c| {
                    c.start > *p
                        && c.start < limit
                        && matches!(
                            c.name,
                            b"moCompFace_c" | b"moCompFeature_c" | b"moUnitComponent_c"
                        )
                })
                .map_or(limit, |c| c.start);
            let region = &state[*p..end];
            let mut found = Vec::new();
            for at in memmem::find_iter(region, &tag) {
                if at >= 6
                    && region[at - 5] == 0x80
                    && (1..=254).contains(&region[at - 6])
                    && region[at - 4..at] == [1, 0, 0, 0]
                {
                    if let Some(c) = payload(&region[at + 2..]) {
                        found.push(c);
                        opaque.push(p + at + 2..end);
                    }
                }
            }
            if found.len() == 1 {
                colors
                    .entry(*id)
                    .and_modify(|v| *v = None)
                    .or_insert(Some(found[0]));
            }
            contexts.push((*id, *p..end));
        }
        if let Some(objects) = archive_body_attributes(&state, &scopes, &opaque, tag) {
            for (id, range) in contexts {
                let region = &state[range];
                if region.len() < 16 {
                    continue;
                }
                let tail = &region[region.len() - 16..];
                if tail[..12] != [1, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0] {
                    continue;
                }
                let reference = u16::from_le_bytes([tail[12], tail[13]]);
                if let Some(color) = objects.get(&reference) {
                    colors
                        .entry(id)
                        .and_modify(|v| {
                            if *v != Some(*color) {
                                *v = None;
                            }
                        })
                        .or_insert(Some(*color));
                }
            }
        }
        let groups = display_body_groups(display, tables)?;
        let mut output = empty();
        for (range, name) in groups {
            if let Some(color) = identities
                .get(&name)
                .and_then(|id| colors.get(id))
                .and_then(|c| *c)
            {
                for i in range {
                    output[i] = Some(color);
                }
            }
        }
        Some(output)
    };
    parse().unwrap_or_else(empty)
}

// MFC archive IDs count class declarations and object constructors. Material
// numeric payloads and framed strings are opaque: their bytes are not pointers.
// This narrow census refuses unknown object classes instead of guessing IDs.
fn archive_body_attributes(
    bytes: &[u8],
    scopes: &[Class<'_>],
    opaque: &[std::ops::Range<usize>],
    visual_tag: [u8; 2],
) -> Option<HashMap<u16, Color>> {
    let mut class_map = HashMap::<u16, &[u8]>::new();
    let mut objects = HashMap::<u16, (&[u8], usize)>::new();
    let mut index = 1u16;
    let mut p = scopes.first()?.start;
    while p + 2 <= bytes.len() {
        if let Some(range) = opaque.iter().find(|r| r.contains(&p)) {
            p = range.end;
            continue;
        }
        if bytes[p..].starts_with(b"\xff\xfe\xff") {
            let mut tail = &bytes[p..];
            string(&mut tail)?;
            p = bytes.len() - tail.len();
            continue;
        }
        if let Some(class) = scopes.iter().find(|c| c.start == p) {
            if !matches!(
                class.name,
                b"visStateKeyData_c"
                    | b"moCompHolder_c"
                    | b"moUnitComponent_c"
                    | b"visStateVisualPropertiesAttribute_c"
                    | b"moVisualProperties_c"
                    | b"visStateVisualStateAttribute_c"
                    | b"moCompSolidBody_c"
                    | b"moFaceRef_c"
                    | b"moSimpleSurfIdRep_c"
                    | b"moFR_c"
                    | b"moExtObject_c"
                    | b"moCStringHandle_c"
                    | b"moCompFeature_c"
            ) {
                return None;
            }
            class_map.insert(index, class.name);
            index = index.checked_add(1)?;
            objects.insert(index, (class.name, class.end));
            index = index.checked_add(1)?;
            p = class.end;
            continue;
        }
        let token = u16::from_le_bytes([bytes[p], bytes[p + 1]]);
        if token & 0x8000 != 0 {
            if let Some(kind) = class_map.get(&(token & 0x7fff)) {
                objects.insert(index, (*kind, p + 2));
                index = index.checked_add(1)?;
                p += 2;
                continue;
            }
        }
        p += 1;
    }
    let visual_class = u16::from_le_bytes(visual_tag) & 0x7fff;
    if class_map.get(&visual_class).copied() != Some(b"moVisualProperties_c".as_slice()) {
        return None;
    }
    let mut result = HashMap::new();
    for (id, (kind, start)) in &objects {
        if *kind != b"visStateVisualPropertiesAttribute_c"
            || bytes.get(*start..start + 4) != Some(&[1, 0, 0, 0])
        {
            continue;
        }
        let p = start + 4;
        let token = u16::from_le_bytes(bytes.get(p..p + 2)?.try_into().ok()?);
        let color = if token == u16::from_le_bytes(visual_tag) {
            payload(&bytes[p + 2..])
        } else if token < 0x8000 {
            objects
                .get(&token)
                .filter(|(kind, _)| *kind == b"moVisualProperties_c")
                .and_then(|(_, p)| payload(&bytes[*p..]))
        } else {
            scopes
                .iter()
                .find(|c| c.start == p && c.name == b"moVisualProperties_c")
                .and_then(|c| payload(&bytes[c.end..]))
        };
        if let Some(color) = color {
            result.insert(*id, color);
        }
    }
    Some(result)
}

fn display_body_groups(
    data: &[u8],
    tables: &[(usize, usize)],
) -> Option<Vec<(std::ops::Range<usize>, String)>> {
    let scopes = classes(data);
    let body = scopes.iter().find(|c| c.name == b"uoTempBodyTessData_c")?;
    let property = scopes.iter().find(|c| c.name == b"uoBodyPropInfo_c")?;
    let count = data.get(body.end..body.end + 2)?;
    let mut count = usize::from(u16::from_le_bytes(count.try_into().ok()?));
    let mut first = 0usize;
    let mut output = Vec::new();
    let mut tags = None;
    while first < tables.len() {
        if count == 0 || count > tables.len() - first {
            return None;
        }
        let end = first + count;
        let boundary = if end < tables.len() {
            let start = tables[end].0;
            let mut candidates = Vec::new();
            for width in [8usize, 40] {
                let Some(at) = start.checked_sub(width + 6) else {
                    continue;
                };
                let bytes = data.get(at..start)?;
                if bytes[0] == 0 || bytes[1] != 0x80 || bytes[4] == 0 || bytes[5] != 0x80 {
                    continue;
                }
                let n = usize::from(u16::from_le_bytes(bytes[2..4].try_into().ok()?));
                if n == 0 || n > tables.len() - end {
                    continue;
                }
                let strips = word(data.get(start + 12..start + 16)?) as usize;
                let lengths = data.get(start + 16..start + 16 + strips * 4)?;
                let triangles = lengths
                    .chunks_exact(4)
                    .try_fold(0u32, |total, b| total.checked_add(word(b).checked_sub(2)?))?;
                if word(&bytes[6..]) != triangles || word(&bytes[10..]) as usize != strips {
                    continue;
                }
                if width == 40
                    && (word(&bytes[14..]) != 1
                        || bytes[18..26] != [0; 8]
                        || word(&bytes[26..]) == 0
                        || bytes[30..46] != [0; 16])
                {
                    continue;
                }
                let pair = ([bytes[0], bytes[1]], [bytes[4], bytes[5]]);
                if tags.is_some_and(|old| old != pair) {
                    continue;
                }
                candidates.push((at, n, pair));
            }
            if candidates.len() != 1 {
                return None;
            }
            let (at, n, pair) = candidates[0];
            count = n;
            tags = Some(pair);
            at
        } else {
            data.len()
        };
        let start = tables[end - 1].1;
        let tail = data.get(start..boundary)?;
        let mut names = Vec::new();
        for at in memmem::find_iter(tail, b"\xff\xfe\xff") {
            if at < 22 {
                continue;
            }
            let p = at - 22;
            let prefix = &tail[p..at];
            if word(&prefix[2..]) != 1
                || word(&prefix[6..]) != 6
                || word(&prefix[10..]) != 1
                || prefix[14..22] != [0; 8]
            {
                continue;
            }
            if first == 0 {
                if start + p != property.end {
                    continue;
                }
            } else if p < 2 || tail[p - 1] != 0x80 || tail[p - 2] == 0 {
                continue;
            }
            let mut rest = &tail[at..];
            let Some(name) = string(&mut rest) else {
                continue;
            };
            if !name.is_empty() && rest == [0; 4] {
                names.push(name);
            }
        }
        // Unstyled bodies can omit the named-property form. Their table range
        // stays intact and inherits its normal part/face appearance.
        if names.len() == 1 {
            output.push((first..end, names.remove(0)));
        }
        first = end;
    }
    let mut unique = std::collections::HashSet::new();
    if !output.iter().all(|(_, name)| unique.insert(name)) {
        return None;
    }
    Some(output)
}

fn payload(bytes: &[u8]) -> Option<Color> {
    let head = bytes.get(..12)?;
    if head[3] != 0 || head[11] != 0 {
        return None;
    }
    let mut rest = &bytes[12..];
    string(&mut rest)?;
    string(&mut rest)?;
    let optical = rest.get(..48)?;
    if !optical
        .chunks_exact(8)
        .all(|v| f64::from_le_bytes(v.try_into().unwrap()).is_finite())
    {
        return None;
    }
    if word(rest.get(48..52)?) > 1 {
        return None;
    }
    let transparency = f64::from_le_bytes(optical[32..40].try_into().unwrap());
    if !(0.0..=1.0).contains(&transparency) {
        return None;
    }
    let rgb = appearance_rgb(rest).unwrap_or([head[0], head[1], head[2]]);
    Some([
        rgb[0],
        rgb[1],
        rgb[2],
        ((1.0 - transparency) * 255.0).round() as u8,
    ])
}

// Named render appearances can override the legacy packed RGB. Validate the
// complete framed appearance run before using its explicit RGB field.
fn appearance_rgb(optical: &[u8]) -> Option<[u8; 3]> {
    let flag = word(optical.get(48..52)?);
    let mut rest = optical.get(52..)?;
    match flag {
        0 => (),
        1 => {
            rest = rest.get(104..)?;
            string(&mut rest)?;
            rest = rest.get(22..)?;
        }
        _ => return None,
    }
    for cells in [5, 6] {
        let parse = || -> Option<[u8; 3]> {
            let mut tail = rest.get(cells * 4..)?;
            if !string(&mut tail)?.is_empty() {
                return None;
            }
            tail = tail.get(16..)?;
            if string(&mut tail)?.is_empty() {
                return None;
            }
            let properties = tail.get(..81)?;
            tail = tail.get(81..)?;
            let path = string(&mut tail)?;
            tail = tail.get(12..)?;
            string(&mut tail)?;
            if !path.to_ascii_lowercase().ends_with(".p2m")
                || !matches!(properties[0], 2 | 3)
                || properties[4] != 0
            {
                return None;
            }
            let scale = f32::from_le_bytes(properties[5..9].try_into().ok()?);
            if !scale.is_finite() || !(0.0..=1.0).contains(&scale) {
                return None;
            }
            Some([properties[1], properties[2], properties[3]])
        };
        if let Some(rgb) = parse() {
            return Some(rgb);
        }
    }
    None
}

/// A sole solid-body appearance can bind without an ambiguous body-ID join.
/// Multi-body files retain their explicitly scoped display-table assignments.
pub fn sole_body_color(
    blocks: &Blocks<'_>,
    display: &[u8],
    tables: &[(usize, usize)],
) -> Option<Color> {
    let display_scopes = classes(display);
    let bodies: Vec<_> = display_scopes
        .iter()
        .filter(|c| c.name == b"uoTempBodyTessData_c")
        .collect();
    let props: Vec<_> = display_scopes
        .iter()
        .filter(|c| c.name == b"uoBodyPropInfo_c")
        .collect();
    if bodies.len() != 1 || props.len() != 1 || tables.is_empty() {
        return None;
    }
    let count = display.get(bodies[0].end..bodies[0].end + 2)?;
    if usize::from(u16::from_le_bytes(count.try_into().ok()?)) != tables.len()
        || tables.iter().any(|(_, end)| *end > props[0].start)
    {
        return None;
    }
    let state = blocks.get("ThirdPtyStore/VisualStates")?.expand().ok()?;
    let scopes = classes(&state);
    if scopes
        .iter()
        .filter(|c| c.name == b"visStateKeyData_c")
        .count()
        != 1
    {
        return None;
    }
    let bodies: Vec<_> = scopes
        .iter()
        .filter(|c| c.name == b"moCompSolidBody_c")
        .collect();
    if bodies.len() != 1 {
        return None;
    }
    let start = bodies[0].end;
    let prefix = state.get(start..start + 17)?;
    if prefix[0] == 0
        || prefix[1] != 0x80
        || prefix[2..9] != [0; 7]
        || word(&prefix[9..]) == 0
        || word(&prefix[9..]) != word(&prefix[13..])
    {
        return None;
    }
    let end = scopes
        .iter()
        .find(|c| {
            c.start > start
                && matches!(
                    c.name,
                    b"moCompFace_c" | b"moCompFeature_c" | b"moUnitComponent_c"
                )
        })
        .map_or(state.len(), |c| c.start);
    let region = &state[start..end];
    let tag = tag(&state)?;
    let mut colors = Vec::new();
    for at in memmem::find_iter(region, &tag) {
        if at >= 6
            && region[at - 5] == 0x80
            && (1..=254).contains(&region[at - 6])
            && region[at - 4..at] == [1, 0, 0, 0]
        {
            if let Some(color) = payload(&region[at + 2..]) {
                colors.push(color);
            }
        }
    }
    (colors.len() == 1).then(|| colors[0])
}

/// A supported assembly context record carries an exact framed component name
/// followed by its visual-properties attribute. Only one saved display state
/// and one occurrence of each component name may bind an override.
pub fn component_overrides(
    blocks: &Blocks<'_>,
    names: &[String],
    state_name: &str,
) -> HashMap<String, Color> {
    let parse = || -> Option<HashMap<String, Color>> {
        let bytes = blocks.get("ThirdPtyStore/VisualStates")?.expand().ok()?;
        let scopes = classes(&bytes);
        let keys: Vec<_> = scopes
            .iter()
            .enumerate()
            .filter(|(_, c)| c.name == b"visStateKeyData_c")
            .collect();
        if keys.len() != 1 {
            return None;
        }
        let (i, key) = keys[0];
        let key_end = scopes.get(i + 1).map_or(bytes.len(), |c| c.start);
        let key_bytes = &bytes[key.end..key_end];
        let at = memmem::find(key_bytes, b"\xff\xfe\xff")?;
        if string(&mut &key_bytes[at..])? != state_name {
            return None;
        }
        let tag = tag(&bytes);
        let mut frames = Vec::new();
        for at in memmem::find_iter(&bytes, b"\xff\xfe\xff") {
            let mut tail = &bytes[at..];
            if let Some(text) = string(&mut tail) {
                if names.contains(&text) {
                    frames.push((at, bytes.len() - tail.len(), text));
                }
            }
        }
        let mut output = HashMap::new();
        for (index, (_, end, name)) in frames.iter().enumerate() {
            if frames.iter().filter(|(_, _, other)| other == name).count() != 1 {
                continue;
            }
            let limit = frames.get(index + 1).map_or(bytes.len(), |next| next.0);
            let region = &bytes[*end..limit];
            let classes = classes(region);
            // Nested feature/body definitions cannot become component overrides.
            let stop = classes
                .iter()
                .find(|c| {
                    matches!(
                        c.name,
                        b"moCompFeature_c"
                            | b"moUnitComponent_c"
                            | b"moCompSolidBody_c"
                            | b"moCompFace_c"
                    )
                })
                .map_or(region.len(), |c| c.start);
            let region = &region[..stop];
            let mut colors = Vec::new();
            for group in classes.windows(2) {
                if group[0].name == b"visStateVisualPropertiesAttribute_c"
                    && group[1].name == b"moVisualProperties_c"
                    && region.get(group[0].end..group[1].start) == Some(&[1, 0, 0, 0][..])
                {
                    if let Some(color) = region.get(group[1].end..).and_then(payload) {
                        colors.push(color);
                    }
                }
            }
            if let Some(tag) = tag {
                for at in memmem::find_iter(region, &tag) {
                    // Reused attribute class tag, enabled flag, then reused
                    // visual-property class tag. Neither float bytes nor bare
                    // definitions establish this relation.
                    if at < 6
                        || region[at - 5] != 0x80
                        || region[at - 6] == 0
                        || region[at - 6] == 255
                        || region[at - 4..at] != [1, 0, 0, 0]
                    {
                        continue;
                    }
                    if let Some(color) = payload(&region[at + 2..]) {
                        colors.push(color);
                    }
                }
            }
            if colors.len() == 1 {
                output.insert(name.clone(), colors[0]);
            }
        }
        Some(output)
    };
    parse().unwrap_or_default()
}

/// Only a unique tag with a valid visual-property payload is accepted. Archive
/// indices vary between files; hard-coding 0x8032 also misreads float data.
pub fn tag(bytes: &[u8]) -> Option<[u8; 2]> {
    let decl = classes(bytes)
        .into_iter()
        .find(|c| c.name == b"moVisualProperties_c")?;
    let mut found = None;
    for high in memchr::memchr_iter(0x80, &bytes[decl.end..]) {
        let high = decl.end + high;
        if high == 0 || !(1..=254).contains(&bytes[high - 1]) {
            continue;
        }
        if payload(&bytes[high + 1..]).is_none() {
            continue;
        }
        let candidate = [bytes[high - 1], 0x80];
        if found.is_some_and(|previous| previous != candidate) {
            return None;
        }
        found = Some(candidate);
    }
    found
}

pub fn direct(bytes: &[u8], tag: Option<[u8; 2]>) -> Option<Color> {
    let tag = tag?;
    let mut found = None;
    for at in memmem::find_iter(bytes, &tag) {
        if let Some(color) = payload(&bytes[at + 2..]) {
            if found.is_some_and(|previous| previous != color) {
                return None;
            }
            found = Some(color);
        }
    }
    found
}

#[cfg(test)]
pub fn display_colors(bytes: &[u8], tables: &[(usize, usize)]) -> Vec<Option<Color>> {
    display_colors_with_body(bytes, tables, None)
}

pub fn display_colors_with_body(
    bytes: &[u8],
    tables: &[(usize, usize)],
    body_override: Option<Color>,
) -> Vec<Option<Color>> {
    let tag = tag(bytes);
    let scopes = classes(bytes);
    let mut colors = vec![None; tables.len()];
    let mut previous_body_end = 0;
    for (i, scope) in scopes.iter().enumerate() {
        let end = scopes.get(i + 1).map_or(bytes.len(), |next| next.start);
        if scope.name == b"uoBodyPropInfo_c" {
            // A later body reuses class tags, so its face tables may appear
            // before the next class declaration. They cannot join this scope.
            let end = tables
                .iter()
                .find(|(start, _)| *start >= scope.end)
                .map_or(end, |(start, _)| end.min(*start));
            let region = &bytes[scope.end..end];
            let first = tag
                .and_then(|tag| region.strip_prefix(&tag))
                .and_then(payload);
            if let Some(color) = body_override.or(first).or_else(|| direct(region, tag)) {
                for (index, &(start, finish)) in tables.iter().enumerate() {
                    if start >= previous_body_end && finish <= scope.start {
                        colors[index] = Some(color);
                    }
                }
            }
            previous_body_end = end;
        }
    }
    // Face assignments take precedence over body defaults. The next class
    // declaration ends a face-property scope even when more tables follow it.
    for (index, &(start, finish)) in tables.iter().enumerate() {
        let Some((i, scope)) = scopes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, c)| c.end <= start)
        else {
            continue;
        };
        if scope.name != b"uoTempFaceTessData_c" {
            continue;
        }
        let end = scopes
            .get(i + 1)
            .map_or(bytes.len(), |next| next.start)
            .min(tables.get(index + 1).map_or(bytes.len(), |next| next.0));
        if let Some(color) = bytes
            .get(finish..end)
            .and_then(|metadata| direct(metadata, tag))
        {
            colors[index] = Some(color);
        }
    }
    colors
}

fn part_state(bytes: &[u8], configuration: Option<&str>) -> Option<Color> {
    let classes = classes(bytes);
    let keys: Vec<_> = classes
        .iter()
        .enumerate()
        .filter(|(_, c)| c.name == b"visStateKeyData_c")
        .collect();
    // Multiple display states need an explicit selection, not a first/last win.
    if keys.len() != 1 {
        return None;
    }
    let (key_index, key) = keys[0];
    let end = classes.get(key_index + 1).map_or(bytes.len(), |c| c.start);
    let key_bytes = &bytes[key.end..end];
    let at = memmem::find(key_bytes, b"\xff\xfe\xff")?;
    let key = string(&mut &key_bytes[at..])?;
    if let Some(configuration) = configuration {
        let prefix = format!("{configuration}_");
        let quoted = format!("<{configuration}>_");
        if !key.starts_with(&prefix) && !key.starts_with(&quoted) {
            return None;
        }
    }
    let mut found = None;
    for (i, definition) in classes
        .iter()
        .enumerate()
        .filter(|(_, c)| c.name == b"moVisualProperties_c")
    {
        let names: Vec<_> = classes[..i].iter().rev().take(4).map(|c| c.name).collect();
        let attribute = if names.starts_with(&[
            b"visStateVisualPropertiesAttribute_c".as_slice(),
            b"moUnitComponent_c",
            b"moCompHolder_c",
        ]) {
            i - 1
        } else if names.as_slice()
            == [
                b"visStateVisualStateAttribute_c".as_slice(),
                b"visStateVisualPropertiesAttribute_c",
                b"moUnitComponent_c",
                b"moCompHolder_c",
            ]
        {
            i - 2
        } else if names.as_slice()
            == [
                b"moCompHolder_c".as_slice(),
                b"visStateVisualStateAttribute_c",
                b"visStateVisualPropertiesAttribute_c",
                b"moUnitComponent_c",
            ]
        {
            i - 3
        } else {
            continue;
        };
        let relation = &bytes[classes[attribute].end..classes[attribute + 1].start];
        if relation != [1, 0, 0, 0] && relation != [1, 0, 0, 0, 0, 0] {
            continue;
        }
        let color = payload(&bytes[definition.end..])?;
        if found.is_some_and(|previous| previous != color) {
            return None;
        }
        found = Some(color);
    }
    found
}

pub fn configured_part_color(blocks: &Blocks<'_>, configuration: &str) -> Option<Color> {
    let state = blocks
        .get("ThirdPtyStore/VisualStates")
        .or_else(|| blocks.get("ThirdPtyStore/VisualStates__ZLB"))?
        .expand()
        .ok()?;
    part_state(&state, Some(configuration))
}

pub fn part_color(blocks: &Blocks<'_>, configuration: Option<&str>) -> Option<Color> {
    let state = blocks
        .get("ThirdPtyStore/VisualStates")
        .or_else(|| blocks.get("ThirdPtyStore/VisualStates__ZLB"))?
        .expand()
        .ok()?;
    let color = part_state(&state, configuration)?;
    // A standalone part uses its saved display selection. Don't apply an
    // inactive state whose default disagrees with that display header.
    let display = blocks.get("Contents/DisplayLists")?.expand().ok()?;
    let decl = classes(&display)
        .into_iter()
        .find(|c| c.name == b"moVisualProperties_c")?;
    (payload(&display[decl.end..])? == color).then_some(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_faces_keep_source_overrides_and_reject_ambiguous_owners() {
        let mut mesh = super::super::Mesh::default();
        mesh.color = Some([224, 223, 219, 255]);
        mesh.face_colors = vec![(0..2, [192, 192, 192, 255])];
        mesh.face_references = vec![(0..2, vec!["case".into()]), (2..4, vec!["screw".into()])];
        let source = SourceAppearance::from_mesh(&mesh);
        assert_eq!(
            source.matched_color(&["case".into()]),
            Some([192, 192, 192, 255])
        );
        assert_eq!(source.matched_color(&["screw".into()]), mesh.color);
        assert_eq!(
            source.matched_color(&["case".into(), "missing".into()]),
            None
        );
        assert_eq!(source.matched_color(&["case".into(), "screw".into()]), None);
        mesh.face_references.push((4..6, vec!["case".into()]));
        assert_eq!(
            SourceAppearance::from_mesh(&mesh).matched_color(&["case".into()]),
            None
        );
    }

    #[test]
    fn persistent_surface_frames_are_canonical_and_not_guessed() {
        let mut bytes = Vec::new();
        for text in [
            "moSimpleSurfIdRep_c,38,294,",
            "moSimpleSurfIdRep_c,38,294,",
            "moSimpleSurfIdRep_c,38,295,-1,",
            "moSimpleSurfIdRep_c,38,296,opaque,",
        ] {
            let words: Vec<_> = text.encode_utf16().collect();
            bytes.extend_from_slice(b"\xff\xfe\xff");
            bytes.push(words.len() as u8);
            for word in words {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
        }
        assert_eq!(
            surface_references(&bytes),
            vec![
                "moSimpleSurfIdRep_c,38,294",
                "moSimpleSurfIdRep_c,38,295,4294967295"
            ]
        );
    }

    fn definition(color: Color) -> Vec<u8> {
        let mut bytes = vec![
            color[0], color[1], color[2], 0, 0, 0, 0, 0, 192, 192, 192, 0,
        ];
        bytes.extend_from_slice(b"\xff\xfe\xff\0\xff\xfe\xff\0");
        bytes.extend_from_slice(&[0; 52]);
        bytes[52..60].copy_from_slice(&(1.0 - f64::from(color[3]) / 255.0).to_le_bytes());
        bytes
    }

    fn framed(text: &str) -> Vec<u8> {
        let words: Vec<_> = text.encode_utf16().collect();
        let mut bytes = b"\xff\xfe\xff".to_vec();
        bytes.push(words.len() as u8);
        for word in words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn component_contexts_keep_transparency_and_do_not_bleed_between_names() {
        let mut bytes = class(b"visStateKeyData_c");
        bytes.extend(framed("Display State-1"));
        bytes.extend(class(b"moContextChainComponent_c"));
        bytes.extend(framed("heatsink-1"));
        bytes.extend(class(b"visStateVisualPropertiesAttribute_c"));
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend(class(b"moVisualProperties_c"));
        bytes.extend(definition([221, 232, 255, 64]));
        bytes.extend(framed("unchanged-1"));
        bytes.extend_from_slice(&[0; 20]);
        bytes.extend(framed("housing-1"));
        bytes.extend_from_slice(&[0x11, 0x80, 1, 0, 0, 0, 0x13, 0x80]);
        bytes.extend(definition([202, 209, 238, 64]));
        let mut blocks = Blocks::new();
        blocks.insert(
            "ThirdPtyStore/VisualStates".into(),
            super::super::Block {
                compressed: &bytes,
                expanded: bytes.len(),
                crc: crc32fast::hash(&bytes),
            },
        );
        let names = vec![
            "heatsink-1".into(),
            "housing-1".into(),
            "unchanged-1".into(),
        ];
        let colors = component_overrides(&blocks, &names, "Display State-1");
        assert_eq!(colors.len(), 2);
        assert_eq!(colors["heatsink-1"], [221, 232, 255, 64]);
        assert_eq!(colors["housing-1"], [202, 209, 238, 64]);
        assert!(component_overrides(&blocks, &names, "Inactive state").is_empty());
    }
    fn class(name: &[u8]) -> Vec<u8> {
        let mut bytes = vec![255, 255, 1, 0];
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(name);
        bytes
    }
    #[test]
    fn variable_tags_and_false_float_tags() {
        for low in [0x32, 0x34, 0x38] {
            let mut bytes = class(b"moVisualProperties_c");
            bytes.extend(definition([49, 169, 84, 255]));
            bytes.extend_from_slice(&[0, 0, 0x80, 0xbf]);
            bytes.extend(definition([1, 2, 3, 255]));
            bytes.extend_from_slice(&[low, 0x80]);
            bytes.extend(definition([49, 169, 84, 255]));
            assert_eq!(tag(&bytes), Some([low, 0x80]));
            assert_eq!(direct(&bytes, tag(&bytes)), Some([49, 169, 84, 255]));
            bytes.extend_from_slice(&[low, 0x80]);
            bytes.extend(definition([255, 0, 0, 255]));
            assert_eq!(direct(&bytes, tag(&bytes)), None);
        }
    }
    #[test]
    fn shared_body_appearance_objects_count_constructors_and_skip_strings() {
        let mut bytes = class(b"visStateKeyData_c"); // class 1, object 2
        bytes.extend(framed("\u{8003} pointer-like bytes are text"));
        bytes.extend(class(b"visStateVisualPropertiesAttribute_c")); // class 3, object 4
        bytes.extend(1u32.to_le_bytes());
        let material_start = bytes.len();
        bytes.extend(class(b"moVisualProperties_c")); // class 5, object 6
        let payload_start = bytes.len();
        bytes.extend(definition([21, 51, 112, 255]));
        let payload_end = bytes.len();
        bytes.extend([3, 128]); // attribute object 7 reuses class 3
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(6u16.to_le_bytes()); // shared material object 6
        let opaque = vec![payload_start..payload_end];
        let attrs = archive_body_attributes(&bytes, &classes(&bytes), &opaque, [5, 128]).unwrap();
        assert_eq!(attrs.get(&4), Some(&[21, 51, 112, 255]));
        assert_eq!(attrs.get(&7), Some(&[21, 51, 112, 255]));
        assert!(archive_body_attributes(&bytes, &classes(&bytes), &opaque, [6, 128]).is_none());
        let mut unknown = bytes.clone();
        unknown.extend(class(b"unknownObject_c"));
        assert!(archive_body_attributes(&unknown, &classes(&unknown), &opaque, [5, 128]).is_none());
        bytes[material_start..material_start + 2].fill(0);
        assert!(archive_body_attributes(&bytes, &classes(&bytes), &opaque, [5, 128]).is_none());
    }
    #[test]
    fn rejects_truncated_payloads_and_unowned_definitions() {
        let bytes = definition([1, 2, 3, 255]);
        for end in 0..bytes.len() {
            assert_eq!(payload(&bytes[..end]), None);
        }
        assert_eq!(part_state(&bytes, None), None);
        assert_eq!(payload(&bytes), Some([1, 2, 3, 255]));
    }

    fn state() -> Vec<u8> {
        let mut bytes = class(b"visStateKeyData_c");
        let key: Vec<_> = "<Default>_Display State 1".encode_utf16().collect();
        bytes.extend_from_slice(b"\xff\xfe\xff");
        bytes.push(key.len() as u8);
        for word in key {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        for name in [
            b"moCompHolder_c".as_slice(),
            b"moUnitComponent_c",
            b"visStateVisualPropertiesAttribute_c",
        ] {
            bytes.extend(class(name));
        }
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend(class(b"moVisualProperties_c"));
        bytes.extend(definition([73, 169, 84, 255]));
        bytes
    }
    #[test]
    fn part_scope_requires_matching_configuration_and_unique_display_state() {
        let mut bytes = state();
        assert_eq!(
            part_state(&bytes, Some("Default")),
            Some([73, 169, 84, 255])
        );
        assert_eq!(part_state(&bytes, Some("Other")), None);
        bytes.extend(state());
        assert_eq!(part_state(&bytes, Some("Default")), None);
    }

    #[test]
    fn face_override_stays_in_its_class_and_beats_body_color() {
        let mut bytes = class(b"moVisualProperties_c");
        bytes.extend(definition([192, 192, 192, 255]));
        bytes.extend(class(b"uoTempFaceTessData_c"));
        let first = (bytes.len(), bytes.len() + 16);
        bytes.extend_from_slice(&[0; 16]);
        bytes.extend_from_slice(&[0x34, 0x80]);
        bytes.extend(definition([255, 0, 0, 255]));
        let second = (bytes.len(), bytes.len() + 16);
        bytes.extend_from_slice(&[0; 16]);
        bytes.extend(class(b"uoBodyPropInfo_c"));
        bytes.extend_from_slice(&[0x34, 0x80]);
        bytes.extend(definition([0, 255, 0, 255]));
        let third = (bytes.len(), bytes.len() + 16);
        bytes.extend_from_slice(&[0; 16]);
        assert_eq!(
            display_colors(&bytes, &[first, second, third]),
            vec![Some([255, 0, 0, 255]), Some([0, 255, 0, 255]), None]
        );
    }

    #[test]
    fn body_property_slot_is_not_confused_with_later_reused_face_objects() {
        let mut bytes = class(b"moVisualProperties_c");
        bytes.extend(definition([192, 192, 192, 255]));
        bytes.extend(class(b"uoTempFaceTessData_c"));
        let face = (bytes.len(), bytes.len() + 16);
        bytes.extend([0; 16]);
        bytes.extend(class(b"uoBodyPropInfo_c"));
        bytes.extend([0x34, 0x80]);
        bytes.extend(definition([255, 239, 35, 255]));
        // A reused face class/visual property follows without a declaration.
        bytes.extend([0x33, 0x80, 0x34, 0x80]);
        bytes.extend(definition([202, 209, 238, 255]));
        let next = (bytes.len(), bytes.len() + 16);
        bytes.extend([0; 16]);
        assert_eq!(
            display_colors(&bytes, &[face, next]),
            vec![Some([255, 239, 35, 255]), None]
        );
    }

    #[test]
    fn named_appearance_rgb_requires_the_complete_framed_run() {
        let mut bytes = definition([165, 132, 0, 255]);
        bytes.extend([0; 20]);
        bytes.extend(framed(""));
        bytes.extend([0; 16]);
        bytes.extend(framed("polishedbrass"));
        let mut props = [0; 81];
        props[..5].copy_from_slice(&[3, 211, 170, 33, 0]);
        props[5..9].copy_from_slice(&1f32.to_le_bytes());
        bytes.extend(props);
        bytes.extend(framed("<SystemTexture>\\metal\\brass.p2m"));
        bytes.extend([0; 12]);
        bytes.extend(framed(""));
        assert_eq!(payload(&bytes), Some([211, 170, 33, 255]));
        assert_eq!(payload(&bytes[..bytes.len() - 1]), Some([165, 132, 0, 255]));
    }

    #[test]
    fn sole_body_scope_rejects_multiple_bodies_and_preserves_face_overrides() {
        let mut display = class(b"moVisualProperties_c");
        display.extend(definition([202, 209, 238, 255]));
        display.extend(class(b"uoTempBodyTessData_c"));
        display.extend(1u16.to_le_bytes());
        display.extend(class(b"uoTempFaceTessData_c"));
        let face = (display.len(), display.len() + 16);
        display.extend([0; 16]);
        display.extend([0x34, 0x80]);
        display.extend(definition([1, 2, 3, 255]));
        display.extend(class(b"uoBodyPropInfo_c"));
        display.extend([0x34, 0x80]);
        display.extend(definition([202, 209, 238, 255]));
        let mut visual = state();
        visual.extend(class(b"moCompSolidBody_c"));
        visual.extend([5, 0x80, 0, 0, 0, 0, 0, 0, 0]);
        visual.extend(101u32.to_le_bytes());
        visual.extend(101u32.to_le_bytes());
        visual.extend([7, 0x80, 1, 0, 0, 0, 0x34, 0x80]);
        visual.extend(definition([211, 170, 33, 255]));
        let parse = |visual: &[u8], display: &[u8]| {
            let mut blocks = Blocks::new();
            blocks.insert(
                "ThirdPtyStore/VisualStates".into(),
                super::super::Block {
                    compressed: visual,
                    expanded: visual.len(),
                    crc: crc32fast::hash(visual),
                },
            );
            sole_body_color(&blocks, display, &[face])
        };
        assert_eq!(parse(&visual, &display), Some([211, 170, 33, 255]));
        assert_eq!(
            display_colors_with_body(&display, &[face], Some([211, 170, 33, 255])),
            vec![Some([1, 2, 3, 255])]
        );
        display.extend(class(b"uoTempBodyTessData_c"));
        display.extend(1u16.to_le_bytes());
        assert_eq!(parse(&visual, &display), None);
    }

    #[test]
    fn configured_color_does_not_require_supported_display_geometry() {
        for reordered in [false, true] {
            let mut bytes = class(b"visStateKeyData_c");
            bytes.extend(framed("Default_Display State 1"));
            let sequence: &[&[u8]] = if reordered {
                &[
                    b"moUnitComponent_c",
                    b"visStateVisualPropertiesAttribute_c",
                    b"visStateVisualStateAttribute_c",
                    b"moCompHolder_c",
                    b"moVisualProperties_c",
                ]
            } else {
                &[
                    b"moCompHolder_c",
                    b"moUnitComponent_c",
                    b"visStateVisualPropertiesAttribute_c",
                    b"visStateVisualStateAttribute_c",
                    b"moVisualProperties_c",
                ]
            };
            for name in sequence {
                bytes.extend(class(name));
                if *name == b"visStateVisualPropertiesAttribute_c" {
                    bytes.extend(1u32.to_le_bytes());
                }
            }
            bytes.extend(definition([192, 192, 192, 255]));
            let mut blocks = Blocks::new();
            blocks.insert(
                "ThirdPtyStore/VisualStates".into(),
                super::super::Block {
                    compressed: &bytes,
                    expanded: bytes.len(),
                    crc: crc32fast::hash(&bytes),
                },
            );
            assert_eq!(
                configured_part_color(&blocks, "Default"),
                Some([192, 192, 192, 255])
            );
            assert_eq!(configured_part_color(&blocks, "Inactive"), None);
        }
    }

    #[test]
    fn feature_color_requires_owned_source_timestamp_and_agreeing_references() {
        let mut state = class(b"visStateKeyData_c");
        state.extend(framed("Default_Display State 1"));
        state.extend(class(b"moVisualProperties_c"));
        state.extend(definition([192, 192, 192, 255]));
        state.extend(class(b"moCompFeature_c"));
        for n in [18000u32, 173, 1234567, 1, 1, 2] {
            state.extend(n.to_le_bytes());
        }
        state.extend([7, 0x80, 1, 0, 0, 0, 9, 0x80]);
        state.extend(definition([255, 239, 35, 255]));
        let keyword =
            b"<Keywords><Configuration id=\"0\"/><Extrusion id=\"173\" Name=\"Boss\"/></Keywords>";
        let resolved: Vec<u8> = [18000u32, 173, 1234567]
            .iter()
            .flat_map(|n| n.to_le_bytes())
            .collect();
        let parse = |resolved: &[u8]| {
            let mut blocks = Blocks::new();
            for (name, bytes) in [
                ("ThirdPtyStore/VisualStates", state.as_slice()),
                ("swXmlContents/KeyWords", keyword.as_slice()),
                ("Contents/Config-0-ResolvedFeatures", resolved),
            ] {
                blocks.insert(
                    name.into(),
                    super::super::Block {
                        compressed: bytes,
                        expanded: bytes.len(),
                        crc: crc32fast::hash(bytes),
                    },
                );
            }
            feature_colors(&blocks)
        };
        let colors = parse(&resolved);
        assert_eq!(
            feature_color(&["moFromSktEntSurfIdRep_c,173,2".into()], &colors),
            Some([255, 239, 35, 255])
        );
        assert_eq!(
            feature_color(
                &[
                    "moFromSktEntSurfIdRep_c,173,2".into(),
                    "moFromSktEntSurfIdRep_c,174,2".into()
                ],
                &colors
            ),
            None
        );
        assert!(parse(&[0; 12]).is_empty());
    }
}
