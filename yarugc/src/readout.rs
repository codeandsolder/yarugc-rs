// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

mod damage;

use damage::DamageGrid;
pub use damage::ReadoutRasterStats;

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use masonry_core::kurbo::Rect;
use masonry_imaging::{
    texture_render::Renderer,
    vello_hybrid::{OutlineGlyphStyle, PreparedOutlineGlyphs},
};
use peniko::FontData;
use skrifa::{
    FontRef, MetadataProvider,
    instance::{LocationRef, NormalizedCoord, Size},
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
    /// The selected backend could not prepare the shared glyph atlas.
    Atlas(String),
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
            Self::Atlas(error) => write!(f, "shared glyph atlas: {error}"),
        }
    }
}

impl std::error::Error for ReadoutError {}

/// Fixed-cell font policy, independent of geometry and current text.
#[derive(Clone, Copy, Debug)]
pub struct ReadoutStyle<'a> {
    /// Physical pixels per em, matching the ordinary text renderer.
    pub font_size: f32,
    /// Optional explicit physical cell advance. Otherwise use the maximum
    /// advance and ink width of the preflight alphabet, with padding.
    pub advance: Option<f32>,
    /// Enable font hinting, matching the ordinary glyph cache policy.
    pub hint: bool,
    /// Signed F2Dot14 variation coordinates, in font-axis order.
    pub normalized_coords: &'a [i16],
}

/// Leased glyph subset and GPU pipeline for fixed-width readouts.
///
/// This is a scalar telemetry renderer, not a general Unicode shaping engine.
/// Ordinary rich, bidi and editable text should use Masonry/Parley.
#[derive(Debug)]
pub struct ReadoutRenderer {
    device: wgpu::Device,
    glyphs: Arc<GlyphMap>,
    queue: wgpu::Queue,
    cell_size: [f32; 2],
    atlas: PreparedOutlineGlyphs,
    glyph_table: wgpu::Buffer,
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    clear_pipeline: wgpu::RenderPipeline,
}

/// Persistent readout geometry and glyph data for a single dynamic layer.
/// Set values and render only when they change; no new GPU mesh is generated.
#[derive(Debug)]
pub struct ReadoutBatch {
    surface: Surface,
    // Keep the shared atlas allocations pinned for the lifetime of the batch.
    _atlas: PreparedOutlineGlyphs,
    glyphs: Arc<GlyphMap>,
    pipeline: wgpu::RenderPipeline,
    clear_pipeline: wgpu::RenderPipeline,
    damage: DamageGrid,
    last_stats: Option<ReadoutRasterStats>,
    queue: wgpu::Queue,
    ranges: Vec<Range<usize>>,
    values: Vec<u32>,
    dirty: Option<Range<usize>>,
    vertices: Option<wgpu::Buffer>,
    vertex_count: u32,
    glyph_buffer: wgpu::Buffer,
    binding: wgpu::BindGroup,
}

fn create_pipeline(
    device: &wgpu::Device,
    pages: usize,
) -> Result<(wgpu::BindGroupLayout, wgpu::RenderPipeline), ReadoutError> {
    let count = u32::try_from(pages).map_err(|_| ReadoutError::InvalidExtent)?;
    let limits = device.limits();
    if count > limits.max_sampled_textures_per_shader_stage
        || count
            .checked_add(3)
            .is_none_or(|n| n > limits.max_bindings_per_bind_group)
    {
        return Err(ReadoutError::InvalidExtent);
    }
    let mut entries = vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
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
    ];
    let mut declarations = String::new();
    let mut cases = String::new();
    use std::fmt::Write as _;
    for page in 0..count {
        let binding = page + 3;
        entries.push(wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
        writeln!(
            declarations,
            "@group(0) @binding({binding}) var page_{page}: texture_2d<f32>;"
        )
        .expect("writing to a string cannot fail");
        writeln!(cases, "case {page}u: {{ return textureSampleLevel(page_{page}, atlas_sampler, texel / vec2<f32>(textureDimensions(page_{page})), 0.0).a; }}").expect("writing to a string cannot fail");
    }
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("yarugc readout layout"),
        entries: &entries,
    });
    let source = include_str!("readout.wgsl")
        .replace("// ATLAS_BINDINGS", &declarations)
        .replace("// ATLAS_CASES", &cases);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("yarugc readout shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
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
    Ok((layout, pipeline))
}

