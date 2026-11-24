// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

//! Fixed-cell direct writes: native-size inverse coverage, no blending.

use super::*;
use wgpu::util::DeviceExt;

const AW: u32 = 256;
const AH: u32 = 32;
const TILE_W: u32 = 16;
const TILE_H: u32 = 32;
const CELLS: usize = COUNT * 8;

#[derive(Clone, Copy)]
struct Bounds {
    min: [u32; 2],
    max: [u32; 2],
}

impl Bounds {
    fn empty() -> Self {
        Self {
            min: [TILE_W, TILE_H],
            max: [0, 0],
        }
    }

    fn include(&mut self, x: u32, y: u32) {
        self.min[0] = self.min[0].min(x);
        self.min[1] = self.min[1].min(y);
        self.max[0] = self.max[0].max(x + 1);
        self.max[1] = self.max[1].max(y + 1);
    }

    fn union(self, other: Self) -> Self {
        Self {
            min: [self.min[0].min(other.min[0]), self.min[1].min(other.min[1])],
            max: [self.max[0].max(other.max[0]), self.max[1].max(other.max[1])],
        }
    }

    fn extent(self) -> [u32; 2] {
        [
            self.max[0].saturating_sub(self.min[0]),
            self.max[1].saturating_sub(self.min[1]),
        ]
    }
}

fn pack(a: [u32; 2]) -> u32 {
    a[0] | (a[1] << 16)
}

fn jobs(old: &[u32], new: &[u32], bounds: &[Bounds], changed_only: bool) -> Vec<[u32; 4]> {
    old.iter()
        .zip(new)
        .enumerate()
        .filter_map(|(i, (&a, &b))| {
            if changed_only && a == b {
                return None;
            }
            let r = bounds[a as usize].union(bounds[b as usize]);
            let size = r.extent();
            if changed_only && size.contains(&0) {
                return None;
            }
            Some([i as u32, b, pack(r.min), pack(size)])
        })
        .collect()
}

fn atlas_texture(
    device: &Device,
    queue: &Queue,
    format: wgpu::TextureFormat,
    width: u32,
    data: &[u8],
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("inverse coverage atlas"),
        size: wgpu::Extent3d {
            width,
            height: AH,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width),
            rows_per_image: Some(AH),
        },
        texture.size(),
    );
    texture.create_view(&Default::default())
}

struct Fixture {
    mask8: wgpu::TextureView,
    mask4: wgpu::TextureView,
    inverse: Vec<u8>,
    bounds: Vec<Bounds>,
    layout: wgpu::BindGroupLayout,
    graphics_layout: wgpu::BindGroupLayout,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
}
impl Fixture {
    fn new(device: &Device, queue: &Queue, compositor: &Compositor) -> Self {
        let (alpha, _) = opaque::glyph_atlas(device, queue, compositor);
        let inverse: Vec<u8> = pixels(device, queue, &alpha)
            .chunks_exact(4)
            .map(|p| 255 - p[3])
            .collect();
        let packed: Vec<u8> = inverse
            .chunks_exact(2)
            .map(|p| {
                let q = |a: u8| ((u16::from(a) + 8) / 17) as u8;
                q(p[0]) | (q(p[1]) << 4)
            })
            .collect();
        let mut bounds = vec![Bounds::empty(); (AW / TILE_W) as usize];
        for y in 0..AH {
            for x in 0..AW {
                if inverse[(y * AW + x) as usize] != 255 {
                    bounds[(x / TILE_W) as usize].include(x % TILE_W, y);
                }
            }
        }
        let mask8 = atlas_texture(device, queue, wgpu::TextureFormat::R8Unorm, AW, &inverse);
        let mask4 = atlas_texture(device, queue, wgpu::TextureFormat::R8Uint, AW / 2, &packed);
        let vis = wgpu::ShaderStages::COMPUTE;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: vis,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: vis,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: vis,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: vis,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let graphics_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyph overwrite graphics layout"),
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
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
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
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compute glyph cells"),
            source: wgpu::ShaderSource::Wgsl(include_str!("compute.wgsl").into()),
        });
        Self {
            mask8,
            mask4,
            inverse,
            bounds,
            layout,
            graphics_layout,
            shader,
            pipeline_layout,
        }
    }

    fn bind(&self, device: &Device, buffer: &wgpu::Buffer, target: &Surface) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.mask8),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.mask4),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(target.view()),
                },
            ],
        })
    }

    fn pipeline(&self, device: &Device, variant: Variant) -> wgpu::ComputePipeline {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(variant.name),
            layout: Some(&self.pipeline_layout),
            module: &self.shader,
            entry_point: Some("write_cells"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &[
                    ("packed", f64::from(u8::from(variant.packed))),
                    ("white", f64::from(u8::from(variant.white))),
                    ("tight", f64::from(u8::from(variant.tight))),
                    ("lanes", f64::from(variant.lanes)),
                ],
                ..Default::default()
            },
            cache: None,
        })
    }

    fn graphics(
        &self,
        device: &Device,
        buffer: &wgpu::Buffer,
        tight: bool,
    ) -> (wgpu::RenderPipeline, wgpu::BindGroup) {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&self.graphics_layout)],
            immediate_size: 0,
        });
        let constants = [
            ("tight", f64::from(u8::from(tight))),
            ("packed", 0.0),
            ("white", 1.0),
        ];
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.graphics_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.mask8),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.mask4),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buffer.as_entire_binding(),
                },
            ],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glyph overwrite graphics"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("glyph_vs"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some("glyph_fs"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        (pipeline, binding)
    }

    fn expected(&self, ids: &[u32], packed: bool, white: bool) -> Vec<u8> {
        let mut result = vec![0; (WIDTH * HEIGHT * 4) as usize];
        for (i, &glyph) in ids.iter().enumerate() {
            let slot = i / 8;
            let row = slot / 16;
            let col = slot % 16;
            let bg = if (row + col) % 2 == 0 {
                [16u32, 24, 32]
            } else {
                [24u32, 32, 40]
            };
            for y in 0..TILE_H {
                for x in 0..TILE_W {
                    let p = self.inverse[(y * AW + glyph * TILE_W + x) as usize];
                    let t = if packed {
                        u32::from((u16::from(p) + 8) / 17) * 17
                    } else {
                        u32::from(p)
                    };
                    let dx = col * 128 + (i % 8) * 16 + x as usize;
                    let dy = row * 32 + y as usize;
                    let offset = (dy * WIDTH as usize + dx) * 4;
                    for c in 0..3 {
                        result[offset + c] = if white {
                            255 - (((255 - bg[c]) * t + 127) / 255) as u8
                        } else {
                            ((bg[c] * t + 127) / 255) as u8
                        };
                    }
                    result[offset + 3] = 255;
                }
            }
        }
        result
    }
}

