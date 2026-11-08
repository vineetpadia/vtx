//! Network work starts only after first presentation. Updates apply on exit.
pub const VERSION: &str = env!("VTX_VERSION");
pub const REPOSITORY: &str = env!("VTX_UPDATE_REPO");
const INTERVAL: u64 = 6 * 60 * 60;

#[derive(Clone)]
pub struct State {
    pub version: String,
    pub message: String,
    pub busy: bool,
    pub ready: Option<String>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            version: VERSION.into(),
            message: "Automatic updates are checked after the first view appears.".into(),
            busy: false,
            ready: None,
        }
    }
}
pub enum Notice {
    Status(State),
    #[cfg(windows)]
    Ready(Box<Pending>),
}
pub struct Controller {
    pub state: State,
    pub started: bool,
    next_check: Option<std::time::Instant>,
    #[cfg(windows)]
    pending: Option<Pending>,
}
impl Default for Controller {
    fn default() -> Self {
        Self {
            state: State::default(),
            started: false,
            next_check: None,
            #[cfg(windows)]
            pending: None,
        }
    }
}
impl Controller {
    pub fn automatic_due(&self) -> bool {
        cfg!(windows)
            && !self.state.busy
            && self.state.ready.is_none()
            && (!self.started
                || self
                    .next_check
                    .is_some_and(|t| t <= std::time::Instant::now()))
    }
    pub fn deadline(&self) -> Option<std::time::Instant> {
        if cfg!(windows) && !self.state.busy && self.state.ready.is_none() {
            self.next_check
        } else {
            None
        }
    }
    pub fn check(&mut self, manual: bool, notify: impl Fn(Notice) + Send + 'static) {
        if self.state.busy {
            return;
        }
        self.started = true;
        self.next_check =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(INTERVAL));
        self.state.busy = true;
        self.state.message = "Checking for updates…".into();
        let result = std::thread::Builder::new()
            .name("VTX updates".into())
            .spawn(move || {
                #[cfg(windows)]
                {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        windows::check(manual, &notify)
                    }));
                    match result {
                        Ok(Ok(notice)) => notify(notice),
                        Ok(Err(error)) => {
                            windows::log_error(&error);
                            notify(Notice::Status(State {
                                message: error,
                                ..State::default()
                            }));
                        }
                        Err(_) => notify(Notice::Status(State {
                            message: "Could not check for updates. Try again later.".into(),
                            ..State::default()
                        })),
                    }
                }
                #[cfg(not(windows))]
                {
                    let _ = manual;
                    notify(Notice::Status(State {
                        message:
                            "Automatic updates are available in the Windows installer edition."
                                .into(),
                        ..State::default()
                    }));
                }
            });
        if let Err(error) = result {
            self.state.busy = false;
            self.state.message = format!("Could not start update check: {error}");
        }
    }
    pub fn receive(&mut self, notice: Notice) {
        match notice {
            Notice::Status(state) => self.state = state,
            #[cfg(windows)]
            Notice::Ready(pending) => {
                self.state = State {
                    version: pending.manager.get_current_version_as_string(),
                    message: format!(
                        "VTX {} is downloaded and will install when the last VTX window closes.",
                        pending.asset.Version
                    ),
                    busy: false,
                    ready: Some(pending.asset.Version.clone()),
                };
                self.pending = Some(*pending);
            }
        }
    }
    pub fn apply_on_exit(
        &mut self,
        restart: bool,
        args: Vec<std::ffi::OsString>,
    ) -> Result<(), String> {
        #[cfg(windows)]
        {
            // Another window may have completed the download, or a previous
            // session may have exited before its Ready event was delivered.
            let pending = self.pending.take().or_else(windows::cached);
            if let Some(pending) = pending {
                if windows::another_instance() {
                    self.pending = Some(pending);
                    return if restart {
                        Err("Close other VTX windows before restarting to update.".into())
                    } else {
                        Ok(())
                    };
                }
                pending
                    .manager
                    .wait_exit_then_apply_updates(&pending.asset, true, restart, args)
                    .map_err(|e| format!("Could not apply update: {e}"))?;
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (restart, args);
        }
        Ok(())
    }
}
fn due(last: Option<u64>, now: u64) -> bool {
    last.and_then(|last| now.checked_sub(last))
        .is_none_or(|age| age >= INTERVAL)
}
pub fn startup() {
    #[cfg(windows)]
    {
        if std::env::current_exe()
            .ok()
            .and_then(|p| {
                p.parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n == "rollback")
            })
            .unwrap_or(false)
        {
            return;
        }
        velopack::VelopackApp::build()
            .set_auto_apply_on_startup(false)
            .on_after_install_fast_callback(|_| {
                if let Err(e) = windows::registration(false) {
                    windows::log_error(&e);
                }
            })
            .on_after_update_fast_callback(|_| {
                if let Err(e) = windows::registration(false) {
                    windows::log_error(&e);
                }
            })
            .on_before_update_fast_callback(|_| {
                if let Err(e) = windows::backup() {
                    windows::log_error(&e);
                }
            })
            .on_before_uninstall_fast_callback(|_| {
                if let Err(e) = windows::registration(true) {
                    windows::log_error(&e);
                }
            })
            .run();
    }
}
#[cfg(windows)]
pub struct Pending {
    manager: velopack::UpdateManager,
    asset: velopack::VelopackAsset,
}

