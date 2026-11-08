// Container and display strip layout follow the format documentation and
// sldprt-export reader credited in vendor/sldprt/NOTICE.md.
mod appearance;
mod assembly;
mod bodies;
mod decals;

pub use assembly::{Assembly, PlacedMesh};

use std::io::{Read, Seek, SeekFrom};
use std::{borrow::Cow, collections::HashMap, convert::TryInto, fs, path::Path, time::Instant};

use libdeflater::Decompressor;
use memchr::memmem;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
type Point = [f32; 3];
type Triangle = [u32; 3];

const BLOCK_MARKER: &[u8] = b"\x14\x00\x06\x00\x08\x00";
const TABLE_MARKER: &[u8] = b"\x04\x00\x00\x00\x08\x00\x00\x00\x02\x00\x00\x00";
const TABLE_DESCRIPTORS: [(usize, u32); 6] = [(4, 8), (12, 100), (12, 100), (4, 8), (4, 8), (1, 8)];
const MAX_BLOCK: usize = 512 * 1024 * 1024;
const MAX_TRIANGLES: usize = 32_000_000;
const PART_WELD_INVERSE: f64 = 10_000.0;

/// Explorer's fast path reads the saved thumbnail without initializing a GPU
/// or opening SolidWorks. Modern files normally keep PreviewPNG near the end.
pub fn saved_preview(path: &Path, output: &Path) -> Result<()> {
    let start = Instant::now();
    let mut source = fs::File::open(path)?;
    let length = source.metadata()?.len();
    let tail_start = length.saturating_sub(4 * 1024 * 1024);
    source.seek(SeekFrom::Start(tail_start))?;
    let mut bytes = vec![0; 8];
    source.read_to_end(&mut bytes)?;
    let mut directory = blocks(&bytes);
    if !directory.contains_key("PreviewPNG") && tail_start != 0 {
        bytes = fs::read(path)?;
        directory = blocks(&bytes);
    }
    let block = directory.get("PreviewPNG").ok_or("No saved PNG preview")?;
    if block.expanded > 16 * 1024 * 1024 {
        return Err("Saved preview is too large".into());
    }
    let png = block.expand()?;
    if png.len() < 24 || !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("Invalid saved PNG preview".into());
    }
    let dimension = |at| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
    if !(1..=4096).contains(&dimension(16)) || !(1..=4096).contains(&dimension(20)) {
        return Err("Saved preview dimensions exceed the limit".into());
    }
    fs::write(output, &png)?;
    println!(
        "saved_preview_ms={:.3} bytes={}",
        start.elapsed().as_secs_f64() * 1000.0,
        png.len()
    );
    Ok(())
}

#[derive(Clone, Default)]
pub struct Mesh {
    pub positions: Vec<Point>,
    pub triangles: Vec<Triangle>,
    pub color: Option<appearance::Color>,
    pub face_colors: Vec<(std::ops::Range<usize>, appearance::Color)>,
    pub face_references: Vec<(std::ops::Range<usize>, Vec<String>)>,
    pub decals: Vec<decals::Decal>,
    pub incomplete_decals: bool,
}

#[derive(Clone, Copy)]
struct Block<'a> {
    compressed: &'a [u8],
    expanded: usize,
    crc: u32,
}

impl<'a> Block<'a> {
    fn expand(self) -> Result<Cow<'a, [u8]>> {
        if self.expanded == 0 || self.expanded > MAX_BLOCK {
            return Err("SolidWorks block exceeds the decode limit".into());
        }
        let mut bytes = vec![0; self.expanded];
        let mut decoder = Decompressor::new();
        for decompress in [
            Decompressor::deflate_decompress,
            Decompressor::zlib_decompress,
        ] {
            if decompress(&mut decoder, self.compressed, &mut bytes) == Ok(self.expanded)
                && crc32fast::hash(&bytes) == self.crc
            {
                return Ok(Cow::Owned(bytes));
            }
        }
        if self.compressed.len() == self.expanded && crc32fast::hash(self.compressed) == self.crc {
            return Ok(Cow::Borrowed(self.compressed));
        }
        Err("SolidWorks block failed its size or CRC check".into())
    }
}

fn word(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes[..4].try_into().unwrap())
}

type Blocks<'a> = HashMap<String, Block<'a>>;

fn blocks(bytes: &[u8]) -> Blocks<'_> {
    let mut result = HashMap::new();
    let Some(data) = bytes.get(8..) else {
        return result;
    };
    for offset in memmem::find_iter(data, BLOCK_MARKER) {
        let record = &data[offset..];
        let Some(header) = record.get(..26) else {
            break;
        };
        let crc = word(&header[10..]);
        let compressed = word(&header[14..]) as usize;
        let expanded = word(&header[18..]) as usize;
        let name_len = word(&header[22..]) as usize;
        // The cache section index uses the same marker with an encoded length.
        if u64::from(crc) == 2 * expanded as u64 && compressed == expanded / 2 {
            continue;
        }
        if compressed == 0 || !(1..=1024).contains(&name_len) {
            continue;
        }
        let payload = 26 + name_len;
        let Some(compressed) = record
            .get(payload..)
            .and_then(|rest| rest.get(..compressed))
        else {
            continue;
        };
        let name: Vec<_> = record[26..payload]
            .iter()
            .map(|byte| byte.rotate_left(4))
            .collect();
        if name.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
            result
                .entry(String::from_utf8(name).unwrap())
                .or_insert(Block {
                    compressed,
                    expanded,
                    crc,
                });
        }
    }
    result
}

