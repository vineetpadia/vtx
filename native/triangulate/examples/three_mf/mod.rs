mod archive;
mod textures;

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    convert::TryFrom,
    error::Error,
    io::{BufRead, BufReader, Read},
    path::Path,
    time::Instant,
};

use quick_xml::{
    events::{attributes::Attribute, BytesStart, Event},
    name::{Namespace, NamespaceResolver, ResolveResult},
    NsReader, Reader, XmlVersion,
};

use self::archive::Archive;
use self::textures::{TextureBank, TextureGroup, TextureResource};

pub use self::textures::{Image, ImageFormat, SurfaceTexture, Tile};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
type Color = [u8; 4];
type Triangle = [u32; 3];
pub type Instance = [[f32; 4]; 3];

const CORE: &[u8] = b"http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
const MATERIAL: &[u8] = b"http://schemas.microsoft.com/3dmanufacturing/material/2015/02";
const PRODUCTION: &[u8] = b"http://schemas.microsoft.com/3dmanufacturing/production/2015/06";
const RELATIONSHIPS: &[u8] = b"http://schemas.openxmlformats.org/package/2006/relationships";
const START: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
const TEXTURE_REL: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dtexture";
const MAX_XML: u64 = 512 * 1024 * 1024;
const MAX_PACKAGE_XML: u64 = 1024 * 1024 * 1024;
const MAX_DEPTH: usize = 128;
const MAX_PLACEMENTS: usize = 100_000;
const DEFAULT_COLOR: Color = [84, 191, 242, 255];

pub struct Model {
    pub meshes: Vec<Mesh>,
    pub issues: usize,
    pub images: Vec<Image>,
    pub textures: Vec<SurfaceTexture>,
}

pub struct Mesh {
    pub points: Vec<[f32; 3]>,
    pub triangles: Vec<Triangle>,
    pub color: Option<Color>,
    pub colors: Option<Vec<Color>>,
    pub texture: Option<(usize, Vec<[f32; 2]>)>,
    // A single placement is baked in f64. Shared meshes retain their local positions.
    pub instances: Vec<(Instance, bool)>,
}

#[derive(Clone, Copy)]
struct Matrix([f64; 12]);

impl Matrix {
    const IDENTITY: Self = Self([1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.]);

    fn then(self, parent: Self) -> Self {
        let a = self.0;
        let b = parent.0;
        Self(std::array::from_fn(|i| {
            let row = i / 3 * 3;
            let col = i % 3;
            let product = a[row] * b[col] + a[row + 1] * b[col + 3] + a[row + 2] * b[col + 6];
            if row == 9 {
                product + b[col + 9]
            } else {
                product
            }
        }))
    }

    fn point(self, [x, y, z]: [f64; 3], unit: f64) -> [f32; 3] {
        std::array::from_fn(|i| {
            let m = self.0;
            ((x * m[i] + y * m[i + 3] + z * m[i + 6] + m[i + 9]) * unit) as f32
        })
    }

    fn instance(self, unit: f64) -> Instance {
        std::array::from_fn(|col| std::array::from_fn(|row| (self.0[row * 3 + col] * unit) as f32))
    }

