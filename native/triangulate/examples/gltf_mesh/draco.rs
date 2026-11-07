use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    convert::TryFrom,
    ops::Range,
    time::{Duration, Instant},
};

use draco_core::{DataType as DracoType, DecodeLimits, DecoderBuffer, Mesh, MeshDecoder, PointAttribute};
use gltf::{
    accessor::DataType,
    json::{self, validation::{Error, Validate}},
    mesh::{Mode, Primitive},
    Accessor, Document,
};
use serde::Deserialize;

use super::{Result, Triangle};

const EXTENSION: &str = "KHR_draco_mesh_compression";
const MAX_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Descriptor {
    buffer_view: usize,
    attributes: HashMap<String, u32>,
}

struct Job {
    buffer: usize,
    range: Range<usize>,
    attributes: HashMap<String, u32>,
}

#[derive(Default)]
pub(super) struct Plan {
    jobs: HashMap<(usize, usize), Job>,
    used_bytes: u64,
    pub decoded: usize,
    pub elapsed: Duration,
}

impl Plan {
    pub(super) fn document(mut root: json::Root) -> Result<(Document, Self)> {
        let mut plan = Self::default();
        if !root.extensions_used.iter().chain(&root.extensions_required).any(|name| name == EXTENSION) {
            return Ok((Document::from_json(root)?, plan));
        }

        let mut supplied = HashSet::new();
        let mut mark = |index: usize| -> Result<()> {
            let accessor = root.accessors.get(index).ok_or("Invalid Draco accessor")?;
            if accessor.buffer_view.is_none() && accessor.sparse.is_none() {
                supplied.insert(json::Path::new().field("accessors").index(index).field("bufferView").0);
            }
            Ok(())
        };
        for (mesh_index, mesh) in root.meshes.iter().enumerate() {
            for (primitive_index, primitive) in mesh.primitives.iter().enumerate() {
                let Some(value) = primitive.extensions.as_ref().and_then(|e| e.others.get(EXTENSION)) else { continue };
                let descriptor: Descriptor = serde_json::from_value(value.clone())?;
                let view = root.buffer_views.get(descriptor.buffer_view).ok_or("Invalid Draco buffer view")?;
                let buffer = view.buffer.value();
                let length = root.buffers.get(buffer).ok_or("Invalid Draco source buffer")?.byte_length.0;
                let start = view.byte_offset.map_or(0, |offset| offset.0);
                let end = start.checked_add(view.byte_length.0).ok_or("Draco source range overflow")?;
                if start == end || end > length { return Err("Invalid Draco source range".into()); }

                let mut matched = 0;
                for (semantic, accessor) in &primitive.attributes {
                    if descriptor.attributes.contains_key(&semantic.to_string()) {
                        mark(accessor.value())?;
                        matched += 1;
                    }
                }
                if matched == 0 || matched != descriptor.attributes.len() {
                    return Err("Draco attributes must refer to primitive accessors".into());
                }
                if let Some(indices) = &primitive.indices { mark(indices.value())?; }
                plan.jobs.insert((mesh_index, primitive_index), Job {
                    buffer, range: usize::try_from(start)?..usize::try_from(end)?,
                    attributes: descriptor.attributes,
                });
            }
        }
        if plan.jobs.is_empty() {
            return Ok((Document::from_json(root)?, plan));
        }
        root.extensions_required.retain(|name| name != EXTENSION);

        // The glTF 1.4 validator predates this extension's viewless accessors.
        // Retain all other validation, including missing views on ordinary accessors.
        let mut errors = Vec::new();
        root.validate(&root, json::Path::new, &mut |path, error| {
            let path = path();
            if !matches!(error, Error::Missing) || !supplied.contains(path.as_str()) {
                errors.push((path, error));
            }
        });
        if !errors.is_empty() { return Err(gltf::Error::Validation(errors).into()); }
        Ok((Document::from_json_without_validation(root), plan))
    }

