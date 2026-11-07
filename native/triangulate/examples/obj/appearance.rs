use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::Instant,
};

use super::{
    distance2, finish_geometry, load_chunks, parse_index, polygon_slots, resolve_index, suffix_end,
    tagged_index, trim, Chunk, Faces, Obj, Point,
};

#[cfg(feature = "rayon")]
use rayon::prelude::*;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const NONE: u32 = u32::MAX;

#[derive(Default)]
pub(super) struct ChunkLook {
    pub uv: Vec<[f32; 2]>,
    corners: Vec<i32>,
    libraries: Vec<String>,
    changes: Vec<(usize, String)>,
}

impl ChunkLook {
    pub(super) fn directive(&mut self, line: &[u8], corners: usize) {
        if !line.starts_with(b"vt") && !line.starts_with(b"mtllib") && !line.starts_with(b"usemtl")
        {
            return;
        }
        let Some(end) = line.iter().position(u8::is_ascii_whitespace) else {
            return;
        };
        let command = &line[..end];
        let value = strip_comment(&line[end..]).trim_ascii();
        match command {
            b"vt" => {
                self.uv.push(read_uv(value).unwrap_or([f32::NAN; 2]));
            }
            b"mtllib" if !value.is_empty() => self
                .libraries
                .push(String::from_utf8_lossy(value).into_owned()),
            b"usemtl" => self
                .changes
                .push((corners, String::from_utf8_lossy(value).into_owned())),
            _ => (),
        }
    }

    pub(super) fn push_uv(&mut self, corners: usize, uv: i32) {
        if uv != 0 && self.corners.is_empty() {
            self.corners.resize(corners, 0);
        }
        if uv != 0 || !self.corners.is_empty() {
            self.corners.push(uv);
        }
    }
}

fn read_uv(mut input: &[u8]) -> Option<[f32; 2]> {
    let mut result = [0.0; 2];
    for (axis, value) in result.iter_mut().enumerate() {
        if axis == 1 && input.is_empty() {
            break;
        }
        let (number, length) = fast_float::parse_partial::<f32, _>(input).ok()?;
        input = &input[length..];
        if input
            .first()
            .is_some_and(|byte| !byte.is_ascii_whitespace())
        {
            return None;
        }
        *value = number;
        input = trim(input);
    }
    Some(result)
}

pub(super) fn corner(
    bytes: &[u8],
    positions: usize,
    uvs: usize,
) -> super::Result<(i32, i32, &[u8])> {
    let (number, suffix) = parse_index(bytes)?;
    let position = tagged_index(number, positions)?;
    if suffix.first() != Some(&b'/') {
        return Ok((position, 0, trim(suffix)));
    }
    let field = &suffix[1..];
    if !field.is_empty() && field[0] != b'/' {
        if let Ok((index, after)) = parse_index(field) {
            let uv = tagged_index(index, uvs).unwrap_or(0);
            let next = if after.first() == Some(&b'/') {
                &after[suffix_end(after)..]
            } else {
                after
            };
            return Ok((position, uv, trim(next)));
        }
    }
    let end = suffix_end(suffix);
    Ok((position, 0, trim(&suffix[end..])))
}

#[derive(Default)]
pub struct Material {
    pub color: Option<[f32; 4]>,
    pub texture: Option<Texture>,
}

pub struct Texture {
    pub path: PathBuf,
    pub clamp: bool,
    pub transform: [f32; 4],
}

pub type CornerTriangle = [[u32; 2]; 3];

pub struct Part {
    pub material: usize,
    pub batches: Vec<Vec<CornerTriangle>>,
    pub textured: bool,
}

pub enum Model {
    Geometry(Obj, usize),
    Styled {
        points: Vec<Point>,
        uv: Vec<[f32; 2]>,
        parts: Vec<Part>,
        materials: Vec<Material>,
        issues: usize,
    },
}

fn strip_comment(bytes: &[u8]) -> &[u8] {
    let mut quoted = false;
    for (index, &byte) in bytes.iter().enumerate() {
        match byte {
            b'"' => quoted = !quoted,
            b'#' if !quoted => return &bytes[..index],
            _ => (),
        }
    }
    bytes
}

