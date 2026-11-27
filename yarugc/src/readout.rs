// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use ab_glyph_rasterizer::{Rasterizer, point};
use masonry_core::kurbo::Rect;
use skrifa::{
    FontRef, MetadataProvider,
    instance::{LocationRef, Size},
    outline::{
        DrawSettings,
        pen::{ControlBoundsPen, PathElement},
    },
};
use wgpu::util::DeviceExt;

use crate::geometry::SubrectMesh;
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

/// Shared glyph atlas and GPU pipeline for fixed-width readouts.
///
/// This is a scalar telemetry renderer, not a general Unicode shaping engine.
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

fn create_pipeline(device: &wgpu::Device) -> (wgpu::BindGroupLayout, wgpu::RenderPipeline) {
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
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 36,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Uint32, 3 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
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
    (layout, pipeline)
}

impl ReadoutRenderer {
    /// Rasterize a caller-selected font and alphabet once into a texture array.
    /// Space and empty cells are transparent; other characters must be in the font.
    ///
    /// # Errors
    /// Returns an error for invalid fonts, missing glyphs or unsupported atlas extents.
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
        let (layout, pipeline) = create_pipeline(device);
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
    #[must_use = "Use the glyph cell dimensions."]
    pub const fn cell_size(&self) -> [u32; 2] {
        self.cell_size
    }

    /// Prepare fixed readout geometry. Repeat only after layout/DPI changes.
    /// All cells of all slots are rendered in a single draw call.
    ///
    /// # Errors
    /// Returns an error for mismatched devices, invalid extents or resource limits.
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
        let mut ranges = Vec::new();
        ranges
            .try_reserve_exact(slots.len())
            .map_err(|_| ReadoutError::InvalidExtent)?;
        let mut count = 0_usize;
        let mut vertices = Vec::<[u32; 9]>::new();
        for slot in slots {
            let end = count
                .checked_add(slot.cells)
                .ok_or(ReadoutError::InvalidExtent)?;
            let start_u32 = u32::try_from(count).map_err(|_| ReadoutError::InvalidExtent)?;
            let end_u32 = u32::try_from(end).map_err(|_| ReadoutError::InvalidExtent)?;
            if u64::from(end_u32.max(1)) * 4
                > limits
                    .max_storage_buffer_binding_size
                    .min(limits.max_buffer_size)
            {
                return Err(ReadoutError::InvalidExtent);
            }
            append_slot_vertices(
                &mut vertices,
                slot,
                self.cell_size,
                extent,
                start_u32..end_u32,
                limits.max_buffer_size,
            )?;
            ranges.push(count..end);
            count = end;
        }
        let vertex_count =
            u32::try_from(vertices.len()).map_err(|_| ReadoutError::InvalidExtent)?;
        let vertex_buffer = (!vertices.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("yarugc readout geometry"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });
        if (count.max(1) as u64) * 4
            > limits
                .max_storage_buffer_binding_size
                .min(limits.max_buffer_size)
        {
            return Err(ReadoutError::InvalidExtent);
        }
        let mut values = Vec::new();
        values
            .try_reserve_exact(count.max(1))
            .map_err(|_| ReadoutError::InvalidExtent)?;
        values.resize(count.max(1), 0_u32);
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

fn append_slot_vertices(
    vertices: &mut Vec<[u32; 9]>,
    slot: &ReadoutSlot,
    cell_size: [u32; 2],
    extent: [u32; 2],
    cells: Range<u32>,
    max_buffer_size: u64,
) -> Result<(), ReadoutError> {
    let [width, height] = slot.geometry.logical_size();
    let [cell_w, cell_h] = cell_size.map(f64::from);
    let scale = slot.geometry.scale_factor();
    let dx = cell_w / (width * scale);
    let dy = cell_h / (height * scale);
    let top = (1.0 - dy) * 0.5;
    if !dx.is_finite() || !dy.is_finite() || dx <= 0.0 || dy <= 0.0 {
        return Err(ReadoutError::InvalidExtent);
    }
    let Some(mut mesh) = SubrectMesh::new(slot.geometry.placement(), extent) else {
        return Ok(());
    };
    let bounds = mesh.bounds();
    let bottom = top + dy;
    if bottom <= bounds.y0 || top >= bounds.y1 {
        return Ok(());
    }
    // Compute every boundary from the right edge, avoiding cumulative error.
    // The binary searches omit cells that cannot intersect the visible polygon.
    let edge = |cell: u32| f64::from(cells.end - cell).mul_add(-dx, 1.0);
    let visible = visible_cells(cells.clone(), &edge, bounds.x0, bounds.x1);
    let [r, g, b, a] = slot.color.map(f32::to_bits);
    for cell in visible {
        let quad = mesh.vertices(Rect::new(edge(cell), top, edge(cell + 1), bottom));
        let next_len = vertices
            .len()
            .checked_add(quad.len())
            .ok_or(ReadoutError::InvalidExtent)?;
        let count = u32::try_from(next_len).map_err(|_| ReadoutError::InvalidExtent)?;
        if u64::from(count) * 36 > max_buffer_size {
            return Err(ReadoutError::InvalidExtent);
        }
        vertices
            .try_reserve(quad.len())
            .map_err(|_| ReadoutError::InvalidExtent)?;
        vertices.extend(quad.iter().map(|[x, y, u, v]| {
            [
                x.to_bits(),
                y.to_bits(),
                u.to_bits(),
                v.to_bits(),
                cell,
                r,
                g,
                b,
                a,
            ]
        }));
    }
    Ok(())
}

// A monotonic edge function means a logarithmic search is sufficient even
// when a slot contains millions of logical cells outside its visible area.
fn visible_cells(cells: Range<u32>, edge: &impl Fn(u32) -> f64, min: f64, max: f64) -> Range<u32> {
    fn first_true(mut range: Range<u32>, predicate: impl Fn(u32) -> bool) -> u32 {
        while range.start < range.end {
            let mid = range.start.midpoint(range.end);
            if predicate(mid) {
                range.end = mid;
            } else {
                range.start = mid + 1;
            }
        }
        range.start
    }
    let start = first_true(cells.clone(), |cell| edge(cell + 1) > min);
    let end = first_true(start..cells.end, |cell| edge(cell) >= max);
    start..end
}

impl ReadoutBatch {
    /// Cached dynamic surface; compose it after the static window content.
    #[must_use = "Use the dynamic surface."]
    pub const fn surface(&self) -> &Surface {
        &self.surface
    }

