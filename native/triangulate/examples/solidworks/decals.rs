//! Bounded, read-only saved projection decals. No SolidWorks startup or network.
//! The supported display record was verified against the installed native API.
use super::{appearance::string, word, Blocks, Mesh};
use memchr::memmem;
use std::{collections::HashMap, convert::TryInto, ops::Range, path::Path, sync::Arc};

#[derive(Clone)]
pub struct Decal {
    pub range: Range<usize>,
    pub image: Arc<Vec<u8>>,
    u: [f32; 3],
    v: [f32; 3],
    center: [f32; 3],
    size: [f32; 2],
    rotation: f32,
}
impl Decal {
    pub fn uv(&self, point: [f32; 3]) -> [f32; 2] {
        let p = std::array::from_fn::<_, 3, _>(|a| point[a] * 0.001 - self.center[a]);
        let dot = |v: [f32; 3]| (0..3).map(|a| p[a] * v[a]).sum::<f32>();
        let (s, c) = self.rotation.sin_cos();
        let x = dot(self.u);
        let y = dot(self.v);
        [
            (c * x + s * y) / self.size[0] + 0.5,
            0.5 - (-s * x + c * y) / self.size[1],
        ]
    }
}
fn f32_at(d: &[u8], at: usize) -> Option<f32> {
    let n = f32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?);
    n.is_finite().then_some(n)
}
fn f64_at(d: &[u8], at: usize) -> Option<f64> {
    let n = f64::from_le_bytes(d.get(at..at + 8)?.try_into().ok()?);
    n.is_finite().then_some(n)
}
fn text(d: &[u8], at: usize) -> Option<(String, usize)> {
    let mut tail = d.get(at..)?;
    let s = string(&mut tail)?;
    Some((s, d.len() - tail.len()))
}
struct Placement {
    path: String,
    size: [f32; 2],
    rotation: f32,
}
fn placements(blocks: &Blocks<'_>) -> Vec<Placement> {
    let parse = || -> Option<Vec<Placement>> {
        let data = blocks.get("Contents/CMgr")?.expand().ok()?;
        let token = b"moDecalProperties_c";
        let occurrences: Vec<_> = memmem::find_iter(&data, token).collect();
        if occurrences.len() != 1 {
            return None;
        }
        let first = occurrences[0] + token.len();
        if occurrences[0] < 6 || data[occurrences[0] - 6..occurrences[0] - 4] != [255, 255] {
            return None;
        }
        let mut candidates = vec![first];
        // Reused class constructors have the same complete 84-byte scalar body.
        for at in memmem::find_iter(&data, &[1, 0, 0, 0, 4, 0, 0, 0]) {
            if at >= 2 && data[at - 1] & 0x80 != 0 && data[at - 2] != 0 {
                candidates.push(at)
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        let mut result = Vec::new();
        for at in candidates {
            let body = data.get(at..at + 84)?;
            // Projection mapping only. Curved/surface mapping needs a surface UV frame.
            if word(body) != 1 || word(&body[4..]) != 4 {
                continue;
            }
            let (preset, end) = text(&data, at + 84)?;
            let (path, image_end) = text(&data, end)?;
            if !preset.to_ascii_lowercase().ends_with(".p2d") {
                continue;
            }
            // No color-key/custom mask: PNG alpha is handled in the shader.
            if data.get(image_end..image_end + 12)
                != Some(&[0, 0, 0, 0, 255, 254, 255, 0, 0, 0, 0, 0][..])
            {
                continue;
            }
            let size = [f64_at(body, 32)? as f32, f64_at(body, 40)? as f32];
            let rotation = (f64_at(body, 24)? as f32).to_radians();
            if size
                .iter()
                .any(|n| !n.is_finite() || !(1e-7..1000.).contains(n))
                || !rotation.is_finite()
            {
                continue;
            }
            // Mirroring, fitting and offset variants must be resolved explicitly.
            if body[48..84].iter().any(|b| *b != 0)
                || f64_at(body, 8)? != 0.
                || f64_at(body, 16)? != 0.
            {
                continue;
            }
            result.push(Placement {
                path,
                size,
                rotation,
            });
        }
        Some(result)
    };
    parse().unwrap_or_default()
}
fn image(blocks: &Blocks<'_>, model: &Path, path: &str) -> Option<Arc<Vec<u8>>> {
    const MAX: usize = 32 * 1024 * 1024;
    let mut embedded = Vec::new();
    if let Some(map) = blocks
        .get("UserTextures/TextureNameMap")
        .and_then(|b| b.expand().ok())
    {
        if map.len() < 4 {
            return None;
        }
        let count = word(&map) as usize;
        if count > 1024 {
            return None;
        }
        let mut tail = &map[4..];
        for index in 0..count {
            let name = string(&mut tail)?;
            if name.eq_ignore_ascii_case(path) {
                embedded.push(index)
            }
        }
        if embedded.len() > 1 {
            return None;
        }
    }
    let bytes = if let Some(index) = embedded.first() {
        let block = blocks.get(&format!("UserTextures/Texture_{index}"))?;
        if block.expanded > MAX {
            return None;
        }
        block.expand().ok()?.into_owned()
    } else {
        // Only local sibling assets: predictable latency and cache invalidation.
        let source = Path::new(path);
        let parent = model.parent()?;
        let sibling = parent.join(source.file_name()?);
        if !source.is_absolute() && source.components().count() != 1 {
            return None;
        }
        if source.is_absolute()
            && !source
                .parent()?
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&parent.as_os_str().to_string_lossy())
        {
            return None;
        }
        if std::fs::metadata(&sibling).ok()?.len() > MAX as u64 {
            return None;
        }
        std::fs::read(sibling).ok()?
    };
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") && !bytes.starts_with(&[255, 216, 255]) {
        return None;
    }
    Some(Arc::new(bytes))
}

