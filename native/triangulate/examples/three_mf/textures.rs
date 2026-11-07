use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    convert::TryFrom,
    io::Read,
};

use quick_xml::events::BytesStart;

use super::{
    attribute, finite, numeric_bytes, part_name, place_geometry, relationships, required,
    triangle_property, Archive, Color, Document, Matrix, Mesh, Object, Result, Triangle,
    DEFAULT_COLOR, TEXTURE_REL,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Png,
    Jpeg,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tile {
    Wrap,
    Mirror,
    Clamp,
}

pub struct Image {
    pub bytes: Vec<u8>,
    pub format: ImageFormat,
}

pub struct SurfaceTexture {
    pub image: usize,
    pub tiles: [Tile; 2],
    pub nearest: bool,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct TextureResource {
    path: String,
    format: ImageFormat,
    tiles: [Tile; 2],
    nearest: bool,
}

impl TextureResource {
    pub(super) fn parse(tag: &BytesStart<'_>, source: &str) -> Result<Self> {
        let format = match required(tag, b"contenttype")?.as_ref() {
            "image/png" => ImageFormat::Png,
            "image/jpeg" => ImageFormat::Jpeg,
            _ => return Err("Unsupported 3MF texture content type".into()),
        };
        let tile = |attribute_name| -> Result<Tile> {
            match attribute(tag, attribute_name)?.as_deref().unwrap_or("wrap") {
                "wrap" => Ok(Tile::Wrap),
                "mirror" => Ok(Tile::Mirror),
                // A standalone texture is opaque, so none uses the edge color.
                "clamp" | "none" => Ok(Tile::Clamp),
                _ => Err("Unsupported 3MF texture tile style".into()),
            }
        };
        let nearest = match attribute(tag, b"filter")?.as_deref().unwrap_or("auto") {
            "nearest" => true,
            "linear" | "auto" => false,
            _ => return Err("Unsupported 3MF texture filter".into()),
        };
        Ok(Self {
            path: part_name(&required(tag, b"path")?, source)?,
            format,
            tiles: [tile(b"tilestyleu")?, tile(b"tilestylev")?],
            nearest,
        })
    }
}

pub(super) struct TextureGroup {
    pub texture: u32,
    pub coordinates: Vec<[f32; 2]>,
}

pub(super) fn coordinates(tag: &BytesStart<'_>) -> Result<[f32; 2]> {
    let mut coordinates = [None; 2];
    let mut attributes = tag.attributes();
    attributes.with_checks(false);
    for attribute in attributes {
        let attribute = attribute?;
        let axis = match attribute.key.as_ref() {
            b"u" => 0,
            b"v" => 1,
            _ => continue,
        };
        let value = finite(numeric_bytes(&attribute)?)?;
        if coordinates[axis].replace(value).is_some() {
            return Err("Duplicate 3MF texture coordinate attribute".into());
        }
    }
    let [Some(u), Some(v)] = coordinates else {
        return Err("Incomplete 3MF texture coordinate".into());
    };
    let coordinates = [u as f32, (1.0 - v) as f32];
    if coordinates.iter().any(|value| !value.is_finite()) {
        return Err("3MF texture coordinates exceed the supported range".into());
    }
    Ok(coordinates)
}

#[derive(Default)]
pub(super) struct TextureBank {
    ids: HashMap<TextureResource, usize>,
    resources: Vec<TextureResource>,
    uses: HashMap<usize, HashSet<usize>>,
}

impl TextureBank {
    fn register(&mut self, part: usize, resource: &TextureResource) -> usize {
        let id = if let Some(&id) = self.ids.get(resource) {
            id
        } else {
            let id = self.resources.len();
            self.ids.insert(resource.clone(), id);
            self.resources.push(resource.clone());
            id
        };
        self.uses.entry(part).or_default().insert(id);
        id
    }

    pub(super) fn load(
        self,
        archive: &mut Archive,
        root: &str,
        referenced: &HashMap<String, usize>,
    ) -> Result<(Vec<Image>, Vec<SurfaceTexture>)> {
        if self.resources.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut names = vec![root; referenced.len() + 1];
        for (name, &id) in referenced {
            names[id] = name;
        }
        for (part, resources) in self.uses {
            let related: HashSet<_> = relationships(archive, names[part], TEXTURE_REL)?
                .into_iter()
                .collect();
            for resource in resources {
                let path = &self.resources[resource].path;
                if !related.contains(path) {
                    return Err(format!("Missing 3MF texture relationship for {}", path).into());
                }
            }
        }
        let mut images = Vec::new();
        let mut textures = Vec::with_capacity(self.resources.len());
        let mut image_ids = HashMap::new();
        let mut remaining = 256 * 1024 * 1024;
        for resource in self.resources {
            let key = (resource.path, resource.format);
            let image = match image_ids.get(&key) {
                Some(&id) => id,
                None => {
                    let file = archive.by_name(&key.0)?;
                    let size = file.size();
                    if size == 0 || size > 64 * 1024 * 1024 || size > remaining {
                        return Err("3MF texture data exceeds the decode limit".into());
                    }
                    remaining -= size;
                    let mut bytes = Vec::with_capacity(size as usize);
                    file.take(size + 1).read_to_end(&mut bytes)?;
                    if bytes.len() as u64 != size {
                        return Err("3MF texture size differs from the archive directory".into());
                    }
                    let id = images.len();
                    images.push(Image {
                        bytes,
                        format: resource.format,
                    });
                    image_ids.insert(key, id);
                    id
                }
            };
            textures.push(SurfaceTexture {
                image,
                tiles: resource.tiles,
                nearest: resource.nearest,
            });
        }
        Ok((images, textures))
    }
}

#[derive(Default)]
struct Run {
    texture: Option<u32>,
    points: Vec<[f64; 3]>,
    indices: Vec<Triangle>,
    attributes: Vec<[u32; 2]>,
    seams: HashMap<(u32, [u32; 2]), u32>,
}

impl Run {
    fn push(
        &mut self,
        triangle: Triangle,
        attributes: [[u32; 2]; 3],
        points: &[[f64; 3]],
        first: &mut [[u32; 2]],
        generation: u32,
    ) -> Result<()> {
        let mut result = [0; 3];
        let vertices = &mut self.points;
        let values = &mut self.attributes;
        for (corner, (vertex, attribute)) in triangle.iter().copied().zip(attributes).enumerate() {
            let original = vertex as usize;
            let [seen, index] = &mut first[original];
            // Most corners share the first UV of their vertex. Only seams need hashing.
            if *seen == generation && values[*index as usize] == attribute {
                result[corner] = *index;
                continue;
            }
            let mut append = || -> Result<u32> {
                let index = u32::try_from(vertices.len())?;
                vertices.push(points[original]);
                values.push(attribute);
                Ok(index)
            };
            result[corner] = if *seen != generation {
                *index = append()?;
                *seen = generation;
                *index
            } else {
                match self.seams.entry((vertex, attribute)) {
                    Entry::Occupied(value) => *value.get(),
                    Entry::Vacant(slot) => *slot.insert(append()?),
                }
            };
        }
        self.indices.push(result);
        Ok(())
    }

    fn finish(
        self,
        document: &Document,
        placements: &[Matrix],
        unit: f64,
        part: usize,
        bank: &mut TextureBank,
    ) -> Result<Mesh> {
        let Self {
            texture,
            points,
            mut indices,
            attributes,
            ..
        } = self;
        let (points, instances) = place_geometry(points, &mut indices, placements, unit);
        let (color, colors, texture) = if let Some(id) = texture {
            let resource = document
                .textures
                .get(&id)
                .ok_or("Missing 3MF texture resource")?;
            let key = bank.register(part, resource);
            let coordinates = attributes
                .into_iter()
                .map(|bits| bits.map(f32::from_bits))
                .collect();
            (Some([255; 4]), None, Some((key, coordinates)))
        } else {
            let colors = attributes
                .into_iter()
                .map(|bits| bits[0].to_le_bytes())
                .collect();
            (None, Some(colors), None)
        };
        Ok(Mesh {
            points,
            triangles: indices,
            color,
            colors,
            texture,
            instances,
        })
    }
}

impl Document {
    pub(super) fn has_texture(&self, object: &Object) -> bool {
        !self.texture_groups.is_empty()
            && (object
                .default_property
                .is_some_and(|(pid, _)| self.texture_groups.contains_key(&pid))
                || object.properties.iter().any(|(_, property)| {
                    property[0].is_some_and(|pid| self.texture_groups.contains_key(&pid))
                }))
    }

    pub(super) fn textured_meshes(
        &self,
        object: Object,
        placements: &[Matrix],
        unit: f64,
        part: usize,
        bank: &mut TextureBank,
        issues: &mut usize,
    ) -> Result<Vec<Mesh>> {
        let mut meshes = Vec::new();
        let mut run = Run::default();
        let mut first = vec![[u32::MAX, 0]; object.points.len()];
        let mut generation = 0;
        let mut last_group = None;
        let mut overrides = object.properties.into_iter().peekable();
        for (face, triangle) in object.triangles.into_iter().enumerate() {
            let property = if overrides.peek().is_some_and(|(index, _)| *index == face) {
                Some(triangle_property(
                    object.default_property,
                    overrides.next().unwrap().1,
                )?)
            } else {
                object
                    .default_property
                    .map(|(pid, index)| (pid, [index; 3]))
            };
            let mut texture = None;
            let mut values = [[u32::from_le_bytes(DEFAULT_COLOR), 0]; 3];
            if let Some((pid, indices)) = property {
                if last_group.map(|(key, _)| key) != Some(pid) {
                    last_group = Some((pid, self.texture_groups.get(&pid)));
                }
                if let Some((_, Some(group))) = last_group {
                    texture = Some(group.texture);
                    for (output, index) in values.iter_mut().zip(indices) {
                        *output = group
                            .coordinates
                            .get(index as usize)
                            .ok_or("Invalid 3MF texture coordinate index")?
                            .map(f32::to_bits);
                    }
                } else {
                    let colors: [Color; 3] = self.display_colors(pid, indices, issues)?;
                    values = colors.map(|color| [u32::from_le_bytes(color), 0]);
                }
            }
            if run.texture != texture && !run.indices.is_empty() {
                meshes.push(run.finish(self, placements, unit, part, bank)?);
                run = Run::default();
                generation += 1;
            }
            run.texture = texture;
            run.push(triangle, values, &object.points, &mut first, generation)?;
        }
        if !run.indices.is_empty() {
            meshes.push(run.finish(self, placements, unit, part, bank)?);
        }
        Ok(meshes)
    }
}
