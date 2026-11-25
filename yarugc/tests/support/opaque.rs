// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

//! Separate experiment: opaque precolored glyph tiles versus transparent coverage.
//! Fixed cell backgrounds are known and opaque; this is not general pixel damage.

use super::*;
use wgpu::util::DeviceExt;

const GLYPHS: &str = " 0123456789.km";
const CELLS: u32 = 8;
const TILE_W: u32 = 16;
const TILE_H: u32 = 32;
const ATLAS_W: u32 = 256;
const BG: [[u8; 3]; 2] = [[16, 24, 32], [24, 32, 40]];

pub(super) fn glyph_atlas(
    device: &Device,
    queue: &Queue,
    compositor: &Compositor,
) -> (Surface, Surface) {
    // Use Glyphon, not a synthetic pattern or a new font rasterizer.
    let mut fs = font_system();
    let buffers: Vec<_> = GLYPHS
        .chars()
        .map(|c| shape(&mut fs, &c.to_string()))
        .collect();
    let cache = glyphon::Cache::new(device);
    let mut atlas = glyphon::TextAtlas::new(device, queue, &cache, FORMAT);
    let renderer = glyphon::TextRenderer::new(&mut atlas, device, Default::default(), None);
    let mut viewport = glyphon::Viewport::new(device, &cache);
    viewport.update(
        queue,
        glyphon::Resolution {
            width: ATLAS_W,
            height: TILE_H,
        },
    );
    let mut renderer = renderer;
    let mut swash = glyphon::SwashCache::new();
    let alpha = compositor
        .create_surface([ATLAS_W, TILE_H], AlphaMode::Premultiplied)
        .unwrap();
    renderer
        .prepare(
            device,
            queue,
            &mut fs,
            &mut atlas,
            &viewport,
            buffers.iter().enumerate().map(|(i, b)| {
                let x = i as i32 * TILE_W as i32;
                glyphon::TextArea {
                    buffer: b,
                    left: x as f32 + 2.0,
                    top: 4.0,
                    scale: 1.0,
                    bounds: glyphon::TextBounds {
                        left: x,
                        top: 0,
                        right: x + TILE_W as i32,
                        bottom: TILE_H as i32,
                    },
                    default_color: glyphon::Color::rgb(255, 255, 255),
                    custom_glyphs: &[],
                }
            }),
            &mut swash,
        )
        .unwrap();
    let mut enc = device.create_command_encoder(&Default::default());
    renderer
        .render(&atlas, &viewport, &mut clear_pass(&mut enc, &alpha))
        .unwrap();
    queue.submit(Some(enc.finish()));
    let coverage = pixels(device, queue, &alpha);
    assert!(
        coverage.chunks_exact(4).any(|p| p[3] > 128),
        "glyph atlas should contain visible ink"
    );
    assert!(
        coverage.chunks_exact(4).all(|p| p[0].abs_diff(p[3]) <= 1
            && p[1].abs_diff(p[3]) <= 1
            && p[2].abs_diff(p[3]) <= 1),
        "white glyph coverage must be premultiplied"
    );
    // Bake exactly the same coverage over both known backgrounds. The atlas
    // preparation itself is outside the steady-state timing.
    let opaque = compositor
        .create_surface([ATLAS_W, TILE_H * 2], AlphaMode::Premultiplied)
        .unwrap();
    let mut data = Vec::with_capacity(coverage.len() * 2);
    for bg in BG {
        for p in coverage.chunks_exact(4) {
            let a = u16::from(p[3]);
            for channel in bg {
                data.push(((255 * a + u16::from(channel) * (255 - a) + 127) / 255) as u8);
            }
            data.push(255);
        }
    }
    queue.write_texture(
        opaque.texture().as_image_copy(),
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(ATLAS_W * 4),
            rows_per_image: Some(TILE_H * 2),
        },
        opaque.texture().size(),
    );
    (alpha, opaque)
}

