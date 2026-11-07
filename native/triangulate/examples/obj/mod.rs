use memchr::{memchr, memchr_iter};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Instant;

#[cfg(feature = "desktop")]
pub mod appearance;

#[cfg(feature = "rayon")]
use rayon::prelude::*;

type Point = [f32; 3];
type Triangle = [u32; 3];
type Result<T> = std::result::Result<T, &'static str>;
const RELATIVE_BIAS: i64 = 1 << 30;
const READ_BLOCK: u64 = 4 * 1024 * 1024;

pub struct Obj {
    pub positions: Vec<Point>,
    pub triangles: Vec<Vec<Triangle>>,
    pub count: usize,
}

#[derive(Default)]
struct Chunk {
    positions: Vec<Point>,
    corners: Vec<i32>,
    faces: Faces,
    #[cfg(feature = "desktop")]
    look: appearance::ChunkLook,
}

#[derive(Default)]
enum Faces {
    #[default]
    Empty,
    Uniform(u32),
    Mixed(Vec<u32>),
}

impl Faces {
    fn push(&mut self, corners_before: usize, size: u32) {
        match self {
            Self::Empty => *self = Self::Uniform(size),
            Self::Uniform(previous) if *previous != size => {
                let mut sizes = vec![*previous; corners_before / *previous as usize];
                sizes.push(size);
                *self = Self::Mixed(sizes);
            }
            Self::Mixed(sizes) => sizes.push(size),
            Self::Uniform(_) => {}
        }
    }
}

fn trim(bytes: &[u8]) -> &[u8] {
    if bytes.first().is_some_and(|byte| !byte.is_ascii_whitespace()) {
        return bytes;
    }
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    &bytes[start..]
}

fn position(mut input: &[u8]) -> Result<Point> {
    let mut point = [0.0; 3];
    for coordinate in &mut point {
        input = trim(input);
        let (value, count) = fast_float::parse_partial::<f32, _>(input)
            .map_err(|_| "Invalid OBJ position")?;
        input = &input[count..];
        if !value.is_finite() || input.first().is_some_and(|b| !b.is_ascii_whitespace() && *b != b'#') {
            return Err("Invalid OBJ position");
        }
        *coordinate = value;
    }
    Ok(point)
}

fn parse_chunk<const TRACK: bool>(bytes: &[u8]) -> Result<Chunk> {
    let mut chunk = Chunk::default();
    let mut start = 0;
    for end in memchr_iter(b'\n', bytes).chain(std::iter::once(bytes.len())) {
        let line = trim(&bytes[start..end]);
        start = end + 1;
        if line.len() < 2 {
            continue;
        }
        if !line[1].is_ascii_whitespace() {
            #[cfg(feature = "desktop")]
            if TRACK {
                chunk.look.directive(line, chunk.corners.len());
            }
            continue;
        }
        match line[0] {
            b'v' => chunk.positions.push(position(&line[2..])?),
            b'f' => {
                let first = chunk.corners.len();
                let mut rest = trim(&line[2..]);
                while !rest.is_empty() && rest[0] != b'#' {
                    #[cfg(feature = "desktop")]
                    if TRACK {
                        let (corner, uv, next) = appearance::corner(rest, chunk.positions.len(), chunk.look.uv.len())?;
                        chunk.look.push_uv(chunk.corners.len(), uv);
                        chunk.corners.push(corner);
                        rest = next;
                        continue;
                    }
                    let (corner, next) = parse_corner(rest, chunk.positions.len())?;
                    chunk.corners.push(corner);
                    rest = next;
                }
                let size = chunk.corners.len() - first;
                if size < 3 { return Err("OBJ face has fewer than three vertices"); }
                if size > u32::MAX as usize { return Err("OBJ face has too many vertices"); }
                chunk.faces.push(first, size as u32);
            }
            _ => {}
        }
    }
    Ok(chunk)
}

#[inline]
fn parse_corner(bytes: &[u8], positions: usize) -> Result<(i32, &[u8])> {
    let (number, suffix) = parse_index(bytes)?;
    let index = tagged_index(number, positions)?;
    let end = if suffix.first() == Some(&b'/') {
        suffix_end(suffix)
    } else {
        0
    };
    Ok((index, trim(&suffix[end..])))
}

