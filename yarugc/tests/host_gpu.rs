// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

//! Real GPU integration with host-owned rendering and readback.

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
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("host readback"),
        size: u64::from(EXTENT[0] * EXTENT[1] * 4),
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
                bytes_per_row: Some(EXTENT[0] * 4),
                rows_per_image: Some(EXTENT[1]),
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
    let prepare = |w: &CachedWindow, x: f32| {
        compositor.prepare(
            EXTENT,
            &[
                Layer {
                    surface: w.surface().unwrap(),
                    origin: [x, 4.0],
                },
                Layer {
                    surface: &dynamic,
                    origin: [x + 1.0, 5.0],
                },
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
