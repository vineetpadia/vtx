use std::{convert::TryInto, path::Path, time::{Duration, Instant}};

use memchr::memchr;

#[cfg(feature = "rayon")]
use rayon::prelude::*;

type Point = [f32; 3];
type Triangle = [u32; 3];
type Color = [u8; 4];
type Result<T> = std::result::Result<T, &'static str>;

const MAX_HEADER: usize = 64 * 1024;
const MAX_RECORDS: usize = 32_000_000;
const MAX_POLYGON: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Ascii,
    Little,
    Big,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn parse(name: &str) -> Result<Self> {
        match name {
            "char" | "int8" => Ok(Self::I8),
            "uchar" | "uint8" => Ok(Self::U8),
            "short" | "int16" => Ok(Self::I16),
            "ushort" | "uint16" => Ok(Self::U16),
            "int" | "int32" => Ok(Self::I32),
            "uint" | "uint32" => Ok(Self::U32),
            "float" | "float32" => Ok(Self::F32),
            "double" | "float64" => Ok(Self::F64),
            _ => Err("Unsupported PLY property type"),
        }
    }

    fn width(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }

    fn integer(self) -> bool {
        !matches!(self, Self::F32 | Self::F64)
    }

    fn ascii(self, bytes: &[u8]) -> Result<f64> {
        if self == Self::F32 {
            return fast_float::parse::<f32, _>(bytes)
                .map(f64::from).map_err(|_| "Invalid PLY float");
        }
        if self == Self::F64 {
            return fast_float::parse::<f64, _>(bytes).map_err(|_| "Invalid PLY double");
        }
        let token = std::str::from_utf8(bytes).map_err(|_| "Invalid PLY integer")?;
        let value = token.parse::<i64>().map_err(|_| "Invalid PLY integer")?;
        let valid = match self {
            Self::I8 => (i8::MIN as i64..=i8::MAX as i64).contains(&value),
            Self::U8 => (0..=u8::MAX as i64).contains(&value),
            Self::I16 => (i16::MIN as i64..=i16::MAX as i64).contains(&value),
            Self::U16 => (0..=u16::MAX as i64).contains(&value),
            Self::I32 => (i32::MIN as i64..=i32::MAX as i64).contains(&value),
            Self::U32 => (0..=u32::MAX as i64).contains(&value),
            _ => unreachable!(),
        };
        if !valid { return Err("PLY integer out of range"); }
        Ok(value as f64)
    }

    fn binary(self, bytes: &[u8], little: bool) -> f64 {
        macro_rules! read {
            ($ty:ty) => {{
                let word = bytes.try_into().unwrap();
                if little { <$ty>::from_le_bytes(word) as f64 }
                else { <$ty>::from_be_bytes(word) as f64 }
            }};
        }
        match self {
            Self::I8 => bytes[0] as i8 as f64,
            Self::U8 => bytes[0] as f64,
            Self::I16 => read!(i16),
            Self::U16 => read!(u16),
            Self::I32 => read!(i32),
            Self::U32 => read!(u32),
            Self::F32 => read!(f32),
            Self::F64 => read!(f64),
        }
    }
}

#[derive(Clone, Copy)]
enum Layout {
    Scalar(Scalar),
    List(Scalar, Scalar),
}

struct Property<'a> {
    name: &'a str,
    layout: Layout,
}

struct Element<'a> {
    name: &'a str,
    count: usize,
    properties: Vec<Property<'a>>,
}

impl Element<'_> {
    fn property(&self, name: &str) -> Option<usize> {
        self.properties.iter().position(|property| property.name == name)
    }
}

