// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Types and logic commonly used across widgets.
//!
//! See [properties documentation](crate::doc::widget_properties) for details.

// Each color has its own property identity while sharing its value representation.
macro_rules! color_property {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct $name {
            /// Resolved color.
            pub color: crate::peniko::color::AlphaColor<crate::peniko::color::Srgb>,
        }

        impl crate::core::Property for $name {
            fn static_default() -> &'static Self {
                static DEFAULT: $name = $name::new(crate::peniko::color::AlphaColor::BLACK);
                &DEFAULT
            }
        }

        impl $name {
            /// Creates this property with the supplied color.
            #[must_use = "Use the returned property."]
            pub const fn new(color: crate::peniko::color::AlphaColor<crate::peniko::color::Srgb>) -> Self {
                Self { color }
            }
        }

        impl Default for $name {
            fn default() -> Self {
                *<Self as crate::core::Property>::static_default()
            }
        }
    };
}

mod animation;
mod checkmark;
mod content_color;
mod gap;
mod line_breaking;
mod object_fit;
mod placeholder_color;
mod portal;
mod progress_bar;
mod scroll_bar;
mod selection;
mod slider;
mod step_input;
mod track;

pub mod types;

pub use animation::*;
pub use checkmark::*;
pub use content_color::*;
pub use gap::*;
pub use line_breaking::*;
pub use object_fit::*;
pub use placeholder_color::*;
pub use portal::*;
pub use progress_bar::*;
pub use scroll_bar::*;
pub use selection::*;
pub use slider::*;
pub use step_input::*;
pub use track::*;

pub use masonry_core::properties::*;
