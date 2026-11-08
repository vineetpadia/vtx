use std::{
    cell::OnceCell,
    collections::HashMap,
    convert::{TryFrom, TryInto},
    fs,
    path::Path,
    sync::mpsc::{self, Receiver, SyncSender},
    time::Instant,
};

use step::step_file::StepFile;
use wgpu::util::DeviceExt;

use super::Result;
use crate::{dxf_drawing, fbx_mesh, gltf_mesh, ifc_mesh, obj, ply, solidworks, three_mf};

type Point = [f32; 3];
type Triangle = [u32; 3];

pub const FORMATS: &[&str] = &[
    "stl", "obj", "ply", "glb", "gltf", "fbx", "step", "stp", "p21", "3mf", "sldprt", "sldasm",
    "ifc", "ifczip", "igs", "iges", "dxf", "x_t", "x_b", "xt", "xb", "xmt_txt", "xmt_bin", "brep", "f3d", "slddrw",
];

#[derive(Clone)]
pub struct Uploader {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub layout: wgpu::BindGroupLayout,
    pub empty: wgpu::Buffer,
    pub white: Texture,
}

pub struct UploadTarget {
    ready: OnceCell<(Uploader, f64)>,
    pending: Option<Receiver<Uploader>>,
}

impl UploadTarget {
    pub fn ready(uploader: Uploader) -> Self {
        Self {
            ready: OnceCell::from((uploader, 0.0)),
            pending: None,
        }
    }

    pub fn deferred() -> (SyncSender<Uploader>, Self) {
        let (sender, receiver) = mpsc::sync_channel(1);
        (
            sender,
            Self {
                ready: OnceCell::new(),
                pending: Some(receiver),
            },
        )
    }

    fn get(&self) -> Result<&Uploader> {
        if let Some((uploader, _)) = self.ready.get() {
            return Ok(uploader);
        }
        let start = Instant::now();
        let uploader = self
            .pending
            .as_ref()
            .ok_or("Missing GPU upload target")?
            .recv()
            .map_err(|_| "GPU initialization ended before upload")?;
        Ok(&self
            .ready
            .get_or_init(|| (uploader, start.elapsed().as_secs_f64() * 1000.0))
            .0)
    }

    fn wait_ms(&self) -> f64 {
        self.ready.get().map_or(0.0, |(_, wait)| *wait)
    }
}

#[derive(Clone)]
pub struct Texture {
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

pub struct Part {
    pub source_positions: Option<wgpu::Buffer>,
    pub transparent: bool,
    pub positions: wgpu::Buffer,
    pub indices: Option<wgpu::Buffer>,
    pub material: wgpu::BindGroup,
    pub count: u32,
    pub points: bool,
    pub lines: bool,
    pub obj_corners: bool,
    pub instances: Option<Instances>,
}

pub struct Instances {
    pub transforms: wgpu::Buffer,
    pub count: u32,
    pub positive_count: u32,
    pub mirrored_indices: Option<wgpu::Buffer>,
}

pub struct Scene {
    pub raster_size: Option<[u32;2]>,
    pub curves: Vec<dxf_drawing::Curve>,
    pub meters_per_unit: Option<f64>,
    pub bounds: [Point; 2],
    pub parts: Vec<Part>,
    pub center: [f32; 3],
    pub diameter: f32,
    pub triangles: usize,
    pub points: usize,
    pub lines: usize,
    pub plan_view: bool,
    pub issues: usize,
    pub notice: Option<&'static str>,
    pub retain_saved_preview: bool,
    pub diagnostics: Vec<String>,
    pub upload_ms: f64,
    pub gpu_wait_ms: f64,
}

#[derive(Default)]
struct Style<'a> {
    unlit: bool,
    transparent: bool,
    material: Option<&'a gltf_mesh::Appearance>,
    texture: Option<&'a Texture>,
    texture_uv: Option<&'a [[f32; 2]]>,
    encoded_texture: bool,
    decal: bool,
    colors: Option<&'a [[u8; 4]]>,
    color: Option<[u8; 4]>,
    linear_color: Option<[f32; 4]>,
    instances: &'a [(three_mf::Instance, bool)],
}

struct Builder<'a> {
    uploader: &'a UploadTarget,
    scene: Scene,
    min: Point,
    max: Point,
}

fn point_bounds(points: &[Point]) -> Result<(Point, Point)> {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for point in points {
        for (axis, &value) in point.iter().enumerate() {
            if !value.is_finite() {
                return Err("Invalid model position".into());
            }
            min[axis] = min[axis].min(value);
            max[axis] = max[axis].max(value);
        }
    }
    Ok((min, max))
}

impl Uploader {
    fn buffer(&self, bytes: &[u8], usage: wgpu::BufferUsages) -> Result<wgpu::Buffer> {
        self.buffer_with(bytes.len(), usage, |mapped| mapped.copy_from_slice(bytes))
    }