    fn mirrored(self) -> bool {
        let [a, b, c, d, e, f, g, h, i, _, _, _] = self.0;
        a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g) < 0.0
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct ObjectKey {
    part: usize,
    id: u32,
}

type Placement = (ObjectKey, Matrix);

#[derive(Default)]
struct Object {
    points: Vec<[f64; 3]>,
    triangles: Vec<Triangle>,
    properties: Vec<(usize, [Option<u32>; 4])>,
    default_property: Option<(u32, u32)>,
    components: Vec<Placement>,
}

struct Palette {
    colors: Vec<Color>,
    base_materials: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Node {
    Model,
    Resources,
    Object,
    Mesh,
    Vertices,
    Vertex,
    Triangles,
    Triangle,
    Components,
    Component,
    Build,
    Item,
    BaseMaterials,
    Base,
    ColorGroup,
    Color,
    Texture,
    TextureGroup,
    TexCoord,
    Other,
}

fn attribute<'a>(tag: &'a BytesStart<'_>, key: &[u8]) -> Result<Option<Cow<'a, str>>> {
    Ok(tag
        .try_get_attribute(key)?
        .map(|attr| attr.normalized_value(XmlVersion::Implicit1_0))
        .transpose()?)
}

fn required<'a>(tag: &'a BytesStart<'_>, key: &[u8]) -> Result<Cow<'a, str>> {
    attribute(tag, key)?
        .ok_or_else(|| format!("Missing 3MF attribute {}", String::from_utf8_lossy(key)).into())
}

fn uint(tag: &BytesStart<'_>, key: &[u8]) -> Result<Option<u32>> {
    attribute(tag, key)?
        .map(|value| Ok(value.trim().parse()?))
        .transpose()
}

fn finite(text: impl AsRef<[u8]>) -> Result<f64> {
    let value: f64 = fast_float::parse(text.as_ref().trim_ascii())?;
    if !value.is_finite() {
        return Err("Non-finite 3MF coordinate or transform".into());
    }
    Ok(value)
}

fn numeric_bytes<'a>(attribute: &'a Attribute<'_>) -> Result<Cow<'a, [u8]>> {
    let bytes = attribute.value.as_ref();
    if memchr::memchr(b'&', bytes).is_none() {
        return Ok(Cow::Borrowed(bytes));
    }
    let text = attribute.normalized_value(XmlVersion::Implicit1_0)?;
    Ok(Cow::Owned(text.into_owned().into_bytes()))
}

fn plain_triangle(mut attributes: &[u8]) -> Option<Triangle> {
    let mut indices = [0; 3];
    for (index, prefix) in indices.iter_mut().zip([b" v1=\"", b" v2=\"", b" v3=\""]) {
        let value = attributes.strip_prefix(prefix)?;
        let end = memchr::memchr(b'"', value)?;
        if !(1..=10).contains(&end) {
            return None;
        }
        let mut number = 0u64;
        for &byte in &value[..end] {
            let digit = byte.wrapping_sub(b'0');
            if digit > 9 {
                return None;
            }
            number = number * 10 + u64::from(digit);
        }
        *index = u32::try_from(number).ok()?;
        attributes = &value[end + 1..];
    }
    matches!(attributes, b"" | b" ").then_some(indices)
}

fn id(tag: &BytesStart<'_>, key: &[u8]) -> Result<u32> {
    Ok(uint(tag, key)?
        .filter(|id| *id != 0)
        .ok_or("Missing or zero 3MF resource ID")?)
}

fn placement(
    tag: &BytesStart<'_>,
    namespaces: &NamespaceResolver,
    part: usize,
    referenced: &mut HashMap<String, usize>,
) -> Result<Placement> {
    let mut source = None;
    for attr in tag.attributes() {
        let attr = attr?;
        if attr.key.local_name().as_ref() != b"path" {
            continue;
        }
        if matches!(
            namespaces.resolve_attribute(attr.key).0,
            ResolveResult::Bound(Namespace(PRODUCTION))
        ) {
            if part != 0 {
                return Err("Only root 3MF components can reference other model parts".into());
            }
            if source.is_some() {
                return Err("Duplicate 3MF placement path".into());
            }
            let path = attr.normalized_value(XmlVersion::Implicit1_0)?;
            if !path.starts_with('/') {
                return Err("Expected an absolute 3MF Production path".into());
            }
            let name = part_name(&path, "")?;
            let next = referenced.len() + 1;
            source = Some(*referenced.entry(name).or_insert(next));
        }
    }
    let mut matrix = Matrix::IDENTITY;
    if let Some(transform) = attribute(tag, b"transform")? {
        let mut fields = transform.split_ascii_whitespace();
        for value in &mut matrix.0 {
            *value = finite(
                fields
                    .next()
                    .ok_or("Expected twelve 3MF transform coefficients")?,
            )?;
        }
        if fields.next().is_some() {
            return Err("Expected twelve 3MF transform coefficients".into());
        }
    }
    Ok((
        ObjectKey {
            part: source.unwrap_or(part),
            id: id(tag, b"objectid")?,
        },
        matrix,
    ))
}

fn color(text: &str) -> Result<Color> {
    let text = text
        .trim()
        .strip_prefix('#')
        .ok_or("Invalid 3MF display color")?;
    let mut rgba = [255; 4];
    if !matches!(text.len(), 6 | 8) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid 3MF display color".into());
    }
    for (i, channel) in rgba.iter_mut().take(text.len() / 2).enumerate() {
        *channel = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)?;
    }
    Ok(rgba)
}