#[derive(Clone, Copy)]
struct Variant {
    name: &'static str,
    packed: bool,
    white: bool,
    tight: bool,
    changed_only: bool,
    lanes: u32,
}

fn encode(
    encoder: &mut CommandEncoder,
    pipeline: &wgpu::ComputePipeline,
    binding: &wgpu::BindGroup,
    n: usize,
) {
    if n == 0 {
        return;
    }
    let mut pass = encoder.begin_compute_pass(&Default::default());
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, binding, &[]);
    pass.dispatch_workgroups(n as u32, 1, 1);
}

pub(super) fn run(device: &Device, queue: &Queue, compositor: &Compositor) {
    let f = Fixture::new(device, queue, compositor);
    let panel = compositor
        .create_surface([WIDTH, HEIGHT], AlphaMode::Premultiplied)
        .unwrap();
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&vec![[0u32; 4]; CELLS]),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let binding = f.bind(device, &buffer, &panel);
    let a = opaque::glyph_ids(1, COUNT);
    let base = Variant {
        name: "r8_full_cells",
        packed: false,
        white: true,
        tight: false,
        changed_only: false,
        lanes: 64,
    };
    let variants = [
        base,
        Variant {
            name: "r8_changed_glyph",
            changed_only: true,
            ..base
        },
        Variant {
            name: "r8_tight",
            changed_only: true,
            tight: true,
            ..base
        },
        Variant {
            name: "r8_tight_32",
            lanes: 32,
            changed_only: true,
            tight: true,
            ..base
        },
        Variant {
            name: "r8_tight_128",
            lanes: 128,
            changed_only: true,
            tight: true,
            ..base
        },
        Variant {
            name: "r4_tight",
            packed: true,
            changed_only: true,
            tight: true,
            ..base
        },
        Variant {
            name: "r8_black_tight",
            white: false,
            changed_only: true,
            tight: true,
            ..base
        },
    ];
    eprintln!(
        "COMPUTE,variant,changed_readouts,jobs,pixels_written,cpu_update_med_us,gpu_med_us,gpu_p95_us"
    );
    // Prepare both directions of each transition once, so timing excludes
    // formatting/shaping. Time list construction and upload separately.
    for variant in variants {
        let p = f.pipeline(device, variant);
        let init = f.pipeline(
            device,
            Variant {
                tight: false,
                ..variant
            },
        );
        let all = jobs(&a, &a, &f.bounds, false);
        for changed in [1, 8, 64, COUNT] {
            let b = opaque::glyph_ids(0, changed);
            let begin = jobs(&a, &b, &f.bounds, variant.changed_only);
            let end = jobs(&b, &a, &f.bounds, variant.changed_only);
            let count = begin.len();
            let work = if variant.tight {
                begin
                    .iter()
                    .map(|j| u64::from(j[3] & 65535) * u64::from(j[3] >> 16))
                    .sum::<u64>()
            } else {
                count as u64 * 512
            };
            queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&all));
            let mut enc = device.create_command_encoder(&Default::default());
            encode(&mut enc, &init, &binding, all.len());
            queue.submit(Some(enc.finish()));
            opaque::compare(
                &pixels(device, queue, &panel),
                &f.expected(&a, variant.packed, variant.white),
            );
            // Validate BOTH directions including ink erasure, and unchanged cells.
            for (list, expected) in [(&begin, &b), (&end, &a)] {
                queue.write_buffer(&buffer, 0, bytemuck::cast_slice(list));
                let mut enc = device.create_command_encoder(&Default::default());
                encode(&mut enc, &p, &binding, list.len());
                queue.submit(Some(enc.finish()));
                opaque::compare(
                    &pixels(device, queue, &panel),
                    &f.expected(expected, variant.packed, variant.white),
                );
            }
            let timer = Timings::new(device);
            let mut cpu = Vec::new();
            for sample in 0..SAMPLES {
                let start = Instant::now();
                let (old, new) = if sample % 2 == 0 { (&a, &b) } else { (&b, &a) };
                let list = jobs(old, new, &f.bounds, variant.changed_only);
                queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&list));
                cpu.push(start.elapsed().as_secs_f64() * 1e6);
                let mut enc = device.create_command_encoder(&Default::default());
                enc.write_timestamp(&timer.query, sample * 2);
                encode(&mut enc, &p, &binding, list.len());
                enc.write_timestamp(&timer.query, sample * 2 + 1);
                queue.submit(Some(enc.finish()));
            }
            let (c, _) = stats(cpu);
            let (g, p95) = stats(timer.finish(device, queue));
            eprintln!(
                "COMPUTE,{},{changed},{count},{work},{c:.2},{g:.2},{p95:.2}",
                variant.name
            );
        }
    }
    // Graphics overwrite with the identical sparse job list, bounds and atlas.
    for tight in [false, true] {
        let (pipeline, bind) = f.graphics(device, &buffer, tight);
        let init = f.pipeline(device, base);
        let all = jobs(&a, &a, &f.bounds, false);
        for changed in [1, 8, 64, COUNT] {
            let b = opaque::glyph_ids(0, changed);
            let begin = jobs(&a, &b, &f.bounds, true);
            let end = jobs(&b, &a, &f.bounds, true);
            let count = begin.len();
            let work = if tight {
                begin
                    .iter()
                    .map(|j| u64::from(j[3] & 65535) * u64::from(j[3] >> 16))
                    .sum::<u64>()
            } else {
                count as u64 * 512
            };
            queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&all));
            let mut enc = device.create_command_encoder(&Default::default());
            encode(&mut enc, &init, &binding, all.len());
            queue.submit(Some(enc.finish()));
            let draw = |enc: &mut CommandEncoder, n: usize| {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: panel.view(),
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..4, 0..n as u32);
            };
            for (list, expected) in [(&begin, &b), (&end, &a)] {
                queue.write_buffer(&buffer, 0, bytemuck::cast_slice(list));
                let mut enc = device.create_command_encoder(&Default::default());
                draw(&mut enc, list.len());
                queue.submit(Some(enc.finish()));
                opaque::compare(
                    &pixels(device, queue, &panel),
                    &f.expected(expected, false, true),
                );
            }
            let timer = Timings::new(device);
            let mut cpu = Vec::new();
            for sample in 0..SAMPLES {
                let start = Instant::now();
                let (old, new) = if sample % 2 == 0 { (&a, &b) } else { (&b, &a) };
                let list = jobs(old, new, &f.bounds, true);
                queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&list));
                cpu.push(start.elapsed().as_secs_f64() * 1e6);
                let mut enc = device.create_command_encoder(&Default::default());
                enc.write_timestamp(&timer.query, sample * 2);
                draw(&mut enc, list.len());
                enc.write_timestamp(&timer.query, sample * 2 + 1);
                queue.submit(Some(enc.finish()));
            }
            let (c, _) = stats(cpu);
            let (g, p95) = stats(timer.finish(device, queue));
            let name = if tight {
                "graphics_r8_tight"
            } else {
                "graphics_r8_changed_glyph"
            };
            eprintln!("COMPUTE,{name},{changed},{count},{work},{c:.2},{g:.2},{p95:.2}");
        }
    }
    let expected8 = f.expected(&a, false, true);
    let expected4 = f.expected(&a, true, true);
    let errors: Vec<u8> = expected8
        .iter()
        .zip(&expected4)
        .map(|(a, b)| a.abs_diff(*b))
        .collect();
    eprintln!(
        "COMPUTE,quantization,max_error,{},mean_abs_error,{:.4},atlas_r8_bytes,{},atlas_r4_bytes,{}",
        errors.iter().max().unwrap(),
        errors.iter().map(|&e| f64::from(e)).sum::<f64>() / errors.len() as f64,
        AW * AH,
        AW * AH / 2
    );
}