fn tokens(mut value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    while !value.trim_start().is_empty() {
        value = value.trim_start();
        if let Some(rest) = value.strip_prefix('"') {
            if let Some((word, tail)) = rest.split_once('"') {
                parts.push(word);
                value = tail;
                continue;
            }
        }
        let end = value.find(char::is_whitespace).unwrap_or(value.len());
        parts.push(&value[..end]);
        value = &value[end..];
    }
    parts
}

fn companion(root: &Path, directory: &Path, name: &str) -> Option<PathBuf> {
    let name = name.trim_matches('"').replace('\\', "/");
    let path = Path::new(&name);
    if name.is_empty() || name.contains(':') || path.is_absolute() {
        return None;
    }
    let path = directory.join(path).canonicalize().ok()?;
    (path.starts_with(root) && path.is_file()).then_some(path)
}

fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    let length = file.metadata()?.len();
    if length > limit {
        return Err("OBJ companion file exceeds the size limit".into());
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("OBJ companion file exceeds the size limit".into());
    }
    Ok(bytes)
}

pub fn image_bytes(path: &Path) -> Result<Vec<u8>> {
    read_file(path, 64 * 1024 * 1024)
}

fn texture(
    root: &Path,
    directory: &Path,
    value: &str,
    missing: &mut HashSet<(PathBuf, String)>,
) -> std::result::Result<Option<Texture>, ()> {
    let words = tokens(value);
    let mut i = 0;
    let mut scale = [1.0; 2];
    let mut offset = [0.0; 2];
    let mut clamp = false;
    while let Some(option) = words
        .get(i)
        .copied()
        .filter(|option| option.starts_with('-'))
    {
        i += 1;
        match option {
            "-s" | "-o" => {
                let output = if option == "-s" {
                    &mut scale
                } else {
                    &mut offset
                };
                let mut read = 0;
                while read < 3 {
                    let Some(number) = words
                        .get(i)
                        .and_then(|word| word.parse::<f32>().ok())
                        .filter(|number| number.is_finite())
                    else {
                        break;
                    };
                    if read < 2 {
                        output[read] = number;
                    }
                    read += 1;
                    i += 1;
                }
                if read == 0 {
                    return Err(());
                }
            }
            "-clamp" => {
                clamp = match *words.get(i).ok_or(())? {
                    "on" => true,
                    "off" => false,
                    _ => return Err(()),
                };
                i += 1;
            }
            "-blendu" | "-blendv" | "-imfchan" | "-type" | "-texres" | "-boost" | "-cc" => i += 1,
            "-mm" => i += 2,
            _ => return Err(()),
        }
    }
    let name = words.get(i..).ok_or(())?.join(" ");
    if name.is_empty() {
        return Err(());
    }
    let Some(path) = companion(root, directory, &name) else {
        missing.insert((directory.to_owned(), name));
        return Ok(None);
    };
    Ok(Some(Texture {
        path,
        clamp,
        transform: [scale[0], scale[1], offset[0], offset[1]],
    }))
}

