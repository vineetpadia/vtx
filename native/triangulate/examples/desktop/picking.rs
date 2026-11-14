//! On-demand, asynchronous surface picking. No CPU mesh copy or startup index.
use super::{model::Scene, render::Renderer, Result};
use std::convert::TryInto;
pub struct Pending {
    buffer: wgpu::Buffer,
    receive: std::sync::mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>,
}
impl Pending {
    pub fn poll(&self, device: &wgpu::Device) -> Result<Option<Option<[f32; 3]>>> {
        device.poll(wgpu::PollType::Poll)?;
        match self.receive.try_recv() {
            Ok(status) => {
                status?;
                let view = self.buffer.slice(..).get_mapped_range();
                let values: Vec<f32> = view[..16]
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                    .collect();
                let point = (values[3] > 0.5 && values[..3].iter().all(|v| v.is_finite()))
                    .then(|| [values[0], values[1], values[2]]);
                drop(view);
                self.buffer.unmap();
                Ok(Some(point))
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
pub struct Picker {
    triangles: wgpu::RenderPipeline,
    points: wgpu::RenderPipeline,
    obj: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    group: wgpu::BindGroup,
    image: wgpu::Texture,
    depth: wgpu::TextureView,
}
impl Picker {
    pub fn new(renderer: &Renderer) -> Self {
        let device = &renderer.uploader.device;
        let settings = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VTX pick position"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("VTX surface pick"),
            bind_group_layouts: &[
                &renderer.camera_layout,
                &renderer.uploader.layout,
                &settings,
            ],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("VTX pick shader"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}",
                    include_str!("mesh.wgsl"),
                    include_str!("picking.wgsl")
                )
                .into(),
            ),
        });
        let pipeline = |entry, fragment, topology, buffers: &[wgpu::VertexBufferLayout<'_>]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("VTX pick"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &[("CLIPPING", 1.), ("PROJECTION", 2.)],
                        ..Default::default()
                    },
                    buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth24PlusStencil8,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &[("CLIPPING", 1.), ("PROJECTION", 2.)],
                        ..Default::default()
                    },
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba32Float,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            })
        };
        let mesh = [
            wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0=>Float32x3],
            },
            wgpu::VertexBufferLayout {
                array_stride: 48,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![1=>Float32x4,2=>Float32x4,3=>Float32x4],
            },
        ];
        let triangles = pipeline(
            "pick_vertex",
            "pick_surface",
            wgpu::PrimitiveTopology::TriangleList,
            &mesh,
        );
        let points = pipeline(
            "pick_vertex",
            "pick_point",
            wgpu::PrimitiveTopology::PointList,
            &mesh,
        );
        let obj = pipeline(
            "pick_obj",
            "pick_surface",
            wgpu::PrimitiveTopology::TriangleList,
            &[wgpu::VertexBufferLayout {
                array_stride: 8,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0=>Uint32x2],
            }],
        );
        let lines = pipeline(
            "pick_line",
            "pick_stroke",
            wgpu::PrimitiveTopology::TriangleList,
            &[wgpu::VertexBufferLayout {
                array_stride: 28,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![0=>Float32x3,4=>Float32x3,5=>Unorm8x4],
            }],
        );
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &settings,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let image = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("VTX one-pixel surface pick"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self {
            triangles,
            points,
            obj,
            lines,
            uniform,
            group,
            image,
            depth: renderer.depth(1, 1),
        }
    }
    pub fn begin(
        &self,
        renderer: &Renderer,
        scene: &Scene,
        size: [u32; 2],
        at: [u32; 2],
    ) -> Pending {
        let device = &renderer.uploader.device;
        let queue = &renderer.uploader.queue;
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::cast_slice(&[
                (at[0] as f32 + 0.5) / size[0] as f32 * 2. - 1.,
                1. - (at[1] as f32 + 0.5) / size[1] as f32 * 2.,
                size[0] as f32,
                size[1] as f32,
            ]),
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        let view = self.image.create_view(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("VTX surface query"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &renderer.camera_group, &[]);
            pass.set_bind_group(2, &self.group, &[]);
            for part in &scene.parts {
                pass.set_bind_group(1, &part.material, &[]);
                pass.set_vertex_buffer(0, part.positions.slice(..));
                pass.set_pipeline(if part.lines {
                    &self.lines
                } else if part.obj_corners {
                    &self.obj
                } else if part.points {
                    &self.points
                } else {
                    &self.triangles
                });
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
                    .map_or((&renderer.identity, 1), |batch| {
                        (&batch.transforms, batch.count)
                    });
                pass.set_vertex_buffer(1, transforms.slice(..));
                if let Some(indices) = &part.indices {
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..part.count, 0, 0..count);
                } else {
                    pass.draw(0..part.count, 0..count);
                }
            }
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("VTX picked point"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            self.image.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = send.send(result);
            });
        Pending { buffer, receive }
    }
}

#[cfg(test)]
mod shader_tests {
    #[test]
    fn inspection_shaders_validate() {
        let picking = format!(
            "{}\n{}",
            include_str!("mesh.wgsl"),
            include_str!("picking.wgsl")
        );
        for source in [
            include_str!("mesh.wgsl"),
            picking.as_str(),
            include_str!("ui_gpu.wgsl"),
        ] {
            let module = wgpu::naga::front::wgsl::parse_str(source).unwrap();
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap();
        }
    }
}
