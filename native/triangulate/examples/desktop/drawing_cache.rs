//! Content-verified geometry cache. Unusable entries are always a cache miss.
use super::{
    drawing::{self, Drawing},
    Result,
};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Instant, SystemTime},
};

const MAX_ENTRY: u64 = 256 * 1024 * 1024;
const BUDGET: u64 = 512 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"VTXDC001";

fn executable(path: PathBuf) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path);
    }
    env::split_paths(&env::var_os("PATH")?)
        .map(|p| p.join(&path))
        .find(|p| p.is_file())
}

fn hash_path(hash: &mut Sha256, path: &Path) {
    let bytes = path.as_os_str().as_encoded_bytes();
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}
fn stamp(hash: &mut Sha256, path: &Path) -> Option<()> {
    hash_path(hash, path);
    if !path.exists() {
        hash.update(b"absent");
        return Some(());
    }
    let meta = fs::metadata(path).ok()?;
    hash.update(meta.len().to_le_bytes());
    hash.update(
        meta.modified()
            .ok()?
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_nanos()
            .to_le_bytes(),
    );
    Some(())
}

fn tree(hash: &mut Sha256, path: &Path) -> Option<()> {
    tree_inner(hash, path, &mut std::collections::HashSet::new())
}
fn tree_inner(
    hash: &mut Sha256,
    path: &Path,
    seen: &mut std::collections::HashSet<PathBuf>,
) -> Option<()> {
    if !path.exists() {
        return stamp(hash, path);
    }
    let canonical = fs::canonicalize(path).ok()?;
    hash_path(hash, &canonical);
    if !seen.insert(canonical) {
        return Some(());
    }
    let mut entries = fs::read_dir(path)
        .ok()?
        .collect::<std::io::Result<Vec<_>>>()
        .ok()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        // Compiled bytecode is an output, not an importer dependency.
        if entry.file_name() == "__pycache__" {
            continue;
        }
        let kind = entry.file_type().ok()?;
        if kind.is_dir() {
            tree_inner(hash, &path, seen)?;
        } else if kind.is_file() || kind.is_symlink() {
            if path.is_dir() {
                tree_inner(hash, &path, seen)?;
            } else {
                stamp(hash, &path)?;
            }
        }
    }
    Some(())
}

fn key(path: &Path) -> Option<(PathBuf, [u8; 32])> {
    if env::var_os("VTX_CAD_CACHE").is_some_and(|v| v == "0")
        || env::var_os("VTX_CAD_FORCE_FALLBACK").is_some()
        || env::var_os("PYTHONPATH").is_some_and(|v| !v.is_empty())
        || env::var_os("PYTHONHOME").is_some()
    {
        return None;
    }
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let root = env::var_os("VTX_CAD_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .or_else(|| env::var_os("LOCALAPPDATA").map(PathBuf::from))
                .or_else(|| home.as_ref().map(|p| p.join(".cache")))
                .map(|p| p.join("vtx/drawings"))
        })?;
    let python = executable(drawing::python())?;
    let prefix = python.parent()?.parent()?;
    // Unknown global/custom Python environments remain uncached; their dependency
    // and custom font search paths cannot be fingerprinted reliably here.
    if !prefix.join("pyvenv.cfg").is_file() {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(b"VTX drawing geometry cache schema 1");
    hash.update(include_bytes!("drawing.rs"));
    hash.update(include_bytes!("dxf_fast.rs"));
    hash.update(include_bytes!("drawing_cache.rs"));
    hash.update(include_bytes!("../../../../scripts/cad_bridge.py"));
    stamp(&mut hash, &python)?;
    stamp(&mut hash, &fs::canonicalize(&python).ok()?)?;
    stamp(&mut hash, &prefix.join("pyvenv.cfg"))?;
    tree(&mut hash, &prefix.join("lib"))?;
    tree(&mut hash, &prefix.join("Lib"))?;
    for variable in [
        "EZDXF_CONFIG_FILE",
        "EZDXF_FONTS",
        "EZDXF_DISABLE_C_EXT",
        "XDG_CONFIG_HOME",
    ] {
        if env::var_os(variable).is_some() {
            return None;
        }
    }
    if Path::new("ezdxf.ini").exists() {
        return None;
    }
    let font_cache = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|p| p.join(".cache")))?;
    stamp(&mut hash, &font_cache.join("ezdxf/font_manager_cache.json"))?;
    if let Some(home) = &home {
        if home.join(".config/ezdxf/ezdxf.ini").exists() {
            return None;
        }
        for directory in [
            ".fonts",
            ".local/share/fonts",
            ".local/share/texmf/fonts",
            "Library/Fonts",
        ] {
            tree(&mut hash, &home.join(directory))?;
        }
    }
    for directory in [
        "/usr/share/fonts",
        "/usr/local/share/fonts",
        "/Library/Fonts",
        "/System/Library/Fonts",
    ] {
        tree(&mut hash, Path::new(directory))?;
    }
    if let Some(windows) = env::var_os("SystemRoot") {
        tree(&mut hash, &PathBuf::from(windows).join("Fonts"))?;
    }
    if path
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("dwg"))
    {
        stamp(&mut hash, &executable(drawing::converter())?)?;
    }
    hash_path(&mut hash, &fs::canonicalize(path).ok()?);
    let mut source = fs::File::open(path).ok()?;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = source.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let digest: [u8; 32] = hash.finalize().into();
    let name: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    Some((root.join(format!("{name}.vtxcad")), digest))
}

