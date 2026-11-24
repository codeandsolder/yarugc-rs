// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::layout::{LenDef, Length};

/// Specifies how a widget dimension's length is derived.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
#[must_use = "Use the widget dimension."]
pub enum Dim {
    /// Automatically determine a reasonable length with a strategy chosen by the container widget.
    ///
    /// This may result in a [`measure`] invocation, which can be slow.
    ///
    /// [`measure`]: crate::core::Widget::measure
    #[default]
    Auto,
    /// Specific fixed [`Length`].
    Fixed(Length),
    /// Multiple of context length.
    ///
    /// For example, `Ratio(0.5)` will result in 50% of the context length.
    ///
    /// Context length is usually the container widget's length excluding its borders and padding.
    /// Examples of exceptions include `Grid` which will provide the child's area length,
    /// i.e. the sum of cell lengths that the child occupies, and `Portal` which will provide
    /// its viewport length.
    ///
    /// If there is no context length, e.g. the container hasn't calculated its dynamic length yet,
    /// then `Ratio` will fall back to [`Auto`].
    ///
    /// The ratio value must be finite and non-negative.
    ///
    /// [`Auto`]: Self::Auto
    Ratio(f64),
    /// Mimics the context length.
    ///
    /// Essentially a shorthand for [`Ratio(1.)`].
    ///
    /// [`Ratio(1.)`]: Self::Ratio
    Stretch,
    /// Minimum preferred length.
    ///
    /// This will result in a [`measure`] invocation, which can be slow.
    ///
    /// [`measure`]: crate::core::Widget::measure
    MinContent,
    /// Maximum preferred length.
    ///
    /// This will result in a [`measure`] invocation, which can be slow.
    ///
    /// [`measure`]: crate::core::Widget::measure
    MaxContent,
    /// The widget should attempt to fit into the context length.
    ///
    /// If there is no context length, e.g. the container hasn't calculated its dynamic length yet,
    /// then `FitContent` will fall back to [`Auto`].
    ///
    /// This may result in a [`measure`] invocation, which can be slow.
    ///
    /// [`Auto`]: Self::Auto
    /// [`measure`]: crate::core::Widget::measure
    FitContent,
}

impl From<Length> for Dim {
    fn from(value: Length) -> Self {
        Self::Fixed(value)
    }
}

impl Dim {
    /// Resolves, if possible, into a [`LenDef`].
    ///
    /// An unavailable context length returns None for context-dependent dimensions.
    /// A ratio that produces a non-finite or negative length falls back to `MaxContent`.
    pub fn resolve(self, context_length: Option<Length>) -> Option<LenDef> {
        match self {
            Self::Fixed(length) => Some(LenDef::Fixed(length)),
            Self::Ratio(mul) => context_length
                .map(|cl| Length::try_px(cl.get() * mul).map_or(LenDef::MaxContent, LenDef::Fixed)),
            Self::Stretch => context_length.map(LenDef::Fixed),
            Self::MinContent => Some(LenDef::MinContent),
            Self::MaxContent => Some(LenDef::MaxContent),
            Self::FitContent => context_length.map(LenDef::FitContent),
            Self::Auto => None,
        }
    }
}