    fn bind_material(
        &self,
        style: &Style,
        attributes: [&wgpu::Buffer; 2],
        has_vertex_colors: bool,
        has_uv: bool,
        transform: [f32; 4],
    ) -> Result<wgpu::BindGroup> {
        let texture = style.texture.filter(|_| has_uv).unwrap_or(&self.white);
        let mut uniform = [
            0.33f32, 0.75, 0.95, 1.0, 0.0, 0.0, 0.0, -1.0, 1.0, 1.0, 0.0, 0.0,
        ];
        if let Some(color) = style.color {
            uniform[..4].copy_from_slice(&color.map(|value| f32::from(value) / 255.0));
        }
        if let Some(color) = style.linear_color {
            uniform[..4].copy_from_slice(&color);
        }
        if let Some(material) = style.material {
            uniform[..4].copy_from_slice(&material.color);
            uniform[7] = material.cutoff.unwrap_or(-1.0);
        }
        uniform[4] = if style.unlit { 2. } else { u8::from(style.material.is_some() || style.linear_color.is_some()) as f32 };
        uniform[5] = u8::from(has_vertex_colors) as f32;
        if has_uv {
            uniform[6] = if style.decal {
                3.0
            } else if style.encoded_texture {
                2.0
            } else {
                1.0
            };
        }
        uniform[8..].copy_from_slice(&transform);
        let uniform = self.buffer(bytemuck::cast_slice(&uniform), wgpu::BufferUsages::UNIFORM)?;
        Ok(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("VTX part material"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: attributes[0].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: attributes[1].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&texture.sampler),
                },
            ],
        }))
    }

    fn buffer_with(
        &self,
        len: usize,
        usage: wgpu::BufferUsages,
        write: impl FnOnce(&mut [u8]),
    ) -> Result<wgpu::Buffer> {
        let usage = if usage.intersects(
            wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::INDEX | wgpu::BufferUsages::STORAGE,
        ) {
            usage | wgpu::BufferUsages::COPY_SRC
        } else {
            usage
        };
        let limits = self.device.limits();
        let size = (len as u64).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        if len == 0
            || size > limits.max_buffer_size
            || (usage.contains(wgpu::BufferUsages::STORAGE)
                && size > u64::from(limits.max_storage_buffer_binding_size))
        {
            return Err("Model buffer exceeds the GPU limit".into());
        }
        // Queue staging avoids zeroing the full allocation before we fill it.
        // Keep the mapped-at-creation path for small buffers.
        if size >= 1024 * 1024 {
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("VTX mesh data"),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut staging = self
                .queue
                .write_buffer_with(&buffer, 0, wgpu::BufferSize::new(size).unwrap())
                .ok_or("Unable to stage model data for the GPU")?;
            let (data, padding) = staging.as_mut().split_at_mut(len);
            write(data);
            padding.fill(0);
            drop(staging);
            return Ok(buffer);
        }
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("VTX mesh data"),
            size,
            usage,
            mapped_at_creation: true,
        });
        write(&mut buffer.slice(..).get_mapped_range_mut()[..len]);
        buffer.unmap();
        Ok(buffer)
    }

    fn index_buffer<'a>(
        &self,
        count: usize,
        batches: impl IntoIterator<Item = &'a [Triangle]>,
        mirrored: bool,
    ) -> Result<wgpu::Buffer> {
        let size = count * std::mem::size_of::<Triangle>();
        self.buffer_with(size, wgpu::BufferUsages::INDEX, |mut mapped| {
            for triangles in batches {
                let bytes = bytemuck::cast_slice(triangles);
                let (target, remaining) = mapped.split_at_mut(bytes.len());
                if mirrored {
                    for (output, &[a, b, c]) in target.chunks_exact_mut(12).zip(triangles) {
                        output.copy_from_slice(bytemuck::bytes_of(&[a, c, b]));
                    }
                } else {
                    target.copy_from_slice(bytes);
                }
                mapped = remaining;
            }
            debug_assert!(mapped.is_empty());
        })
    }

    fn upload_image(
        &self,
        rgba: &image::RgbaImage,
        format: wgpu::TextureFormat,
    ) -> wgpu::TextureView {
        let texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("VTX base color"),
                size: wgpu::Extent3d {
                    width: rgba.width(),
                    height: rgba.height(),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba,
        );
        texture.create_view(&Default::default())
    }

    fn fbx_sampler(&self, asset: &ufbx::Texture) -> wgpu::Sampler {
        let address = |wrap| match wrap {
            ufbx::WrapMode::Repeat => wgpu::AddressMode::Repeat,
            ufbx::WrapMode::Clamp => wgpu::AddressMode::ClampToEdge,
        };
        self.linear_sampler(address(asset.wrap_u), address(asset.wrap_v))
    }

    fn linear_sampler(&self, u: wgpu::AddressMode, v: wgpu::AddressMode) -> wgpu::Sampler {
        self.device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: u,
            address_mode_v: v,
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        })
    }
}

impl UploadTarget {
    fn decode_image(&self, bytes: &[u8], format: image::ImageFormat) -> Result<image::RgbaImage> {
        use image::{ImageDecoder, ImageReader, Limits};
        let mut reader = ImageReader::with_format(std::io::Cursor::new(bytes), format);
        let mut limits = Limits::default();
        limits.max_alloc = Some(256 * 1024 * 1024);
        if let Some((gpu, _)) = self.ready.get() {
            let max = gpu.device.limits().max_texture_dimension_2d;
            limits.max_image_width = Some(max);
            limits.max_image_height = Some(max);
        }
        reader.limits(limits.clone());
        let mut decoder = reader.into_decoder()?;
        let (width, height) = decoder.dimensions();
        // Decode one image while GPU setup runs, then apply the device's limits.
        let decoded: image::ImageResult<_> = (|| {
            let mut remaining = limits.clone();
            remaining.reserve(decoder.total_bytes())?;
            decoder.set_limits(remaining)?;
            Ok(image::DynamicImage::from_decoder(decoder)?.into_rgba8())
        })();
        let max = self.get()?.device.limits().max_texture_dimension_2d;
        limits.max_image_width = Some(max);
        limits.max_image_height = Some(max);
        limits.check_dimensions(width, height)?;
        Ok(decoded?)
    }

