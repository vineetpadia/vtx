mod camera;
mod cad_import;
mod edges;
#[cfg(windows)]
mod embedded;
mod features;
mod model;
mod navigation;
mod picking;
mod render;
mod saved_image;
mod ui;
mod ui_gpu;
mod updates;

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
enum Event {
    Loaded(Loaded),
    Update(updates::Notice),
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
    dragging: Option<MouseButton>,
    modifiers: ModifiersState,
    navigation: navigation::Settings,
    middle_clicks: navigation::Clicks,
    first_frame: Option<Instant>,
    ui: Option<ui::Ui>,
    ui_enabled: bool,
    ui_repaint: Option<Instant>,
    name: String,
    scene_dirty: bool,
    picker: Option<picking::Picker>,
    pending_pick: Option<picking::Pending>,
    pick_context: Option<ui::PickContext>,
    feature_job: Option<features::Job>,
    screen_edges: Option<edges::ScreenEdges>,
    hover_due: Option<Instant>,
    hover_recheck: bool,
    hover_at: Option<[u32; 2]>,
    hover_queue: Option<edges::Candidate>,
    pending_hover: Option<edges::Candidate>,
    click_candidate: Option<edges::Candidate>,
    pick_additive: bool,
    pick_at: Option<[u32; 2]>,
    measure_click: Option<PhysicalPosition<f64>>,
    #[cfg(windows)]
    embedded: Option<embedded::EmbeddedPreview>,
    #[cfg(windows)]
    preview_child: Option<usize>,
    #[cfg(windows)]
    snapshot: Option<render::Snapshot>,
}

