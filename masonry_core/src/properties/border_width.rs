// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::core::{Property, UsesProperty, Widget};
use crate::kurbo::{Axis, Insets, Rect, RoundedRect};
use crate::layout::Length;
use crate::properties::CornerRadius;

// Every widget has a border width.
impl<W: Widget> UsesProperty<BorderWidth> for W {}

/// The width of a widget's border.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
#[must_use = "Use the border width in layout or painting."]
pub struct BorderWidth {
    /// Uniform width of all four border edges.
    pub width: Length,
}

// TODO - To match CSS, we should use a non-zero default width
// and a "border style" of "None".

impl Property for BorderWidth {
    fn static_default() -> &'static Self {
        static DEFAULT: BorderWidth = BorderWidth {
            width: Length::ZERO,
        };
        &DEFAULT
    }
}

impl BorderWidth {
    /// Creates new `BorderWidth` with given value.
    pub const fn all(width: Length) -> Self {
        Self { width }
    }

    /// Returns the total [`Length`] of this border on the given `axis`.
    ///
    /// For [`Axis::Horizontal`] it will return the sum of the left and right border width.
    /// For [`Axis::Vertical`] it will return the sum of the top and bottom border height.
    pub const fn length(self, _axis: Axis) -> Length {
        self.width.saturating_add(self.width)
    }

    /// Returns the uniform border insets.
    #[must_use = "Use the border insets in box geometry."]
    pub const fn insets(self) -> Insets {
        Insets::uniform(self.width.get())
    }

    /// Creates a rounded rectangle that is inset by the border width.
    ///
    /// Use to display a box's background.
    ///
    /// Helper function to be called in [`Widget::paint`].
    #[must_use = "Use the inset background rectangle."]
    pub fn bg_rect(self, border_box: Rect, border_radius: CornerRadius) -> RoundedRect {
        let border_width = self.width.get();
        border_box
            .inset(-border_width)
            .to_rounded_rect(border_radius.radius.saturating_sub(self.width).get())
    }

    /// Creates a rounded rectangle that is inset by half the border width.
    ///
    /// Use to display a box's border.
    ///
    /// Helper function to be called in [`Widget::paint`].
    #[must_use = "Use the border stroke rectangle."]
    pub fn border_rect(self, border_box: Rect, border_radius: CornerRadius) -> RoundedRect {
        let border_width = self.width.get();
        border_box
            .inset(-border_width / 2.0)
            .to_rounded_rect(border_radius.radius.get())
    }
}