fn classify(namespace: ResolveResult<'_>, local: &[u8], parent: Option<Node>) -> Node {
    use Node::*;
    match namespace {
        ResolveResult::Bound(Namespace(CORE)) => match (local, parent) {
            (b"model", None) => Model,
            (b"resources", Some(Model)) => Resources,
            (b"object", Some(Resources)) => Object,
            (b"mesh", Some(Object)) => Mesh,
            (b"vertices", Some(Mesh)) => Vertices,
            (b"vertex", Some(Vertices)) => Vertex,
            (b"triangles", Some(Mesh)) => Triangles,
            (b"triangle", Some(Triangles)) => Triangle,
            (b"components", Some(Object)) => Components,
            (b"component", Some(Components)) => Component,
            (b"build", Some(Model)) => Build,
            (b"item", Some(Build)) => Item,
            (b"basematerials", Some(Resources)) => BaseMaterials,
            (b"base", Some(BaseMaterials)) => Base,
            _ => Other,
        },
        ResolveResult::Bound(Namespace(MATERIAL)) => match (local, parent) {
            (b"colorgroup", Some(Resources)) => ColorGroup,
            (b"color", Some(ColorGroup)) => Color,
            (b"texture2d", Some(Resources)) => Texture,
            (b"texture2dgroup", Some(Resources)) => TextureGroup,
            (b"tex2coord", Some(TextureGroup)) => TexCoord,
            _ => Other,
        },
        _ => Other,
    }
}

struct Document {
    objects: HashMap<u32, Object>,
    palettes: HashMap<u32, Palette>,
    textures: HashMap<u32, TextureResource>,
    texture_groups: HashMap<u32, TextureGroup>,
    build: Vec<Placement>,
    referenced: HashMap<String, usize>,
    unit: f64,
    require_materials: bool,
}

