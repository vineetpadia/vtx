use std::{convert::TryInto, path::Path, time::Instant};

use wgpu::util::DeviceExt;

use super::{
    model::{Scene, Texture, UploadTarget, Uploader},
    spawn_import, Result,
};

const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;
const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.003,
    g: 0.006,
    b: 0.012,
    a: 1.0,
};

pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
    pub orthographic: bool,
    pub target: Option<([f32; 3], f32)>,
    pub section: Option<[f32; 4]>,
    pub z_up: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: -0.6,
            pitch: 0.4,
            zoom: 1.0,
            orthographic: false,
            target: None,
            section: None,
            z_up: false,
        }
    }
}

pub struct Renderer {
    pub uploader: Uploader,
    camera: wgpu::Buffer,
    camera_group: wgpu::BindGroup,
    triangles: wgpu::RenderPipeline,
    obj_corners: wgpu::RenderPipeline,
    points: wgpu::RenderPipeline,
    identity: wgpu::Buffer,
    lines: wgpu::RenderPipeline,
    query_triangles: wgpu::RenderPipeline,
    query_obj: wgpu::RenderPipeline,
    query_points: wgpu::RenderPipeline,
    query_lines: wgpu::RenderPipeline,
}

pub fn initialize<T>(mut create: impl FnMut(&wgpu::Instance) -> Result<T>) -> Result<T> {
    let backends = if cfg!(target_os = "macos") {
        wgpu::Backends::METAL
    } else {
        wgpu::Backends::VULKAN
    };
    let descriptor = wgpu::InstanceDescriptor {
        backends,
        ..Default::default()
    }
    .with_env();
    let primary = create(&wgpu::Instance::new(&descriptor));
    #[cfg(windows)]
    if std::env::var_os("WGPU_BACKEND").is_none() {
        // Delay loading Direct3D and enumerating its adapters until Vulkan fails.
        return primary.or_else(|vulkan_error| {
            let fallback = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends: wgpu::Backends::DX12,
                ..descriptor
            });
            create(&fallback).map_err(|dx12_error| {
                format!(
                    "Vulkan initialization failed ({}) and Direct3D 12 initialization failed ({})",
                    vulkan_error, dx12_error
                )
                .into()
            })
        });
    }
    primary
}

pub fn adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<wgpu::Adapter> {
    Ok(pollster::block_on(instance.request_adapter(
        &wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: surface,
            force_fallback_adapter: false,
        },
    ))?)
}

