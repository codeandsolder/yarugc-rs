// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

//! Damage is derived from the actual clipped, transformed cell triangles.
//! Each tile retains painter-ordered vertex ranges, including unchanged cells
//! that overlap the changed cell. Clear and reconstruct disjoint tiles.

use std::ops::Range;

use super::ReadoutError;

// Small enough to isolate individual numerical readouts while amortizing
// scissor and draw commands. Scheduling is independent of glyph raster size.
const TILE_SIZE: u32 = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PixelRect {
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl PixelRect {
    fn area(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    pub(super) fn scissor(self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_scissor_rect(self.x, self.y, self.width, self.height);
    }
}

#[derive(Clone, Debug)]
struct Cell {
    id: u32,
    columns: Range<u32>,
    rows: Range<u32>,
}

#[derive(Debug, Default)]
pub(super) struct Tile {
    pub(super) rect: PixelRect,
    pub(super) vertices: Vec<Range<u32>>,
    dirty: bool,
}

impl Tile {
    fn append(&mut self, vertices: &Range<u32>) -> Result<(), ReadoutError> {
        if let Some(last) = self.vertices.last_mut()
            && last.end == vertices.start
        {
            last.end = vertices.end;
        } else {
            self.vertices
                .try_reserve(1)
                .map_err(|_| ReadoutError::InvalidExtent)?;
            self.vertices.push(vertices.clone());
        }
        Ok(())
    }
}

/// Work actually encoded by the most recent readout raster pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadoutRasterStats {
    /// Whether the entire retained texture was reconstructed.
    pub full: bool,
    /// Number of target pixels cleared and eligible for drawing.
    pub pixels: u64,
    /// Number of draw calls (including scissored clears).
    pub draws: u64,
    /// Number of readout vertices submitted, excluding clear triangles.
    pub vertices: u64,
}

#[derive(Debug)]
pub(super) struct DamageGrid {
    extent: [u32; 2],
    columns: usize,
    cells: Vec<Cell>,
    tiles: Vec<Tile>,
    dirty: Vec<usize>,
    pending: ReadoutRasterStats,
    full: bool,
    vertex_count: u32,
}

impl DamageGrid {
    pub(super) fn new(extent: [u32; 2], vertices: &[[u32; 9]]) -> Result<Self, ReadoutError> {
        let [width, height] = extent;
        let columns = width.div_ceil(TILE_SIZE);
        let rows = height.div_ceil(TILE_SIZE);
        let count = usize::try_from(u64::from(columns) * u64::from(rows))
            .map_err(|_| ReadoutError::InvalidExtent)?;
        let mut tiles = Vec::new();
        tiles
            .try_reserve_exact(count)
            .map_err(|_| ReadoutError::InvalidExtent)?;
        for row in 0..rows {
            for column in 0..columns {
                let x = column * TILE_SIZE;
                let y = row * TILE_SIZE;
                tiles.push(Tile {
                    rect: PixelRect {
                        x,
                        y,
                        width: (width - x).min(TILE_SIZE),
                        height: (height - y).min(TILE_SIZE),
                    },
                    ..Tile::default()
                });
            }
        }
        let mut dirty = Vec::new();
        dirty
            .try_reserve_exact(count)
            .map_err(|_| ReadoutError::InvalidExtent)?;
        let mut grid = Self {
            extent,
            columns: columns as usize,
            cells: Vec::new(),
            tiles,
            dirty,
            pending: ReadoutRasterStats::default(),
            full: true,
            vertex_count: u32::try_from(vertices.len()).map_err(|_| ReadoutError::InvalidExtent)?,
        };
        let mut start = 0;
        for group in vertices.chunk_by(|a, b| a[4] == b[4]) {
            let end = start + group.len();
            let range = u32::try_from(start).map_err(|_| ReadoutError::InvalidExtent)?
                ..u32::try_from(end).map_err(|_| ReadoutError::InvalidExtent)?;
            grid.insert(group, range)?;
            start = end;
        }
        Ok(grid)
    }

    fn insert(&mut self, group: &[[u32; 9]], vertices: Range<u32>) -> Result<(), ReadoutError> {
        let Some(first) = group.first() else {
            return Ok(());
        };
        let Some(rect) = vertex_bounds(self.extent, group) else {
            return Ok(());
        };
        let columns = (rect.x / TILE_SIZE)..(rect.x + rect.width).div_ceil(TILE_SIZE);
        let rows = (rect.y / TILE_SIZE)..(rect.y + rect.height).div_ceil(TILE_SIZE);
        for row in rows.clone() {
            for column in columns.clone() {
                self.tiles[row as usize * self.columns + column as usize].append(&vertices)?;
            }
        }
        self.cells
            .try_reserve(1)
            .map_err(|_| ReadoutError::InvalidExtent)?;
        self.cells.push(Cell {
            id: first[4],
            columns,
            rows,
        });
        Ok(())
    }

    pub(super) fn mark(&mut self, id: usize) {
        // IDs were bounded by u32 at geometry preparation. Invisible cells have
        // no record and cannot change the retained texture.
        if self.full {
            return;
        }
        let Ok(id) = u32::try_from(id) else {
            return;
        };
        let Ok(index) = self.cells.binary_search_by_key(&id, |cell| cell.id) else {
            return;
        };
        let cell = &self.cells[index];
        for row in cell.rows.clone() {
            for column in cell.columns.clone() {
                let index = row as usize * self.columns + column as usize;
                let tile = &mut self.tiles[index];
                if !tile.dirty {
                    tile.dirty = true;
                    // Reserved once, at geometry preparation.
                    self.dirty.push(index);
                    self.pending.pixels += tile.rect.area();
                    self.pending.draws += 1 + tile.vertices.len() as u64;
                    self.pending.vertices += tile
                        .vertices
                        .iter()
                        .map(|range| u64::from(range.end - range.start))
                        .sum::<u64>();
                }
                // Stop doing damage bookkeeping once a full redraw is cheaper.
                if self.pending.vertices >= u64::from(self.vertex_count)
                    || self.pending.pixels >= self.area().div_ceil(2)
                {
                    self.full = true;
                    return;
                }
            }
        }
    }

    pub(super) const fn invalidate(&mut self) {
        self.full = true;
    }

    fn area(&self) -> u64 {
        u64::from(self.extent[0]) * u64::from(self.extent[1])
    }

    pub(super) fn plan(&self) -> Option<ReadoutRasterStats> {
        if self.full {
            Some(ReadoutRasterStats {
                full: true,
                pixels: self.area(),
                draws: u64::from(self.vertex_count != 0),
                vertices: u64::from(self.vertex_count),
            })
        } else if self.dirty.is_empty() {
            None
        } else {
            Some(self.pending)
        }
    }

    pub(super) fn dirty_tiles(&self) -> impl Iterator<Item = &Tile> {
        self.dirty.iter().map(|&index| &self.tiles[index])
    }

    pub(super) fn finish(&mut self) {
        for index in self.dirty.drain(..) {
            self.tiles[index].dirty = false;
        }
        self.pending = ReadoutRasterStats::default();
        self.full = false;
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Conservative integer bounds are clamped to the finite u32 texture extent."
)]
fn vertex_bounds(extent: [u32; 2], group: &[[u32; 9]]) -> Option<PixelRect> {
    let [width, height] = extent.map(f64::from);
    let mut min = [width, height];
    let mut max = [0.0_f64; 2];
    for vertex in group {
        let [x, y] = [vertex[0], vertex[1]].map(|bits| f64::from(f32::from_bits(bits)));
        let position = [(x + 1.0) * width * 0.5, (1.0 - y) * height * 0.5];
        for axis in 0..2 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
    }
    // Include a pixel of guard for GPU viewport conversion/subpixel rounding.
    let x = (min[0].floor() - 1.0).clamp(0.0, width) as u32;
    let y = (min[1].floor() - 1.0).clamp(0.0, height) as u32;
    let end_x = (max[0].ceil() + 1.0).clamp(0.0, width) as u32;
    let end_y = (max[1].ceil() + 1.0).clamp(0.0, height) as u32;
    (end_x > x && end_y > y).then_some(PixelRect {
        x,
        y,
        width: end_x.saturating_sub(x),
        height: end_y.saturating_sub(y),
    })
}

pub(super) fn clear_pipeline(device: &wgpu::Device) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("yarugc damage clear shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("clear.wgsl").into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("yarugc damage clear pipeline"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(id: u32, x: f32, y: f32, width: f32, height: f32) -> [[u32; 9]; 6] {
        let vertex = |x: f32, y: f32| {
            let mut v = [0; 9];
            v[0] = x.to_bits();
            v[1] = y.to_bits();
            v[4] = id;
            v
        };
        let a = vertex(x, y);
        let b = vertex(x + width, y);
        let c = vertex(x + width, y - height);
        let d = vertex(x, y - height);
        [a, b, c, a, c, d]
    }

    #[test]
    fn dirty_tiles_include_unchanged_overlapping_cells_in_painter_order() {
        let vertices = [
            cell(0, -0.95, 0.95, 0.1, 0.1),
            cell(1, 0.5, -0.5, 0.1, 0.1),
            cell(2, -0.93, 0.93, 0.12, 0.12),
            cell(4, 0.3, 0.5, 0.1, 0.1),
        ]
        .concat();
        let mut grid = DamageGrid::new([256, 256], &vertices).unwrap();
        assert!(grid.plan().unwrap().full);
        grid.finish();
        grid.mark(0);
        grid.mark(0);
        grid.mark(3); // invisible
        let stats = grid.plan().unwrap();
        assert!(!stats.full);
        assert_eq!(stats.pixels, 32 * 32);
        assert_eq!(stats.vertices, 12);
        assert_eq!(stats.draws, 3);
        assert_eq!(grid.dirty_tiles().next().unwrap().vertices, [0..6, 12..18]);
        grid.finish();
        assert!(grid.plan().is_none());
        grid.invalidate();
        assert!(grid.plan().unwrap().full);
    }

    #[test]
    fn dense_damage_uses_single_full_redraw() {
        let vertices = [cell(0, -0.9, 0.9, 0.8, 0.8), cell(1, 0.1, -0.1, 0.8, 0.8)].concat();
        let mut grid = DamageGrid::new([128, 128], &vertices).unwrap();
        grid.finish();
        grid.mark(0);
        grid.mark(1);
        assert!(grid.plan().unwrap().full);
    }
}
