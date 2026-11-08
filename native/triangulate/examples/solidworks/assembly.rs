use std::{
    collections::{HashMap, HashSet},
    convert::TryInto,
    fs,
    path::{Path, PathBuf},
};

use memchr::memmem;
use quick_xml::{
    encoding::Decoder,
    events::{BytesStart, Event},
    Reader, XmlVersion,
};

use super::{
    load_part, load_part_appearance, tables, word, Blocks, Mesh, MeshBuilder, Result, MAX_TRIANGLES,
};

const INSTANCE_TREE: &str = "swXmlContents/COMPINSTANCETREE";
const CACHE_DIRECTORY: &str = "FaceTessellations/Directory";
const STRING_MARKER: &[u8] = b"\xff\xfe\xff";
const MAX_COMPONENTS: usize = 2000;
const MAX_REFERENCES: usize = 16_000;
const MAX_DEPTH: usize = 8;
const CACHE_WELD_INVERSE: f64 = 1.0 / 1e-9;

type Matrix = [[f64; 4]; 4];
type Key = [u8; 8];
type Transform = ([[f32; 4]; 3], bool);
const IDENTITY: Matrix = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

pub struct PlacedMesh {
    pub mesh: Mesh,
    pub instances: Vec<Transform>,
}

#[derive(Default)]
pub struct Assembly {
    pub meshes: Vec<PlacedMesh>,
    pub issues: usize,
    pub snapshot: bool,
    pub diagnostics: Vec<String>,
}

#[derive(Default)]
struct Tree {
    files: HashMap<String, ModelFile>,
    models: HashMap<String, Model>,
    root: Option<String>,
    recent: bool,
    display_states: HashMap<String, Vec<String>>,
}

struct ModelFile {
    assembly: bool,
    name: String,
    original: PathBuf,
}

struct Model {
    file: String,
    name: String,
    configuration: String,
    references: Vec<Reference>,
}

