// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::layout::Length;

/// Widget length measurement algorithm request.
///
/// It is up to the widget itself to define how it responds to these requests.
#[derive(Copy, Clone, Debug, PartialEq)]
#[must_use = "Use the computed measurement request."]
pub enum LenReq {
    /// The widget should measure its minimum preferred length.
    Min,
    /// The widget should measure its maximum preferred length.
    Max,
    /// The widget should attempt to fit into the specified available space.
    Fit(Length),
}

impl LenReq {
    /// Returns [`LenReq`] with `delta` subtracted from it.
    ///
    /// [`Fit`] will have its value reduced by `delta`, but clamped to zero.
    /// [`Min`] and [`Max`] are returned as-is.
    ///
    /// [`Fit`]: Self::Fit
    /// [`Min`]: Self::Min
    /// [`Max`]: Self::Max
    pub const fn reduce(self, delta: Length) -> Self {
        match self {
            Self::Min | Self::Max => self,
            Self::Fit(space) => Self::Fit(space.saturating_sub(delta)),
        }
    }
}