struct Table<'a> {
    lengths: &'a [u8],
    positions: &'a [u8],
    start: usize,
    end: usize,
}

fn read_table(mut bytes: &[u8]) -> Option<(Table<'_>, usize)> {
    let start_len = bytes.len();
    let mut descriptors = [&[][..]; TABLE_DESCRIPTORS.len()];
    for ((size, kind), target) in TABLE_DESCRIPTORS.iter().copied().zip(&mut descriptors) {
        let header = bytes.get(..16)?;
        let count = word(&header[12..]) as usize;
        if word(header) != size as u32
            || word(&header[4..]) != kind
            || word(&header[8..]) != 2
            || count > 2_000_000
        {
            return None;
        }
        (*target, bytes) = bytes[16..].split_at_checked(count * size)?;
    }
    let [lengths, positions, normals, _, strip_checks, _] = descriptors;
    if lengths.is_empty()
        || lengths.len() / 4 > 250_000
        || strip_checks.len() != lengths.len()
        || (!normals.is_empty() && normals.len() != positions.len())
    {
        return None;
    }
    let mut remaining = positions.len() / 12;
    for (length, check) in lengths.chunks_exact(4).zip(strip_checks.chunks_exact(4)) {
        let length = word(length);
        if !(3..=1_000_000).contains(&length) || word(check) != 2 * length - 2 {
            return None;
        }
        remaining = remaining.checked_sub(length as usize)?;
    }
    (remaining == 0).then_some((
        Table {
            lengths,
            positions,
            start: 0,
            end: start_len - bytes.len(),
        },
        start_len - bytes.len(),
    ))
}

fn tables(mut bytes: &[u8]) -> impl Iterator<Item = Table<'_>> {
    let finder = memmem::Finder::new(TABLE_MARKER);
    let total = bytes.len();
    std::iter::from_fn(move || loop {
        bytes = &bytes[finder.find(bytes)?..];
        if let Some((mut table, used)) = read_table(bytes) {
            table.start = total - bytes.len();
            table.end = table.start + used;
            bytes = &bytes[used..];
            return Some(table);
        }
        bytes = &bytes[1..];
    })
}

#[derive(Default)]
struct MeshBuilder {
    mesh: Mesh,
    vertices: HashMap<[u64; 3], u32>,
}

impl MeshBuilder {
    fn add_table(&mut self, table: Table<'_>, inverse: f64) -> Result<()> {
        let possible = table.positions.len() / 12 - 2 * (table.lengths.len() / 4);
        if self.mesh.triangles.len() + possible > MAX_TRIANGLES {
            return Err("SolidWorks mesh exceeds the triangle limit".into());
        }
        let mut positions = table.positions.chunks_exact(12);
        let vertices = &mut self.mesh.positions;
        for length in table.lengths.chunks_exact(4) {
            let mut previous = [0; 2];
            for (corner, bytes) in positions.by_ref().take(word(length) as usize).enumerate() {
                let point = [0, 4, 8]
                    .map(|offset| 1000.0 * f64::from(f32::from_bits(word(&bytes[offset..]))));
                let position = point.map(|value| value as f32);
                if !position.iter().all(|value| value.is_finite()) {
                    return Err("Invalid SolidWorks vertex position".into());
                }
                let key = point.map(|coordinate| {
                    let rounded = (coordinate * inverse).round_ties_even();
                    if rounded == 0.0 {
                        0
                    } else {
                        rounded.to_bits()
                    }
                });
                let index = *self.vertices.entry(key).or_insert_with(|| {
                    let index = vertices.len() as u32;
                    vertices.push(position);
                    index
                });
                let [a, b] = previous;
                if corner >= 2 && a != b && a != index && b != index {
                    self.mesh.triangles.push(if corner & 1 == 0 {
                        [a, b, index]
                    } else {
                        [a, index, b]
                    });
                }
                previous = [b, index];
            }
        }
        Ok(())
    }

    fn add_display_table(
        &mut self,
        table: Table<'_>,
        inverse: f64,
        color: Option<appearance::Color>,
        references: Vec<String>,
    ) -> Result<()> {
        let start = self.mesh.triangles.len();
        self.add_table(table, inverse)?;
        let range = start..self.mesh.triangles.len();
        if !references.is_empty() {
            self.mesh.face_references.push((range.clone(), references));
        }
        if let Some(color) = color {
            self.mesh.face_colors.push((range, color));
        }
        Ok(())
    }
}