fn parse_model(
    source: impl BufRead,
    part: usize,
    size: u64,
    source_name: &str,
) -> Result<Document> {
    let mut document = Document {
        objects: HashMap::new(),
        palettes: HashMap::new(),
        textures: HashMap::new(),
        texture_groups: HashMap::new(),
        build: Vec::new(),
        referenced: HashMap::new(),
        unit: 1.0,
        require_materials: false,
    };
    let mut reader = Reader::from_reader(source);
    reader.config_mut().trim_text(true);
    let mut namespaces = NamespaceResolver::default();
    let namespace_declaration = memchr::memmem::Finder::new(b"xmlns");
    let mut buffer = Vec::new();
    let mut stack = Vec::new();
    let mut seen_root = false;
    let mut ids = HashSet::new();
    let mut object = None::<(u32, Object)>;
    let mut palette = None::<(u32, Palette)>;
    let mut texture_group = None;

    loop {
        buffer.clear();
        let (tag, closed) = match reader.read_event_into(&mut buffer)? {
            Event::Start(tag) => (tag, false),
            Event::Empty(tag) => (tag, true),
            Event::End(_) => {
                match stack.pop() {
                    Some(Node::Object) => {
                        let (id, mesh) = object.take().unwrap();
                        document.objects.insert(id, mesh);
                    }
                    Some(Node::BaseMaterials | Node::ColorGroup) => {
                        let (id, colors) = palette.take().unwrap();
                        document.palettes.insert(id, colors);
                    }
                    Some(Node::TextureGroup) => {
                        let (id, group) = texture_group.take().unwrap();
                        document.texture_groups.insert(id, group);
                    }
                    Some(_) => (),
                    None => return Err("Invalid 3MF XML nesting".into()),
                }
                namespaces.pop();
                continue;
            }
            Event::DocType(_) => return Err("DTDs are unsupported in 3MF".into()),
            Event::Eof => break,
            _ => continue,
        };
        // Most vertices and triangles are empty elements with no namespace declarations.
        // Their attributes only need to be scanned by the geometry decoder.
        let scoped = !closed || namespace_declaration.find(tag.attributes_raw()).is_some();
        if scoped {
            namespaces.push(&tag).map_err(quick_xml::Error::from)?;
        }
        let (namespace, name) = namespaces.resolve_element(tag.name());
        let node = classify(namespace, name.as_ref(), stack.last().copied());
        if stack.len() >= MAX_DEPTH || (stack.is_empty() && (seen_root || node != Node::Model)) {
            return Err("Invalid 3MF model root or nesting".into());
        }
        match node {
            Node::Model => {
                seen_root = true;
                document.unit = match attribute(&tag, b"unit")?
                    .as_deref()
                    .unwrap_or("millimeter")
                    .trim()
                {
                    "micron" => 0.001,
                    "millimeter" => 1.0,
                    "centimeter" => 10.0,
                    "inch" => 25.4,
                    "foot" => 304.8,
                    "meter" => 1000.0,
                    _ => return Err("Unknown 3MF model unit".into()),
                };
                if let Some(extensions) = attribute(&tag, b"requiredextensions")? {
                    for prefix in extensions.split_ascii_whitespace() {
                        let declaration = format!("xmlns:{}", prefix);
                        match attribute(&tag, declaration.as_bytes())?
                            .as_deref()
                            .map(str::as_bytes)
                        {
                            Some(MATERIAL) => document.require_materials = true,
                            Some(PRODUCTION) => (),
                            _ => {
                                return Err(format!(
                                    "Unsupported required 3MF extension {}",
                                    prefix
                                )
                                .into())
                            }
                        }
                    }
                }
            }
            Node::Object => {
                let id = id(&tag, b"id")?;
                if !ids.insert(id) {
                    return Err("Duplicate 3MF resource ID".into());
                }
                let default_property = match (uint(&tag, b"pid")?, uint(&tag, b"pindex")?) {
                    (Some(pid), Some(index)) => Some((pid, index)),
                    (None, None) => None,
                    _ => return Err("Incomplete 3MF object property".into()),
                };
                object = Some((
                    id,
                    Object {
                        default_property,
                        ..Default::default()
                    },
                ));
            }
            Node::Vertex => {
                let mut xyz = [None; 3];
                let mut attributes = tag.attributes();
                attributes.with_checks(false);
                for attr in attributes {
                    let attr = attr?;
                    let axis = match attr.key.as_ref() {
                        b"x" => 0,
                        b"y" => 1,
                        b"z" => 2,
                        _ => continue,
                    };
                    if xyz[axis].replace(finite(numeric_bytes(&attr)?)?).is_some() {
                        return Err("Duplicate 3MF vertex coordinate".into());
                    }
                }
                let [Some(x), Some(y), Some(z)] = xyz else {
                    return Err("Incomplete 3MF vertex".into());
                };
                object.as_mut().unwrap().1.points.push([x, y, z]);
            }
            Node::Triangle => {
                if closed && !scoped {
                    // The common record has exactly three ordered decimal attributes.
                    // Any other spelling or material property uses the XML iterator.
                    if let Some(indices) = plain_triangle(tag.attributes_raw()) {
                        object.as_mut().unwrap().1.triangles.push(indices);
                        continue;
                    }
                }
                let mut indices = [None; 3];
                let mut property = [None; 4];
                let mut attributes = tag.attributes();
                attributes.with_checks(false);
                for attr in attributes {
                    let attr = attr?;
                    let target = match attr.key.as_ref() {
                        b"v1" => &mut indices[0],
                        b"v2" => &mut indices[1],
                        b"v3" => &mut indices[2],
                        b"pid" => &mut property[0],
                        b"p1" => &mut property[1],
                        b"p2" => &mut property[2],
                        b"p3" => &mut property[3],
                        _ => continue,
                    };
                    let bytes = numeric_bytes(&attr)?;
                    let index = std::str::from_utf8(bytes.trim_ascii())?.parse()?;
                    if target.replace(index).is_some() {
                        return Err("Duplicate 3MF triangle attribute".into());
                    }
                }
                let [Some(a), Some(b), Some(c)] = indices else {
                    return Err("Incomplete 3MF triangle".into());
                };
                let mesh = &mut object.as_mut().unwrap().1;
                if property.iter().any(Option::is_some) {
                    mesh.properties.push((mesh.triangles.len(), property));
                }
                mesh.triangles.push([a, b, c]);
            }
            Node::Item if part == 0 => {
                document.build.push(placement(
                    &tag,
                    &namespaces,
                    part,
                    &mut document.referenced,
                )?);
            }
            Node::Component => object.as_mut().unwrap().1.components.push(placement(
                &tag,
                &namespaces,
                part,
                &mut document.referenced,
            )?),
            Node::BaseMaterials | Node::ColorGroup => {
                let id = id(&tag, b"id")?;
                if !ids.insert(id) {
                    return Err("Duplicate 3MF resource ID".into());
                }
                palette = Some((
                    id,
                    Palette {
                        colors: Vec::new(),
                        base_materials: node == Node::BaseMaterials,
                    },
                ));
            }
            Node::Base | Node::Color => {
                let key: &[u8] = if node == Node::Base {
                    b"displaycolor"
                } else {
                    b"color"
                };
                palette
                    .as_mut()
                    .unwrap()
                    .1
                    .colors
                    .push(color(&required(&tag, key)?)?);
            }
            Node::Texture => {
                let id = id(&tag, b"id")?;
                if !ids.insert(id) {
                    return Err("Duplicate 3MF resource ID".into());
                }
                document
                    .textures
                    .insert(id, TextureResource::parse(&tag, source_name)?);
            }
            Node::TextureGroup => {
                let key = id(&tag, b"id")?;
                if !ids.insert(key) {
                    return Err("Duplicate 3MF resource ID".into());
                }
                texture_group = Some((
                    key,
                    TextureGroup {
                        texture: id(&tag, b"texid")?,
                        coordinates: Vec::new(),
                    },
                ));
            }
            Node::TexCoord => {
                texture_group
                    .as_mut()
                    .unwrap()
                    .1
                    .coordinates
                    .push(textures::coordinates(&tag)?);
            }
            _ => (),
        }
        if !closed {
            stack.push(node);
        } else if node == Node::Object {
            let (id, mesh) = object.take().unwrap();
            document.objects.insert(id, mesh);
        } else if matches!(node, Node::BaseMaterials | Node::ColorGroup) {
            let (id, colors) = palette.take().unwrap();
            document.palettes.insert(id, colors);
        } else if node == Node::TextureGroup {
            let (id, group) = texture_group.take().unwrap();
            document.texture_groups.insert(id, group);
        }
        if closed && scoped {
            namespaces.pop();
        }
    }
    if !seen_root || !stack.is_empty() || reader.buffer_position() > size {
        return Err("Truncated or oversized 3MF model XML".into());
    }
    for mesh in document.objects.values() {
        if !mesh.triangles.is_empty()
            && (!mesh.components.is_empty()
                || mesh
                    .triangles
                    .iter()
                    .flatten()
                    .any(|&index| index as usize >= mesh.points.len()))
        {
            return Err("Invalid 3MF mesh indices or components".into());
        }
    }
    Ok(document)
}

