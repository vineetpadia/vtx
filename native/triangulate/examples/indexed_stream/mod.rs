use std::{convert::TryInto, io::{self, Write}, time::{Duration, Instant}};

use super::write_frame;
use super::gltf_mesh::{Appearance, Texture};

const RECORDS: usize = 16_384;

fn position_bounds(positions: &[[f32; 3]]) -> Option<([f64; 3], [f64; 3])> {
    // Keep four independent XYZ accumulators so each block can be reduced in lanes.
    let mut blocks = positions.as_flattened().chunks_exact(12);
    let mut lower = [f32::INFINITY; 12];
    let mut upper = [f32::NEG_INFINITY; 12];
    let mut finite = true;
    for block in &mut blocks {
        let block: &[f32; 12] = block.try_into().unwrap();
        for (lane, &value) in block.iter().enumerate() {
            finite &= value.is_finite();
            if value < lower[lane] { lower[lane] = value; }
            if value > upper[lane] { upper[lane] = value; }
        }
    }
    if !finite { return None; }

    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for lane in 0..12 {
        let axis = lane % 3;
        minimum[axis] = minimum[axis].min(lower[lane]);
        maximum[axis] = maximum[axis].max(upper[lane]);
    }
    for point in blocks.remainder().chunks_exact(3) {
        for (axis, &value) in point.iter().enumerate() {
            if !value.is_finite() { return None; }
            minimum[axis] = minimum[axis].min(value);
            maximum[axis] = maximum[axis].max(value);
        }
    }
    Some((minimum.map(f64::from), maximum.map(f64::from)))
}

pub struct IndexedStream {
    out: io::BufWriter<io::StdoutLock<'static>>,
    payload: Vec<u8>,
    start: Instant,
    bytes: usize,
    draws: usize,
    import_ms: f64,
    triangles: usize,
    points: usize,
    profile: bool,
    bounds_time: Duration,
}

impl IndexedStream {
    pub fn new(import_ms: f64) -> io::Result<Self> {
        Self::with_read_time(import_ms, import_ms)
    }

    pub fn with_read_time(read_ms: f64, import_ms: f64) -> io::Result<Self> {
        let mut out = io::BufWriter::new(io::stdout().lock());
        write_frame(&mut out, &format!(
            r#"{{"type":"meta","read_ms":{read_ms:.3},"payload_bytes":0}}"#
        ), &[])?;
        Ok(Self {
            out, payload: Vec::with_capacity(RECORDS * 12), start: Instant::now(),
            bytes: 0, draws: 0, import_ms, triangles: 0, points: 0,
            profile: std::env::var_os("STEPFLASH_PROFILE").is_some(), bounds_time: Duration::ZERO,
        })
    }