fn header(mut bytes: &[u8]) -> Result<(Encoding, Vec<Element<'_>>, &[u8])> {
    let mut encoding = None;
    let mut elements: Vec<Element> = Vec::new();
    let mut remaining = MAX_HEADER;
    while let Some(end) = memchr(b'\n', &bytes[..bytes.len().min(remaining)]) {
        let line = std::str::from_utf8(&bytes[..end]).map_err(|_| "Invalid PLY header")?;
        if remaining == MAX_HEADER {
            if line.trim_ascii_end() != "ply" { return Err("Invalid PLY signature"); }
        } else {
            let words: Vec<_> = line.split_ascii_whitespace().collect();
            match words.as_slice() {
                [] | ["comment" | "obj_info", ..] => {}
                ["format", format, "1.0"] if encoding.is_none() && elements.is_empty() => {
                    encoding = Some(match *format {
                        "ascii" => Encoding::Ascii,
                        "binary_little_endian" => Encoding::Little,
                        "binary_big_endian" => Encoding::Big,
                        _ => return Err("Unsupported PLY encoding"),
                    });
                }
                ["element", name, count] if encoding.is_some() => {
                    let count = count.parse::<usize>().map_err(|_| "Invalid PLY element count")?;
                    if count > MAX_RECORDS { return Err("PLY element exceeds the record limit"); }
                    if elements.iter().any(|element| element.name == *name) {
                        return Err("Duplicate PLY element");
                    }
                    elements.push(Element { name, count, properties: Vec::new() });
                }
                ["property", rest @ ..] => {
                    let element = elements.last_mut().ok_or("PLY property declared before an element")?;
                    let (name, layout) = match rest {
                        [kind, name] => (*name, Layout::Scalar(Scalar::parse(kind)?)),
                        ["list", size, kind, name] => {
                            let size = Scalar::parse(size)?;
                            if !size.integer() { return Err("PLY list count must be an integer"); }
                            (*name, Layout::List(size, Scalar::parse(kind)?))
                        }
                        _ => return Err("Invalid PLY property"),
                    };
                    if element.property(name).is_some() { return Err("Duplicate PLY property"); }
                    element.properties.push(Property { name, layout });
                }
                ["end_header"] => return Ok((encoding.ok_or("Missing PLY encoding")?, elements, &bytes[end + 1..])),
                _ => return Err("Unsupported or malformed PLY header"),
            }
        }
        remaining -= end + 1;
        bytes = &bytes[end + 1..];
    }
    Err("Missing end of PLY header")
}

struct Reader<'a> {
    bytes: &'a [u8],
    encoding: Encoding,
}

impl<'a> Reader<'a> {
    fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        let (value, rest) = self.bytes.split_at_checked(size).ok_or("Truncated PLY body")?;
        self.bytes = rest;
        Ok(value)
    }

    fn token(&mut self) -> Result<&'a [u8]> {
        self.bytes = self.bytes.trim_ascii_start();
        if self.bytes.is_empty() { return Err("Truncated ASCII PLY body"); }
        let end = self.bytes.iter().position(u8::is_ascii_whitespace).unwrap_or(self.bytes.len());
        self.take(end)
    }

    fn value(&mut self, kind: Scalar) -> Result<f64> {
        if self.encoding == Encoding::Ascii {
            kind.ascii(self.token()?)
        } else {
            Ok(kind.binary(self.take(kind.width())?, self.encoding == Encoding::Little))
        }
    }

    fn count(&mut self, kind: Scalar) -> Result<usize> {
        let value = self.value(kind)?;
        if value < 0.0 { return Err("Negative PLY list count"); }
        Ok(value as usize)
    }

    fn skip(&mut self, kind: Scalar, count: usize) -> Result<()> {
        if self.encoding == Encoding::Ascii {
            if count > self.bytes.len() { return Err("Truncated ASCII PLY list"); }
            for _ in 0..count { self.token()?; }
        } else {
            self.take(count.checked_mul(kind.width()).ok_or("PLY list too large")?)?;
        }
        Ok(())
    }

    fn skip_property(&mut self, layout: Layout) -> Result<()> {
        match layout {
            Layout::Scalar(kind) => self.skip(kind, 1),
            Layout::List(size, kind) => {
                let count = self.count(size)?;
                self.skip(kind, count)
            }
        }
    }

    fn check_rows(&self, element: &Element) -> Result<()> {
        let width = element.properties.iter().map(|property| match property.layout {
            Layout::Scalar(kind) | Layout::List(kind, _) => {
                if self.encoding == Encoding::Ascii { 1 } else { kind.width() }
            }
        }).sum::<usize>();
        if width == 0 && element.count != 0 { return Err("PLY element has no properties"); }
        if element.count.checked_mul(width).is_none_or(|min| min > self.bytes.len()) {
            return Err("Truncated PLY element");
        }
        Ok(())
    }
}