fn libraries(
    path: &Path,
    chunks: &[Chunk],
) -> Result<(Vec<Material>, HashMap<String, usize>, usize)> {
    let mut palette = vec![Material::default()];
    let mut names = HashMap::new();
    let mut issues = 0;
    if chunks.iter().all(|chunk| chunk.look.libraries.is_empty()) {
        return Ok((palette, names, issues));
    }
    let root = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    let mut loaded = HashSet::new();
    let mut missing = HashSet::new();
    let mut declarations = HashSet::new();
    for declaration in chunks.iter().flat_map(|chunk| &chunk.look.libraries) {
        if !declarations.insert(declaration) {
            continue;
        }
        let paths = if let Some(path) = companion(&root, &root, declaration) {
            vec![Some(path)]
        } else {
            tokens(declaration)
                .into_iter()
                .map(|name| companion(&root, &root, name))
                .collect()
        };
        for library in paths {
            let Some(library) = library else {
                issues += 1;
                continue;
            };
            if !loaded.insert(library.clone()) {
                continue;
            }
            let Ok(bytes) = read_file(&library, 8 * 1024 * 1024) else {
                issues += 1;
                continue;
            };
            let directory = library.parent().unwrap_or(&root);
            let source = String::from_utf8_lossy(&bytes);
            let mut current = None;
            for line in source.lines() {
                let line = std::str::from_utf8(strip_comment(line.as_bytes()))?
                    .trim_start_matches('\u{feff}')
                    .trim();
                let Some((command, value)) = line.split_once(char::is_whitespace) else {
                    continue;
                };
                let value = value.trim();
                if command == "newmtl" && !value.is_empty() {
                    let id = palette.len();
                    names.insert(value.to_owned(), id);
                    palette.push(Material::default());
                    current = Some(id);
                } else if let Some(id) = current {
                    let material = &mut palette[id];
                    match command {
                        "Kd" => {
                            let mut values = value.split_ascii_whitespace().map(str::parse::<f32>);
                            match (values.next(), values.next(), values.next()) {
                                (Some(Ok(r)), Some(Ok(g)), Some(Ok(b)))
                                    if [r, g, b].iter().all(|v| v.is_finite()) =>
                                {
                                    material.color = Some([
                                        r.clamp(0.0, 1.0),
                                        g.clamp(0.0, 1.0),
                                        b.clamp(0.0, 1.0),
                                        1.0,
                                    ]);
                                }
                                _ => issues += 1,
                            }
                        }
                        "map_Kd" => {
                            material.texture = match texture(&root, directory, value, &mut missing)
                            {
                                Ok(map) => map,
                                Err(()) => {
                                    issues += 1;
                                    None
                                }
                            };
                        }
                        _ => (),
                    }
                }
            }
        }
    }
    Ok((palette, names, issues + missing.len()))
}

struct Work {
    chunk: Chunk,
    base: usize,
    uv_base: usize,
    initial: usize,
    changes: Vec<(usize, usize)>,
}

struct Piece {
    material: usize,
    triangles: Vec<CornerTriangle>,
    textured: bool,
}

fn pieces(
    work: Work,
    points: &[Point],
    uvs: &[[f32; 2]],
    materials: &[Material],
) -> super::Result<(Vec<Piece>, bool)> {
    let Work {
        chunk,
        base,
        uv_base,
        initial,
        changes,
    } = work;
    let mut pieces = Vec::<Piece>::new();
    let mut lookup = vec![[usize::MAX; 2]; materials.len()];
    let mut material = initial;
    let mut changes = changes.into_iter().peekable();
    let mut start = 0;
    let mut positions = Vec::with_capacity(4);
    let mut coordinates = Vec::with_capacity(4);
    let mut missing_uv = false;
    let mut append = |face: &[i32]| -> super::Result<()> {
        while changes.peek().is_some_and(|(corner, _)| *corner <= start) {
            material = changes.next().unwrap().1;
        }
        positions.clear();
        for &position in face {
            positions.push(resolve_index(position, base, points.len())?);
        }
        let textured = materials[material].texture.is_some();
        coordinates.clear();
        if textured {
            for corner in start..start + face.len() {
                let uv = chunk
                    .look
                    .corners
                    .get(corner)
                    .copied()
                    .filter(|&uv| uv != 0)
                    .and_then(|uv| resolve_index(uv, uv_base, uvs.len()).ok())
                    .filter(|&uv| uvs[uv as usize].iter().all(|value| value.is_finite()));
                coordinates.push(uv.unwrap_or(NONE));
            }
        }
        start += face.len();
        let mut emit = |corners: [usize; 3]| {
            let mapped = textured
                .then(|| corners.map(|corner| coordinates[corner]))
                .filter(|indices| !indices.contains(&NONE));
            missing_uv |= textured && mapped.is_none();
            let slot = &mut lookup[material][usize::from(mapped.is_some())];
            if *slot == usize::MAX {
                *slot = pieces.len();
                pieces.push(Piece {
                    material,
                    triangles: Vec::new(),
                    textured: mapped.is_some(),
                });
            }
            let piece = &mut pieces[*slot];
            let uv = mapped.unwrap_or([0; 3]);
            piece
                .triangles
                .push(std::array::from_fn(|i| [positions[corners[i]], uv[i]]));
        };
        match positions.as_slice() {
            [_, _, _] => emit([0, 1, 2]),
            [a, b, c, d] => {
                if distance2(points[*a as usize], points[*c as usize])
                    < distance2(points[*b as usize], points[*d as usize])
                {
                    emit([0, 1, 2]);
                    emit([0, 2, 3]);
                } else {
                    emit([0, 1, 3]);
                    emit([1, 2, 3]);
                }
            }
            _ => polygon_slots(&positions, points, emit)?,
        }
        Ok(())
    };
    match &chunk.faces {
        Faces::Empty => (),
        Faces::Uniform(size) => {
            for face in chunk.corners.chunks_exact(*size as usize) {
                append(face)?;
            }
        }
        Faces::Mixed(sizes) => {
            let mut corners = chunk.corners.as_slice();
            for &size in sizes {
                let (face, rest) = corners.split_at(size as usize);
                append(face)?;
                corners = rest;
            }
        }
    }
    Ok((pieces, missing_uv))
}

