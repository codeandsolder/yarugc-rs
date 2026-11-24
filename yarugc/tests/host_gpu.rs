// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

//! Real GPU integration with host-owned rendering and readback.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use masonry::widgets::SizedBox;
use masonry_core::app::{RenderRootOptions, WindowSizePolicy};
use masonry_core::core::{DefaultProperties, NewWidget, WindowEvent};
use masonry_core::dpi::PhysicalSize;
use masonry_core::properties::{Background, Dimensions};
use masonry_imaging::texture_render::Renderer;
use peniko::Color;
use yarugc::{AlphaMode, CachedWindow, Composition, Compositor, Layer, Refresh, Surface};

const EXTENT: [u32; 2] = [64, 16];

fn read_composition(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    compositor: &Compositor,
    composition: &Composition,
    target: &Surface,
) -> Vec<u8> {
    let extent = target.size();
    assert_eq!(extent[0] * 4 % 256, 0, "readback rows must be aligned");
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("host readback"),
        size: u64::from(extent[0] * extent[1] * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    // A real host would already have drawn its scene. Clear in an independent
    // pass and load it in the compositor pass to test the actual integration.
    for load in [wgpu::LoadOp::Clear(wgpu::Color::BLUE), wgpu::LoadOp::Load] {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
        });
        if load == wgpu::LoadOp::Load {
            compositor.draw(&mut pass, composition);
        }
    }
    encoder.copy_texture_to_buffer(
        target.texture().as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(extent[0] * 4),
                rows_per_image: Some(extent[1]),
            },
        },
        target.texture().size(),
    );
    queue.submit(Some(encoder.finish()));
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let bytes = buffer.slice(..).get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    bytes
}

fn assert_pixel(bytes: &[u8], x: u32, y: u32, expected: [u8; 4]) {
    let offset = usize::try_from((y * EXTENT[0] + x) * 4).unwrap();
    let actual = &bytes[offset..offset + 4];
    assert!(
        actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2),
        "pixel ({x}, {y}): {actual:?} != {expected:?}"
    );
}

fn write_dynamic(queue: &wgpu::Queue, surface: &Surface, color: [u8; 4]) {
    queue.write_texture(
        surface.texture().as_image_copy(),
        &color.repeat(4),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8),
            rows_per_image: Some(2),
        },
        surface.texture().size(),
    );
}

#[test]
#[ignore = "requires a working wgpu adapter; run explicitly on GPU hosts"]
fn cached_window_and_dynamic_layer_on_host_device() {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let compositor = Compositor::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    let mut renderer = Renderer::new();
    let mut window = CachedWindow::new(
        NewWidget::new(SizedBox::empty())
            .with_props(Dimensions::STRETCH)
            .with_props(Background::Color(Color::from_rgba8(255, 0, 0, 128))),
        RenderRootOptions {
            default_properties: Arc::new(DefaultProperties::new()),
            use_system_fonts: false,
            size_policy: WindowSizePolicy::User,
            size: PhysicalSize::new(4, 4),
            scale_factor: 1.0,
            test_font: None,
        },
        |_| {},
    );
    let refresh = |w: &mut CachedWindow, r: &mut Renderer| {
        w.refresh(&compositor, r, &adapter, &device, &queue)
            .unwrap()
    };
    assert!(matches!(
        refresh(&mut window, &mut renderer),
        Refresh::Painted {
            reallocated: true,
            ..
        }
    ));
    assert!(matches!(
        refresh(&mut window, &mut renderer),
        Refresh::Unchanged
    ));

    let dynamic = compositor
        .create_surface([2, 2], AlphaMode::Premultiplied)
        .unwrap();
    let target = compositor
        .create_surface(EXTENT, AlphaMode::Premultiplied)
        .unwrap();
    write_dynamic(&queue, &dynamic, [0, 128, 0, 128]);
    let prepare = |w: &CachedWindow, x: f64| {
        compositor.prepare(
            EXTENT,
            &[
                Layer::at(w.surface().unwrap(), [x, 4.0]),
                Layer::at(&dynamic, [x + 1.0, 5.0]),
            ],
        )
    };
    let composition = prepare(&window, 4.0);
    let pixels = read_composition(&device, &queue, &compositor, &composition, &target);
    assert_pixel(&pixels, 0, 0, [0, 0, 255, 255]);
    assert_pixel(&pixels, 4, 4, [128, 0, 127, 255]);
    assert_pixel(&pixels, 5, 5, [64, 128, 63, 255]);

    // Changing only dynamic data preserves the static texture AND the
    // prepared composition, so the root never performs another paint.
    write_dynamic(&queue, &dynamic, [128, 128, 0, 128]);
    assert!(matches!(
        refresh(&mut window, &mut renderer),
        Refresh::Unchanged
    ));
    let pixels = read_composition(&device, &queue, &compositor, &composition, &target);
    assert_pixel(&pixels, 5, 5, [192, 128, 63, 255]);

    // Moving the window requires only a placement update, with viewport clipping.
    let moved = prepare(&window, -1.0);
    assert!(matches!(
        refresh(&mut window, &mut renderer),
        Refresh::Unchanged
    ));
    let pixels = read_composition(&device, &queue, &compositor, &moved, &target);
    assert_pixel(&pixels, 0, 4, [128, 0, 127, 255]);
    assert_pixel(&pixels, 0, 5, [192, 128, 63, 255]);
    assert_pixel(&pixels, 3, 5, [0, 0, 255, 255]);

    // Resize invalidates the static texture; the next idle frame does not.
    window
        .root()
        .handle_window_event(WindowEvent::Resize(PhysicalSize::new(8, 8)));
    assert!(matches!(
        refresh(&mut window, &mut renderer),
        Refresh::Painted {
            reallocated: true,
            ..
        }
    ));
    assert_eq!(window.surface().unwrap().size(), [8, 8]);
    assert!(matches!(
        refresh(&mut window, &mut renderer),
        Refresh::Unchanged
    ));
}

