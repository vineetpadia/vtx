//! Standalone, lazy CAD conversion with a shared on-disk mesh cache.
use std::{
    collections::hash_map::DefaultHasher,
    convert::TryInto,
    fs,
    hash::{Hash, Hasher},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex, OnceLock, Weak},
    time::{Duration, Instant, UNIX_EPOCH},
};
use serde::{Deserialize, Serialize};
use super::Result;

const LIMIT: u64 = 512 * 1024 * 1024;
static JOBS: OnceLock<Mutex<Vec<Weak<Mutex<Child>>>>> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(0);

pub fn cancel_all() {
    if let Some(jobs) = JOBS.get() {
        if let Ok(mut jobs) = jobs.lock() {
            for job in jobs.drain(..).filter_map(|j| j.upgrade()) {
                if let Ok(mut child) = job.lock() { let _ = child.kill(); let _ = child.wait(); }
            }
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct Identity { source: PathBuf, size: u64, modified: u128, engine: String }
#[derive(Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema: u32,
    pub success: bool,
    pub complete: bool,
    pub meters_per_unit: Option<f64>,
    #[serde(default)] pub notice: String,
    #[serde(default)] pub error: String,
}
#[derive(Serialize, Deserialize)]
struct Record { identity: Identity, report: Report }
pub struct Converted { pub path: PathBuf, pub report: Report, pub cached: bool }

fn stamp(path: &Path) -> Result<(u64, u128)> {
    let m = fs::metadata(path)?;
    Ok((m.len(), m.modified()?.duration_since(UNIX_EPOCH)?.as_nanos()))
}
fn cache_root() -> Result<PathBuf> {
    #[cfg(windows)]
    let root = PathBuf::from(std::env::var_os("USERPROFILE").ok_or("No user cache directory")?)
        .join("AppData/LocalLow/VTX/CAD");
    #[cfg(target_os = "macos")]
    let root = PathBuf::from(std::env::var_os("HOME").ok_or("No user cache directory")?)
        .join("Library/Caches/VTX/CAD");
    #[cfg(all(unix, not(target_os = "macos")))]
    let root = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(||
        std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cache")))
        .ok_or("No user cache directory")?.join("vtx/cad");
    fs::create_dir_all(&root)?;
    Ok(root)
}
fn helper() -> Result<(Command, PathBuf)> {
    // Development override is explicit. Production never searches the model
    // directory, current directory, PATH, or a global Python installation.
    if let Some(python) = std::env::var_os("VTX_CAD_PYTHON") {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cad-importer/convert.py").canonicalize()?;
        let mut command = Command::new(python); command.arg("-I").arg(&script);
        return Ok((command, script));
    }
    let executable = std::env::current_exe()?;
    let parent = executable.parent().ok_or("No application directory")?;
    let name = if cfg!(windows) { "vtx-cad-import.exe" } else { "vtx-cad-import" };
    let paths = [parent.join("cad_import").join(name), parent.join("../Resources/cad_import").join(name)];
    let path = paths.iter().find(|p| p.is_file()).ok_or(
        "The standalone CAD importer is missing. Install a VTX package that includes CAD import support.")?;
    Ok((Command::new(&path), path.to_path_buf()))
}
fn read_small(path: &Path) -> Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new(); file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 { return Err("CAD report exceeds its size limit".into()); }
    Ok(bytes)
}
fn valid_report(report: &Report) -> bool {
    report.schema == 1 && report.success && report.complete &&
        report.meters_per_unit.is_none_or(|u| u.is_finite() && u > 0.)
}
fn valid_glb(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else { return false; };
    if meta.len() < 28 || meta.len() > LIMIT { return false; }
    let Ok(mut f) = fs::File::open(path) else { return false; };
    let mut header = [0; 12];
    f.read_exact(&mut header).is_ok() && &header[..4] == b"glTF" &&
        u32::from_le_bytes(header[4..8].try_into().unwrap()) == 2 &&
        u64::from(u32::from_le_bytes(header[8..12].try_into().unwrap())) == meta.len()
}
fn cached(identity: &Identity, mesh: &Path, record: &Path) -> Option<Converted> {
    let record: Record = serde_json::from_slice(&read_small(record).ok()?).ok()?;
    (record.identity == *identity && valid_report(&record.report) && valid_glb(mesh)).then(||
        Converted { path: mesh.into(), report: record.report, cached: true })
}
struct Remove(Vec<PathBuf>);
impl Drop for Remove { fn drop(&mut self) { for p in &self.0 { let _ = fs::remove_file(p); } } }
struct Kill(Arc<Mutex<Child>>);
impl Drop for Kill { fn drop(&mut self) { if let Ok(mut c) = self.0.lock() { let _=c.kill(); let _=c.wait(); } } }

pub fn load(source: &Path) -> Result<Converted> {
    let (mut command, engine) = helper()?;
    let source = source.canonicalize()?;
    let (size, modified) = stamp(&source)?;
    if size > LIMIT { return Err("CAD input exceeds 512 MiB".into()); }
    let (engine_size, engine_modified) = stamp(&engine)?;
    let identity = Identity { source: source.clone(), size, modified,
        engine: format!("v2:{}:{engine_size}:{engine_modified}", env!("VTX_VERSION")) };
    let mut hash = DefaultHasher::new();
    identity.source.hash(&mut hash); identity.size.hash(&mut hash);
    identity.modified.hash(&mut hash); identity.engine.hash(&mut hash);
    let key = format!("{:016x}", hash.finish());
    let root = cache_root()?;
    let mesh = root.join(format!("{key}.glb"));
    let record = root.join(format!("{key}.json"));
    if let Some(result) = cached(&identity, &mesh, &record) { return Ok(result); }
    let lock = root.join(format!("{key}.lock"));
    let lock_file = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&lock)?;
    let started = Instant::now();
    loop {
        match lock_file.try_lock() {
            Ok(_) => break,
            Err(fs::TryLockError::WouldBlock) => {
                if let Some(result) = cached(&identity, &mesh, &record) { return Ok(result); }
                if started.elapsed() > Duration::from_secs(125) { return Err("Another CAD import did not finish in time".into()); }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
    // The OS releases this lock on exit or crash. Keep the empty lock file so
    // waiters cannot accidentally lock different inodes after a rename/delete.
    // Another process may have completed between the initial check and lock.
    if let Some(result) = cached(&identity, &mesh, &record) { return Ok(result); }
    let suffix = format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let output = root.join(format!("{key}-{suffix}.tmp.glb"));
    let report_path = root.join(format!("{key}-{suffix}.tmp.json"));
    let record_tmp = root.join(format!("{key}-{suffix}.record.json"));
    let _temp = Remove(vec![output.clone(), report_path.clone(), record_tmp.clone()]);
    command.arg(&source).arg(&output).arg("--report").arg(&report_path).arg("--parent-pipe")
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
    let child = Arc::new(Mutex::new(command.spawn()?));
    let _kill = Kill(child.clone());
    {
        let mut jobs = JOBS.get_or_init(Default::default).lock().unwrap();
        jobs.retain(|j| j.strong_count() != 0); jobs.push(Arc::downgrade(&child));
    }
    let deadline = Instant::now();
    let status = loop {
        if let Some(status) = child.lock().unwrap().try_wait()? { break status; }
        if deadline.elapsed() > Duration::from_secs(120) { return Err("CAD conversion exceeded two minutes".into()); }
        std::thread::sleep(Duration::from_millis(25));
    };
    let report: Report = serde_json::from_slice(&read_small(&report_path).map_err(|_|
        "The standalone CAD importer exited without a conversion report")?)?;
    if !status.success() || !valid_report(&report) {
        return Err(format!("Standalone CAD import failed: {}", if report.error.is_empty() {
            format!("converter exited with {status}")
        } else { report.error }).into());
    }
    if !valid_glb(&output) { return Err("The CAD importer produced an invalid or oversized mesh".into()); }
    if stamp(&source)? != (size, modified) { return Err("The source file changed during CAD conversion; reopen it".into()); }
    fs::write(&record_tmp, serde_json::to_vec(&Record { identity, report: report.clone() })?)?;
    // Replacing a stale cache entry never affects the source CAD file.
    if mesh.exists() { fs::remove_file(&mesh)?; }
    if record.exists() { fs::remove_file(&record)?; }
    fs::rename(&output, &mesh)?; fs::rename(&record_tmp, &record)?;
    Ok(Converted { path: mesh, report, cached: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn incomplete_or_unvalidated_conversions_are_never_cached() {
        let mut r=Report { schema:1, success:true, complete:true, meters_per_unit:None, notice:String::new(), error:String::new() };
        assert!(valid_report(&r));r.complete=false;assert!(!valid_report(&r));
        r.complete=true;r.meters_per_unit=Some(f64::NAN);assert!(!valid_report(&r));
    }
}
