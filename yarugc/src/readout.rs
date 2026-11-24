// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use ab_glyph::{Font, FontRef, ScaleFont, point};
use masonry_core::kurbo::Rect;
use wgpu::util::DeviceExt;

use crate::{AlphaMode, Compositor, SlotGeometry, Surface};

/// Configuration of a fixed-width numerical readout. Glyph cells are right
/// aligned and vertically centered; oversized content is clipped locally.
#[derive(Clone, Debug)]
pub struct ReadoutSlot {
    /// Resolved widget geometry, independent of text content.
    pub geometry: SlotGeometry,
    /// Maximum number of glyph cells.
    pub cells: usize,
    /// Straight RGBA tint, in the compositor's color space.
    pub color: [f32; 4],
}

/// Errors in readout atlas, geometry or value preparation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadoutError {
    /// The font could not be parsed.
    InvalidFont,
    /// Unsupported font size or resource extent.
    InvalidExtent,
    /// The configured alphabet contains a character missing from the font.
    MissingGlyph(char),
    /// A text update used a character not present in the atlas.
    UnknownCharacter(char),
    /// The text is longer than its fixed slot.
    TextTooLong,
    /// The specified slot does not exist.
    UnknownSlot,
    /// The compositor must use the same device as the renderer.
    DifferentDevice,
}

impl std::fmt::Display for ReadoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFont => f.write_str("invalid font"),
            Self::InvalidExtent => f.write_str("invalid glyph atlas or readout extent"),
            Self::MissingGlyph(c) => write!(f, "font does not contain {c:?}"),
            Self::UnknownCharacter(c) => write!(f, "readout atlas does not contain {c:?}"),
            Self::TextTooLong => f.write_str("readout exceeds its fixed cell capacity"),
            Self::UnknownSlot => f.write_str("unknown readout slot"),
            Self::DifferentDevice => f.write_str("readout compositor uses a different device"),
        }
    }
}

impl std::error::Error for ReadoutError {}

/// Shared glyph atlas and GPU pipeline for fixed-width readouts. This is a
/// purpose-built scalar telemetry renderer, not a general Unicode shaping engine.
/// Ordinary rich, bidi and editable text should use Masonry/Parley.
#[derive(Debug)]
pub struct ReadoutRenderer {
    device: wgpu::Device,
    glyphs: Arc<GlyphMap>,
    queue: wgpu::Queue,
    cell_size: [u32; 2],
    atlas: wgpu::TextureView,
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
}

/// Persistent readout geometry and glyph data for a single dynamic layer.
/// Set values and render only when they change; no new GPU mesh is generated.
#[derive(Debug)]
pub struct ReadoutBatch {
    surface: Surface,
    glyphs: Arc<GlyphMap>,
    pipeline: wgpu::RenderPipeline,
    queue: wgpu::Queue,
    ranges: Vec<Range<usize>>,
    values: Vec<u32>,
    dirty: Option<Range<usize>>,
    vertices: Option<wgpu::Buffer>,
    vertex_count: u32,
    glyph_buffer: wgpu::Buffer,
    binding: wgpu::BindGroup,
}