struct Reference {
    model: String,
    name: String,
    number: String,
    placement: Option<(Matrix, bool)>,
    suppressed: bool,
    hidden: bool,
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn placement(text: &str) -> Option<(Matrix, bool)> {
    let mut values = text.split_ascii_whitespace().map(str::parse::<f64>);
    let mut matrix = [[0.0; 4]; 4];
    for value in matrix.iter_mut().flatten() {
        *value = values.next()?.ok()?;
        if !value.is_finite() {
            return None;
        }
    }
    if values.next().is_some()
        || matrix[..3].iter().any(|row| row[3].abs() > 1e-12)
        || (matrix[3][3] - 1.0).abs() > 1e-12
    {
        return None;
    }
    let [x, y, z, _] = matrix;
    let cross = [
        y[1] * z[2] - y[2] * z[1],
        y[2] * z[0] - y[0] * z[2],
        y[0] * z[1] - y[1] * z[0],
    ];
    let det = dot(&x[..3], &cross);
    let nearly_zero = |value: f64| value.is_finite() && value.abs() <= 1e-6;
    if !nearly_zero(det.abs() - 1.0)
        || ![x, y, z]
            .iter()
            .all(|axis| nearly_zero(dot(&axis[..3], &axis[..3]) - 1.0))
        || [(x, y), (x, z), (y, z)]
            .iter()
            .any(|(a, b)| !nearly_zero(dot(&a[..3], &b[..3])))
    {
        return None;
    }
    Some((matrix, det < 0.0))
}

fn compose(local: Matrix, parent: Matrix) -> Matrix {
    std::array::from_fn(|row| {
        std::array::from_fn(|col| (0..4).map(|k| local[row][k] * parent[k][col]).sum())
    })
}

impl Tree {
    fn parse(bytes: &[u8]) -> Result<Self> {
        let mut tree = Self::default();
        let mut xml = Reader::from_reader(bytes);
        let mut path: Vec<Vec<u8>> = Vec::new();
        let mut model = None;
        let mut configuration = None;
        let mut references = 0;
        loop {
            let event = xml.read_event()?;
            let opening = matches!(&event, Event::Start(_));
            match event {
                Event::Start(element) | Event::Empty(element) => {
                    let local = element.local_name();
                    let tag = local.as_ref();
                    let parent = path.last().map(Vec::as_slice);
                    match (path.len(), parent, tag) {
                        (2, Some(b"swHeader"), b"swFile") => {
                            let attrs = attributes(&element, xml.decoder())?;
                            let filename = get(&attrs, b"swPath")
                                .rsplit(['/', '\\'])
                                .next()
                                .unwrap_or("");
                            tree.files.insert(
                                get(&attrs, b"id").into(),
                                ModelFile {
                                    assembly: get(&attrs, b"swDocType") == "ASSEMBLY",
                                    name: filename.to_lowercase(),
                                    original: PathBuf::from(get(&attrs, b"swPath")),
                                },
                            );
                        }
                        (2, Some(b"swModelList"), b"swModel") => {
                            let attrs = attributes(&element, xml.decoder())?;
                            let id = get(&attrs, b"id").to_owned();
                            tree.models.insert(
                                id.clone(),
                                Model {
                                    file: get(&attrs, b"swFileRef").into(),
                                    name: get(&attrs, b"swName").into(),
                                    configuration: get(&attrs, b"swConfigurationName").into(),
                                    references: Vec::new(),
                                },
                            );
                            model = opening.then_some(id);
                        }
                        (3, Some(b"swModel"), b"swReference") => {
                            if let Some(current) =
                                model.as_ref().and_then(|id| tree.models.get_mut(id))
                            {
                                let attrs = attributes(&element, xml.decoder())?;
                                current.references.push(Reference {
                                    model: get(&attrs, b"swModelRef").into(),
                                    name: get(&attrs, b"swName").into(),
                                    number: get(&attrs, b"swReferenceNumber").into(),
                                    placement: placement(get(&attrs, b"swTransform")),
                                    suppressed: get(&attrs, b"swSuppressed") == "YES",
                                    hidden: get(&attrs, b"swHidden") == "YES",
                                });
                                references += 1;
                            }
                        }
                        (2, Some(b"swConfigurationList"), b"swConfiguration") => {
                            let attrs = attributes(&element, xml.decoder())?;
                            let recent = get(&attrs, b"swMostRecentConfiguration") == "YES";
                            configuration = opening.then(|| get(&attrs, b"swModelRef").to_owned());
                            if tree.root.is_none() || (recent && !tree.recent) {
                                tree.root = Some(get(&attrs, b"swModelRef").into());
                                tree.recent = recent;
                            }
                        }
                        (4, Some(b"swDisplayStateList"), b"swDisplayState") => {
                            let attrs = attributes(&element, xml.decoder())?;
                            if let Some(root) = &configuration {
                                tree.display_states
                                    .entry(root.clone())
                                    .or_default()
                                    .push(get(&attrs, b"swDisplayStateName").to_owned());
                            }
                        }
                        _ => {}
                    }
                    if opening {
                        path.push(tag.to_vec());
                    }
                    if path.len() > 64
                        || references > MAX_REFERENCES
                        || tree.files.len() > MAX_REFERENCES
                        || tree.models.len() > MAX_REFERENCES
                    {
                        return Err("SolidWorks instance tree exceeds the decode limits".into());
                    }
                }
                Event::End(_) => {
                    if path.pop().is_none() {
                        return Err("Invalid SolidWorks instance tree".into());
                    }
                    if path.len() == 2 {
                        model = None;
                        configuration = None;
                    }
                }
                Event::Eof if path.is_empty() => return Ok(tree),
                Event::Eof => return Err("Truncated SolidWorks instance tree".into()),
                _ => {}
            }
        }
    }
}

type Attributes = HashMap<Vec<u8>, String>;

fn attributes(element: &BytesStart<'_>, decoder: Decoder) -> Result<Attributes> {
    element
        .attributes()
        .map(|attribute| {
            let attribute = attribute?;
            Ok((
                attribute.key.as_ref().to_vec(),
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)?
                    .into_owned(),
            ))
        })
        .collect()
}

fn get<'a>(attrs: &'a Attributes, name: &[u8]) -> &'a str {
    attrs.get(name).map_or("", String::as_str)
}

struct Entry {
    name: String,
    alias: Key,
    id: Key,
    chunk: String,
    index: u32,
}

fn take<'a>(bytes: &mut &'a [u8], count: usize) -> Option<&'a [u8]> {
    let (head, tail) = bytes.split_at_checked(count)?;
    *bytes = tail;
    Some(head)
}