struct Pipelines {
    layout: wgpu::BindGroupLayout,
    alpha: wgpu::RenderPipeline,
    opaque: wgpu::RenderPipeline,
    mixed: wgpu::RenderPipeline,
    background: wgpu::RenderPipeline,
    full_alpha: wgpu::RenderPipeline,
    full_opaque: wgpu::RenderPipeline,
}
impl Pipelines {
    fn new(device: &Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
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
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(include_str!("opaque.wgsl").into()),
        });
        let pipe = |v, f, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(f),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(v),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(f),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: if v == "tile_vs" {
                        wgpu::PrimitiveTopology::TriangleStrip
                    } else {
                        wgpu::PrimitiveTopology::TriangleList
                    },
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let alpha = pipe(
            "tile_vs",
            "alpha_fs",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let opaque = pipe("tile_vs", "opaque_fs", None);
        let mixed = pipe("tile_vs", "mixed_fs", None);
        let background = pipe("tile_vs", "background_fs", None);
        let full_alpha = pipe(
            "full_vs",
            "full_fs",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let full_opaque = pipe("full_vs", "full_fs", None);
        Self {
            layout,
            alpha,
            opaque,
            mixed,
            background,
            full_alpha,
            full_opaque,
        }
    }
    fn bind(&self, device: &Device, surface: &Surface, ids: &wgpu::Buffer) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(surface.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: ids.as_entire_binding(),
                },
            ],
        })
    }
}

pub(super) fn glyph_ids(frame: u32, changed: usize) -> Vec<u32> {
    (0..COUNT)
        .flat_map(|i| {
            let s = &strings(i)[selected(frame, changed, i) % 2];
            let mut result = [0; CELLS as usize];
            let len = s.chars().count();
            assert!(
                len <= result.len(),
                "benchmark text exceeds fixed glyph cells"
            );
            for (c, slot) in s.chars().zip(result[CELLS as usize - len..].iter_mut()) {
                *slot = GLYPHS.chars().position(|g| g == c).unwrap() as u32;
            }
            result
        })
        .collect()
}

fn pass<'a>(
    encoder: &'a mut CommandEncoder,
    target: &Surface,
    load: wgpu::LoadOp<wgpu::Color>,
) -> RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target.view(),
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    })
}

fn tiles(
    pass: &mut RenderPass<'_>,
    pipeline: &wgpu::RenderPipeline,
    binding: &wgpu::BindGroup,
    count: usize,
) {
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, binding, &[]);
    pass.draw(0..4, 0..count as u32 * CELLS);
}

fn full(pass: &mut RenderPass<'_>, pipeline: &wgpu::RenderPipeline, binding: &wgpu::BindGroup) {
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, binding, &[]);
    pass.draw(0..3, 0..1);
}

pub(super) fn compare(a: &[u8], b: &[u8]) {
    assert_eq!(
        a.len(),
        b.len(),
        "readback images must have the same extent"
    );
    let max = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap();
    assert!(max <= 2, "opaque and transparent output differ by {max}");
}

