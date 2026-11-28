// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

//! Host-owned GPU surfaces and retained window composition.
//!
//! No surface, adapter selection, event loop, or presentation is owned by this crate.
//! The host also chooses and enables a Masonry imaging backend.

mod compositor;
mod geometry;
#[cfg(not(target_arch = "wasm32"))]
mod readout;
#[cfg(not(target_arch = "wasm32"))]
mod window;

pub use compositor::{AlphaMode, Composition, CompositionError, Compositor, Layer, Surface};
pub use geometry::{Placement, SlotGeometry};
pub use masonry_core;
pub use masonry_imaging;
#[cfg(not(target_arch = "wasm32"))]
pub use readout::{ReadoutBatch, ReadoutError, ReadoutRenderer, ReadoutSlot, ReadoutStyle};
#[cfg(not(target_arch = "wasm32"))]
pub use window::{CachedWindow, Refresh, RefreshError};