fn utf16_string(bytes: &mut &[u8]) -> Option<String> {
    *bytes = bytes.strip_prefix(STRING_MARKER)?;
    let mut length = usize::from(take(bytes, 1)?[0]);
    if length == 0xff {
        let size: [u8; 2] = take(bytes, 2)?.try_into().ok()?;
        length = usize::from(u16::from_le_bytes(size));
        if length == 0xffff {
            length = word(take(bytes, 4)?) as usize;
        }
    }
    if length > 4096 {
        return None;
    }
    let words = take(bytes, length * 2)?
        .chunks_exact(2)
        .map(|w| u16::from_le_bytes([w[0], w[1]]));
    char::decode_utf16(words)
        .collect::<std::result::Result<String, _>>()
        .ok()
}

fn directory(mut bytes: &[u8]) -> Option<Vec<Entry>> {
    let version = word(take(&mut bytes, 4)?);
    let capacity = word(take(&mut bytes, 4)?);
    let count = word(take(&mut bytes, 4)?) as usize;
    if version != 2 || !(1..=100_000).contains(&capacity) || !(1..=MAX_COMPONENTS).contains(&count)
    {
        return None;
    }
    let mut entries = Vec::new();
    let Some(start) = memmem::find(bytes, STRING_MARKER).and_then(|index| index.checked_sub(4))
    else {
        return Some(entries);
    };
    bytes = &bytes[start..];
    let sequence = word(bytes);
    let mut marker = [0; 7];
    marker[4..].copy_from_slice(STRING_MARKER);
    for i in 0..count {
        marker[..4].copy_from_slice(&sequence.checked_add(i as u32)?.to_le_bytes());
        let Some(next) = memmem::find(bytes, &marker) else {
            break;
        };
        bytes = &bytes[next + 4..];
        let name = utf16_string(&mut bytes)?;
        take(&mut bytes, 12)?;
        let alias = take(&mut bytes, 8)?.try_into().ok()?;
        let id = take(&mut bytes, 8)?.try_into().ok()?;
        for _ in 0..4 {
            if bytes.starts_with(STRING_MARKER) {
                break;
            }
            take(&mut bytes, 8)?;
        }
        let chunk = utf16_string(&mut bytes)?;
        // Leave the last word searchable for the next sequence marker.
        let index = word(bytes.get(..4)?);
        entries.push(Entry {
            name,
            alias,
            id,
            chunk,
            index,
        });
    }
    Some(entries)
}

fn cached_meshes(blocks: &Blocks<'_>, entries: &[Entry]) -> Result<HashMap<Key, Mesh>> {
    let mut chunks: HashMap<&str, Vec<&Entry>> = HashMap::new();
    for entry in entries.iter().filter(|entry| !entry.chunk.is_empty()) {
        chunks.entry(&entry.chunk).or_default().push(entry);
    }
    let mut stored = HashMap::new();
    let mut triangles = 0;
    for (chunk, entries) in chunks {
        let Some(block) = blocks.get(&format!("FaceTessellations/{chunk}")) else {
            continue;
        };
        let Ok(data) = block.expand() else {
            continue;
        };
        let mut records = Vec::new();
        for entry in entries {
            let mut marker = [0; 12];
            marker[..4].copy_from_slice(&entry.index.to_le_bytes());
            marker[4..].copy_from_slice(&entry.id);
            let Some(offset) = memmem::find(&data, &marker) else {
                continue;
            };
            let Some(count) = data.get(offset + 12..offset + 16).map(word) else {
                continue;
            };
            if memmem::find(&data[offset + 1..], &marker).is_none()
                && (1..=250_000).contains(&count)
            {
                records.push((offset, count as usize, entry.id));
            }
        }
        records.sort_unstable_by_key(|record| record.0);
        for (index, &(start, count, id)) in records.iter().enumerate() {
            let end = records.get(index + 1).map_or(data.len(), |record| record.0);
            let Some(bytes) = data.get(start + 16..end) else {
                continue;
            };
            let tables: Vec<_> = tables(bytes).take(count).collect();
            let ends: Vec<_> = tables.iter().map(|table| table.start).collect();
            let mut builder = MeshBuilder::default();
            let mut decoded = 0;
            for (index, table) in tables.into_iter().enumerate() {
                let end = ends.get(index + 1).copied().unwrap_or(bytes.len());
                let references = super::appearance::surface_references(&bytes[table.end..end]);
                builder.add_display_table(table, CACHE_WELD_INVERSE, None, references)?;
                decoded += 1;
            }
            if decoded == count && !builder.mesh.triangles.is_empty() {
                triangles += builder.mesh.triangles.len();
                if triangles > MAX_TRIANGLES {
                    return Err("SolidWorks mesh cache exceeds the triangle limit".into());
                }
                stored.insert(id, builder.mesh);
            }
        }
    }
    Ok(stored)
}