    fn load_texture(&self, texture: &gltf_mesh::Texture) -> Result<Texture> {
        let format = if texture.mime == "image/png" {
            image::ImageFormat::Png
        } else {
            image::ImageFormat::Jpeg
        };
        let rgba = self.decode_image(&texture.bytes, format)?;
        let gpu = self.get()?;
        let address = |value| match value {
            33071 => wgpu::AddressMode::ClampToEdge,
            33648 => wgpu::AddressMode::MirrorRepeat,
            _ => wgpu::AddressMode::Repeat,
        };
        let filter = |nearest| {
            if nearest {
                wgpu::FilterMode::Nearest
            } else {
                wgpu::FilterMode::Linear
            }
        };
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: address(texture.wrap_s),
            address_mode_v: address(texture.wrap_t),
            mag_filter: filter(texture.mag == 9728),
            min_filter: filter(matches!(texture.min, 9728 | 9984 | 9986)),
            ..Default::default()
        });
        Ok(Texture {
            view: gpu.upload_image(&rgba, wgpu::TextureFormat::Rgba8UnormSrgb),
            sampler,
        })
    }

    fn load_fbx_image(&self, path: &Path, asset: &ufbx::Texture) -> Result<wgpu::TextureView> {
        let bytes = fbx_mesh::appearance::texture_bytes(path, asset)?;
        self.load_base_image(&bytes)
    }

    fn load_base_image(&self, bytes: &[u8]) -> Result<wgpu::TextureView> {
        let format = image::guess_format(bytes)?;
        if !matches!(format, image::ImageFormat::Png | image::ImageFormat::Jpeg) {
            return Err("Unsupported base-color image format".into());
        }
        let rgba = self.decode_image(bytes, format)?;
        Ok(self
            .get()?
            .upload_image(&rgba, wgpu::TextureFormat::Rgba8UnormSrgb))
    }

    fn load_3mf_textures(
        &self,
        images: Vec<three_mf::Image>,
        surfaces: &[three_mf::SurfaceTexture],
    ) -> Result<Vec<Texture>> {
        let mut views = Vec::with_capacity(images.len());
        let mut decode_ms = 0.0;
        let mut upload = std::time::Duration::ZERO;
        for asset in images {
            let format = match asset.format {
                three_mf::ImageFormat::Png => image::ImageFormat::Png,
                three_mf::ImageFormat::Jpeg => image::ImageFormat::Jpeg,
            };
            let start = Instant::now();
            let waited = self.wait_ms();
            let rgba = self.decode_image(&asset.bytes, format)?;
            decode_ms += start.elapsed().as_secs_f64() * 1000.0 - (self.wait_ms() - waited);
            let start = Instant::now();
            // 3MF interpolates encoded sRGB samples before converting to linear.
            views.push(
                self.get()?
                    .upload_image(&rgba, wgpu::TextureFormat::Rgba8Unorm),
            );
            upload += start.elapsed();
        }
        let mut textures = Vec::with_capacity(surfaces.len());
        for surface in surfaces {
            let [u, v] = surface.tiles.map(|tile| match tile {
                three_mf::Tile::Wrap => wgpu::AddressMode::Repeat,
                three_mf::Tile::Mirror => wgpu::AddressMode::MirrorRepeat,
                three_mf::Tile::Clamp => wgpu::AddressMode::ClampToEdge,
            });
            let filter = if surface.nearest {
                wgpu::FilterMode::Nearest
            } else {
                wgpu::FilterMode::Linear
            };
            let sampler = self.get()?.device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: u,
                address_mode_v: v,
                min_filter: filter,
                mag_filter: filter,
                ..Default::default()
            });
            textures.push(Texture {
                view: views[surface.image].clone(),
                sampler,
            });
        }
        if !views.is_empty() && std::env::var_os("STEPFLASH_PROFILE").is_some() {
            eprintln!(
                "3MF texture GPU phases: decode={:.2} ms, upload={:.2} ms, images={}, samplers={}",
                decode_ms,
                upload.as_secs_f64() * 1000.0,
                views.len(),
                textures.len()
            );
        }
        Ok(textures)
    }
}

