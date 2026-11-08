use std::{path::Path, time::Instant};

use wgpu::util::DeviceExt;

use super::{
    model::{Scene, Texture, UploadTarget, Uploader},
    spawn_import, Result,
};

const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;
pub use super::camera::Camera;

pub struct Renderer {
    pipelines: GeometryPipelines,
    tool_pipelines: [std::cell::OnceCell<GeometryPipelines>; 3],
    mesh_layout: wgpu::PipelineLayout,
    mesh_shader: wgpu::ShaderModule,
    pub uploader: Uploader,
    pub(super) camera: wgpu::Buffer,
    pub(super) camera_group: wgpu::BindGroup,
    pub(super) camera_layout: wgpu::BindGroupLayout,
    pub(super) identity: wgpu::Buffer,
    composite: wgpu::RenderPipeline,
    transparency_layout: wgpu::BindGroupLayout,
    transparency: std::cell::RefCell<Option<TransparencyTargets>>,
    format: wgpu::TextureFormat,
    style_layout: wgpu::BindGroupLayout,
    style_pipeline: wgpu::RenderPipeline,
    antialias: wgpu::RenderPipeline,
    style_targets: std::cell::RefCell<Option<StyleTargets>>,
}

struct GeometryPipelines {
    triangles: wgpu::RenderPipeline,
    points: wgpu::RenderPipeline,
    obj_corners: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    transparent: wgpu::RenderPipeline,
    geometry: wgpu::RenderPipeline,
    geometry_obj: wgpu::RenderPipeline,
}
fn geometry_pipelines(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    clipping: bool,
    ortho: bool,
) -> GeometryPipelines {
    let constants = [
        ("CLIPPING", if clipping { 1. } else { 0. }),
        ("PROJECTION", if ortho { 1. } else { 0. }),
    ];
    let pipeline = |topology,
                    fragment,
                    entry_point,
                    buffers: &[wgpu::VertexBufferLayout<'_>],
                    transparent: bool,
                    normal: bool| {
        let targets = if normal {
            vec![Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba16Float,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })]
        } else if transparent {
            vec![
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                }),
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::R16Float,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::REPLACE,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                }),
            ]
        } else {
            vec![Some(wgpu::ColorTargetState {
                format,
                blend: if entry_point == "line_vertex" {
                    Some(wgpu::BlendState::ALPHA_BLENDING)
                } else {
                    None
                },
                write_mask: wgpu::ColorWrites::ALL,
            })]
        };
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("VTX geometry pipeline"),
            layout: Some(pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some(entry_point),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: !transparent,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fragment),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                targets: &targets,
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
        false,
    );
    let points = pipeline(
        wgpu::PrimitiveTopology::PointList,
        "point",
        "vertex",
        &mesh_layout,
        false,
        false,
    );
    let lines = pipeline(
        wgpu::PrimitiveTopology::TriangleList,
        "line_surface",
        "line_vertex",
        &[wgpu::VertexBufferLayout {
            array_stride: 28,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0=>Float32x3,4=>Float32x3,5=>Unorm8x4],
        }],
        false,
        false,
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
        false,
        false,
    );
    let transparent = pipeline(
        wgpu::PrimitiveTopology::TriangleList,
        "transparent_surface",
        "vertex",
        &mesh_layout,
        true,
        false,
    );
    let geometry = pipeline(
        wgpu::PrimitiveTopology::TriangleList,
        "geometry_normal",
        "vertex",
        &mesh_layout,
        false,
        true,
    );
    let geometry_obj = pipeline(
        wgpu::PrimitiveTopology::TriangleList,
        "geometry_normal",
        "obj_vertex",
        &[wgpu::VertexBufferLayout {
            array_stride: 8,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Uint32x2],
        }],
        false,
        true,
    );
    GeometryPipelines {
        triangles,
        points,
        obj_corners,
        lines,
        transparent,
        geometry,
        geometry_obj,
    }
}
struct StyleTargets {
    size: [u32; 2],
    scene_texture: wgpu::Texture,
    scene: wgpu::TextureView,
    geometry: wgpu::TextureView,
    interior_geometry: wgpu::TextureView,
    group: wgpu::BindGroup,
    styled: wgpu::TextureView,
    antialias_group: wgpu::BindGroup,
}