fn siblings(path: &Path) -> HashMap<String, Option<PathBuf>> {
    let folder = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut parts = HashMap::new();
    for entry in fs::read_dir(folder).into_iter().flatten().flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.ends_with(".sldprt") {
            parts
                .entry(name)
                .and_modify(|path| *path = None)
                .or_insert_with(|| Some(entry.path()));
        }
    }
    parts
}

struct Rebuilder<'a> {
    tree: &'a Tree,
    entries: HashMap<&'a str, &'a Entry>,
    ids: HashMap<Key, &'a Entry>,
    stored: HashMap<Key, Mesh>,
    paths: HashMap<String, Option<PathBuf>>,
    cached: HashMap<Key, usize>,
    external: HashMap<String, Option<usize>>,
    scene: Assembly,
    triangles: usize,
    visited: usize,
    appearances: HashMap<(String, String), Option<super::appearance::SourceAppearance>>,
    variants: HashMap<(usize, String, String, bool), usize>,
    component_colors: HashMap<String, super::appearance::Color>,
    component_variants: HashMap<(usize, super::appearance::Color), usize>,
    source_visibility: bool,
}

// Only placement-only assemblies may defer to current part-body visibility.
// Unknown root features conservatively keep the assembly's own cached mesh.
fn placement_only(blocks: &Blocks<'_>) -> Option<bool> {
    let bytes = blocks.get("swXmlContents/KeyWords")?.expand().ok()?;
    let start = bytes.iter().position(|b| *b == b'<')?;
    let mut xml = Reader::from_reader(&bytes[start..]);
    let mut depth = 0usize;
    loop {
        let event = xml.read_event().ok()?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                if depth == 1 {
                    let attrs = attributes(e, xml.decoder()).ok()?;
                    let allowed = match e.name().as_ref() {
                        b"Configuration" | b"Reference" => true,
                        b"Sketch" => get(&attrs, b"Name") == "Origin",
                        b"Feature" => matches!(
                            get(&attrs, b"Type"),
                            "Exploded Views"
                                | "Live Section Planes"
                                | "Mates"
                                | "Ambient"
                                | "Directional"
                                | "Equations"
                                | "Notes"
                                | "Markups"
                                | "Sensors"
                                | "Favorites"
                                | "History"
                                | "Selection Sets"
                                | "Annotations"
                                | "PLANE"
                                | "Folder"
                                | "Lights and Cameras"
                                | "Design Binder"
                                | "Comments"
                        ),
                        _ => false,
                    };
                    if !allowed {
                        return Some(false);
                    }
                }
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                }
            }
            Event::End(_) => depth = depth.checked_sub(1)?,
            Event::Eof => return Some(depth == 0),
            _ => (),
        }
    }
}

type GeometricTriangle = [[i64; 3]; 3];
fn geometric_triangle(mesh: &Mesh, triangle: &super::Triangle) -> Option<GeometricTriangle> {
    let mut points = [[0; 3]; 3];
    for (point, index) in points.iter_mut().zip(triangle) {
        for (a, value) in mesh.positions.get(*index as usize)?.iter().enumerate() {
            let n = (f64::from(*value) * 10_000.0).round_ties_even();
            if !n.is_finite() || n.abs() > i64::MAX as f64 {
                return None;
            }
            point[a] = n as i64;
        }
    }
    points.sort_unstable();
    if points[0] == points[1] || points[1] == points[2] {
        return None;
    }
    Some(points)
}