impl Renderer {
    pub fn new(adapter: &wgpu::Adapter, format: wgpu::TextureFormat) -> Result<Self> {
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("VTX desktop device"),
                required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                ..Default::default()
            }))?;
        let buffer_entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VTX camera layout"),
            entries: &[buffer_entry(
                0,
                wgpu::ShaderStages::VERTEX_FRAGMENT,
                wgpu::BufferBindingType::Uniform,
            )],
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VTX material layout"),
            entries: &[
                buffer_entry(
                    0,
                    wgpu::ShaderStages::VERTEX_FRAGMENT,
                    wgpu::BufferBindingType::Uniform,
                ),
                buffer_entry(
                    1,
                    wgpu::ShaderStages::VERTEX,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
                buffer_entry(
                    2,
                    wgpu::ShaderStages::VERTEX,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("VTX pipeline layout"),
            bind_group_layouts: &[&camera_layout, &layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("VTX mesh shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mesh.wgsl").into()),
        });
        let pipeline = |topology,
                        fragment,
                        entry_point,
                        buffers: &[wgpu::VertexBufferLayout<'_>],
                        query: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("VTX geometry pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(if query {
                        if fragment == "surface" {
                            "query_surface"
                        } else {
                            "query_point"
                        }
                    } else {
                        fragment
                    }),
                    compilation_options: Default::default(),
                    targets: &if query {
                        vec![
                            Some(wgpu::ColorTargetState {
                                format: wgpu::TextureFormat::Rgba32Float,
                                blend: None,
                                write_mask: wgpu::ColorWrites::ALL
                            });
                            2
                        ]
                    } else {
                        vec![Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })]
                    },
                }),
                multiview: None,
                cache: None,
            })
        };
        let mesh_layout = [
            wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3],
            },
            wgpu::VertexBufferLayout {
                array_stride: 48,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4, 3 => Float32x4],
            },
        ];
        let triangles = pipeline(
            wgpu::PrimitiveTopology::TriangleList,
            "surface",
            "vertex",
            &mesh_layout,
            false,
        );
        let points = pipeline(
            wgpu::PrimitiveTopology::PointList,
            "point",
            "vertex",
            &mesh_layout,
            false,
        );
        let obj_layout = [wgpu::VertexBufferLayout {
            array_stride: 8,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Uint32x2],
        }];
        let obj_corners = pipeline(
            wgpu::PrimitiveTopology::TriangleList,
            "surface",
            "obj_vertex",
            &obj_layout,
            false,
        );
        let lines = pipeline(
            wgpu::PrimitiveTopology::LineList,
            "point",
            "vertex",
            &mesh_layout,
            false,
        );
        let query_triangles = pipeline(
            wgpu::PrimitiveTopology::TriangleList,
            "surface",
            "vertex",
            &mesh_layout,
            true,
        );
        let query_obj = pipeline(
            wgpu::PrimitiveTopology::TriangleList,
            "surface",
            "obj_vertex",
            &obj_layout,
            true,
        );
        let query_points = pipeline(
            wgpu::PrimitiveTopology::PointList,
            "point",
            "vertex",
            &mesh_layout,
            true,
        );
        let query_lines = pipeline(
            wgpu::PrimitiveTopology::LineList,
            "point",
            "vertex",
            &mesh_layout,
            true,
        );
        let identity = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("VTX identity placement"),
            contents: bytemuck::cast_slice(&[
                [1.0f32, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ]),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("VTX camera"),
            size: 64,
            mapped_at_creation: false,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let camera_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("VTX camera group"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera.as_entire_binding(),
            }],
        });
        let empty = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("VTX empty vertex attribute"),
            contents: &[0; 8],
            usage: wgpu::BufferUsages::STORAGE,
        });
        let white = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: Some("VTX white texture"),
                size: extent(1, 1),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255; 4],
        );
        let white = Texture {
            view: white.create_view(&Default::default()),
            sampler: device.create_sampler(&Default::default()),
        };
        Ok(Self {
            uploader: Uploader {
                device,
                queue,
                layout,
                empty,
                white,
            },
            camera,
            camera_group,
            triangles,
            obj_corners,
            points,
            identity,
            lines,
            query_triangles,
            query_obj,
            query_points,
            query_lines,
        })
    }

    pub fn depth(&self, width: u32, height: u32) -> wgpu::TextureView {
        self.uploader
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("VTX depth buffer"),
                size: extent(width, height),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default())
    }

    pub fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        scene: Option<&Scene>,
        camera: &Camera,
        aspect: f32,
    ) {
        self.draw_pass(encoder, color, depth, scene, camera, aspect, None);
    }

    fn draw_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        scene: Option<&Scene>,
        camera: &Camera,
        aspect: f32,
        query: Option<&wgpu::TextureView>,
    ) {
        if let Some(scene) = scene {
            let (center, diameter) = camera.target.unwrap_or((scene.center, scene.diameter));
            let [x, y, z] = center;
            let scale = 0.9 * aspect.min(1.0) * camera.zoom / diameter;
            let (sy, cy) = camera.yaw.sin_cos();
            let (sx, cx) = camera.pitch.sin_cos();
            let data = [
                x,
                y,
                z,
                scale,
                cy,
                sy,
                cx,
                sx,
                aspect,
                (u8::from(camera.orthographic) + 2 * u8::from(camera.z_up)) as f32,
                diameter / scene.diameter,
                u8::from(camera.section.is_some()) as f32,
            ];
            let data: Vec<f32> = data
                .iter()
                .copied()
                .chain(camera.section.unwrap_or([0.0; 4]).iter().copied())
                .collect();
            self.uploader
                .queue
                .write_buffer(&self.camera, 0, bytemuck::cast_slice(&data));
        }
        let mut attachments = vec![Some(wgpu::RenderPassColorAttachment {
            view: color,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(if query.is_some() {
                    wgpu::Color::TRANSPARENT
                } else {
                    CLEAR
                }),
                store: wgpu::StoreOp::Store,
            },
        })];
        if let Some(view) = query {
            attachments.push(Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            }));
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("VTX frame"),
            color_attachments: &attachments,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        if let Some(scene) = scene {
            for part in &scene.parts {
                pass.set_pipeline(
                    match (query.is_some(), part.obj_corners, part.points, part.lines) {
                        (true, true, _, _) => &self.query_obj,
                        (true, _, true, _) => &self.query_points,
                        (true, _, _, true) => &self.query_lines,
                        (true, _, _, _) => &self.query_triangles,
                        (_, true, _, _) => &self.obj_corners,
                        (_, _, true, _) => &self.points,
                        (_, _, _, true) => &self.lines,
                        _ => &self.triangles,
                    },
                );
                pass.set_bind_group(0, &self.camera_group, &[]);
                pass.set_bind_group(1, &part.material, &[]);
                pass.set_vertex_buffer(0, part.positions.slice(..));
                if part.obj_corners {
                    pass.draw(0..part.count, 0..1);
                    continue;
                }
                let (transforms, count) = part
                    .instances
                    .as_ref()
                    .map_or((&self.identity, 1), |batch| {
                        (&batch.transforms, batch.count)
                    });
                pass.set_vertex_buffer(1, transforms.slice(..));
                if let Some(indices) = &part.indices {
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    if let Some(batch) = part
                        .instances
                        .as_ref()
                        .filter(|batch| batch.mirrored_indices.is_some())
                    {
                        pass.draw_indexed(0..part.count, 0, 0..batch.positive_count);
                        pass.set_index_buffer(
                            batch.mirrored_indices.as_ref().unwrap().slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..part.count, 0, batch.positive_count..count);
                    } else {
                        pass.draw_indexed(0..part.count, 0, 0..count);
                    }
                } else {
                    pass.draw(0..part.count, 0..1);
                }
            }
        }
    }
}