struct Record {
    u: [f32; 3],
    v: [f32; 3],
    center: [f32; 3],
    faces: Vec<u32>,
}
fn record(data: &[u8], start: usize, end: usize) -> Option<Record> {
    let prefix = data.get(start.checked_sub(170)?..start)?;
    if word(prefix) != 1 || word(&prefix[4..]) != 1 || word(&prefix[8..]) != 1 || prefix[12] != 1 {
        return None;
    }
    let u = std::array::from_fn(|a| f32_at(prefix, 13 + a * 4).unwrap_or(f32::NAN));
    let v = std::array::from_fn(|a| f32_at(prefix, 25 + a * 4).unwrap_or(f32::NAN));
    let center = std::array::from_fn(|a| f32_at(prefix, 37 + a * 4).unwrap_or(f32::NAN));
    let dot = |a: [f32; 3], b: [f32; 3]| (0..3).map(|i| a[i] * b[i]).sum::<f32>();
    if u.iter()
        .chain(v.iter())
        .chain(center.iter())
        .any(|n| !n.is_finite())
        || (dot(u, u) - 1.).abs() > 1e-4
        || (dot(v, v) - 1.).abs() > 1e-4
        || dot(u, v).abs() > 1e-4
    {
        return None;
    }
    if prefix[53..57] != [255, 254, 255, 0] {
        return None;
    }
    let tail = data.get(end..)?;
    if tail.len() < 435
        || tail[..6] != [1, 0, 0, 0, 0, 0]
        || tail[22..34] != [0, 0, 0, 0, 255, 254, 255, 0, 255, 254, 255, 0]
    {
        return None;
    }
    let extents = [f64_at(tail, 6)?, f64_at(tail, 14)?];
    let count = word(&tail[431..]) as usize;
    if !(1..=4096).contains(&count) {
        return None;
    }
    let entries = tail.get(435..435 + count * 32)?;
    let mut faces = Vec::new();
    for entry in entries.chunks_exact(32) {
        let id = word(entry);
        if id == 0
            || f64_at(entry, 4)? != 0.
            || f64_at(entry, 12)? != 0.
            || entry[28..32] != [0; 4]
            || f32_at(entry, 20)? as f64 != extents[0]
            || f32_at(entry, 24)? as f64 != extents[1]
            || faces.contains(&id)
        {
            return None;
        }
        faces.push(id);
    }
    Some(Record {
        u,
        v,
        center,
        faces,
    })
}

fn face_owner(data: &[u8], bounds: &[(usize, usize)], id: u32) -> Option<usize> {
    let mut found = Vec::new();
    for (index, &(_, end)) in bounds.iter().enumerate() {
        let limit = bounds.get(index + 1).map_or(data.len(), |b| b.0);
        let region = data.get(end..limit)?;
        for at in memmem::find_iter(region, &id.to_le_bytes()) {
            // Shared visual-property object slot, followed by the complete
            // face trailer and a repeated persistent-surface reference frame.
            let Some(head) = at.checked_sub(10).and_then(|p| region.get(p..at)) else {
                continue;
            };
            let Some(tail) = region.get(at + 4..at + 28) else {
                continue;
            };
            if head[..8] != [1, 0, 0, 0, 255, 255, 255, 255]
                || head[8..] == [0, 0]
                || head[9] & 0x80 != 0
                || word(tail) == 0
                || tail[4..16] != [1, 0, 0, 0, 255, 255, 255, 255, 1, 0, 0, 0]
                || tail[20..24] != [0; 4]
            {
                continue;
            }
            let Some((refs, _)) = text(region, at + 28) else {
                continue;
            };
            if !refs.contains("SurfIdRep_c,") || !refs.ends_with(", \n") {
                continue;
            }
            // Compare framed UTF-16 references, not their ASCII representation.
            let encoded: Vec<_> = refs.encode_utf16().flat_map(u16::to_le_bytes).collect();
            if memmem::find(&region[..at - 10], &encoded).is_none() {
                continue;
            }
            found.push(index);
        }
    }
    (found.len() == 1).then(|| found[0])
}

