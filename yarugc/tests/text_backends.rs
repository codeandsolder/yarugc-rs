// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

//! Native-resolution backend comparison. Run on a GPU host in release mode.
#![cfg(not(target_arch = "wasm32"))]

use std::{sync::Arc, time::Instant};
#[path = "support/compute.rs"]
mod compute;
#[path = "support/opaque.rs"]
mod opaque;

use glyphon::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping};
use masonry_core::kurbo::Affine;
use peniko::{Blob, Color, Fill, FontData};
use wgpu::{CommandEncoder, Device, Queue, RenderPass};
use yarugc::{AlphaMode, Compositor, Surface};

const WIDTH: u32 = 2048;
const HEIGHT: u32 = 512;
const COUNT: usize = 256;
const CELL_WIDTH: u32 = 128;
const CELL_HEIGHT: u32 = 32;
const SIZE: f32 = 18.0;
const SAMPLES: u32 = 48;
const FONT: &[u8] =
    include_bytes!("../../masonry_testing/resources/fonts/roboto/Roboto-Regular.ttf");
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const ALPHABET: &str = "0123456789.-+− kmµ/s°";

fn origin(i: usize) -> [f32; 2] {
    [
        (i % 16) as f32 * CELL_WIDTH as f32 + 4.0,
        (i / 16) as f32 * CELL_HEIGHT as f32 + 4.0,
    ]
}

fn strings(i: usize) -> [String; 2] {
    [
        format!("{} m", 700 + i),
        format!("{}.{:02} km", 1 + i / 100, i % 100),
    ]
}

fn font_system() -> FontSystem {
    let mut db = glyphon::fontdb::Database::new();
    db.load_font_data(FONT.to_vec());
    FontSystem::new_with_locale_and_db("en-US".to_owned(), db)
}

fn shape(fs: &mut FontSystem, text: &str) -> Buffer {
    let mut b = Buffer::new(fs, Metrics::new(SIZE, 24.0));
    b.set_size(Some(120.0), Some(24.0));
    b.set_text(
        text,
        &Attrs::new().family(Family::Name("Roboto")),
        Shaping::Advanced,
        None,
    );
    b.shape_until_scroll(fs, false);
    assert_eq!(b.layout_runs().count(), 1);
    b
}

fn selected(frame: u32, changed: usize, i: usize) -> usize {
    2 * i + usize::from(i < changed && frame % 2 == 0)
}

fn clear_pass<'a>(encoder: &'a mut CommandEncoder, surface: &Surface) -> RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: surface.view(),
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    })
}

fn read_buffer(device: &Device, buffer: &wgpu::Buffer) -> Vec<u8> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let bytes = buffer.slice(..).get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    bytes
}

fn pixels(device: &Device, queue: &Queue, target: &Surface) -> Vec<u8> {
    let [w, h] = target.size();
    assert_eq!(w * 4 % 256, 0);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(w) * u64::from(h) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        target.texture().as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
        },
        target.texture().size(),
    );
    queue.submit(Some(encoder.finish()));
    read_buffer(device, &buffer)
}

fn verify_ink(device: &Device, queue: &Queue, target: &Surface) {
    let p = pixels(device, queue, target);
    for i in 0..COUNT {
        let x = (i % 16) * CELL_WIDTH as usize;
        let y = (i / 16) * CELL_HEIGHT as usize;
        let ink = (y..y + CELL_HEIGHT as usize)
            .map(|r| {
                let start = (r * WIDTH as usize + x) * 4;
                p[start..start + CELL_WIDTH as usize * 4]
                    .chunks_exact(4)
                    .filter(|c| c[3] > 16)
                    .count()
            })
            .sum::<usize>();
        assert!(
            ink > 20 && ink < 1500,
            "slot {i} has unexpected coverage {ink}"
        );
    }
}

struct Timings {
    query: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
}
impl Timings {
    fn new(device: &Device) -> Self {
        let size = u64::from(SAMPLES * 2) * 8;
        Self {
            query: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: None,
                ty: wgpu::QueryType::Timestamp,
                count: SAMPLES * 2,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            read: device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
        }
    }