struct TransparencyTargets {
    size: [u32; 2],
    accumulation: wgpu::TextureView,
    revealage: wgpu::TextureView,
    group: wgpu::BindGroup,
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
        let pipelines =
            geometry_pipelines(&device, format, &pipeline_layout, &shader, false, false);
        let transparency_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("VTX transparency composite layout"),
                entries: &[0, 1].map(|binding| wgpu::BindGroupLayoutEntry {
                    binding,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                }),
            });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("VTX transparency composite"),
            bind_group_layouts: &[&transparency_layout],
            push_constant_ranges: &[],
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("VTX transparency composite shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("transparency.wgsl").into()),
        });
        let composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("VTX transparency composite"),
            layout: Some(&composite_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("composite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
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
            size: 80,
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
        let mut style_entries: Vec<_> = [0, 1, 3]
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            })
            .to_vec();
        style_entries.push(buffer_entry(
            2,
            wgpu::ShaderStages::FRAGMENT,
            wgpu::BufferBindingType::Uniform,
        ));
        let style_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VTX CAD style layout"),
            entries: &style_entries,
        });
        let style_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("VTX CAD style pipeline layout"),
                bind_group_layouts: &[&style_layout],
                push_constant_ranges: &[],
            });
        let style_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("VTX CAD style shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("cad_style.wgsl").into()),
        });
        let style_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("VTX CAD style"),
            layout: Some(&style_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &style_shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &style_shader,
                entry_point: Some("composite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        let antialias = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("VTX CAD antialias"),
            layout: Some(&style_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &style_shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &style_shader,
                entry_point: Some("antialias"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
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
            pipelines,
            tool_pipelines: std::array::from_fn(|_| std::cell::OnceCell::new()),
            mesh_layout: pipeline_layout,
            mesh_shader: shader,
            uploader: Uploader {
                device,
                queue,
                layout,
                empty,
                white,
            },
            camera,
            camera_group,
            camera_layout,
            identity,
            composite,
            transparency_layout,
            transparency: std::cell::RefCell::new(None),
            format,
            style_layout,
            style_pipeline,
            antialias,
            style_targets: std::cell::RefCell::new(None),
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

    pub fn present_cached(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) -> bool {
        let cache = self.style_targets.borrow();
        let Some(targets) = cache.as_ref() else {
            return false;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("VTX cached viewport"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.antialias);
        pass.set_bind_group(0, &targets.antialias_group, &[]);
        pass.draw(0..3, 0..1);
        true
    }

    pub fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        scene: Option<&Scene>,
        camera: &Camera,
        aspect: f32,
        size: [u32; 2],
    ) {
        let mut cache = self.style_targets.borrow_mut();
        if cache.as_ref().is_none_or(|targets| targets.size != size) {
            let view = |format| {
                self.uploader
                    .device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("VTX CAD style target"),
                        size: extent(size[0], size[1]),
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING
                            | wgpu::TextureUsages::COPY_SRC,
                        view_formats: &[],
                    })
            };
            let scene_texture = view(self.format);
            let scene = scene_texture.create_view(&Default::default());
            let geometry = view(wgpu::TextureFormat::Rgba16Float).create_view(&Default::default());
            let interior_geometry =
                view(wgpu::TextureFormat::Rgba16Float).create_view(&Default::default());
            let styled = view(self.format).create_view(&Default::default());
            let group = self
                .uploader
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("VTX CAD style targets"),
                    layout: &self.style_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&scene),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&geometry),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: self.camera.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(&interior_geometry),
                        },
                    ],
                });
            let antialias_group =
                self.uploader
                    .device
                    .create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("VTX antialias input"),
                        layout: &self.style_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&styled),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(&geometry),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: self.camera.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::TextureView(&interior_geometry),
                            },
                        ],
                    });
            *cache = Some(StyleTargets {
                size,
                scene_texture,
                scene,
                geometry,
                interior_geometry,
                group,
                styled,
                antialias_group,
            });
        }
        let targets = cache.as_ref().unwrap();
        self.draw_scene(encoder, &targets.scene, depth, scene, camera, aspect, size);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("VTX feature edge geometry"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.geometry,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
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
        if camera.edges {
            if let Some(scene) = scene {
                self.draw_parts(&mut pass, scene, false, true, camera);
                self.draw_parts(&mut pass, scene, true, true, camera);
            }
        }
        drop(pass);
        // Retain the nearest opaque detail beneath transparent covers. A
        // single front-surface normal buffer hides its holes and fasteners.
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("VTX details beneath transparent covers"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.interior_geometry,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
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
        if camera.edges {
            if let Some(scene) = scene {
                self.draw_parts(&mut pass, scene, false, true, camera);
            }
        }
        drop(pass);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("VTX CAD style composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.styled,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.style_pipeline);
        pass.set_bind_group(0, &targets.group, &[]);
        pass.draw(0..3, 0..1);
        drop(pass);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("VTX CAD antialias"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.antialias);
        pass.set_bind_group(0, &targets.antialias_group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn draw_scene(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        scene: Option<&Scene>,
        camera: &Camera,
        aspect: f32,
        size: [u32; 2],
    ) {
        let flags = [
            aspect,
            u8::from(camera.cad_lighting) as f32,
            u8::from(camera.light_background) as f32,
            u8::from(camera.edges) as f32,
        ];
        self.uploader
            .queue
            .write_buffer(&self.camera, 32, bytemuck::cast_slice(&flags));
        self.uploader.queue.write_buffer(
            &self.camera,
            48,
            bytemuck::cast_slice(&[
                size[0] as f32,
                size[1] as f32,
                u8::from(camera.orthographic) as f32,
                0.,
            ]),
        );
        self.uploader
            .queue
            .write_buffer(&self.camera, 64, bytemuck::cast_slice(&camera.clip));
        if let Some(scene) = scene {
            let [x, y, z] = scene.center;
            let scale = 0.9 * aspect.min(1.0) * camera.zoom / scene.diameter;
            let data = [
                x + camera.pan[0],
                y + camera.pan[1],
                z + camera.pan[2],
                scale,
                camera.rotation.i,
                camera.rotation.j,
                camera.rotation.k,
                camera.rotation.w,
            ];
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
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        if let Some(scene) = scene {
            self.draw_parts(&mut pass, scene, false, false, camera);
        }
        drop(pass);
        let Some(scene) = scene.filter(|scene| scene.parts.iter().any(|part| part.transparent))
        else {
            return;
        };
        let mut cache = self.transparency.borrow_mut();
        if cache.as_ref().is_none_or(|targets| targets.size != size) {
            let view = |format| {
                self.uploader
                    .device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("VTX transparency target"),
                        size: extent(size[0], size[1]),
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            };
            let accumulation = view(wgpu::TextureFormat::Rgba16Float);
            let revealage = view(wgpu::TextureFormat::R16Float);
            let group = self
                .uploader
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("VTX transparency targets"),
                    layout: &self.transparency_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&accumulation),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&revealage),
                        },
                    ],
                });
            *cache = Some(TransparencyTargets {
                size,
                accumulation,
                revealage,
                group,
            });
        }
        let targets = cache.as_ref().unwrap();
        let attachment = |view, clear| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("VTX transparent surfaces"),
            color_attachments: &[
                attachment(&targets.accumulation, wgpu::Color::TRANSPARENT),
                attachment(&targets.revealage, wgpu::Color::WHITE),
            ],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        self.draw_parts(&mut pass, scene, true, false, camera);
        drop(pass);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("VTX transparency composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.composite);
        pass.set_bind_group(0, &targets.group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn geometry_for(&self, camera: &Camera) -> &GeometryPipelines {
        let clipping = camera.clip[..3].iter().any(|&v| v != 0.);
        let variant = usize::from(clipping) + usize::from(camera.orthographic) * 2;
        if variant == 0 {
            return &self.pipelines;
        }
        self.tool_pipelines[variant - 1].get_or_init(|| {
            geometry_pipelines(
                &self.uploader.device,
                self.format,
                &self.mesh_layout,
                &self.mesh_shader,
                clipping,
                camera.orthographic,
            )
        })
    }

    fn draw_parts<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        scene: &'a Scene,
        transparent: bool,
        normal: bool,
        camera: &Camera,
    ) {
        let pipelines = self.geometry_for(camera);
        for part in scene.parts.iter().filter(|part| {
            part.transparent == transparent && !(normal && (part.points || part.lines))
        }) {
            pass.set_pipeline(if part.lines {
                &pipelines.lines
            } else if normal && part.obj_corners {
                &pipelines.geometry_obj
            } else if normal {
                &pipelines.geometry
            } else if part.obj_corners {
                &pipelines.obj_corners
            } else if part.points {
                &pipelines.points
            } else if transparent {
                &pipelines.transparent
            } else {
                &pipelines.triangles
            });
            pass.set_bind_group(0, &self.camera_group, &[]);
            pass.set_bind_group(1, &part.material, &[]);
            pass.set_vertex_buffer(0, part.positions.slice(..));
            if part.lines {
                pass.draw(0..6, 0..part.count);
                continue;
            }
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

fn extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}

/// A one-shot capture submitted on the existing live GPU. Mapping and PNG
/// encoding finish later; neither blocks the first interactive frame.
pub struct Snapshot {
    buffer: wgpu::Buffer,
    receive: std::sync::mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>,
    pub path: std::path::PathBuf,
    pub bgra: bool,
}
impl Snapshot {
    pub fn pixels(&self, device: &wgpu::Device) -> Result<Option<Vec<u8>>> {
        device.poll(wgpu::PollType::Poll)?;
        match self.receive.try_recv() {
            Ok(status) => {
                status?;
                let pixels = self.buffer.slice(..).get_mapped_range().to_vec();
                self.buffer.unmap();
                Ok(Some(pixels))
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}
impl Renderer {
    pub fn snapshot(&self, scene: &Scene, path: std::path::PathBuf) -> Result<Snapshot> {
        let (width, height) = (1920, 1440);
        let size = extent(width, height);
        let device = &self.uploader.device;
        let image = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("VTX live preview snapshot"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("VTX live snapshot readback"),
            size: u64::from(width * height * 4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        let mut camera = Camera::default();
        camera.fit(scene, width as f32 / height as f32);
        self.draw(
            &mut encoder,
            &image.create_view(&Default::default()),
            &self.depth(width, height),
            Some(scene),
            &camera,
            width as f32 / height as f32,
            [width, height],
        );
        encoder.copy_texture_to_buffer(
            image.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
            },
            size,
        );
        self.uploader.queue.submit([encoder.finish()]);
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |status| {
                let _ = send.send(status);
            });
        Ok(Snapshot {
            buffer,
            receive,
            path,
            bgra: matches!(
                self.format,
                wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
            ),
        })
    }
}

pub fn check(path: &Path, screenshot: Option<&Path>) -> Result<()> {
    offscreen(path, screenshot, 768, 512, true)
}

fn read_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("VTX UI check pixels"),
        size: u64::from(width * height * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
        },
        extent(width, height),
    );
    queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = send.send(r);
    });
    device.poll(wgpu::PollType::Wait)?;
    receive.recv()??;
    let pixels = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    Ok(pixels)
}

/// Render the actual native UI to a deterministic artifact and validate GPU
/// surface queries without desktop capture or a second graphics stack.
pub fn inspect_check(path: &Path, output: &Path, mode: &str) -> Result<()> {
    use super::{picking::Picker, ui::Ui};
    let start = Instant::now();
    let (sender, uploader) = UploadTarget::deferred();
    let import = spawn_import(path.to_path_buf(), uploader, |loaded| loaded.result)?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let renderer = initialize(|instance| {
        let adapter = adapter(instance, None)?;
        Renderer::new(&adapter, format)
    })?;
    let _ = sender.send(renderer.uploader.clone());
    let scene = import.join().map_err(|_| "UI check import failed")??;
    let (width, height) = (1280, 832);
    let device = &renderer.uploader.device;
    let queue = &renderer.uploader.queue;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("VTX UI check"),
        size: extent(width, height),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let depth = renderer.depth(width, height);
    let mut camera = Camera::default();
    camera.fit(&scene, width as f32 / height as f32);
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.draw(
        &mut encoder,
        &view,
        &depth,
        Some(&scene),
        &camera,
        width as f32 / height as f32,
        [width, height],
    );
    queue.submit([encoder.finish()]);
    let mut ui = Ui::headless(device, format);
    ui.tools.measure = mode == "measure";
    if mode == "measure" {
        ui.tools.mode = super::ui::Mode::Distance;
    }
    ui.tools.section = mode == "section";
    if mode == "ortho" {
        camera.orthographic = true;
        camera.frame(&scene, width as f32 / height as f32);
    }
    let mut picked = 0;
    if mode == "diameter"
        || mode == "cylinder"
        || mode == "edge"
        || mode == "features"
        || mode == "edgepair"
        || mode == "hover"
        || mode == "curve"
        || mode.starts_with("diameter=")
    {
        use super::{
            features::{Feature, Job},
            ui::{Mode, PickContext},
        };
        let job =
            Job::start(&scene, device, queue).map_err(|e| format!("Feature analysis: {e}"))?;
        let index = loop {
            if let Some(result) = job.poll() {
                break result.map_err(|e| format!("Feature analysis: {e}"))?;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        let rows:Vec<_>=index.features.iter().map(|f|match f {
            Feature::Circle(c)=>serde_json::json!({"kind":"circle","radius":c.radius,"center":c.center,"sweep":c.sweep,"exact":c.exact,"residual":c.deviation,"samples":c.samples}),
            Feature::Cylinder{rim,height}=>serde_json::json!({"kind":"cylinder","radius":rim.radius,"center":rim.center,"height":height}),
            Feature::Line{a,b}=>serde_json::json!({"kind":"line","a":a,"b":b}),
            Feature::Curve{points}=>serde_json::json!({"kind":"curve","points":points,"length":f.length()}),
        }).collect();
        std::fs::write(
            output.with_extension("features.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({"analysis_ms":index.elapsed_ms,"features":rows}),
            )?,
        )?;
        if mode == "edgepair" || mode == "hover" || mode == "curve" {
            use super::edges::{visible, ScreenEdges};
            let cache = ScreenEdges::new(&index, &camera, &scene, [width, height]);
            let mut ids: Vec<_> = index
                .features
                .iter()
                .enumerate()
                .filter_map(|(i, f)| {
                    let eligible = if mode == "curve" {
                        matches!(f, Feature::Curve { .. })
                    } else {
                        matches!(f, Feature::Line { .. })
                    };
                    eligible.then_some(i)
                })
                .collect();
            ids.sort_by(|&a, &b| {
                index.features[b]
                    .length()
                    .total_cmp(&index.features[a].length())
            });
            let query_started = Instant::now();
            for i in 0..1000u32 {
                let at = [(i * 109 + 17) % width, (i * 191 + 31) % height];
                let _ = cache.nearest(&index, at, 10., false, &scene, &camera);
            }
            let query_us = query_started.elapsed().as_secs_f64() * 1000.;
            ui.tools.mode = Mode::Edge;
            ui.tools.edge_discovery = true;
            ui.tools.set_index(index, &scene);
            let picker = Picker::new(&renderer);
            for id in ids {
                let index = ui.tools.index.as_ref().unwrap();
                let feature = &index.features[id];
                let paths = feature.paths();
                let path = &paths[0];
                let p = if let Feature::Line { a, b } = feature {
                    [0, 1, 2].map(|i| (a[i] + b[i]) * 0.5)
                } else {
                    path[path.len() / 2]
                };
                let q = match camera.project(p.map(|v| v as f32), &scene, [width, height]) {
                    Some(p) => p,
                    None => continue,
                };
                if q[0] < 8. || q[0] > 940. || q[1] < 60. || q[1] > 790. {
                    continue;
                }
                let at = [q[0] as u32, q[1] as u32];
                let candidate = match cache.nearest(index, at, 10., false, &scene, &camera) {
                    Some(c) => c,
                    None => continue,
                };
                if mode == "curve"
                    && !matches!(&index.features[candidate.id], Feature::Curve { .. })
                {
                    continue;
                }
                let feature = index.features[candidate.id].clone();
                if ui.tools.selected_edges.contains(&feature) {
                    continue;
                }
                let pending = picker.begin(&renderer, &scene, [width, height], candidate.at);
                device.poll(wgpu::PollType::Wait)?;
                let hit = pending.poll(device)?.ok_or("Edge query not ready")?;
                if !visible(&candidate, hit, &camera, &scene) {
                    continue;
                }
                picked += 1;
                if mode == "hover" {
                    ui.tools.hovered = Some(feature);
                    break;
                }
                ui.tools.select_edge(Some(feature), picked > 1);
                if mode == "curve" || ui.tools.selected_edges.len() == 2 {
                    break;
                }
            }
            if picked == 0 || (mode == "edgepair" && ui.tools.selected_edges.len() != 2) {
                return Err("Could not select visible edges in native UI check".into());
            }
            let pair = ui.tools.pair.as_ref();
            std::fs::write(
                output.with_extension("selection.json"),
                serde_json::to_vec_pretty(
                    &serde_json::json!({"screen_build_ms":cache.build_ms,"query_mean_us":query_us,"selected":ui.tools.selected_edges.len(),"hovered":ui.tools.hovered.is_some(),"gap":pair.map(|p|p.distance),"angle":pair.and_then(|p|p.angle),"curved":pair.map(|p|p.curved)}),
                )?,
            )?;
            println!(
                "screen_build_ms={:.3} query_mean_us={query_us:.3}",
                cache.build_ms
            );
        } else if mode != "features" {
            ui.tools.measure = true;
            ui.tools.mode = if mode == "edge" {
                Mode::Edge
            } else {
                Mode::Diameter
            };
            let target = mode
                .strip_prefix("diameter=")
                .and_then(|s| s.parse::<f64>().ok());
            let mut candidates: Vec<_> = index
                .features
                .iter()
                .filter(|f| {
                    if mode == "edge" {
                        matches!(f, Feature::Line { .. })
                    } else if mode == "cylinder" {
                        matches!(f, Feature::Cylinder { .. })
                    } else {
                        matches!(f, Feature::Circle(_))
                    }
                })
                .cloned()
                .collect();
            candidates.sort_by(|a, b| {
                let score = |f: &Feature| {
                    if mode == "edge" {
                        -f.length()
                    } else {
                        let c = f.circle().unwrap();
                        target.map_or(-c.radius, |t| (c.radius * 2. - t).abs())
                    }
                };
                score(a).total_cmp(&score(b))
            });
            ui.tools.set_index(index, &scene);
            let picker = Picker::new(&renderer);
            'search: for candidate in candidates {
                let paths = match &candidate {
                    Feature::Cylinder { rim, height } => {
                        let mut c = rim.clone();
                        let normal = c.normal();
                        for i in 0..3 {
                            c.center[i] += normal[i] * height * 0.5;
                        }
                        vec![c.polyline()]
                    }
                    Feature::Line { a, b } => vec![vec![[0, 1, 2].map(|i| (a[i] + b[i]) * 0.5)]],
                    _ => candidate.paths(),
                };
                for p in paths
                    .iter()
                    .flat_map(|p| p.iter())
                    .step_by(if mode == "edge" { 1 } else { 3 })
                {
                    let q = match camera.project(p.map(|v| v as f32), &scene, [width, height]) {
                        Some(q) => q,
                        None => continue,
                    };
                    if q[0] < 5. || q[0] > 945. || q[1] < 60. || q[1] > 800. {
                        continue;
                    }
                    for offset in [[0i32, 0], [1, 0], [-1, 0], [0, 1], [0, -1]] {
                        let at = [
                            (q[0] as i32 + offset[0]) as u32,
                            (q[1] as i32 + offset[1]) as u32,
                        ];
                        let pending = picker.begin(&renderer, &scene, [width, height], at);
                        device.poll(wgpu::PollType::Wait)?;
                        let hit = pending.poll(device)?.ok_or("Feature query not ready")?;
                        if hit.is_none() {
                            continue;
                        }
                        ui.tools.picked_at(
                            hit,
                            &PickContext {
                                camera: camera.clone(),
                                size: [width, height],
                                at,
                                serial: ui.tools.serial,
                                additive: false,
                            },
                            &scene,
                        );
                        if let Some(selected) = &ui.tools.selected {
                            let matched = if mode == "edge" {
                                (selected.length() - candidate.length()).abs()
                                    < candidate.length() * 0.001
                            } else {
                                (selected.circle().unwrap().radius
                                    - candidate.circle().unwrap().radius)
                                    .abs()
                                    < candidate.circle().unwrap().radius * 0.001
                            };
                            let matched = matched
                                && (mode != "cylinder"
                                    || matches!(selected, Feature::Cylinder { .. }));
                            if matched {
                                picked += 1;
                                break 'search;
                            }
                        }
                    }
                }
            }
            if picked == 0 {
                return Err("No visible recognized feature could be selected".into());
            }
        }
    }
    if mode == "measure" {
        let coverage = read_rgba(
            device,
            queue,
            &renderer
                .style_targets
                .borrow()
                .as_ref()
                .unwrap()
                .scene_texture,
            width,
            height,
        )?;
        let pixels: Vec<_> = coverage
            .chunks_exact(4)
            .enumerate()
            .filter_map(|(i, p)| (p[3] > 16).then(|| [(i as u32) % width, (i as u32) / width]))
            .collect();
        if pixels.len() < 2 {
            return Err("No measurable geometry in UI check".into());
        }
        let picker = Picker::new(&renderer);
        for q in [1usize, 2] {
            let at = pixels[pixels.len() * q / 3];
            let pending = picker.begin(&renderer, &scene, [width, height], at);
            device.poll(wgpu::PollType::Wait)?;
            let hit = pending
                .poll(device)?
                .ok_or("Surface readback did not finish")?
                .ok_or("Surface query missed visible geometry")?;
            let screen = camera
                .project(hit, &scene, [width, height])
                .ok_or("Picked point did not project")?;
            if (screen[0] - at[0] as f32 - 0.5).abs() > 1.
                || (screen[1] - at[1] as f32 - 0.5).abs() > 1.
            {
                return Err(
                    format!("Surface query reprojection error: {screen:?} at {at:?}").into(),
                );
            }
            ui.tools.picked(Some(hit));
            picked += 1;
        }
        if ui.tools.result(&scene).is_none() {
            return Err("Measurement result missing".into());
        }
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    for _ in 0..3 {
        let frame = ui.begin(None, Some(&scene), &mut camera, [width, height], &name);
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.draw(
            &mut encoder,
            &view,
            &depth,
            Some(&scene),
            &camera,
            width as f32 / height as f32,
            [width, height],
        );
        ui.paint(device, queue, &mut encoder, &view, [width, height], &frame);
        queue.submit([encoder.finish()]);
    }
    let pixels = read_rgba(device, queue, &texture, width, height)?;
    image::save_buffer(output, &pixels, width, height, image::ColorType::Rgba8)?;
    println!(
        "ui_mode={mode} gpu_picks={picked} result={} total_ms={:.2}",
        ui.tools.result(&scene).unwrap_or_else(|| "none".into()),
        start.elapsed().as_secs_f64() * 1000.
    );
    Ok(())
}

pub fn preview(path: &Path, screenshot: &Path, allow_incomplete: bool) -> Result<()> {
    offscreen(path, Some(screenshot), 1920, 1440, allow_incomplete)
}

fn offscreen(
    path: &Path,
    screenshot: Option<&Path>,
    width: u32,
    height: u32,
    allow_incomplete: bool,
) -> Result<()> {
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
    let size = extent(width, height);
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
    let depth = renderer.depth(width, height);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("VTX check readback"),
        size: u64::from(width * height * 4),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let coverage = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("VTX check model coverage"),
        size: u64::from(width * height * 4),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let initialized = init.elapsed();
    let start = Instant::now();
    let _ = sender.send(renderer.uploader.clone());
    let scene = import.join().map_err(|_| "Model import thread failed")??;
    if !allow_incomplete && scene.retain_saved_preview {
        return Err(
            "Retaining the saved SolidWorks preview: some decals could not be mapped completely."
                .into(),
        );
    }
    let loaded = start.elapsed();
    let mut encoder = device.create_command_encoder(&Default::default());
    let mut camera = Camera::default();
    camera.fit(&scene, width as f32 / height as f32);
    renderer.draw(
        &mut encoder,
        &view,
        &depth,
        Some(&scene),
        &camera,
        width as f32 / height as f32,
        [width, height],
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
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
        },
        size,
    );
    encoder.copy_texture_to_buffer(
        renderer
            .style_targets
            .borrow()
            .as_ref()
            .unwrap()
            .scene_texture
            .as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &coverage,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
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
    let pixels = slice.get_mapped_range();
    let coverage_slice = coverage.slice(..);
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    coverage_slice.map_async(wgpu::MapMode::Read, move |status| {
        let _ = send.send(status);
    });
    device.poll(wgpu::PollType::Wait)?;
    receive.recv()??;
    let visible = coverage_slice
        .get_mapped_range()
        .chunks_exact(4)
        .filter(|pixel| pixel[3] != 0)
        .count();
    if visible == 0 {
        return Err("The GPU rendered only the background".into());
    }
    if let Some(path) = screenshot {
        image::save_buffer(path, &pixels, width, height, image::ColorType::Rgba8)?;
    }
    let ms = |time: std::time::Duration| time.as_secs_f64() * 1000.0;
    let info = adapter.get_info();
    println!("backend={:?} init_ms={:.3} ready_ms={:.3} upload_ms={:.3} submit_ms={:.3} gpu_done_ms={:.3} first_frame_ms={:.3} gpu_wait_ms={:.3} triangles={} points={} pixels={} issues={} lines={}",
        info.backend, ms(initialized), ms(loaded), scene.upload_ms, ms(submitted), ms(completed),
        ms(initialized + completed), scene.gpu_wait_ms,
        scene.triangles, scene.points, visible, scene.issues, scene.lines);
    eprintln!("GPU: {}", info.name);
    for detail in &scene.diagnostics {
        eprintln!("VTX: {detail}");
    }
    Ok(())
}
