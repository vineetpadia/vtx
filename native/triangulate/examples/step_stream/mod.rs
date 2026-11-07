use std::{io, time::Instant};

use triangulate::mesh::Mesh;

use super::indexed_stream::IndexedStream;

const FIRST_TRIANGLES: usize = 8_192;
const BATCH_TRIANGLES: usize = 32_768;
const UNMAPPED: u16 = u16::MAX;
const LOCAL_VERTEX_LIMIT: usize = UNMAPPED as usize;

fn position(mesh: &Mesh, index: usize) -> [f32; 3] {
    let point = mesh.verts[index].pos;
    [point.x as f32, point.y as f32, point.z as f32]
}

fn color(mesh: &Mesh, index: usize) -> [u8; 4] {
    let value = mesh.verts[index].color;
    let [r, g, b] = [value.x, value.y, value.z].map(|value| (value * 255.0).round() as u8);
    [r, g, b, 255]
}

pub fn stream(mesh: &Mesh, read_ms: f64, total_ms: f64, errors: usize) -> io::Result<()> {
    let mut writer = IndexedStream::with_read_time(read_ms, total_ms)?;
    // Start large previews before allocating or populating the vertex remap.
    let large = mesh.triangles.len() > BATCH_TRIANGLES;
    let preview = if large && !mesh.has_colors { FIRST_TRIANGLES } else { 0 };
    if preview != 0 {
        writer.packed_batch(mesh.triangles[..preview].iter().map(|triangle| {
            [0, 1, 2].map(|corner| position(mesh, triangle.verts[corner] as usize))
        }))?;
    }
    let mut remap = vec![UNMAPPED; mesh.verts.len()];
    let mut used = Vec::with_capacity(mesh.verts.len().min(LOCAL_VERTEX_LIMIT));
    let mut positions = Vec::with_capacity(used.capacity());
    let mut colors = if mesh.has_colors { Vec::with_capacity(used.capacity()) } else { Vec::new() };
    let mut indices = Vec::with_capacity(mesh.triangles.len().min(BATCH_TRIANGLES));
    let mut remaining = mesh.triangles[preview..].iter();
    let profile = std::env::var_os("STEPFLASH_PROFILE").is_some();
    let mut compact_time = std::time::Duration::ZERO;

    let mut batch_limit = if large && mesh.has_colors { FIRST_TRIANGLES } else { BATCH_TRIANGLES };
    while remaining.len() != 0 {
        let started = profile.then(Instant::now);
        while indices.len() < batch_limit && positions.len() <= LOCAL_VERTEX_LIMIT - 3 {
            let Some(triangle) = remaining.next() else { break; };
            let local = [0, 1, 2].map(|corner| {
                let global = triangle.verts[corner] as usize;
                let mapped = &mut remap[global];
                if *mapped == UNMAPPED {
                    *mapped = positions.len() as u16;
                    positions.push(position(mesh, global));
                    if mesh.has_colors {
                        colors.push(color(mesh, global));
                    }
                    used.push(global);
                }
                *mapped
            });
            indices.push(local);
        }
        if let Some(started) = started { compact_time += started.elapsed(); }
        writer.mesh_u16(&positions, &indices, mesh.has_colors.then_some(&colors))?;
        batch_limit = BATCH_TRIANGLES;
        for global in used.drain(..) {
            remap[global] = UNMAPPED;
        }
        positions.clear();
        colors.clear();
        indices.clear();
    }
    if profile {
        eprintln!("STEP stream compaction: {:.2} ms", compact_time.as_secs_f64() * 1000.0);
    }
    writer.finish_with_errors(errors)
}
