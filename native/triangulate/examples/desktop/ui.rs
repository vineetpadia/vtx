use super::{
    camera::Camera,
    edges,
    features::{Feature, Index},
    model::Scene,
    ui_gpu::Painter,
};
use egui::{Color32, RichText, Vec2};
use nalgebra_glm as glm;
use winit::{event::WindowEvent, window::Window};

const INK: Color32 = Color32::from_rgb(28, 39, 53);
const ACCENT: Color32 = Color32::from_rgb(0, 124, 168);
pub fn schedule_repaint(event: &WindowEvent, repaint: bool) -> bool {
    repaint && !matches!(event, WindowEvent::RedrawRequested)
}
fn tool_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::WHITE)
        .stroke(egui::Stroke::new(1., Color32::from_gray(224)))
        .corner_radius(9)
        .inner_margin(14)
}
#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Distance,
    Angle,
    Diameter,
    Edge,
}
#[derive(Clone)]
pub struct PickContext {
    pub camera: Camera,
    pub size: [u32; 2],
    pub at: [u32; 2],
    pub serial: u64,
    pub additive: bool,
}
pub struct Tools {
    pub measure: bool,
    pub section: bool,
    pub points: Vec<[f32; 3]>,
    pub mode: Mode,
    pub pending: bool,
    pub message: String,
    pub open: bool,
    pub index: Option<Index>,
    pub indexing: bool,
    pub index_failed: bool,
    pub selected: Option<Feature>,
    pub selected_edges: Vec<Feature>,
    pub pair: Option<edges::Gap>,
    pub hovered: Option<Feature>,
    pub edge_discovery: bool,
    pub snap: bool,
    pub serial: u64,
    deferred: Option<(Option<[f32; 3]>, PickContext)>,
    axis: usize,
    position: f32,
    flip: bool,
    source_units: Option<f64>,
    inches: bool,
    help: bool,
    info: bool,
    pub updates_open: bool,
}
impl Default for Tools {
    fn default() -> Self {
        Self {
            measure: false,
            section: false,
            points: Vec::new(),
            mode: Mode::Edge,
            pending: false,
            message: String::new(),
            open: false,
            index: None,
            indexing: false,
            index_failed: false,
            selected: None,
            selected_edges: Vec::new(),
            pair: None,
            hovered: None,
            edge_discovery: false,
            snap: true,
            serial: 0,
            deferred: None,
            axis: 0,
            position: 0.5,
            flip: false,
            source_units: None,
            inches: false,
            help: false,
            info: false,
            updates_open: false,
        }
    }
}
pub fn distance(a: [f32; 3], b: [f32; 3]) -> f64 {
    (0..3)
        .map(|i| (f64::from(a[i]) - f64::from(b[i])).powi(2))
        .sum::<f64>()
        .sqrt()
}
pub fn angle(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Option<f64> {
    let ab = [0, 1, 2].map(|i| f64::from(a[i]) - f64::from(b[i]));
    let cb = [0, 1, 2].map(|i| f64::from(c[i]) - f64::from(b[i]));
    let length = distance(a, b) * distance(c, b);
    (length > 1e-15).then(|| {
        ((0..3).map(|i| ab[i] * cb[i]).sum::<f64>() / length)
            .clamp(-1., 1.)
            .acos()
            .to_degrees()
    })
}
impl Tools {
    pub fn needs_features(&self) -> bool {
        self.edge_discovery
            || (self.measure && (self.snap || matches!(self.mode, Mode::Diameter | Mode::Edge)))
    }
    pub fn clear(&mut self) {
        self.points.clear();
        self.selected = None;
        self.selected_edges.clear();
        self.pair = None;
        self.hovered = None;
        self.deferred = None;
        self.pending = false;
        self.serial = self.serial.wrapping_add(1);
        self.message.clear();
    }
    pub fn cancel_pending(&mut self) {
        self.pending = false;
        self.deferred = None;
        self.serial = self.serial.wrapping_add(1);
    }
    pub fn set_index(&mut self, index: Index, scene: &Scene) {
        self.indexing = false;
        self.index = Some(index);
        if let Some((point, context)) = self.deferred.take() {
            self.picked_at(point, &context, scene);
        }
    }
    pub fn picked_at(&mut self, point: Option<[f32; 3]>, context: &PickContext, scene: &Scene) {
        if scene.raster_size.is_some()
            || (!self.measure && !(self.edge_discovery && self.mode == Mode::Edge))
            || self.serial != context.serial
        {
            self.pending = false;
            return;
        }
        if self.needs_features() && self.indexing {
            self.deferred = Some((point, context.clone()));
            return;
        }
        self.pending = false;
        if matches!(self.mode, Mode::Diameter | Mode::Edge) {
            let selected = self.index.as_ref().and_then(|index| {
                index.select(
                    &context.camera,
                    scene,
                    context.size,
                    context.at,
                    point,
                    self.mode == Mode::Diameter,
                )
            });
            if self.mode == Mode::Edge {
                self.select_edge(selected, context.additive);
            } else {
                self.selected = selected;
            }
            self.message = if self.selected.is_none() {
                if self.mode == Mode::Diameter {
                    "No circular feature here. Click a circular rim or cylindrical wall."
                } else {
                    "No edge here. Click near a straight edge or arc."
                }
                .into()
            } else {
                String::new()
            };
        } else {
            let snap = self
                .snap
                .then(|| {
                    self.index.as_ref().and_then(|index| {
                        index.snap(&context.camera, scene, context.size, context.at, point)
                    })
                })
                .flatten();
            self.picked(snap.map(|s| s.0).or(point));
            if let Some((_, name)) = snap {
                self.message = format!("{name} snap");
            }
        }
    }
    pub fn select_edge(&mut self, feature: Option<Feature>, additive: bool) {
        if let Some(feature) = feature {
            if additive {
                if let Some(i) = self.selected_edges.iter().position(|f| *f == feature) {
                    self.selected_edges.remove(i);
                } else {
                    if self.selected_edges.len() == 2 {
                        self.selected_edges.remove(0);
                    }
                    self.selected_edges.push(feature);
                }
            } else {
                self.selected_edges = vec![feature];
            }
            if !self.selected_edges.is_empty() {
                self.measure = true;
            }
        } else if !additive {
            self.selected_edges.clear();
        }
        self.selected = self.selected_edges.last().cloned();
        self.pair = if self.selected_edges.len() == 2 {
            Some(edges::gap(&self.selected_edges[0], &self.selected_edges[1]))
        } else {
            None
        };
    }
    pub fn picked(&mut self, point: Option<[f32; 3]>) {
        self.pending = false;
        if !self.measure {
            return;
        }
        if let Some(p) = point {
            let required = if self.mode == Mode::Distance { 2 } else { 3 };
            if self.points.len() >= required {
                self.points.clear();
            }
            self.points.push(p);
            self.message.clear();
        } else {
            self.message = "No surface here. Click on the model.".into();
        }
    }
    fn scale(&self, scene: &Scene) -> (f64, &'static str) {
        match scene.meters_per_unit.or(self.source_units) {
            Some(m) => (
                m * if self.inches { 1. / 0.0254 } else { 1000. },
                if self.inches { "in" } else { "mm" },
            ),
            None => (1., "units"),
        }
    }
    pub fn result(&self, scene: &Scene) -> Option<String> {
        if scene.raster_size.is_some() { return None; }
        let (scale, suffix) = self.scale(scene);
        if self.mode == Mode::Diameter {
            return self
                .selected
                .as_ref()?
                .circle()
                .map(|c| format!("Ø {:.3} {suffix}", c.radius * 2. * scale));
        }
        if self.mode == Mode::Edge {
            if let Some(pair) = &self.pair {
                return Some(format!("{:.3} {suffix}", pair.distance * scale));
            }
            return self
                .selected
                .as_ref()
                .map(|f| format!("{:.3} {suffix}", f.length() * scale));
        }
        if self.mode == Mode::Angle && self.points.len() == 3 {
            return angle(self.points[0], self.points[1], self.points[2])
                .map(|v| format!("{v:.2}°"));
        }
        if self.mode == Mode::Distance && self.points.len() == 2 {
            let (scale, suffix) = self.scale(scene);
            return Some(format!(
                "{:.3} {suffix}",
                distance(self.points[0], self.points[1]) * scale
            ));
        }
        None
    }
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        scene: Option<&Scene>,
        camera: &mut Camera,
        size: [u32; 2],
        name: &str,
    ) -> bool {
        let mut changed = false;
        let available = scene.is_some();
        if !available {
            egui::Area::new(egui::Id::new("empty-state"))
                .anchor(egui::Align2::CENTER_CENTER, [0., 0.])
                .interactable(false)
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("Open a model").size(23.).color(INK));
                        ui.add_space(7.);
                        ui.label(
                            RichText::new("Drop a model or drawing here, or use Open above.")
                                .size(13.)
                                .color(Color32::from_gray(120)),
                        );
                    });
                });
        }
        egui::TopBottomPanel::top("toolbar")
            .exact_height(48.)
            .frame(
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .stroke(egui::Stroke::new(1., Color32::from_gray(233))),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("VTX").strong().color(INK).size(15.));
                    ui.add_space(10.);
                    if ui
                        .button("Open")
                        .on_hover_text("Open model · Ctrl+O")
                        .clicked()
                    {
                        self.open = true;
                    }
                    ui.add_enabled_ui(available && scene.is_some_and(|s| s.raster_size.is_none()), |ui| {
                        if ui
                            .selectable_label(self.measure, "Measure")
                            .on_hover_text("Select edges or measure points · M")
                            .clicked()
                        {
                            self.measure = !self.measure;
                            self.message.clear();
                        }
                        if ui
                            .selectable_label(self.section, "Section")
                            .on_hover_text("Inspect inside with a visual cut · S")
                            .clicked()
                        {
                            self.section = !self.section;
                            changed = true;
                        }
                        ui.menu_button("Views", |ui| {
                            let views = [
                                ("Isometric", -0.6f32, 0.4f32),
                                ("Front", 0., 0.),
                                ("Back", std::f32::consts::PI, 0.),
                                ("Left", -std::f32::consts::FRAC_PI_2, 0.),
                                ("Right", std::f32::consts::FRAC_PI_2, 0.),
                                ("Top", 0., std::f32::consts::FRAC_PI_2),
                                ("Bottom", 0., -std::f32::consts::FRAC_PI_2),
                            ];
                            for (label, yaw, pitch) in views {
                                if ui.button(label).clicked() {
                                    camera.rotation =
                                        glm::quat_angle_axis(pitch, &glm::vec3(1., 0., 0.))
                                            * glm::quat_angle_axis(yaw, &glm::vec3(0., 1., 0.));
                                    if let Some(scene) = scene {
                                        camera.frame(scene, size[0] as f32 / size[1].max(1) as f32);
                                    }
                                    changed = true;
                                    ui.close();
                                }
                            }
                            ui.separator();
                            if ui
                                .checkbox(&mut camera.orthographic, "Orthographic")
                                .changed()
                            {
                                if let Some(scene) = scene {
                                    camera.frame(scene, size[0] as f32 / size[1].max(1) as f32);
                                }
                                changed = true;
                            }
                            changed |= ui.checkbox(&mut camera.edges, "Edges").changed();
                            if ui.button("Fit view     F").clicked() {
                                if let Some(scene) = scene {
                                    camera.frame(scene, size[0] as f32 / size[1].max(1) as f32);
                                }
                                changed = true;
                                ui.close();
                            }
                        });
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button("···", |ui| {
                            ui.label(format!("VTX {}", super::updates::VERSION));
                            if ui.button("Check for updates").clicked() {
                                self.updates_open = true;
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Model info").clicked() {
                                self.info = true;
                                ui.close();
                            }
                            if ui.button("Controls").clicked() {
                                self.help = true;
                                ui.close();
                            }
                            if ui.button("Reset view").clicked() {
                                if let Some(scene) = scene {
                                    camera.fit(scene, size[0] as f32 / size[1].max(1) as f32);
                                }
                                changed = true;
                                ui.close();
                            }
                        });
                        ui.add(
                            egui::Label::new(
                                RichText::new(name).color(Color32::from_gray(120)).size(12.),
                            )
                            .truncate(),
                        );
                    });
                });
            });
        if self.measure {
            egui::Window::new("Measure")
                .id(egui::Id::new("measure-panel"))
                .title_bar(false)
                .frame(tool_frame())
                .anchor(egui::Align2::RIGHT_TOP, [-18., 66.])
                .default_width(264.)
                .fade_in(false)
                .fade_out(false)
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Measure").strong().size(15.));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button("×")
                                .on_hover_text("Close measure")
                                .clicked()
                            {
                                self.measure = false;
                            }
                        });
                    });
                    ui.add_space(5.);
                    let old = self.mode;
                    egui::ComboBox::from_id_salt("measurement-mode")
                        .width(235.)
                        .selected_text(match self.mode {
                            Mode::Distance => "Point distance",
                            Mode::Angle => "Angle",
                            Mode::Diameter => "Diameter / radius",
                            Mode::Edge => "Edges",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.mode, Mode::Edge, "Edges");
                            ui.selectable_value(&mut self.mode, Mode::Distance, "Point distance");
                            ui.selectable_value(&mut self.mode, Mode::Angle, "Angle");
                            ui.selectable_value(
                                &mut self.mode,
                                Mode::Diameter,
                                "Diameter / radius",
                            );
                        });
                    if old != self.mode {
                        self.clear();
                    }
                    ui.add_space(8.);
                    let hint = match self.mode {
                        Mode::Distance => "Click two points on the model.",
                        Mode::Angle => "Click three points. The middle point is the vertex.",
                        Mode::Diameter => "Click a circular rim or cylindrical wall.",
                        Mode::Edge => "Click an edge. Ctrl-click another to compare.",
                    };
                    ui.label(RichText::new(hint).color(Color32::from_gray(105)).size(12.));
                    if matches!(self.mode, Mode::Distance | Mode::Angle) {
                        ui.checkbox(&mut self.snap, "Snap to endpoints and centers");
                    }
                    if let Some(scene) = scene {
                        if scene.meters_per_unit.is_none() {
                            ui.add_space(8.);
                            ui.horizontal(|ui| {
                                ui.label("Source units");
                                egui::ComboBox::from_id_salt("source-units")
                                    .selected_text(match self.source_units {
                                        Some(v) if v == 0.001 => "mm",
                                        Some(v) if v == 0.0254 => "inches",
                                        Some(_) => "meters",
                                        None => "Unspecified",
                                    })
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.source_units,
                                            None,
                                            "Unspecified",
                                        );
                                        ui.selectable_value(
                                            &mut self.source_units,
                                            Some(0.001),
                                            "Millimeters",
                                        );
                                        ui.selectable_value(
                                            &mut self.source_units,
                                            Some(0.0254),
                                            "Inches",
                                        );
                                        ui.selectable_value(
                                            &mut self.source_units,
                                            Some(1.),
                                            "Meters",
                                        );
                                    });
                            });
                        }
                        if scene.meters_per_unit.or(self.source_units).is_some() {
                            ui.horizontal(|ui| {
                                ui.label("Display");
                                ui.selectable_value(&mut self.inches, false, "mm");
                                ui.selectable_value(&mut self.inches, true, "in");
                            });
                        }
                        ui.add_space(10.);
                        if let Some(result) = self.result(scene) {
                            if self.mode == Mode::Edge {ui.label(if self.pair.as_ref().is_some_and(|p|p.curved){"Approx. minimum gap"}else if self.pair.is_some(){"Minimum gap"}else{"Edge length"});}
                            ui.label(RichText::new(&result).size(28.).strong().color(INK));
                            if self.mode == Mode::Edge && self.pair.is_some() {
                                let (scale,suffix)=self.scale(scene);
                                let mut remove=None;
                                for (i,f) in self.selected_edges.iter().enumerate(){ui.horizontal(|ui|{ui.label(RichText::new(format!("Edge {} · {:.3} {suffix}",i+1,f.length()*scale)).color(if i==0{ACCENT}else{Color32::from_rgb(0,139,98)}));if ui.small_button("×").on_hover_text("Remove edge").clicked(){remove=Some(i);}});}
                                if let Some(i)=remove{let f=self.selected_edges[i].clone();self.select_edge(Some(f),true);}
                                if let Some(pair)=&self.pair {
                                    if let Some(angle)=pair.angle {ui.label(format!("Acute angle  {angle:.2}°"));}
                                    if let (Some(a),Some(b))=(self.selected_edges[0].circle(),self.selected_edges[1].circle()){ui.label(format!("Center distance  {:.3} {suffix}",(0..3).map(|i|(a.center[i]-b.center[i]).powi(2)).sum::<f64>().sqrt()*scale));}
                                    ui.label(format!("Total edge length  {:.3} {suffix}",self.selected_edges.iter().map(Feature::length).sum::<f64>()*scale));
                                }
                            } else if matches!(self.mode, Mode::Diameter | Mode::Edge) {
                                if let Some(feature) = &self.selected {
                                    let (scale, suffix) = self.scale(scene);
                                    if let Some(c) = feature.circle() {
                                        if self.mode==Mode::Edge{ui.label(format!("Diameter  {:.3} {suffix}",c.radius*2.*scale));}
                                        ui.label(format!(
                                            "Radius  {:.3} {suffix}",
                                            c.radius * scale
                                        ));
                                        ui.label(if matches!(feature, Feature::Cylinder { .. }) {
                                            "Cylindrical wall"
                                        } else if c.sweep.abs() > std::f64::consts::TAU - 0.02 {
                                            "Circular edge"
                                        } else {
                                            "Circular arc"
                                        });
                                    } else {
                                        ui.label(if matches!(feature,Feature::Line{..}){"Straight edge"}else{"Curved edge"});
                                    }
                                }
                            }
                            if self.mode == Mode::Distance {
                                let (scale, suffix) = self.scale(scene);
                                let a = self.points[0];
                                let b = self.points[1];
                                ui.label(
                                    RichText::new(format!(
                                        "ΔX  {:.3}     ΔY  {:.3}     ΔZ  {:.3} {suffix}",
                                        (f64::from(b[0]) - f64::from(a[0])) * scale,
                                        (f64::from(b[1]) - f64::from(a[1])) * scale,
                                        (f64::from(b[2]) - f64::from(a[2])) * scale
                                    ))
                                    .size(11.)
                                    .color(Color32::from_gray(100)),
                                );
                            }
                            ui.add_space(6.);
                            ui.horizontal(|ui| {
                                if ui.button("Copy").clicked() {
                                    let mut text = result.clone();
                                    if let Some(pair)=&self.pair {
                                        let (scale,suffix)=self.scale(scene);
                                        text=format!("Minimum gap: {result}; edge 1: {:.3} {suffix}; edge 2: {:.3} {suffix}",self.selected_edges[0].length()*scale,self.selected_edges[1].length()*scale);
                                        if let Some(angle)=pair.angle{text.push_str(&format!("; acute angle: {angle:.2}°"));}
                                        text.push_str("; approximate edge measurement");
                                    }
                                    if let Some(c) =
                                        self.selected.as_ref().and_then(Feature::circle)
                                    {
                                        let (scale, suffix) = self.scale(scene);
                                        text.push_str(&format!(
                                            "; R {:.3} {suffix}; {}",
                                            c.radius * scale,
                                            if c.exact {
                                                "source DXF circle"
                                            } else {
                                                "approximate mesh fit"
                                            }
                                        ));
                                    }
                                    ctx.copy_text(text);
                                }
                                if ui.button("Clear").clicked() {
                                    self.clear();
                                }
                            });
                        } else if matches!(self.mode, Mode::Distance | Mode::Angle) {
                            ui.label(
                                RichText::new(format!(
                                    "{} / {} points",
                                    self.points.len(),
                                    if self.mode == Mode::Distance { 2 } else { 3 }
                                ))
                                .size(23.)
                                .color(Color32::from_gray(160)),
                            );
                            if !self.points.is_empty() && ui.button("Clear").clicked() {
                                self.clear();
                            }
                        } else {
                            ui.label(
                                RichText::new("Select an edge")
                                    .size(23.)
                                    .color(Color32::from_gray(160)),
                            );
                        }
                        ui.add_space(9.);
                        ui.separator();
                        let exact = self
                            .selected
                            .as_ref()
                            .and_then(Feature::circle)
                            .is_some_and(|c| c.exact) && self.pair.is_none();
                        ui.label(
                            RichText::new(if exact {
                                "Source geometry · DXF circle / arc"
                            } else if matches!(self.mode, Mode::Diameter | Mode::Edge) {
                                "Recognized feature · mesh approximation"
                            } else {
                                "Surface measurement · mesh approximation"
                            })
                            .size(11.)
                            .color(Color32::from_gray(125)),
                        );
                        if self
                            .selected
                            .as_ref()
                            .and_then(Feature::circle)
                            .is_some_and(|c| !c.exact)
                        {
                            ui.label(
                                RichText::new("Mesh fit; not an exact CAD dimension.")
                                    .size(11.)
                                    .color(Color32::from_gray(125)),
                            );
                        }
                        if scene.meters_per_unit.is_none() && self.source_units.is_none() {
                            ui.label(
                                RichText::new("Choose source units to read mm or inches.")
                                    .size(11.)
                                    .color(Color32::from_rgb(145, 100, 25)),
                            );
                        }
                        if self.indexing {
                            ui.label("Finding edges…");
                        } else if self.pending {
                            ui.label("Reading surface…");
                        } else if !self.message.is_empty() {
                            ui.label(&self.message);
                        }
                    }
                    if ui.small_button("Done").clicked() {
                        self.measure = false;
                    }
                });
        }
        if self.section {
            egui::Window::new("Section")
                .id(egui::Id::new("section-panel"))
                .title_bar(false)
                .frame(tool_frame())
                .anchor(egui::Align2::LEFT_TOP, [18., 66.])
                .default_width(232.)
                .fade_in(false)
                .fade_out(false)
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Section").strong().size(15.));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button("×")
                                .on_hover_text("Close section")
                                .clicked()
                            {
                                self.section = false;
                                changed = true;
                            }
                        });
                    });
                    ui.add_space(5.);
                    ui.horizontal(|ui| {
                        for (i, label) in ["X", "Y", "Z"].iter().enumerate() {
                            changed |= ui.selectable_value(&mut self.axis, i, *label).changed();
                        }
                        changed |= ui.checkbox(&mut self.flip, "Flip").changed();
                    });
                    changed |= ui
                        .add(egui::Slider::new(&mut self.position, 0.0..=1.0).show_value(false))
                        .changed();
                    ui.label(
                        RichText::new("Visual cut · no generated cap")
                            .size(11.)
                            .color(Color32::from_gray(125)),
                    );
                    if ui.small_button("Done").clicked() {
                        self.section = false;
                        changed = true;
                    }
                });
        }
        let old_clip = camera.clip;
        camera.clip = [0.; 4];
        if self.section {
            if let Some(scene) = scene {
                let sign = if self.flip { -1. } else { 1. };
                camera.clip[self.axis] = sign;
                camera.clip[3] = sign
                    * (scene.bounds[0][self.axis]
                        + self.position
                            * (scene.bounds[1][self.axis] - scene.bounds[0][self.axis]));
            }
        }
        changed |= old_clip != camera.clip;
        if changed {
            self.hovered = None;
        }
        if let Some(scene) = scene {
            egui::TopBottomPanel::bottom("status")
                .exact_height(26.)
                .frame(
                    egui::Frame::new()
                        .fill(Color32::WHITE)
                        .inner_margin(egui::Margin::symmetric(14, 4)),
                )
                .show(ctx, |ui| {
                    let (scale, suffix) = self.scale(scene);
                    let spans = [0, 1, 2]
                        .map(|i| f64::from(scene.bounds[1][i] - scene.bounds[0][i]) * scale);
                    let dimensions = if let Some([w, h]) = scene.raster_size {
                        format!("Saved image · {w} × {h} px")
                    } else {
                        format!("{:.2} × {:.2} × {:.2} {suffix}", spans[0], spans[1], spans[2])
                    };
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(dimensions)
                            .size(11.)
                            .color(Color32::from_gray(115)),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(if scene.raster_size.is_some() {
                                    "Middle drag pan  ·  Scroll zoom  ·  F fit"
                                } else {
                                    "Middle drag rotate  ·  Ctrl+middle pan  ·  Scroll zoom"
                                })
                                .size(11.)
                                .color(Color32::from_gray(125)),
                            );
                        });
                    });
                });
            if self.measure || self.hovered.is_some() {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Background,
                    egui::Id::new("measurement"),
                ));
                let ppp = ctx.pixels_per_point();
                let mut highlights = Vec::new();
                if self.measure {
                    if self.mode == Mode::Edge {
                        for (i, f) in self.selected_edges.iter().enumerate() {
                            highlights.push((
                                f,
                                if i == 0 {
                                    ACCENT
                                } else {
                                    Color32::from_rgb(0, 139, 98)
                                },
                            ));
                        }
                    } else if let Some(feature) = &self.selected {
                        highlights.push((feature, ACCENT));
                    }
                }
                if let Some(hovered) = &self.hovered {
                    if !highlights.iter().any(|(f, _)| **f == *hovered) {
                        highlights.push((hovered, Color32::from_rgb(226, 135, 23)));
                    }
                }
                for (feature, color) in highlights {
                    for path in feature.paths() {
                        for pair in path.windows(2) {
                            let clipped = |p: [f64; 3]| {
                                camera.clip[..3].iter().any(|v| *v != 0.)
                                    && (0..3)
                                        .map(|i| f64::from(camera.clip[i]) * p[i])
                                        .sum::<f64>()
                                        > f64::from(camera.clip[3])
                            };
                            if clipped(pair[0]) || clipped(pair[1]) {
                                continue;
                            }
                            if let (Some(a), Some(b)) = (
                                camera.project(pair[0].map(|v| v as f32), scene, size),
                                camera.project(pair[1].map(|v| v as f32), scene, size),
                            ) {
                                painter.line_segment(
                                    [
                                        egui::pos2(a[0] / ppp, a[1] / ppp),
                                        egui::pos2(b[0] / ppp, b[1] / ppp),
                                    ],
                                    egui::Stroke::new(2.5, color),
                                );
                            }
                        }
                    }
                    if let Some(c) = feature.circle() {
                        if let Some(p) = camera.project(c.center.map(|v| v as f32), scene, size) {
                            let p = egui::pos2(p[0] / ppp, p[1] / ppp);
                            painter.line_segment(
                                [p - Vec2::new(7., 0.), p + Vec2::new(7., 0.)],
                                egui::Stroke::new(1.5, color),
                            );
                            painter.line_segment(
                                [p - Vec2::new(0., 7.), p + Vec2::new(0., 7.)],
                                egui::Stroke::new(1.5, color),
                            );
                        }
                    }
                }
                if let Some(pair) = self
                    .pair
                    .as_ref()
                    .filter(|_| self.measure && self.mode == Mode::Edge)
                {
                    if let (Some(a), Some(b)) = (
                        camera.project(pair.a.map(|v| v as f32), scene, size),
                        camera.project(pair.b.map(|v| v as f32), scene, size),
                    ) {
                        let a = egui::pos2(a[0] / ppp, a[1] / ppp);
                        let b = egui::pos2(b[0] / ppp, b[1] / ppp);
                        painter.line_segment(
                            [a, b],
                            egui::Stroke::new(1.5, Color32::from_rgb(110, 85, 155)),
                        );
                        for p in [a, b] {
                            painter.circle_filled(p, 3., Color32::WHITE);
                            painter.circle_stroke(
                                p,
                                3.,
                                egui::Stroke::new(1.5, Color32::from_rgb(110, 85, 155)),
                            );
                        }
                    }
                }
                let points: Vec<_> = self
                    .points
                    .iter()
                    .filter(|_| self.measure && matches!(self.mode, Mode::Distance | Mode::Angle))
                    .map(|&p| {
                        camera
                            .project(p, scene, size)
                            .map(|p| egui::pos2(p[0] / ppp, p[1] / ppp))
                    })
                    .collect();
                for pair in points.windows(2) {
                    if let [Some(a), Some(b)] = pair {
                        painter.line_segment([*a, *b], egui::Stroke::new(2., ACCENT));
                    }
                }
                for (i, p) in points.iter().enumerate() {
                    if let Some(p) = p {
                        painter.circle_filled(*p, 5., Color32::WHITE);
                        painter.circle_stroke(*p, 5., egui::Stroke::new(2., ACCENT));
                        painter.text(
                            *p + Vec2::new(10., -10.),
                            egui::Align2::LEFT_CENTER,
                            ["A", "B", "C"][i],
                            egui::FontId::proportional(13.),
                            ACCENT,
                        );
                    }
                }
            }
        }
        if self.help {
            egui::Window::new("Controls").fade_in(false).fade_out(false).resizable(false).collapsible(false).open(&mut self.help).show(ctx,|ui|{ui.label("Middle drag — rotate\nCtrl + middle drag — pan\nShift + middle drag — zoom\nAlt + middle drag — roll\nScroll — zoom\nF — fit  ·  R — reset\nM — measure  ·  S — section\nEsc — leave tools");});
        }
        if self.info {
            egui::Window::new("Model info")
                .fade_in(false)
                .fade_out(false)
                .resizable(false)
                .default_width(350.)
                .open(&mut self.info)
                .show(ctx, |ui| {
                    ui.label(RichText::new(name).strong());
                    if let Some(scene) = scene {
                        if let Some([w, h]) = scene.raster_size {
                            ui.label(format!("Saved image · {w} × {h} px"));
                        } else {
                            ui.label(format!(
                                "{} triangles  ·  {} segments  ·  {} points",
                                scene.triangles, scene.lines, scene.points
                            ));
                            ui.label(if scene.meters_per_unit.is_some() {
                                "Source units recognized"
                            } else {
                                "Source units unspecified"
                            });
                        }
                        if !scene.diagnostics.is_empty() {
                            ui.separator();
                            egui::ScrollArea::vertical()
                                .max_height(240.)
                                .show(ui, |ui| {
                                    for detail in &scene.diagnostics {
                                        ui.label(detail);
                                        ui.add_space(5.);
                                    }
                                });
                        }
                    }
                });
        }
        changed
    }
}
pub struct Frame {
    pub output: egui::FullOutput,
    pub meshes: Vec<egui::ClippedPrimitive>,
    pub changed: bool,
}
pub struct Ui {
    pub ctx: egui::Context,
    pub state: Option<egui_winit::State>,
    pub tools: Tools,
    pub update_state: super::updates::State,
    pub update_check: bool,
    pub update_restart: bool,
    painter: Painter,
}
impl Ui {
    pub fn headless(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let ctx = egui::Context::default();
        let mut style = egui::Style::default();
        style.visuals = egui::Visuals::light();
        style.visuals.panel_fill = Color32::WHITE;
        style.visuals.window_fill = Color32::WHITE;
        style.visuals.override_text_color = Some(INK);
        style.visuals.selection.bg_fill = Color32::from_rgb(224, 243, 249);
        style.visuals.selection.stroke = egui::Stroke::new(1., ACCENT);
        style.visuals.window_shadow = egui::epaint::Shadow::NONE;
        style.visuals.window_corner_radius = egui::CornerRadius::same(9);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(246, 248, 250);
        style.visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1., Color32::from_gray(227));
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(235, 245, 249);
        style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(235, 245, 249);
        style.visuals.widgets.hovered.bg_stroke =
            egui::Stroke::new(1., Color32::from_rgb(186, 216, 228));
        style.animation_time = 0.;
        style.spacing.button_padding = Vec2::new(10., 6.);
        style.spacing.item_spacing = Vec2::new(8., 8.);
        ctx.set_style(style);
        Self {
            ctx,
            state: None,
            tools: Tools::default(),
            update_state: super::updates::State::default(),
            update_check: false,
            update_restart: false,
            painter: Painter::new(device, format),
        }
    }
    pub fn new(window: &Window, device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let mut ui = Self::headless(device, format);
        ui.state = Some(egui_winit::State::new(
            ui.ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            Some(winit::window::Theme::Light),
            Some(device.limits().max_texture_dimension_2d as usize),
        ));
        ui.state.as_mut().unwrap().egui_input_mut().focused = window.has_focus();
        ui
    }
    pub fn event(&mut self, window: &Window, event: &WindowEvent) -> egui_winit::EventResponse {
        self.state.as_mut().unwrap().on_window_event(window, event)
    }
    pub fn begin(
        &mut self,
        window: Option<&Window>,
        scene: Option<&Scene>,
        camera: &mut Camera,
        size: [u32; 2],
        name: &str,
    ) -> Frame {
        if scene.is_some_and(|s| s.raster_size.is_some()) {
            self.tools.measure = false; self.tools.section = false; self.tools.clear();
        }
        let raw = if let (Some(state), Some(window)) = (&mut self.state, window) {
            state.take_egui_input(window)
        } else {
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0] as f32, size[1] as f32),
                )),
                ..Default::default()
            }
        };
        let ctx = self.ctx.clone();
        let mut changed = false;
        let output = ctx.run(raw, |ctx| {
            changed |= self.tools.show(ctx, scene, camera, size, name);
            if self.tools.updates_open {
                let mut updates_open = true;
                egui::Window::new("Updates")
                    .open(&mut updates_open)
                    .frame(tool_frame())
                    .resizable(false)
                    .collapsible(false)
                    .default_width(300.)
                    .show(ctx, |ui| {
                        ui.label(
                            RichText::new(format!("VTX {}", self.update_state.version))
                                .strong()
                                .size(18.),
                        );
                        ui.add_space(6.);
                        ui.label(&self.update_state.message);
                        ui.add_space(8.);
                        if ui
                            .add_enabled(!self.update_state.busy, egui::Button::new("Check now"))
                            .clicked()
                        {
                            self.update_check = true;
                        }
                        if self.update_state.ready.is_some()
                            && ui.button("Restart to update").clicked()
                        {
                            self.update_restart = true;
                        }
                        ui.add_space(6.);
                        ui.label(
                            RichText::new("Updates install when the last VTX window closes.")
                                .size(11.)
                                .color(Color32::from_gray(120)),
                        );
                    });
                self.tools.updates_open = updates_open;
            }
        });
        let meshes = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
        Frame {
            output,
            meshes,
            changed,
        }
    }
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: [u32; 2],
        frame: &Frame,
    ) {
        self.painter.paint(
            device,
            queue,
            encoder,
            target,
            size,
            frame.output.pixels_per_point,
            &frame.output,
            &frame.meshes,
        );
    }
    pub fn finish(&mut self, window: &Window, frame: Frame) {
        if let Some(state) = &mut self.state {
            state.handle_platform_output(window, frame.output.platform_output);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_redraw_event_does_not_schedule_another_redraw() {
        assert!(!schedule_repaint(&WindowEvent::RedrawRequested, true));
        assert!(schedule_repaint(
            &WindowEvent::Resized(winit::dpi::PhysicalSize::new(800, 600)),
            true
        ));
        assert!(!schedule_repaint(
            &WindowEvent::Resized(winit::dpi::PhysicalSize::new(800, 600)),
            false
        ));
    }
    fn sample_scene(units: Option<f64>) -> Scene {
        Scene {
            raster_size: None,
            curves: Vec::new(),
            meters_per_unit: units,
            bounds: [[0.; 3], [100.; 3]],
            parts: Vec::new(),
            center: [50.; 3],
            diameter: 173.205,
            triangles: 12,
            points: 0,
            lines: 0,
            plan_view: false,
            issues: 0,
            notice: None,
            retain_saved_preview: false,
            diagnostics: Vec::new(),
            upload_ms: 0.,
            gpu_wait_ms: 0.,
        }
    }
    #[test]
    fn diameter_radius_units_and_stale_queries() {
        let scene = sample_scene(Some(0.001));
        let circle = super::super::features::Circle {
            center: [50., 50., 50.],
            u: [1., 0., 0.],
            v: [0., 1., 0.],
            radius: 12.7,
            start: 0.,
            sweep: std::f64::consts::TAU,
            deviation: 0.,
            samples: 64,
            exact: false,
            step: 0.1,
        };
        let mut tools = Tools {
            measure: true,
            mode: Mode::Diameter,
            selected: Some(Feature::Circle(circle)),
            ..Default::default()
        };
        assert_eq!(tools.result(&scene).as_deref(), Some("Ø 25.400 mm"));
        tools.inches = true;
        assert_eq!(tools.result(&scene).as_deref(), Some("Ø 1.000 in"));
        let context = PickContext {
            camera: Camera::default(),
            size: [1000, 800],
            at: [500, 400],
            serial: tools.serial,
            additive: false,
        };
        tools.indexing = true;
        tools.picked_at(Some([50., 50., 50.]), &context, &scene);
        tools.clear();
        tools.set_index(Index::default(), &scene);
        assert!(tools.selected.is_none());
        assert!(!tools.pending);
        tools.picked_at(Some([50., 50., 50.]), &context, &scene);
        assert!(tools.selected.is_none());
    }
    #[test]
    fn feature_selection_and_center_snapping_respect_visibility_and_section() {
        let scene = sample_scene(Some(0.001));
        let mut camera = Camera::default();
        camera.rotation = glm::quat_identity();
        let circle = super::super::features::Circle {
            center: [50., 50., 50.],
            u: [1., 0., 0.],
            v: [0., 1., 0.],
            radius: 12.7,
            start: 0.,
            sweep: std::f64::consts::TAU,
            deviation: 0.,
            samples: 64,
            exact: false,
            step: 0.1,
        };
        let index = Index {
            features: vec![
                Feature::Circle(circle.clone()),
                Feature::Cylinder {
                    rim: circle,
                    height: 20.,
                },
                Feature::Line {
                    a: [25., 25., 50.],
                    b: [75., 25., 50.],
                },
            ],
            elapsed_ms: 0.,
        };
        let at = |p| {
            let p = camera.project(p, &scene, [1000, 800]).unwrap();
            [p[0] as u32, p[1] as u32]
        };
        let rim = [62.7, 50., 50.];
        assert!(index
            .select(&camera, &scene, [1000, 800], at(rim), Some(rim), true)
            .is_some());
        let wall = [62.7, 50., 60.];
        assert!(matches!(
            index.select(&camera, &scene, [1000, 800], at(wall), Some(wall), true),
            Some(Feature::Cylinder { .. })
        ));
        assert!(index
            .select(
                &camera,
                &scene,
                [1000, 800],
                at(rim),
                Some([62.7, 50., 80.]),
                true
            )
            .is_none());
        assert_eq!(
            index
                .snap(&camera, &scene, [1000, 800], at([50.; 3]), None)
                .unwrap(),
            ([50., 50., 70.], "Center")
        );
        assert!(index
            .snap(
                &camera,
                &scene,
                [1000, 800],
                at([50.; 3]),
                Some([50., 50., 80.])
            )
            .is_none());
        camera.clip = [0., 0., 1., 40.];
        let p = camera.project(rim, &scene, [1000, 800]).unwrap();
        assert!(index
            .select(
                &camera,
                &scene,
                [1000, 800],
                [p[0] as u32, p[1] as u32],
                Some(rim),
                true
            )
            .is_none());
    }
    #[test]
    fn measurement_units_are_explicit_and_convert_mm_to_inches() {
        let scene = sample_scene(Some(0.001));
        let mut tools = Tools {
            measure: true,
            mode: Mode::Distance,
            points: vec![[0.; 3], [25.4, 0., 0.]],
            ..Default::default()
        };
        assert_eq!(tools.result(&scene).as_deref(), Some("25.400 mm"));
        tools.inches = true;
        assert_eq!(tools.result(&scene).as_deref(), Some("1.000 in"));
        let unknown = sample_scene(None);
        assert_eq!(tools.result(&unknown).as_deref(), Some("25.400 units"));
        tools.source_units = Some(0.001);
        assert_eq!(tools.result(&unknown).as_deref(), Some("1.000 in"));
    }
    #[test]
    fn saved_images_cannot_produce_model_measurements() {
        let mut scene = sample_scene(Some(0.001));
        scene.raster_size = Some([640, 480]);
        let tools = Tools {
            measure: true,
            mode: Mode::Distance,
            points: vec![[0.; 3], [25.4, 0., 0.]],
            ..Default::default()
        };
        assert!(tools.result(&scene).is_none());
    }
    #[test]
    fn toolbar_click_opens_measurement_and_section_plane_tracks_bounds() {
        let ctx = egui::Context::default();
        let scene = sample_scene(Some(0.001));
        let mut camera = Camera::default();
        let mut tools = Tools::default();
        let mut run = |events: Vec<egui::Event>| {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280., 832.),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                tools.show(ctx, Some(&scene), &mut camera, [1280, 832], "test.sldprt");
            });
        };
        run(vec![]);
        run(vec![]);
        let pos = egui::pos2(150., 24.);
        run(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        run(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        drop(run);
        assert!(
            tools.measure,
            "the actual toolbar should open the measure panel"
        );
        tools.section = true;
        tools.axis = 2;
        tools.position = 0.25;
        let _ = ctx.run(Default::default(), |ctx| {
            tools.show(ctx, Some(&scene), &mut camera, [1280, 832], "test");
        });
        assert_eq!(camera.clip, [0., 0., 1., 25.]);
        tools.flip = true;
        let _ = ctx.run(Default::default(), |ctx| {
            tools.show(ctx, Some(&scene), &mut camera, [1280, 832], "test");
        });
        assert_eq!(camera.clip, [0., 0., -1., -25.]);
    }
    #[test]
    fn distances_and_degenerate_angles() {
        assert!((distance([0.; 3], [3., 4., 0.]) - 5.).abs() < 1e-12);
        assert!((angle([1., 0., 0.], [0.; 3], [0., 1., 0.]).unwrap() - 90.).abs() < 1e-12);
        assert!(angle([0.; 3], [0.; 3], [1.; 3]).is_none());
    }
    #[test]
    fn edge_clicks_replace_ctrl_adds_and_ctrl_deselects() {
        let a = Feature::Line {
            a: [0.; 3],
            b: [10., 0., 0.],
        };
        let b = Feature::Line {
            a: [0., 5., 0.],
            b: [10., 5., 0.],
        };
        let c = Feature::Line {
            a: [0., 7., 0.],
            b: [10., 7., 0.],
        };
        let mut t = Tools::default();
        assert_eq!(t.mode as u8, Mode::Edge as u8);
        t.select_edge(Some(a.clone()), false);
        assert!(t.measure);
        assert_eq!(t.selected_edges.len(), 1);
        t.select_edge(Some(b.clone()), true);
        assert_eq!(t.selected_edges.len(), 2);
        assert!((t.pair.as_ref().unwrap().distance - 5.).abs() < 1e-12);
        assert_eq!(t.pair.as_ref().unwrap().angle, Some(0.));
        assert_eq!(
            t.result(&sample_scene(Some(0.001))).as_deref(),
            Some("5.000 mm")
        );
        t.select_edge(Some(b), true);
        assert_eq!(t.selected_edges, vec![a.clone()]);
        assert!(t.pair.is_none());
        t.select_edge(Some(c.clone()), false);
        assert_eq!(t.selected_edges, vec![c]);
        t.select_edge(None, true);
        assert_eq!(t.selected_edges.len(), 1);
        t.select_edge(None, false);
        assert!(t.selected_edges.is_empty());
        assert!(t.selected.is_none());
    }
    #[test]
    fn screen_cache_finds_edges_and_rejects_occlusion_and_stale_views() {
        let scene = sample_scene(Some(0.001));
        let mut camera = Camera::default();
        camera.rotation = glm::quat_identity();
        let line = Feature::Line {
            a: [25., 50., 50.],
            b: [75., 50., 50.],
        };
        let index = Index {
            features: vec![line],
            elapsed_ms: 0.,
        };
        let size = [1000, 800];
        let cache = edges::ScreenEdges::new(&index, &camera, &scene, size);
        let p = camera.project([50.; 3], &scene, size).unwrap();
        let c = cache
            .nearest(
                &index,
                [p[0] as u32, p[1] as u32],
                10.,
                false,
                &scene,
                &camera,
            )
            .unwrap();
        assert_eq!(c.id, 0);
        assert!(edges::visible(&c, Some([50.; 3]), &camera, &scene));
        assert!(!edges::visible(&c, Some([50., 50., 80.]), &camera, &scene));
        assert!(cache.matches(&camera, size));
        camera.zoom *= 2.;
        assert!(!cache.matches(&camera, size));
        assert!(!cache.matches(&Camera::default(), [1200, 800]));
        camera.rotation = glm::quat_identity();
        camera.clip = [1., 0., 0., 40.];
        let clipped = edges::ScreenEdges::new(&index, &camera, &scene, size);
        let p = camera.project([60., 50., 50.], &scene, size).unwrap();
        assert!(clipped
            .nearest(
                &index,
                [p[0] as u32, p[1] as u32],
                10.,
                false,
                &scene,
                &camera
            )
            .is_none());
    }
    #[test]
    fn a_new_measurement_replaces_the_previous_pair() {
        let mut t = Tools {
            measure: true,
            mode: Mode::Distance,
            ..Default::default()
        };
        for i in 0..3 {
            t.picked(Some([i as f32, 0., 0.]));
        }
        assert_eq!(t.points, vec![[2., 0., 0.]]);
        t.picked(None);
        assert_eq!(t.points.len(), 1);
        assert!(!t.message.is_empty());
    }
}