fn tagged_index(number: i32, positions: usize) -> Result<i32> {
    // Keep relative references local until the preceding chunk sizes are known.
    Ok(if number > 0 {
        number
    } else {
        let tagged = positions as i64 + i64::from(number) - RELATIVE_BIAS;
        if tagged < i32::MIN as i64 || tagged >= 0 {
            return Err("OBJ relative index out of range");
        }
        tagged as i32
    })
}

fn parse_index(bytes: &[u8]) -> Result<(i32, &[u8])> {
    let negative = bytes[0] == b'-';
    let start = usize::from(negative || bytes[0] == b'+');
    let (mut value, digits) = index_prefix(&bytes[start..]);
    let mut end = start + digits;
    while let Some(&byte) = bytes.get(end) {
        let digit = byte.wrapping_sub(b'0');
        if digit > 9 { break; }
        value = value * 10 + i64::from(digit);
        if value > i64::from(i32::MAX) { return Err("OBJ position index out of range"); }
        end += 1;
    }
    if value == 0 || bytes.get(end).is_some_and(|b| !b.is_ascii_whitespace() && *b != b'/' && *b != b'#') {
        return Err("Invalid OBJ position index");
    }
    let value = value as i32;
    Ok((if negative { -value } else { value }, &bytes[end..]))
}

const HIGH_BITS: u64 = 0x8080_8080_8080_8080;
const LOW_BITS: u64 = !HIGH_BITS;

#[inline]
fn index_prefix(bytes: &[u8]) -> (i64, usize) {
    let Some(head) = bytes.first_chunk::<8>() else {
        return (0, 0);
    };
    let word = u64::from_le_bytes(*head);
    let low = word & LOW_BITS;
    // Adding these offsets sets each lane's high bit at '0' and just past '9'.
    let below_zero = !(low + u64::from_le_bytes([0x50; 8]));
    let above_nine = low + u64::from_le_bytes([0x46; 8]);
    let non_digits = (word | below_zero | above_nine) & HIGH_BITS;
    let digits = non_digits.trailing_zeros() as usize / 8;
    if digits == 0 {
        return (0, 0);
    }
    // Pad with leading zeroes, then combine 8 digits into 4, then 2, then 1 number.
    let value = (word << ((8 - digits) * 8)) & 0x0f0f_0f0f_0f0f_0f0f;
    let pairs = value.wrapping_mul(10).wrapping_add(value >> 8) & 0x00ff_00ff_00ff_00ff;
    let quads = pairs.wrapping_mul(100).wrapping_add(pairs >> 16) & 0x0000_ffff_0000_ffff;
    let number = quads.wrapping_mul(10_000).wrapping_add(quads >> 32) as u32;
    (i64::from(number), digits)
}

#[inline]
fn suffix_end(bytes: &[u8]) -> usize {
    let mut rest = bytes;
    while let Some(head) = rest.first_chunk::<8>() {
        let word = u64::from_le_bytes(*head);
        // Skip whole words that contain no space, control character or '#'.
        let space = !(word | ((word & LOW_BITS) + u64::from_le_bytes([0x5f; 8])));
        let comment = word ^ u64::from_le_bytes([b'#'; 8]);
        let comment = !(comment | ((comment & LOW_BITS) + LOW_BITS));
        let candidates = (space | comment) & HIGH_BITS;
        if candidates != 0 {
            rest = &rest[candidates.trailing_zeros() as usize / 8..];
            break;
        }
        rest = &rest[8..];
    }
    bytes.len() - rest.len() + rest.iter().position(|b| b.is_ascii_whitespace() || *b == b'#')
        .unwrap_or(rest.len())
}

fn distance2(a: Point, b: Point) -> f32 {
    let [x, y, z] = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    x * x + y * y + z * z
}

pub(super) fn polygon(indices: &[u32], points: &[Point], triangles: &mut Vec<Triangle>) -> Result<()> {
    polygon_slots(indices, points, |slots| triangles.push(slots.map(|i| indices[i])))
}

