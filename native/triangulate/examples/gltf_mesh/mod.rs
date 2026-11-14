use std::{borrow::Cow, collections::HashMap, convert::{TryFrom, TryInto}, error::Error, path::Path, time::Instant};

use gltf::{accessor::{DataType, Dimensions}, buffer::Source, mesh::{util::ReadIndices, Mode}, Node, Semantic};
use nalgebra_glm::{Mat4, Vec4};

mod compressed;
mod draco;
mod embedded;

type Point = [f32; 3];
type Triangle = [u32; 3];
type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Files selected together in the browser are sent as one bounded manifest
/// plus their bytes. Resource names never become local filesystem paths.
fn read_bundle(bytes: &[u8]) -> Result<(&[u8], HashMap<String, &[u8]>)> {
    let prefix = bytes.get(..4).ok_or("Truncated glTF bundle")?;
    let length = u32::from_le_bytes(prefix.try_into()?) as usize;
    if length == 0 || length > 64 * 1024 { return Err("Invalid glTF bundle header size".into()); }
    let header = bytes.get(4..4 + length).ok_or("Truncated glTF bundle header")?;
    let entries: Vec<serde_json::Value> = serde_json::from_slice(header)?;
    if entries.is_empty() || entries.len() > 512 { return Err("Invalid glTF bundle entries".into()); }
    let mut remaining = &bytes[4 + length..];
    let mut model = None;
    let mut assets = HashMap::new();
    for entry in &entries {
        let name = entry["name"].as_str().ok_or("Missing glTF asset name")?;
        let size = usize::try_from(entry["size"].as_u64().ok_or("Missing glTF asset size")?)?;
        if name.is_empty() || name.contains(['/', '\\', ':', '\0']) || size == 0 {
            return Err("Invalid glTF asset name or size".into());
        }
        let data = remaining.get(..size).ok_or("Truncated glTF asset")?;
        remaining = &remaining[size..];
        let extension = name.rsplit_once('.').map_or("", |(_, ext)| ext);
        if extension.eq_ignore_ascii_case("gltf") || extension.eq_ignore_ascii_case("glb") {
            if model.replace(data).is_some() { return Err("Select exactly one glTF or GLB model".into()); }
        } else if ["bin", "png", "jpg", "jpeg"].iter().any(|ext| extension.eq_ignore_ascii_case(ext)) {
            // Borrow the bytes. The name is owned because the JSON manifest is
            // temporary, so use an owned key in the resource lookup table.
            if assets.insert(name.to_owned(), data).is_some() {
                return Err(format!("Duplicate glTF asset: {name}").into());
            }
        } else {
            return Err(format!("Unsupported glTF companion: {name}").into());
        }
    }
    if !remaining.is_empty() { return Err("Unexpected bytes after glTF bundle".into()); }
    Ok((model.ok_or("Missing glTF model")?, assets))
}

fn companion_uri(uri: &str) -> Result<Cow<'_, str>> {
    let decoded = percent_encoding::percent_decode_str(uri).decode_utf8()?;
    if decoded.starts_with('/') || decoded.contains([':', '\\', '\0', '?', '#']) ||
        decoded.split('/').any(|part| part == ".." || part.is_empty()) {
        return Err("External glTF buffers must be relative asset names".into());
    }
    Ok(decoded)
}

struct Primitive<'a> {
    points: Cow<'a, [Point]>,
    triangles: Cow<'a, [Triangle]>,
    appearance: Option<Appearance>,
}

pub struct Appearance {
    pub color: [f32; 4],
    pub cutoff: Option<f32>,
    pub texture: Option<usize>,
    pub uv: Vec<[f32; 2]>,
}

pub struct Texture {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    pub wrap_s: u32,
    pub wrap_t: u32,
    pub min: u32,
    pub mag: u32,
}

