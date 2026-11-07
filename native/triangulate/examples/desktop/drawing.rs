//! Optional Python bridge keeps DXF/DWG dependencies outside the desktop core.
use super::Result;
use serde::{ser::SerializeStruct, Deserialize, Serialize, Serializer};
use std::sync::Arc;
use std::{
    convert::TryInto,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Deserialize)]
pub struct Drawing {
    pub parts: Vec<DrawPart>,
    pub source_info: serde_json::Value,
}
#[derive(Deserialize)]
pub struct DrawPart {
    #[serde(default)]
    pub positions: Vec<[f32; 3]>,
    pub kind: Primitive,
    pub color: [u8; 4],
    pub metadata: EntityMetadata,
}
#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Primitive {
    Lines,
    Points,
    Triangles,
}
fn symbol<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Arc<str>, D::Error> {
    Ok(Arc::from(String::deserialize(deserializer)?))
}
#[derive(Deserialize)]
pub struct EntityMetadata {
    pub handle: String,
    #[serde(deserialize_with = "symbol")]
    pub entity_type: Arc<str>,
    #[serde(deserialize_with = "symbol")]
    pub layer: Arc<str>,
    #[serde(default)]
    pub block_path: Vec<String>,
    pub text: Option<String>,
}
impl Serialize for EntityMetadata {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("EntityMetadata", 5)?;
        out.serialize_field("handle", &self.handle)?;
        out.serialize_field("entity_type", self.entity_type.as_ref())?;
        out.serialize_field("layer", self.layer.as_ref())?;
        out.serialize_field(
            "block_path",
            if self.block_path.is_empty() {
                std::slice::from_ref(&self.handle)
            } else {
                &self.block_path
            },
        )?;
        out.serialize_field("text", &self.text)?;
        out.end()
    }
}
fn nearby(relative: &str) -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    for parent in executable.ancestors().skip(1).take(6) {
        let file = parent.join(relative);
        if file.is_file() {
            return Some(file);
        }
    }
    None
}
pub fn load(path: &Path) -> Result<Drawing> {
    super::drawing_cache::load(path)
}
pub(super) fn uncached(path: &Path) -> Result<Drawing> {
    let start = std::time::Instant::now();
    let mut drawing = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("dwg"))
        && std::env::var_os("VTX_CAD_FORCE_FALLBACK").is_none()
    {
        let converted = convert_dwg(path)?;
        let mut drawing = if let Some(drawing) = super::dxf_fast::load(&converted.path) {
            drawing
        } else {
            bridge(&converted.path).map_err(|e| {
                format!(
                    "{}\nDWG conversion diagnostics:\n{}",
                    e, converted.diagnostics
                )
            })?
        };
        drawing.source_info["format"] = serde_json::json!("dwg");
        drawing.source_info["conversion"] = serde_json::json!({"tool":"LibreDWG dwg2dxf","diagnostics":converted.diagnostics,"conversion_ms":converted.elapsed_ms});
        let diagnostics = converted.diagnostics.to_ascii_lowercase();
        if diagnostics.contains("warning") || diagnostics.contains("error") {
            if let Some(warnings) = drawing.source_info["warnings"].as_array_mut() {
                warnings.push(serde_json::json!({"reason":"DWG converter reported warnings or errors; inspect conversion diagnostics"}));
            }
        }
        drawing
    } else if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("dxf"))
    {
        super::dxf_fast::load(path)
            .map(Ok)
            .unwrap_or_else(|| bridge(path))?
    } else {
        bridge(path)?
    };
    drawing.source_info["load_ms"] = serde_json::json!(start.elapsed().as_secs_f64() * 1000.0);
    if drawing.source_info["reader"].is_null() {
        drawing.source_info["reader"] = serde_json::json!("ezdxf_fallback");
    }
    Ok(drawing)
}
pub(super) fn converter() -> PathBuf {
    std::env::var_os("VTX_DWG2DXF")
        .map(PathBuf::from)
        .or_else(|| {
            nearby(if cfg!(windows) {
                "cad-tools/bin/dwg2dxf.exe"
            } else {
                "cad-tools/bin/dwg2dxf"
            })
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|p| PathBuf::from(p).join(".local/share/vtx/cad-tools/bin/dwg2dxf"))
                .filter(|p| p.is_file())
        })
        .unwrap_or_else(|| PathBuf::from("dwg2dxf"))
}
struct Converted {
    path: PathBuf,
    diagnostics: String,
    elapsed_ms: f64,
}
impl Drop for Converted {
    fn drop(&mut self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
fn convert_dwg(path: &Path) -> Result<Converted> {
    use std::{
        fs,
        io::{Read, Seek, SeekFrom},
        process::Stdio,
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let start = Instant::now();
    let directory = std::env::temp_dir().join(format!(
        "vtx-dwg-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&directory)?;
    let mut converted = Converted {
        path: directory.join("drawing.dxf"),
        diagnostics: String::new(),
        elapsed_ms: 0.0,
    };
    let log = fs::File::create(directory.join("converter.log"))?;
    let mut child=Command::new(converter()).arg("-o").arg(&converted.path).arg(path)
        .stdout(Stdio::from(log.try_clone()?)).stderr(Stdio::from(log)).spawn()
        .map_err(|e|format!("DWG conversion requires LibreDWG dwg2dxf; set VTX_DWG2DXF or install it on PATH: {}",e))?;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("DWG conversion exceeded 120-second timeout".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let mut log = fs::File::open(directory.join("converter.log"))?;
    let length = log.metadata()?.len();
    log.seek(SeekFrom::Start(length.saturating_sub(8192)))?;
    let mut bytes = Vec::new();
    log.read_to_end(&mut bytes)?;
    converted.diagnostics = String::from_utf8_lossy(&bytes).into_owned();
    if !status.success() || !converted.path.is_file() {
        return Err(format!(
            "DWG conversion failed ({}): {}",
            status, converted.diagnostics
        )
        .into());
    }
    converted.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    Ok(converted)
}
pub(super) fn python() -> PathBuf {
    std::env::var_os("VTX_CAD_PYTHON")
        .map(PathBuf::from)
        .or_else(|| {
            nearby(if cfg!(windows) {
                "cad-venv/Scripts/python.exe"
            } else {
                "cad-venv/bin/python"
            })
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|p| PathBuf::from(p).join(".local/share/vtx/cad-venv/bin/python"))
                .filter(|p| p.is_file())
        })
        .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "python" } else { "python3" }))
}
fn bridge(path: &Path) -> Result<Drawing> {
    let python = python();
    let mut command = Command::new(&python);
    command.env("VTX_CAD_BINARY", "1");
    command
        .arg("-c")
        .arg(include_str!("../../../../scripts/cad_bridge.py"))
        .arg(path);
    if std::env::var_os("VTX_DWG2DXF").is_none() {
        if let Some(converter) = nearby(if cfg!(windows) {
            "cad-tools/bin/dwg2dxf.exe"
        } else {
            "cad-tools/bin/dwg2dxf"
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|p| PathBuf::from(p).join(".local/share/vtx/cad-tools/bin/dwg2dxf"))
                .filter(|p| p.is_file())
        }) {
            command.env("VTX_DWG2DXF", converter);
        }
    }
    let output = command.output().map_err(|e| {
        format!(
            "Cannot start CAD Python {:?}: {}. Install CAD support or set VTX_CAD_PYTHON.",
            python, e
        )
    })?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    decode(&output.stdout)
}

#[derive(Deserialize)]
struct WirePart {
    vertex_count: usize,
    #[serde(flatten)]
    part: DrawPart,
}
#[derive(Deserialize)]
struct WireDrawing {
    parts: Vec<WirePart>,
    source_info: serde_json::Value,
}
pub(super) fn decode(bytes: &[u8]) -> Result<Drawing> {
    if !bytes.starts_with(b"VTXCAD01") {
        return Ok(serde_json::from_slice(bytes)?);
    }
    let length =
        u64::from_le_bytes(bytes.get(8..16).ok_or("Truncated CAD header")?.try_into()?) as usize;
    if length > 256 * 1024 * 1024 {
        return Err("CAD metadata exceeds 256 MiB".into());
    }
    let header = bytes.get(16..16 + length).ok_or("Truncated CAD metadata")?;
    let drawing: WireDrawing = serde_json::from_slice(header)?;
    let mut payload = &bytes[16 + length..];
    let mut parts = Vec::with_capacity(drawing.parts.len());
    for wire in drawing.parts {
        let count = wire
            .vertex_count
            .checked_mul(12)
            .ok_or("CAD vertex count overflow")?;
        let vertices = payload.get(..count).ok_or("Truncated CAD positions")?;
        payload = &payload[count..];
        let mut part = wire.part;
        part.positions = vertices
            .chunks_exact(12)
            .map(|v| {
                [
                    f32::from_le_bytes(v[..4].try_into().unwrap()),
                    f32::from_le_bytes(v[4..8].try_into().unwrap()),
                    f32::from_le_bytes(v[8..12].try_into().unwrap()),
                ]
            })
            .collect();
        parts.push(part);
    }
    if !payload.is_empty() {
        return Err("Unexpected trailing CAD data".into());
    }
    Ok(Drawing {
        parts,
        source_info: drawing.source_info,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_positions_and_metadata_are_validated() {
        let header = serde_json::json!({"parts":[{"kind":"lines","color":[255,0,0,255],"vertex_count":2,
            "metadata":{"handle":"A1","entity_type":"LINE","layer":"0","block_path":["A1"],"text":null}}],"source_info":{}});
        let header = serde_json::to_vec(&header).unwrap();
        let mut wire = b"VTXCAD01".to_vec();
        wire.extend_from_slice(&(header.len() as u64).to_le_bytes());
        wire.extend(header);
        for v in [1_f32, 2., 0., 3., 4., 0.] {
            wire.extend_from_slice(&v.to_le_bytes());
        }
        let drawing = decode(&wire).unwrap();
        assert_eq!(drawing.parts[0].positions, vec![[1., 2., 0.], [3., 4., 0.]]);
        assert_eq!(drawing.parts[0].metadata.handle, "A1");
        assert!(decode(&wire[..wire.len() - 1]).is_err());
        wire.push(0);
        assert!(decode(&wire).is_err());
    }
}
