// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::layout::Length;

/// Utility trait for wrapping numbers in logical units.
pub trait AsUnit {
    /// Creates a length, in logical pixels.
    ///
    /// # Panics
    ///
    /// Panics if value is negative, infinite.
    #[track_caller]
    fn px(self) -> Length;
}

macro_rules! impl_lossless_as_unit {
    ($($ty:ty),* $(,)?) => {
        $(
            impl AsUnit for $ty {
                #[track_caller]
                fn px(self) -> Length {
                    Length::px(f64::from(self))
                }
            }
        )*
    };
}

impl AsUnit for f64 {
    #[track_caller]
    fn px(self) -> Length {
        Length::px(self)
    }
}

impl_lossless_as_unit!(f32, u32, u16, u8, i32, i16, i8);

impl AsUnit for u64 {
    #[track_caller]
    #[expect(
        clippy::cast_precision_loss,
        reason = "Logical pixels use f64; integers beyond its exact range are rounded."
    )]
    fn px(self) -> Length {
        Length::px(self as f64)
    }
}

impl AsUnit for i64 {
    #[track_caller]
    #[expect(
        clippy::cast_precision_loss,
        reason = "Logical pixels use f64; integers beyond its exact range are rounded."
    )]
    fn px(self) -> Length {
        Length::px(self as f64)
    }
}