struct TextureLoader<'a, 'b> {
    buffers: &'a [Cow<'b, [u8]>],
    assets: &'a HashMap<String, &'b [u8]>,
    directory: Option<&'a Path>,
    paths: &'a mut HashMap<String, String>,
    textures: &'a mut HashMap<usize, Option<Texture>>,
    incomplete: &'a mut bool,
}

fn resolve_companion<'a>(
    uri: &str, assets: &HashMap<String, &'a [u8]>, directory: Option<&Path>,
    paths: &mut HashMap<String, String>,
) -> Result<Cow<'a, [u8]>> {
    let Some(directory) = directory else {
        return Ok(Cow::Borrowed(companion(uri, assets, paths)?));
    };
    use std::io::Read;
    const MAX_ASSET_BYTES: u64 = 512 * 1024 * 1024;
    let relative = companion_uri(uri)?;
    let directory = directory.canonicalize()?;
    let path = directory.join(relative.as_ref()).canonicalize()?;
    if !path.starts_with(&directory) {
        return Err("glTF resource is outside the model directory".into());
    }
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_ASSET_BYTES {
        return Err("Invalid glTF companion file size".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_ASSET_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_ASSET_BYTES { return Err("glTF companion exceeds 512 MiB".into()); }
    Ok(Cow::Owned(bytes))
}

fn companion<'a>(
    uri: &str, assets: &HashMap<String, &'a [u8]>, paths: &mut HashMap<String, String>,
) -> Result<&'a [u8]> {
    let relative = companion_uri(uri)?;
    let name = relative.rsplit('/').next().unwrap();
    if let Some(previous) = paths.get(name) {
        if previous != relative.as_ref() {
            return Err(format!("Ambiguous glTF asset name: '{name}'").into());
        }
    } else {
        paths.insert(name.to_owned(), relative.to_string());
    }
    assets.get(name).copied().ok_or_else(|| {
        format!("Select glTF asset '{name}' together with the model").into()
    })
}

impl TextureLoader<'_, '_> {
    fn load(&mut self, texture: gltf::Texture) -> Result<Texture> {
        use gltf::image::Source;
        let bytes: Cow<[u8]> = match texture.source().source() {
            Source::View { view, .. } => {
                let start = view.offset();
                let end = start.checked_add(view.length()).ok_or("Image size overflow")?;
                Cow::Borrowed(self.buffers[view.buffer().index()].get(start..end).ok_or("Truncated glTF image")?)
            }
            Source::Uri { uri, .. } if uri.starts_with("data:") => {
                let encoded = embedded::base64_payload(uri).ok_or("Invalid glTF image URI")?;
                Cow::Owned(embedded::decode(encoded)?)
            }
            Source::Uri { uri, .. } => resolve_companion(uri, self.assets, self.directory, self.paths)?,
        };
        let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            "image/jpeg"
        } else {
            return Err("Unsupported glTF base-color image".into());
        };
        let sampler = texture.sampler();
        Ok(Texture {
            bytes: bytes.into_owned(),
            mime,
            wrap_s: sampler.wrap_s().as_gl_enum(),
            wrap_t: sampler.wrap_t().as_gl_enum(),
            min: sampler.min_filter().map_or(9987, |filter| filter.as_gl_enum()),
            mag: sampler.mag_filter().map_or(9729, |filter| filter.as_gl_enum()),
        })
    }
}

fn triangulate_indices(mut indices: impl Iterator<Item = u32>, mode: Mode) -> Result<Vec<Triangle>> {
    let (count, _) = indices.size_hint();
    if mode == Mode::Triangles {
        let mut triangles = Vec::with_capacity(count / 3);
        while let Some(a) = indices.next() {
            let b = indices.next().ok_or("Incomplete glTF triangle list")?;
            let c = indices.next().ok_or("Incomplete glTF triangle list")?;
            triangles.push([a, b, c]);
        }
        return Ok(triangles);
    }
    let (Some(mut a), Some(mut b)) = (indices.next(), indices.next()) else {
        return Ok(Vec::new());
    };
    let mut triangles = Vec::with_capacity(count.saturating_sub(2));
    for (i, c) in indices.enumerate() {
        triangles.push(if mode == Mode::TriangleStrip && i % 2 != 0 {
            [b, a, c]
        } else {
            [a, b, c]
        });
        if mode == Mode::TriangleStrip { a = b; }
        b = c;
    }
    Ok(triangles)
}

