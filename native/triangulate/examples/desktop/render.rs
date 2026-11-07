use std::{path::Path, time::Instant};

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
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: -0.6,
            pitch: 0.4,
            zoom: 1.0,
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
                wgpu::ShaderStages::VERTEX,
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
        let pipeline =
            |topology, fragment, entry_point, buffers: &[wgpu::VertexBufferLayout<'_>]| {
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
                        entry_point: Some(fragment),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
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
        );
        let points = pipeline(
            wgpu::PrimitiveTopology::PointList,
            "point",
            "vertex",
            &mesh_layout,
        );
        let obj_corners = pipeline(
            wgpu::PrimitiveTopology::TriangleList,
            "surface",
            "obj_vertex",
            &[wgpu::VertexBufferLayout {
                array_stride: 8,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Uint32x2],
            }],
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
            size: 48,
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
        if let Some(scene) = scene {
            let [x, y, z] = scene.center;
            let scale = 0.9 * aspect.min(1.0) * camera.zoom / scene.diameter;
            let (sy, cy) = camera.yaw.sin_cos();
            let (sx, cx) = camera.pitch.sin_cos();
            let data = [x, y, z, scale, cy, sy, cx, sx, aspect, 0.0, 0.0, 0.0];
            self.uploader
                .queue
                .write_buffer(&self.camera, 0, bytemuck::cast_slice(&data));
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("VTX frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(CLEAR),
                    store: wgpu::StoreOp::Store,
                },
            })],
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
                pass.set_pipeline(if part.obj_corners {
                    &self.obj_corners
                } else if part.points {
                    &self.points
                } else {
                    &self.triangles
                });
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
        &Camera::default(),
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
    Ok(())
}
