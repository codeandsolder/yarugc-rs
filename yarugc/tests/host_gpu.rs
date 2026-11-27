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
use yarugc::{
    AlphaMode, CachedWindow, Composition, CompositionError, Compositor, Layer, Refresh, Surface,
};

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
            compositor.draw(&mut pass, composition).unwrap();
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
        compositor
            .prepare(
                EXTENT,
                &[
                    Layer::at(w.surface().unwrap(), [x, 4.0]),
                    Layer::at(&dynamic, [x + 1.0, 5.0]),
                ],
            )
            .unwrap()
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

    // Failed visual refreshes must not consume the incremental accessibility
    // update: retry after removing an unsupported external layer.
    let base_id = window.root().get_layer_root(0).unwrap().id();
    let external = NewWidget::new(
        masonry::testing::ModularWidget::new(())
            .measure_fn(|_, _, _, _, _, _| masonry::layout::AsUnit::px(2.0))
            .paint_fn(|_, ctx, _, _| {
                ctx.set_paint_layer_mode(masonry_core::core::PaintLayerMode::External)
            }),
    );
    let external_id = external.id();
    window.root().add_layer(
        external,
        masonry::core::LayerPlacement::Absolute(masonry::kurbo::Point::ZERO),
    );
    window
        .root()
        .handle_window_event(WindowEvent::EnableAccessTree);
    assert!(matches!(
        window.refresh(&compositor, &mut renderer, &adapter, &device, &queue),
        Err(yarugc::RefreshError::ExternalLayer)
    ));
    window.root().remove_layer(external_id);
    let Refresh::Painted {
        accessibility: Some(update),
        ..
    } = refresh(&mut window, &mut renderer)
    else {
        panic!("the successful retry must deliver accessibility");
    };
    assert!(
        update.nodes.iter().any(|(id, _)| *id == base_id),
        "retry must include the original base widget"
    );
    assert!(matches!(
        refresh(&mut window, &mut renderer),
        Refresh::Unchanged
    ));

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
    let timestamp_features =
        wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    // Software adapters exercise the same rendering assertions, but their
    // timestamp measurements are not representative GPU performance data.
    let timestamps_available = adapter.get_info().device_type != wgpu::DeviceType::Cpu
        && adapter.features().contains(timestamp_features);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: if timestamps_available {
            timestamp_features
        } else {
            wgpu::Features::empty()
        },
        ..Default::default()
    }))
    .unwrap();
    eprintln!("readout adapter: {:?}", adapter.get_info());
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
    assert!(batch.set_text(0, "−1.0 µm").unwrap());
    assert!(!batch.set_text(0, "−1.0 µm").unwrap());
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
    let mut discarded = device.create_command_encoder(&Default::default());
    assert!(batch.render(&mut discarded));
    drop(discarded);
    batch.invalidate_surface();
    assert!(draw_batch(&mut batch));
    assert!(
        !draw_batch(&mut batch),
        "unchanged dynamic layer must not encode work"
    );

    let target = compositor
        .create_surface(EXTENT, AlphaMode::Premultiplied)
        .unwrap();
    let composition = compositor
        .prepare(
            EXTENT,
            &[
                Layer::at(window.surface().unwrap(), [0.0, 0.0]),
                Layer::at(batch.surface(), [0.0, 0.0]),
            ],
        )
        .unwrap();
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
    for count in [1, 8, 64, COUNT] {
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

    if timestamps_available {
        measure_readout_gpu(
            &device,
            &queue,
            &compositor,
            &composition,
            &target,
            &mut batch,
            COUNT,
        );
    }

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

// Isolate GPU raster/composition using timestamps. Queue staging uploads happen
// before the first encoder timestamp, and formatting is outside these intervals.
fn measure_readout_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    compositor: &Compositor,
    composition: &Composition,
    target: &Surface,
    batch: &mut yarugc::ReadoutBatch,
    count: usize,
) {
    const SAMPLES: u32 = 64;
    const QUERY_COUNT: u32 = SAMPLES * 3;
    const BYTES: u64 = QUERY_COUNT as u64 * 8;
    let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("readout timings"),
        ty: wgpu::QueryType::Timestamp,
        count: QUERY_COUNT,
    });
    let resolve = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readout timestamp resolve"),
        size: BYTES,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readout timestamp readback"),
        size: BYTES,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for changed in [1, count] {
        for frame in 0..SAMPLES {
            let text = if frame % 2 == 0 { "999 m" } else { "1.00 km" };
            for i in 0..changed {
                assert!(
                    batch.set_text(i, text).unwrap(),
                    "slot {i} must change at frame {frame}"
                );
            }
            let mut encoder = device.create_command_encoder(&Default::default());
            let base = frame * 3;
            encoder.write_timestamp(&query_set, base);
            assert!(
                batch.render(&mut encoder),
                "changed readout must encode a raster pass"
            );
            encoder.write_timestamp(&query_set, base + 1);
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("readout host composition"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target.view(),
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                compositor.draw(&mut pass, composition).unwrap();
            }
            encoder.write_timestamp(&query_set, base + 2);
            queue.submit(Some(encoder.finish()));
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.resolve_query_set(&query_set, 0..QUERY_COUNT, &resolve, 0);
        encoder.copy_buffer_to_buffer(&resolve, 0, &read, 0, Some(BYTES));
        queue.submit(Some(encoder.finish()));
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        read.slice(..)
            .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        {
            let bytes = read.slice(..).get_mapped_range().unwrap();
            let stamps: &[u64] = bytemuck::cast_slice(&bytes);
            let period = f64::from(queue.get_timestamp_period());
            let mut raster: Vec<_> = stamps
                .chunks_exact(3)
                .map(|s| (s[1] - s[0]) as f64 * period / 1000.0)
                .collect();
            let mut compose: Vec<_> = stamps
                .chunks_exact(3)
                .map(|s| (s[2] - s[1]) as f64 * period / 1000.0)
                .collect();
            raster.sort_by(f64::total_cmp);
            compose.sort_by(f64::total_cmp);
            let mid = raster.len() / 2;
            let p95 = raster.len() * 95 / 100;
            eprintln!(
                "GPU ({changed} changed, {count} visible): raster median {:.1} µs p95 {:.1} µs; compose median {:.1} µs p95 {:.1} µs",
                raster[mid], raster[p95], compose[mid], compose[p95]
            );
        }
        read.unmap();
    }
}