pub struct SceneMesh<'a> {
    meshes: Vec<Option<Vec<Primitive<'a>>>>,
    instances: Vec<(usize, Mat4)>,
    pub count: usize,
    pub textures: HashMap<usize, Option<Texture>>,
    pub incomplete_appearance: bool,
}

pub struct Part<'a> {
    pub node: usize,
    pub points: Cow<'a, [Point]>,
    pub triangles: &'a [Triangle],
    pub mirrored: bool,
    pub appearance: Option<&'a Appearance>,
}

fn facing([a, b, c]: Triangle, mirrored: bool) -> Triangle {
    if mirrored { [a, c, b] } else { [a, b, c] }
}

impl SceneMesh<'_> {
    pub fn triangles(&self) -> impl Iterator<Item = [Point; 3]> + '_ {
        self.parts().flat_map(|part| {
            part.triangles.iter().map(move |&triangle| {
                facing(triangle, part.mirrored).map(|i| part.points[i as usize])
            })
        })
    }

    pub fn parts(&self) -> impl Iterator<Item = Part<'_>> + '_ {
        self.instances.iter().enumerate().flat_map(move |(node, (index, world))| {
            self.meshes[*index].as_ref().unwrap().iter().map(move |primitive| {
                let points = if *world == Mat4::identity() {
                    Cow::Borrowed(primitive.points.as_ref())
                } else {
                    Cow::Owned(primitive.points.iter().map(|&[x, y, z]| {
                        let p = world * Vec4::new(x, y, z, 1.0);
                        [p.x, p.y, p.z]
                    }).collect())
                };
                let mirrored = world.fixed_slice::<3, 3>(0, 0).determinant() < 0.0;
                Part { node, points, triangles: &primitive.triangles, mirrored, appearance: primitive.appearance.as_ref() }
            })
        })
    }

    pub fn node_count(&self) -> usize {
        self.instances.len()
    }
}

fn packed_accessor<'a, T: bytemuck::Pod>(
    accessor: &gltf::Accessor, buffers: &'a [Cow<[u8]>], elements_per_record: usize,
) -> Option<Cow<'a, [T]>> {
    let bytes = accessor_bytes(accessor, buffers, accessor.size())?;
    let records = bytes.chunks_exact(std::mem::size_of::<T>());
    if !records.remainder().is_empty() || records.len().checked_mul(elements_per_record)? != accessor.count() {
        return None;
    }
    // A bundle's manifest can place an otherwise packed accessor at an unaligned offset.
    Some(match bytemuck::try_cast_slice::<u8, T>(bytes) {
        Ok(aligned) => Cow::Borrowed(aligned),
        Err(_) => Cow::Owned(records.map(bytemuck::pod_read_unaligned).collect()),
    })
}

fn accessor_bytes<'a>(
    accessor: &gltf::Accessor, buffers: &'a [Cow<[u8]>], stride: usize,
) -> Option<&'a [u8]> {
    if !cfg!(target_endian = "little") || accessor.sparse().is_some() {
        return None;
    }
    let view = accessor.view()?;
    if view.stride().unwrap_or(accessor.size()) != stride { return None; }
    let size = accessor.count().checked_mul(stride)?;
    let start = accessor.offset();
    if start.checked_add(size)? > view.length() { return None; }
    let offset = view.offset().checked_add(start)?;
    buffers.get(view.buffer().index())?.get(offset..offset.checked_add(size)?)
}