fn polygon_slots(indices: &[u32], points: &[Point], mut emit: impl FnMut([usize; 3])) -> Result<()> {
    // Project onto the plane with the largest absolute signed area.
    let planes = [(1, 2), (0, 2), (0, 1)];
    let area = planes.map(|(u, v)| {
        let mut sum = 0.0f32;
        let mut prev = points[*indices.last().unwrap() as usize];
        for &index in indices {
            let p = points[index as usize];
            sum += prev[u] * p[v] - p[u] * prev[v];
            prev = p;
        }
        sum.abs()
    });
    let axis = if area[0] > area[1] && area[0] > area[2] { 0 }
        else if area[1] > area[2] { 1 } else { 2 };
    let (u, v) = planes[axis];
    let projected: Vec<f64> = indices.iter().flat_map(|&i| {
        let p = points[i as usize];
        [f64::from(p[u]), f64::from(p[v])]
    }).collect();
    let mesh = earcutr::earcut(&projected, &[], 2).map_err(|_| "Cannot triangulate OBJ polygon")?;
    if mesh.is_empty() { return Err("Degenerate OBJ polygon"); }
    // Earcut emits a fixed winding. Restore the face's original orientation.
    let mut signed = 0.0;
    for i in 0..indices.len() {
        let j = (i + 1) % indices.len();
        signed += projected[2*i] * projected[2*j+1] - projected[2*j] * projected[2*i+1];
    }
    for triangle in mesh.chunks_exact(3) {
        let [a, mut b, mut c] = [triangle[0], triangle[1], triangle[2]];
        if signed < 0.0 { std::mem::swap(&mut b, &mut c); }
        emit([a, b, c]);
    }
    Ok(())
}

fn resolve_index(index: i32, base: usize, count: usize) -> Result<u32> {
    let index = if index > 0 { i64::from(index) - 1 }
            else { base as i64 + i64::from(index) + RELATIVE_BIAS };
    if index < 0 || index >= count as i64 {
        return Err("OBJ position index out of range");
    }
    Ok(index as u32)
}

fn triangulate(chunk: Chunk, base: usize, points: &[Point]) -> Result<Vec<Triangle>> {
    let resolve = |index| resolve_index(index, base, points.len());
    let push_quad = |triangles: &mut Vec<Triangle>, [a, b, c, d]: [i32; 4]| -> Result<()> {
        let [a, b, c, d] = [resolve(a)?, resolve(b)?, resolve(c)?, resolve(d)?];
        if distance2(points[a as usize], points[c as usize]) <
            distance2(points[b as usize], points[d as usize]) {
            triangles.extend([[a, b, c], [a, c, d]]);
        } else {
            triangles.extend([[a, b, d], [b, c, d]]);
        }
        Ok(())
    };
    let face_count = match &chunk.faces {
        Faces::Empty => return Ok(Vec::new()),
        Faces::Uniform(size) => chunk.corners.len() / *size as usize,
        Faces::Mixed(sizes) => sizes.len(),
    };
    let mut triangles = Vec::with_capacity(chunk.corners.len() - 2 * face_count);
    match chunk.faces {
        Faces::Uniform(3) => {
            for &[a, b, c] in chunk.corners.as_chunks::<3>().0 {
                triangles.push([resolve(a)?, resolve(b)?, resolve(c)?]);
            }
        }
        Faces::Uniform(4) => {
            for &quad in chunk.corners.as_chunks::<4>().0 {
                push_quad(&mut triangles, quad)?;
            }
        }
        faces => {
            let mut resolved = Vec::new();
            let mut push_face = |face: &[i32]| -> Result<()> {
                match face {
                    &[a, b, c] => triangles.push([resolve(a)?, resolve(b)?, resolve(c)?]),
                    &[a, b, c, d] => push_quad(&mut triangles, [a, b, c, d])?,
                    _ => {
                        resolved.clear();
                        for &index in face {
                            resolved.push(resolve(index)?);
                        }
                        polygon(&resolved, points, &mut triangles)?;
                    }
                }
                Ok(())
            };
            match faces {
                Faces::Uniform(size) => {
                    for face in chunk.corners.chunks_exact(size as usize) {
                        push_face(face)?;
                    }
                }
                Faces::Mixed(sizes) => {
                    let mut corners = chunk.corners.as_slice();
                    for size in sizes {
                        let (face, rest) = corners.split_at(size as usize);
                        push_face(face)?;
                        corners = rest;
                    }
                }
                Faces::Empty => unreachable!(),
            }
        }
    }
    Ok(triangles)
}