impl Builder<'_> {
    fn add_saved_image(&mut self, path: &Path) -> Result<()> {
        let bytes = super::saved_image::load(path)?;
        let [w,h] = super::saved_image::dimensions(&bytes)?;
        let format = image::guess_format(&bytes)?;
        let rgba = self.uploader.decode_image(&bytes, format)?;
        let gpu = self.uploader.get()?;
        let texture = Texture {
            view: gpu.upload_image(&rgba, wgpu::TextureFormat::Rgba8UnormSrgb),
            sampler: gpu.linear_sampler(wgpu::AddressMode::ClampToEdge, wgpu::AddressMode::ClampToEdge),
        };
        let (x,y) = (w as f32*0.5, h as f32*0.5);
        self.add(&[[-x,-y,0.],[x,-y,0.],[x,y,0.],[-x,y,0.]], &[[0,1,2],[0,2,3]], false,
            Style { unlit: true, transparent: true, linear_color: Some([1.;4]),
                texture: Some(&texture), texture_uv: Some(&[[0.,1.],[1.,1.],[1.,0.],[0.,0.]]),
                ..Default::default() })?;
        self.scene.plan_view = true;
        self.scene.raster_size = Some([w,h]);
        self.scene.notice = Some("saved image only");
        self.scene.diagnostics.push("Saved image preview: pan and zoom are available. Native geometry, measurement and sectioning are unavailable for this document.".into());
        Ok(())
    }

    fn add_gltf(&mut self, path: &Path) -> Result<()> {
        gltf_mesh::with_desktop_scene(path, |mesh| {
            let mut textures = HashMap::new();
            for part in mesh.parts() {
                let texture =
                    if let Some(id) = part.appearance.and_then(|material| material.texture) {
                        textures
                            .entry(id)
                            .or_insert_with(|| {
                                let start = Instant::now();
                                let waited = self.uploader.wait_ms();
                                let texture = mesh.textures.get(&id)?.as_ref()?;
                                let result = self.uploader.load_texture(texture).ok();
                                self.scene.upload_ms += start.elapsed().as_secs_f64() * 1000.0
                                    - (self.uploader.wait_ms() - waited);
                                if result.is_none() {
                                    self.scene.issues += 1;
                                }
                                result
                            })
                            .as_ref()
                    } else {
                        None
                    };
                self.triangle_part(
                    &part.points,
                    part.triangles,
                    part.mirrored,
                    Style {
                        material: part.appearance,
                        texture,
                        ..Default::default()
                    },
                )?;
            }
            self.scene.issues += usize::from(mesh.incomplete_appearance);
            Ok(())
        })
    }

    fn add_dxf(&mut self, drawing: dxf_drawing::Scene) -> Result<()> {
        let count = drawing.segments.len();
        let mut bytes = Vec::with_capacity(count * 28);
        for segment in &drawing.segments {
            for p in [segment.a, segment.b] {
                self.include_bounds(p, p)?;
                for value in p {
                    bytes.extend(value.to_le_bytes());
                }
            }
            bytes.extend(segment.color);
        }
        let gpu = self.uploader.get()?;
        let start = Instant::now();
        let material = gpu.bind_material(
            &Style::default(),
            [&gpu.empty, &gpu.empty],
            false,
            false,
            [1., 1., 0., 0.],
        )?;
        self.scene.parts.push(Part {
            source_positions: None,
            transparent: false,
            positions: gpu.buffer(&bytes, wgpu::BufferUsages::VERTEX)?,
            indices: None,
            material,
            count: u32::try_from(count)?,
            points: false,
            lines: true,
            obj_corners: false,
            instances: None,
        });
        self.scene.lines = count;
        self.scene.curves = drawing.curves;
        self.scene.plan_view = true;
        self.scene.notice = Some("2D drawing");
        self.scene.issues += drawing.issues;
        self.scene.diagnostics = drawing.diagnostics;
        self.scene.upload_ms += start.elapsed().as_secs_f64() * 1000.;
        Ok(())
    }
    fn add_solidworks(
        &mut self,
        mesh: &solidworks::Mesh,
        instances: &[(three_mf::Instance, bool)],
    ) -> Result<()> {
        self.scene.retain_saved_preview |= mesh.incomplete_decals;
        if mesh.incomplete_decals
            && !self
                .scene
                .diagnostics
                .iter()
                .any(|s| s.contains("unresolved decals"))
        {
            self.scene.diagnostics.push("SolidWorks has unresolved decals; Explorer retains the saved image to preserve labels.".into());
        }
        let mut groups = std::collections::BTreeMap::<Option<[u8; 4]>, Vec<&[Triangle]>>::new();
        let mut cuts = vec![0, mesh.triangles.len()];
        let colors: std::collections::BTreeMap<_, _> = mesh
            .face_colors
            .iter()
            .map(|(range, color)| {
                cuts.extend([range.start, range.end]);
                (range.start, (range.end, *color))
            })
            .collect();
        for decal in &mesh.decals {
            cuts.extend([decal.range.start, decal.range.end]);
        }
        cuts.sort_unstable();
        cuts.dedup();
        let mut textures = HashMap::new();
        for cut in cuts.windows(2) {
            let range = cut[0]..cut[1];
            if range.is_empty() {
                continue;
            }
            let color = colors
                .range(..=range.start)
                .next_back()
                .filter(|(_, (end, _))| *end >= range.end)
                .map(|(_, (_, color))| *color)
                .or(mesh.color);
            if let Some(decal) = mesh
                .decals
                .iter()
                .find(|d| d.range.start <= range.start && d.range.end >= range.end)
            {
                let key = std::sync::Arc::as_ptr(&decal.image) as usize;
                if !textures.contains_key(&key) {
                    match self.uploader.load_base_image(&decal.image) {
                        Ok(view) => {
                            let gpu = self.uploader.get()?;
                            textures.insert(
                                key,
                                Texture {
                                    view,
                                    sampler: gpu.linear_sampler(
                                        wgpu::AddressMode::ClampToEdge,
                                        wgpu::AddressMode::ClampToEdge,
                                    ),
                                },
                            );
                        }
                        Err(error) => {
                            self.scene.retain_saved_preview = true;
                            self.scene
                                .diagnostics
                                .push(format!("Saved decal image could not be decoded: {error}"));
                        }
                    }
                }
                if let Some(texture) = textures.get(&key) {
                    let uv: Vec<_> = mesh.positions.iter().map(|p| decal.uv(*p)).collect();
                    let color = color.unwrap_or([192, 192, 192, 255]);
                    self.add(
                        &mesh.positions,
                        &mesh.triangles[range],
                        false,
                        Style {
                            color: Some(color),
                            transparent: color[3] < 255,
                            texture: Some(texture),
                            texture_uv: Some(&uv),
                            decal: true,
                            instances,
                            ..Default::default()
                        },
                    )?;
                    continue;
                }
            }
            groups
                .entry(color)
                .or_default()
                .push(&mesh.triangles[range]);
        }
        if std::env::var_os("STEPFLASH_PROFILE").is_some() {
            eprintln!(
                "SolidWorks appearance: default={:?}, face_assignments={}, color_groups={}",
                mesh.color,
                mesh.face_colors.len(),
                groups.len()
            );
        }
        for (color, batches) in groups {
            let count = batches.iter().map(|batch| batch.len()).sum();
            if count == 0 {
                continue;
            }
            if color.is_none() {
                let message =
                    "Some SolidWorks faces have no resolved saved appearance; using neutral gray.";
                if !self.scene.diagnostics.iter().any(|d| d == message) {
                    self.scene.diagnostics.push(message.into());
                }
            }
            let color = color.or(Some([192, 192, 192, 255]));
            self.add_batches(
                &mesh.positions,
                count,
                batches,
                false,
                Style {
                    color,
                    transparent: color.is_some_and(|color| color[3] < 255),
                    instances,
                    ..Default::default()
                },
            )?;
        }
        Ok(())
    }

    fn include_bounds(&mut self, min: Point, max: Point) -> Result<()> {
        for axis in 0..3 {
            if !min[axis].is_finite() || !max[axis].is_finite() {
                return Err("Model placement exceeds the coordinate range".into());
            }
            self.min[axis] = self.min[axis].min(min[axis]);
            self.max[axis] = self.max[axis].max(max[axis]);
        }
        Ok(())
    }

    fn add(
        &mut self,
        points: &[Point],
        triangles: &[Triangle],
        cloud: bool,
        style: Style,
    ) -> Result<()> {
        self.add_batches(
            points,
            triangles.len(),
            std::iter::once(triangles),
            cloud,
            style,
        )
    }

    fn add_batches<'a>(
        &mut self,
        points: &[Point],
        triangle_count: usize,
        batches: impl IntoIterator<Item = &'a [Triangle]> + Clone,
        cloud: bool,
        style: Style,
    ) -> Result<()> {
        if points.is_empty() || (!cloud && triangle_count == 0 && points.len() % 3 != 0) {
            return Err("Invalid mesh vertex count".into());
        }
        let bounds = Instant::now();
        let (min, max) = point_bounds(points)?;
        let bounds_ms = bounds.elapsed().as_secs_f64() * 1000.0;
        let gpu = self.uploader.get()?;
        let start = Instant::now();
        let instances = if style.instances.is_empty() {
            self.include_bounds(min, max)?;
            None
        } else {
            let mut transforms = Vec::with_capacity(style.instances.len());
            for mirrored in [false, true] {
                for &(matrix, _) in style.instances.iter().filter(|(_, flip)| *flip == mirrored) {
                    let (lo, hi) = instance_bounds(matrix, min, max)?;
                    self.include_bounds(lo, hi)?;
                    transforms.push(matrix);
                }
            }
            let count = u32::try_from(transforms.len())?;
            let positive_count = style
                .instances
                .iter()
                .filter(|(_, mirrored)| !mirrored)
                .count() as u32;
            let mirrored_indices = if positive_count != 0 && positive_count < count {
                Some(gpu.index_buffer(triangle_count, batches.clone(), true)?)
            } else {
                None
            };
            Some(Instances {
                transforms: gpu.buffer(
                    bytemuck::cast_slice(&transforms),
                    wgpu::BufferUsages::VERTEX,
                )?,
                count,
                positive_count,
                mirrored_indices,
            })
        };
        let positions = gpu.buffer(bytemuck::cast_slice(points), wgpu::BufferUsages::VERTEX)?;
        let indices = if triangle_count == 0 {
            None
        } else {
            let mirrored = instances
                .as_ref()
                .is_some_and(|batch| batch.positive_count == 0);
            Some(gpu.index_buffer(triangle_count, batches, mirrored)?)
        };
        let colors = style
            .colors
            .map(|rgba| {
                if rgba.len() != points.len() {
                    return Err("Invalid vertex colors".into());
                }
                gpu.buffer(bytemuck::cast_slice(rgba), wgpu::BufferUsages::STORAGE)
            })
            .transpose()?;
        let coordinates = style
            .texture_uv
            .or_else(|| style.material.map(|material| material.uv.as_slice()));
        let uv = match (coordinates, style.texture) {
            (Some(coordinates), Some(_)) if coordinates.len() == points.len() => Some(gpu.buffer(
                bytemuck::cast_slice(coordinates),
                wgpu::BufferUsages::STORAGE,
            )?),
            _ => None,
        };
        let material = gpu.bind_material(
            &style,
            [
                colors.as_ref().unwrap_or(&gpu.empty),
                uv.as_ref().unwrap_or(&gpu.empty),
            ],
            colors.is_some(),
            uv.is_some(),
            [1.0, 1.0, 0.0, 0.0],
        )?;
        let count = if indices.is_some() {
            triangle_count * 3
        } else {
            points.len()
        };
        let copies = instances.as_ref().map_or(1, |batch| batch.count as usize);
        self.scene.parts.push(Part {
            source_positions: None,
            transparent: style.transparent,
            positions,
            indices,
            material,
            count: u32::try_from(count)?,
            points: cloud,
            lines: false,
            obj_corners: false,
            instances,
        });
        if cloud {
            self.scene.points += count;
        } else {
            self.scene.triangles += (count / 3) * copies;
        }
        self.scene.upload_ms += bounds_ms + start.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }

    fn triangle_part(
        &mut self,
        points: &[Point],
        triangles: &[Triangle],
        mirrored: bool,
        style: Style,
    ) -> Result<()> {
        if triangles.is_empty() {
            return Ok(());
        }
        if mirrored {
            let flipped: Vec<_> = triangles.iter().map(|&[a, b, c]| [a, c, b]).collect();
            self.add(points, &flipped, false, style)
        } else {
            self.add(points, triangles, false, style)
        }
    }

    fn add_ifc_batch(&mut self, batch: &ifc_mesh::Batch<'_>, anchor: [f64; 3]) -> Result<()> {
        let gpu = self.uploader.get()?;
        let start = Instant::now();
        let mut bounds = Ok(([0.0; 3], [0.0; 3]));
        let positions = gpu.buffer_with(
            batch.vertices * std::mem::size_of::<Point>(),
            wgpu::BufferUsages::VERTEX,
            |mapped| bounds = batch.write_positions(bytemuck::cast_slice_mut(mapped), anchor),
        )?;
        let (min, max) = bounds?;
        self.include_bounds(min, max)?;
        let count = u32::try_from(batch.triangles * 3)?;
        let indices = gpu.buffer_with(
            batch.triangles * std::mem::size_of::<Triangle>(),
            wgpu::BufferUsages::INDEX,
            |mapped| batch.write_indices(bytemuck::cast_slice_mut(mapped)),
        )?;
        let material = gpu.bind_material(
            &Style {
                linear_color: Some(batch.color),
                ..Default::default()
            },
            [&gpu.empty, &gpu.empty],
            false,
            false,
            [1.0, 1.0, 0.0, 0.0],
        )?;
        self.scene.parts.push(Part {
            source_positions: None,
            transparent: false,
            positions,
            indices: Some(indices),
            material,
            count,
            points: false,
            lines: false,
            obj_corners: false,
            instances: None,
        });
        self.scene.triangles += batch.triangles;
        self.scene.upload_ms += start.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }
}