fn packed_quantized_positions(accessor: &gltf::Accessor, buffers: &[Cow<[u8]>]) -> Option<Vec<Point>> {
    // Quantized VEC3 attributes commonly pad each 16-bit triplet to eight bytes.
    let bytes = accessor_bytes(accessor, buffers, 8)?;
    macro_rules! unpack {
        ($ty:ty, $max:expr) => {{
            let values: &[[$ty; 4]] = bytemuck::try_cast_slice(bytes).ok()?;
            if accessor.normalized() {
                values.iter().map(|&[x, y, z, _]| {
                    [x, y, z].map(|value| (f32::from(value) / $max).max(-1.0))
                }).collect()
            } else {
                values.iter().map(|&[x, y, z, _]| [x, y, z].map(f32::from)).collect()
            }
        }};
    }
    Some(match accessor.data_type() {
        DataType::I16 => unpack!(i16, 32767.0),
        DataType::U16 => unpack!(u16, 65535.0),
        _ => return None,
    })
}

fn quantized_attribute<const N: usize>(accessor: gltf::Accessor, buffers: &[Cow<[u8]>]) -> Option<Vec<[f32; N]>>
where
    [i8; N]: gltf::accessor::Item, [u8; N]: gltf::accessor::Item,
    [i16; N]: gltf::accessor::Item, [u16; N]: gltf::accessor::Item,
{
    use gltf::accessor::Iter;
    let normalized = accessor.normalized();
    let data = |buffer: gltf::Buffer| buffers.get(buffer.index()).map(Cow::as_ref);
    macro_rules! read {
        ($ty:ty, $max:expr) => {
            Iter::<[$ty; N]>::new(accessor, data)?.map(|values| values.map(|value| {
                let value = f32::from(value);
                if normalized { (value / $max).max(-1.0) } else { value }
            })).collect()
        };
    }
    Some(match accessor.data_type() {
        DataType::I8 => read!(i8, 127.0),
        DataType::U8 => read!(u8, 255.0),
        DataType::I16 => read!(i16, 32767.0),
        DataType::U16 => read!(u16, 65535.0),
        _ => return None,
    })
}

