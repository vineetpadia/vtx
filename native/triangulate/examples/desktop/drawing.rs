//! Optional Python bridge keeps DXF/DWG dependencies outside the desktop core.
use super::Result;
use serde::Deserialize;
use std::{
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
    pub positions: Vec<[f32; 3]>,
    pub kind: String,
    pub color: [u8; 4],
    pub metadata: serde_json::Value,
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
    let python = std::env::var_os("VTX_CAD_PYTHON")
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
        .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "python" } else { "python3" }));
    let mut command = Command::new(&python);
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
    Ok(serde_json::from_slice(&output.stdout)?)
}
