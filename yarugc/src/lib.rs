// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

//! Host-owned GPU surfaces and retained window composition.
//!
//! No surface, adapter selection, event loop, or presentation is owned by this crate.
//! The host chooses and enables a Masonry imaging backend. The optional
//! `readouts` feature selects the shared Hybrid glyph atlas for fixed-cell layers.

mod compositor;
mod geometry;
#[cfg(all(not(target_arch = "wasm32"), feature = "readouts"))]
mod readout;
#[cfg(not(target_arch = "wasm32"))]
mod window;

pub use compositor::{Composition, CompositionError, Compositor, Layer, Surface};
pub use geometry::{Placement, SlotGeometry};
pub use masonry_core;
pub use masonry_imaging;
pub use masonry_imaging::AlphaMode;
#[cfg(all(not(target_arch = "wasm32"), feature = "readouts"))]
pub use readout::{
    ReadoutBatch, ReadoutError, ReadoutRasterStats, ReadoutRenderer, ReadoutSlot, ReadoutStyle,
};
#[cfg(not(target_arch = "wasm32"))]
pub use window::{CachedWindow, Refresh, RefreshError};