    /// Change a slot's text without allocation, layout, or geometry generation.
    /// Invalid input leaves the previous value intact.
    ///
    /// # Errors
    /// Returns an error for unknown slots, unknown characters or text exceeding slot capacity.
    pub fn set_text(&mut self, slot: usize, text: &str) -> Result<bool, ReadoutError> {
        let range = self
            .ranges
            .get(slot)
            .ok_or(ReadoutError::UnknownSlot)?
            .clone();
        let Some(changed) = replace_glyphs(&self.glyphs, &mut self.values[range.clone()], text)?
        else {
            return Ok(false);
        };
        self.mark_dirty((range.start + changed.start)..(range.start + changed.end));
        Ok(true)
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
                (dirty.start as u64) * 4,
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
    pub const fn invalidate_surface(&mut self) {
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
    #[must_use = "Use the allocated cell count."]
    pub fn cell_count(&self) -> usize {
        self.ranges.last().map_or(0, |r| r.end)
    }
}

// Validate before writing to keep updates transactional. The immutable glyph map
// is then read again so the hot path requires no scratch allocation.
fn replace_glyphs(
    map: &GlyphMap,
    values: &mut [u32],
    text: &str,
) -> Result<Option<Range<usize>>, ReadoutError> {
    let len = text.chars().count();
    let padding = values
        .len()
        .checked_sub(len)
        .ok_or(ReadoutError::TextTooLong)?;
    for ch in text.chars() {
        if map.get(ch).is_none() {
            return Err(ReadoutError::UnknownCharacter(ch));
        }
    }
    let glyphs = std::iter::repeat_n(Ok(0), padding).chain(
        text.chars()
            .map(|ch| map.get(ch).ok_or(ReadoutError::UnknownCharacter(ch))),
    );
    let mut changed: Option<Range<usize>> = None;
    for (index, (value, glyph)) in values.iter_mut().zip(glyphs).enumerate() {
        let glyph = glyph?;
        if *value != glyph {
            *value = glyph;
            changed = Some(changed.map_or(index..index + 1, |range| range.start..index + 1));
        }
    }
    Ok(changed)
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
            if let Ok(index) = u8::try_from(u32::from(ch))
                && ch.is_ascii()
            {
                ascii[usize::from(index)] = glyph;
            } else {
                unicode.insert(ch, glyph);
            }
        }
        Self { ascii, unicode }
    }