    fn finish(&self, device: &Device, queue: &Queue) -> Vec<f64> {
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.resolve_query_set(&self.query, 0..SAMPLES * 2, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(
            &self.resolve,
            0,
            &self.read,
            0,
            Some(u64::from(SAMPLES * 2) * 8),
        );
        queue.submit(Some(encoder.finish()));
        let bytes = read_buffer(device, &self.read);
        bytemuck::cast_slice::<u8, u64>(&bytes)
            .chunks_exact(2)
            .map(|s| (s[1] - s[0]) as f64 * f64::from(queue.get_timestamp_period()) / 1000.0)
            .collect()
    }
}

fn stats(mut a: Vec<f64>) -> (f64, f64) {
    a.sort_by(f64::total_cmp);
    (a[a.len() / 2], a[a.len() * 95 / 100])
}

trait Backend {
    fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        encoder: &mut CommandEncoder,
        frame: u32,
        changed: usize,
    );
    fn draw(&mut self, pass: &mut RenderPass<'_>);
}

fn bench(
    name: &str,
    device: &Device,
    queue: &Queue,
    target: &Surface,
    renderer: &mut impl Backend,
) {
    // Warm glyph caches and both content states. No other test runs concurrently.
    let cold = Instant::now();
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.prepare(device, queue, &mut encoder, 0, COUNT);
    let cold_prepare = cold.elapsed().as_secs_f64() * 1e6;
    renderer.draw(&mut clear_pass(&mut encoder, target));
    queue.submit(Some(encoder.finish()));
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    eprintln!("COLD,{name},prepare_us,{cold_prepare:.2}");
    for f in 0..16 {
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.prepare(device, queue, &mut encoder, f, COUNT);
        renderer.draw(&mut clear_pass(&mut encoder, target));
        queue.submit(Some(encoder.finish()));
    }
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    verify_ink(device, queue, target);
    for changed in [1, 8, 64, COUNT] {
        let timer = Timings::new(device);
        let mut cpu = Vec::new();
        let mut encode = Vec::new();
        for f in 0..SAMPLES {
            let mut encoder = device.create_command_encoder(&Default::default());
            let t = Instant::now();
            renderer.prepare(device, queue, &mut encoder, f, changed);
            cpu.push(t.elapsed().as_secs_f64() * 1e6);
            let t = Instant::now();
            encoder.write_timestamp(&timer.query, f * 2);
            renderer.draw(&mut clear_pass(&mut encoder, target));
            encoder.write_timestamp(&timer.query, f * 2 + 1);
            queue.submit(Some(encoder.finish()));
            encode.push(t.elapsed().as_secs_f64() * 1e6);
        }
        let (c, c95) = stats(cpu);
        let (e, e95) = stats(encode);
        let (g, g95) = stats(timer.finish(device, queue));
        eprintln!("TEXT,{name},{changed},{c:.2},{c95:.2},{e:.2},{e95:.2},{g:.2},{g95:.2}");
    }
    verify_ink(device, queue, target);
}

struct Glyphon {
    fs: FontSystem,
    buffers: Vec<Buffer>,
    atlas: glyphon::TextAtlas,
    renderer: glyphon::TextRenderer,
    viewport: glyphon::Viewport,
    swash: glyphon::SwashCache,
}
impl Glyphon {
    fn new(device: &Device, queue: &Queue) -> Self {
        let mut fs = font_system();
        let buffers = (0..COUNT)
            .flat_map(strings)
            .map(|s| shape(&mut fs, &s))
            .collect();
        let cache = glyphon::Cache::new(device);
        let mut atlas = glyphon::TextAtlas::new(device, queue, &cache, FORMAT);
        let renderer = glyphon::TextRenderer::new(&mut atlas, device, Default::default(), None);
        let mut viewport = glyphon::Viewport::new(device, &cache);
        viewport.update(
            queue,
            glyphon::Resolution {
                width: WIDTH,
                height: HEIGHT,
            },
        );
        Self {
            fs,
            buffers,
            atlas,
            renderer,
            viewport,
            swash: glyphon::SwashCache::new(),
        }
    }
}
impl Backend for Glyphon {
    fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        _: &mut CommandEncoder,
        frame: u32,
        changed: usize,
    ) {
        let areas = (0..COUNT).map(|i| {
            let [left, top] = origin(i);
            glyphon::TextArea {
                buffer: &self.buffers[selected(frame, changed, i)],
                left,
                top,
                scale: 1.0,
                bounds: glyphon::TextBounds {
                    left: left as i32 - 4,
                    top: top as i32 - 4,
                    right: left as i32 + 124,
                    bottom: top as i32 + 28,
                },
                default_color: glyphon::Color::rgb(255, 255, 255),
                custom_glyphs: &[],
            }
        });
        self.renderer
            .prepare(
                device,
                queue,
                &mut self.fs,
                &mut self.atlas,
                &self.viewport,
                areas,
                &mut self.swash,
            )
            .unwrap();
    }
    fn draw(&mut self, pass: &mut RenderPass<'_>) {
        self.renderer
            .render(&self.atlas, &self.viewport, pass)
            .unwrap();
    }
}

