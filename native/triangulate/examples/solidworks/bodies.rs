//! Narrow read-only imported-body identity reader. Format: vendor/sldprt/NOTICE.md.
//! No tessellation or CAD reconstruction; only owned name/ATOM_ID attributes.
use super::{word, Blocks};
use libdeflater::Decompressor;
use memchr::memmem;
use std::{collections::HashMap, convert::TryInto};

const LIMIT: usize = 128 * 1024 * 1024;
const MAGIC: &[u8] = b"\x23\x1d\xd5\x71\xda\x81\x48\xa2\xa8\x58\x98\xb2\x1b\x89\xef\x99";
fn be16(d: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(p..p + 2)?.try_into().ok()?))
}
fn be32(d: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(p..p + 4)?.try_into().ok()?))
}
fn put<T: PartialEq>(map: &mut HashMap<u16, Option<T>>, id: u16, value: T) {
    map.entry(id)
        .and_modify(|old| {
            if old.as_ref() != Some(&value) {
                *old = None;
            }
        })
        .or_insert(Some(value));
}
fn get<T>(map: &HashMap<u16, Option<T>>, id: u16) -> Option<&T> {
    map.get(&id)?.as_ref()
}

fn identity(data: &[u8]) -> Option<(String, u32)> {
    if !data.starts_with(b"PS\0\0") {
        return None;
    }
    let mut keys = HashMap::new();
    let mut text = HashMap::new();
    let mut integers = HashMap::new();
    let mut occupied = Vec::new();
    // Variable arrays own their full extent; embedded tag bytes are data.
    let mut at = 0;
    while at + 8 <= data.len() {
        if data[at] != 0 || !matches!(data[at + 1], 0x4f | 0x52 | 0x54) {
            at += 1;
            continue;
        }
        let kind = data[at + 1];
        let p = at + 2 + usize::from(data.get(at + 2) == Some(&255));
        let Some((count, id)) = be32(data, p).zip(be16(data, p + 4)) else {
            break;
        };
        let n = count as usize;
        let end = p + 6 + n * if kind == 0x52 { 4 } else { 1 };
        if n == 0
            || n > 4096
            || !(2..0x8000).contains(&id)
            || end > data.len()
            || data.get(end).is_some_and(|b| *b != 0)
        {
            at += 1;
            continue;
        }
        let payload = &data[p + 6..end];
        if kind == 0x52 {
            put(
                &mut integers,
                id,
                payload
                    .chunks_exact(4)
                    .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
                    .collect::<Vec<_>>(),
            );
        } else {
            if !payload.iter().all(|b| (32..=126).contains(b)) {
                at += 1;
                continue;
            }
            let value = std::str::from_utf8(payload).ok()?.to_owned();
            if kind == 0x4f {
                put(&mut keys, id, value);
            } else {
                put(&mut text, id, value);
            }
        }
        occupied.push(at..end);
        at = end;
    }
    let inside = |at| occupied.iter().any(|r| r.contains(&at));
    let mut definitions = HashMap::new();
    for at in memmem::find_iter(data, b"\0\x50") {
        if inside(at) {
            continue;
        }
        let p = at + 2 + usize::from(data.get(at + 2) == Some(&255));
        let Some((count, id)) = be32(data, p).zip(be16(data, p + 4)) else {
            continue;
        };
        if !matches!(count, 1 | 2) || !(2..0x8000).contains(&id) {
            continue;
        }
        let Some(name) = be16(data, p + 8).and_then(|i| get(&keys, i)) else {
            continue;
        };
        if !matches!(name.as_str(), "ATOM_ID_2001" | "SDL/TYSA_NAME") {
            continue;
        }
        let mut forms = Vec::new();
        // Embedded base schemas extend the Boolean legal-owner roster.
        for width in 38..=56 {
            let end = p + width + count as usize;
            let Some(fields) = data.get(p + width..end) else {
                continue;
            };
            if data.get(end).is_some_and(|b| *b != 0)
                || !data[p + 24..p + width].iter().all(|b| *b <= 1)
            {
                continue;
            }
            if (name == "ATOM_ID_2001" && fields == [9, 1])
                || (name == "SDL/TYSA_NAME" && fields == [3])
            {
                forms.push((name.clone(), fields.to_vec()));
            }
        }
        if forms.len() == 1 {
            put(&mut definitions, id, forms.remove(0));
        }
    }
    let mut ids = HashMap::new();
    let mut names = HashMap::new();
    for at in memmem::find_iter(data, b"\0\x51") {
        if inside(at) {
            continue;
        }
        let p = at + 2 + usize::from(data.get(at + 2) == Some(&255));
        let Some(count) = be32(data, p) else {
            continue;
        };
        if !(1..=32).contains(&count) {
            continue;
        }
        let Some(definition) = be16(data, p + 10).and_then(|id| get(&definitions, id)) else {
            continue;
        };
        if definition.1.len() != count as usize {
            continue;
        }
        let end = p + 12 + (5 + count as usize) * 2;
        if end > data.len() || data.get(end).is_some_and(|b| *b != 0) {
            continue;
        }
        let Some(owner) = be16(data, p + 12).filter(|id| (2..0x8000).contains(id)) else {
            continue;
        };
        let Some(value) = be16(data, end - 2) else {
            continue;
        };
        if definition.0 == "ATOM_ID_2001" {
            if let Some(values) =
                get(&integers, value).filter(|v| v.len() == 1 && v[0] != 0 && v[0] != u32::MAX)
            {
                put(&mut ids, owner, values[0]);
            }
        } else if let Some(name) = get(&text, value) {
            put(&mut names, owner, name.clone());
        }
    }
    let pairs: Vec<_> = ids
        .iter()
        .filter_map(|(owner, id)| Some((get(&names, *owner)?.clone(), (*id)?)))
        .collect();
    // LocalBodies frames exactly one plain body per chain.
    if pairs.len() == 1 {
        pairs.into_iter().next()
    } else {
        None
    }
}

