use std::{
    collections::HashMap,
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

use super::{load_part, tables, word, Blocks, Mesh, MeshBuilder, Result, MAX_TRIANGLES};

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
}

#[derive(Default)]
struct Tree {
    files: HashMap<String, ModelFile>,
    models: HashMap<String, Model>,
    root: Option<String>,
    recent: bool,
}

struct ModelFile {
    assembly: bool,
    name: String,
}

struct Model {
    file: String,
    name: String,
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
                            if tree.root.is_none() || (recent && !tree.recent) {
                                tree.root = Some(get(&attrs, b"swModelRef").into());
                                tree.recent = recent;
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
            let mut tables = tables(bytes).take(count);
            let mut builder = MeshBuilder::default();
            let mut decoded = 0;
            for table in &mut tables {
                builder.add_table(table, CACHE_WELD_INVERSE)?;
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
}

impl Rebuilder<'_> {
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
            .paths
            .get(&file.name)
            .and_then(Option::as_ref)
            .and_then(|path| load_part(path).ok());
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
            self.scene.issues += 1;
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
                self.scene.issues += 1;
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
                self.scene.issues += 1;
                continue;
            }
            let Some(index) = self.cached_index(&name).or_else(|| self.part_index(file)) else {
                self.scene.issues += 1;
                continue;
            };
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
    };
    builder.visit(root, "", (IDENTITY, false), 0)?;
    if builder.scene.meshes.is_empty() {
        return Err("SolidWorks assembly has no visible component meshes. Keep its parts beside the assembly.".into());
    }
    Ok(builder.scene)
}
