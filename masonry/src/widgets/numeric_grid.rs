// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Shared floating-point arithmetic for numeric controls.

/// Above this limit, grid indices cannot reliably retain fractional offsets.
pub(super) const EXACT_GRID_LIMIT: f64 = 4_503_599_627_370_496.0;

/// Add a scaled step, rounding the displacement separately for ordinary
/// decimal UI steps (such as -1 + 1000 * 0.001). If the displacement alone
/// overflows, use FMA to retain representable cancellation with the base.
pub(super) fn step_sum(base: f64, step: f64, index: f64) -> f64 {
    let displacement = index * step;
    if displacement.is_finite() {
        base + displacement
    } else {
        index.mul_add(step, base)
    }
}
