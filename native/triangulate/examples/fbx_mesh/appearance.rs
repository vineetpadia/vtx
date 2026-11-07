use std::{
    borrow::Cow,
    collections::HashMap,
    convert::TryFrom,
    fs::File,
    io::Read,
    path::{Component, Path},
    time::Instant,
};

use super::{triangulate, Point, Result, Triangle};

type Placement = ([[f32; 4]; 3], bool);

pub struct Surface<'a> {
    pub points: Vec<Point>,
    pub triangles: Vec<Triangle>,
    pub instances: Vec<Placement>,
    pub color: Option<[f32; 4]>,
    pub uv: Option<Vec<[f32; 2]>>,
    pub texture: Option<&'a ufbx::Texture>,
}

struct Batch {
    slot: usize,
    corners: Vec<Triangle>,
}

fn batches(mesh: &ufbx::Mesh) -> Result<Vec<Batch>> {
    if mesh.face_material.is_empty() || mesh.material_parts.len() <= 1 {
        let mut corners = Vec::with_capacity(mesh.num_triangles);
        triangulate(mesh, |_, triangle| corners.push(triangle))?;
        return Ok(vec![Batch {
            slot: mesh.face_material.first().copied().unwrap_or(0) as usize,
            corners,
        }]);
    }
    let mut indices = HashMap::new();
    let mut batches = Vec::new();
    triangulate(mesh, |face, triangle| {
        let slot = mesh.face_material[face] as usize;
        let id = *indices.entry(slot).or_insert_with(|| {
            let id = batches.len();
            batches.push(Batch {
                slot,
                corners: Vec::new(),
            });
            id
        });
        batches[id].corners.push(triangle);
    })?;
    Ok(batches)
}

#[derive(Default)]
struct Group {
    mesh: usize,
    batch: usize,
    material: Option<u32>,
    worlds: Vec<ufbx::Matrix>,
}

