// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

use crate::Placement;
use masonry_core::kurbo::Rect;
use wgpu::util::DeviceExt;

use masonry_imaging::AlphaMode;

/// Persistent RGBA8 content, in physical pixels.
#[derive(Debug)]
pub struct Surface {
    device: wgpu::Device,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    binding: wgpu::BindGroup,
    alpha_mode: AlphaMode,
}

impl Surface {
    pub(crate) fn belongs_to(&self, device: &wgpu::Device) -> bool {
        &self.device == device
    }

    /// Texture for the host's own GPU rendering or copying.
    #[must_use = "Use the retained texture."]
    pub const fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// View suitable for a render or compute target. Content must use the alpha representation specified at allocation.
    #[must_use = "Use the retained texture view."]
    pub const fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Alpha representation of the texture content.
    #[must_use = "Use the alpha representation."]
    pub const fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    /// Physical pixel extent.
    #[must_use = "Use the physical extent."]
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
    #[must_use = "Use the placed layer."]
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
    device: wgpu::Device,
    vertices: Option<wgpu::Buffer>,
    bindings: Vec<(std::ops::Range<u32>, wgpu::BindGroup)>,
}

/// A failure to prepare or draw a retained composition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompositionError {
    /// A visible layer or total vertex buffer exceeds the host device's limits.
    InvalidExtent,
    /// A surface or prepared composition belongs to a different WGPU device.
    DifferentDevice,
}

impl std::fmt::Display for CompositionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidExtent => "composition exceeds device limits",
            Self::DifferentDevice => "composition resource belongs to a different device",
        })
    }
}

impl std::error::Error for CompositionError {}

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
    #[must_use = "Use the compositor."]
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
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 20,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
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

    pub(crate) const fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Allocate a persistent texture compatible with Vello and native GPU producers.
    ///
    /// Returns `None` for empty or unsupported extents.
    #[must_use = "Use the allocated surface or handle an unsupported extent."]
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
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
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
            device: self.device.clone(),
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
    ///
    /// # Errors
    /// Returns an error for surfaces from another device, vertex index overflow,
    /// allocation failure, or a vertex buffer larger than the device supports.
    pub fn prepare(
        &self,
        extent: [u32; 2],
        layers: &[Layer<'_>],
    ) -> Result<Composition, CompositionError> {
        let mut vertices = Vec::<[f32; 5]>::new();
        let mut bindings = Vec::new();
        let max_buffer_size = self.device.limits().max_buffer_size;
        let mut first = 0_u32;
        for layer in layers {
            if !layer.surface.belongs_to(&self.device) {
                return Err(CompositionError::DifferentDevice);
            }
            let quad = layer.placement.vertices(extent);
            if quad.is_empty() {
                continue;
            }
            let count = u32::try_from(quad.len()).map_err(|_| CompositionError::InvalidExtent)?;
            let end = first
                .checked_add(count)
                .ok_or(CompositionError::InvalidExtent)?;
            if u64::from(end) * 20 > max_buffer_size {
                return Err(CompositionError::InvalidExtent);
            }
            vertices
                .try_reserve(quad.len())
                .map_err(|_| CompositionError::InvalidExtent)?;
            bindings
                .try_reserve(1)
                .map_err(|_| CompositionError::InvalidExtent)?;
            let straight = f32::from(u8::from(layer.surface.alpha_mode == AlphaMode::Straight));
            vertices.extend(quad.into_iter().map(|[x, y, u, v]| [x, y, u, v, straight]));
            bindings.push((first..end, layer.surface.binding.clone()));
            first = end;
        }
        let vertices = (!vertices.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("yarugc retained placement"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });
        Ok(Composition {
            device: self.device.clone(),
            vertices,
            bindings,
        })
    }

    /// Draw cached layers over the current host frame. The host controls load/store
    /// operations, submission and presentation. The pass must have the full target
    /// viewport and scissor, use the same device, and match the compositor format.
    ///
    /// # Errors
    /// Returns an error before modifying the pass if the composition was
    /// prepared on a different device.
    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        composition: &Composition,
    ) -> Result<(), CompositionError> {
        if self.device != composition.device {
            return Err(CompositionError::DifferentDevice);
        }
        let Some(vertices) = &composition.vertices else {
            return Ok(());
        };
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, vertices.slice(..));
        for (range, binding) in &composition.bindings {
            pass.set_bind_group(0, binding, &[]);
            pass.draw(range.clone(), 0..1);
        }
        Ok(())
    }
}
