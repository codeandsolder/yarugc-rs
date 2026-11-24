// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

/// A value representing a width, height, or similar distance value.
///
/// It is always finite and non-negative.
#[derive(Default, Clone, Copy, PartialEq)]
#[must_use = "Use the computed logical length."]
pub struct Length {
    value: f64,
}

impl std::fmt::Debug for Length {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

impl std::fmt::Display for Length {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}px", self.value)
    }
}

impl Length {
    /// A length of zero.
    pub const ZERO: Self = Self { value: 0. };

    /// Creates a length, in logical pixels.
    ///
    /// # Panics
    ///
    /// If debug assertions are on, this will panic in these cases:
    ///
    /// - `value` is NaN.
    /// - `value` is infinite.
    /// - `value` is negative.
    ///
    /// If debug assertions are off, this will return zero instead of panicking.
    #[track_caller]
    pub fn px(value: f64) -> Self {
        match Self::try_px(value) {
            Some(length) => length,
            None => {
                debug_panic!("Invalid length value '{value}'");
                Self::ZERO
            }
        }
    }

    /// Creates a length, in logical pixels.
    ///
    /// Returns `None` if the provided `value` is non-finite or negative.
    pub const fn try_px(value: f64) -> Option<Self> {
        if value < 0. || !value.is_finite() {
            return None;
        }
        Some(Self { value })
    }

    /// Returns the value, in logical pixels.
    ///
    /// The value is guaranteed to be finite and non-negative.
    #[must_use = "Use the logical pixel count."]
    pub const fn get(self) -> f64 {
        self.value
    }

    /// Scales the value to device pixels.
    #[must_use = "Use the scaled device pixel count."]
    pub const fn dp(self, scale: f64) -> f64 {
        self.value * scale
    }

    /// Returns the minimum of the two lengths.
    pub const fn min(self, other: Self) -> Self {
        if self.value < other.value {
            self
        } else {
            other
        }
    }

    /// Returns the maximum of the two lengths.
    pub const fn max(self, other: Self) -> Self {
        if self.value > other.value {
            self
        } else {
            other
        }
    }

    /// Clamps this length to the inclusive interval, returning None if the
    /// minimum is greater than the maximum.
    pub const fn try_clamp(self, min: Self, max: Self) -> Option<Self> {
        if min.value > max.value {
            return None;
        }
        Some(self.max(min).min(max))
    }

    /// Adds the `other` value but the result doesn't go above the maximum value.
    pub const fn saturating_add(self, other: Self) -> Self {
        Self {
            value: (self.value + other.value).min(f64::MAX),
        }
    }

    /// Subtracts the `other` value but the result doesn't go below zero.
    pub const fn saturating_sub(self, other: Self) -> Self {
        Self {
            value: (self.value - other.value).max(0.),
        }
    }
}

/// Creates a finite, non-negative logical length from a constant expression.
///
/// Invalid constants are rejected during compilation. Use [`Length::try_px`] for
/// fallible runtime conversion.
///
/// ```
/// use masonry_core::layout::{Length, length};
/// const GAP: Length = length!(8.0);
/// assert_eq!(GAP.get(), 8.0);
/// ```
///
/// ```compile_fail
/// use masonry_core::layout::length;
/// let _ = length!(-1.0);
/// ```
#[macro_export]
macro_rules! length {
    ($value:expr $(,)?) => {
        const {
            match $crate::layout::Length::try_px($value) {
                Some(length) => length,
                None => panic!("Invalid constant logical length"),
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::Length;

    #[test]
    fn length_invariants_hold_at_numeric_boundaries() {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            assert!(Length::try_px(invalid).is_none());
        }
        let largest = crate::layout::length!(f64::MAX);
        let smallest = crate::layout::length!(f64::MIN_POSITIVE);
        assert_eq!(largest.saturating_add(largest), largest);
        assert_eq!(smallest.saturating_sub(largest), Length::ZERO);
        assert_eq!(largest.try_clamp(Length::ZERO, smallest), Some(smallest));
        assert_eq!(smallest.try_clamp(largest, Length::ZERO), None);
        assert_eq!(smallest.try_clamp(smallest, smallest), Some(smallest));
    }
}