pub fn load(path: &Path) -> Result<Model> {
    let start = Instant::now();
    let (chunks, mode) = load_chunks::<true>(path)?;
    let decoded = start.elapsed();
    let (materials, names, mut issues) = libraries(path, &chunks)?;
    let mut unknown = HashSet::new();
    let mut active = 0;
    if names.is_empty() || chunks.iter().all(|chunk| chunk.look.changes.is_empty()) {
        return Ok(Model::Geometry(
            finish_geometry(chunks, mode, start)?,
            issues,
        ));
    }
    let textured = materials.iter().any(|material| material.texture.is_some());
    let mut points = Vec::with_capacity(chunks.iter().map(|chunk| chunk.positions.len()).sum());
    let mut uv = Vec::new();
    let mut work = Vec::with_capacity(chunks.len());
    for mut chunk in chunks {
        let base = points.len();
        let uv_base = uv.len();
        points.append(&mut chunk.positions);
        if textured {
            uv.append(&mut chunk.look.uv);
        } else {
            chunk.look.corners = Vec::new();
            chunk.look.uv = Vec::new();
        }
        let changes: Vec<_> = std::mem::take(&mut chunk.look.changes)
            .into_iter()
            .map(|(corner, name)| {
                let index = names.get(&name).copied().unwrap_or_else(|| {
                    if !name.is_empty() && name != "off" {
                        unknown.insert(name);
                    }
                    0
                });
                (corner, index)
            })
            .collect();
        let initial = active;
        active = changes.last().map_or(active, |(_, index)| *index);
        work.push(Work {
            chunk,
            base,
            uv_base,
            initial,
            changes,
        });
    }
    issues += unknown.len();
    let merged = start.elapsed();
    #[cfg(feature = "rayon")]
    let iter = work.into_par_iter();
    #[cfg(not(feature = "rayon"))]
    let iter = work.into_iter();
    let results = iter
        .map(|work| pieces(work, &points, &uv, &materials))
        .collect::<super::Result<Vec<_>>>()?;
    let triangulated = start.elapsed();
    let mut grouped: Vec<[Vec<Vec<CornerTriangle>>; 2]> =
        (0..materials.len()).map(|_| Default::default()).collect();
    let mut missing_uv = false;
    for (pieces, missing) in results {
        missing_uv |= missing;
        for piece in pieces {
            let material = piece.material;
            let kind = usize::from(piece.textured);
            grouped[material][kind].push(piece.triangles);
        }
    }
    issues += usize::from(missing_uv);
    let mut parts = Vec::new();
    for (id, [plain, textured]) in grouped.into_iter().enumerate() {
        if !plain.is_empty() {
            parts.push(Part {
                material: id,
                batches: plain,
                textured: false,
            });
        }
        if !textured.is_empty() {
            parts.push(Part {
                material: id,
                batches: textured,
                textured: true,
            });
        }
    }
    if parts.is_empty() {
        return Err("OBJ has no polygon faces".into());
    }
    if std::env::var_os("STEPFLASH_PROFILE").is_some() {
        let ms = |time: std::time::Duration| time.as_secs_f64() * 1000.0;
        eprintln!("OBJ appearance phases: input={}, decode={:.2} ms, materials_and_merge={:.2} ms, triangulate={:.2} ms, group={:.2} ms; parts={}, issues={}",
            mode, ms(decoded), ms(merged-decoded), ms(triangulated-merged), ms(start.elapsed()-triangulated), parts.len(), issues);
    }
    Ok(Model::Styled {
        points,
        uv,
        parts,
        materials,
        issues,
    })
}