/// Explicit diagnostic only: installed test edition + local feed; never used
/// by an opened model or by the automatic network checker.
pub fn smoke(feed: &std::path::Path, apply: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        let manager = if feed == std::path::Path::new("--github") {
            windows::remote_manager()?
        } else {
            velopack::UpdateManager::new(velopack::sources::FileSource::new(feed), None, None)
                .map_err(|e| e.to_string())?
        };
        println!(
            "installed_version={}",
            manager.get_current_version_as_string()
        );
        match manager.check_for_updates().map_err(|e| e.to_string())? {
            velopack::UpdateCheck::UpdateAvailable(info) => {
                manager
                    .download_updates(&info, None)
                    .map_err(|e| e.to_string())?;
                println!("verified_download={}", info.TargetFullRelease.Version);
                if apply {
                    let mut controller = Controller::default();
                    controller.pending = Some(Pending {
                        manager,
                        asset: info.TargetFullRelease,
                    });
                    controller.apply_on_exit(false, Vec::new())?;
                    println!("apply_on_exit=true");
                }
            }
            _ => println!("up_to_date=true"),
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (feed, apply);
        Err("Windows installer test only".into())
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{
        fs,
        io::Write,
        os::windows::process::CommandExt,
        path::{Path, PathBuf},
        process::{Command, Stdio},
        time::{SystemTime, UNIX_EPOCH},
    };
    use velopack::{sources, UpdateCheck, UpdateManager};
    fn layout() -> Option<(PathBuf, PathBuf)> {
        let exe = std::env::current_exe().ok()?;
        let current = exe.parent()?;
        if current.file_name()? != "current" || !current.join("sq.version").is_file() {
            return None;
        }
        let root = current.parent()?.to_path_buf();
        root.join("Update.exe").is_file().then_some((exe, root))
    }
    fn manager() -> Result<UpdateManager, String> {
        UpdateManager::new(sources::NoneSource {}, None, None).map_err(|_| {
            "Install the updater-enabled VTX edition to receive automatic updates.".into()
        })
    }
    pub fn cached() -> Option<Pending> {
        if layout().is_none() {
            return None;
        }
        let manager = manager().ok()?;
        let asset = manager.get_update_pending_restart()?;
        Some(Pending { manager, asset })
    }
    pub fn log_error(error: &str) {
        if let Some((_, root)) = layout() {
            if let Ok(mut f) = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(root.join("updates.log"))
            {
                let _ = writeln!(f, "{}", error);
            }
        }
    }
    struct Lock(PathBuf);
    impl Drop for Lock {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    fn token() -> Result<Option<String>, String> {
        if env!("VTX_UPDATE_PRIVATE") != "true" {
            return Ok(None);
        }
        // Use the user's existing keyring login. No token is embedded in a
        // build, manifest, configuration file, log or process command line.
        let gh =
            PathBuf::from(std::env::var_os("ProgramFiles").ok_or("GitHub login is unavailable.")?)
                .join("GitHub CLI/gh.exe");
        let output=Command::new(gh).args(["auth","token","--hostname","github.com"]).env("GH_PROMPT_DISABLED","1").env("GH_NO_UPDATE_NOTIFIER","1").stderr(Stdio::null()).creation_flags(0x08000000).output().map_err(|_|"Private updates require GitHub CLI and a GitHub login. Run gh auth login once, then check again.".to_string())?;
        if !output.status.success() {
            return Err(
                "Sign in with GitHub CLI (gh auth login) to receive private VTX updates.".into(),
            );
        }
        let token = String::from_utf8(output.stdout)
            .map_err(|_| "Could not read GitHub login.")?
            .trim()
            .to_owned();
        if token.is_empty() {
            return Err("GitHub login is unavailable.".into());
        }
        Ok(Some(token))
    }
    pub fn check(manual: bool, notify: &impl Fn(Notice)) -> Result<Notice, String> {
        let (_, root) = layout()
            .ok_or("Install the updater-enabled VTX edition to receive automatic updates.")?;
        let local = manager()?;
        let version = local.get_current_version_as_string();
        if let Some(asset) = local.get_update_pending_restart() {
            return Ok(Notice::Ready(Box::new(Pending {
                manager: local,
                asset,
            })));
        }
        let last_path = root.join("last-update-check.json");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let last = fs::read(&last_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v["attempt"].as_u64());
        if !manual && !due(last, now) {
            return Ok(Notice::Status(State{version,message:"Updates are checked automatically every six hours. You can check now at any time.".into(),..State::default()}));
        }
        let lock_path = root.join("update-check.lock");
        // A crashed download leaves a lock. Only reclaim an old lock; the
        // framework separately locks package downloads and verifies hashes.
        if let Ok(meta) = fs::metadata(&lock_path) {
            if meta
                .modified()
                .ok()
                .and_then(|t| SystemTime::now().duration_since(t).ok())
                .is_some_and(|age| age.as_secs() > 3600)
            {
                let _ = fs::remove_file(&lock_path);
            }
        }
        let _file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Ok(Notice::Status(State {
                    version,
                    message: "Another VTX window is checking for updates.".into(),
                    ..State::default()
                }))
            }
            Err(e) => return Err(e.to_string()),
        };
        let _lock = Lock(lock_path);
        fs::write(&last_path, format!("{{\"attempt\":{now}}}")).map_err(|e| e.to_string())?;
        let manager = remote_manager()?;
        match manager
            .check_for_updates()
            .map_err(|e| format!("Could not reach VTX updates: {e}"))?
        {
            UpdateCheck::UpdateAvailable(info) => {
                notify(Notice::Status(State {
                    version,
                    message: format!(
                        "Downloading VTX {} in the background…",
                        info.TargetFullRelease.Version
                    ),
                    busy: true,
                    ready: None,
                }));
                manager.download_updates(&info, None).map_err(|e| {
                    format!("Update download failed verification or could not finish: {e}")
                })?;
                Ok(Notice::Ready(Box::new(Pending {
                    manager,
                    asset: info.TargetFullRelease,
                })))
            }
            _ => Ok(Notice::Status(State {
                version,
                message: "VTX is up to date.".into(),
                ..State::default()
            })),
        }
    }
    pub fn backup() -> Result<(), String> {
        let (exe, root) = layout().ok_or("No installed VTX layout")?;
        let rollback = root.join("rollback");
        fs::create_dir_all(&rollback).map_err(|e| e.to_string())?;
        let temp = rollback.join("VTX.exe.tmp");
        fs::copy(&exe, &temp).map_err(|e| e.to_string())?;
        if rollback.join("VTX.exe").exists() {
            fs::remove_file(rollback.join("VTX.exe")).map_err(|e| e.to_string())?;
        }
        fs::rename(temp, rollback.join("VTX.exe")).map_err(|e| e.to_string())?;
        fs::copy(
            exe.parent().unwrap().join("sq.version"),
            rollback.join("previous-sq.version"),
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn remote_manager() -> Result<UpdateManager, String> {
        let source = sources::GithubSource::new(
            &format!("https://github.com/{REPOSITORY}"),
            token()?,
            false,
        );
        UpdateManager::new(source, None, None).map_err(|e| e.to_string())
    }
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Raw {
        kind: u32,
        bytes: Vec<u8>,
    }
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Entry {
        path: String,
        name: String,
        before: Option<Raw>,
        installed: String,
    }
    fn entries(exe: &Path) -> Vec<Entry> {
        let viewer = exe.to_string_lossy();
        let command = format!("\"{viewer}\" \"%1\"");
        let icon = format!("\"{viewer}\",0");
        let app = "Software\\Classes\\Applications\\VTX.exe";
        let model = "Software\\Classes\\VTX.Model";
        let capabilities = "Software\\VTX\\Capabilities";
        let mut values = vec![
            (
                app.to_owned(),
                "FriendlyAppName".to_owned(),
                "VTX".to_owned(),
            ),
            (format!("{app}\\DefaultIcon"), "".into(), icon.clone()),
            (
                format!("{app}\\shell\\open\\command"),
                "".into(),
                command.clone(),
            ),
            (model.into(), "".into(), "VTX model or drawing".into()),
            (format!("{model}\\DefaultIcon"), "".into(), icon.clone()),
            (format!("{model}\\shell\\open\\command"), "".into(), command),
            (
                format!("{model}\\Application"),
                "ApplicationName".into(),
                "VTX".into(),
            ),
            (
                format!("{model}\\Application"),
                "ApplicationIcon".into(),
                icon.clone(),
            ),
            (capabilities.into(), "ApplicationName".into(), "VTX".into()),
            (capabilities.into(), "ApplicationIcon".into(), icon),
            (
                "Software\\RegisteredApplications".into(),
                "VTX".into(),
                capabilities.into(),
            ),
        ];
        for ext in [
            ".sldprt", ".sldasm", ".step", ".stp", ".p21", ".stl", ".obj", ".ply", ".glb", ".gltf",
            ".fbx", ".3mf", ".ifc", ".ifczip", ".dxf",
        ] {
            values.push((format!("{app}\\SupportedTypes"), ext.into(), "".into()));
            values.push((
                format!("Software\\Classes\\{ext}\\OpenWithProgids"),
                "VTX.Model".into(),
                "".into(),
            ));
            values.push((
                format!("{capabilities}\\FileAssociations"),
                ext.into(),
                "VTX.Model".into(),
            ));
        }
        values
            .into_iter()
            .map(|(path, name, installed)| Entry {
                path,
                name,
                installed,
                before: None,
            })
            .collect()
    }
    pub fn registration(uninstall: bool) -> Result<(), String> {
        use winreg::{enums::*, RegKey};
        let (exe, root) = match layout() {
            Some(p) => p,
            None => return Ok(()),
        };
        // Isolated updater test editions must never alter the user's handlers.
        if root.file_name().is_none_or(|name| name != "VTXDesktop") {
            return Ok(());
        }
        let hive = RegKey::predef(HKEY_CURRENT_USER);
        let path = root.join("registration-backup.json");
        let mut saved: Vec<Entry> = if path.exists() {
            serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?
        } else {
            Vec::new()
        };
        if uninstall {
            for entry in saved {
                if let Ok(key) = hive.open_subkey_with_flags(&entry.path, KEY_READ | KEY_WRITE) {
                    if key.get_value::<String, _>(&entry.name).ok().as_deref()
                        != Some(entry.installed.as_str())
                    {
                        continue;
                    }
                    if let Some(raw) = entry.before {
                        let kind = match raw.kind {
                            0 => REG_NONE,
                            1 => REG_SZ,
                            2 => REG_EXPAND_SZ,
                            3 => REG_BINARY,
                            4 => REG_DWORD,
                            7 => REG_MULTI_SZ,
                            11 => REG_QWORD,
                            _ => continue,
                        };
                        key.set_raw_value(
                            &entry.name,
                            &winreg::RegValue {
                                bytes: raw.bytes,
                                vtype: kind,
                            },
                        )
                        .map_err(|e| e.to_string())?;
                    } else {
                        let _ = key.delete_value(&entry.name);
                    }
                }
            }
        } else {
            for entry in entries(&exe) {
                if let Some(old) = saved
                    .iter_mut()
                    .find(|old| old.path == entry.path && old.name == entry.name)
                {
                    old.installed = entry.installed;
                } else {
                    let before = hive
                        .open_subkey(&entry.path)
                        .ok()
                        .and_then(|key| key.get_raw_value(&entry.name).ok())
                        .map(|v| Raw {
                            kind: v.vtype as u32,
                            bytes: v.bytes,
                        });
                    saved.push(Entry { before, ..entry });
                }
            }
            fs::write(
                &path,
                serde_json::to_vec_pretty(&saved).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            for entry in &saved {
                let (key, _) = hive.create_subkey(&entry.path).map_err(|e| e.to_string())?;
                key.set_value(&entry.name, &entry.installed)
                    .map_err(|e| e.to_string())?;
            }
        }
        #[link(name = "shell32")]
        extern "system" {
            fn SHChangeNotify(
                event: u32,
                flags: u32,
                a: *const std::ffi::c_void,
                b: *const std::ffi::c_void,
            );
        }
        unsafe {
            SHChangeNotify(0x08000000, 0, std::ptr::null(), std::ptr::null());
        }
        Ok(())
    }
    // Only another process running this exact main executable delays install.
    // The separately installed Explorer worker is never considered or closed.
    pub fn another_instance() -> bool {
        use std::ffi::c_void;
        #[repr(C)]
        struct Entry {
            size: u32,
            usage: u32,
            pid: u32,
            heap: usize,
            module: u32,
            threads: u32,
            parent: u32,
            priority: i32,
            flags: u32,
            exe: [u16; 260],
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
            fn Process32FirstW(snapshot: *mut c_void, entry: *mut Entry) -> i32;
            fn Process32NextW(snapshot: *mut c_void, entry: *mut Entry) -> i32;
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
            fn QueryFullProcessImageNameW(
                process: *mut c_void,
                flags: u32,
                name: *mut u16,
                size: *mut u32,
            ) -> i32;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }
        let me = match std::env::current_exe() {
            Ok(p) => p.to_string_lossy().to_lowercase(),
            Err(_) => return true,
        };
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(2, 0);
            if snapshot as isize == -1 {
                return true;
            }
            let mut entry: Entry = std::mem::zeroed();
            entry.size = std::mem::size_of::<Entry>() as u32;
            let mut ok = Process32FirstW(snapshot, &mut entry);
            let mut found = false;
            while ok != 0 {
                if entry.pid != std::process::id() {
                    let handle = OpenProcess(0x1000, 0, entry.pid);
                    if !handle.is_null() {
                        let mut name = [0u16; 32768];
                        let mut count = name.len() as u32;
                        if QueryFullProcessImageNameW(handle, 0, name.as_mut_ptr(), &mut count) != 0
                            && String::from_utf16_lossy(&name[..count as usize]).to_lowercase()
                                == me
                        {
                            found = true;
                        }
                        CloseHandle(handle);
                    }
                }
                if found {
                    break;
                }
                ok = Process32NextW(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
            found
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn registration_never_sets_extension_defaults_or_preview_handlers() {
            let values = entries(Path::new("C:\\VTXDesktop\\current\\VTX.exe"));
            assert!(values.iter().all(|e| !e.path.contains("UserChoice")
                && !e.path.contains("shellex")
                && !e.path.contains("PreviewHandlers")));
            for e in values
                .iter()
                .filter(|e| e.path.starts_with("Software\\Classes\\."))
            {
                assert!(e.path.ends_with("\\OpenWithProgids"));
            }
            assert!(values
                .iter()
                .any(|e| e.installed == "\"C:\\VTXDesktop\\current\\VTX.exe\" \"%1\""));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn check_throttles_without_blocking_manual_requests() {
        assert!(due(None, 100));
        assert!(!due(Some(100), 101));
        assert!(due(Some(100), 100 + INTERVAL));
        assert!(due(Some(200), 100));
    }
    #[test]
    fn initial_state_does_not_start_network_work() {
        let c = Controller::default();
        assert!(!c.started);
        assert!(!c.state.busy);
        assert!(c.state.ready.is_none());
    }
    #[test]
    fn periodic_check_sleeps_until_due_and_stops_when_busy_or_ready() {
        let mut c = Controller::default();
        c.started = true;
        c.next_check = Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
        assert!(!c.automatic_due());
        c.next_check = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
        assert_eq!(c.automatic_due(), cfg!(windows));
        c.state.busy = true;
        assert!(!c.automatic_due());
        assert!(c.deadline().is_none());
        c.state.busy = false;
        c.state.ready = Some("0.2.1".into());
        assert!(!c.automatic_due());
        assert!(c.deadline().is_none());
    }
}