fn display_mesh(blocks: &Blocks<'_>, path: &Path) -> Result<Option<Mesh>> {
    for name in ["Contents/DisplayLists", "FaceTessellations"] {
        let Some(block) = blocks.get(name) else {
            continue;
        };
        let start = Instant::now();
        let data = block.expand()?;
        let expanded = start.elapsed();
        let mut builder = MeshBuilder::default();
        builder.mesh.color = appearance::part_color(blocks, None);
        let tables: Vec<_> = tables(&data).collect();
        let bounds: Vec<_> = tables
            .iter()
            .map(|table| (table.start, table.end))
            .collect();
        let body_color = appearance::sole_body_color(blocks, &data, &bounds);
        // A saved display can cover only one body of a multi-body part.
        // Body color belongs to those tables, never the whole-part fallback.
        let colors = appearance::display_colors_with_body(&data, &bounds, body_color);
        let body_colors = appearance::imported_body_colors(blocks, &data, &bounds);
        let feature_colors = appearance::feature_colors(blocks);
        let mut ranges = Vec::with_capacity(tables.len());
        for (index, (table, color)) in tables.into_iter().zip(colors).enumerate() {
            let end = bounds.get(index + 1).map_or(data.len(), |next| next.0);
            let references = appearance::surface_references(&data[table.end..end]);
            // Saved inherited defaults must not hide a current feature-level
            // appearance. Distinct face-local overrides retain precedence.
            let color = if color.is_none() || color == builder.mesh.color {
                appearance::feature_color(&references, &feature_colors)
                    .or(body_colors[index])
                    .or(color)
            } else {
                color
            };
            let first = builder.mesh.triangles.len();
            builder.add_display_table(table, PART_WELD_INVERSE, color, references)?;
            ranges.push(first..builder.mesh.triangles.len());
        }
        (builder.mesh.decals, builder.mesh.incomplete_decals) =
            decals::load(blocks, path, &data, &bounds, &ranges);
        if std::env::var_os("STEPFLASH_PROFILE").is_some() {
            eprintln!("SolidWorks display phases: expand={:.2} ms, decode={:.2} ms; block_bytes={}, triangles={}",
                expanded.as_secs_f64() * 1000.0, (start.elapsed() - expanded).as_secs_f64() * 1000.0,
                data.len(), builder.mesh.triangles.len());
        }
        if !builder.mesh.triangles.is_empty() {
            return Ok(Some(builder.mesh));
        }
    }
    Ok(None)
}

fn load_part_appearance(path: &Path, configuration: &str) -> Option<appearance::SourceAppearance> {
    with_blocks(path, |blocks| {
        if !appearance::configuration_matches(blocks, configuration) {
            return Ok(None);
        }
        Ok(match display_mesh(blocks, path)? {
            Some(mesh) => Some(appearance::SourceAppearance::from_mesh(&mesh)),
            None => appearance::configured_part_color(blocks, configuration).map(|color| {
                appearance::SourceAppearance {
                    color: Some(color),
                    surfaces: HashMap::new(),
                    visible_mesh: None,
                }
            }),
        })
    })
    .ok()
    .flatten()
}

fn with_blocks<T>(path: &Path, read_blocks: impl FnOnce(&Blocks<'_>) -> Result<T>) -> Result<T> {
    let start = Instant::now();
    let bytes = fs::read(path)?;
    let read = start.elapsed();
    let directory = blocks(&bytes);
    if std::env::var_os("STEPFLASH_PROFILE").is_some() {
        eprintln!(
            "SolidWorks file phases: read={:.2} ms, directory={:.2} ms; blocks={}",
            read.as_secs_f64() * 1000.0,
            (start.elapsed() - read).as_secs_f64() * 1000.0,
            directory.len()
        );
    }
    read_blocks(&directory)
}

pub fn load_part(path: &Path) -> Result<Mesh> {
    with_blocks(path, |directory| {
        display_mesh(directory, path)?.ok_or_else(|| {
        "SolidWorks file has no supported saved display geometry. Save the display mesh or export STEP.".into()
    })
    })
}

pub fn load_assembly(path: &Path) -> Result<Assembly> {
    with_blocks(path, |directory| match assembly::rebuild(path, directory) {
        Ok(scene) => Ok(scene),
        Err(reason) => {
            let Some(mesh) = display_mesh(directory, path)? else {
                return Err(reason);
            };
            if std::env::var_os("STEPFLASH_PROFILE").is_some() {
                eprintln!("SolidWorks assembly display snapshot: {reason}");
            }
            Ok(Assembly {
                meshes: vec![PlacedMesh {
                    mesh,
                    instances: Vec::new(),
                }],
                issues: 1,
                snapshot: true,
                diagnostics: vec![format!(
                    "Saved display snapshot; component completeness is unknown: {reason}"
                )],
            })
        }
    })
}
