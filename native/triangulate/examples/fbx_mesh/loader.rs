use std::path::Path;

use super::Result;

// ufbx supplies independent tasks for compressed arrays and other bulk work.
// Complete each batch before returning to C: the scene and the borrowed pool
// then cannot outlive their tasks, including on a failed import.
#[cfg(feature = "parallel")]
unsafe extern "C" fn run(
    user: *mut std::ffi::c_void,
    context: ufbx::ThreadPoolContext,
    _group: u32,
    start: u32,
    count: u32,
) {
    use rayon::prelude::*;
    // `load()` keeps this pool alive until ufbx returns. ufbx owns the task
    // context and guarantees that these indices describe independent tasks.
    let pool = &*(user as *const rayon::ThreadPool);
    pool.install(|| {
        (start..start + count).into_par_iter().for_each(|index| {
            ufbx::thread_pool_run_task(context, index);
        });
    });
}

#[cfg(feature = "parallel")]
unsafe extern "C" fn wait(
    _user: *mut std::ffi::c_void,
    _context: ufbx::ThreadPoolContext,
    _group: u32,
    _max_index: u32,
) {
    // `run()` has already joined every task in the batch.
}

fn load_options(ignore_embedded: bool) -> ufbx::LoadOpts<'static> {
    ufbx::LoadOpts {
        ignore_animation: true,
        ignore_embedded,
        load_external_files: false,
        ..Default::default()
    }
}

pub fn load(path: &Path, ignore_embedded: bool) -> Result<ufbx::SceneRoot> {
    let source = path.to_str().ok_or("FBX path is not Unicode")?;
    let options = load_options(ignore_embedded);
    #[cfg(feature = "parallel")]
    let mut options = options;
    #[cfg(feature = "parallel")]
    let pool = {
        let available = std::thread::available_parallelism().map_or(1, usize::from);
        let threads = std::env::var("VTX_FBX_THREADS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(8)
            .min(available)
            .min(8);
        if threads > 1 && path.metadata().is_ok_and(|metadata| metadata.len() >= 4 * 1024 * 1024) {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|index| format!("vtx fbx {}", index))
                .build()
                .ok()
        } else {
            None
        }
    };
    #[cfg(feature = "parallel")]
    if let Some(pool) = pool.as_ref() {
        // The pointer is borrowed only during this synchronous load. The C
        // callbacks never retain tasks after returning, and never own the pool.
        options.thread_opts.pool = ufbx::ThreadPool::Raw(unsafe {
            ufbx::Unsafe::new(ufbx::RawThreadPool {
                run_fn: Some(run),
                wait_fn: Some(wait),
                user: pool as *const _ as *mut std::ffi::c_void,
                ..Default::default()
            })
        });
    }
    let result = ufbx::load_file(source, options);
    #[cfg(feature = "parallel")]
    let result = if pool.is_some() && result.is_err() {
        // A failed threaded task can replace a useful decoder diagnostic with
        // "Failed to load". Serial retry also covers inputs the threaded parser
        // cannot handle, using less temporary memory and the same options.
        drop(pool);
        ufbx::load_file(source, load_options(ignore_embedded))
    } else {
        result
    };
    result
        .map_err(|error| format!("FBX import: {}", error.description).into())
}
