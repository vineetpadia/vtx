use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use egui::{Color32, RichText};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{camera::Camera, model::Scene, Result};

const MAX_NOTES: usize = 128;
const MAX_TEXT: usize = 2048;
const MAX_FILE: u64 = 2 * 1024 * 1024;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub fn stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct View {
    rotation: [f32; 4],
    zoom: f32,
    pan: [f32; 3],
    orthographic: bool,
    explode: f32,
    #[serde(default)]
    clearance_layout: bool,
    section: Option<Section>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct Section {
    axis: usize,
    position: f32,
    flip: bool,
}

impl View {
    fn capture(camera: &Camera, scene: &Scene) -> Self {
        let section = camera.clip[..3]
            .iter()
            .position(|v| v.abs() > 0.5)
            .map(|axis| {
                let sign = camera.clip[axis];
                let bounds = scene.exploded_bounds(camera.explode, camera.legacy_explode);
                let span = bounds[1][axis] - bounds[0][axis];
                let position = if span > 0. {
                    ((camera.clip[3] / sign - bounds[0][axis]) / span).clamp(0., 1.)
                } else {
                    0.5
                };
                Section {
                    axis,
                    position,
                    flip: sign < 0.,
                }
            });
        Self {
            rotation: camera.rotation.coords.into(),
            zoom: camera.zoom,
            pan: camera.pan,
            orthographic: camera.orthographic,
            explode: camera.explode,
            clearance_layout: !camera.legacy_explode,
            section,
        }
    }

    fn valid(&self, scene: &Scene) -> bool {
        self.rotation.iter().all(|v| v.is_finite())
            && (0.99..=1.01).contains(&self.rotation.iter().map(|v| v * v).sum::<f32>())
            && (0.05..=30.).contains(&self.zoom)
            && self.pan.iter().all(|v| v.is_finite())
            && (0. ..=1.).contains(&self.explode)
            && (self.explode == 0. || scene.explosion.is_some())
            && self
                .section
                .is_none_or(|s| s.axis < 3 && (0. ..=1.).contains(&s.position))
    }

    pub fn restore(self, camera: &mut Camera) {
        let [x, y, z, w] = self.rotation;
        camera.rotation = nalgebra_glm::quat_normalize(&nalgebra_glm::quat(x, y, z, w));
        camera.zoom = self.zoom;
        camera.pan = self.pan;
        camera.orthographic = self.orthographic;
        camera.explode = self.explode;
        camera.legacy_explode = self.explode > 0. && !self.clearance_layout;
    }

    pub fn section(self) -> (bool, usize, f32, bool) {
        self.section.map_or((false, 0, 0.5, false), |s| {
            (true, s.axis, s.position, s.flip)
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Note {
    anchor: [f32; 3],
    view: View,
    text: String,
}

#[derive(Serialize)]
struct MarkupFile<'a> {
    version: u32,
    model_sha256: &'a str,
    notes: &'a [Note],
}

#[derive(Deserialize)]
struct LoadedMarkup {
    version: u32,
    model_sha256: String,
    notes: Vec<Note>,
}

pub enum Action {
    Save,
    Open,
}

#[derive(Default)]
pub struct Markup {
    pub open: bool,
    pub placing: bool,
    pub action: Option<Action>,
    pub message: String,
    notes: Vec<Note>,
    selected: Option<usize>,
    draft: Option<Note>,
    dirty: bool,
    digest: Option<String>,
    model: Option<(PathBuf, u64, std::time::SystemTime)>,
}

impl Markup {
    pub fn set_model(&mut self, path: PathBuf, stamp: Option<(u64, std::time::SystemTime)>) {
        let path = path.canonicalize().unwrap_or(path);
        self.model = stamp.map(|(size, modified)| (path, size, modified));
        self.digest = None;
    }

    pub fn model_path(&self) -> Option<&Path> {
        self.model.as_ref().map(|(path, _, _)| path.as_path())
    }

    pub fn unsaved(&self) -> bool {
        self.dirty
            || self
                .draft
                .as_ref()
                .is_some_and(|n| !n.text.trim().is_empty())
    }

    pub fn picked(&mut self, point: Option<[f32; 3]>, camera: &Camera, scene: &Scene) {
        if !self.placing {
            return;
        }
        if let Some(anchor) = point.filter(|p| p.iter().all(|v| v.is_finite())) {
            self.draft = Some(Note {
                anchor,
                view: View::capture(camera, scene),
                text: String::new(),
            });
            self.selected = None;
            self.placing = false;
            self.message.clear();
        } else {
            self.message = "Click a visible surface to place the note.".into();
        }
    }

    fn add_draft(&mut self) -> Option<View> {
        let mut note = self.draft.take()?;
        note.text = note.text.trim().to_string();
        let view = note.view;
        self.selected = Some(self.notes.len());
        self.notes.push(note);
        self.dirty = true;
        Some(view)
    }

    pub(super) fn verify_round_trip(
        &mut self,
        model: &Path,
        target: &Path,
        point: [f32; 3],
        camera: &Camera,
        scene: &Scene,
    ) -> Result<()> {
        self.set_model(model.into(), stamp(model));
        let model = self
            .model_path()
            .ok_or("Missing review model")?
            .to_path_buf();
        self.placing = true;
        self.picked(Some(point), camera, scene);
        self.draft
            .as_mut()
            .ok_or("Surface did not create a note")?
            .text = "Check this mounting surface".into();
        self.add_draft().ok_or("Could not add the review note")?;
        self.save(&model, target)?;
        self.load(&model, target, scene)?;
        let note = self.notes.first().ok_or("Saved review note did not load")?;
        let mut restored = Camera::default();
        note.view.restore(&mut restored);
        if note.anchor != point
            || restored.explode != camera.explode
            || (restored.rotation.coords - camera.rotation.coords).norm() > 1e-5
            || restored.pan != camera.pan
            || restored.zoom != camera.zoom
        {
            return Err("Saved annotation view or surface anchor changed after reopening".into());
        }
        self.open = true;
        self.selected = Some(0);
        Ok(())
    }

    fn model_digest(&mut self, model: &Path) -> Result<String> {
        self.check_model(model)?;
        if let Some(digest) = &self.digest {
            return Ok(digest.clone());
        }
        let mut file = File::open(model)?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 256 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        let digest = format!("{:x}", hash.finalize());
        self.check_model(model)?;
        self.digest = Some(digest.clone());
        Ok(digest)
    }

    fn check_model(&self, model: &Path) -> Result<()> {
        let metadata = fs::metadata(model)?;
        if !self.model.as_ref().is_some_and(|(path, size, modified)| {
            path == model && metadata.len() == *size && metadata.modified().ok() == Some(*modified)
        }) {
            return Err(
                "The model file changed on disk. Reopen it before using markup files.".into(),
            );
        }
        Ok(())
    }

    pub fn save(&mut self, model: &Path, target: &Path) -> Result<()> {
        if self
            .notes
            .iter()
            .any(|note| note.text.trim().is_empty() || note.text.len() > MAX_TEXT)
        {
            return Err("Each note must contain text and fit within 2,048 bytes.".into());
        }
        if !target.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".vtx-markup.json")
        }) {
            return Err("Use a .vtx-markup.json file for annotations.".into());
        }
        if target == model || target.exists() && target.canonicalize()? == model.canonicalize()? {
            return Err("Choose a separate markup file.".into());
        }
        let digest = self.model_digest(model)?;
        let data = serde_json::to_vec_pretty(&MarkupFile {
            version: 1,
            model_sha256: &digest,
            notes: &self.notes,
        })?;
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let temp = target.with_extension(format!("{}.{}.tmp", std::process::id(), id));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let result = (|| -> io::Result<()> {
            file.write_all(&data)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, target)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result?;
        self.dirty = false;
        Ok(())
    }

    pub fn load(&mut self, model: &Path, target: &Path, scene: &Scene) -> Result<()> {
        let mut bytes = Vec::new();
        File::open(target)?
            .take(MAX_FILE + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_FILE {
            return Err("Markup file exceeds 2 MiB.".into());
        }
        let loaded: LoadedMarkup = serde_json::from_slice(&bytes)?;
        if loaded.version != 1
            || loaded.notes.len() > MAX_NOTES
            || loaded.notes.iter().any(|note| {
                note.text.trim().is_empty()
                    || note.text.len() > MAX_TEXT
                    || !note.anchor.iter().all(|v| v.is_finite())
                    || !note.view.valid(scene)
            })
        {
            return Err("Markup file contains an unsupported note or view.".into());
        }
        if loaded.model_sha256 != self.model_digest(model)? {
            return Err("This markup belongs to a different model or revision.".into());
        }
        self.notes = loaded.notes;
        self.selected = None;
        self.draft = None;
        self.placing = false;
        self.dirty = false;
        Ok(())
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        scene: &Scene,
        camera: &Camera,
        size: [u32; 2],
        frame: egui::Frame,
    ) -> Option<View> {
        if !self.open {
            return None;
        }
        let mut restore = None;
        let mut open = self.open;
        egui::Window::new("Annotations")
            .open(&mut open)
            .frame(frame)
            .anchor(egui::Align2::RIGHT_TOP, [-18., 66.])
            .default_width(248.)
            .max_width(290.)
            .fade_in(false)
            .fade_out(false)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            self.draft.is_none() && self.notes.len() < MAX_NOTES,
                            egui::Button::new("Add note").selected(self.placing),
                        )
                        .clicked()
                    {
                        self.placing = !self.placing;
                        self.message.clear();
                    }
                    if ui.button("Open").clicked() {
                        self.action = Some(Action::Open);
                    }
                    if ui
                        .add_enabled(
                            (self.dirty || !self.notes.is_empty()) && self.draft.is_none(),
                            egui::Button::new("Save as"),
                        )
                        .clicked()
                    {
                        self.action = Some(Action::Save);
                    }
                });
                if self.placing {
                    ui.label("Click the model where the note should point.");
                }
                if let Some(draft) = &mut self.draft {
                    ui.add(
                        egui::TextEdit::multiline(&mut draft.text)
                            .hint_text("Write a note")
                            .desired_rows(3)
                            .desired_width(240.)
                            .char_limit(MAX_TEXT / 4),
                    );
                    let mut save = false;
                    let mut cancel = false;
                    ui.horizontal(|ui| {
                        save = ui
                            .add_enabled(!draft.text.trim().is_empty(), egui::Button::new("Add"))
                            .clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                    if save {
                        restore = self.add_draft();
                    } else if cancel {
                        self.draft = None;
                    }
                }
                if !self.notes.is_empty() {
                    ui.separator();
                }
                let mut delete = None;
                egui::ScrollArea::vertical()
                    .max_height(200.)
                    .show(ui, |ui| {
                        for (index, note) in self.notes.iter_mut().enumerate() {
                            let label: String = note
                                .text
                                .lines()
                                .next()
                                .unwrap_or("Note")
                                .chars()
                                .take(38)
                                .collect();
                            if ui
                                .selectable_label(self.selected == Some(index), label)
                                .clicked()
                            {
                                self.selected = Some(index);
                                self.placing = false;
                                restore = Some(note.view);
                            }
                            if self.selected == Some(index) {
                                self.dirty |= ui
                                    .add(
                                        egui::TextEdit::multiline(&mut note.text)
                                            .desired_rows(2)
                                            .desired_width(240.)
                                            .char_limit(MAX_TEXT / 4),
                                    )
                                    .changed();
                                ui.horizontal(|ui| {
                                    if ui.small_button("Restore view").clicked() {
                                        restore = Some(note.view);
                                    }
                                    if ui.small_button("Delete").clicked() {
                                        delete = Some(index);
                                    }
                                });
                            }
                        }
                    });
                if let Some(index) = delete {
                    self.notes.remove(index);
                    self.selected = None;
                    self.dirty = true;
                }
                if !self.message.is_empty() {
                    ui.label(RichText::new(&self.message).size(11.));
                }
            });
        self.open = open;
        if !open {
            self.placing = false;
            return restore;
        }
        if let Some(note) = self
            .draft
            .as_ref()
            .or_else(|| self.selected.and_then(|i| self.notes.get(i)))
        {
            let mut view = camera.clone();
            if let Some(saved) = restore {
                saved.restore(&mut view);
            }
            if (view.explode - note.view.explode).abs() < 1e-5
                && (view.explode == 0. || view.legacy_explode != note.view.clearance_layout)
            {
                paint(ctx, scene, &view, size, note);
            }
        }
        restore
    }
}