fn extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}

pub fn check(path: &Path, screenshot: Option<&Path>) -> Result<()> {
    const WIDTH: u32 = 768;
    const HEIGHT: u32 = 512;
    let init = Instant::now();
    let (sender, uploader) = UploadTarget::deferred();
    let import = spawn_import(path.to_path_buf(), uploader, |loaded| loaded.result)?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let (adapter, renderer) = initialize(|instance| {
        let adapter = adapter(instance, None)?;
        let renderer = Renderer::new(&adapter, format)?;
        Ok((adapter, renderer))
    })?;
    let device = &renderer.uploader.device;
    let size = extent(WIDTH, HEIGHT);
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("VTX offscreen color"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = color.create_view(&Default::default());
    let depth = renderer.depth(WIDTH, HEIGHT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("VTX check readback"),
        size: u64::from(WIDTH * HEIGHT * 4),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let initialized = init.elapsed();
    let start = Instant::now();
    let _ = sender.send(renderer.uploader.clone());
    let scene = import.join().map_err(|_| "Model import thread failed")??;
    let loaded = start.elapsed();
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.draw(
        &mut encoder,
        &view,
        &depth,
        Some(&scene),
        &Camera {
            z_up: scene.source_info["kind"] == "drawing_3d",
            ..Camera::default()
        },
        WIDTH as f32 / HEIGHT as f32,
    );
    let submission = renderer.uploader.queue.submit([encoder.finish()]);
    let submitted = start.elapsed();
    device.poll(wgpu::PollType::WaitForSubmissionIndex(submission))?;
    let completed = start.elapsed();

    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        color.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(WIDTH * 4),
                rows_per_image: Some(HEIGHT),
            },
        },
        size,
    );
    renderer.uploader.queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    slice.map_async(wgpu::MapMode::Read, move |status| {
        let _ = send.send(status);
    });
    device.poll(wgpu::PollType::Wait)?;
    receive.recv()??;
    let pixels = slice.get_mapped_range();
    let background = &pixels[..4];
    let visible = pixels
        .chunks_exact(4)
        .filter(|pixel| *pixel != background)
        .count();
    if visible == 0 {
        return Err("The GPU rendered only the background".into());
    }
    if let Some(path) = screenshot {
        image::save_buffer(path, &pixels, WIDTH, HEIGHT, image::ColorType::Rgba8)?;
    }
    let ms = |time: std::time::Duration| time.as_secs_f64() * 1000.0;
    let info = adapter.get_info();
    println!("backend={:?} init_ms={:.3} ready_ms={:.3} upload_ms={:.3} submit_ms={:.3} gpu_done_ms={:.3} first_frame_ms={:.3} gpu_wait_ms={:.3} triangles={} points={} pixels={} issues={}",
        info.backend, ms(initialized), ms(loaded), scene.upload_ms, ms(submitted), ms(completed),
        ms(initialized + completed), scene.gpu_wait_ms,
        scene.triangles, scene.points, visible, scene.issues);
    eprintln!("GPU: {}", info.name);
    if scene.source_info["kind"] == "drawing_2d" || scene.source_info["kind"] == "drawing_3d" {
        eprintln!(
            "CAD: reader={} load_ms={:.3} gpu_batches={} entities={}",
            scene.source_info["reader"].as_str().unwrap_or("unknown"),
            scene.source_info["load_ms"].as_f64().unwrap_or(0.0),
            scene.parts.len(),
            scene.part_metadata.len()
        );
    }
    Ok(())
}

