// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

use crate::Placement;
use masonry_core::kurbo::Rect;
use wgpu::util::DeviceExt;

/// Alpha representation of a retained RGBA8 texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaMode {
    /// RGB already multiplied by alpha.
    Premultiplied,
    /// RGB independent of alpha (Vello texture output).
    Straight,
}

/// Persistent RGBA8 content, in physical pixels.
#[derive(Debug)]
pub struct Surface {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    binding: wgpu::BindGroup,
    alpha_mode: AlphaMode,
}

impl Surface {
    /// Texture for the host's own GPU rendering or copying.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// View suitable for a render or compute target. Content must use the alpha representation specified at allocation.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Alpha representation of the texture content.
    pub fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    /// Physical pixel extent.
    pub fn size(&self) -> [u32; 2] {
        [self.texture.width(), self.texture.height()]
    }
}

/// Placement of a retained surface, in target physical pixels.
#[derive(Clone, Debug)]
pub struct Layer<'a> {
    /// Surface to composite.
    pub surface: &'a Surface,
    /// Affine placement and clip in target coordinates.
    pub placement: Placement,
}

impl<'a> Layer<'a> {
    /// Place at the texture's native pixel size.
    pub fn at(surface: &'a Surface, [x, y]: [f64; 2]) -> Self {
        let [w, h] = surface.size().map(f64::from);
        Self {
            surface,
            placement: Placement::rect(Rect::new(x, y, x + w, y + h)),
        }
    }
}

/// Prepared painter-order composition. Rebuild only when placement, target size,
/// layer ordering, or surface allocation changes. Texture content can change in place.
#[derive(Debug)]
pub struct Composition {
    vertices: Option<wgpu::Buffer>,
    bindings: Vec<(std::ops::Range<u32>, wgpu::BindGroup)>,
}

/// Composites retained surfaces into a render pass owned by the host.
#[derive(Debug)]
pub struct Compositor {
    device: wgpu::Device,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
}

impl Compositor {
    /// Create a compositor for the host device and final render target format.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yarugc layer layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("yarugc layer sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yarugc compositor shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("yarugc compositor pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("yarugc compositor"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 20,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            device: device.clone(),
            layout,
            sampler,
            pipeline,
        }
    }

    pub(crate) fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Allocate a persistent texture compatible with Vello and native GPU producers.
    ///
    /// Returns `None` for empty or unsupported extents.
    pub fn create_surface(
        &self,
        [width, height]: [u32; 2],
        alpha_mode: AlphaMode,
    ) -> Option<Surface> {
        let max = self.device.limits().max_texture_dimension_2d;
        if width == 0 || height == 0 || width > max || height > max {
            return None;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("yarugc retained surface"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let binding = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yarugc retained surface binding"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Some(Surface {
            texture,
            view,
            binding,
            alpha_mode,
        })
    }

    /// Prepare visible quads in painter order. Offscreen layers are omitted.
    ///
    /// The prepared composition retains GPU bindings, so a replaced surface requires
    /// rebuilding this composition. Ordinary content updates do not.
    pub fn prepare(&self, extent: [u32; 2], layers: &[Layer<'_>]) -> Composition {
        let mut bytes = Vec::new();
        let mut bindings = Vec::new();
        let mut first = 0_u32;
        for layer in layers {
            let vertices = layer.placement.vertices(extent);
            if !vertices.is_empty() {
                let count = u32::try_from(vertices.len()).expect("vertex count fits u32");
                let end = first.checked_add(count).expect("vertex index fits u32");
                for vertex in vertices {
                    for value in vertex {
                        bytes.extend_from_slice(&value.to_ne_bytes());
                    }
                    let straight = if layer.surface.alpha_mode == AlphaMode::Straight {
                        1.0_f32
                    } else {
                        0.0_f32
                    };
                    bytes.extend_from_slice(&straight.to_ne_bytes());
                }
                bindings.push((first..end, layer.surface.binding.clone()));
                first = end;
            }
        }
        let vertices = (!bytes.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("yarugc retained placement"),
                    contents: &bytes,
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });
        Composition { vertices, bindings }
    }

    /// Draw cached layers over the current host frame. The host controls load/store
    /// operations, submission and presentation. The pass must have the full target
    /// viewport and scissor and match the compositor format.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, composition: &Composition) {
        let Some(vertices) = &composition.vertices else {
            return;
        };
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, vertices.slice(..));
        for (range, binding) in &composition.bindings {
            pass.set_bind_group(0, binding, &[]);
            pass.draw(range.clone(), 0..1);
        }
    }
}