#[test]
#[ignore = "requires a working wgpu adapter; run explicitly on GPU hosts"]
fn fixed_readouts_share_one_dynamic_layer() {
    use masonry::widgets::{Grid, GridParams};
    use yarugc::{ReadoutError, ReadoutRenderer, ReadoutSlot};

    const EXTENT: [u32; 2] = [2048, 512];
    const COUNT: usize = 256;
    const COLUMNS: u16 = 16;
    const ROWS: u16 = 16;
    const FONT: &[u8] =
        include_bytes!("../../masonry_testing/resources/fonts/roboto/Roboto-Regular.ttf");

    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let compositor = Compositor::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    let mut renderer = Renderer::new();
    let mut grid = Grid::with_dimensions(usize::from(COLUMNS), usize::from(ROWS));
    let mut ids = Vec::with_capacity(COUNT);
    for row in 0..ROWS {
        for col in 0..COLUMNS {
            let widget = NewWidget::new(SizedBox::empty())
                .with_props(Dimensions::STRETCH)
                .with_props(Background::Color(if (row + col) % 2 == 0 {
                    Color::from_rgb8(16, 24, 32)
                } else {
                    Color::from_rgb8(24, 32, 40)
                }));
            ids.push(widget.id());
            grid = grid.with(widget, GridParams::pos(col, row));
        }
    }
    let mut window = CachedWindow::new(
        NewWidget::new(grid).with_props(Dimensions::STRETCH),
        RenderRootOptions {
            default_properties: Arc::new(DefaultProperties::new()),
            use_system_fonts: false,
            size_policy: WindowSizePolicy::User,
            size: PhysicalSize::new(EXTENT[0], EXTENT[1]),
            scale_factor: 1.0,
            test_font: None,
        },
        |_| {},
    );
    let refresh = |w: &mut CachedWindow, r: &mut Renderer| {
        w.refresh(&compositor, r, &adapter, &device, &queue)
            .unwrap()
    };
    assert!(matches!(
        refresh(&mut window, &mut renderer),
        Refresh::Painted { .. }
    ));
    let slots: Vec<_> = ids
        .iter()
        .map(|&id| ReadoutSlot {
            geometry: window.slot(id).expect("each grid cell has visible layout"),
            cells: 10,
            color: [1.0, 1.0, 1.0, 1.0],
        })
        .collect();

    let glyphs = ReadoutRenderer::new(
        &device,
        &queue,
        FONT,
        14.0,
        "0123456789.+-− kMµ°m/s".chars(),
    )
    .unwrap();
    let mut batch = glyphs.prepare(&compositor, EXTENT, &slots).unwrap();
    assert_eq!(batch.cell_count(), COUNT * 10);
    for i in 0..COUNT {
        assert!(batch.set_text(i, "999 m").unwrap());
    }
    assert_eq!(
        batch.set_text(0, "not in atlas"),
        Err(ReadoutError::TextTooLong)
    );
    assert_eq!(
        batch.set_text(0, "X"),
        Err(ReadoutError::UnknownCharacter('X'))
    );
    assert!(
        !batch.set_text(0, "999 m").unwrap(),
        "errors cannot mutate previous text"
    );
    let draw_batch = |batch: &mut yarugc::ReadoutBatch| {
        let mut encoder = device.create_command_encoder(&Default::default());
        let rendered = batch.render(&mut encoder);
        if rendered {
            queue.submit(Some(encoder.finish()));
        }
        rendered
    };
    assert!(draw_batch(&mut batch));
    assert!(
        !draw_batch(&mut batch),
        "unchanged dynamic layer must not encode work"
    );

    let target = compositor
        .create_surface(EXTENT, AlphaMode::Premultiplied)
        .unwrap();
    let composition = compositor.prepare(
        EXTENT,
        &[
            Layer::at(window.surface().unwrap(), [0.0, 0.0]),
            Layer::at(batch.surface(), [0.0, 0.0]),
        ],
    );
    let before = read_composition(&device, &queue, &compositor, &composition, &target);

    for i in 0..COUNT {
        assert!(batch.set_text(i, "1.00 km").unwrap());
    }
    assert!(draw_batch(&mut batch));
    assert!(matches!(
        refresh(&mut window, &mut renderer),
        Refresh::Unchanged
    ));
    let after = read_composition(&device, &queue, &compositor, &composition, &target);
    let width = usize::try_from(EXTENT[0]).unwrap();
    let cell_width = width / usize::from(COLUMNS);
    let cell_height = usize::try_from(EXTENT[1]).unwrap() / usize::from(ROWS);
    for (i, (&id, slot)) in ids.iter().zip(&slots).enumerate() {
        assert_eq!(
            window.slot(id).unwrap(),
            slot.geometry,
            "slot {i} must retain layout"
        );
        let x = (i % usize::from(COLUMNS)) * cell_width;
        let y = (i / usize::from(COLUMNS)) * cell_height;
        let changed = (y..y + cell_height).any(|row| {
            let start = (row * width + x) * 4;
            let end = start + cell_width * 4;
            before[start..end] != after[start..end]
        });
        assert!(
            changed,
            "readout {i} must actually change its rendered pixels"
        );
    }

    // Exercise the CPU-only update path without measuring text formatting,
    // layout, GPU execution, or texture allocation. This is not a frame benchmark.
    for count in [8, 64, COUNT] {
        const ITERATIONS: u32 = 1000;
        let start = std::time::Instant::now();
        for frame in 0..ITERATIONS {
            let text = if frame % 2 == 0 { "999 m" } else { "1.00 km" };
            for slot in 0..count {
                std::hint::black_box(batch.set_text(slot, text).unwrap());
            }
        }
        eprintln!(
            "readout update: {count} slots, {:?} mean per iteration",
            start.elapsed() / ITERATIONS
        );
    }
    assert!(draw_batch(&mut batch));
    assert!(matches!(
        refresh(&mut window, &mut renderer),
        Refresh::Unchanged
    ));

    // Resizing resolves new geometry; telemetry itself never changes it.
    window
        .root()
        .handle_window_event(WindowEvent::Resize(PhysicalSize::new(1024, EXTENT[1])));
    assert!(matches!(
        refresh(&mut window, &mut renderer),
        Refresh::Painted {
            reallocated: true,
            ..
        }
    ));
    assert_ne!(window.slot(ids[0]).unwrap(), slots[0].geometry);
    assert!(matches!(
        refresh(&mut window, &mut renderer),
        Refresh::Unchanged
    ));
}