pub struct Model {
    pub positions: Vec<Point>,
    pub colors: Option<Vec<Color>>,
    pub triangles: Vec<Triangle>,
    pub gaussian: bool,
}

fn read_vertices(reader: &mut Reader, element: &Element) -> Result<(Vec<Point>, Option<Vec<Color>>)> {
    let xyz = ["x", "y", "z"].map(|name| element.property(name));
    let [Some(x), Some(y), Some(z)] = xyz else { return Err("PLY vertex requires x, y and z"); };
    let rgb = ["red", "green", "blue"].map(|name| element.property(name));
    let rgb = match rgb {
        [Some(r), Some(g), Some(b)] if [r, g, b].iter().all(|&i|
            matches!(element.properties[i].layout, Layout::Scalar(Scalar::U8))) => Some([r, g, b]),
        _ => None,
    };
    if reader.encoding == Encoding::Little && [x, y, z] == [0, 1, 2] &&
        element.properties[..3].iter().all(|p| matches!(p.layout, Layout::Scalar(Scalar::F32)))
    {
        if let Some((stride, offsets)) = fixed_vertex_layout(element, rgb) {
            let bytes = reader.take(element.count * stride)?;
            #[cfg(feature = "rayon")]
            if bytes.len() >= 16 * 1024 * 1024 {
                match (stride, offsets) {
                    (12, None) => return decode_parallel_vertices(bytes.par_chunks_exact(12), None),
                    (15, Some([12, 13, 14])) => {
                        return decode_parallel_vertices(bytes.par_chunks_exact(15), offsets);
                    }
                    _ => {}
                }
            }
            return match (stride, offsets) {
                (12, None) => decode_fixed_vertices(bytes.chunks_exact(12), element.count, None),
                (15, Some([12, 13, 14])) => decode_fixed_vertices(bytes.chunks_exact(15), element.count, offsets),
                _ => decode_fixed_vertices(bytes.chunks_exact(stride), element.count, offsets),
            };
        }
    }
    let mut slots = vec![None; element.properties.len()];
    for (axis, index) in [x, y, z].iter().copied().enumerate() {
        if !matches!(element.properties[index].layout, Layout::Scalar(_)) {
            return Err("PLY coordinate must be a scalar");
        }
        slots[index] = Some(axis);
    }
    if let Some(rgb) = rgb {
        for (axis, index) in rgb.iter().copied().enumerate() { slots[index] = Some(axis + 3); }
    }
    let mut points = Vec::with_capacity(element.count);
    let mut colors = rgb.map(|_| Vec::with_capacity(element.count));
    for _ in 0..element.count {
        let mut fields = [0.0; 6];
        for (property, slot) in element.properties.iter().zip(&slots) {
            if let (Layout::Scalar(kind), Some(slot)) = (property.layout, slot) {
                fields[*slot] = reader.value(kind)?;
            } else {
                reader.skip_property(property.layout)?;
            }
        }
        let [x, y, z, r, g, b] = fields;
        let point = [x as f32, y as f32, z as f32];
        if !point.iter().all(|n| n.is_finite()) { return Err("Non-finite PLY position"); }
        points.push(point);
        if let Some(colors) = &mut colors { colors.push([r as u8, g as u8, b as u8, 255]); }
    }
    Ok((points, colors))
}

#[inline]
fn packed_position(record: &[u8]) -> Point {
    let bits: [u32; 3] = bytemuck::pod_read_unaligned(&record[..12]);
    bits.map(|value| f32::from_bits(u32::from_le(value)))
}