fn instance_bounds(matrix: three_mf::Instance, min: Point, max: Point) -> Result<(Point, Point)> {
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    for (axis, row) in matrix.iter().enumerate() {
        if !row.iter().all(|value| value.is_finite()) {
            return Err("Invalid 3MF instance transform".into());
        }
        lo[axis] = row[3];
        hi[axis] = row[3];
        for coordinate in 0..3 {
            let a = row[coordinate] * min[coordinate];
            let b = row[coordinate] * max[coordinate];
            lo[axis] += a.min(b);
            hi[axis] += a.max(b);
        }
    }
    Ok((lo, hi))
}

pub fn load(path: &Path, uploader: &UploadTarget) -> Result<Scene> {
    let mut builder = Builder {
        uploader,
        scene: Scene {
            raster_size: None,
            curves: Vec::new(),
            meters_per_unit: None,
            bounds: [[0.0; 3]; 2],
            parts: Vec::new(),
            center: [0.0; 3],
            diameter: 0.0,
            triangles: 0,
            points: 0,
            lines: 0,
            plan_view: false,
            issues: 0,
            notice: None,
            retain_saved_preview: false,
            diagnostics: Vec::new(),
            upload_ms: 0.0,
            gpu_wait_ms: 0.0,
        },
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "obj" => {
            use obj::appearance::Model;
            match obj::appearance::load(path)? {
                Model::Geometry(mesh, issues) => {
                    builder.scene.issues += issues;
                    builder.add_batches(
                        &mesh.positions,
                        mesh.count,
                        mesh.triangles.iter().map(Vec::as_slice),
                        false,
                        Style::default(),
                    )?;
                }
                Model::Styled {
                    points,
                    uv,
                    parts,
                    materials,
                    issues,
                } => {
                    builder.scene.issues += issues;
                    let bounds = Instant::now();
                    let (min, max) = point_bounds(&points)?;
                    builder.include_bounds(min, max)?;
                    builder.scene.upload_ms += bounds.elapsed().as_secs_f64() * 1000.0;
                    let mut images = HashMap::new();
                    let mut textures = HashMap::new();
                    for part in &parts {
                        let material = &materials[part.material];
                        if let Some(map) = material.texture.as_ref().filter(|_| part.textured) {
                            textures.entry((&map.path, map.clamp)).or_insert_with(|| {
                                let view = images
                                    .entry(&map.path)
                                    .or_insert_with(|| {
                                        let start = Instant::now();
                                        let waited = uploader.wait_ms();
                                        let result = obj::appearance::image_bytes(&map.path)
                                            .and_then(|bytes| uploader.load_base_image(&bytes))
                                            .ok();
                                        builder.scene.upload_ms += start.elapsed().as_secs_f64()
                                            * 1000.0
                                            - (uploader.wait_ms() - waited);
                                        builder.scene.issues += usize::from(result.is_none());
                                        result
                                    })
                                    .as_ref()?;
                                let mode = if map.clamp {
                                    wgpu::AddressMode::ClampToEdge
                                } else {
                                    wgpu::AddressMode::Repeat
                                };
                                Some(Texture {
                                    view: view.clone(),
                                    sampler: uploader.get().ok()?.linear_sampler(mode, mode),
                                })
                            });
                        }
                    }
                    let gpu = uploader.get()?;
                    let start = Instant::now();
                    let positions =
                        gpu.buffer(bytemuck::cast_slice(&points), wgpu::BufferUsages::STORAGE)?;
                    drop(points);
                    let coordinates = if uv.is_empty() {
                        gpu.empty.clone()
                    } else {
                        gpu.buffer(bytemuck::cast_slice(&uv), wgpu::BufferUsages::STORAGE)?
                    };
                    drop(uv);
                    builder.scene.upload_ms += start.elapsed().as_secs_f64() * 1000.0;
                    for part in parts {
                        let material = &materials[part.material];
                        let map = material.texture.as_ref().filter(|_| part.textured);
                        let texture = map
                            .and_then(|map| textures.get(&(&map.path, map.clamp)))
                            .and_then(Option::as_ref);
                        let linear_color = material.color.or_else(|| texture.map(|_| [1.0; 4]));
                        let start = Instant::now();
                        let triangle_count = part.batches.iter().map(Vec::len).sum::<usize>();
                        let count = u32::try_from(triangle_count * 3)?;
                        let corners = gpu.buffer_with(
                            triangle_count * std::mem::size_of::<obj::appearance::CornerTriangle>(),
                            wgpu::BufferUsages::VERTEX,
                            |mut mapped| {
                                for batch in &part.batches {
                                    let bytes = bytemuck::cast_slice(batch);
                                    let (target, remaining) = mapped.split_at_mut(bytes.len());
                                    target.copy_from_slice(bytes);
                                    mapped = remaining;
                                }
                            },
                        )?;
                        let style = Style {
                            linear_color,
                            texture,
                            ..Default::default()
                        };
                        let material = gpu.bind_material(
                            &style,
                            [&positions, &coordinates],
                            false,
                            texture.is_some(),
                            map.map_or([1.0, 1.0, 0.0, 0.0], |map| map.transform),
                        )?;
                        builder.scene.parts.push(Part {
                            source_positions: Some(positions.clone()),
                            transparent: false,
                            positions: corners,
                            indices: None,
                            material,
                            count,
                            points: false,
                            lines: false,
                            obj_corners: true,
                            instances: None,
                        });
                        builder.scene.triangles += triangle_count;
                        builder.scene.upload_ms += start.elapsed().as_secs_f64() * 1000.0;
                    }
                    if !images.is_empty() && std::env::var_os("STEPFLASH_PROFILE").is_some() {
                        eprintln!(
                            "OBJ GPU textures: images={}, samplers={}",
                            images.len(),
                            textures.len()
                        );
                    }
                }
            }
        }
        "ply" => {
            let mesh = ply::load(path)?;
            builder.add(
                &mesh.positions,
                &mesh.triangles,
                mesh.triangles.is_empty(),
                Style {
                    colors: mesh.colors.as_deref(),
                    ..Default::default()
                },
            )?;
        }
        "3mf" => {
            let model = three_mf::load(path)?;
            let start = Instant::now();
            let waited = uploader.wait_ms();
            let textures = uploader.load_3mf_textures(model.images, &model.textures)?;
            builder.scene.upload_ms +=
                start.elapsed().as_secs_f64() * 1000.0 - (uploader.wait_ms() - waited);
            for mesh in &model.meshes {
                let (texture, texture_uv) =
                    mesh.texture.as_ref().map_or((None, None), |(id, uv)| {
                        (Some(&textures[*id]), Some(uv.as_slice()))
                    });
                builder.add(
                    &mesh.points,
                    &mesh.triangles,
                    false,
                    Style {
                        color: mesh.color,
                        colors: mesh.colors.as_deref(),
                        texture,
                        texture_uv,
                        encoded_texture: true,
                        instances: &mesh.instances,
                        ..Default::default()
                    },
                )?;
            }
            builder.scene.issues = model.issues;
        }
        "gltf" | "glb" | "gltfpack" => builder.add_gltf(path)?,
        "fbx" => {
            let mut images = HashMap::new();
            let mut textures = HashMap::new();
            let issues = fbx_mesh::appearance::with_surfaces(path, |surface| {
                let texture = if let Some(asset) = surface.texture {
                    let id = asset.element.typed_id;
                    textures
                        .entry(id)
                        .or_insert_with(|| {
                            let key = (
                                asset.has_file,
                                if asset.has_file { asset.file_index } else { id },
                            );
                            let view = images
                                .entry(key)
                                .or_insert_with(|| {
                                    let start = Instant::now();
                                    let waited = uploader.wait_ms();
                                    let result = uploader.load_fbx_image(path, asset).ok();
                                    builder.scene.upload_ms += start.elapsed().as_secs_f64()
                                        * 1000.0
                                        - (uploader.wait_ms() - waited);
                                    builder.scene.issues += usize::from(result.is_none());
                                    result
                                })
                                .as_ref()?;
                            Some(Texture {
                                view: view.clone(),
                                sampler: uploader.get().ok()?.fbx_sampler(asset),
                            })
                        })
                        .as_ref()
                } else {
                    None
                };
                builder.add(
                    &surface.points,
                    &surface.triangles,
                    false,
                    Style {
                        linear_color: surface.color,
                        texture,
                        texture_uv: surface.uv.as_deref(),
                        instances: &surface.instances,
                        ..Default::default()
                    },
                )
            })?;
            builder.scene.issues += issues;
        }
        "igs" | "iges" | "x_t" | "x_b" | "xt" | "xb" | "xmt_txt" | "xmt_bin" | "brep" => {
            let converted = super::cad_import::load(path)?;
            builder.add_gltf(&converted.path)?;
            builder.scene.meters_per_unit = converted.report.meters_per_unit;
            builder.scene.diagnostics.push(converted.report.notice);
            if std::env::var_os("STEPFLASH_PROFILE").is_some() {
                eprintln!("CAD cache_hit={}", converted.cached);
            }
        }
        "f3d" | "slddrw" => builder.add_saved_image(path)?,
        "ifc" | "ifczip" => {
            let model = ifc_mesh::load(path)?;
            builder.scene.issues += ifc_mesh::issues(&model);
            let plan = ifc_mesh::draw_plan(&model)?;
            for batch in &plan.flat {
                builder.add_ifc_batch(batch, plan.anchor)?;
            }
            for batch in &plan.instanced {
                builder.add(
                    batch.geometry.points,
                    batch.geometry.triangles,
                    false,
                    Style {
                        linear_color: Some(batch.color),
                        instances: &batch.placements,
                        ..Default::default()
                    },
                )?;
            }
            if std::env::var_os("STEPFLASH_PROFILE").is_some() {
                eprintln!(
                    "IFC GPU draws: flat={}, instanced={}, triangles={}",
                    plan.flat.len(),
                    plan.instanced.len(),
                    builder.scene.triangles
                );
            }
        }
        "dxf" => builder.add_dxf(dxf_drawing::load(path)?)?,
        "sldprt" => {
            let mesh = solidworks::load_part(path)?;
            builder.add_solidworks(&mesh, &[])?;
            builder.scene.diagnostics.push("SolidWorks saved appearances and supported projection decals; unresolved mappings retain the saved Explorer image.".into());
        }
        "sldasm" => {
            let scene = solidworks::load_assembly(path)?;
            builder.scene.notice = if scene.snapshot {
                Some("saved assembly snapshot")
            } else if scene.issues != 0 {
                Some("partial assembly")
            } else {
                None
            };
            builder.scene.issues += scene.issues;
            builder.scene.diagnostics = scene.diagnostics;
            builder.scene.diagnostics.push("SolidWorks saved appearances and supported projection decals; unresolved mappings retain the saved Explorer image.".into());
            for part in scene.meshes {
                builder.add_solidworks(&part.mesh, &part.instances)?;
            }
        }
        "stp" | "step" | "p21" => {
            let bytes = fs::read(path)?;
            let flat = StepFile::strip_flatten(&bytes);
            let step = StepFile::parse(&flat);
            let (mesh, stats) = triangulate::triangulate::triangulate(&step);
            let points: Vec<Point> = mesh
                .verts
                .iter()
                .map(|vertex| {
                    [
                        vertex.pos.x as f32,
                        vertex.pos.y as f32,
                        vertex.pos.z as f32,
                    ]
                })
                .collect();
            let triangles: Vec<Triangle> = mesh
                .triangles
                .iter()
                .map(|face| [face.verts.x, face.verts.y, face.verts.z])
                .collect();
            let colors: Option<Vec<[u8; 4]>> = mesh.has_colors.then(|| {
                mesh.verts
                    .iter()
                    .map(|vertex| {
                        let [r, g, b] = [vertex.color.x, vertex.color.y, vertex.color.z]
                            .map(|v| (v * 255.0).round() as u8);
                        [r, g, b, 255]
                    })
                    .collect()
            });
            builder.scene.issues = stats.num_errors + stats.num_panics;
            builder.triangle_part(
                &points,
                &triangles,
                false,
                Style {
                    colors: colors.as_deref(),
                    ..Default::default()
                },
            )?;
        }
        "stl" => builder.add(&read_stl(&fs::read(path)?)?, &[], false, Style::default())?,
        _ => return Err("Unsupported desktop model format".into()),
    }
    if builder.scene.parts.is_empty() {
        return Err("The model contains no drawable geometry".into());
    }
    builder.scene.meters_per_unit = match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        // The SolidWorks reader converts saved SI coordinates to millimeters.
        "sldprt" | "sldasm" | "3mf" => Some(0.001),
        "glb" | "gltf" => Some(1.),
        _ => builder.scene.meters_per_unit,
    };
    let spans = [0, 1, 2].map(|i| f64::from(builder.max[i]) - f64::from(builder.min[i]));
    builder.scene.center = [0, 1, 2].map(|i| builder.min[i] * 0.5 + builder.max[i] * 0.5);
    builder.scene.bounds = [builder.min, builder.max];
    builder.scene.diameter = spans
        .iter()
        .map(|span| span * span)
        .sum::<f64>()
        .sqrt()
        .clamp(1e-7, f32::MAX as f64) as f32;
    builder.scene.gpu_wait_ms = uploader.wait_ms();
    Ok(builder.scene)
}

