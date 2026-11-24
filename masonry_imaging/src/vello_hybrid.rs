// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::headless_wgpu;

/// Stable backend name for diagnostics.
pub const BACKEND_NAME: &str = "imaging_vello_hybrid";

/// Masonry alias for the selected Vello Hybrid renderer type.
pub type Renderer = imaging_vello_hybrid::VelloHybridRenderer;

/// Masonry alias for the selected Vello Hybrid texture renderer type.
pub type TargetRenderer = imaging_vello_hybrid::VelloHybridRenderer;

/// Masonry alias for the selected Vello Hybrid texture target wrapper.
pub type TextureTarget = imaging_wgpu::TextureViewTarget;

/// Create a reusable headless Vello Hybrid renderer.
///
/// # Errors
///
/// Returns an error if the GPU or rendering backend cannot be initialized.
pub fn new_headless_renderer() -> Result<Renderer, crate::HeadlessInitError> {
    let (device, queue) = headless_wgpu::try_init_device_and_queue()?;
    Ok(imaging_vello_hybrid::VelloHybridRenderer::new(
        device, queue,
    ))
}

/// Create a reusable Vello Hybrid target renderer bound to an existing WGPU device and queue.
#[must_use = "Use the created texture renderer."]
pub fn new_target_renderer(device: wgpu::Device, queue: wgpu::Queue) -> TargetRenderer {
    imaging_vello_hybrid::VelloHybridRenderer::new(device, queue)
}