struct Slug {
    fs: FontSystem,
    buffers: Vec<Buffer>,
    atlas: sluggrs_skylines::TextAtlas,
    renderer: sluggrs_skylines::TextRenderer,
    viewport: sluggrs_skylines::Viewport,
}
impl Slug {
    fn new(device: &Device, queue: &Queue) -> Self {
        let mut fs = font_system();
        let buffers = (0..COUNT)
            .flat_map(strings)
            .map(|s| shape(&mut fs, &s))
            .collect();
        let cache = sluggrs_skylines::Cache::new(device);
        let mut atlas = sluggrs_skylines::TextAtlas::new(device, queue, &cache, FORMAT);
        let renderer =
            sluggrs_skylines::TextRenderer::new(&mut atlas, device, Default::default(), None);
        let mut viewport = sluggrs_skylines::Viewport::new(device, &cache);
        viewport.update(
            queue,
            sluggrs_skylines::types::Resolution {
                width: WIDTH,
                height: HEIGHT,
            },
        );
        Self {
            fs,
            buffers,
            atlas,
            renderer,
            viewport,
        }
    }
}
impl Backend for Slug {
    fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        encoder: &mut CommandEncoder,
        frame: u32,
        changed: usize,
    ) {
        let areas = (0..COUNT).map(|i| {
            let [left, top] = origin(i);
            sluggrs_skylines::types::TextArea {
                buffer: &self.buffers[selected(frame, changed, i)],
                left,
                top,
                scale: 1.0,
                bounds: sluggrs_skylines::types::TextBounds {
                    left: left as i32 - 4,
                    top: top as i32 - 4,
                    right: left as i32 + 124,
                    bottom: top as i32 + 28,
                },
                default_color: glyphon::Color::rgb(255, 255, 255),
                decorations: &[],
                rotation: 0.0,
                rotation_origin: [0.0, 0.0],
            }
        });
        self.renderer
            .prepare(
                device,
                queue,
                encoder,
                &mut self.fs,
                &mut self.atlas,
                &self.viewport,
                areas,
            )
            .unwrap();
    }
    fn draw(&mut self, pass: &mut RenderPass<'_>) {
        self.renderer
            .render(&self.atlas, &self.viewport, pass)
            .unwrap();
    }
}

struct WgpuFont {
    fonts: wgpu_font_renderer::FontStore,
    paragraphs: Vec<wgpu_font_renderer::Paragraph>,
    renderer: wgpu_font_renderer::TextRenderer,
}
impl WgpuFont {
    fn new(device: &Device, queue: &Queue, mode: wgpu_font_renderer::RenderMode) -> Self {
        let mut fonts = wgpu_font_renderer::FontStore::new(device);
        let key = fonts
            .load_from_bytes(device, queue, FONT, ALPHABET)
            .unwrap();
        let mut writer = wgpu_font_renderer::TypeWriter::new();
        let mut paragraphs = Vec::new();
        for i in 0..COUNT {
            for s in strings(i) {
                let mut p = writer
                    .shape_text(&fonts, key, origin(i), SIZE, [1.0; 4], &s)
                    .unwrap();
                p.set_render_mode(mode);
                paragraphs.push(p);
            }
        }
        let mut renderer = wgpu_font_renderer::TextRenderer::new(device, FORMAT, &fonts);
        renderer.resize(queue, [WIDTH, HEIGHT]);
        Self {
            fonts,
            paragraphs,
            renderer,
        }
    }
}
impl Backend for WgpuFont {
    fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        _: &mut CommandEncoder,
        frame: u32,
        changed: usize,
    ) {
        self.renderer.prepare(
            device,
            queue,
            (0..COUNT).map(|i| &self.paragraphs[selected(frame, changed, i)]),
            &self.fonts,
        );
    }
    fn draw(&mut self, pass: &mut RenderPass<'_>) {
        self.renderer.render(pass);
    }
}

