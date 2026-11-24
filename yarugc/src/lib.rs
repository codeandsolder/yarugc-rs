// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

//! Host-owned GPU surfaces and retained window composition.
//!
//! No surface, adapter selection, event loop, or presentation is owned by this crate.

mod compositor;
mod window;

pub use compositor::{AlphaMode, Composition, Compositor, Layer, Surface};
pub use masonry_core;
pub use masonry_imaging;
pub use window::{CachedWindow, Refresh, RefreshError};