fn decode<'a>(mesh: gltf::Mesh, buffers: &'a [Cow<[u8]>], quantized: bool,
              mut appearance: Option<&mut TextureLoader>, draco: &mut draco::Plan) -> Result<Vec<Primitive<'a>>> {
    mesh.primitives().map(|primitive| {
        let mode = primitive.mode();
        if !matches!(mode, Mode::Triangles | Mode::TriangleStrip | Mode::TriangleFan) {
            return Err("glTF primitive must be a triangle list, strip, or fan".into());
        }
        let reader = primitive.reader(|buffer| Some(buffers[buffer.index()].as_ref()));
        let positions = primitive.get(&Semantic::Positions).ok_or("Missing glTF positions")?;
        if positions.dimensions() != Dimensions::Vec3 { return Err("Invalid glTF position dimensions".into()); }
        let float_positions = positions.data_type() == DataType::F32;
        if !float_positions && !quantized {
            return Err("Integer glTF positions require KHR_mesh_quantization".into());
        }
        let decoded = draco.decode(mesh.index(), &primitive, buffers)?;
        let draco_points = match &decoded {
            Some(mesh) => mesh.attribute("POSITION", &positions)?,
            None => None,
        };
        let points = if let Some(points) = draco_points {
            Cow::Owned(points)
        } else if float_positions {
            match packed_accessor::<Point>(&positions, buffers, 1) {
                Some(points) => points,
                None => Cow::Owned(reader.read_positions().ok_or("Invalid glTF positions")?.collect()),
            }
        } else {
            Cow::Owned(packed_quantized_positions(&positions, buffers)
                .or_else(|| quantized_attribute(positions, buffers))
                .ok_or("Invalid quantized glTF positions")?)
        };
        if float_positions && points.iter().flatten().any(|v| !v.is_finite()) {
            return Err("Non-finite glTF position".into());
        }
        let triangles = if let Some(mesh) = &decoded {
            Cow::Owned(mesh.triangles(&primitive, points.len())?)
        } else if let Some(indices) = primitive.indices() {
            let packed = if mode == Mode::Triangles && indices.data_type() == DataType::U32
                && indices.dimensions() == Dimensions::Scalar
            {
                packed_accessor::<Triangle>(&indices, buffers, 3)
            } else { None };
            if let Some(triangles) = packed {
                triangles
            } else {
                Cow::Owned(match reader.read_indices().ok_or("Invalid glTF indices")? {
                    ReadIndices::U8(indices) => triangulate_indices(indices.map(u32::from), mode)?,
                    ReadIndices::U16(indices) => triangulate_indices(indices.map(u32::from), mode)?,
                    ReadIndices::U32(indices) => triangulate_indices(indices, mode)?,
                })
            }
        } else {
            Cow::Owned(triangulate_indices(0..u32::try_from(points.len())?, mode)?)
        };
        if decoded.is_none() && triangles.iter().flatten().any(|&i| i as usize >= points.len()) {
            return Err("glTF index exceeds position count".into());
        }
        let appearance = appearance.as_deref_mut().map(|loader| -> Result<Appearance> {
            use gltf::material::AlphaMode;
            let material = primitive.material();
            let pbr = material.pbr_metallic_roughness();
            let cutoff = match material.alpha_mode() {
                AlphaMode::Mask => Some(material.alpha_cutoff().unwrap_or(0.5)),
                AlphaMode::Blend => { *loader.incomplete = true; None }
                AlphaMode::Opaque => None,
            };
            let mut result = Appearance { color: pbr.base_color_factor(), cutoff, texture: None, uv: Vec::new() };
            if let Some(info) = pbr.base_color_texture() {
                let texture = info.texture();
                let id = texture.index();
                if !loader.textures.contains_key(&id) {
                    let loaded = loader.load(texture).ok();
                    loader.textures.insert(id, loaded);
                }
                if loader.textures[&id].is_some() {
                    let transform = info.texture_transform();
                    let tex_coord = transform.as_ref().and_then(|transform| transform.tex_coord()).unwrap_or(info.tex_coord());
                    let attribute = primitive.get(&Semantic::TexCoords(tex_coord));
                    let draco_uv = match (&decoded, &attribute) {
                        (Some(mesh), Some(accessor)) => mesh.attribute(&format!("TEXCOORD_{tex_coord}"), accessor)?,
                        _ => None,
                    };
                    let uv = draco_uv.or_else(|| match attribute {
                        Some(attribute) if quantized && attribute.dimensions() == Dimensions::Vec2 &&
                            attribute.data_type() != DataType::F32 => quantized_attribute(attribute, buffers),
                        _ => reader.read_tex_coords(tex_coord).map(|coords| coords.into_f32().collect()),
                    });
                    if let Some(mut uv) = uv {
                        if let Some(transform) = transform {
                            let [sx, sy] = transform.scale();
                            let [ox, oy] = transform.offset();
                            let (sin, cos) = transform.rotation().sin_cos();
                            for uv in &mut uv {
                                let (x, y) = (uv[0] * sx, uv[1] * sy);
                                *uv = [ox + cos * x - sin * y, oy + sin * x + cos * y];
                            }
                        }
                        if uv.len() == points.len() && uv.iter().flatten().all(|x| x.is_finite()) {
                            result.texture = Some(id);
                            result.uv = uv;
                        }
                    }
                }
                if result.texture.is_none() { *loader.incomplete = true; }
            }
            Ok(result)
        }).transpose()?;
        Ok(Primitive { points, triangles, appearance })
    }).collect()
}

