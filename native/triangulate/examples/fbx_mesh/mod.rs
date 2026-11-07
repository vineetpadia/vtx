use std::{error::Error, path::Path, time::Instant};

#[cfg(feature = "desktop")]
pub mod appearance;

type Triangle = [u32; 3];
type Point = [f32; 3];
type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Mesh {
    points: Vec<ufbx::Vec3>,
    triangles: Vec<Triangle>,
}

pub struct SceneMesh {
    meshes: Vec<Option<Mesh>>,
    instances: Vec<(usize, ufbx::Matrix)>,
    pub count: usize,
}

pub struct Part<'a> {
    pub points: Vec<Point>,
    pub triangles: &'a [Triangle],
    pub mirrored: bool,
}

fn facing([a, b, c]: Triangle, mirrored: bool) -> Triangle {
    if mirrored { [a, c, b] } else { [a, b, c] }
}

impl SceneMesh {
    pub fn parts(&self) -> impl Iterator<Item = Part<'_>> + '_ {
        self.instances.iter().map(move |(id, world)| {
            let mesh = self.meshes[*id].as_ref().unwrap();
            let points = mesh.points.iter().map(|&p| {
                let p = ufbx::transform_position(world, p);
                [p.x as f32, p.y as f32, p.z as f32]
            }).collect();
            Part {
                points, triangles: &mesh.triangles,
                mirrored: ufbx::matrix_determinant(world) < 0.0,
            }
        })
    }

    pub fn triangles(&self) -> impl Iterator<Item = [Point; 3]> + '_ {
        self.parts().flat_map(|part| {
            part.triangles.iter().map(move |&triangle| {
                facing(triangle, part.mirrored).map(|i| part.points[i as usize])
            })
        })
    }
}

fn triangulate(mesh: &ufbx::Mesh, mut emit: impl FnMut(usize, Triangle)) -> Result<()> {
    let mut scratch = vec![0; mesh.max_face_triangles * 3];
    for (face_id, &face) in mesh.faces.iter().enumerate() {
        if face.num_indices < 3 { continue; }
        if face.num_indices == 3 {
            let begin = face.index_begin;
            emit(face_id, [begin, begin + 1, begin + 2]);
            continue;
        }
        let count = ufbx::triangulate_face(&mut scratch, mesh, face) as usize;
        if count == 0 { return Err("Could not triangulate an FBX polygon".into()); }
        for corners in scratch[..count * 3].chunks_exact(3) {
            emit(face_id, [corners[0], corners[1], corners[2]]);
        }
    }
    Ok(())
}

fn decode(mesh: &ufbx::Mesh) -> Result<Mesh> {
    let mut triangles = Vec::with_capacity(mesh.num_triangles);
    triangulate(mesh, |_, corners| {
        triangles.push(corners.map(|corner| mesh.vertex_indices[corner as usize]));
    })?;
    Ok(Mesh { points: mesh.vertices.to_vec(), triangles })
}

pub fn load(path: &Path) -> Result<SceneMesh> {
    let start = Instant::now();
    let path = path.to_str().ok_or("FBX path is not Unicode")?;
    let scene = ufbx::load_file(path, ufbx::LoadOpts {
        ignore_animation: true,
        ignore_embedded: true,
        load_external_files: false,
        ..Default::default()
    }).map_err(|error| format!("FBX import: {}", error.description))?;
    let parsed = start.elapsed();
    let mut result = SceneMesh {
        meshes: (0..scene.meshes.len()).map(|_| None).collect(),
        instances: Vec::new(),
        count: 0,
    };
    for node in &scene.nodes {
        if let Some(mesh) = &node.mesh {
            let id = mesh.element.typed_id as usize;
            if result.meshes[id].is_none() {
                result.meshes[id] = Some(decode(mesh)?);
            }
            let count = result.meshes[id].as_ref().unwrap().triangles.len();
            if count > 0 {
                result.count = result.count.checked_add(count).ok_or("FBX triangle count overflow")?;
                result.instances.push((id, node.geometry_to_world));
            }
        }
    }
    if result.count == 0 { return Err("FBX scene has no polygon faces".into()); }
    if std::env::var_os("STEPFLASH_PROFILE").is_some() {
        let ms = |t: std::time::Duration| t.as_secs_f64() * 1000.0;
        eprintln!("FBX phases: file+parse={:.2} ms, triangulation={:.2} ms; loader_temp_bytes={}, loader_scene_bytes={}, meshes={}, instances={}, triangles={}",
            ms(parsed), ms(start.elapsed() - parsed), scene.metadata.temp_memory_used, scene.metadata.result_memory_used,
            result.meshes.iter().flatten().count(), result.instances.len(), result.count);
    }
    Ok(result)
}