    pub(super) fn decode(
        &mut self, mesh_index: usize, primitive: &Primitive, buffers: &[Cow<[u8]>],
    ) -> Result<Option<Decoded<'_>>> {
        let Some(job) = self.jobs.get(&(mesh_index, primitive.index())) else { return Ok(None) };
        if !matches!(primitive.mode(), Mode::Triangles | Mode::TriangleStrip) {
            return Err("Invalid Draco primitive mode".into());
        }
        let source = buffers.get(job.buffer).and_then(|buffer| buffer.get(job.range.clone()))
            .ok_or("Truncated Draco buffer view")?;
        let remaining = MAX_BYTES - self.used_bytes;
        let face_limit = primitive.indices().filter(|_| primitive.mode() == Mode::Triangles)
            .map_or(remaining / 12, |accessor| (accessor.count() as u64 / 3).min(remaining / 12));
        let limits = DecodeLimits::default()
            .with_max_points(remaining / 20)
            .with_max_faces(face_limit)
            .with_max_decoded_bytes(MAX_BYTES);
        let mut mesh = Mesh::new();
        let start = Instant::now();
        MeshDecoder::new().decode(&mut DecoderBuffer::new(source).with_limits(limits), &mut mesh)?;
        self.elapsed += start.elapsed();
        // Reserve space for the positions, triangles and optional UVs kept by VTX.
        let bytes = (mesh.num_points() as u64) * 20 + (mesh.num_faces() as u64) * 12;
        if bytes > remaining { return Err("Draco scene geometry exceeds 512 MiB".into()); }
        self.used_bytes += bytes;
        self.decoded += 1;
        Ok(Some(Decoded { mesh, attributes: &job.attributes }))
    }
}

pub(super) struct Decoded<'a> {
    mesh: Mesh,
    attributes: &'a HashMap<String, u32>,
}

impl Decoded<'_> {
    pub(super) fn attribute<const N: usize>(
        &self, semantic: &str, accessor: &Accessor,
    ) -> Result<Option<Vec<[f32; N]>>>
    where
        [f32; N]: bytemuck::Pod,
        [i8; N]: bytemuck::Pod, [u8; N]: bytemuck::Pod,
        [i16; N]: bytemuck::Pod, [u16; N]: bytemuck::Pod,
    {
        let Some(&id) = self.attributes.get(semantic) else { return Ok(None) };
        let attribute = self.mesh.attribute_by_unique_id(id).ok_or("Missing Draco attribute ID")?;
        let count = self.mesh.num_points();
        if count != accessor.count() || attribute.num_components() as usize != N ||
            accessor.dimensions().multiplicity() != N
        {
            return Err("Draco attribute shape does not match its accessor".into());
        }

        let normalized = accessor.normalized();
        macro_rules! read_integer {
            ($ty:ty, $max:expr) => {
                expand(attribute, count, |x: $ty| {
                    let x = f32::from(<$ty>::from_le(x));
                    if normalized { (x / $max).max(-1.0) } else { x }
                })?
            };
        }
        let values = match (accessor.data_type(), attribute.data_type()) {
            (DataType::F32, DracoType::Float32) if !normalized => {
                expand(attribute, count, |x: f32| f32::from_bits(u32::from_le(x.to_bits())))?
            }
            (DataType::I8, DracoType::Int8) => read_integer!(i8, 127.0),
            (DataType::U8, DracoType::Uint8) => read_integer!(u8, 255.0),
            (DataType::I16, DracoType::Int16) => read_integer!(i16, 32767.0),
            (DataType::U16, DracoType::Uint16) => read_integer!(u16, 65535.0),
            _ => return Err("Draco component type does not match its accessor".into()),
        };
        Ok(Some(values))
    }

    pub(super) fn triangles(&self, primitive: &Primitive, positions: usize) -> Result<Vec<Triangle>> {
        if self.mesh.num_points() != positions {
            return Err("Draco point count does not match the positions".into());
        }
        let max_index = match primitive.indices() {
            Some(accessor) => {
                if accessor.dimensions() != gltf::accessor::Dimensions::Scalar || accessor.normalized() ||
                    (primitive.mode() == Mode::Triangles && accessor.count() != self.mesh.num_faces() * 3)
                {
                    return Err("Draco faces do not match the index accessor".into());
                }
                match accessor.data_type() {
                    DataType::U8 => u32::from(u8::MAX),
                    DataType::U16 => u32::from(u16::MAX),
                    DataType::U32 => u32::MAX,
                    _ => return Err("Invalid Draco index type".into()),
                }
            }
            None => u32::MAX,
        };
        let max_point = u32::try_from(positions)?.checked_sub(1).ok_or("Draco primitive has no positions")?;
        let bound = max_point.min(max_index);
        let mut faces = Vec::with_capacity(self.mesh.num_faces());
        for face in self.mesh.faces() {
            let [a, b, c] = face.map(|index| index.0);
            if a.max(b).max(c) > bound {
                return Err("Draco index exceeds the positions or the declared component type".into());
            }
            faces.push([a, b, c]);
        }
        Ok(faces)
    }
}

