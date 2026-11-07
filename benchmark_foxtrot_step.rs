use std::{env, fs, io::{self, Write}, time::Instant};
use step::step_file::StepFile;
use triangulate::{mesh::Mesh, triangulate::triangulate};

const CHUNK_TRIANGLES: usize = 8192;

fn write_frame(out: &mut impl Write, header: &str, payload: &[u8]) -> io::Result<()> {
    out.write_all(&(header.len() as u32).to_le_bytes())?;
    out.write_all(header.as_bytes())?;
    out.write_all(payload)
}

fn stream(mesh: &Mesh, read_ms: f64, total_ms: f64, errors: usize) -> io::Result<()> {
    let mut out = io::BufWriter::new(io::stdout().lock());
    let chunks = mesh.triangles.chunks(CHUNK_TRIANGLES);
    write_frame(&mut out, &format!(
        r#"{{"type":"meta","parts":{},"read_ms":{read_ms:.3},"payload_bytes":0}}"#,
        chunks.len()
    ), &[])?;
    let mut payload = Vec::with_capacity(CHUNK_TRIANGLES * 36);
    for chunk in chunks {
        payload.clear();
        for triangle in chunk {
            for &index in triangle.verts.iter() {
                for &coordinate in mesh.verts[index as usize].pos.iter() {
                    payload.extend_from_slice(&(coordinate as f32).to_le_bytes());
                }
            }
        }
        write_frame(&mut out, &format!(
            r#"{{"type":"part","triangles":{},"payload_bytes":{}}}"#,
            chunk.len(), payload.len()
        ), &payload)?;
    }
    write_frame(&mut out, &format!(
        r#"{{"type":"done","triangles":{},"total_ms":{total_ms:.3},"errors":{errors},"payload_bytes":0}}"#,
        mesh.triangles.len()
    ), &[])?;
    out.flush()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let path = args.next().expect("STEP path");
    let output = args.next();
    let start = Instant::now();
    let input = fs::read(path)?;
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
        stream(&mesh, parse.as_secs_f64()*1000.0, elapsed.as_secs_f64()*1000.0,
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