fn parse_blocks<T: Send>(mut input: &[u8], decode: impl Fn(&[u8]) -> Result<T> + Sync) -> Result<Vec<T>> {
    const BLOCK: usize = 1024 * 1024;
    if input.len() <= BLOCK {
        return Ok(vec![decode(input)?]);
    }
    let mut blocks = Vec::new();
    while !input.is_empty() {
        let target = input.len().min(BLOCK);
        let end = memchr(b'\n', &input[target..]).map_or(input.len(), |i| target + i + 1);
        blocks.push(&input[..end]);
        input = &input[end..];
    }
    #[cfg(feature = "rayon")]
    let iter = blocks.par_iter();
    #[cfg(not(feature = "rayon"))]
    let iter = blocks.iter();
    iter.map(|block| decode(block)).collect()
}

fn parse_continuations<const TRACK: bool>(bytes: &[u8]) -> Result<Vec<Chunk>> {
    let input = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let normalized = String::from_utf8_lossy(input).replace("\\\r\n", " ").replace("\\\n", " ");
    parse_blocks(normalized.as_bytes(), parse_chunk::<TRACK>)
}

#[cfg(windows)]
pub(super) fn map_file(path: &Path) -> std::io::Result<(std::fs::File, memmap2::Mmap)> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 1;

    // FILE_SHARE_READ refuses existing writers (including writable mappings) and
    // prevents writes and replacement until the file handle is closed.
    let file = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(path)?;
    // SAFETY: the caller retains the read-only file handle while using the map.
    let map = unsafe { memmap2::MmapOptions::new().map(&file)? };
    Ok((file, map))
}

fn decode_block<const TRACK: bool>(bytes: &[u8]) -> Result<Option<Chunk>> {
    if memchr(b'\\', bytes).is_some() {
        return Ok(None);
    }
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    parse_chunk::<TRACK>(bytes).map(Some)
}

// Read and decode independent byte ranges at the same time. Each range owns
// the lines that start inside it, even when a line straddles two ranges.
fn read_chunk(path: &Path, start: u64, end: u64) -> std::io::Result<Vec<u8>> {
    let mut file = BufReader::new(std::fs::File::open(path)?);
    let begin = start.saturating_sub(1);
    file.seek(SeekFrom::Start(begin))?;
    let mut bytes = vec![0; (end - begin) as usize];
    file.read_exact(&mut bytes)?;
    if bytes.last() != Some(&b'\n') {
        file.read_until(b'\n', &mut bytes)?;
    }
    if start != 0 {
        let first = memchr(b'\n', &bytes).map_or(bytes.len(), |i| i + 1);
        // A line may be longer than a range. Its starting range owns it.
        if first as u64 >= end - begin { return Ok(Vec::new()); }
        bytes.drain(..first);
    }
    Ok(bytes)
}

fn read_blocks<const TRACK: bool>(path: &Path, length: u64) -> std::result::Result<Vec<Chunk>, Box<dyn std::error::Error>> {
    if length <= READ_BLOCK {
        let bytes = std::fs::read(path)?;
        return match decode_block::<TRACK>(&bytes)? {
            Some(chunk) => Ok(vec![chunk]),
            None => Ok(parse_continuations::<TRACK>(&bytes)?),
        };
    }
    let ranges: Vec<_> = (0..length).step_by(READ_BLOCK as usize)
        .map(|start| (start, (start + READ_BLOCK).min(length))).collect();
    #[cfg(feature = "rayon")]
    let iter = ranges.par_iter();
    #[cfg(not(feature = "rayon"))]
    let iter = ranges.iter();
    let chunks: std::result::Result<Vec<_>, String> = iter.map(|&(start, end)| {
        let bytes = read_chunk(path, start, end).map_err(|e| e.to_string())?;
        decode_block::<TRACK>(&bytes).map_err(str::to_owned)
    }).collect();
    let chunks = chunks?;
    if chunks.iter().any(Option::is_none) {
        Ok(parse_continuations::<TRACK>(&std::fs::read(path)?)?)
    } else {
        Ok(chunks.into_iter().flatten().collect())
    }
}

