use std::{
    env,
    ffi::{c_char, c_int, c_void, CString},
    path::Path,
    slice,
    sync::OnceLock,
    time::Instant,
};

use libloading::Library;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const MAX_RECORDS: usize = 16_000_000;
const DEFLECTION: f64 = 0.25;

#[derive(Default)]
#[repr(C)]
struct NativeMesh {
    positions: *const [f32; 3],
    indices: *const [u32; 3],
    vertices: usize,
    triangles: usize,
    skipped_faces: usize,
    read_ms: f64,
    transfer_ms: f64,
    mesh_ms: f64,
    extraction_ms: f64,
    storage: *mut c_void,
}

type Load = unsafe extern "C" fn(*const c_char, f64, *mut NativeMesh, *mut c_char, usize) -> c_int;
type Free = unsafe extern "C" fn(*mut NativeMesh);

struct Backend {
    _library: Library,
    load: Load,
    free: Free,
}

fn backend() -> Result<&'static Backend> {
    static BACKEND: OnceLock<Backend> = OnceLock::new();
    if let Some(backend) = BACKEND.get() {
        return Ok(backend);
    }
    let exe = env::current_exe()?;
    let name = format!(
        "{}vtx_iges{}",
        env::consts::DLL_PREFIX,
        env::consts::DLL_SUFFIX
    );
    let path = exe
        .parent()
        .ok_or("VTX executable has no parent directory")?
        .join(&name);
    if !path.is_file() {
        return Err(format!(
            "IGES requires {name} beside the VTX executable. Build it from native/iges_bridge."
        )
        .into());
    }
    // Load only the versioned native extension beside the trusted executable.
    #[cfg(windows)]
    let library: Library = unsafe {
        use libloading::os::windows::{
            Library, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        };
        Library::load_with_flags(
            &path,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        )?
    }
    .into();
    #[cfg(not(windows))]
    let library = unsafe { Library::new(&path)? };
    // The two symbols implement the C ABI declared in native/iges_bridge/iges_bridge.h.
    let load = unsafe { *library.get::<Load>(b"vtx_iges_load_v1\0")? };
    let free = unsafe { *library.get::<Free>(b"vtx_iges_free_v1\0")? };
    Ok(BACKEND.get_or_init(|| Backend {
        _library: library,
        load,
        free,
    }))
}

pub struct Mesh {
    native: NativeMesh,
    free: Free,
}

impl Mesh {
    pub fn positions(&self) -> &[[f32; 3]] {
        // Successful bridge results own non-empty, aligned arrays until drop.
        unsafe { slice::from_raw_parts(self.native.positions, self.native.vertices) }
    }

    pub fn triangles(&self) -> &[[u32; 3]] {
        unsafe { slice::from_raw_parts(self.native.indices, self.native.triangles) }
    }

    pub fn skipped_faces(&self) -> usize {
        self.native.skipped_faces
    }
}

impl Drop for Mesh {
    fn drop(&mut self) {
        unsafe { (self.free)(&mut self.native) }
    }
}

pub fn load(path: &Path) -> Result<Mesh> {
    let start = Instant::now();
    let deflection = match env::var("VTX_IGES_DEFLECTION") {
        Ok(value) => value.parse::<f64>()?,
        Err(env::VarError::NotPresent) => DEFLECTION,
        Err(error) => return Err(error.into()),
    };
    if !deflection.is_finite() || deflection <= 0.0 {
        return Err("VTX_IGES_DEFLECTION must be finite and positive".into());
    }
    let path = CString::new(path.as_os_str().as_encoded_bytes())?;
    let backend = backend()?;
    let library_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut mesh = Mesh {
        native: NativeMesh::default(),
        free: backend.free,
    };
    let mut error = [0u8; 512];
    // The bridge copies error bytes into this buffer and writes the owning mesh.
    let success = unsafe {
        (backend.load)(
            path.as_ptr(),
            deflection,
            &mut mesh.native,
            error.as_mut_ptr().cast(),
            error.len(),
        )
    };
    if success == 0 {
        let length = error
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(error.len());
        return Err(format!("IGES import: {}", String::from_utf8_lossy(&error[..length])).into());
    }
    let native = &mesh.native;
    if native.positions.is_null()
        || native.indices.is_null()
        || !(1..=MAX_RECORDS).contains(&native.vertices)
        || !(1..=MAX_RECORDS).contains(&native.triangles)
    {
        return Err("Invalid IGES mesh returned by the native extension".into());
    }
    if env::var_os("STEPFLASH_PROFILE").is_some() {
        eprintln!(
            "IGES native phases: library={library_ms:.2} ms, read={:.2} ms, transfer={:.2} ms, meshing={:.2} ms, extraction={:.2} ms; vertices={}, triangles={}, skipped={}",
            native.read_ms, native.transfer_ms, native.mesh_ms, native.extraction_ms,
            native.vertices, native.triangles, native.skipped_faces
        );
    }
    Ok(mesh)
}