impl ReadoutRenderer {
    /// Prewarm only the requested alphabet in the same atlas used by ordinary UI text.
    /// Pass the same FontData identity used for normal text to reuse matching entries.
    /// The empty cell and space are transparent.
    /// # Errors
    /// Returns an error for invalid fonts, unsupported glyphs, extents or backend.
    pub fn new(
        renderer: &mut Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font: &FontData,
        style: ReadoutStyle<'_>,
        alphabet: impl IntoIterator<Item = char>,
    ) -> Result<Self, ReadoutError> {
        let limits = device.limits();
        let metrics = preflight(font, style, alphabet, &limits)?;
        let atlas = renderer
            .prepare_outline_glyphs(
                device,
                queue,
                font,
                OutlineGlyphStyle {
                    font_size: style.font_size,
                    hint: style.hint,
                    normalized_coords: style.normalized_coords,
                },
                &metrics.ids,
            )
            .map_err(|error| ReadoutError::Atlas(error.to_string()))?;
        let (cell_size, table) = atlas_layout(&metrics, style, &atlas, &limits)?;
        let glyph_table = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("yarugc shared glyph metadata"),
            contents: bytemuck::cast_slice(&table),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let (layout, pipeline) = create_pipeline(device, atlas.pages().len())?;
        Ok(Self {
            device: device.clone(),
            glyphs: Arc::new(GlyphMap::new(metrics.glyphs)),
            queue: queue.clone(),
            cell_size,
            atlas,
            glyph_table,
            sampler,
            layout,
            pipeline,
            clear_pipeline: damage::clear_pipeline(device),
        })
    }

    /// Physical cell advance and height in pixels.
    #[must_use = "Use the glyph cell dimensions."]
    pub const fn cell_size(&self) -> [f32; 2] {
        self.cell_size
    }