pub fn load(
    blocks: &Blocks<'_>,
    path: &Path,
    data: &[u8],
    bounds: &[(usize, usize)],
    ranges: &[Range<usize>],
) -> (Vec<Decal>, bool) {
    // No extra B-rep/identity work for ordinary files without decals.
    let placements = placements(blocks);
    let expected = blocks
        .get("Contents/CMgr")
        .and_then(|b| b.expand().ok())
        .map_or(0, |data| {
            if memmem::find(&data, b"moDecalProperties_c").is_none() {
                return 0;
            }
            memmem::find_iter(&data, b"\xff\xfe\xff")
                .filter_map(|at| text(&data, at))
                .filter(|(name, _)| name.to_ascii_lowercase().ends_with(".p2d"))
                .count()
                .max(1)
        });
    let profile = std::env::var_os("STEPFLASH_PROFILE").is_some();
    if profile && !placements.is_empty() {
        eprintln!(
            "SolidWorks decal projection placements={}",
            placements.len()
        );
    }
    if placements.is_empty() {
        return (Vec::new(), expected != 0);
    }
    let mut result = Vec::new();
    let mut owners = HashMap::<usize, usize>::new();
    let mut complete = placements.len() == expected;
    for placement in &placements {
        if placements
            .iter()
            .filter(|p| p.path.eq_ignore_ascii_case(&placement.path))
            .count()
            != 1
        {
            complete = false;
            continue;
        }
        let mut matches = Vec::new();
        for at in memmem::find_iter(data, b"\xff\xfe\xff") {
            if let Some((name, end)) = text(data, at) {
                if name.eq_ignore_ascii_case(&placement.path) {
                    if let Some(record) = record(data, at, end) {
                        matches.push(record)
                    }
                }
            }
        }
        if matches.len() != 1 {
            complete = false;
            if profile {
                eprintln!(
                    "SolidWorks decal: validated display records={}",
                    matches.len()
                );
            }
            continue;
        }
        let Some(image) = image(blocks, path, &placement.path) else {
            complete = false;
            if profile {
                eprintln!("SolidWorks decal: no embedded or local sibling PNG/JPEG");
            }
            continue;
        };
        let record = matches.remove(0);
        for id in record.faces {
            let Some(index) = face_owner(data, bounds, id) else {
                complete = false;
                if profile {
                    eprintln!("SolidWorks decal: unresolved unique face id={id}");
                }
                continue;
            };
            *owners.entry(index).or_default() += 1;
            result.push((
                index,
                Decal {
                    range: ranges[index].clone(),
                    image: image.clone(),
                    u: record.u,
                    v: record.v,
                    center: record.center,
                    size: placement.size,
                    rotation: placement.rotation,
                },
            ));
        }
    }
    if result.len() > 32 || owners.values().any(|count| *count != 1) {
        complete = false;
    }
    (
        result
            .into_iter()
            .filter(|(i, _)| owners.get(i) == Some(&1))
            .take(32)
            .map(|(_, d)| d)
            .collect(),
        // GPU projection/alpha is tested, but native SolidWorks screen parity
        // is not established. Keep image decals on the saved-image path until
        // that fidelity check passes; never replace a correct label with an
        // approximate projection. No-thumbnail fallback can still render it.
        !complete || expected != 0,
    )
}