impl ReadoutRenderer {
    /// Rasterize a caller-selected font and alphabet once into a texture array.
    /// Space and empty cells are transparent; other characters must be in the font.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font: &[u8],
        pixel_height: f32,
        alphabet: impl IntoIterator<Item = char>,
    ) -> Result<Self, ReadoutError> {
        let atlas = rasterize(font, pixel_height, alphabet, &device.limits())?;
        let [width, height] = atlas.cell_size;
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: atlas.layers,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("yarugc glyph atlas"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &atlas.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: Some(height),
            },
            extent,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yarugc readout layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yarugc readout shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("readout.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("yarugc readout pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("yarugc readout pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 36,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Uint32, 3 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
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
        Ok(Self {
            device: device.clone(),
            glyphs: Arc::new(GlyphMap::new(atlas.glyphs)),
            queue: queue.clone(),
            cell_size: atlas.cell_size,
            atlas: view,
            sampler,
            layout,
            pipeline,
        })
    }

    /// Glyph cell dimensions in pixels.
    pub fn cell_size(&self) -> [u32; 2] {
        self.cell_size
    }

    /// Prepare fixed readout geometry. Repeat only after layout/DPI changes.
    /// All cells of all slots are rendered in a single draw call.
    pub fn prepare(
        &self,
        compositor: &Compositor,
        extent: [u32; 2],
        slots: &[ReadoutSlot],
    ) -> Result<ReadoutBatch, ReadoutError> {
        if compositor.device() != &self.device {
            return Err(ReadoutError::DifferentDevice);
        }
        let limits = self.device.limits();
        if extent.contains(&0) || extent.iter().any(|&v| v > limits.max_texture_dimension_2d) {
            return Err(ReadoutError::InvalidExtent);
        }
        let mut ranges = Vec::with_capacity(slots.len());
        let mut count = 0_usize;
        let mut vertices = Vec::new();
        for slot in slots {
            let end = count
                .checked_add(slot.cells)
                .ok_or(ReadoutError::InvalidExtent)?;
            let start_u32 = u32::try_from(count).map_err(|_| ReadoutError::InvalidExtent)?;
            let end_u32 = u32::try_from(end).map_err(|_| ReadoutError::InvalidExtent)?;
            if u64::from(end_u32) * 4 > u64::from(limits.max_storage_buffer_binding_size) {
                return Err(ReadoutError::InvalidExtent);
            }
            let [w, h] = slot.geometry.logical_size();
            let [cell_w, cell_h] = self.cell_size.map(f64::from);
            let scale = slot.geometry.scale_factor();
            let dx = cell_w / (w * scale);
            let dy = cell_h / (h * scale);
            let top = (1.0 - dy) * 0.5;
            let mut left = 1.0 - f64::from(end_u32 - start_u32) * dx;
            for cell in start_u32..end_u32 {
                let placement =
                    slot.geometry
                        .placement()
                        .subrect(Rect::new(left, top, left + dx, top + dy));
                for vertex in placement.vertices(extent) {
                    vertices.extend_from_slice(bytemuck::cast_slice(&vertex));
                    vertices.extend_from_slice(&cell.to_ne_bytes());
                    vertices.extend_from_slice(bytemuck::cast_slice(&slot.color));
                }
                left += dx;
            }
            ranges.push(count..end);
            count = end;
        }
        let vertex_count =
            u32::try_from(vertices.len() / 36).map_err(|_| ReadoutError::InvalidExtent)?;
        if u64::try_from(vertices.len()).unwrap_or(u64::MAX) > limits.max_buffer_size {
            return Err(ReadoutError::InvalidExtent);
        }
        let vertex_buffer = (!vertices.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("yarugc readout geometry"),
                    contents: &vertices,
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });
        let values = vec![0_u32; count.max(1)];
        let glyph_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("yarugc readout glyph values"),
                contents: bytemuck::cast_slice(&values),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        let binding = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yarugc readout batch"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.atlas),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: glyph_buffer.as_entire_binding(),
                },
            ],
        });
        Ok(ReadoutBatch {
            surface: compositor
                .create_surface(extent, AlphaMode::Premultiplied)
                .ok_or(ReadoutError::InvalidExtent)?,
            glyphs: Arc::clone(&self.glyphs),
            pipeline: self.pipeline.clone(),
            queue: self.queue.clone(),
            ranges,
            dirty: Some(0..0),
            values,
            vertices: vertex_buffer,
            vertex_count,
            glyph_buffer,
            binding,
        })
    }
}

impl ReadoutBatch {
    /// Cached dynamic surface; compose it after the static window content.
    pub fn surface(&self) -> &Surface {
        &self.surface
    }

