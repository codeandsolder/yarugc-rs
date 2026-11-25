// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use core::fmt;

/// Errors that can occur while creating or using a Skia renderer.
#[derive(Debug)]
pub enum Error {
    /// Creating or using the underlying `imaging_skia` renderer failed.
    Backend(imaging_skia::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(err) => write!(f, "Skia render failed: {err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backend(err) => Some(err),
        }
    }
}

/// Stable backend name for diagnostics.
pub const BACKEND_NAME: &str = "imaging_skia";

/// Masonry alias for the selected Skia image renderer type.
pub type Renderer = imaging_skia::SkiaCpuRenderer;

/// Masonry alias for the selected Skia texture renderer type.
pub type TargetRenderer = imaging_skia::SkiaRenderer;

/// Masonry alias for the selected Skia texture target wrapper.
pub type TextureTarget = wgpu::Texture;

/// Create a reusable headless Skia renderer.
#[must_use = "Use the created headless renderer."]
pub fn new_headless_renderer() -> Renderer {
    imaging_skia::SkiaCpuRenderer::new()
}

/// Create a reusable GPU Skia target renderer bound to an existing WGPU adapter, device, and queue.
///
/// # Errors
///
/// Returns an error if the Skia GPU backend cannot be initialized.
pub fn new_target_renderer(
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
) -> Result<TargetRenderer, Error> {
    imaging_skia::SkiaRenderer::new(adapter, device, queue).map_err(Error::Backend)
}