    fn get(&self, ch: char) -> Option<u32> {
        if let Ok(index) = u8::try_from(u32::from(ch))
            && ch.is_ascii()
        {
            let glyph = self.ascii[usize::from(index)];
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
    reason = "Validated integral glyph bounds and normalized coverage"
)]
#[expect(
    clippy::cast_sign_loss,
    reason = "Validated glyph bounds and coverage are nonnegative"
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
    let font = FontRef::from_index(font, 0).map_err(|_| ReadoutError::InvalidFont)?;
    let location = LocationRef::default();
    let unscaled = font.metrics(Size::unscaled(), location);
    let height = unscaled.ascent - unscaled.descent;
    if !height.is_finite() || height <= 0.0 || unscaled.units_per_em == 0 {
        return Err(ReadoutError::InvalidFont);
    }
    // Preserve the public pixel-height convention (ascent minus descent),
    // whereas Skrifa uses pixels per em.
    let size = Size::new(pixel_height * f32::from(unscaled.units_per_em) / height);
    let metrics = font.metrics(size, location);
    let glyph_metrics = font.glyph_metrics(size, location);
    let charmap = font.charmap();
    let glyph_outlines = font.outline_glyphs();
    let mut glyphs = BTreeMap::from([(' ', 0)]);
    let mut outlines = Vec::new();
    let mut width = 1.0_f32;
    let mut top = -metrics.ascent.ceil();
    let mut bottom = -metrics.descent.floor();
    for ch in alphabet {
        if glyphs.contains_key(&ch) {
            continue;
        }
        let id = charmap.map(ch).ok_or(ReadoutError::MissingGlyph(ch))?;
        let layer = u32::try_from(outlines.len() + 1).map_err(|_| ReadoutError::InvalidExtent)?;
        if layer >= limits.max_texture_array_layers {
            return Err(ReadoutError::InvalidExtent);
        }
        let advance = glyph_metrics
            .advance_width(id)
            .ok_or(ReadoutError::InvalidFont)?;
        width = width.max(advance.ceil());
        let outline = glyph_outlines
            .get(id)
            .map(|glyph| load_outline(&glyph, size, location))
            .transpose()?
            .flatten();
        if let Some((bounds, _)) = &outline {
            width = width.max(bounds[2] - bounds[0]);
            top = top.min(bounds[1]);
            bottom = bottom.max(bounds[3]);
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
        .checked_mul(usize::try_from(layers).map_err(|_| ReadoutError::InvalidExtent)?)
        .ok_or(ReadoutError::InvalidExtent)?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(total)
        .map_err(|_| ReadoutError::InvalidExtent)?;
    pixels.resize(total, 0);
    for (index, outline) in outlines.into_iter().enumerate() {
        if let Some((bounds, path)) = outline {
            let layer = (index + 1) * area;
            draw_outline(
                &path,
                bounds,
                top,
                w,
                cell_size,
                &mut pixels[layer..layer + area],
            )?;
        }
    }
    Ok(RasterAtlas {
        glyphs,
        cell_size,
        layers,
        pixels,
    })
}

type GlyphOutline = ([f32; 4], Vec<PathElement>);

fn load_outline(
    glyph: &skrifa::outline::OutlineGlyph<'_>,
    size: Size,
    location: LocationRef<'_>,
) -> Result<Option<GlyphOutline>, ReadoutError> {
    let mut bounds = ControlBoundsPen::new();
    glyph
        .draw(DrawSettings::unhinted(size, location), &mut bounds)
        .map_err(|_| ReadoutError::InvalidFont)?;
    let Some(bounds) = bounds.bounding_box() else {
        return Ok(None);
    };
    let bounds = [
        bounds.x_min.floor(),
        -bounds.y_max.ceil(),
        bounds.x_max.ceil(),
        -bounds.y_min.floor(),
    ];
    if !bounds.iter().all(|value| value.is_finite()) {
        return Err(ReadoutError::InvalidFont);
    }
    let mut path = Vec::new();
    glyph
        .draw(DrawSettings::unhinted(size, location), &mut path)
        .map_err(|_| ReadoutError::InvalidFont)?;
    Ok(Some((bounds, path)))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "Coverage is normalized to 8-bit alpha"
)]
#[expect(clippy::cast_sign_loss, reason = "Coverage is clamped nonnegative")]
fn draw_outline(
    path: &[PathElement],
    bounds: [f32; 4],
    top: f32,
    w: f32,
    cell_size: [u32; 2],
    pixels: &mut [u8],
) -> Result<(), ReadoutError> {
    let [width, height] =
        cell_size.map(|v| usize::try_from(v).map_err(|_| ReadoutError::InvalidExtent));
    let (width, height) = (width?, height?);
    let x = ((w - (bounds[2] - bounds[0])) * 0.5).floor() - bounds[0];
    let y = 1.0 - top;
    let mut rasterizer = Rasterizer::new(width, height);
    let transform = |px, py: f32| point(px + x, -py + y);
    let mut current = point(0.0, 0.0);
    let mut start = current;
    for &element in path {
        match element {
            PathElement::MoveTo { x, y } => {
                current = transform(x, y);
                start = current;
            }
            PathElement::LineTo { x, y } => {
                let end = transform(x, y);
                rasterizer.draw_line(current, end);
                current = end;
            }
            PathElement::QuadTo { cx0, cy0, x, y } => {
                let end = transform(x, y);
                rasterizer.draw_quad(current, transform(cx0, cy0), end);
                current = end;
            }
            PathElement::CurveTo {
                cx0,
                cy0,
                cx1,
                cy1,
                x,
                y,
            } => {
                let end = transform(x, y);
                rasterizer.draw_cubic(current, transform(cx0, cy0), transform(cx1, cy1), end);
                current = end;
            }
            PathElement::Close => {
                rasterizer.draw_line(current, start);
                current = start;
            }
        }
    }
    rasterizer.for_each_pixel(|pixel, coverage| {
        pixels[pixel] = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_culling_matches_brute_force_and_scales_logarithmically() {
        for count in [0_u32, 1, 7, 100] {
            for dx in [0.01, 0.25, 2.0] {
                let cells = 11..11 + count;
                let edge = |i| f64::from(cells.end - i).mul_add(-dx, 1.0);
                for (min, max) in [
                    (-2.0, -1.0),
                    (0.0, 1.0),
                    (0.24, 0.75),
                    (0.75, 1.0),
                    (1.0, 2.0),
                ] {
                    let expected: Vec<_> = cells
                        .clone()
                        .filter(|&i| edge(i + 1) > min && edge(i) < max)
                        .collect();
                    assert_eq!(
                        visible_cells(cells.clone(), &edge, min, max).collect::<Vec<_>>(),
                        expected
                    );
                }
            }
        }
        let calls = std::cell::Cell::new(0);
        let edge = |i| {
            calls.set(calls.get() + 1);
            f64::from(1_000_000_000_u32 - i).mul_add(-0.25, 1.0)
        };
        assert_eq!(
            visible_cells(0..1_000_000_000, &edge, 0.0, 1.0),
            999_999_996..1_000_000_000
        );
        assert!(calls.get() <= 64);
    }

    #[test]
    fn fontations_readout_rasterizes_distinct_padded_glyphs() {
        let font =
            include_bytes!("../../masonry_testing/resources/fonts/roboto/Roboto-Regular.ttf");
        let atlas = rasterize(font, 24.0, ['0', '1', '−', ' '], &wgpu::Limits::default()).unwrap();
        let [w, h] = atlas.cell_size.map(|value| usize::try_from(value).unwrap());
        let area = w * h;
        assert_eq!(atlas.layers, 4);
        assert!(atlas.pixels[..area].iter().all(|&pixel| pixel == 0));
        for layer in atlas.pixels[area..].chunks_exact(area) {
            assert!(layer.iter().any(|&pixel| pixel > 0));
            assert!(
                layer[..w]
                    .iter()
                    .chain(&layer[area - w..])
                    .all(|&pixel| pixel == 0)
            );
            assert!(
                layer
                    .chunks_exact(w)
                    .all(|row| row[0] == 0 && row[w - 1] == 0)
            );
        }
        assert_ne!(
            &atlas.pixels[area..2 * area],
            &atlas.pixels[2 * area..3 * area]
        );
    }

    #[test]
    fn glyph_updates_are_transactional_and_report_exact_changed_range() {
        let map = GlyphMap::new(BTreeMap::from([(' ', 0), ('a', 1), ('b', 2), ('Ω', 3)]));
        let mut values = [0; 4];
        assert_eq!(replace_glyphs(&map, &mut values, "abΩ"), Ok(Some(1..4)));
        assert_eq!(values, [0, 1, 2, 3]);
        assert_eq!(replace_glyphs(&map, &mut values, "abΩ"), Ok(None));
        assert_eq!(
            replace_glyphs(&map, &mut values, "a?"),
            Err(ReadoutError::UnknownCharacter('?'))
        );
        assert_eq!(
            replace_glyphs(&map, &mut values, "abΩab"),
            Err(ReadoutError::TextTooLong)
        );
        assert_eq!(values, [0, 1, 2, 3]);
        assert_eq!(replace_glyphs(&map, &mut values, "Ω"), Ok(Some(1..3)));
        assert_eq!(values, [0, 0, 0, 3]);
    }
}