#[cfg(feature = "rayon")]
#[inline]
fn decode_parallel_vertices<'a>(
    records: impl IndexedParallelIterator<Item = &'a [u8]>, rgb: Option<[usize; 3]>,
) -> Result<(Vec<Point>, Option<Vec<Color>>)> {
    use std::sync::atomic::{AtomicBool, Ordering};

    let finite = AtomicBool::new(true);
    let positions = records.map(|record| {
        let point = packed_position(record);
        if !point.iter().all(|n| n.is_finite()) { finite.store(false, Ordering::Relaxed); }
        (point, record)
    });
    let (points, colors) = if let Some([r, g, b]) = rgb {
        let (mut points, mut colors) = (Vec::new(), Vec::new());
        positions.map(|(point, record)| (point, [record[r], record[g], record[b], 255]))
            .unzip_into_vecs(&mut points, &mut colors);
        (points, Some(colors))
    } else {
        (positions.map(|(point, _)| point).collect(), None)
    };
    if !finite.load(Ordering::Relaxed) { return Err("Non-finite PLY position"); }
    Ok((points, colors))
}

fn fixed_vertex_layout(element: &Element, rgb: Option<[usize; 3]>) -> Option<(usize, Option<[usize; 3]>)> {
    let mut stride = 0;
    let mut offsets = Vec::with_capacity(element.properties.len());
    for property in &element.properties {
        let Layout::Scalar(kind) = property.layout else { return None; };
        offsets.push(stride);
        stride += kind.width();
    }
    Some((stride, rgb.map(|channels| channels.map(|channel| offsets[channel]))))
}

#[inline]
fn decode_fixed_vertices<'a>(
    records: impl Iterator<Item = &'a [u8]>, count: usize, rgb: Option<[usize; 3]>,
) -> Result<(Vec<Point>, Option<Vec<Color>>)> {
    let mut points = Vec::with_capacity(count);
    let mut colors = rgb.map(|_| Vec::with_capacity(count));
    for record in records {
        let point = packed_position(record);
        if !point.iter().all(|n| n.is_finite()) { return Err("Non-finite PLY position"); }
        points.push(point);
        if let (Some([r, g, b]), Some(colors)) = (rgb, &mut colors) {
            colors.push([record[r], record[g], record[b], 255]);
        }
    }
    Ok((points, colors))
}

struct Faces {
    triangles: Vec<Triangle>,
    polygons: Vec<(usize, Vec<u32>)>,
}

fn read_faces(reader: &mut Reader, element: &Element, vertices: usize) -> Result<Faces> {
    let index = element.property("vertex_indices").or_else(|| element.property("vertex_index"))
        .ok_or("PLY face requires a vertex index list")?;
    let Layout::List(size, kind) = element.properties[index].layout else {
        return Err("PLY face indices must be a list");
    };
    if !kind.integer() { return Err("PLY face indices must be integers"); }
    if reader.encoding == Encoding::Little && element.properties.len() == 1 &&
        size.width() == 1 && kind.width() == 4
    {
        if let Some(triangles) = read_binary_triangles(reader, element.count, vertices)? {
            return Ok(Faces { triangles, polygons: Vec::new() });
        }
    }
    let mut result = Faces { triangles: Vec::with_capacity(element.count), polygons: Vec::new() };
    let mut corners = Vec::new();
    let mut triangle_count = 0;
    for _ in 0..element.count {
        for (i, property) in element.properties.iter().enumerate() {
            if i != index {
                reader.skip_property(property.layout)?;
                continue;
            }
            let count = reader.count(size)?;
            if !(3..=MAX_POLYGON).contains(&count) { return Err("Invalid PLY polygon size"); }
            triangle_count += count - 2;
            if triangle_count > MAX_RECORDS { return Err("PLY triangle count exceeds the limit"); }
            corners.clear();
            for _ in 0..count {
                let vertex = reader.value(kind)?;
                if vertex < 0.0 || vertex >= vertices as f64 { return Err("PLY face index out of range"); }
                corners.push(vertex as u32);
            }
            if let &[a, b, c] = corners.as_slice() {
                result.triangles.push([a, b, c]);
            } else {
                result.polygons.push((result.triangles.len(), corners.clone()));
            }
        }
    }
    Ok(result)
}