fn paint(ctx: &egui::Context, scene: &Scene, camera: &Camera, size: [u32; 2], note: &Note) {
    let Some(pixel) = camera.project(note.anchor, scene, size) else {
        return;
    };
    let scale = ctx.pixels_per_point();
    let anchor = egui::pos2(pixel[0] / scale, pixel[1] / scale);
    let viewport = ctx.screen_rect().shrink(8.);
    if !viewport.contains(anchor) {
        return;
    }
    let ink = Color32::from_rgb(0, 107, 141);
    let painter = ctx
        .layer_painter(egui::LayerId::background())
        .with_clip_rect(viewport);
    let label = if note.text.trim().is_empty() {
        "New note"
    } else {
        &note.text
    };
    let text = painter.layout(label.into(), egui::FontId::proportional(13.), ink, 210.);
    let label_size = text.size() + egui::vec2(16., 10.);
    let mut origin = anchor + egui::vec2(26., -32.);
    if origin.x + label_size.x > viewport.right() {
        origin.x = anchor.x - label_size.x - 26.;
    }
    origin.x = origin.x.clamp(
        viewport.left(),
        (viewport.right() - label_size.x).max(viewport.left()),
    );
    origin.y = origin.y.clamp(
        viewport.top() + 48.,
        (viewport.bottom() - label_size.y).max(viewport.top() + 48.),
    );
    let rect = egui::Rect::from_min_size(origin, label_size);
    let target = rect.clamp(anchor);
    painter.line_segment([anchor, target], egui::Stroke::new(1.5, ink));
    painter.circle_filled(anchor, 3.5, ink);
    painter.rect_filled(rect, 5., Color32::WHITE);
    painter.rect_stroke(
        rect,
        5.,
        egui::Stroke::new(1., ink),
        egui::StrokeKind::Inside,
    );
    painter.galley(origin + egui::vec2(8., 5.), text, ink);
}
