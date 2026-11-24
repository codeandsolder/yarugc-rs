// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

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

    /// Physical pixel extent.
    pub fn size(&self) -> [u32; 2] {
        [self.texture.width(), self.texture.height()]
    }
}

/// Placement of a retained surface, in target physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct Layer<'a> {
    /// Surface to composite.
    pub surface: &'a Surface,
    /// Top-left position; may be negative or outside the viewport.
    pub origin: [f32; 2],
}

/// Prepared painter-order composition. Rebuild only when placement, target size,
/// layer ordering, or surface allocation changes. Texture content can change in place.
#[derive(Debug)]
pub struct Composition {
    vertices: Option<wgpu::Buffer>,
    bindings: Vec<wgpu::BindGroup>,
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
        for layer in layers {
            if let Some(vertices) = quad(extent, layer.origin, layer.surface.size()) {
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
                bindings.push(layer.surface.binding.clone());
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
        for (index, binding) in composition.bindings.iter().enumerate() {
            let first = u32::try_from(index)
                .expect("layer index fits u32")
                .checked_mul(6)
                .expect("layer vertex index fits u32");
            pass.set_bind_group(0, binding, &[]);
            pass.draw(first..first + 6, 0..1);
        }
    }
}

fn quad(extent: [u32; 2], origin: [f32; 2], size: [u32; 2]) -> Option<[[f32; 4]; 6]> {
    let [width, height] = extent.map(|v| v as f32);
    let [x, y] = origin;
    let [w, h] = size.map(|v| v as f32);
    if width == 0.0
        || height == 0.0
        || !x.is_finite()
        || !y.is_finite()
        || w == 0.0
        || h == 0.0
        || x >= width
        || y >= height
        || x + w <= 0.0
        || y + h <= 0.0
    {
        return None;
    }
    let left = x.max(0.0);
    let top = y.max(0.0);
    let right = (x + w).min(width);
    let bottom = (y + h).min(height);
    let vertex = |px: f32, py: f32| {
        [
            2.0 * px / width - 1.0,
            1.0 - 2.0 * py / height,
            (px - x) / w,
            (py - y) / h,
        ]
    };
    let a = vertex(left, top);
    let b = vertex(right, top);
    let c = vertex(left, bottom);
    let d = vertex(right, bottom);
    Some([a, c, b, b, c, d])
}

#[cfg(test)]
mod tests {
    use super::quad;

    #[test]
    fn clipped_window_preserves_texture_coordinates() {
        let q = quad([100, 100], [-25.0, 50.0], [50, 100]).unwrap();
        assert_eq!(q[0], [-1.0, 0.0, 0.5, 0.0]);
        assert_eq!(q[5], [-0.5, -1.0, 1.0, 0.5]);
        assert!(quad([100, 100], [100.0, 0.0], [1, 1]).is_none());
    }
}
