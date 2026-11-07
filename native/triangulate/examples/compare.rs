use std::{env, fs, io::{self, Write}, path::Path, time::Instant};
use step::step_file::StepFile;
use triangulate::triangulate::triangulate;

mod obj;
mod gltf_mesh;
mod fbx_mesh;
mod indexed_stream;
mod ply;
mod step_stream;

fn write_frame(out: &mut impl Write, header: &str, payload: &[u8]) -> io::Result<()> {
    out.write_all(&(header.len() as u32).to_le_bytes())?;
    out.write_all(header.as_bytes())?;
    out.write_all(payload)
}

fn output_mesh(
    count: usize, triangles: impl Iterator<Item = [[f32; 3]; 3]>,
    total: std::time::Duration, output: Option<&str>,
) -> io::Result<()> {
    let ms = total.as_secs_f64() * 1000.0;
    println!("total_ms={ms:.3} triangles={count}");
    if let Some(path) = output {
        let mut file = io::BufWriter::new(fs::File::create(path)?);
        for triangle in triangles {
            for coordinate in triangle.iter().flatten() {
                file.write_all(&coordinate.to_le_bytes())?;
            }
        }
        file.flush()?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let path = args.next().ok_or("Expected a STEP, OBJ, PLY, glTF, GLB, or FBX path")?;
    let output = args.next();
    let start = Instant::now();
    let extension = Path::new(&path).extension().and_then(|ext| ext.to_str())
        .unwrap_or_default().to_ascii_lowercase();
    if extension == "obj" {
        let mesh = obj::load(Path::new(&path))?;
        let total = start.elapsed();
        if output.as_deref() == Some("--stream") {
            let mut writer = indexed_stream::IndexedStream::new(total.as_secs_f64() * 1000.0)?;
            writer.mesh(&mesh.positions, mesh.count, mesh.triangles.iter().map(Vec::as_slice), false, None, None)?;
            writer.finish()?;
            return Ok(());
        }
        let triangles = mesh.triangles.iter().flatten().map(|tri| tri.map(|id| mesh.positions[id as usize]));
        output_mesh(mesh.count, triangles, total, output.as_deref())?;
        return Ok(());
    }
    if extension == "ply" {
        let model = ply::load(Path::new(&path))?;
        let total = start.elapsed();
        if output.as_deref() == Some("--stream") {
            let mut writer = indexed_stream::IndexedStream::new(total.as_secs_f64() * 1000.0)?;
            if model.triangles.is_empty() {
                writer.point_cloud(&model.positions, model.colors.as_deref())?;
            } else {
                writer.mesh(&model.positions, model.triangles.len(), [model.triangles.as_slice()],
                            false, None, model.colors.as_deref())?;
            }
            writer.finish_with_warning(if model.gaussian {
                "Gaussian splat attributes are displayed as points."
            } else { "" })?;
        } else if model.triangles.is_empty() {
            println!("total_ms={:.3} triangles=0 points={}",
                     total.as_secs_f64() * 1000.0, model.positions.len());
            if let Some(path) = output.as_deref() {
                let mut file = io::BufWriter::new(fs::File::create(path)?);
                for coordinate in model.positions.iter().flatten() {
                    file.write_all(&coordinate.to_le_bytes())?;
                }
                file.flush()?;
            }
        } else {
            let triangles = model.triangles.iter().map(|face| face.map(|id| model.positions[id as usize]));
            output_mesh(model.triangles.len(), triangles, total, output.as_deref())?;
        }
        return Ok(());
    }
    if matches!(extension.as_str(), "glb" | "gltf" | "gltfpack") {
        let with_appearance = output.as_deref() == Some("--stream");
        return gltf_mesh::with_scene(Path::new(&path), with_appearance, |mesh| {
            let total = start.elapsed();
            if with_appearance {
                let mut writer = indexed_stream::IndexedStream::new(total.as_secs_f64() * 1000.0)?;
                let mut used = std::collections::BTreeSet::new();
                for part in mesh.parts() {
                    writer.mesh(&part.points, part.triangles.len(), [part.triangles], part.mirrored, part.appearance, None)?;
                    if let Some(id) = part.appearance.and_then(|material| material.texture) {
                        used.insert(id);
                    }
                }
                for id in used {
                    writer.texture(id, mesh.textures[&id].as_ref().unwrap())?;
                }
                writer.finish_with_warning(if mesh.incomplete_appearance {
                    "Some glTF textures or transparency settings could not be displayed."
                } else { "" })?;
            } else {
                output_mesh(mesh.count, mesh.triangles(), total, output.as_deref())?;
            }
            Ok(())
        });
    }
    if extension == "fbx" {
        let mesh = fbx_mesh::load(Path::new(&path))?;
        let total = start.elapsed();
        if output.as_deref() == Some("--stream") {
            let mut writer = indexed_stream::IndexedStream::new(total.as_secs_f64() * 1000.0)?;
            for part in mesh.parts() {
                writer.mesh(&part.points, part.triangles.len(), [part.triangles], part.mirrored, None, None)?;
            }
            writer.finish()?;
        } else {
            output_mesh(mesh.count, mesh.triangles(), total, output.as_deref())?;
        }
        return Ok(());
    }
    let input = fs::read(&path)?;
    let read = start.elapsed();
    let flat = StepFile::strip_flatten(&input);
    let scan = start.elapsed();
    let step = StepFile::parse(&flat);
    let parse = start.elapsed();
    let (mesh, stats) = triangulate(&step);
    let elapsed = start.elapsed();
    if output.as_deref() == Some("--stream") {
        if mesh.triangles.is_empty() {
            return Err("native engine returned no triangles".into());
        }
        step_stream::stream(&mesh, parse.as_secs_f64()*1000.0, elapsed.as_secs_f64()*1000.0,
            stats.num_errors + stats.num_panics)?;
        return Ok(());
    }
    println!("read_ms={:.3} scan_ms={:.3} parse_ms={:.3} mesh_ms={:.3} total_ms={:.3} vertices={} triangles={} faces={} errors={} panics={}",
        read.as_secs_f64()*1000.0, (scan-read).as_secs_f64()*1000.0, (parse-scan).as_secs_f64()*1000.0,
        (elapsed-parse).as_secs_f64()*1000.0, elapsed.as_secs_f64()*1000.0,
        mesh.verts.len(), mesh.triangles.len(), stats.num_faces, stats.num_errors, stats.num_panics);
    if let Some(output) = output { mesh.save_stl(&output)?; }
    Ok(())
}