fn part_name(target: &str, source: &str) -> Result<String> {
    let decoded = percent_encoding::percent_decode_str(target).decode_utf8()?;
    if decoded.contains([':', '?', '#', '\\', '\0']) {
        return Err("Invalid 3MF model part name".into());
    }
    let (folder, _) = source.rsplit_once('/').unwrap_or(("", source));
    let (name, relative) = decoded
        .strip_prefix('/')
        .map_or((decoded.as_ref(), true), |path| (path, false));
    let mut segments: Vec<_> = if relative && !folder.is_empty() {
        folder.split('/').collect()
    } else {
        Vec::new()
    };
    for segment in name.split('/') {
        match segment {
            "." if relative && !source.is_empty() => (),
            ".." if relative && !source.is_empty() => {
                segments.pop().ok_or("Invalid 3MF model part name")?;
            }
            "" | "." | ".." => return Err("Invalid 3MF model part name".into()),
            _ => segments.push(segment),
        }
    }
    if segments.is_empty() {
        return Err("Invalid 3MF model part name".into());
    }
    Ok(segments.join("/"))
}

fn relationships(archive: &mut Archive, source: &str, kind: &str) -> Result<Vec<String>> {
    const MAX_RELATIONSHIPS: u64 = 1024 * 1024;
    let path = match source.rsplit_once('/') {
        Some((folder, name)) => format!("{}/_rels/{}.rels", folder, name),
        None => format!("_rels/{}.rels", source),
    };
    let rels = archive.by_name(&path)?;
    if rels.size() > MAX_RELATIONSHIPS {
        return Err("3MF relationship part is too large".into());
    }
    let size = rels.size();
    let mut reader = NsReader::from_reader(BufReader::new(rels.take(size + 1)));
    let mut buffer = Vec::new();
    let mut targets = Vec::new();
    loop {
        buffer.clear();
        let tag = match reader.read_event_into(&mut buffer)? {
            Event::Start(tag) | Event::Empty(tag) => tag,
            Event::DocType(_) => return Err("DTDs are unsupported in 3MF".into()),
            Event::Eof => break,
            _ => continue,
        };
        let (namespace, local) = reader.resolver().resolve_element(tag.name());
        if matches!(namespace, ResolveResult::Bound(Namespace(RELATIONSHIPS)))
            && local.as_ref() == b"Relationship"
            && attribute(&tag, b"Type")?.as_deref() == Some(kind)
        {
            if attribute(&tag, b"TargetMode")?
                .as_deref()
                .unwrap_or("Internal")
                != "Internal"
            {
                return Err("Expected an internal 3MF model relationship".into());
            }
            targets.push(part_name(&required(&tag, b"Target")?, source)?);
        }
    }
    if reader.buffer_position() > size {
        return Err("3MF relationship part is too large".into());
    }
    Ok(targets)
}