fn load_chunks<const TRACK: bool>(path: &Path) -> std::result::Result<(Vec<Chunk>, &'static str), Box<dyn std::error::Error>> {
    let metadata = path.metadata()?;
    let length = metadata.len();
    #[cfg(windows)]
    if let Some((_file, bytes)) = (metadata.is_file() && length > READ_BLOCK)
        .then(|| map_file(path).ok()).flatten()
    {
        let chunks = parse_blocks(&bytes, decode_block::<TRACK>)?;
        let chunks = if chunks.iter().any(Option::is_none) {
            parse_continuations::<TRACK>(&bytes)?
        } else {
            chunks.into_iter().flatten().collect()
        };
        return Ok((chunks, "mapped"));
    }
    Ok((read_blocks::<TRACK>(path, length)?, "buffered"))
}

pub fn load(path: &Path) -> std::result::Result<Obj, Box<dyn std::error::Error>> {
    let start = Instant::now();
    let (chunks, read_mode) = load_chunks::<false>(path)?;
    finish_geometry(chunks, read_mode, start)
}

fn finish_geometry(chunks: Vec<Chunk>, read_mode: &str, start: Instant) -> std::result::Result<Obj, Box<dyn std::error::Error>> {
    let decoded = Instant::now();
    let mut positions = Vec::with_capacity(chunks.iter().map(|c| c.positions.len()).sum());
    let mut indexed = Vec::with_capacity(chunks.len());
    for mut chunk in chunks {
        let base = positions.len();
        positions.append(&mut chunk.positions);
        indexed.push((chunk, base));
    }
    let assembled = Instant::now();
    let triangulate_chunk = |(chunk, base)| triangulate(chunk, base, &positions);
    let pieces: Vec<_> = if indexed.len() <= 1 {
        indexed.into_iter().map(triangulate_chunk).collect::<Result<_>>()?
    } else {
        #[cfg(feature = "rayon")]
        let iter = indexed.into_par_iter();
        #[cfg(not(feature = "rayon"))]
        let iter = indexed.into_iter();
        iter.map(triangulate_chunk).collect::<Result<_>>()?
    };
    let count = pieces.iter().map(Vec::len).sum();
    if count == 0 { return Err("OBJ has no polygon faces".into()); }
    if std::env::var_os("STEPFLASH_PROFILE").is_some() {
        eprintln!(
            "obj input={read_mode} read_and_decode_ms={:.3} merge_positions_ms={:.3} triangulate_ms={:.3}",
            (decoded - start).as_secs_f64() * 1000.0,
            (assembled - decoded).as_secs_f64() * 1000.0,
            assembled.elapsed().as_secs_f64() * 1000.0,
        );
    }
    Ok(Obj { positions, triangles: pieces, count })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_index_prefix_stops_at_each_non_digit() {
        let prefixes = [0, 1, 12, 123, 1234, 12345, 123456, 1234567];
        for (length, expected) in prefixes.iter().copied().enumerate() {
            for byte in (0..=u8::MAX).filter(|byte| !byte.is_ascii_digit()) {
                let mut word = *b"12345678";
                word[length] = byte;
                assert_eq!(index_prefix(&word), (expected, length));
            }
        }
        assert_eq!(parse_index(b"-000000002147483647/1"), Ok((-i32::MAX, &b"/1"[..])));
        assert!(parse_index(b"2147483648/1").is_err());
    }

    #[test]
    fn face_suffix_keeps_ascii_whitespace_and_comment_rules() {
        for offset in [1, 7, 8, 15, 16] {
            for byte in 0..=u8::MAX {
                let mut suffix = *b"/1234567890/1234567890\t1";
                suffix[offset] = byte;
                let expected = suffix.iter()
                    .position(|byte| byte.is_ascii_whitespace() || *byte == b'#')
                    .unwrap_or(suffix.len());
                assert_eq!(suffix_end(&suffix), expected);
            }
        }
    }
}