fn bench_vello(device: &Device, queue: &Queue, target: &Surface) {
    let mut fs = font_system();
    let font = FontData::new(Blob::new(Arc::new(FONT.to_vec())), 0);
    let mut runs = Vec::new();
    for i in 0..COUNT {
        for s in strings(i) {
            let b = shape(&mut fs, &s);
            let [x, y] = origin(i);
            let mut glyphs = Vec::new();
            for run in b.layout_runs() {
                for g in run.glyphs {
                    glyphs.push(vello::Glyph {
                        id: u32::from(g.glyph_id),
                        x: x + g.x + g.font_size * g.x_offset,
                        y: y + run.line_y + g.y - g.font_size * g.y_offset,
                    });
                }
            }
            runs.push(glyphs);
        }
    }
    let mut renderer = vello::Renderer::new(device, Default::default()).unwrap();
    let params = vello::RenderParams {
        base_color: Color::TRANSPARENT,
        width: WIDTH,
        height: HEIGHT,
        antialiasing_method: vello::AaConfig::Area,
    };
    let mut scene = vello::Scene::new();
    let prepare = |scene: &mut vello::Scene, frame, changed| {
        scene.reset();
        for i in 0..COUNT {
            scene
                .draw_glyphs(&font)
                .font_size(SIZE)
                .transform(Affine::IDENTITY)
                .brush(Color::WHITE)
                .draw(
                    Fill::NonZero,
                    runs[selected(frame, changed, i)].iter().copied(),
                );
        }
    };
    let cold = Instant::now();
    prepare(&mut scene, 0, COUNT);
    renderer
        .render_to_texture(device, queue, &scene, target.view(), &params)
        .unwrap();
    eprintln!(
        "COLD,vello,prepare_and_submit_us,{:.2}",
        cold.elapsed().as_secs_f64() * 1e6
    );
    for f in 0..12 {
        prepare(&mut scene, f, COUNT);
        renderer
            .render_to_texture(device, queue, &scene, target.view(), &params)
            .unwrap();
    }
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    verify_ink(device, queue, target);
    for changed in [1, 8, 64, COUNT] {
        let timer = Timings::new(device);
        let mut cpu = Vec::new();
        let mut submit = Vec::new();
        for f in 0..SAMPLES {
            let t = Instant::now();
            prepare(&mut scene, f, changed);
            cpu.push(t.elapsed().as_secs_f64() * 1e6);
            let mut e = device.create_command_encoder(&Default::default());
            e.write_timestamp(&timer.query, f * 2);
            queue.submit(Some(e.finish()));
            let t = Instant::now();
            renderer
                .render_to_texture(device, queue, &scene, target.view(), &params)
                .unwrap();
            submit.push(t.elapsed().as_secs_f64() * 1e6);
            let mut e = device.create_command_encoder(&Default::default());
            e.write_timestamp(&timer.query, f * 2 + 1);
            queue.submit(Some(e.finish()));
        }
        let (c, c95) = stats(cpu);
        let (e, e95) = stats(submit);
        let (g, g95) = stats(timer.finish(device, queue));
        eprintln!("TEXT,vello_bracketed,{changed},{c:.2},{c95:.2},{e:.2},{e95:.2},{g:.2},{g95:.2}");
    }
    verify_ink(device, queue, target);
}

static GPU_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn benchmark_device() -> (Device, Queue) {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let features =
        wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: features,
        ..Default::default()
    }))
    .unwrap();
    eprintln!("ADAPTER,{:?}", adapter.get_info());
    eprintln!(
        "TEXT,backend,changed,prepare_med_us,prepare_p95_us,encode_submit_med_us,encode_submit_p95_us,gpu_med_us,gpu_p95_us"
    );
    (device, queue)
}

#[test]
#[ignore = "requires timestamp-capable GPU; use release mode"]
fn text_backends() {
    let _guard = GPU_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (device, queue) = benchmark_device();
    let compositor = Compositor::new(&device, FORMAT);
    let target = compositor
        .create_surface([WIDTH, HEIGHT], AlphaMode::Premultiplied)
        .unwrap();
    bench(
        "glyphon",
        &device,
        &queue,
        &target,
        &mut Glyphon::new(&device, &queue),
    );
    bench(
        "sluggrs",
        &device,
        &queue,
        &target,
        &mut Slug::new(&device, &queue),
    );
    bench(
        "wgpu_font_analytic",
        &device,
        &queue,
        &target,
        &mut WgpuFont::new(&device, &queue, wgpu_font_renderer::RenderMode::Analytic),
    );
    bench_vello(&device, &queue, &target);
}

#[test]
#[ignore = "requires timestamp-capable GPU; use release mode"]
fn cached_vector_text() {
    let _guard = GPU_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (device, queue) = benchmark_device();
    let compositor = Compositor::new(&device, FORMAT);
    let target = compositor
        .create_surface([WIDTH, HEIGHT], AlphaMode::Premultiplied)
        .unwrap();
    bench(
        "wgpu_font_cached",
        &device,
        &queue,
        &target,
        &mut WgpuFont::new(&device, &queue, wgpu_font_renderer::RenderMode::Cached),
    );
}

#[test]
#[ignore = "requires timestamp-capable GPU; use release mode"]
fn opaque_glyph_cells() {
    let _guard = GPU_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (device, queue) = benchmark_device();
    let compositor = Compositor::new(&device, FORMAT);
    opaque::run(&device, &queue, &compositor);
}

#[test]
#[ignore = "requires timestamp-capable GPU; use release mode"]
fn compute_glyph_cells() {
    let _guard = GPU_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (device, queue) = benchmark_device();
    let compositor = Compositor::new(&device, FORMAT);
    compute::run(&device, &queue, &compositor);
}
