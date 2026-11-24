// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;

use crate::core::UpdateCtx;

color_property!(
    /// The color of a widget's content, often text and text decorations.
    ///
    /// **IMPORTANT:** This property is defined for [`Label`] and [`TextArea`], *not*
    /// for widgets embedding them such as [`Button`], [`Checkbox`], [`TextInput`], [`Prose`], etc.
    ///
    /// This property is also defined for [`Spinner`] and [`StepInput`].
    ///
    /// [`Label`]: crate::widgets::Label
    /// [`TextArea`]: crate::widgets::TextArea
    /// [`Button`]: crate::widgets::Button
    /// [`Checkbox`]: crate::widgets::Checkbox
    /// [`TextInput`]: crate::widgets::TextInput
    /// [`Spinner`]: crate::widgets::Spinner
    /// [`StepInput`]: crate::widgets::StepInput
    /// [`Prose`]: crate::widgets::Prose
    ContentColor
);

impl ContentColor {
    /// Helper function to be called in [`Widget::property_changed`](crate::core::Widget::property_changed).
    pub fn prop_changed(ctx: &mut UpdateCtx<'_>, property_type: TypeId) {
        if property_type != TypeId::of::<Self>() {
            return;
        }
        ctx.request_paint_only();
    }
}