    /// Change a slot's text without allocation, layout, or geometry generation.
    /// Invalid input leaves the previous value intact.
    pub fn set_text(&mut self, slot: usize, text: &str) -> Result<bool, ReadoutError> {
        let range = self
            .ranges
            .get(slot)
            .ok_or(ReadoutError::UnknownSlot)?
            .clone();
        let len = text.chars().count();
        if len > range.len() {
            return Err(ReadoutError::TextTooLong);
        }
        for ch in text.chars() {
            if self.glyphs.get(ch).is_none() {
                return Err(ReadoutError::UnknownCharacter(ch));
            }
        }
        let glyphs = std::iter::repeat_n(0, range.len() - len).chain(
            text.chars()
                .map(|ch| self.glyphs.get(ch).expect("validated glyph")),
        );
        let mut changed: Option<Range<usize>> = None;
        for (index, glyph) in range.zip(glyphs) {
            if self.values[index] != glyph {
                self.values[index] = glyph;
                changed = Some(match changed {
                    Some(range) => range.start..index + 1,
                    None => index..index + 1,
                });
            }
        }
        if let Some(range) = changed {
            self.mark_dirty(range);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Upload changed glyph IDs and redraw the dynamic layer in one draw call.
    /// Returns false for an unchanged batch. Submit the encoder before changing
    /// its values again; the caller owns frame scheduling and submission.
    /// If the encoder is discarded, call [`Self::invalidate_surface`] before retrying.
    pub fn render(&mut self, encoder: &mut wgpu::CommandEncoder) -> bool {
        let Some(dirty) = self.dirty.take() else {
            return false;
        };
        if !dirty.is_empty() {
            self.queue.write_buffer(
                &self.glyph_buffer,
                u64::try_from(dirty.start).unwrap() * 4,
                bytemuck::cast_slice(&self.values[dirty]),
            );
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("yarugc dynamic readout layer"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: self.surface.view(),
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        if let Some(vertices) = &self.vertices {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.binding, &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.draw(0..self.vertex_count, 0..1);
        }
        true
    }

    /// Request another raster pass without changing glyph IDs. Use after discarding
    /// an encoded frame; queued buffer uploads are retained by the queue.
    pub fn invalidate_surface(&mut self) {
        if self.dirty.is_none() {
            self.dirty = Some(0..0);
        }
    }

    fn mark_dirty(&mut self, changed: Range<usize>) {
        self.dirty = Some(match self.dirty.take() {
            Some(range) if !range.is_empty() => {
                range.start.min(changed.start)..range.end.max(changed.end)
            }
            _ => changed,
        });
    }

    /// Number of glyph cells allocated for this layer.
    pub fn cell_count(&self) -> usize {
        self.ranges.last().map_or(0, |r| r.end)
    }
}

#[derive(Debug)]
struct GlyphMap {
    ascii: [u32; 128],
    unicode: BTreeMap<char, u32>,
}

impl GlyphMap {
    fn new(glyphs: BTreeMap<char, u32>) -> Self {
        let mut ascii = [u32::MAX; 128];
        let mut unicode = BTreeMap::new();
        for (ch, glyph) in glyphs {
            if ch.is_ascii() {
                ascii[usize::try_from(u32::from(ch)).expect("ASCII index")] = glyph;
            } else {
                unicode.insert(ch, glyph);
            }
        }
        Self { ascii, unicode }
    }

    fn get(&self, ch: char) -> Option<u32> {
        if ch.is_ascii() {
            let glyph = self.ascii[usize::try_from(u32::from(ch)).expect("ASCII index")];
            (glyph != u32::MAX).then_some(glyph)
        } else {
            self.unicode.get(&ch).copied()
        }
    }
}

struct RasterAtlas {
    glyphs: BTreeMap<char, u32>,
    cell_size: [u32; 2],
    layers: u32,
    pixels: Vec<u8>,
}

// All raster bounds are checked against the device's limits before conversion.
// The font rasterizer supplies integral pixel bounds and [0, 1] coverage.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "validated integral glyph bounds and normalized coverage"
)]
fn rasterize(
    font: &[u8],
    pixel_height: f32,
    alphabet: impl IntoIterator<Item = char>,
    limits: &wgpu::Limits,
) -> Result<RasterAtlas, ReadoutError> {
    if !pixel_height.is_finite()
        || pixel_height <= 0.0
        || f64::from(pixel_height) > f64::from(limits.max_texture_dimension_2d)
    {
        return Err(ReadoutError::InvalidExtent);
    }
    let font = FontRef::try_from_slice(font).map_err(|_| ReadoutError::InvalidFont)?;
    let scaled = font.as_scaled(pixel_height);
    let mut glyphs = BTreeMap::from([(' ', 0)]);
    let mut outlines = Vec::new();
    let mut width = 1.0_f32;
    let mut top = -scaled.ascent().ceil();
    let mut bottom = -scaled.descent().floor();
    for ch in alphabet {
        if glyphs.contains_key(&ch) {
            continue;
        }
        let id = scaled.glyph_id(ch);
        if id.0 == 0 {
            return Err(ReadoutError::MissingGlyph(ch));
        }
        let layer = u32::try_from(outlines.len() + 1).map_err(|_| ReadoutError::InvalidExtent)?;
        if layer >= limits.max_texture_array_layers {
            return Err(ReadoutError::InvalidExtent);
        }
        let outline =
            scaled.outline_glyph(id.with_scale_and_position(pixel_height, point(0.0, 0.0)));
        width = width.max(scaled.h_advance(id).ceil());
        if let Some(outline) = &outline {
            let bounds = outline.px_bounds();
            width = width.max(bounds.width());
            top = top.min(bounds.min.y);
            bottom = bottom.max(bounds.max.y);
        }
        glyphs.insert(ch, layer);
        outlines.push(outline);
    }
    // Transparent padding avoids filtering across an ink cell's boundary.
    let w = width + 2.0;
    let h = bottom - top + 2.0;
    if !w.is_finite()
        || !h.is_finite()
        || w <= 0.0
        || h <= 0.0
        || f64::from(w.max(h)) > f64::from(limits.max_texture_dimension_2d)
    {
        return Err(ReadoutError::InvalidExtent);
    }
    let cell_size = [w as u32, h as u32];
    let layers = u32::try_from(outlines.len() + 1).map_err(|_| ReadoutError::InvalidExtent)?;
    let area = usize::try_from(u64::from(cell_size[0]) * u64::from(cell_size[1]))
        .map_err(|_| ReadoutError::InvalidExtent)?;
    let total = area
        .checked_mul(usize::try_from(layers).unwrap())
        .ok_or(ReadoutError::InvalidExtent)?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(total)
        .map_err(|_| ReadoutError::InvalidExtent)?;
    pixels.resize(total, 0);
    for (index, outline) in outlines.into_iter().enumerate() {
        if let Some(outline) = outline {
            let bounds = outline.px_bounds();
            let x0 = (w - bounds.width()) as u32 / 2;
            let y0 = (1.0 + bounds.min.y - top) as u32;
            let layer = (index + 1) * area;
            outline.draw(|x, y, coverage| {
                let pixel = usize::try_from((y0 + y) * cell_size[0] + x0 + x).unwrap();
                if pixel < area {
                    pixels[layer + pixel] = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            });
        }
    }
    Ok(RasterAtlas {
        glyphs,
        cell_size,
        layers,
        pixels,
    })
}