fn read_binary_triangles(reader: &mut Reader, count: usize, vertices: usize) -> Result<Option<Vec<Triangle>>> {
    let Some(bytes) = reader.bytes.get(..count * 13) else { return Ok(None); };
    let mut triangles = Vec::with_capacity(count);
    for row in bytes.chunks_exact(13) {
        if row[0] != 3 { return Ok(None); }
        let indices: [u32; 3] = bytemuck::pod_read_unaligned(&row[1..]);
        let indices = indices.map(u32::from_le);
        if indices.iter().any(|&i| i >= vertices as u32) { return Err("PLY face index out of range"); }
        triangles.push(indices);
    }
    reader.take(bytes.len())?;
    Ok(Some(triangles))
}

fn triangulate(faces: Faces, positions: &[Point]) -> Result<Vec<Triangle>> {
    if faces.polygons.is_empty() { return Ok(faces.triangles); }
    let count = faces.triangles.len() + faces.polygons.iter().map(|(_, corners)| corners.len() - 2).sum::<usize>();
    let mut output = Vec::with_capacity(count);
    let mut start = 0;
    for (end, corners) in faces.polygons {
        output.extend_from_slice(&faces.triangles[start..end]);
        super::obj::polygon(&corners, positions, &mut output)?;
        start = end;
    }
    output.extend_from_slice(&faces.triangles[start..]);
    Ok(output)
}

pub fn load(path: &Path) -> std::result::Result<Model, Box<dyn std::error::Error>> {
    let start = Instant::now();
    #[cfg(windows)]
    {
        let metadata = path.metadata()?;
        if metadata.is_file() && metadata.len() >= 4 * 1024 * 1024 {
            if let Ok((_file, bytes)) = super::obj::map_file(path) {
                return Ok(decode(&bytes, start.elapsed(), "mapped")?);
            }
        }
    }
    let bytes = std::fs::read(path)?;
    Ok(decode(&bytes, start.elapsed(), "buffered")?)
}

fn decode(bytes: &[u8], read: Duration, mode: &str) -> Result<Model> {
    let (encoding, elements, bytes) = header(bytes)?;
    let vertex = elements.iter().find(|element| element.name == "vertex").ok_or("Missing PLY vertices")?;
    if vertex.count == 0 { return Err("PLY contains no vertices"); }
    let gaussian = vertex.property("scale_0").is_some() && vertex.property("opacity").is_some();
    let mut reader = Reader { bytes, encoding };
    let mut positions = Vec::new();
    let mut colors = None;
    let mut faces = None;
    let mut vertex_time = std::time::Duration::ZERO;
    let mut face_time = std::time::Duration::ZERO;
    for element in &elements {
        reader.check_rows(element)?;
        let began = Instant::now();
        match element.name {
            "vertex" => {
                (positions, colors) = read_vertices(&mut reader, element)?;
                vertex_time = began.elapsed();
            }
            "face" if element.count > 0 => {
                faces = Some(read_faces(&mut reader, element, vertex.count)?);
                face_time = began.elapsed();
            }
            _ => {
                for _ in 0..element.count {
                    for property in &element.properties { reader.skip_property(property.layout)?; }
                }
            }
        }
    }
    let triangulate_start = Instant::now();
    let triangles = match faces {
        Some(faces) => triangulate(faces, &positions)?,
        None => Vec::new(),
    };
    if std::env::var_os("STEPFLASH_PROFILE").is_some() {
        eprintln!("PLY input={mode} file_ms={:.3} vertex_ms={:.3} face_ms={:.3} triangulate_ms={:.3}",
            read.as_secs_f64() * 1000.0, vertex_time.as_secs_f64() * 1000.0,
            face_time.as_secs_f64() * 1000.0, triangulate_start.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(Model { positions, colors, triangles, gaussian })
}