fn referenced_parts(root: &Document) -> Result<HashSet<usize>> {
    let mut parts = HashSet::new();
    let mut visited = HashSet::new();
    let mut pending: Vec<_> = root.build.iter().map(|(key, _)| *key).collect();
    while let Some(key) = pending.pop() {
        if key.part != 0 {
            parts.insert(key.part);
        } else if visited.insert(key.id) {
            let object = root
                .objects
                .get(&key.id)
                .ok_or("Missing 3MF build object")?;
            pending.extend(object.components.iter().map(|(key, _)| *key));
        }
    }
    Ok(parts)
}

fn collect(
    placement: Placement,
    parent: Matrix,
    documents: &[Option<Document>],
    active: &mut Vec<ObjectKey>,
    groups: &mut HashMap<ObjectKey, Vec<Matrix>>,
    order: &mut Vec<ObjectKey>,
    remaining: &mut usize,
) -> Result<()> {
    let (key, local) = placement;
    if active.len() >= MAX_DEPTH || active.contains(&key) {
        return Err("Cyclic or excessively nested 3MF components".into());
    }
    *remaining = remaining.checked_sub(1).ok_or("Too many 3MF placements")?;
    let object = documents[key.part]
        .as_ref()
        .and_then(|document| document.objects.get(&key.id))
        .ok_or("Missing 3MF build object")?;
    let matrix = local.then(parent);
    if !matrix.0.iter().all(|value| value.is_finite()) {
        return Err("3MF placement transform overflow".into());
    }
    if !object.triangles.is_empty() {
        groups
            .entry(key)
            .or_insert_with(|| {
                order.push(key);
                Vec::new()
            })
            .push(matrix);
    } else {
        active.push(key);
        for &component in &object.components {
            collect(
                component, matrix, documents, active, groups, order, remaining,
            )?;
        }
        active.pop();
    }
    Ok(())
}

fn triangle_property(
    default: Option<(u32, u32)>,
    [pid, p1, p2, p3]: [Option<u32>; 4],
) -> Result<(u32, [u32; 3])> {
    let pid = pid
        .or(default.map(|(pid, _)| pid))
        .ok_or("Missing 3MF property group")?;
    let p1 = p1
        .or(default
            .filter(|(group, _)| *group == pid)
            .map(|(_, index)| index))
        .ok_or("Missing 3MF triangle property index")?;
    Ok((pid, [p1, p2.unwrap_or(p1), p3.unwrap_or(p1)]))
}

fn place_geometry(
    points: Vec<[f64; 3]>,
    triangles: &mut [Triangle],
    placements: &[Matrix],
    unit: f64,
) -> (Vec<[f32; 3]>, Vec<(Instance, bool)>) {
    let (matrix, scale, instances) = if let [matrix] = placements {
        if matrix.mirrored() {
            for triangle in triangles {
                triangle.swap(1, 2);
            }
        }
        (*matrix, unit, Vec::new())
    } else {
        (
            Matrix::IDENTITY,
            1.0,
            placements
                .iter()
                .map(|matrix| (matrix.instance(unit), matrix.mirrored()))
                .collect(),
        )
    };
    (
        points
            .into_iter()
            .map(|point| matrix.point(point, scale))
            .collect(),
        instances,
    )
}

impl Document {
    fn display_colors(
        &self,
        pid: u32,
        indices: [u32; 3],
        issues: &mut usize,
    ) -> Result<[Color; 3]> {
        let Some(palette) = self.palettes.get(&pid) else {
            if self.require_materials {
                return Err("Unsupported 3MF material property group".into());
            }
            *issues += 1;
            return Ok([DEFAULT_COLOR; 3]);
        };
        if palette.base_materials && (indices[1] != indices[0] || indices[2] != indices[0]) {
            return Err("3MF base materials cannot form gradients".into());
        }
        let lookup = |index: u32| {
            palette
                .colors
                .get(index as usize)
                .copied()
                .ok_or("Invalid 3MF color index")
        };
        Ok([
            lookup(indices[0])?,
            lookup(indices[1])?,
            lookup(indices[2])?,
        ])
    }