#[test]
#[ignore = "requires a working wgpu adapter; run explicitly on GPU hosts"]
fn composition_rejects_foreign_device_before_encoding() {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let foreign_adapter =
        pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (foreign, _) =
        pollster::block_on(foreign_adapter.request_device(&Default::default())).unwrap();
    let compositor = Compositor::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    let other = Compositor::new(&foreign, wgpu::TextureFormat::Rgba8Unorm);
    let target = compositor
        .create_surface(EXTENT, AlphaMode::Premultiplied)
        .unwrap();
    let surface = other
        .create_surface([2, 2], AlphaMode::Premultiplied)
        .unwrap();
    let layer = Layer::at(&surface, [0.0, 0.0]);
    assert!(matches!(
        compositor.prepare(EXTENT, std::slice::from_ref(&layer)),
        Err(CompositionError::DifferentDevice)
    ));
    let filled = other.prepare(EXTENT, &[layer]).unwrap();
    let empty = other.prepare(EXTENT, &[]).unwrap();
    let local = compositor.prepare(EXTENT, &[]).unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.view(),
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLUE),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        for composition in [&filled, &empty] {
            assert_eq!(
                compositor.draw(&mut pass, composition),
                Err(CompositionError::DifferentDevice)
            );
        }
        compositor.draw(&mut pass, &local).unwrap();
    }
    queue.submit(Some(encoder.finish()));
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

#[test]
#[ignore = "requires a working wgpu adapter; run explicitly on GPU hosts"]
fn content_window_refresh_uses_post_rewrite_extent() {
    use masonry_core::core::WidgetMut;
    use masonry_core::layout::Length;

    fn defer_resize(mut widget: WidgetMut<'_, SizedBox>, remaining: u32, side: Length) {
        if remaining == 0 {
            SizedBox::set_size(&mut widget, side, side);
        } else {
            widget.ctx.mutate_self_later::<SizedBox>(move |widget| {
                defer_resize(widget, remaining - 1, side);
            });
        }
    }

    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let compositor = Compositor::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    let mut renderer = Renderer::new();
    let mut window = CachedWindow::new(
        NewWidget::new(SizedBox::empty().size(Length::ZERO, Length::ZERO)),
        RenderRootOptions {
            default_properties: Arc::new(DefaultProperties::new()),
            use_system_fonts: false,
            size_policy: WindowSizePolicy::Content,
            size: PhysicalSize::new(0, 0),
            scale_factor: 1.0,
            test_font: None,
        },
        |_| {},
    );
    let mut refresh = |w: &mut CachedWindow| {
        w.refresh(&compositor, &mut renderer, &adapter, &device, &queue)
            .unwrap()
    };
    assert!(matches!(refresh(&mut window), Refresh::Hidden));
    // Four mutate passes are run per edit; the fifth queued callback must
    // run during refresh, including when the old extent is zero.
    for side in [Length::px(4.0), Length::px(8.0), Length::ZERO] {
        window
            .root()
            .edit_base_layer(|widget| defer_resize(widget, 5, side))
            .unwrap();
        assert!(window.root().needs_rewrite_passes());
        let result = refresh(&mut window);
        if side == Length::ZERO {
            assert!(matches!(result, Refresh::Hidden));
            assert!(window.surface().is_none());
        } else {
            assert!(matches!(
                result,
                Refresh::Painted {
                    reallocated: true,
                    ..
                }
            ));
            let size = window.root().size();
            assert_eq!(window.surface().unwrap().size(), [size.width, size.height]);
            assert!(matches!(refresh(&mut window), Refresh::Unchanged));
        }
    }
}