pub(super) fn run(device: &Device, queue: &Queue, compositor: &Compositor) {
    let (alpha, opaque) = glyph_atlas(device, queue, compositor);
    let p = Pipelines::new(device);
    let surface = || {
        compositor
            .create_surface([WIDTH, HEIGHT], AlphaMode::Premultiplied)
            .unwrap()
    };
    let alpha_panel = surface();
    let opaque_panel = surface();
    let background = surface();
    let transparent_final = surface();
    let opaque_final = surface();
    let ids = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&glyph_ids(1, COUNT)),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let ab = p.bind(device, &alpha, &ids);
    let ob = p.bind(device, &opaque, &ids);
    let bgb = p.bind(device, &background, &ids);
    let apb = p.bind(device, &alpha_panel, &ids);
    let opb = p.bind(device, &opaque_panel, &ids);
    let mut enc = device.create_command_encoder(&Default::default());
    tiles(
        &mut clear_pass(&mut enc, &background),
        &p.background,
        &ab,
        COUNT,
    );
    tiles(
        &mut clear_pass(&mut enc, &alpha_panel),
        &p.alpha,
        &ab,
        COUNT,
    );
    tiles(
        &mut clear_pass(&mut enc, &opaque_panel),
        &p.opaque,
        &ob,
        COUNT,
    );
    {
        let mut pass = clear_pass(&mut enc, &transparent_final);
        full(&mut pass, &p.full_opaque, &bgb);
        full(&mut pass, &p.full_alpha, &apb);
    }
    full(
        &mut clear_pass(&mut enc, &opaque_final),
        &p.full_opaque,
        &opb,
    );
    queue.submit(Some(enc.finish()));
    compare(
        &pixels(device, queue, &transparent_final),
        &pixels(device, queue, &opaque_final),
    );
    let mut enc = device.create_command_encoder(&Default::default());
    tiles(
        &mut clear_pass(&mut enc, &opaque_final),
        &p.mixed,
        &ab,
        COUNT,
    );
    queue.submit(Some(enc.finish()));
    compare(
        &pixels(device, queue, &transparent_final),
        &pixels(device, queue, &opaque_final),
    );
    eprintln!(
        "OPAQUE,verified_equal_pixels,atlas_bytes_alpha,{},atlas_bytes_two_backgrounds,{}",
        ATLAS_W * TILE_H * 4,
        ATLAS_W * TILE_H * 2 * 4
    );

    // Compare whole-layer redraws and logical cell updates. Alpha path needs
    // to restore the background before drawing coverage into a retained panel.
    for changed in [1, 8, 64, COUNT] {
        for kind in [
            "transparent_full_layer",
            "opaque_full_panel",
            "mixed_full_panel",
            "alpha_restore_changed_cells",
            "opaque_changed_cells",
            "mixed_changed_cells",
        ] {
            let timer = Timings::new(device);
            // Start from identical all-A data and a valid opaque panel.
            queue.write_buffer(&ids, 0, bytemuck::cast_slice(&glyph_ids(1, COUNT)));
            let mut enc = device.create_command_encoder(&Default::default());
            tiles(
                &mut clear_pass(&mut enc, &opaque_panel),
                &p.opaque,
                &ob,
                COUNT,
            );
            queue.submit(Some(enc.finish()));
            for f in 0..SAMPLES {
                let values = glyph_ids(f, changed);
                queue.write_buffer(
                    &ids,
                    0,
                    bytemuck::cast_slice(&values[..changed * CELLS as usize]),
                );
                let mut enc = device.create_command_encoder(&Default::default());
                enc.write_timestamp(&timer.query, f * 2);
                match kind {
                    "transparent_full_layer" => tiles(
                        &mut clear_pass(&mut enc, &alpha_panel),
                        &p.alpha,
                        &ab,
                        COUNT,
                    ),
                    "opaque_full_panel" => tiles(
                        &mut clear_pass(&mut enc, &opaque_panel),
                        &p.opaque,
                        &ob,
                        COUNT,
                    ),
                    "mixed_full_panel" => tiles(
                        &mut clear_pass(&mut enc, &opaque_panel),
                        &p.mixed,
                        &ab,
                        COUNT,
                    ),
                    "mixed_changed_cells" => tiles(
                        &mut pass(&mut enc, &opaque_panel, wgpu::LoadOp::Load),
                        &p.mixed,
                        &ab,
                        changed,
                    ),
                    "alpha_restore_changed_cells" => {
                        let mut pass = pass(&mut enc, &opaque_panel, wgpu::LoadOp::Load);
                        tiles(&mut pass, &p.background, &ab, changed);
                        tiles(&mut pass, &p.alpha, &ab, changed);
                    }
                    "opaque_changed_cells" => tiles(
                        &mut pass(&mut enc, &opaque_panel, wgpu::LoadOp::Load),
                        &p.opaque,
                        &ob,
                        changed,
                    ),
                    _ => unreachable!(),
                }
                enc.write_timestamp(&timer.query, f * 2 + 1);
                queue.submit(Some(enc.finish()));
            }
            let (med, p95) = stats(timer.finish(device, queue));
            eprintln!("OPAQUE,{kind},{changed},{med:.2},{p95:.2}");
            if kind != "transparent_full_layer" {
                compare(
                    &pixels(device, queue, &transparent_final),
                    &pixels(device, queue, &opaque_panel),
                );
            }
        }
    }

    // Identical native-resolution pixels: two-layer opaque+alpha composition
    // vs one premixed panel. No scaling, filtering or alpha conversion here.
    for kind in ["compose_two_layers", "compose_one_opaque"] {
        let timer = Timings::new(device);
        for f in 0..SAMPLES {
            let mut enc = device.create_command_encoder(&Default::default());
            enc.write_timestamp(&timer.query, f * 2);
            {
                let mut pass = pass(&mut enc, &transparent_final, wgpu::LoadOp::Load);
                if kind == "compose_two_layers" {
                    full(&mut pass, &p.full_opaque, &bgb);
                    full(&mut pass, &p.full_alpha, &apb);
                } else {
                    full(&mut pass, &p.full_opaque, &opb);
                }
            }
            enc.write_timestamp(&timer.query, f * 2 + 1);
            queue.submit(Some(enc.finish()));
        }
        let (med, p95) = stats(timer.finish(device, queue));
        eprintln!("OPAQUE,{kind},256,{med:.2},{p95:.2}");
    }
}