    fn mesh(
        &self,
        mut object: Object,
        placements: Vec<Matrix>,
        unit: f64,
        issues: &mut usize,
    ) -> Result<Mesh> {
        let mut color = match object.default_property {
            Some((pid, index)) => Some(self.display_colors(pid, [index; 3], issues)?[0]),
            None => None,
        };
        let mut colors = None;
        if !object.properties.is_empty() {
            let default_color = color.unwrap_or(DEFAULT_COLOR);
            let mut colored_points = Vec::new();
            let mut rgba = Vec::new();
            let mut remap = HashMap::<(u32, Color), u32>::new();
            let mut overrides = object.properties.into_iter().peekable();
            let positions = &object.points;
            for (face, triangle) in object.triangles.iter_mut().enumerate() {
                let mut triangle_colors = [default_color; 3];
                if overrides.peek().is_some_and(|(index, _)| *index == face) {
                    let (_, property) = overrides.next().unwrap();
                    let (pid, indices) = triangle_property(object.default_property, property)?;
                    triangle_colors = self.display_colors(pid, indices, issues)?;
                }
                for (vertex, color) in triangle.iter_mut().zip(triangle_colors) {
                    let index = u32::try_from(colored_points.len())?;
                    *vertex = *remap.entry((*vertex, color)).or_insert_with(|| {
                        colored_points.push(positions[*vertex as usize]);
                        rgba.push(color);
                        index
                    });
                }
            }
            color = None;
            colors = Some(rgba);
            object.points = colored_points;
        }
        let (points, instances) =
            place_geometry(object.points, &mut object.triangles, &placements, unit);
        Ok(Mesh {
            points,
            triangles: object.triangles,
            color,
            colors,
            instances,
            texture: None,
        })
    }
}

fn read_document(archive: &mut Archive, name: &str, part: usize) -> Result<(Document, u64)> {
    let xml = archive.by_name(name)?;
    let size = xml.size();
    if size > MAX_XML {
        return Err("Expanded 3MF model exceeds the decode limit".into());
    }
    let source = BufReader::with_capacity(128 * 1024, xml.take(size + 1));
    Ok((parse_model(source, part, size, name)?, size))
}

fn read_children(
    archive: &mut Archive,
    root_name: &str,
    root: &Document,
    root_size: u64,
) -> Result<(Vec<(usize, Document)>, &'static str)> {
    if root.referenced.is_empty() {
        return Ok((Vec::new(), "local"));
    }
    let needed = referenced_parts(root)?;
    if needed.is_empty() {
        return Ok((Vec::new(), "local"));
    }
    let related: HashSet<_> = relationships(archive, root_name, START)?
        .into_iter()
        .collect();
    let mut jobs = Vec::with_capacity(needed.len());
    let mut expanded = root_size;
    for (name, &part) in &root.referenced {
        if !needed.contains(&part) {
            continue;
        }
        if name == root_name || !related.contains(name) {
            return Err(format!("Missing 3MF model relationship for {}", name).into());
        }
        let size = archive.by_name(name)?.size();
        if size > MAX_XML || size > MAX_PACKAGE_XML - expanded {
            return Err("Expanded 3MF package exceeds the decode limit".into());
        }
        expanded += size;
        jobs.push((part, name.as_str()));
    }
    jobs.sort_unstable_by_key(|&(part, _)| part);
    let read = |reader: &mut Archive, part, name: &str| {
        read_document(reader, name, part)
            .map(|(document, _)| (part, document))
            .map_err(|error| format!("3MF model {}: {}", name, error))
    };
    #[cfg(feature = "rayon")]
    if jobs.len() > 1 && expanded - root_size >= 1024 * 1024 {
        use rayon::prelude::*;
        let documents = jobs
            .par_iter()
            .map(|&(part, name)| read(&mut archive.clone(), part, name))
            .collect::<std::result::Result<_, _>>()?;
        return Ok((documents, "parallel"));
    }
    let documents = jobs
        .into_iter()
        .map(|(part, name)| read(archive, part, name))
        .collect::<std::result::Result<_, _>>()?;
    Ok((documents, "serial"))
}