impl Viewer {
    fn new(
        event_loop: &ActiveEventLoop,
        preview_parent: Option<usize>,
        cache: Option<PathBuf>,
    ) -> Result<Self> {
        let icon =
            image::load_from_memory(include_bytes!("../../../../assets/vtx-64.png"))?.into_rgba8();
        let icon = winit::window::Icon::from_rgba(icon.into_raw(), 64, 64)?;
        let attributes = Window::default_attributes()
            .with_title(EMPTY_TITLE)
            .with_window_icon(Some(icon))
            .with_inner_size(LogicalSize::new(1100, 760));
        #[cfg(windows)]
        let embedded = preview_parent
            .map(|parent| embedded::EmbeddedPreview::new(parent, cache))
            .transpose()?;
        #[cfg(windows)]
        let attributes = if let Some(preview) = &embedded {
            preview.attributes(attributes)?
        } else {
            attributes
        };
        #[cfg(not(windows))]
        let _ = (preview_parent, cache);
        let window = Arc::new(event_loop.create_window(attributes)?);
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
            dragging: None,
            modifiers: ModifiersState::empty(),
            navigation: navigation::Settings::load(),
            middle_clicks: navigation::Clicks::default(),
            first_frame: None,
            ui: None,
            ui_enabled: preview_parent.is_none(),
            ui_repaint: None,
            name: "Open a model".into(),
            scene_dirty: true,
            picker: None,
            pending_pick: None,
            pick_context: None,
            feature_job: None,
            screen_edges: None,
            hover_due: None,
            hover_recheck: false,
            hover_at: None,
            hover_queue: None,
            pending_hover: None,
            click_candidate: None,
            pick_additive: false,
            pick_at: None,
            measure_click: None,
            #[cfg(windows)]
            embedded,
            #[cfg(windows)]
            preview_child: None,
            #[cfg(windows)]
            snapshot: None,
        })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width > 0 && size.height > 0 {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface
                .configure(&self.renderer.uploader.device, &self.config);
            self.depth = self.renderer.depth(size.width, size.height);
            self.scene_dirty = true;
            self.window.request_redraw();
        }
    }
    fn queue_hover(&mut self) {
        if self.ui.is_none() || self.scene.is_none() || self.dragging.is_some()
            || self.scene.as_ref().is_some_and(|s| s.raster_size.is_some()) {
            return;
        }
        if let Some(p) = self.cursor {
            let ui = self.ui.as_mut().unwrap();
            if ui.ctx.is_pointer_over_area() {
                self.hover_due = None;
                self.hover_at = None;
                ui.tools.hovered = None;
                return;
            }
            if ui.tools.measure && !matches!(ui.tools.mode, ui::Mode::Edge | ui::Mode::Diameter) {
                return;
            }
            ui.tools.edge_discovery = true;
            self.hover_at = Some([
                p.x.clamp(0., self.config.width.saturating_sub(1) as f64) as u32,
                p.y.clamp(0., self.config.height.saturating_sub(1) as f64) as u32,
            ]);
            self.hover_due
                .get_or_insert_with(|| Instant::now() + std::time::Duration::from_millis(32));
        }
    }

    fn redraw(&mut self) -> std::result::Result<(), wgpu::SurfaceError> {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let frame = self.surface.get_current_texture()?;
        let view = frame.texture.create_view(&Default::default());
        if self.screen_edges.as_ref().is_some_and(|cache| {
            !cache.matches(&self.camera, [self.config.width, self.config.height])
        }) {
            if let Some(ui) = &mut self.ui {
                ui.tools.hovered = None;
            }
        }
        let ui_frame = if let Some(ui) = &mut self.ui {
            Some(ui.begin(
                Some(&self.window),
                self.scene.as_ref(),
                &mut self.camera,
                [self.config.width, self.config.height],
                &self.name,
            ))
        } else {
            None
        };
        if ui_frame.as_ref().is_some_and(|frame| frame.changed) {
            self.scene_dirty = true;
        }
        let mut encoder = self
            .renderer
            .uploader
            .device
            .create_command_encoder(&Default::default());
        if self.scene_dirty || !self.renderer.present_cached(&mut encoder, &view) {
            self.renderer.draw(
                &mut encoder,
                &view,
                &self.depth,
                self.scene.as_ref(),
                &self.camera,
                self.config.width as f32 / self.config.height as f32,
                [self.config.width, self.config.height],
            );
            self.scene_dirty = false;
        }
        if let (Some(ui), Some(ui_frame)) = (&mut self.ui, &ui_frame) {
            ui.paint(
                &self.renderer.uploader.device,
                &self.renderer.uploader.queue,
                &mut encoder,
                &view,
                [self.config.width, self.config.height],
                ui_frame,
            );
        }
        self.renderer.uploader.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        frame.present();
        if let (Some(ui), Some(frame)) = (&mut self.ui, ui_frame) {
            let delay = frame
                .output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .map(|v| v.repaint_delay)
                .unwrap_or(std::time::Duration::MAX);
            self.ui_repaint =
                (delay < std::time::Duration::from_secs(2)).then(|| Instant::now() + delay);
            ui.finish(&self.window, frame);
        }
        // egui's hit regions reflect the current pointer only after this
        // frame. Recheck a move once, without scheduling an idle hover loop.
        if std::mem::take(&mut self.hover_recheck) {
            self.queue_hover();
        }
        if let (Some(at), Some(scene)) = (self.pick_at.take(), &self.scene) {
            self.pick_context = Some(ui::PickContext {
                camera: self.camera.clone(),
                size: [self.config.width, self.config.height],
                at,
                serial: self.ui.as_ref().map_or(0, |ui| ui.tools.serial),
                additive: self.pick_additive,
            });
            self.click_candidate = None;
            if let Some(index) = self
                .ui
                .as_ref()
                .and_then(|ui| ui.tools.index.as_ref())
                .filter(|_| {
                    self.ui
                        .as_ref()
                        .is_some_and(|ui| ui.tools.mode == ui::Mode::Edge)
                })
            {
                let size = [self.config.width, self.config.height];
                if self
                    .screen_edges
                    .as_ref()
                    .is_none_or(|cache| !cache.matches(&self.camera, size))
                {
                    self.screen_edges =
                        Some(edges::ScreenEdges::new(index, &self.camera, scene, size));
                }
                self.click_candidate = self.screen_edges.as_ref().unwrap().nearest(
                    index,
                    at,
                    10. * self.window.scale_factor(),
                    false,
                    scene,
                    &self.camera,
                );
            }
            if self.picker.is_none() {
                self.picker = Some(picking::Picker::new(&self.renderer));
            }
            self.pending_pick = Some(self.picker.as_ref().unwrap().begin(
                &self.renderer,
                scene,
                [self.config.width, self.config.height],
                self.click_candidate.as_ref().map_or(at, |c| c.at),
            ));
            self.pending_hover = None;
        } else if self.pending_pick.is_none() {
            if let (Some(candidate), Some(scene)) = (self.hover_queue.take(), &self.scene) {
                let size = [self.config.width, self.config.height];
                if candidate.view == edges::key(&self.camera, size) {
                    if self.picker.is_none() {
                        self.picker = Some(picking::Picker::new(&self.renderer));
                    }
                    self.pending_pick = Some(self.picker.as_ref().unwrap().begin(
                        &self.renderer,
                        scene,
                        size,
                        candidate.at,
                    ));
                    self.pending_hover = Some(candidate);
                    self.pick_context = None;
                }
            }
        }
        #[cfg(windows)]
        if self.embedded.is_some() {
            embedded::EmbeddedPreview::presented_zoom(&self.window, self.camera.zoom);
        }
        if let Some(started) = self.first_frame.take() {
            if std::env::var_os("STEPFLASH_PROFILE").is_some() {
                eprintln!(
                    "vtx_first_present_ms={:.3}",
                    started.elapsed().as_secs_f64() * 1000.0
                );
            }
            #[cfg(windows)]
            if let Some(preview) = &self.embedded {
                self.preview_child = preview
                    .ready(
                        &self.window,
                        self.scene.as_ref().is_some_and(|s| s.retain_saved_preview),
                    )
                    .ok();
                if let (Some(path), Some(scene)) = (&preview.cache, &self.scene) {
                    if !scene.retain_saved_preview {
                        match self.renderer.snapshot(scene, path.clone()) {
                            Ok(snapshot) => self.snapshot = Some(snapshot),
                            Err(error) => eprintln!("VTX: snapshot unavailable: {error}"),
                        }
                    }
                }
            }
        }
        if self.ui_enabled && self.ui.is_none() {
            self.ui = Some(ui::Ui::new(
                &self.window,
                &self.renderer.uploader.device,
                self.config.format,
            ));
            self.window.request_redraw();
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
    proxy: EventLoopProxy<Event>,
    viewer: Option<Viewer>,
    startup: Option<PathBuf>,
    loading: bool,
    fatal: Option<Box<dyn Error>>,
    preview_parent: Option<usize>,
    preview_cache: Option<PathBuf>,
    updater: updates::Controller,
    current_file: Option<PathBuf>,
}

impl App {
    fn start_load(&mut self, path: PathBuf, uploader: UploadTarget) -> std::io::Result<()> {
        cad_import::cancel_all();
        self.current_file = Some(path.clone());
        let proxy = self.proxy.clone();
        spawn_import(path, uploader, move |loaded| {
            let _ = proxy.send_event(Event::Loaded(loaded));
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

impl ApplicationHandler<Event> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.viewer.is_none() {
            let startup = self.startup.take().map(|path| {
                let name = model_name(&path);
                let (sender, uploader) = UploadTarget::deferred();
                (name, self.start_load(path, uploader).map(|()| sender))
            });
            let viewer =
                match Viewer::new(event_loop, self.preview_parent, self.preview_cache.clone()) {
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

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        let loaded = match event {
            Event::Loaded(loaded) => loaded,
            Event::Update(notice) => {
                self.updater.receive(notice);
                if let Some(viewer) = &mut self.viewer {
                    if let Some(ui) = &mut viewer.ui {
                        ui.update_state = self.updater.state.clone();
                    }
                    viewer.window.request_redraw();
                }
                return;
            }
        };
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
                if scene.points > 0 {
                    title.push_str(&format!("  ·  {} points", scene.points));
                }
                if scene.lines > 0 {
                    title.push_str(&format!("  ·  {} segments", scene.lines));
                }
                if let Some(notice) = scene.notice {
                    title.push_str("  ·  ");
                    title.push_str(notice);
                }
                if scene.issues > 0 {
                    title.push_str(&format!("  ·  {} import warnings", scene.issues));
                }
                if !scene.diagnostics.is_empty() {
                    title.push_str("  ·  I: details");
                }
                if viewer.ui_enabled {
                    title = format!("VTX  ·  {}", loaded.name);
                }
                viewer.window.set_title(&title);
                viewer.name = loaded.name.clone();
                viewer.camera.clip = [0.; 4];
                viewer.pending_pick = None;
                viewer.pick_context = None;
                viewer.feature_job = None;
                viewer.screen_edges = None;
                viewer.hover_due = None;
                viewer.hover_at = None;
                viewer.hover_queue = None;
                viewer.pending_hover = None;
                viewer.click_candidate = None;
                viewer.pick_at = None;
                if let Some(ui) = &mut viewer.ui {
                    ui.tools = ui::Tools::default();
                }
                viewer.camera.fit(
                    &scene,
                    viewer.config.width as f32 / viewer.config.height as f32,
                );
                viewer.scene = Some(scene);
                viewer.first_frame = Some(loaded.started);
                // A hidden Windows child receives no WM_PAINT. Present its
                // first complete scene explicitly, then reveal it over the
                // saved image without a blank loading frame.
                #[cfg(windows)]
                if viewer.embedded.is_some() {
                    if let Err(error) = viewer.redraw() {
                        self.fatal = Some(error.into());
                        event_loop.exit();
                    }
                }
                viewer.scene_dirty = true;
                viewer.window.request_redraw();
            }
            Err(error) => {
                if self.preview_parent.is_some() {
                    self.fatal = Some(Box::new(error));
                    event_loop.exit();
                } else {
                    viewer.show_error(&loaded.name, &error.message);
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let first_view = self
            .viewer
            .as_ref()
            .is_some_and(|v| v.ui_enabled && v.ui.is_some() && v.first_frame.is_none())
            && !self.loading;
        let manual = self
            .viewer
            .as_mut()
            .and_then(|v| v.ui.as_mut())
            .is_some_and(|ui| std::mem::take(&mut ui.update_check));
        if first_view && (self.updater.automatic_due() || manual) {
            let proxy = self.proxy.clone();
            self.updater.check(manual, move |notice| {
                let _ = proxy.send_event(Event::Update(notice));
            });
            if let Some(viewer) = &mut self.viewer {
                if let Some(ui) = &mut viewer.ui {
                    ui.update_state = self.updater.state.clone();
                }
                viewer.window.request_redraw();
            }
        }
        let restart = self
            .viewer
            .as_mut()
            .and_then(|v| v.ui.as_mut())
            .is_some_and(|ui| std::mem::take(&mut ui.update_restart));
        if restart {
            let args = self
                .current_file
                .as_ref()
                .map(|p| vec![p.as_os_str().to_owned()])
                .unwrap_or_default();
            match self.updater.apply_on_exit(true, args) {
                Ok(()) => {
                    event_loop.exit();
                    return;
                }
                Err(error) => {
                    self.updater.state.message = error;
                    if let Some(viewer) = &mut self.viewer {
                        if let Some(ui) = &mut viewer.ui {
                            ui.update_state = self.updater.state.clone();
                        }
                        viewer.window.request_redraw();
                    }
                }
            }
        }
        if let Some(viewer) = &mut self.viewer {
            if viewer.ui_enabled {
                if let (Some(ui), Some(scene)) = (&mut viewer.ui, &viewer.scene) {
                    if !ui.tools.needs_features()
                        && (viewer.feature_job.is_some()
                            || viewer.pending_pick.is_some()
                            || ui.tools.pending)
                    {
                        viewer.feature_job = None;
                        viewer.pending_pick = None;
                        viewer.pick_context = None;
                        ui.tools.indexing = false;
                        ui.tools.cancel_pending();
                    }
                    if ui.tools.needs_features()
                        && ui.tools.index.is_none()
                        && !ui.tools.index_failed
                        && viewer.feature_job.is_none()
                    {
                        match features::Job::start(
                            scene,
                            &viewer.renderer.uploader.device,
                            &viewer.renderer.uploader.queue,
                        ) {
                            Ok(job) => {
                                viewer.feature_job = Some(job);
                                ui.tools.indexing = true;
                            }
                            Err(error) => {
                                ui.tools.index_failed = true;
                                ui.tools.message = error;
                            }
                        }
                        viewer.window.request_redraw();
                    }
                    if let Some(result) = viewer.feature_job.as_ref().and_then(features::Job::poll)
                    {
                        viewer.feature_job = None;
                        match result {
                            Ok(index) => {
                                ui.tools.set_index(index, scene);
                                if viewer.hover_at.is_some() {
                                    viewer.hover_due = Some(Instant::now());
                                }
                            }
                            Err(error) => {
                                ui.tools.indexing = false;
                                ui.tools.index_failed = true;
                                ui.tools.cancel_pending();
                                ui.tools.message =
                                    format!("Could not analyze these edges: {error}");
                            }
                        }
                        viewer.window.request_redraw();
                    }
                }
                if let Some(pending) = &viewer.pending_pick {
                    match pending.poll(&viewer.renderer.uploader.device) {
                        Ok(Some(point)) => {
                            viewer.pending_pick = None;
                            if let Some(candidate) = viewer.pending_hover.take() {
                                if let (Some(ui), Some(scene)) = (&mut viewer.ui, &viewer.scene) {
                                    let current = viewer.cursor.is_some_and(|p| {
                                        (p.x - candidate.pointer[0] as f64).abs() < 4.
                                            && (p.y - candidate.pointer[1] as f64).abs() < 4.
                                    });
                                    if current
                                        && (!ui.tools.measure
                                            || matches!(
                                                ui.tools.mode,
                                                ui::Mode::Edge | ui::Mode::Diameter
                                            ))
                                        && candidate.view
                                            == edges::key(
                                                &viewer.camera,
                                                [viewer.config.width, viewer.config.height],
                                            )
                                        && viewer.dragging.is_none()
                                        && !ui.ctx.is_pointer_over_area()
                                    {
                                        let visible = edges::visible(
                                            &candidate,
                                            point,
                                            &viewer.camera,
                                            scene,
                                        );
                                        ui.tools.hovered =
                                            ui.tools.index.as_ref().and_then(|index| {
                                                visible
                                                    .then(|| index.features[candidate.id].clone())
                                            });
                                    }
                                }
                            } else if let (Some(ui), Some(scene), Some(context)) =
                                (&mut viewer.ui, &viewer.scene, viewer.pick_context.take())
                            {
                                if ui.tools.mode == ui::Mode::Edge
                                    && ui.tools.index.is_some()
                                    && ui.tools.serial == context.serial
                                {
                                    let selected = viewer
                                        .click_candidate
                                        .take()
                                        .filter(|c| {
                                            edges::visible(c, point, &context.camera, scene)
                                        })
                                        .and_then(|c| {
                                            ui.tools
                                                .index
                                                .as_ref()
                                                .map(|index| index.features[c.id].clone())
                                        });
                                    ui.tools.select_edge(selected, context.additive);
                                    ui.tools.pending = false;
                                    ui.tools.message.clear();
                                } else {
                                    ui.tools.picked_at(point, &context, scene);
                                }
                            }
                            viewer.window.request_redraw();
                        }
                        Err(error) => {
                            viewer.pending_pick = None;
                            if let Some(ui) = &mut viewer.ui {
                                ui.tools.pending = false;
                                ui.tools.message = format!("Could not read this surface: {error}");
                            }
                            viewer.window.request_redraw();
                        }
                        Ok(None) => (),
                    }
                }
                if viewer.ui.as_ref().is_some_and(|ui| {
                    ui.tools.measure
                        && !matches!(ui.tools.mode, ui::Mode::Edge | ui::Mode::Diameter)
                }) {
                    viewer.hover_due = None;
                    viewer.hover_queue = None;
                }
                if viewer.hover_due.is_some_and(|due| due <= Instant::now())
                    && viewer.pending_pick.is_none()
                {
                    let camera = &viewer.camera;
                    if let (Some(at), Some(scene), Some(ui)) =
                        (viewer.hover_at, &viewer.scene, &mut viewer.ui)
                    {
                        if let Some(index) = &ui.tools.index {
                            viewer.hover_due = None;
                            let size = [viewer.config.width, viewer.config.height];
                            if viewer
                                .screen_edges
                                .as_ref()
                                .is_none_or(|cache| !cache.matches(camera, size))
                            {
                                viewer.screen_edges =
                                    Some(edges::ScreenEdges::new(index, camera, scene, size));
                            }
                            viewer.hover_queue = viewer.screen_edges.as_ref().unwrap().nearest(
                                index,
                                at,
                                10. * viewer.window.scale_factor(),
                                ui.tools.measure && ui.tools.mode == ui::Mode::Diameter,
                                scene,
                                camera,
                            );
                            if viewer.hover_queue.is_none() {
                                ui.tools.hovered = None;
                            }
                            viewer.window.request_redraw();
                        } else if ui.tools.index_failed {
                            viewer.hover_due = None;
                        }
                    }
                }
                if viewer.ui_repaint.is_some_and(|at| at <= Instant::now()) {
                    viewer.ui_repaint = None;
                    viewer.window.request_redraw();
                }
                let deadline = if viewer.pending_pick.is_some() || viewer.feature_job.is_some() {
                    Some(Instant::now() + std::time::Duration::from_millis(8))
                } else {
                    match (viewer.ui_repaint, viewer.hover_due) {
                        (Some(a), Some(b)) => Some(a.min(b)),
                        (a, b) => a.or(b),
                    }
                };
                event_loop.set_control_flow(deadline.map_or(
                    winit::event_loop::ControlFlow::Wait,
                    winit::event_loop::ControlFlow::WaitUntil,
                ));
            }
        }
        #[cfg(windows)]
        if let Some(viewer) = &mut self.viewer {
            if let Some(preview) = &viewer.embedded {
                if !preview.alive() {
                    event_loop.exit();
                    return;
                }
                if let Some(snapshot) = &viewer.snapshot {
                    match snapshot.pixels(&viewer.renderer.uploader.device) {
                        Ok(Some(mut pixels)) => {
                            let snapshot = viewer.snapshot.take().unwrap();
                            let parent = preview.parent;
                            let child = viewer.preview_child.unwrap_or(0);
                            let _ = thread::Builder::new()
                                .name("vtx preview cache".into())
                                .spawn(move || {
                                    if snapshot.bgra {
                                        for p in pixels.chunks_exact_mut(4) {
                                            p.swap(0, 2);
                                        }
                                    }
                                    if image::save_buffer(
                                        &snapshot.path,
                                        &pixels,
                                        1920,
                                        1440,
                                        image::ColorType::Rgba8,
                                    )
                                    .is_ok()
                                    {
                                        embedded::EmbeddedPreview::cached(parent, child);
                                    }
                                });
                        }
                        Err(error) => {
                            eprintln!("VTX: snapshot readback failed: {error}");
                            viewer.snapshot = None;
                        }
                        Ok(None) => (),
                    }
                }
                let delay = if viewer.snapshot.is_some() { 16 } else { 500 };
                event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                    Instant::now() + std::time::Duration::from_millis(delay),
                ));
            }
        }
        if let Some(deadline) = self.updater.deadline() {
            use winit::event_loop::ControlFlow;
            match event_loop.control_flow() {
                ControlFlow::Wait => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
                ControlFlow::WaitUntil(current) if deadline < current => {
                    event_loop.set_control_flow(ControlFlow::WaitUntil(deadline))
                }
                _ => (),
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(viewer) = self.viewer.as_mut() else {
            return;
        };
        if viewer.window.id() != id {
            return;
        }
        if let Some(ui) = &mut viewer.ui {
            let response = ui.event(&viewer.window, &event);
            if ui::schedule_repaint(&event, response.repaint) {
                viewer.window.request_redraw();
            }
            // Always track the cursor when crossing UI boundaries, and end a
            // canvas drag even if its release happens over a tool panel.
            if response.consumed && !matches!(&event, WindowEvent::CursorMoved { .. }) {
                if matches!(
                    &event,
                    WindowEvent::MouseInput {
                        state: ElementState::Released,
                        ..
                    }
                ) {
                    viewer.dragging = None;
                    viewer.measure_click = None;
                }
                return;
            }
        }
        match event {
            WindowEvent::CloseRequested => {
                cad_import::shutdown();
                if self.preview_parent.is_none() {
                    if let Err(error) = self.updater.apply_on_exit(false, Vec::new()) {
                        eprintln!("VTX: {error}");
                    }
                }
                event_loop.exit();
            }
            WindowEvent::DroppedFile(path) => {
                if self.preview_parent.is_none() {
                    self.open(path)
                }
            }
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
                if event.logical_key
                    == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape)
                {
                    if let Some(ui) = &mut viewer.ui {
                        ui.tools.measure = false;
                        ui.tools.section = false;
                        ui.tools.pending = false;
                        ui.tools.clear();
                        ui.tools.edge_discovery = false;
                    }
                    viewer.pending_pick = None;
                    viewer.pick_at = None;
                    viewer.hover_due = None;
                    viewer.hover_at = None;
                    viewer.hover_queue = None;
                    viewer.pending_hover = None;
                    viewer.camera.clip = [0.; 4];
                    viewer.scene_dirty = true;
                    viewer.window.request_redraw();
                    return;
                }
                if let Some(key) = event.logical_key.to_text() {
                    if key.eq_ignore_ascii_case("m") && viewer.ui.is_some() {
                        let ui = viewer.ui.as_mut().unwrap();
                        ui.tools.measure = !ui.tools.measure;
                        viewer.window.request_redraw();
                    } else if key.eq_ignore_ascii_case("s") && viewer.ui.is_some() {
                        let ui = viewer.ui.as_mut().unwrap();
                        ui.tools.section = !ui.tools.section;
                        viewer.scene_dirty = true;
                        viewer.window.request_redraw();
                    } else if key.eq_ignore_ascii_case("o")
                        && self.preview_parent.is_none()
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
                    } else if key.eq_ignore_ascii_case("e") {
                        viewer.camera.edges = !viewer.camera.edges;
                        viewer.scene_dirty = true;
                        viewer.window.request_redraw();
                    } else if key.eq_ignore_ascii_case("b") {
                        viewer.camera.light_background = !viewer.camera.light_background;
                        viewer.scene_dirty = true;
                        viewer.window.request_redraw();
                    } else if key.eq_ignore_ascii_case("i") {
                        if let Some(scene) = &viewer.scene {
                            let mut details = format!(
                                "{} triangles\n{} import warnings\nE: edge outlines | B: light/dark background | R: fit view\n\n",
                                scene.triangles, scene.issues
                            );
                            if scene.diagnostics.is_empty() {
                                details.push_str("No additional import diagnostics.");
                            } else {
                                details.push_str(
                                    &scene
                                        .diagnostics
                                        .iter()
                                        .take(8)
                                        .cloned()
                                        .collect::<Vec<_>>()
                                        .join("\n\n"),
                                );
                                if scene.diagnostics.len() > 8 {
                                    details.push_str(&format!("\n\n{} additional details are available in --check diagnostics.", scene.diagnostics.len() - 8));
                                }
                            }
                            rfd::MessageDialog::new()
                                .set_parent(&*viewer.window)
                                .set_title("VTX model details")
                                .set_description(&details)
                                .show();
                        }
                    } else if key.eq_ignore_ascii_case("f") {
                        if let Some(scene) = &viewer.scene {
                            viewer.camera.frame(
                                scene,
                                viewer.config.width as f32 / viewer.config.height as f32,
                            );
                            viewer.scene_dirty = true;
                            viewer.window.request_redraw();
                        }
                    } else if key.eq_ignore_ascii_case("r") {
                        if let Some(scene) = &viewer.scene {
                            viewer.camera.fit(
                                scene,
                                viewer.config.width as f32 / viewer.config.height as f32,
                            );
                        }
                        viewer.scene_dirty = true;
                        viewer.window.request_redraw();
                    }
                }
            }
            WindowEvent::Focused(false) => {
                viewer.hover_due = None;
                viewer.hover_at = None;
                viewer.hover_queue = None;
                if let Some(ui) = &mut viewer.ui {
                    ui.tools.hovered = None;
                }
                viewer.dragging = None;
                viewer.modifiers = ModifiersState::empty();
                viewer.middle_clicks.clear();
            }
            WindowEvent::CursorLeft { .. } => {
                viewer.cursor = None;
                viewer.hover_due = None;
                viewer.hover_at = None;
                viewer.hover_queue = None;
                if let Some(ui) = &mut viewer.ui {
                    ui.tools.hovered = None;
                }
                viewer.window.request_redraw();
            }
            WindowEvent::CursorEntered { .. } =>
            {
                #[cfg(windows)]
                if viewer.embedded.is_some() {
                    embedded::EmbeddedPreview::focus_on_hover(&viewer.window);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Left && viewer.ui.is_some() {
                    if state == ElementState::Pressed {
                        viewer.measure_click = viewer.cursor;
                        if viewer.scene.as_ref().is_some_and(|s| s.plan_view)
                            && viewer.ui.as_ref().is_some_and(|ui| !ui.tools.measure)
                        {
                            viewer.dragging = Some(MouseButton::Left);
                        }
                    } else if let (Some(start), Some(end)) =
                        (viewer.measure_click.take(), viewer.cursor)
                    {
                        viewer.dragging = None;
                        if viewer.scene.as_ref().is_some_and(|s| s.raster_size.is_some()) {
                            return;
                        }
                        if viewer.pending_hover.take().is_some() {
                            viewer.pending_pick = None;
                        }
                        if (start.x - end.x).abs() < 5.
                            && (start.y - end.y).abs() < 5.
                            && viewer.pending_pick.is_none()
                            && viewer.scene.is_some()
                        {
                            viewer.pick_at = Some([
                                end.x
                                    .max(0.)
                                    .min(viewer.config.width.saturating_sub(1) as f64)
                                    as u32,
                                end.y
                                    .max(0.)
                                    .min(viewer.config.height.saturating_sub(1) as f64)
                                    as u32,
                            ]);
                            if let Some(ui) = &mut viewer.ui {
                                if !ui.tools.measure {
                                    if ui.tools.mode != ui::Mode::Edge {
                                        ui.tools.clear();
                                    }
                                    ui.tools.mode = ui::Mode::Edge;
                                    ui.tools.edge_discovery = true;
                                }
                                ui.tools.pending = true;
                                ui.tools.message.clear();
                            }
                            viewer.pick_additive = viewer.modifiers.control_key();
                            viewer.hover_queue = None;
                            viewer.window.request_redraw();
                        }
                    }
                    return;
                }
                if matches!(
                    button,
                    MouseButton::Left | MouseButton::Right | MouseButton::Middle
                ) {
                    if state == ElementState::Pressed {
                        #[cfg(windows)]
                        if viewer.embedded.is_some() {
                            embedded::EmbeddedPreview::focus(&viewer.window);
                        } else {
                            viewer.window.focus_window();
                        }
                        #[cfg(not(windows))]
                        viewer.window.focus_window();
                        viewer.dragging = Some(button);
                        if button == MouseButton::Middle && viewer.modifiers.is_empty() {
                            viewer
                                .middle_clicks
                                .press(viewer.cursor.map(|p| [p.x, p.y]));
                        } else {
                            viewer.middle_clicks.clear();
                        }
                    } else if viewer.dragging == Some(button) {
                        viewer.dragging = None;
                        if button == MouseButton::Middle
                            && viewer.modifiers.is_empty()
                            && viewer.middle_clicks.release(
                                Instant::now(),
                                viewer.cursor.map(|p| [p.x, p.y]),
                                viewer.navigation.double_click_ms,
                            )
                        {
                            if let Some(scene) = &viewer.scene {
                                viewer.camera.frame(
                                    scene,
                                    viewer.config.width as f32 / viewer.config.height as f32,
                                );
                                viewer.scene_dirty = true;
                                viewer.window.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                viewer.hover_recheck = true;
                #[cfg(windows)]
                if viewer.embedded.is_some() {
                    embedded::EmbeddedPreview::focus_on_hover(&viewer.window);
                }
                viewer.middle_clicks.motion([position.x, position.y]);
                if viewer.dragging.is_some() {
                    viewer.hover_due = None;
                    viewer.hover_at = None;
                    viewer.hover_queue = None;
                    if let Some(ui) = &mut viewer.ui {
                        ui.tools.hovered = None;
                    }
                }
                if let Some(previous) = viewer
                    .cursor
                    .replace(position)
                    .filter(|_| viewer.dragging.is_some())
                {
                    if let Some(scene) = viewer.scene.as_ref() {
                        let size = viewer.window.inner_size();
                        let delta = [
                            (position.x - previous.x) as f32,
                            (position.y - previous.y) as f32,
                        ];
                        match navigation::drag(viewer.dragging, viewer.modifiers, scene.plan_view) {
                            Some(navigation::Drag::Orbit) => {
                                viewer.camera.orbit(delta, viewer.navigation.rotate_speed)
                            }
                            Some(navigation::Drag::Pan) => viewer.camera.pan_by(
                                delta,
                                scene.diameter,
                                [size.width, size.height],
                            ),
                            Some(navigation::Drag::Zoom) => viewer.camera.zoom_at(
                                -delta[1] * 0.01,
                                None,
                                scene.diameter,
                                [size.width, size.height],
                            ),
                            Some(navigation::Drag::Roll) => {
                                viewer.camera.roll(delta[0], viewer.navigation.rotate_speed)
                            }
                            None => return,
                        }
                    }
                    viewer.scene_dirty = true;
                    viewer.window.request_redraw();
                }
                viewer.queue_hover();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(position) => position.y as f32 / 40.0,
                };
                if let Some(scene) = &viewer.scene {
                    let size = viewer.window.inner_size();
                    let anchor = if viewer.navigation.zoom_about_center {
                        None
                    } else {
                        viewer.cursor.map(|p| [p.x as f32, p.y as f32])
                    };
                    viewer.camera.zoom_at(
                        viewer.navigation.wheel_exponent(lines),
                        anchor,
                        scene.diameter,
                        [size.width, size.height],
                    );
                }
                viewer.scene_dirty = true;
                if let Some(ui) = &mut viewer.ui {
                    ui.tools.hovered = None;
                }
                viewer.queue_hover();
                viewer.window.request_redraw();
            }
            _ => (),
        }
        if let Some(viewer) = &mut self.viewer {
            if viewer
                .ui
                .as_mut()
                .is_some_and(|ui| std::mem::take(&mut ui.tools.open))
            {
                let path = rfd::FileDialog::new()
                    .add_filter("Models and drawings", model::FORMATS)
                    .set_parent(&*viewer.window)
                    .pick_file();
                if let Some(path) = path {
                    self.open(path);
                }
            }
        }
    }
}

pub fn run() -> Result<()> {
    updates::startup();
    let mut args = std::env::args_os().skip(1);
    let startup = args.next();
    if startup.as_deref().is_some_and(|s| s == "--version") {
        println!("VTX {}", updates::VERSION);
        return Ok(());
    }
    if startup.as_deref().is_some_and(|s| s == "--update-smoke") {
        let path = args
            .next()
            .ok_or("Usage: vtx --update-smoke LOCAL_FEED [--apply]")?;
        let apply = args.next().is_some_and(|s| s == "--apply");
        return updates::smoke(Path::new(&path), apply).map_err(Into::into);
    }
    if startup
        .as_deref()
        .is_some_and(|arg| arg == "--saved-preview")
    {
        let path = args
            .next()
            .ok_or("Usage: vtx --saved-preview MODEL OUTPUT.png")?;
        let output = args.next().ok_or("Missing preview output path")?;
        return crate::solidworks::saved_preview(Path::new(&path), Path::new(&output));
    }
    if startup.as_deref().is_some_and(|arg| arg == "--preview") {
        let path = args.next().ok_or("Usage: vtx --preview MODEL OUTPUT.png")?;
        let output = args.next().ok_or("Missing preview output path")?;
        let allow_incomplete = args
            .next()
            .is_some_and(|s| s == "--allow-incomplete-decals");
        return render::preview(Path::new(&path), Path::new(&output), allow_incomplete);
    }
    if startup.as_deref().is_some_and(|arg| arg == "--check") {
        let path = args
            .next()
            .ok_or("Usage: vtx --check MODEL [SCREENSHOT.png]")?;
        let screenshot = args.next();
        return render::check(Path::new(&path), screenshot.as_deref().map(Path::new));
    }
    if startup.as_deref().is_some_and(|arg| arg == "--bench-open") {
        return render::bench_open(&args.map(PathBuf::from).collect::<Vec<_>>());
    }
    if startup.as_deref().is_some_and(|arg| arg == "--ui-check") {
        let model = args
            .next()
            .ok_or("Usage: vtx --ui-check MODEL OUTPUT.png [clean|measure|section|ortho]")?;
        let output = args.next().ok_or("Missing UI screenshot output")?;
        let mode = args.next().unwrap_or_else(|| "clean".into());
        return render::inspect_check(
            Path::new(&model),
            Path::new(&output),
            &mode.to_string_lossy(),
        );
    }
    let mut preview_parent = None;
    let mut preview_cache = None;
    let startup = if startup
        .as_deref()
        .is_some_and(|s| s == "--explorer-preview")
    {
        #[cfg(not(windows))]
        return Err("Embedded Explorer preview requires Windows".into());
        let parent = args
            .next()
            .ok_or("Missing Explorer preview owner")?
            .to_string_lossy()
            .parse::<usize>()?;
        let model = args.next().ok_or("Missing Explorer model path")?;
        preview_parent = Some(parent);
        preview_cache = args.next().map(PathBuf::from);
        #[cfg(windows)]
        let _ = embedded::EmbeddedPreview::new(parent, preview_cache.clone())?;
        Some(model)
    } else {
        startup
    };
    let mut builder = EventLoop::<Event>::with_user_event();
    #[cfg(windows)]
    if preview_parent.is_some() {
        use winit::platform::windows::EventLoopBuilderExtWindows;
        builder.with_dpi_aware(false);
    }
    let event_loop = builder.build()?;
    let mut app = App {
        proxy: event_loop.create_proxy(),
        viewer: None,
        startup: startup.map(PathBuf::from),
        loading: false,
        fatal: None,
        preview_parent,
        preview_cache,
        updater: updates::Controller::default(),
        current_file: None,
    };
    event_loop.run_app(&mut app)?;
    app.fatal.map_or(Ok(()), Err)
}
