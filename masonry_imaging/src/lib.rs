// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/linebender/xilem/main/docs/assets/masonry-logo.svg"
)]
//! Imaging helpers owned by Masonry.
//!
//! `masonry_imaging` owns the bridge between Masonry paint output and concrete imaging backends.
//! In this first slice that means:
//!
//! - preparing flattened Masonry frame content from base content plus overlays
//! - exposing backend-specific renderer modules for `imaging_vello`,
//!   `imaging_vello_hybrid`, `imaging_vello_cpu`, and `imaging_skia`
//! - exposing host-neutral texture rendering helpers for writing into caller-provided WGPU
//!   targets
//!
//! This crate does not own window integration, surfaces, or compositor policy.
//!
//! # Feature flags
//!
//! No rendering backend is enabled by default. Hosts opt into the implementations they use.
//!
//! - `imaging_vello`: Enables the `vello` module and texture rendering support.
//! - `imaging_vello_hybrid`: Enables the `vello_hybrid` module and texture rendering support.
//! - `imaging_vello_cpu`: Enables the `vello_cpu` module for headless image rendering only.
//! - `imaging_skia`: Enables the `skia` module and texture rendering support on non-wasm targets.

// LINEBENDER LINT SET - lib.rs - v3
// See https://linebender.org/wiki/canonical-lints/
#![cfg_attr(not(test), deny(unused_crate_dependencies))]
#![deny(clippy::print_stdout, clippy::print_stderr)]
#![cfg_attr(target_pointer_width = "64", deny(clippy::trivially_copy_pass_by_ref))]
// END LINEBENDER LINT SET
#![cfg_attr(docsrs, feature(doc_cfg))]

use imaging::record::{Scene, ValidateError, replay_transformed};
use imaging::render::RenderSource;
use imaging::{PaintSink, Painter};
use kurbo::{Affine, Rect};
use peniko::Color;

#[cfg(all(
    not(target_arch = "wasm32"),
    any(feature = "imaging_vello", feature = "imaging_vello_hybrid")
))]
#[expect(
    clippy::redundant_pub_crate,
    reason = "Keep device initialization internal under the workspace unreachable_pub lint."
)]
mod headless_wgpu;

#[cfg(all(
    not(target_arch = "wasm32"),
    any(feature = "imaging_vello", feature = "imaging_vello_hybrid")
))]
pub use headless_wgpu::InitError as HeadlessInitError;

/// Masonry helpers for rendering retained scenes with `imaging_skia`.
#[cfg(all(feature = "imaging_skia", not(target_arch = "wasm32")))]
pub mod skia;
/// Host-neutral texture rendering helpers for texture-capable backends.
#[cfg(not(target_arch = "wasm32"))]
pub mod texture_render;
/// Masonry helpers for rendering retained scenes with `imaging_vello`.
#[cfg(all(not(target_arch = "wasm32"), feature = "imaging_vello"))]
pub mod vello;
/// Masonry helpers for rendering retained scenes with `imaging_vello_cpu`.
#[cfg(feature = "imaging_vello_cpu")]
pub mod vello_cpu;
/// Masonry helpers for rendering retained scenes with `imaging_vello_hybrid`.
#[cfg(all(not(target_arch = "wasm32"), feature = "imaging_vello_hybrid"))]
pub mod vello_hybrid;

pub use imaging::render::ImageRenderer;
#[cfg(not(target_arch = "wasm32"))]
pub use imaging_wgpu::TextureRenderer;

/// Alpha representation of a GPU texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaMode {
    /// RGB already multiplied by alpha.
    Premultiplied,
    /// RGB independent of alpha.
    Straight,
}

/// A Masonry overlay layer ready to be composited into window space.
#[derive(Clone, Copy)]
pub struct Layer<'a> {
    /// The retained scene for this layer in layer-local coordinates.
    pub scene: &'a Scene,
    /// Transform from layer-local coordinates into window coordinates.
    pub transform: Affine,
}

impl core::fmt::Debug for Layer<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Layer")
            .field("scene", &"(Scene)")
            .field("transform", &self.transform)
            .finish()
    }
}

/// A flattened Masonry frame ready to be adapted to a concrete render target.
///
/// This is intentionally a single-target convenience type for Masonry's current rendering paths.
/// Future compositor-oriented work is expected to preserve more layer structure above this level.
#[derive(Clone, Copy, Debug)]
pub struct PreparedFrame<'a> {
    /// Frame width in physical pixels.
    pub width: u32,
    /// Frame height in physical pixels.
    pub height: u32,
    /// Window scale factor.
    pub scale_factor: f64,
    /// Background color to paint before replaying scene content.
    pub background_color: Color,
    /// Base retained scene in root coordinates.
    pub base: &'a Scene,
    /// Overlay layers in painter order.
    pub overlays: &'a [Layer<'a>],
}

impl<'a> PreparedFrame<'a> {
    /// Create a flattened Masonry frame from base content plus overlays.
    #[must_use = "Use the prepared render frame."]
    pub const fn new(
        width: u32,
        height: u32,
        scale_factor: f64,
        background_color: Color,
        base: &'a Scene,
        overlays: &'a [Layer<'a>],
    ) -> Self {
        Self {
            width,
            height,
            scale_factor,
            background_color,
            base,
            overlays,
        }
    }
}

impl RenderSource for PreparedFrame<'_> {
    fn validate(&self) -> Result<(), ValidateError> {
        validate_layers(self.base, self.overlays)
    }

    fn paint_into(&mut self, sink: &mut dyn PaintSink) {
        {
            let mut painter = Painter::new(sink);
            painter.fill_rect(
                Rect::new(0.0, 0.0, f64::from(self.width), f64::from(self.height)),
                self.background_color,
            );
        }

        let scale = Affine::scale(self.scale_factor);
        replay_transformed(self.base, sink, scale);
        for layer in self.overlays {
            replay_transformed(layer.scene, sink, scale * layer.transform);
        }
    }
}

fn validate_layers(base: &Scene, overlays: &[Layer<'_>]) -> Result<(), ValidateError> {
    base.validate()?;
    for layer in overlays {
        layer.scene.validate()?;
    }
    Ok(())
}