fn local_identities(data: &[u8]) -> Option<Vec<(String, u32)>> {
    let mut rest = data;
    let features = take_word(&mut rest)?;
    if !(1..=1024).contains(&features) {
        return None;
    }
    let mut result = Vec::new();
    let mut total = 0usize;
    for _ in 0..features {
        super::appearance::string(&mut rest)?;
        if take_word(&mut rest)? != 0 {
            return None;
        }
        let count = take_word(&mut rest)?;
        if !(1..=4096).contains(&count) {
            return None;
        }
        for _ in 0..count {
            if take(&mut rest, 5)? != [0; 5] {
                return None;
            }
            let length = take_word(&mut rest)? as usize;
            if take(&mut rest, 1)? != [0] {
                return None;
            }
            let chain = take_word(&mut rest)? as usize;
            if length != chain + 5 || chain < 16 || chain > LIMIT {
                return None;
            }
            let mut frames = take(&mut rest, chain)?;
            if take(&mut frames, 16)? != MAGIC {
                return None;
            }
            let mut stream = Vec::new();
            while !frames.is_empty() {
                if frames.iter().all(|b| *b == 0) {
                    break;
                }
                let expanded = take_word(&mut frames)? as usize;
                let compressed = take_word(&mut frames)? as usize;
                if expanded == 0 || expanded > LIMIT || total.checked_add(expanded)? > LIMIT {
                    return None;
                }
                let packed = take(&mut frames, compressed)?;
                let start = stream.len();
                stream.resize(start + expanded, 0);
                if Decompressor::new().zlib_decompress(packed, &mut stream[start..]) != Ok(expanded)
                {
                    return None;
                }
                total += expanded;
            }
            if let Some(pair) = identity(&stream) {
                result.push(pair);
            }
        }
    }
    if rest.iter().any(|b| *b != 0) {
        return None;
    }
    Some(result)
}
fn take<'a>(data: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    let (a, b) = data.split_at_checked(n)?;
    *data = b;
    Some(a)
}
fn take_word(data: &mut &[u8]) -> Option<u32> {
    Some(word(take(data, 4)?))
}