pub fn with_surfaces(
    path: &Path,
    mut emit: impl FnMut(Surface<'_>) -> Result<()>,
) -> Result<usize> {
    let start = Instant::now();
    let source = path.to_str().ok_or("FBX path is not Unicode")?;
    let scene = ufbx::load_file(
        source,
        ufbx::LoadOpts {
            ignore_animation: true,
            load_external_files: false,
            ..Default::default()
        },
    )
    .map_err(|error| format!("FBX import: {}", error.description))?;
    let parsed = start.elapsed();
    let mut topology = (0..scene.meshes.len()).map(|_| None).collect::<Vec<_>>();
    let mut indices = HashMap::new();
    let mut groups: Vec<Group> = Vec::new();
    for node in &scene.nodes {
        let Some(mesh) = &node.mesh else { continue };
        let mesh_id = mesh.element.typed_id as usize;
        if topology[mesh_id].is_none() {
            topology[mesh_id] = Some(batches(mesh)?);
        }
        for (batch_id, batch) in topology[mesh_id].as_ref().unwrap().iter().enumerate() {
            if batch.corners.is_empty() {
                continue;
            }
            let material = node
                .materials
                .get(batch.slot)
                .or_else(|| mesh.materials.get(batch.slot))
                .map(|mat| mat.element.typed_id);
            let key = (mesh_id, batch_id, material);
            let id = *indices.entry(key).or_insert_with(|| {
                let id = groups.len();
                groups.push(Group {
                    mesh: mesh_id,
                    batch: batch_id,
                    material,
                    worlds: Vec::new(),
                });
                id
            });
            groups[id].worlds.push(node.geometry_to_world);
        }
    }
    if groups.is_empty() {
        return Err("FBX scene has no polygon faces".into());
    }
    let triangulated = start.elapsed();
    let mut prepare = std::time::Duration::ZERO;
    let mut warnings = 0;
    let mut copies = 0;
    for group in &groups {
        let start = Instant::now();
        let mesh = &scene.meshes[group.mesh];
        let batches = topology[group.mesh].as_ref().unwrap();
        let batch = &batches[group.batch];
        let material = group.material.map(|id| &scene.materials[id as usize]);
        let surface = prepare_surface(
            mesh,
            batch,
            batches.len() == 1,
            material,
            &group.worlds,
            &mut warnings,
        )?;
        copies += group.worlds.len();
        prepare += start.elapsed();
        emit(surface)?;
    }
    if std::env::var_os("STEPFLASH_PROFILE").is_some() {
        let ms = |t: std::time::Duration| t.as_secs_f64() * 1000.0;
        eprintln!("FBX appearance phases: file+parse={:.2} ms, triangulation={:.2} ms, vertices+uv={:.2} ms; surfaces={}, surface_placements={}, loader_scene_bytes={}",
            ms(parsed), ms(triangulated - parsed), ms(prepare), groups.len(), copies,
            scene.metadata.result_memory_used);
    }
    Ok(warnings)
}

fn prepare_surface<'a>(
    mesh: &ufbx::Mesh,
    batch: &Batch,
    whole_mesh: bool,
    material: Option<&'a ufbx::Material>,
    worlds: &[ufbx::Matrix],
    warnings: &mut usize,
) -> Result<Surface<'a>> {
    let mut color = None;
    let mut texture = None;
    let mut coordinates = None;
    if let Some(material) = material {
        let base = &material.pbr.base_color;
        if base.has_value {
            let v = base.value_vec4;
            let factor = material.pbr.base_factor.value_vec4.x;
            let rgb = [v.x, v.y, v.z].map(|channel| (channel * factor).clamp(0.0, 1.0) as f32);
            if rgb.iter().all(|c| c.is_finite()) {
                color = Some([rgb[0], rgb[1], rgb[2], 1.0]);
            }
        }
        if let Some(asset) = base.texture.as_deref().filter(|_| base.texture_enabled) {
            coordinates = if asset.uv_set.is_empty() {
                mesh.vertex_uv.exists.then_some(&mesh.vertex_uv)
            } else {
                mesh.uv_sets
                    .iter()
                    .find(|set| set.name.as_ref() == asset.uv_set.as_ref())
                    .map(|set| &set.vertex_uv)
            };
            if asset.type_ == ufbx::TextureType::File && coordinates.is_some() {
                texture = Some(asset);
                color.get_or_insert([1.0; 4]);
            } else {
                coordinates = None;
                *warnings += 1;
            }
        }
    }
    let world = match worlds {
        [matrix] => Some(matrix),
        _ => None,
    };
    let position = |vertex: u32| {
        let mut point = mesh.vertices[vertex as usize];
        if let Some(world) = world {
            point = ufbx::transform_position(world, point);
        }
        [point.x as f32, point.y as f32, point.z as f32]
    };
    let mut triangles = Vec::with_capacity(batch.corners.len());
    let (points, uv) = if coordinates.is_none() && whole_mesh {
        triangles.extend(
            batch
                .corners
                .iter()
                .map(|triangle| triangle.map(|corner| mesh.vertex_indices[corner as usize])),
        );
        (
            (0..mesh.vertices.len() as u32).map(position).collect(),
            None,
        )
    } else {
        let mut points = Vec::new();
        let mut uv = Vec::new();
        let mut first = vec![[u32::MAX; 2]; mesh.vertices.len()];
        let mut seams = HashMap::new();
        let mut missing = false;
        for triangle in &batch.corners {
            let mut indices = [0; 3];
            for (output, &corner) in indices.iter_mut().zip(triangle) {
                let vertex = mesh.vertex_indices[corner as usize];
                let coordinate = coordinates
                    .and_then(|coords| coords.indices.get(corner as usize).copied())
                    .unwrap_or(u32::MAX);
                let [seen, mapped] = &mut first[vertex as usize];
                if *mapped != u32::MAX && *seen == coordinate {
                    *output = *mapped;
                    continue;
                }
                let mut append = || -> Result<u32> {
                    let index = u32::try_from(points.len())?;
                    points.push(position(vertex));
                    if let (Some(coords), Some(asset)) = (coordinates, texture) {
                        let coord = coords
                            .values
                            .get(coordinate as usize)
                            .copied()
                            .unwrap_or_else(|| {
                                missing = true;
                                ufbx::Vec2 { x: 0.0, y: 0.0 }
                            });
                        let mut coord = ufbx::Vec3 {
                            x: coord.x,
                            y: coord.y,
                            z: 0.0,
                        };
                        if asset.has_uv_transform {
                            coord = ufbx::transform_position(&asset.uv_to_texture, coord);
                        }
                        let value = [coord.x as f32, (1.0 - coord.y) as f32];
                        if !value.iter().all(|v| v.is_finite()) {
                            return Err("Invalid FBX texture coordinate".into());
                        }
                        uv.push(value);
                    }
                    Ok(index)
                };
                *output = if *mapped == u32::MAX {
                    *seen = coordinate;
                    *mapped = append()?;
                    *mapped
                } else {
                    match seams.entry((vertex, coordinate)) {
                        std::collections::hash_map::Entry::Occupied(slot) => *slot.get(),
                        std::collections::hash_map::Entry::Vacant(slot) => *slot.insert(append()?),
                    }
                };
            }
            triangles.push(indices);
        }
        *warnings += usize::from(missing);
        (points, texture.map(|_| uv))
    };
    if world.is_some_and(|matrix| ufbx::matrix_determinant(matrix) < 0.0) {
        for triangle in &mut triangles {
            triangle.swap(1, 2);
        }
    }
    let instances = if world.is_some() {
        Vec::new()
    } else {
        worlds
            .iter()
            .map(|m| {
                (
                    [
                        [m.m00 as f32, m.m01 as f32, m.m02 as f32, m.m03 as f32],
                        [m.m10 as f32, m.m11 as f32, m.m12 as f32, m.m13 as f32],
                        [m.m20 as f32, m.m21 as f32, m.m22 as f32, m.m23 as f32],
                    ],
                    ufbx::matrix_determinant(m) < 0.0,
                )
            })
            .collect()
    };
    Ok(Surface {
        points,
        triangles,
        instances,
        color,
        uv,
        texture,
    })
}

pub fn texture_bytes<'a>(source: &Path, asset: &'a ufbx::Texture) -> Result<Cow<'a, [u8]>> {
    const MAX_BYTES: u64 = 64 * 1024 * 1024;
    if asset.content.size != 0 {
        if asset.content.size as u64 > MAX_BYTES {
            return Err("Embedded FBX image exceeds the size limit".into());
        }
        return Ok(Cow::Borrowed(asset.content.as_ref()));
    }
    let relative = asset.relative_filename.replace('\\', "/");
    let path = Path::new(&relative);
    if relative.contains(':')
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err("FBX image path is outside the model folder".into());
    }
    let folder = source
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    let path = folder.join(path).canonicalize()?;
    if !path.starts_with(&folder) || !path.is_file() {
        return Err("FBX image path is outside the model folder".into());
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("FBX image exceeds the size limit".into());
    }
    Ok(Cow::Owned(bytes))
}
