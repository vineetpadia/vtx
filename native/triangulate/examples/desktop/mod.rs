mod drawing;
mod drawing_cache;
mod dxf_fast;
mod inspect;
mod measure;
mod model;
mod render;
mod units;

use std::{
    error::Error,
    fmt,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Instant,
};

use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::ModifiersState,
    window::{Window, WindowId},
};

use self::{
    model::{Scene, UploadTarget},
    render::{Camera, Renderer},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const EMPTY_TITLE: &str = if cfg!(target_os = "macos") {
    "VTX  ·  Drop a 3D file or press ⌘O"
} else {
    "VTX  ·  Drop a 3D file or press Ctrl+O"
};

struct Loaded {
    name: String,
    started: Instant,
    result: std::result::Result<Scene, ImportError>,
}

// Import errors are formatted on the worker so legacy decoders can keep their
// local error types and both the CLI and the window preserve their diagnostics.
struct ImportError {
    message: String,
    detail: String,
}

impl From<Box<dyn Error>> for ImportError {
    fn from(error: Box<dyn Error>) -> Self {
        Self {
            message: error.to_string(),
            detail: format!("{error:?}"),
        }
    }
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl fmt::Debug for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

impl Error for ImportError {}

fn model_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn spawn_import<T: Send + 'static>(
    path: PathBuf,
    uploader: UploadTarget,
    complete: impl FnOnce(Loaded) -> T + Send + 'static,
) -> std::io::Result<thread::JoinHandle<T>> {
    let started = Instant::now();
    let name = model_name(&path);
    thread::Builder::new()
        .name("vtx import".into())
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| model::load(&path, &uploader)))
                .unwrap_or_else(|_| Err("Model import failed".into()))
                .map_err(ImportError::from);
            complete(Loaded {
                name,
                started,
                result,
            })
        })
}

struct Viewer {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    depth: wgpu::TextureView,
    scene: Option<Scene>,
    camera: Camera,
    cursor: Option<PhysicalPosition<f64>>,
    dragging: bool,
    modifiers: ModifiersState,
    first_frame: Option<Instant>,
}

impl Viewer {
    fn new(event_loop: &ActiveEventLoop) -> Result<Self> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(EMPTY_TITLE)
                    .with_inner_size(LogicalSize::new(1100, 760)),
            )?,
        );
        let size = window.inner_size();
        let (surface, config, renderer) = render::initialize(|instance| {
            let surface = instance.create_surface(window.clone())?;
            let adapter = render::adapter(instance, Some(&surface))?;
            let mut config = surface
                .get_default_config(&adapter, size.width.max(1), size.height.max(1))
                .ok_or("The GPU has no compatible window format")?;
            if let Some(format) = surface
                .get_capabilities(&adapter)
                .formats
                .into_iter()
                .find(wgpu::TextureFormat::is_srgb)
            {
                config.format = format;
            }
            let renderer = Renderer::new(&adapter, config.format)?;
            Ok((surface, config, renderer))
        })?;
        surface.configure(&renderer.uploader.device, &config);
        let depth = renderer.depth(config.width, config.height);
        window.request_redraw();
        Ok(Self {
            window,
            surface,
            config,
            renderer,
            depth,
            scene: None,
            camera: Camera::default(),
            cursor: None,
            dragging: false,
            modifiers: ModifiersState::empty(),
            first_frame: None,
        })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width > 0 && size.height > 0 {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface
                .configure(&self.renderer.uploader.device, &self.config);
            self.depth = self.renderer.depth(size.width, size.height);
            self.window.request_redraw();
        }
    }

    fn redraw(&mut self) -> std::result::Result<(), wgpu::SurfaceError> {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let frame = self.surface.get_current_texture()?;
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self
            .renderer
            .uploader
            .device
            .create_command_encoder(&Default::default());
        self.renderer.draw(
            &mut encoder,
            &view,
            &self.depth,
            self.scene.as_ref(),
            &self.camera,
            self.config.width as f32 / self.config.height as f32,
        );
        self.renderer.uploader.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        frame.present();
        if let Some(started) = self.first_frame.take() {
            if std::env::var_os("STEPFLASH_PROFILE").is_some() {
                eprintln!(
                    "vtx_first_present_ms={:.3}",
                    started.elapsed().as_secs_f64() * 1000.0
                );
            }
        }
        Ok(())
    }

    fn show_error(&self, name: &str, error: &str) {
        eprintln!("VTX: {}: {}", name, error);
        let message = error.lines().next().unwrap_or("Unable to open model");
        self.window
            .set_title(&format!("VTX  ·  {}  ·  {}", name, message));
    }
}

struct App {
    proxy: EventLoopProxy<Loaded>,
    viewer: Option<Viewer>,
    startup: Option<PathBuf>,
    loading: bool,
    fatal: Option<Box<dyn Error>>,
}

impl App {
    fn start_load(&mut self, path: PathBuf, uploader: UploadTarget) -> std::io::Result<()> {
        let proxy = self.proxy.clone();
        spawn_import(path, uploader, move |loaded| {
            let _ = proxy.send_event(loaded);
        })?;
        self.loading = true;
        Ok(())
    }

    fn open(&mut self, path: PathBuf) {
        if self.loading {
            return;
        }
        let Some(viewer) = self.viewer.as_ref() else {
            return;
        };
        let name = model_name(&path);
        viewer
            .window
            .set_title(&format!("VTX  ·  Opening {}…", name));
        let uploader = UploadTarget::ready(viewer.renderer.uploader.clone());
        if let Err(error) = self.start_load(path, uploader) {
            if let Some(viewer) = &self.viewer {
                viewer.show_error(&name, &error.to_string());
            }
        }
    }
}