/// Assembly cache faces can inherit a decal only through unique complete
/// persistent references. The source projection remains in part coordinates.
pub fn matched(source: &Mesh, cached: &Mesh) -> Vec<Decal> {
    let mut result = Vec::new();
    for (range, refs) in &cached.face_references {
        let candidates: Vec<_> = source
            .face_references
            .iter()
            .filter(|(_, r)| !refs.is_empty() && r == refs)
            .collect();
        if candidates.len() != 1 {
            continue;
        }
        if let Some(decal) = source.decals.iter().find(|d| d.range == candidates[0].0) {
            let mut copy = decal.clone();
            copy.range = range.clone();
            result.push(copy)
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(s: &str) -> Vec<u8> {
        let mut d = vec![255, 254, 255, s.len() as u8];
        d.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
        d
    }
    fn fixture() -> (Vec<u8>, usize, usize) {
        let mut d = vec![0; 170];
        for p in [0, 4, 8] {
            d[p..p + 4].copy_from_slice(&1u32.to_le_bytes())
        }
        d[12] = 1;
        d[13..17].copy_from_slice(&1f32.to_le_bytes());
        d[29..33].copy_from_slice(&1f32.to_le_bytes());
        d[53..57].copy_from_slice(&[255, 254, 255, 0]);
        let start = d.len();
        d.extend(frame("label.png"));
        let end = d.len();
        let mut tail = vec![0; 467];
        tail[0] = 1;
        tail[26..34].copy_from_slice(&[255, 254, 255, 0, 255, 254, 255, 0]);
        tail[6..14].copy_from_slice(&0.5f64.to_le_bytes());
        tail[14..22].copy_from_slice(&0.25f64.to_le_bytes());
        tail[431..435].copy_from_slice(&1u32.to_le_bytes());
        tail[435..439].copy_from_slice(&123u32.to_le_bytes());
        tail[447..455].copy_from_slice(&(-0.0f64).to_le_bytes());
        tail[455..459].copy_from_slice(&0.5f32.to_le_bytes());
        tail[459..463].copy_from_slice(&0.25f32.to_le_bytes());
        d.extend(tail);
        (d, start, end)
    }
    #[test]
    fn projection_requires_complete_frame_and_face_entries() {
        let (d, start, end) = fixture();
        assert_eq!(record(&d, start, end).unwrap().faces, vec![123]);
        for stop in 0..d.len() {
            assert!(record(&d[..stop], start, end).is_none());
        }
        let mut bad = d.clone();
        bad[end + 455..end + 459].copy_from_slice(&0.7f32.to_le_bytes());
        assert!(record(&bad, start, end).is_none());
        let mut bad = d.clone();
        bad[13..17].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(record(&bad, start, end).is_none());
        let mut bad = d;
        bad[end + 431..end + 435].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(record(&bad, start, end).is_none());
    }
    #[test]
    fn projection_uses_part_coordinates_and_clips_in_shader() {
        let mut d = Decal {
            range: 0..1,
            image: Arc::new(Vec::new()),
            u: [1., 0., 0.],
            v: [0., 1., 0.],
            center: [0.01, 0.02, 0.],
            size: [0.02, 0.01],
            rotation: 0.,
        };
        let near = |a: [f32; 2], b: [f32; 2]| {
            assert!(
                (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5,
                "{a:?} {b:?}"
            )
        };
        near(d.uv([10., 20., 0.]), [0.5, 0.5]);
        near(d.uv([0., 25., 0.]), [0., 0.]);
        assert!(d.uv([40., 20., 0.])[0] > 1.);
        d.rotation = std::f32::consts::FRAC_PI_2;
        near(d.uv([10., 30., 0.]), [1., 0.5]);
        let source = Mesh {
            face_references: vec![(0..1, vec!["face".into()])],
            decals: vec![d],
            ..Default::default()
        };
        let mut cached = Mesh {
            face_references: vec![(10..12, vec!["face".into()])],
            ..Default::default()
        };
        assert_eq!(matched(&source, &cached)[0].range, 10..12);
        cached.face_references[0].1 = vec!["different".into()];
        assert!(matched(&source, &cached).is_empty());
    }
    #[test]
    fn face_identifier_requires_unique_trailer_and_repeated_references() {
        let refs = "moSimpleSurfIdRep_c,173,2, \n";
        let mut metadata = frame(refs);
        metadata.extend([0; 12]);
        metadata.extend([1, 0, 0, 0, 255, 255, 255, 255, 58, 0]);
        metadata.extend(123u32.to_le_bytes());
        metadata.extend(173u32.to_le_bytes());
        metadata.extend([1, 0, 0, 0, 255, 255, 255, 255, 1, 0, 0, 0]);
        metadata.extend(2u32.to_le_bytes());
        metadata.extend([0; 4]);
        metadata.extend(frame(refs));
        assert_eq!(face_owner(&metadata, &[(0, 0)], 123), Some(0));
        let mut duplicated = metadata.clone();
        duplicated.extend(&metadata);
        assert_eq!(
            face_owner(
                &duplicated,
                &[(0, 0), (metadata.len(), metadata.len())],
                123
            ),
            None
        );
        metadata[4] = b'x';
        assert_eq!(face_owner(&metadata, &[(0, 0)], 123), None);
    }
}
