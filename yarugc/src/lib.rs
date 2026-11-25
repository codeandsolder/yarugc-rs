// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

//! Host-owned GPU surfaces and retained window composition.
//!
//! No surface, adapter selection, event loop, or presentation is owned by this crate.

mod compositor;
mod geometry;
mod readout;
#[cfg(not(target_arch = "wasm32"))]
mod window;

pub use compositor::{AlphaMode, Composition, CompositionError, Compositor, Layer, Surface};
pub use geometry::{Placement, SlotGeometry};
pub use masonry_core;
pub use masonry_imaging;
pub use readout::{ReadoutBatch, ReadoutError, ReadoutRenderer, ReadoutSlot};
#[cfg(not(target_arch = "wasm32"))]
pub use window::{CachedWindow, Refresh, RefreshError};