// Exact quantized triangle containment plus closed-manifold topology avoids
// substituting a partial display, a stale part, or an assembly-cut surface.
fn closed_visible_subset(visible: &Mesh, cached: &Mesh) -> bool {
    if visible.triangles.is_empty() || visible.triangles.len() >= cached.triangles.len() {
        return false;
    }
    let cached: HashSet<_> = cached
        .triangles
        .iter()
        .filter_map(|t| geometric_triangle(cached, t))
        .collect();
    let mut edges = HashMap::<([i64; 3], [i64; 3]), usize>::new();
    for t in &visible.triangles {
        let Some(p) = geometric_triangle(visible, t) else {
            return false;
        };
        if !cached.contains(&p) {
            return false;
        }
        for edge in [(p[0], p[1]), (p[1], p[2]), (p[0], p[2])] {
            *edges.entry(edge).or_default() += 1;
        }
    }
    edges.values().all(|n| *n == 2)
}

impl Rebuilder<'_> {
    fn issue(&mut self, message: String) {
        self.scene.issues += 1;
        if self.scene.diagnostics.len() < 100 && !self.scene.diagnostics.contains(&message) {
            self.scene.diagnostics.push(message);
        }
    }

    fn part_path(&self, file: &ModelFile) -> Option<PathBuf> {
        match self.paths.get(&file.name) {
            Some(path) => path.clone(), // Ambiguous siblings must not silently pick one.
            None => {
                // Resolve an existing original local reference as well as packed
                // siblings. Never initiate a network lookup from a CAD reference.
                let path = &file.original;
                let text = path.to_string_lossy();
                (path.is_absolute()
                    && !text.starts_with("\\\\")
                    && !text.starts_with("//")
                    && path.is_file())
                .then(|| path.clone())
            }
        }
    }

    fn colored_cached(
        &mut self,
        index: usize,
        file: &ModelFile,
        configuration: &str,
        source_visibility: bool,
    ) -> usize {
        let key = (file.name.clone(), configuration.to_owned());
        let variant_key = (index, key.0.clone(), key.1.clone(), source_visibility);
        if let Some(&variant) = self.variants.get(&variant_key) {
            return variant;
        }
        if !self.appearances.contains_key(&key) {
            let appearance = self
                .part_path(file)
                .and_then(|path| load_part_appearance(&path, configuration));
            self.appearances.insert(key.clone(), appearance);
        }
        let appearance = self.appearances.get(&key).and_then(Option::as_ref);
        if source_visibility {
            if let Some(visible) = appearance
                .and_then(|a| a.visible_mesh.as_ref())
                .filter(|visible| closed_visible_subset(visible, &self.scene.meshes[index].mesh))
            {
                let mesh = visible.clone();
                let variant = self.add_mesh(mesh);
                self.variants.insert(variant_key, variant);
                self.scene.diagnostics.push(format!("Respected saved visible bodies from {}; omitted extra assembly-cache body geometry.",file.name));
                return variant;
            }
        }
        let color = appearance.and_then(|appearance| appearance.color);
        let mut incomplete_decals = appearance
            .and_then(|a| a.visible_mesh.as_ref())
            .is_some_and(|m| m.incomplete_decals);
        let decals = appearance
            .and_then(|a| a.visible_mesh.as_ref())
            .map(|source| super::decals::matched(source, &self.scene.meshes[index].mesh))
            .unwrap_or_default();
        incomplete_decals |= appearance
            .and_then(|a| a.visible_mesh.as_ref())
            .is_some_and(|m| m.decals.len() != decals.len());
        let face_colors: Vec<_> = self.scene.meshes[index]
            .mesh
            .face_references
            .iter()
            .filter_map(|(range, refs)| {
                appearance
                    .and_then(|appearance| appearance.matched_color(refs))
                    .map(|color| (range.clone(), color))
            })
            .collect();
        if std::env::var_os("STEPFLASH_PROFILE").is_some() {
            eprintln!(
                "SolidWorks cached face appearances: file={}, matched={}/{}",
                file.name,
                face_colors.len(),
                self.scene.meshes[index].mesh.face_references.len()
            );
        }
        if !self
            .variants
            .keys()
            .any(|(source, _, _, _)| *source == index)
        {
            self.scene.meshes[index].mesh.color = color;
            self.scene.meshes[index].mesh.face_colors = face_colors;
            self.scene.meshes[index].mesh.decals = decals;
            self.scene.meshes[index].mesh.incomplete_decals = incomplete_decals;
            self.variants.insert(variant_key, index);
            return index;
        }
        let mut mesh = self.scene.meshes[index].mesh.clone();
        mesh.color = color;
        mesh.face_colors = face_colors;
        mesh.decals = decals;
        mesh.incomplete_decals = incomplete_decals;
        let variant = self.add_mesh(mesh);
        self.variants.insert(variant_key, variant);
        variant
    }
    fn add_mesh(&mut self, mesh: Mesh) -> usize {
        let index = self.scene.meshes.len();
        self.scene.meshes.push(PlacedMesh {
            mesh,
            instances: Vec::new(),
        });
        index
    }

    fn cached_index(&mut self, name: &str) -> Option<usize> {
        let mut entry = *self.entries.get(name)?;
        for _ in 0..self.ids.len() {
            if !entry.chunk.is_empty() {
                if let Some(&index) = self.cached.get(&entry.id) {
                    return Some(index);
                }
                let mesh = self.stored.remove(&entry.id)?;
                let index = self.add_mesh(mesh);
                self.cached.insert(entry.id, index);
                return Some(index);
            }
            entry = *self.ids.get(&entry.alias)?;
        }
        None
    }

    fn part_index(&mut self, file: &ModelFile) -> Option<usize> {
        if let Some(index) = self.external.get(&file.name) {
            return *index;
        }
        let mesh = self
            .part_path(file)
            .and_then(|path| match load_part(&path) {
                Ok(mesh) => Some(mesh),
                Err(error) => {
                    let message = format!("Cannot read {}: {error}", file.name);
                    if self.scene.diagnostics.len() < 100 {
                        self.scene.diagnostics.push(message);
                    }
                    None
                }
            });
        let index = mesh.map(|mesh| self.add_mesh(mesh));
        self.external.insert(file.name.clone(), index);
        index
    }

    fn visit(
        &mut self,
        model: &Model,
        prefix: &str,
        parent: (Matrix, bool),
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            self.issue(format!("Subassembly depth limit reached: {prefix}"));
            return Ok(());
        }
        let tree = self.tree;
        for reference in &model.references {
            self.visited += 1;
            if self.visited > MAX_REFERENCES {
                return Err("SolidWorks assembly exceeds the placement limit".into());
            }
            if reference.hidden || reference.suppressed {
                continue;
            }
            let target = tree
                .models
                .get(&reference.model)
                .and_then(|child| Some((child, tree.files.get(&child.file)?)));
            let (Some((child, file)), Some((local, mirrored))) = (target, reference.placement)
            else {
                self.issue(format!(
                    "Unresolved component or invalid placement: {}",
                    reference.name
                ));
                continue;
            };
            let name = format!(
                "{prefix}{}-{}@{}",
                reference.name, reference.number, model.name
            );
            let world = compose(local, parent.0);
            let mirrored = parent.1 ^ mirrored;
            if file.assembly {
                self.visit(child, &format!("{name}/"), (world, mirrored), depth + 1)?;
                continue;
            }
            let transform: [[f32; 4]; 3] = std::array::from_fn(|axis| {
                [
                    world[0][axis] as f32,
                    world[1][axis] as f32,
                    world[2][axis] as f32,
                    (world[3][axis] * 1000.0) as f32,
                ]
            });
            if !transform.iter().flatten().all(|value| value.is_finite()) {
                self.issue(format!("Non-finite component placement: {name}"));
                continue;
            }
            let index = if let Some(index) = self.cached_index(&name) {
                Some(self.colored_cached(
                    index,
                    file,
                    &child.configuration,
                    self.source_visibility && depth == 0,
                ))
            } else {
                self.part_index(file)
            };
            let Some(mut index) = index else {
                self.issue(format!("Missing or unreadable part: {}. Keep it beside the assembly or at its original local path: {}", file.name, file.original.display()));
                continue;
            };
            let component_name = format!("{}-{}", reference.name, reference.number);
            if let Some(&color) = self.component_colors.get(&component_name) {
                index = if let Some(&variant) = self.component_variants.get(&(index, color)) {
                    variant
                } else {
                    let mut mesh = self.scene.meshes[index].mesh.clone();
                    mesh.color = Some(color);
                    mesh.face_colors.clear();
                    let variant = self.add_mesh(mesh);
                    self.component_variants.insert((index, color), variant);
                    variant
                };
            }
            let part = &mut self.scene.meshes[index];
            self.triangles += part.mesh.triangles.len();
            if self.triangles > MAX_TRIANGLES {
                return Err("SolidWorks assembly exceeds the triangle limit".into());
            }
            part.instances.push((transform, mirrored));
        }
        Ok(())
    }
}

