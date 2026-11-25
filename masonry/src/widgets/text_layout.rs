// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::kurbo::Axis;
use crate::layout::{LenReq, Length};

/// Resolves the inline width for horizontal text measurement.
/// Horizontal measurement ignores the cross constraint, whereas vertical
/// measurement uses it when available. Without wrapping the width is unbounded.
pub(super) fn measure_max_advance(
    word_wrap: bool,
    axis: Axis,
    len_req: LenReq,
    cross_length: Option<Length>,
) -> Option<f32> {
    if !word_wrap {
        return None;
    }
    let width = if axis == Axis::Horizontal {
        match len_req {
            LenReq::Min => Some(Length::ZERO),
            LenReq::Max => None,
            LenReq::Fit(space) => Some(space),
        }
    } else {
        match len_req {
            LenReq::Min => cross_length.or(Some(Length::ZERO)),
            LenReq::Max | LenReq::Fit(_) => cross_length,
        }
    };
    width.map(|value| value.get() as f32)
}
