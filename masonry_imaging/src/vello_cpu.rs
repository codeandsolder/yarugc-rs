// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

/// Stable backend name for diagnostics.
pub const BACKEND_NAME: &str = "imaging_vello_cpu";

/// Masonry alias for the selected Vello CPU renderer type.
pub type Renderer = imaging_vello_cpu::VelloCpuRenderer;

/// Create a reusable headless Vello CPU renderer.
#[must_use = "Use the created headless renderer."]
pub fn new_headless_renderer() -> Renderer {
    imaging_vello_cpu::VelloCpuRenderer::new(1, 1)
}