    /// Prepare fixed readout geometry. Repeat only after layout/DPI changes.
    /// Retain the geometry and spatial damage index for all visible cells.
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
        let damage = DamageGrid::new(extent, &vertices)?;
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
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: glyph_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: self.glyph_table.as_entire_binding(),
            },
        ];
        for (page, view) in self.atlas.pages().iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: u32::try_from(page).map_err(|_| ReadoutError::InvalidExtent)? + 3,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        let binding = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yarugc readout batch"),
            layout: &self.layout,
            entries: &entries,
        });
        Ok(ReadoutBatch {
            surface: compositor
                .create_surface(extent, AlphaMode::Premultiplied)
                .ok_or(ReadoutError::InvalidExtent)?,
            _atlas: self.atlas.clone(),
            glyphs: Arc::clone(&self.glyphs),
            pipeline: self.pipeline.clone(),
            clear_pipeline: self.clear_pipeline.clone(),
            damage,
            last_stats: None,
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
    cell_size: [f32; 2],
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
                (u * cell_size[0]).to_bits(),
                (v * cell_size[1]).to_bits(),
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
        let Some(changed) = replace_glyphs(
            &self.glyphs,
            &mut self.values[range.clone()],
            text,
            |index| self.damage.mark(range.start + index),
        )?
        else {
            return Ok(false);
        };
        self.mark_dirty((range.start + changed.start)..(range.start + changed.end));
        Ok(true)
    }

    /// Upload changed glyph IDs and reconstruct damaged tiles in painter order.
    /// Dense updates use one full-surface redraw. Returns false if no visible
    /// pixels need reconstruction. Submit the encoder before changing its
    /// values again; the caller owns frame scheduling and submission.
    /// If the encoder is discarded, call [`Self::invalidate_surface`] before retrying.
    pub fn render(&mut self, encoder: &mut wgpu::CommandEncoder) -> bool {
        if let Some(dirty) = self.dirty.take()
            && !dirty.is_empty()
        {
            self.queue.write_buffer(
                &self.glyph_buffer,
                (dirty.start as u64) * 4,
                bytemuck::cast_slice(&self.values[dirty]),
            );
        }
        self.last_stats = self.damage.plan();
        let Some(stats) = self.last_stats else {
            return false;
        };
        self.encode_raster(encoder, stats.full);
        self.damage.finish();
        true
    }

    fn encode_raster(&self, encoder: &mut wgpu::CommandEncoder, full: bool) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("yarugc dynamic readout layer"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: self.surface.view(),
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: if full {
                        wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        if !full {
            pass.set_pipeline(&self.clear_pipeline);
            for tile in self.damage.dirty_tiles() {
                tile.rect.scissor(&mut pass);
                pass.draw(0..3, 0..1);
            }
        }
        if let Some(vertices) = &self.vertices {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.binding, &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            if full {
                pass.draw(0..self.vertex_count, 0..1);
            } else {
                for tile in self.damage.dirty_tiles() {
                    tile.rect.scissor(&mut pass);
                    for range in &tile.vertices {
                        pass.draw(range.clone(), 0..1);
                    }
                }
            }
        }
    }

    /// Request a full reconstruction without changing glyph IDs, including
    /// after a discarded encoded frame. Queued uploads are retained by the queue.
    pub const fn invalidate_surface(&mut self) {
        self.damage.invalidate();
    }

    /// Work encoded by the last call to [`Self::render`], or none if unchanged.
    #[must_use = "Inspect the actual raster work."]
    pub const fn last_raster_stats(&self) -> Option<ReadoutRasterStats> {
        self.last_stats
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
    mut on_change: impl FnMut(usize),
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
            on_change(index);
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

#[derive(Debug)]
struct Preflight {
    glyphs: BTreeMap<char, u32>,
    ids: Vec<u32>,
    max_advance: f32,
    top: f32,
    bottom: f32,
}

fn preflight(
    font: &FontData,
    style: ReadoutStyle<'_>,
    alphabet: impl IntoIterator<Item = char>,
    limits: &wgpu::Limits,
) -> Result<Preflight, ReadoutError> {
    if !style.font_size.is_finite()
        || style.font_size <= 0.0
        || style
            .advance
            .is_some_and(|advance| !advance.is_finite() || advance <= 0.0)
        || f64::from(style.font_size) > f64::from(limits.max_texture_dimension_2d)
    {
        return Err(ReadoutError::InvalidExtent);
    }
    let font = FontRef::from_index(font.data.as_ref(), font.index)
        .map_err(|_| ReadoutError::InvalidFont)?;
    if style.normalized_coords.len() > font.axes().len()
        || style
            .normalized_coords
            .iter()
            .any(|&c| !(-16384..=16384).contains(&c))
    {
        return Err(ReadoutError::InvalidFont);
    }
    let coords: Vec<_> = style
        .normalized_coords
        .iter()
        .copied()
        .map(NormalizedCoord::from_bits)
        .collect();
    let location = LocationRef::new(&coords);
    let size = Size::new(style.font_size);
    let metrics = font.metrics(size, location);
    let glyph_metrics = font.glyph_metrics(size, location);
    let charmap = font.charmap();
    let mut glyphs = BTreeMap::from([(' ', 0)]);
    let mut ids = Vec::new();
    let mut glyph_indices = BTreeMap::new();
    let mut max_advance = 1.0_f32;
    for ch in alphabet {
        if glyphs.contains_key(&ch) {
            continue;
        }
        let id = charmap.map(ch).ok_or(ReadoutError::MissingGlyph(ch))?;
        let advance = glyph_metrics
            .advance_width(id)
            .ok_or(ReadoutError::InvalidFont)?;
        if !advance.is_finite() {
            return Err(ReadoutError::InvalidFont);
        }
        max_advance = max_advance.max(advance.ceil());
        let index = if let Some(&index) = glyph_indices.get(&id.to_u32()) {
            index
        } else {
            let index = u32::try_from(ids.len() + 1).map_err(|_| ReadoutError::InvalidExtent)?;
            if (u64::from(index) + 1) * 32
                > limits
                    .max_storage_buffer_binding_size
                    .min(limits.max_buffer_size)
            {
                return Err(ReadoutError::InvalidExtent);
            }
            glyph_indices.insert(id.to_u32(), index);
            ids.push(id.to_u32());
            index
        };
        glyphs.insert(ch, index);
    }
    Ok(Preflight {
        glyphs,
        ids,
        max_advance,
        top: -metrics.ascent.ceil(),
        bottom: -metrics.descent.floor(),
    })
}

fn atlas_layout(
    metrics: &Preflight,
    style: ReadoutStyle<'_>,
    atlas: &PreparedOutlineGlyphs,
    limits: &wgpu::Limits,
) -> Result<([f32; 2], Vec<[u32; 8]>), ReadoutError> {
    let mut width = metrics.max_advance;
    let mut top = metrics.top;
    let mut bottom = metrics.bottom;
    for slot in atlas.slots() {
        width = width.max(f32::from(slot.width));
        top = top.min(f32::from(slot.bearing_y));
        bottom = bottom.max(f32::from(slot.bearing_y) + f32::from(slot.height));
    }
    let cell_size = [style.advance.unwrap_or(width + 2.0), bottom - top + 2.0];
    if cell_size.iter().any(|&v| {
        !v.is_finite() || v <= 0.0 || f64::from(v) > f64::from(limits.max_texture_dimension_2d)
    }) {
        return Err(ReadoutError::InvalidExtent);
    }
    let mut table = vec![[0; 8]];
    for slot in atlas.slots() {
        let x = ((cell_size[0] - f32::from(slot.width)) * 0.5).floor();
        let y = 1.0 - top + f32::from(slot.bearing_y);
        table.push([
            f32::from(slot.x).to_bits(),
            f32::from(slot.y).to_bits(),
            f32::from(slot.width).to_bits(),
            f32::from(slot.height).to_bits(),
            x.to_bits(),
            y.to_bits(),
            slot.page_index,
            0,
        ]);
    }
    Ok((cell_size, table))
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
    fn preflight_deduplicates_and_maps_only_requested_glyphs() {
        let font = FontData::new(
            peniko::Blob::new(Arc::new(include_bytes!(
                "../../masonry_testing/resources/fonts/roboto/Roboto-Regular.ttf"
            ))),
            0,
        );
        let style = ReadoutStyle {
            font_size: 24.0,
            advance: Some(12.5),
            hint: false,
            normalized_coords: &[],
        };
        let metrics = preflight(
            &font,
            style,
            ['0', '1', '−', ' ', '0'],
            &wgpu::Limits::default(),
        )
        .unwrap();
        assert_eq!(metrics.ids.len(), 3);
        assert_eq!(metrics.glyphs.get(&' '), Some(&0));
        assert_eq!(metrics.glyphs.get(&'0'), Some(&1));
        assert!(!metrics.glyphs.contains_key(&'2'));
    }

    #[test]
    fn glyph_updates_are_transactional_and_report_exact_changed_range() {
        let map = GlyphMap::new(BTreeMap::from([(' ', 0), ('a', 1), ('b', 2), ('Ω', 3)]));
        let mut values = [0; 4];
        assert_eq!(
            replace_glyphs(&map, &mut values, "abΩ", |_| {}),
            Ok(Some(1..4))
        );
        assert_eq!(values, [0, 1, 2, 3]);
        assert_eq!(replace_glyphs(&map, &mut values, "abΩ", |_| {}), Ok(None));
        assert_eq!(
            replace_glyphs(&map, &mut values, "a?", |_| {}),
            Err(ReadoutError::UnknownCharacter('?'))
        );
        assert_eq!(
            replace_glyphs(&map, &mut values, "abΩab", |_| {}),
            Err(ReadoutError::TextTooLong)
        );
        assert_eq!(values, [0, 1, 2, 3]);
        assert_eq!(
            replace_glyphs(&map, &mut values, "Ω", |_| {}),
            Ok(Some(1..3))
        );
        assert_eq!(values, [0, 0, 0, 3]);
    }
}