pub fn load(path: &Path) -> Result<Model> {
    let start = Instant::now();
    let mut archive = archive::open(path)?;
    let mut relationships = relationships(&mut archive, "", START)?;
    if relationships.len() != 1 {
        return Err("Expected one 3MF start-model relationship".into());
    }
    let name = relationships.pop().unwrap();
    let opened = start.elapsed();
    let (mut root, root_size) = read_document(&mut archive, &name, 0)?;
    let root_parsed = start.elapsed();
    let (children, mode) = read_children(&mut archive, &name, &root, root_size)?;
    let parsed = start.elapsed();
    let child_count = children.len();
    let mut documents = Vec::with_capacity(root.referenced.len() + 1);
    documents.resize_with(root.referenced.len() + 1, || None);
    let unit = root.unit;
    let build = std::mem::take(&mut root.build);
    documents[0] = Some(root);
    for (part, document) in children {
        documents[part] = Some(document);
    }
    let mut groups = HashMap::new();
    let mut order = Vec::new();
    let mut active = Vec::new();
    let mut remaining = MAX_PLACEMENTS;
    for item in build {
        collect(
            item,
            Matrix::IDENTITY,
            &documents,
            &mut active,
            &mut groups,
            &mut order,
            &mut remaining,
        )?;
    }
    let mut issues = 0;
    let mut meshes = Vec::with_capacity(order.len());
    let mut bank = TextureBank::default();
    for key in order {
        let document = documents[key.part].as_mut().unwrap();
        let object = document.objects.remove(&key.id).unwrap();
        let placements = groups.remove(&key).unwrap();
        if document.has_texture(&object) {
            meshes.extend(document.textured_meshes(
                object,
                &placements,
                unit,
                key.part,
                &mut bank,
                &mut issues,
            )?);
        } else {
            meshes.push(document.mesh(object, placements, unit, &mut issues)?);
        }
    }
    if meshes.is_empty() {
        return Err("3MF build has no mesh triangles".into());
    }
    let placed = start.elapsed();
    let (images, textures) = bank.load(
        &mut archive,
        &name,
        &documents[0].as_ref().unwrap().referenced,
    )?;
    if std::env::var_os("STEPFLASH_PROFILE").is_some() {
        let ms = |time: std::time::Duration| time.as_secs_f64() * 1000.0;
        eprintln!(
            "3MF phases: package={:.2} ms, xml_geometry={:.2} ms, child_xml={:.2} ms, placement={:.2} ms, texture_read={:.2} ms; meshes={}, child_parts={}, part_mode={}, images={}",
            ms(opened),
            ms(parsed - opened),
            ms(parsed - root_parsed),
            ms(placed - parsed),
            ms(start.elapsed() - placed),
            meshes.len(), child_count, mode, images.len()
        );
    }
    Ok(Model {
        meshes,
        issues,
        images,
        textures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_triangle_checks_the_full_record_and_u32_range() {
        assert_eq!(
            plain_triangle(br#" v1="0" v2="12" v3="4294967295""#),
            Some([0, 12, u32::MAX])
        );
        assert_eq!(
            plain_triangle(br#" v1="0" v2="1" v3="2" "#),
            Some([0, 1, 2])
        );
        for attributes in [
            br#" v1="" v2="1" v3="2""#.as_slice(),
            br#" v1="4294967296" v2="1" v3="2""#,
            br#" v1="00000000000" v2="1" v3="2""#,
            br#" v1="+0" v2="1" v3="2""#,
            br#" v1="&#48;" v2="1" v3="2""#,
            br#" v1="0" v2="1" v3="2" v1="3""#,
            br#" v1="0" v2="1" v3="2" pid="4" p1="0""#,
        ] {
            assert_eq!(plain_triangle(attributes), None);
        }
    }

    #[test]
    fn geometry_records_keep_namespace_scope() {
        let xml = br#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
            <resources><object id="1"><mesh><vertices>
            <vertex xmlns="urn:other" x="99" y="99" z="99"/>
            <vertex x="0" y="0" z="0"/>
            <c:vertex xmlns:c="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" x="1" y="0" z="0"/>
            <vertex x="0" y="1" z="0"></vertex>
            </vertices><triangles>
            <triangle xmlns="urn:other" v1="99" v2="99" v3="99"/>
            <triangle v1="0" v2="1" v3="2"/>
            <triangle v3="2" v1="&#48;" v2="+1"></triangle>
            </triangles></mesh></object></resources><build><item objectid="1"/></build></model>"#;
        let document = parse_model(xml.as_slice(), 0, xml.len() as u64, "3D/model.model").unwrap();
        let mesh = &document.objects[&1];
        assert_eq!(mesh.points, [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]);
        assert_eq!(mesh.triangles, [[0, 1, 2], [0, 1, 2]]);
    }
}