impl ApplicationHandler<Loaded> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.viewer.is_none() {
            let startup = self.startup.take().map(|path| {
                let name = model_name(&path);
                let (sender, uploader) = UploadTarget::deferred();
                (name, self.start_load(path, uploader).map(|()| sender))
            });
            let viewer = match Viewer::new(event_loop) {
                Ok(viewer) => viewer,
                Err(error) => {
                    self.fatal = Some(error);
                    event_loop.exit();
                    return;
                }
            };
            if let Some((name, result)) = startup {
                match result {
                    Ok(sender) => {
                        viewer
                            .window
                            .set_title(&format!("VTX  ·  Opening {}…", name));
                        let _ = sender.send(viewer.renderer.uploader.clone());
                    }
                    Err(error) => viewer.show_error(&name, &error.to_string()),
                }
            }
            self.viewer = Some(viewer);
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, loaded: Loaded) {
        self.loading = false;
        let Some(viewer) = self.viewer.as_mut() else {
            return;
        };
        match loaded.result {
            Ok(scene) => {
                let mut title = format!("VTX  ·  {}", loaded.name);
                if scene.triangles > 0 {
                    title.push_str(&format!("  ·  {} triangles", scene.triangles));
                }
                if scene.line_segments > 0 {
                    title.push_str(&format!("  ·  {} line segments", scene.line_segments));
                }
                if scene.points > 0 {
                    title.push_str(&format!("  ·  {} points", scene.points));
                }
                if let Some(notice) = scene.notice {
                    title.push_str("  ·  ");
                    title.push_str(notice);
                }
                if scene.issues > 0 {
                    title.push_str(&format!("  ·  {} import warnings", scene.issues));
                }
                viewer.window.set_title(&title);
                viewer.camera = if scene.source_info["kind"] == "drawing_2d" {
                    inspect::plan_camera()
                } else {
                    Camera {
                        z_up: scene.source_info["kind"] == "drawing_3d",
                        ..Camera::default()
                    }
                };
                viewer.scene = Some(scene);
                viewer.first_frame = Some(loaded.started);
                viewer.window.request_redraw();
            }
            Err(error) => viewer.show_error(&loaded.name, &error.message),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(viewer) = self.viewer.as_mut() else {
            return;
        };
        if viewer.window.id() != id {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::DroppedFile(path) => self.open(path),
            WindowEvent::Resized(size) => viewer.resize(size),
            WindowEvent::RedrawRequested => match viewer.redraw() {
                Ok(()) => (),
                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                    viewer.resize(viewer.window.inner_size());
                }
                Err(wgpu::SurfaceError::Timeout) => viewer.window.request_redraw(),
                Err(error) => {
                    self.fatal = Some(error.into());
                    event_loop.exit();
                }
            },
            WindowEvent::ModifiersChanged(modifiers) => viewer.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, .. }
                if event.state.is_pressed() && !event.repeat =>
            {
                if let Some(key) = event.logical_key.to_text() {
                    if key.eq_ignore_ascii_case("o")
                        && !self.loading
                        && (viewer.modifiers.control_key() || viewer.modifiers.super_key())
                    {
                        let path = rfd::FileDialog::new()
                            .add_filter("Models and drawings", model::FORMATS)
                            .set_parent(&*viewer.window)
                            .pick_file();
                        if let Some(path) = path {
                            self.open(path);
                        }
                    } else if key.eq_ignore_ascii_case("r") {
                        viewer.camera = if viewer
                            .scene
                            .as_ref()
                            .is_some_and(|s| s.source_info["kind"] == "drawing_2d")
                        {
                            inspect::plan_camera()
                        } else {
                            Camera {
                                z_up: viewer
                                    .scene
                                    .as_ref()
                                    .is_some_and(|s| s.source_info["kind"] == "drawing_3d"),
                                ..Camera::default()
                            }
                        };
                        viewer.window.request_redraw();
                    }
                }
            }
            WindowEvent::Focused(false) => {
                viewer.dragging = false;
                viewer.modifiers = ModifiersState::empty();
            }
            WindowEvent::CursorLeft { .. } => viewer.cursor = None,
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                viewer.dragging = state == ElementState::Pressed;
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(previous) = viewer.cursor.replace(position).filter(|_| viewer.dragging)
                {
                    viewer.camera.yaw += (position.x - previous.x) as f32 * 0.008;
                    viewer.camera.pitch = (viewer.camera.pitch
                        + (position.y - previous.y) as f32 * 0.008)
                        .clamp(-1.55, 1.55);
                    viewer.window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(position) => position.y as f32 / 40.0,
                };
                viewer.camera.zoom = (viewer.camera.zoom * (lines * 0.12).exp()).clamp(0.05, 30.0);
                viewer.window.request_redraw();
            }
            _ => (),
        }
    }
}

pub fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let startup = args.next();
    if startup.as_deref().is_some_and(|arg| arg == "--check") {
        let path = args
            .next()
            .ok_or("Usage: vtx --check MODEL [SCREENSHOT.png]")?;
        let screenshot = args.next();
        return render::check(Path::new(&path), screenshot.as_deref().map(Path::new));
    }
    if startup.as_deref().is_some_and(|arg| arg == "--inspect") {
        return inspect::run(args.collect());
    }
    if startup
        .as_deref()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        println!("VTX: vtx [MODEL] | --check MODEL [PNG]\n{}", inspect::USAGE);
        return Ok(());
    }
    let event_loop = EventLoop::<Loaded>::with_user_event().build()?;
    let mut app = App {
        proxy: event_loop.create_proxy(),
        viewer: None,
        startup: startup.map(PathBuf::from),
        loading: false,
        fatal: None,
    };
    event_loop.run_app(&mut app)?;
    app.fatal.map_or(Ok(()), Err)
}