pub fn imported_identities(blocks: &Blocks<'_>) -> HashMap<String, u32> {
    let mut by_name = HashMap::<String, Option<u32>>::new();
    let mut id_counts = HashMap::<u32, usize>::new();
    let sections: Vec<_> = blocks
        .iter()
        .filter(|(name, _)| name.ends_with("-FeatureBodies/LocalBodies"))
        .collect();
    if sections.len() != 1 {
        return HashMap::new();
    }
    if sections[0].1.expanded > LIMIT {
        return HashMap::new();
    }
    if let Ok(data) = sections[0].1.expand() {
        if let Some(pairs) = local_identities(&data) {
            for (name, id) in pairs {
                *id_counts.entry(id).or_default() += 1;
                by_name
                    .entry(name)
                    .and_modify(|v| *v = None)
                    .or_insert(Some(id));
            }
        }
    }
    by_name
        .into_iter()
        .filter_map(|(name, id)| {
            id.filter(|id| id_counts.get(id) == Some(&1))
                .map(|id| (name, id))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn array(d: &mut Vec<u8>, kind: u8, id: u16, payload: &[u8], count: u32) {
        d.extend([0, kind]);
        d.extend(count.to_be_bytes());
        d.extend(id.to_be_bytes());
        d.extend(payload);
        d.push(0);
    }
    fn definition(d: &mut Vec<u8>, id: u16, key: u16, fields: &[u8]) {
        d.extend([0, 0x50]);
        let mut p = vec![0; 41];
        p[..4].copy_from_slice(&(fields.len() as u32).to_be_bytes());
        p[4..6].copy_from_slice(&id.to_be_bytes());
        p[8..10].copy_from_slice(&key.to_be_bytes());
        d.extend(p);
        d.extend(fields);
        d.push(0);
    }
    fn attribute(d: &mut Vec<u8>, definition: u16, owner: u16, value: u16, count: usize) {
        d.extend([0, 0x51]);
        let mut p = vec![0; 12 + (5 + count) * 2];
        p[..4].copy_from_slice(&(count as u32).to_be_bytes());
        p[4..6].copy_from_slice(&90u16.to_be_bytes());
        p[10..12].copy_from_slice(&definition.to_be_bytes());
        p[12..14].copy_from_slice(&owner.to_be_bytes());
        let n = p.len();
        p[n - 2..].copy_from_slice(&value.to_be_bytes());
        d.extend(p);
        d.push(0);
    }
    fn fixture(name_owner: u16) -> Vec<u8> {
        let mut d = b"PS\0\0".to_vec();
        array(&mut d, 0x4f, 10, b"ATOM_ID_2001", 12);
        array(&mut d, 0x4f, 11, b"SDL/TYSA_NAME", 13);
        array(&mut d, 0x52, 12, &111u32.to_be_bytes(), 1);
        array(&mut d, 0x54, 13, b"axon_logo[1]", 12);
        definition(&mut d, 20, 10, &[9, 1]);
        definition(&mut d, 21, 11, &[3]);
        attribute(&mut d, 20, 2, 12, 2);
        attribute(&mut d, 21, name_owner, 13, 1);
        d
    }
    #[test]
    fn body_name_and_atom_must_share_one_owner() {
        assert_eq!(identity(&fixture(2)), Some(("axon_logo[1]".into(), 111)));
        assert_eq!(identity(&fixture(3)), None);
        let mut d = fixture(2);
        array(&mut d, 0x52, 12, &112u32.to_be_bytes(), 1);
        assert_eq!(identity(&d), None);
    }
    #[test]
    fn truncation_and_oversized_chains_are_bounded() {
        let d = fixture(2);
        for end in 0..d.len() {
            let _ = identity(&d[..end]);
        }
        for d in [vec![], vec![255; 1024], vec![0; 1024]] {
            assert!(local_identities(&d).is_none());
        }
        let mut d = vec![];
        d.extend(1u32.to_le_bytes());
        d.extend([255, 254, 255, 0]);
        d.extend(0u32.to_le_bytes());
        d.extend(1u32.to_le_bytes());
        d.extend([0; 5]);
        d.extend(u32::MAX.to_le_bytes());
        d.push(0);
        d.extend(u32::MAX.to_le_bytes());
        assert!(local_identities(&d).is_none());
    }
}