fn read_stl(bytes: &[u8]) -> Result<Vec<Point>> {
    if let Some(header) = bytes.get(..84) {
        let count = u32::from_le_bytes(header[80..84].try_into()?) as usize;
        if count != 0
            && count.checked_mul(50).and_then(|size| size.checked_add(84)) == Some(bytes.len())
        {
            let mut points = Vec::with_capacity(count * 3);
            for record in bytes[84..].chunks_exact(50) {
                for vertex in record[12..48].chunks_exact(12) {
                    let coordinate = |i| f32::from_le_bytes(vertex[i..i + 4].try_into().unwrap());
                    points.push([coordinate(0), coordinate(4), coordinate(8)]);
                }
            }
            return Ok(points);
        }
    }
    read_ascii_stl(std::str::from_utf8(bytes)?.trim_start_matches('\u{feff}'))
}

fn read_ascii_stl(mut input: &str) -> Result<Vec<Point>> {
    fn word<'a>(input: &mut &'a str) -> &'a str {
        let remaining = input.trim_start();
        let end = remaining
            .find(char::is_whitespace)
            .unwrap_or(remaining.len());
        let (token, tail) = remaining.split_at(end);
        *input = tail;
        token
    }

    fn expect(input: &mut &str, expected: &str) -> Result<()> {
        if word(input) != expected {
            return Err(format!("Malformed ASCII STL: expected {}", expected).into());
        }
        Ok(())
    }

    fn coordinates(input: &mut &str) -> Result<Point> {
        let mut point = [0.0f32; 3];
        for value in &mut point {
            *value = fast_float::parse(word(input))?;
            if !value.is_finite() {
                return Err("Invalid ASCII STL coordinate".into());
            }
        }
        Ok(point)
    }

    let after_name = |text: &str| text.find('\n').map_or(text.len(), |end| end + 1);
    let mut points = Vec::new();
    while !input.trim_start().is_empty() {
        expect(&mut input, "solid")?;
        input = &input[after_name(input)..];
        loop {
            match word(&mut input) {
                "endsolid" => break,
                "facet" => {
                    expect(&mut input, "normal")?;
                    coordinates(&mut input)?;
                    expect(&mut input, "outer")?;
                    expect(&mut input, "loop")?;
                    for _ in 0..3 {
                        expect(&mut input, "vertex")?;
                        points.push(coordinates(&mut input)?);
                    }
                    expect(&mut input, "endloop")?;
                    expect(&mut input, "endfacet")?;
                }
                _ => return Err("Malformed ASCII STL: expected facet or endsolid".into()),
            }
        }
        input = &input[after_name(input)..];
    }
    if points.is_empty() {
        return Err("Invalid ASCII STL geometry".into());
    }
    Ok(points)
}