pub(super) fn rebuild(path: &Path, blocks: &Blocks<'_>) -> Result<Assembly> {
    let tree = Tree::parse(
        &blocks
            .get(INSTANCE_TREE)
            .ok_or("SolidWorks assembly has no saved instance tree")?
            .expand()?,
    )?;
    let root = tree
        .root
        .as_ref()
        .and_then(|id| tree.models.get(id))
        .filter(|model| {
            tree.files
                .get(&model.file)
                .is_some_and(|file| file.assembly)
        })
        .ok_or("SolidWorks assembly has no saved configuration")?;
    let entries = match blocks.get(CACHE_DIRECTORY) {
        Some(block) => {
            directory(&block.expand()?).ok_or("Invalid SolidWorks component directory")?
        }
        None => Vec::new(),
    };
    let names: Vec<_> = tree
        .models
        .values()
        .flat_map(|model| {
            model
                .references
                .iter()
                .map(|reference| format!("{}-{}", reference.name, reference.number))
        })
        .collect();
    let component_colors = tree
        .root
        .as_ref()
        .and_then(|root| tree.display_states.get(root))
        .filter(|states| states.len() == 1)
        .map(|states| super::appearance::component_overrides(blocks, &names, &states[0]))
        .unwrap_or_default();
    if std::env::var_os("STEPFLASH_PROFILE").is_some() {
        eprintln!(
            "SolidWorks component appearance overrides: {:?}",
            component_colors
        );
    }
    let mut builder = Rebuilder {
        tree: &tree,
        entries: entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry))
            .collect(),
        ids: entries.iter().map(|entry| (entry.id, entry)).collect(),
        stored: cached_meshes(blocks, &entries)?,
        paths: siblings(path),
        cached: HashMap::new(),
        external: HashMap::new(),
        scene: Assembly::default(),
        triangles: 0,
        visited: 0,
        appearances: HashMap::new(),
        variants: HashMap::new(),
        component_colors,
        component_variants: HashMap::new(),
        source_visibility: placement_only(blocks) == Some(true),
    };
    builder.visit(root, "", (IDENTITY, false), 0)?;
    // Discard cache entries that have no visible placements.
    builder
        .scene
        .meshes
        .retain(|part| !part.instances.is_empty());
    if builder.scene.meshes.is_empty() {
        return Err("SolidWorks assembly has no visible component meshes. Keep its parts beside the assembly.".into());
    }
    Ok(builder.scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visibility_substitution_requires_a_closed_exact_subset() {
        let visible = Mesh {
            positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            triangles: vec![[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]],
            ..Default::default()
        };
        let mut cached = visible.clone();
        cached
            .positions
            .extend([[2., 0., 0.], [3., 0., 0.], [2., 1., 0.], [2., 0., 1.]]);
        cached
            .triangles
            .extend([[4, 5, 6], [4, 7, 5], [4, 6, 7], [5, 7, 6]]);
        assert!(closed_visible_subset(&visible, &cached));
        let mut open = visible.clone();
        open.triangles.pop();
        assert!(!closed_visible_subset(&open, &cached));
        let mut modified = visible.clone();
        modified.positions[0][0] = 0.1;
        assert!(!closed_visible_subset(&modified, &cached));
        assert!(!closed_visible_subset(&cached, &cached));
    }
    #[test]
    fn assembly_features_disable_source_visibility_substitution() {
        for (xml, expected) in [
            (
                b"<Keywords><Reference id=\"1\"/><Feature Type=\"Mates\"/></Keywords>".as_slice(),
                true,
            ),
            (
                b"<Keywords><Extrusion id=\"1\"/></Keywords>".as_slice(),
                false,
            ),
            (
                b"<Keywords><Feature Type=\"Unknown\"/></Keywords>".as_slice(),
                false,
            ),
        ] {
            let mut blocks = Blocks::new();
            blocks.insert(
                "swXmlContents/KeyWords".into(),
                super::super::Block {
                    compressed: xml,
                    expanded: xml.len(),
                    crc: crc32fast::hash(xml),
                },
            );
            assert_eq!(placement_only(&blocks), Some(expected));
        }
    }
}