fn expand<T: Copy, const N: usize>(
    attribute: &PointAttribute, points: usize, convert: impl Fn(T) -> f32 + Copy,
) -> Result<Vec<[f32; N]>>
where
    [T; N]: bytemuck::Pod,
{
    let stride = usize::try_from(attribute.byte_stride())?;
    let width = std::mem::size_of::<[T; N]>();
    if stride < width { return Err("Invalid Draco attribute stride".into()); }
    let data = attribute.buffer().data();
    let mapping = attribute.explicit_mapping().map(|map| {
        map.get(..points).ok_or("Truncated Draco point mapping")
    }).transpose()?;

    // Use the packed rows directly when Draco's attribute buffer is aligned.
    // The explicit map keeps seams in point order when attribute values are shared.
    if stride == width {
        let length = attribute.size().checked_mul(width).ok_or("Draco attribute size overflow")?;
        let bytes = data.get(..length).ok_or("Truncated Draco attribute data")?;
        if let Ok(rows) = bytemuck::try_cast_slice::<u8, [T; N]>(bytes) {
            if let Some(mapping) = mapping {
                let mut values = Vec::with_capacity(points);
                for index in mapping {
                    let row = rows.get(index.0 as usize).ok_or("Invalid Draco point mapping")?;
                    values.push(row.map(convert));
                }
                return Ok(values);
            }
            let rows = rows.get(..points).ok_or("Truncated Draco attribute data")?;
            return Ok(rows.iter().map(|row| row.map(convert)).collect());
        }
    }

    let read = |index: usize| -> Result<[f32; N]> {
        if index >= attribute.size() { return Err("Invalid Draco point mapping".into()); }
        let offset = index.checked_mul(stride).ok_or("Draco attribute offset overflow")?;
        let bytes = data.get(offset..).and_then(|bytes| bytes.get(..width))
            .ok_or("Truncated Draco attribute data")?;
        Ok(bytemuck::pod_read_unaligned::<[T; N]>(bytes).map(convert))
    };
    if let Some(mapping) = mapping {
        mapping.iter().map(|index| read(index.0 as usize)).collect()
    } else {
        (0..points).map(read).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draco_core::{AttributeValueIndex, GeometryAttributeType};

    #[test]
    fn expands_shared_attribute_values_in_point_order() {
        let mut attribute = PointAttribute::new();
        attribute.init(GeometryAttributeType::TexCoord, 2, DracoType::Uint16, true, 2);
        let bytes: Vec<_> = [0u16, 256, 255, u16::MAX].iter().flat_map(|value| value.to_le_bytes()).collect();
        attribute.buffer_mut().update(&bytes, None);
        attribute.set_explicit_mapping_from(&[AttributeValueIndex(1), AttributeValueIndex(0), AttributeValueIndex(1)]);

        let convert = |value: u16| f32::from(u16::from_le(value)) / 65535.0;
        let values: Vec<[f32; 2]> = expand(&attribute, 3, convert).unwrap();
        assert_eq!(values, [[255.0 / 65535.0, 1.0], [0.0, 256.0 / 65535.0], [255.0 / 65535.0, 1.0]]);

        attribute.set_explicit_mapping_from(&[AttributeValueIndex(2)]);
        assert!(expand::<u16, 2>(&attribute, 1, convert).is_err());
    }
}