/// Render into an RGBA image without a window or display server.
pub fn snapshot(
    renderer: &Renderer,
    scene: &Scene,
    camera: &Camera,
    width: u32,
    height: u32,
) -> Result<image::RgbaImage> {
    let device = &renderer.uploader.device;
    if width == 0 || height == 0 || width.max(height) > device.limits().max_texture_dimension_2d {
        return Err("Invalid snapshot dimensions for this GPU".into());
    }
    let size = extent(width, height);
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("VTX snapshot"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let stride = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("VTX snapshot readback"),
        size: u64::from(stride) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.draw(
        &mut encoder,
        &color.create_view(&Default::default()),
        &renderer.depth(width, height),
        Some(scene),
        camera,
        width as f32 / height as f32,
    );
    encoder.copy_texture_to_buffer(
        color.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(height),
            },
        },
        size,
    );
    renderer.uploader.queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    slice.map_async(wgpu::MapMode::Read, move |status| {
        let _ = send.send(status);
    });
    device.poll(wgpu::PollType::Wait)?;
    receive.recv()??;
    let mapped = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in mapped.chunks_exact(stride as usize) {
        pixels.extend_from_slice(&row[..width as usize * 4]);
    }
    Ok(image::RgbaImage::from_raw(width, height, pixels).ok_or("Invalid GPU image buffer")?)
}

/// Query only requested pixels; avoid downloading full position/normal images.
pub fn query_pixels(
    renderer: &Renderer,
    scene: &Scene,
    camera: &Camera,
    size: u32,
    pixels: &[[u32; 2]],
) -> Result<Vec<serde_json::Value>> {
    if pixels.is_empty() {
        return Ok(Vec::new());
    }
    let device = &renderer.uploader.device;
    let textures: Vec<_> = (0..2)
        .map(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("VTX geometry query"),
                size: extent(size, size),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        })
        .collect();
    let views: Vec<_> = textures
        .iter()
        .map(|t| t.create_view(&Default::default()))
        .collect();
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("VTX query readback"),
        size: pixels.len() as u64 * 512,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.draw_pass(
        &mut encoder,
        &views[0],
        &renderer.depth(size, size),
        Some(scene),
        camera,
        1.0,
        Some(&views[1]),
    );
    for (index, &[x, y]) in pixels.iter().enumerate() {
        if x >= size || y >= size {
            return Err("Query pixel is outside the model viewport".into());
        }
        for (attachment, texture) in textures.iter().enumerate() {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: (index * 512 + attachment * 256) as u64,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                extent(1, 1),
            );
        }
    }
    renderer.uploader.queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    slice.map_async(wgpu::MapMode::Read, move |status| {
        let _ = send.send(status);
    });
    device.poll(wgpu::PollType::Wait)?;
    receive.recv()??;
    let data = slice.get_mapped_range();
    Ok(pixels.iter().enumerate().map(|(index,pixel)| {
        let at = index * 512;
        let value = |offset| f32::from_le_bytes(data[offset..offset+4].try_into().unwrap());
        let part = value(at+12) as usize;
        if part == 0 { return serde_json::json!({"pixel":pixel,"hit":false}); }
        let normal = [value(at+256),value(at+260),value(at+264)];
        serde_json::json!({"pixel":pixel, "hit":true, "position": [value(at),value(at+4),value(at+8)],
            "normal": if normal == [0.0;3] {None} else {Some(normal)}, "part_id":part,
            "instance_index": (value(at+268) as u32).saturating_sub(1),
            "part":scene.part_metadata.get(part-1),
            "normal_convention":"camera-facing geometric surface normal; null for lines/points"})
    }).collect())
}