fn read(path: &Path, key: &[u8; 32]) -> Option<Drawing> {
    if fs::metadata(path).ok()?.len() > MAX_ENTRY {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    if bytes.get(..8)? != MAGIC || bytes.get(8..40)? != key {
        return None;
    }
    let payload = bytes.get(72..)?;
    if bytes.get(40..72)? != Sha256::digest(payload).as_slice() {
        return None;
    }
    let drawing = drawing::decode(payload).ok()?;
    let _ = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(SystemTime::now()));
    Some(drawing)
}

#[derive(Serialize)]
struct Part<'a> {
    vertex_count: usize,
    kind: drawing::Primitive,
    color: [u8; 4],
    metadata: &'a drawing::EntityMetadata,
}
fn write(path: &Path, key: &[u8; 32], drawing: &Drawing) -> Option<()> {
    let mut position_bytes = 0_usize;
    for part in &drawing.parts {
        position_bytes = position_bytes.checked_add(part.positions.len().checked_mul(12)?)?;
        if position_bytes as u64 > MAX_ENTRY {
            return None;
        }
    }
    let parts: Vec<_> = drawing
        .parts
        .iter()
        .map(|p| Part {
            vertex_count: p.positions.len(),
            kind: p.kind,
            color: p.color,
            metadata: &p.metadata,
        })
        .collect();
    let header =
        serde_json::to_vec(&json!({"parts":parts,"source_info":drawing.source_info})).ok()?;
    if (header.len().checked_add(position_bytes)?.checked_add(88)?) as u64 > MAX_ENTRY {
        return None;
    }
    let mut payload = b"VTXCAD01".to_vec();
    payload.extend_from_slice(&(header.len() as u64).to_le_bytes());
    payload.extend(header);
    for part in &drawing.parts {
        for point in &part.positions {
            for coordinate in point {
                payload.extend_from_slice(&coordinate.to_le_bytes());
            }
        }
        if payload.len() as u64 + 72 > MAX_ENTRY {
            return None;
        }
    }
    fs::create_dir_all(path.parent()?).ok()?;
    let temporary = path.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_nanos()
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(MAGIC)?;
        file.write_all(key)?;
        file.write_all(&Sha256::digest(&payload))?;
        file.write_all(&payload)?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
        return None;
    }
    // Bound total disk usage; concurrent readers/writers can safely miss entries.
    let mut entries: Vec<_> = fs::read_dir(path.parent()?)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension()? != "vtxcad" {
                return None;
            }
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), p))
        })
        .collect();
    entries.sort_by_key(|e| e.0);
    let mut total: u64 = entries.iter().map(|e| e.1).sum();
    for (_, size, entry) in entries {
        if total <= BUDGET {
            break;
        }
        if fs::remove_file(entry).is_ok() {
            total -= size;
        }
    }
    Some(())
}

pub fn load(path: &Path) -> Result<Drawing> {
    let start = Instant::now();
    // Simple ASCII linework is cheaper to parse than dependency/cache lookup.
    if path
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("dxf"))
    {
        if let Some(mut drawing) = super::dxf_fast::load(path) {
            drawing.source_info["cache"] = json!({"status":"bypassed_native"});
            drawing.source_info["load_ms"] = json!(start.elapsed().as_secs_f64() * 1000.0);
            return Ok(drawing);
        }
    }
    let cache_key = if fs::metadata(path).map_or(false, |m| m.len() < 64 * 1024) {
        None
    } else {
        key(path)
    };
    let lookup_ms = start.elapsed().as_secs_f64() * 1000.0;
    if let Some((file, digest)) = &cache_key {
        if let Some(mut drawing) = read(file, digest) {
            let original = drawing.source_info["load_ms"].clone();
            drawing.source_info["cache"] =
                json!({"status":"hit","lookup_ms":lookup_ms,"original_import_ms":original});
            drawing.source_info["load_ms"] = json!(start.elapsed().as_secs_f64() * 1000.0);
            return Ok(drawing);
        }
    }
    let mut drawing = drawing::uncached(path)?;
    // Never associate old geometry with a source/dependency changed during import.
    let stored = cache_key.as_ref().is_some_and(|(_, before)| {
        key(path).is_some_and(|(file, after)| {
            before == &after && write(&file, &after, &drawing).is_some()
        })
    });
    drawing.source_info["cache"] = json!({"status":if cache_key.is_some() {"miss"} else {"disabled"},"stored":stored,"lookup_ms":lookup_ms});
    drawing.source_info["load_ms"] = json!(start.elapsed().as_secs_f64() * 1000.0);
    Ok(drawing)
}