/// Keep file and decompression buffers alive while their accessors are in use.
pub fn with_scene<T>(
    path: &Path, load_appearance: bool, consume: impl FnOnce(&SceneMesh<'_>) -> Result<T>,
) -> Result<T> {
    with_scene_file(path, load_appearance, None, consume)
}

#[cfg(feature = "desktop")]
pub fn with_desktop_scene<T>(
    path: &Path, consume: impl FnOnce(&SceneMesh<'_>) -> Result<T>,
) -> Result<T> {
    let directory = path.parent().filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    with_scene_file(path, true, Some(directory), consume)
}

fn with_scene_file<T>(
    path: &Path, load_appearance: bool, directory: Option<&Path>,
    consume: impl FnOnce(&SceneMesh<'_>) -> Result<T>,
) -> Result<T> {
    let start = Instant::now();
    #[cfg(windows)]
    {
        let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or_default();
        let map_threshold = if extension.eq_ignore_ascii_case("gltf") {
            Some(4 * 1024 * 1024)
        } else if extension.eq_ignore_ascii_case("glb") {
            Some(16 * 1024 * 1024)
        } else {
            None
        };
        if let Some(threshold) = map_threshold {
            let metadata = path.metadata()?;
            if metadata.is_file() && metadata.len() > threshold {
                if let Ok((_file, bytes)) = super::obj::map_file(path) {
                    return with_scene_bytes(&bytes, path, load_appearance, directory, start, "mapped", consume);
                }
            }
        }
    }
    let bytes = std::fs::read(path)?;
    with_scene_bytes(&bytes, path, load_appearance, directory, start, "buffered", consume)
}

fn with_scene_bytes<T>(
    bytes: &[u8], path: &Path, load_appearance: bool, directory: Option<&Path>,
    start: Instant, input: &str,
    consume: impl FnOnce(&SceneMesh<'_>) -> Result<T>,
) -> Result<T> {
    let read = start.elapsed();
    let bundled = path.extension().and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gltfpack"));
    let (document_bytes, assets) = if bundled {
        read_bundle(bytes)?
    } else {
        (bytes, HashMap::new())
    };
    let directory = directory.filter(|_| !bundled);
    // Borrow the binary chunk directly instead of copying it out of the GLB.
    let container = if document_bytes.starts_with(b"glTF") {
        Some(gltf::binary::Glb::from_slice(document_bytes)?)
    } else { None };
    let json = container.as_ref().map_or(document_bytes, |glb| glb.json.as_ref());
    let (json, payloads) = embedded::prepare(json);
    let mut root: gltf::json::Root = serde_json::from_slice(&json)?;
    let quantized = root.extensions_used.iter().chain(&root.extensions_required)
        .any(|extension| extension == "KHR_mesh_quantization");
    root.extensions_required.retain(|extension| extension != "KHR_mesh_quantization");
    let compression = compressed::Plan::prepare(&mut root)?;
    let (document, mut draco) = draco::Plan::document(root)?;
    let parsed = start.elapsed();
    let mut external_paths = HashMap::<String, String>::new();
    let sources: Vec<Cow<[u8]>> = document.buffers().take(compression.original_buffers).map(|buffer| -> Result<Cow<[u8]>> {
        if compression.unused(buffer.index()) { return Ok(Cow::Borrowed(&[])); }
        let data = match buffer.source() {
            Source::Bin if buffer.index() == 0 => Cow::Borrowed(container.as_ref().and_then(|glb| glb.bin.as_deref())
                .ok_or("Missing GLB binary chunk")?),
            Source::Bin => return Err("glTF buffer has no data source".into()),
            Source::Uri(uri) if uri.starts_with("data:") => {
                let encoded = payloads.get(buffer.index()).copied().flatten()
                    .or_else(|| embedded::base64_payload(uri)).ok_or("Invalid glTF base64 buffer URI")?;
                Cow::Owned(embedded::decode(encoded)?)
            }
            Source::Uri(uri) => resolve_companion(uri, &assets, directory, &mut external_paths)?,
        };
        if data.len() < buffer.length() { return Err("Truncated glTF buffer".into()); }
        Ok(data)
    }).collect::<Result<_>>()?;
    let resolved = start.elapsed();
    let decoded = compression.decode(&sources)?;
    let mut buffers = sources;
    buffers.extend(decoded.iter().map(|view| Cow::Borrowed(view.as_bytes())));
    let decompressed = start.elapsed();
    let scene = document.default_scene().or_else(|| document.scenes().next()).ok_or("glTF has no scene")?;
    let mut result = SceneMesh {
        meshes: (0..document.meshes().len()).map(|_| None).collect(),
        instances: Vec::new(),
        count: 0,
        textures: HashMap::new(),
        incomplete_appearance: false,
    };
    let mut pending: Vec<(Node, Mat4)> = scene.nodes().map(|node| (node, Mat4::identity())).collect();
    pending.reverse();
    let mut visited = vec![false; document.nodes().len()];
    while let Some((node, parent)) = pending.pop() {
        if std::mem::replace(&mut visited[node.index()], true) {
            return Err("Invalid glTF scene hierarchy: repeated node".into());
        }
        let matrix = node.transform().matrix();
        let local = Mat4::from_fn(|row, col| matrix[col][row]);
        let world = parent * local;
        if !world.iter().all(|v| v.is_finite()) { return Err("Invalid glTF transform".into()); }
        if let Some(mesh) = node.mesh() {
            let index = mesh.index();
            if result.meshes[index].is_none() {
                let mut textures = TextureLoader {
                    buffers: &buffers, assets: &assets, directory, paths: &mut external_paths,
                    textures: &mut result.textures, incomplete: &mut result.incomplete_appearance,
                };
                result.meshes[index] = Some(decode(mesh, &buffers, quantized, load_appearance.then_some(&mut textures), &mut draco)?);
            }
            for primitive in result.meshes[index].as_ref().unwrap() {
                result.count = result.count.checked_add(primitive.triangles.len())
                    .ok_or("glTF triangle count overflow")?;
            }
            result.instances.push((index, world));
        }
        let first_child = pending.len();
        pending.extend(node.children().map(|child| (child, world)));
        pending[first_child..].reverse();
    }
    if result.count == 0 { return Err("glTF scene has no triangles".into()); }
    if std::env::var_os("STEPFLASH_PROFILE").is_some() {
        let geometry = start.elapsed() - decompressed;
        let ms = |t: std::time::Duration| t.as_secs_f64() * 1000.0;
        let mut borrowed_geometry_bytes = 0;
        for primitive in result.meshes.iter().flatten().flatten() {
            if let Cow::Borrowed(points) = &primitive.points {
                borrowed_geometry_bytes += std::mem::size_of_val(*points);
            }
            if let Cow::Borrowed(triangles) = &primitive.triangles {
                borrowed_geometry_bytes += std::mem::size_of_val(*triangles);
            }
        }
        eprintln!("glTF phases: read={:.2} ms, document={:.2} ms, buffers={:.2} ms, meshopt={:.2} ms, geometry={:.2} ms; input={input}, input_bytes={}, borrowed_payloads={}, external_buffers={}, compressed_views={}, decoded_bytes={}, borrowed_geometry_bytes={borrowed_geometry_bytes}, meshes={}, instances={}, triangles={}",
            ms(read), ms(parsed - read), ms(resolved - parsed), ms(decompressed - resolved), ms(geometry),
            bytes.len(), payloads.iter().flatten().count(), external_paths.len(),
            compression.jobs_count, compression.decoded_bytes,
            result.meshes.iter().flatten().count(), result.instances.len(), result.count);
        if draco.decoded != 0 {
            eprintln!("Draco bitstream={:.2} ms, decoded_primitives={}", ms(draco.elapsed), draco.decoded);
            use draco_core::decode_phase_probe as probe;
            if probe::enabled() {
                for (name, nanos) in probe::PHASE_NAMES.iter().zip(probe::take()) {
                    eprintln!("Draco {name}={:.2} ms", nanos as f64 / 1_000_000.0);
                }
            }
        }
    }
    consume(&result)
}