    pub fn mesh<'a>(
        &mut self, points: &[[f32; 3]], count: usize,
        triangles: impl IntoIterator<Item = &'a [[u32; 3]]>,
        mirrored: bool, appearance: Option<&Appearance>, vertex_colors: Option<&[[u8; 4]]>,
    ) -> io::Result<()> {
        if count == 0 { return Ok(()); }
        let mut header = serde_json::json!({
            "type": "indexed", "vertices": points.len(), "triangles": count, "payload_bytes": 0
        });
        if let Some(appearance) = appearance {
            header["color"] = serde_json::json!(appearance.color);
            header["cutoff"] = serde_json::json!(appearance.cutoff);
            header["texture"] = serde_json::json!(appearance.texture);
        }
        if vertex_colors.is_some() {
            header["vertex_colors"] = serde_json::json!(true);
        }
        write_frame(&mut self.out, &header.to_string(), &[])?;
        self.vertices(points, vertex_colors)?;
        if let Some(appearance) = appearance {
            for chunk in appearance.uv.chunks(RECORDS) {
                self.float_batch("uv", chunk, None)?;
            }
        }
        let mut batches = triangles.into_iter().filter(|batch| !batch.is_empty());
        let mut batch = &[][..];
        for offset in (0..count).step_by(RECORDS) {
            let mut remaining = (count - offset).min(RECORDS);
            self.batch_header("indices", remaining * 12)?;
            // Keep frame sizes independent of the importer's chunk boundaries.
            while remaining > 0 {
                if batch.is_empty() {
                    batch = batches.next().ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "Incomplete triangle batches")
                    })?;
                }
                let chunk;
                (chunk, batch) = batch.split_at(remaining.min(batch.len()));
                remaining -= chunk.len();
                if cfg!(target_endian = "little") && !mirrored {
                    self.out.write_all(bytemuck::cast_slice(chunk))?;
                    continue;
                }
                self.payload.clear();
                for &[a, b, c] in chunk {
                    let indices = if mirrored { [a, c, b] } else { [a, b, c] };
                    for index in indices.iter() {
                        self.payload.extend_from_slice(&index.to_le_bytes());
                    }
                }
                self.out.write_all(&self.payload)?;
            }
        }
        self.triangles += count;
        self.draws += 1;
        Ok(())
    }

    pub fn point_cloud(&mut self, points: &[[f32; 3]], colors: Option<&[[u8; 4]]>) -> io::Result<()> {
        let header = serde_json::json!({
            "type": "indexed", "mode": "points", "vertices": points.len(), "triangles": 0,
            "vertex_colors": colors.is_some(), "payload_bytes": 0
        });
        write_frame(&mut self.out, &header.to_string(), &[])?;
        if let Some(colors) = colors {
            for (positions, colors) in points.chunks(RECORDS).zip(colors.chunks(RECORDS)) {
                self.position_batch(positions)?;
                self.batch("colors", bytemuck::cast_slice(colors))?;
            }
        } else {
            self.vertices(points, None)?;
        }
        self.points += points.len();
        self.draws += 1;
        Ok(())
    }

    pub fn mesh_u16(
        &mut self, points: &[[f32; 3]], triangles: &[[u16; 3]], colors: Option<&[[u8; 4]]>,
    ) -> io::Result<()> {
        if colors.is_none() && std::mem::size_of_val(points) + std::mem::size_of_val(triangles) >= triangles.len() * 36 {
            return self.packed_batches(points, triangles);
        }
        let mut header = serde_json::json!({
            "type": "indexed", "vertices": points.len(), "triangles": triangles.len(),
            "index_width": 2, "payload_bytes": 0
        });
        if colors.is_some() {
            header["vertex_colors"] = serde_json::json!(true);
        }
        write_frame(&mut self.out, &header.to_string(), &[])?;
        self.vertices(points, colors)?;
        for batch in triangles.chunks(RECORDS) {
            if cfg!(target_endian = "little") {
                self.batch("indices", bytemuck::cast_slice(batch))?;
            } else {
                self.payload.clear();
                for index in batch.iter().flatten() {
                    self.payload.extend_from_slice(&index.to_le_bytes());
                }
                self.batch_header("indices", self.payload.len())?;
                self.out.write_all(&self.payload)?;
            }
        }
        self.triangles += triangles.len();
        self.draws += 1;
        Ok(())
    }

    fn packed_batches(&mut self, points: &[[f32; 3]], triangles: &[[u16; 3]]) -> io::Result<()> {
        for batch in triangles.chunks(8_192) {
            self.packed_batch(batch.iter().map(|triangle| triangle.map(|index| points[index as usize])))?;
        }
        Ok(())
    }

    pub fn packed_batch(&mut self, triangles: impl ExactSizeIterator<Item = [[f32; 3]; 3]>) -> io::Result<()> {
        let count = triangles.len();
        self.payload.clear();
        for triangle in triangles {
            for coordinate in triangle.iter().flatten() {
                self.payload.extend_from_slice(&coordinate.to_le_bytes());
            }
        }
        write_frame(&mut self.out, &format!(
            r#"{{"type":"part","triangles":{count},"payload_bytes":{}}}"#,
            self.payload.len()
        ), &self.payload)?;
        self.bytes += self.payload.len();
        self.draws += 1;
        self.triangles += count;
        Ok(())
    }

    fn vertices(&mut self, points: &[[f32; 3]], colors: Option<&[[u8; 4]]>) -> io::Result<()> {
        for chunk in points.chunks(RECORDS) {
            self.position_batch(chunk)?;
        }
        if let Some(colors) = colors {
            for chunk in colors.chunks(RECORDS) {
                self.batch("colors", bytemuck::cast_slice(chunk))?;
            }
        }
        Ok(())
    }

    pub fn texture(&mut self, id: usize, texture: &Texture) -> io::Result<()> {
        let header = serde_json::json!({
            "type": "texture", "id": id, "mime": texture.mime, "size": texture.bytes.len(),
            "wrap_s": texture.wrap_s, "wrap_t": texture.wrap_t,
            "min": texture.min, "mag": texture.mag, "payload_bytes": 0
        });
        write_frame(&mut self.out, &header.to_string(), &[])?;
        for chunk in texture.bytes.chunks(RECORDS * 12) {
            self.batch("texture_bytes", chunk)?;
        }
        Ok(())
    }

    fn position_batch(&mut self, values: &[[f32; 3]]) -> io::Result<()> {
        let started = self.profile.then(Instant::now);
        let bounds = position_bounds(values);
        if let Some(started) = started { self.bounds_time += started.elapsed(); }
        self.float_batch("positions", values, bounds)
    }

    fn float_batch<const N: usize>(
        &mut self, kind: &str, values: &[[f32; N]], bounds: Option<([f64; 3], [f64; 3])>,
    ) -> io::Result<()>
    where [f32; N]: bytemuck::Pod {
        let length = std::mem::size_of_val(values);
        if let Some((minimum, maximum)) = bounds {
            self.bytes += length;
            write_frame(&mut self.out, &format!(
                r#"{{"type":"{kind}","payload_bytes":{length},"minimum":{minimum:?},"maximum":{maximum:?}}}"#
            ), &[])?;
        } else {
            self.batch_header(kind, length)?;
        }
        if cfg!(target_endian = "little") {
            return self.out.write_all(bytemuck::cast_slice(values));
        }
        self.payload.clear();
        for coordinate in values.iter().flatten() {
            self.payload.extend_from_slice(&coordinate.to_le_bytes());
        }
        self.out.write_all(&self.payload)
    }

    fn batch(&mut self, kind: &str, payload: &[u8]) -> io::Result<()> {
        self.batch_header(kind, payload.len())?;
        self.out.write_all(payload)
    }

    fn batch_header(&mut self, kind: &str, length: usize) -> io::Result<()> {
        self.bytes += length;
        write_frame(&mut self.out, &format!(
            r#"{{"type":"{kind}","payload_bytes":{length}}}"#
        ), &[])
    }

    pub fn finish(self) -> io::Result<()> {
        self.complete("", 0)
    }

    pub fn finish_with_warning(self, warning: &str) -> io::Result<()> {
        self.complete(warning, 0)
    }

    pub fn finish_with_errors(self, errors: usize) -> io::Result<()> {
        self.complete("", errors)
    }

    fn complete(mut self, warning: &str, errors: usize) -> io::Result<()> {
        let mut header = serde_json::json!({
            "type": "done", "triangles": self.triangles, "total_ms": self.import_ms,
            "payload_bytes": 0, "warning": warning
        });
        if self.points != 0 {
            header["points"] = serde_json::json!(self.points);
        }
        if errors != 0 {
            header["errors"] = serde_json::json!(errors);
        }
        write_frame(&mut self.out, &header.to_string(), &[])?;
        self.out.flush()?;
        if self.profile {
            eprintln!("Indexed output: {} bytes, {} meshes, {:.2} ms writing",
                self.bytes, self.draws, self.start.elapsed().as_secs_f64() * 1000.0);
            eprintln!("Indexed bounds: {:.2} ms", self.bounds_time.as_secs_f64() * 1000.0);
        }
        Ok(())
    }
}
