use memchr::{memchr, memchr2, memchr_iter};
use log::warn;

#[cfg(feature = "rayon")]
use rayon::prelude::*;

use crate::{
    ap214::Entity,
    id::Id,
    parse::{decode_entity, split_entity_decl},
};

#[derive(Debug)]
pub struct StepFile<'a>(pub Vec<Entity<'a>>);
impl<'a> StepFile<'a> {
    /// Parses a STEP file from a raw array of bytes
    /// `data` must be preprocessed by [`strip_flatten`] first
    pub fn parse(data: &'a [u8]) -> Self {
        let profile = std::env::var_os("STEPFLASH_PROFILE").map(|_| std::time::Instant::now());
        let blocks = Self::into_blocks(&data);
        let data_start = blocks.iter()
            .position(|b| b == b"DATA;")
            .unwrap_or(0) + 1;
        let data_end = blocks.iter()
            .skip(data_start)
            .position(|b| b == b"ENDSEC;")
            .unwrap_or(0) + data_start;
        let block_time = profile.map(|start| start.elapsed());

        // Index the borrowed input before decoding. Ordered collection then
        // writes each large Entity directly into its final slot.
        let declarations = &blocks[data_start..data_end];
        let mut indexed = Vec::with_capacity(declarations.len() + 1);
        indexed.push(None);
        for &block in declarations {
            match split_entity_decl(block) {
                Ok((body, id)) => {
                    if id.0 >= indexed.len() {
                        indexed.resize(id.0 + 1, None);
                    }
                    indexed[id.0] = Some(body);
                }
                Err(error) => warn!("Failed to read declaration ID: {:?}", error),
            }
        }
        let index_time = profile.map(|start| start.elapsed());

        #[cfg(feature = "rayon")]
        let slots = indexed.into_par_iter();
        #[cfg(not(feature = "rayon"))]
        let slots = indexed.into_iter();

        let out: Vec<_> = slots.enumerate().map(|(id, body)| {
            match body {
                None => Entity::_EmptySlot,
                Some(body) => match decode_entity(body) {
                    Ok((_, entity)) => entity,
                    Err(error) => {
                        warn!("Failed to parse #{}{}: {:?}", id, body, error);
                        Entity::_FailedToParse
                    }
                },
            }
        }).collect();

        if let (Some(start), Some(blocks), Some(indexed)) = (profile, block_time, index_time) {
            eprintln!(
                "STEP parse: blocks={:.2} ms, index={:.2} ms, decode={:.2} ms, slots={}, entity_size={} bytes",
                blocks.as_secs_f64() * 1000.0,
                (indexed - blocks).as_secs_f64() * 1000.0,
                (start.elapsed() - indexed).as_secs_f64() * 1000.0,
                out.len(), std::mem::size_of::<Entity>(),
            );
        }
        Self(out)
    }

    /// Flattens a STEP file, removing comments and whitespace
    pub fn strip_flatten(data: &[u8]) -> Vec<u8> {
        // Files without slashes need no comment handling. Compact independent
        // blocks concurrently and concatenate in file order.
        #[cfg(feature = "rayon")]
        if data.len() >= 2 * 1024 * 1024 && memchr(b'/', data).is_none() {
            let chunks: Vec<Vec<u8>> = data.par_chunks(1024 * 1024)
                .map(|chunk| {
                    let mut compact = vec![0; chunk.len()];
                    let mut len = 0;
                    for &byte in chunk {
                        compact[len] = byte;
                        len += usize::from(!byte.is_ascii_whitespace());
                    }
                    compact.truncate(len);
                    compact
                })
                .collect();
            return chunks.concat();
        }

        let mut out = Vec::with_capacity(data.len());
        let mut i = 0;
        while i < data.len() {
            match data[i] {
                b'/' => if i + 1 < data.len() && data[i + 1] == b'*' {
                    for j in memchr_iter(b'/', &data[i + 2..]) {
                        if data[i + j + 1] == b'*' {
                            i += j + 2;
                            break;
                        }
                    }
                }
                // TODO: don't skip whitespace inside of strings
                c if c.is_ascii_whitespace() => (),
                c => out.push(c),
            }
            i += 1;
        }
        out
    }

    /// Splits a STEP file into individual blocks.  The input must be pre-processed
    /// by [`strip_flatten`] beforehand.
    fn into_blocks(data: &[u8]) -> Vec<&[u8]> {
        let mut blocks = Vec::new();
        let mut i = 0;
        let mut start = 0;
        while i < data.len() {
            let next = memchr2(b'\'', b';', &data[i..]).unwrap();
            match data[i + next] {
                // Skip over quoted blocks
                b'\'' => i += next + memchr(b'\'', &data[i + next..]).unwrap() + 1,
                b';' => {
                    blocks.push(&data[start..=(i + next)]);

                    i += next + 1; // Skip the semicolon
                    start = i;
                },
                _ => unreachable!(),
            }
        }
        blocks
    }

    pub fn entity<T: FromEntity<'a>>(&'a self, i: Id<T>) -> Option<&'a T> {
        T::try_from_entity(&self.0[i.0])
    }
}

impl<'a, T> std::ops::Index<Id<T>> for StepFile<'a> {
    type Output = Entity<'a>;

    fn index(&self, id: Id<T>) -> &Self::Output {
        &self.0[id.0]
    }
}

pub trait FromEntity<'a> {
    fn try_from_entity(e: &'a Entity<'a>) -> Option<&'a Self>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_index_preserves_sparse_slots_and_last_declaration() {
        let step = StepFile::parse(
            b"DATA;#10=CARTESIAN_POINT('',(1.,2.,3.));#2=CARTESIAN_POINT('',(4.,5.,6.));#10=UNKNOWN();ENDSEC;",
        );
        assert_eq!(step.0.len(), 11);
        assert!(matches!(step.0[1], Entity::_EmptySlot));
        assert!(matches!(step.0[2], Entity::CartesianPoint(_)));
        assert!(matches!(step.0[10], Entity::_FailedToParse));
    }
}
