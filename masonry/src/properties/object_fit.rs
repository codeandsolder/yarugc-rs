// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::core::Property;
use crate::kurbo::{Affine, Axis, Rect, Size};
use crate::layout::{LenReq, Length};
use crate::util::Sanitize;

// These are based on https://developer.mozilla.org/en-US/docs/Web/CSS/object-fit
/// Strategies for inscribing a rectangle inside another rectangle.
///
/// Default value is [`Self::Contain`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectFit {
    /// The content is scaled to fully fit within the container.
    ///
    /// The content's aspect ratio is maintained.
    ///
    /// If the content's aspect ratio does not match the aspect ratio of its container,
    /// then the content will not cover the whole container and nothing will overflow.
    Contain,
    /// The content is scaled to fully fill the container.
    ///
    /// The content's aspect ratio is maintained.
    ///
    /// If the content's aspect ratio does not match the aspect ratio of its container,
    /// then the content will overflow the container.
    Cover,
    /// The content is scaled to fully fill the container's height.
    ///
    /// The content's aspect ratio is maintained.
    ///
    /// This may result in letterboxed or overflowing width.
    FitHeight,
    /// The content is scaled to fully fill the container's width.
    ///
    /// The content's aspect ratio is maintained.
    ///
    /// This may result in letterboxed or overflowing height.
    FitWidth,
    /// The content's size is not changed at all.
    None,
    /// The content is only scaled down.
    ///
    /// This behaves as a mix of [`None`] and [`Contain`], resulting in whichever variant
    /// gives the smaller size.
    ///
    /// [`None`]: ObjectFit::None
    /// [`Contain`]: ObjectFit::Contain
    ScaleDown,
    /// The content is stretched to fully fill the container.
    ///
    /// If the content's aspect ratio does not match the aspect ratio of its container,
    /// then the content will be stretched to fit exactly, changing its aspect ratio.
    Stretch,
}

impl Property for ObjectFit {
    fn static_default() -> &'static Self {
        &Self::Contain
    }
}

impl Default for ObjectFit {
    fn default() -> Self {
        *Self::static_default()
    }
}

impl ObjectFit {
    /// Calculates an [`Affine`] transform to fit `content` inside `container`.
    ///
    /// Both `content` and `container` can have arbitrary origins, which will be
    /// correctly translated in the returned `Affine`.
    ///
    /// See [`ObjectFit`] variant documentation for fitting details.
    ///
    /// # Panics
    ///
    /// Panics if either `content` or `container` has non-finite or negative size
    /// and debug assertions are enabled.
    #[must_use = "Use the returned value."]
    pub fn affine(self, container: Rect, content: Rect) -> Affine {
        // Guard against invalid input
        let container_width = container.width().sanitize("container width");
        let container_height = container.height().sanitize("container height");
        let content_width = content.width().sanitize("content width");
        let content_height = content.height().sanitize("content height");

        // Guard against division by zero
        if content_width == 0. || content_height == 0. {
            return Affine::IDENTITY;
        }

        let horizontal = container_width / content_width;
        let vertical = container_height / content_height;
        let uniform = |scale| (scale, scale);
        let (sx, sy) = match self {
            Self::Contain => uniform(horizontal.min(vertical)),
            Self::Cover => uniform(horizontal.max(vertical)),
            Self::FitHeight => uniform(vertical),
            Self::FitWidth => uniform(horizontal),
            Self::None => (1., 1.),
            Self::ScaleDown => uniform(horizontal.min(vertical).min(1.)),
            Self::Stretch => (horizontal, vertical),
        };
        // Map the content centre to the container centre, including nonzero origins.
        let center = container.center();
        let source = content.center();
        Affine::new([
            sx,
            0.,
            0.,
            sy,
            sx.mul_add(-source.x, center.x),
            sy.mul_add(-source.y, center.y),
        ])
    }

    /// Calculates the [`Length`] of `axis`.
    ///
    /// `preferred_size` is the natural size that is used for
    /// both aspect ratio and minimum preferred size.
    pub fn measure(
        self,
        axis: Axis,
        len_req: LenReq,
        cross_length: Option<Length>,
        preferred_size: Size,
    ) -> Length {
        let preferred_length = preferred_size.get_coord(axis);

        let (space, space_or_preferred) = match len_req {
            LenReq::Min => return Length::px(preferred_length),
            LenReq::Max => (f64::INFINITY, preferred_length),
            LenReq::Fit(space) => (space.get(), space.get()),
        };

        let mut ar = preferred_size.get_coord(axis) / preferred_size.get_coord(axis.cross());
        if !ar.is_finite() {
            ar = 1.;
        }

        let length = match self {
            // Use all available space or if cross is known attempt to maintain AR,
            // but don't exceed available space (will letterbox cross).
            Self::Contain => {
                cross_length.map_or(space_or_preferred, |cl| (cl.get() * ar).min(space))
            }
            // Always use all available space.
            Self::Cover | Self::Stretch => space_or_preferred,
            // Always use all available vertical space.
            // Greedily take all horizontal space unless cross is known.
            Self::FitHeight => match axis {
                Axis::Horizontal => {
                    cross_length.map_or(space_or_preferred, |cl| (cl.get() * ar).min(space))
                }
                Axis::Vertical => space_or_preferred,
            },
            // Always use all available horizontal space.
            // Greedily take all vertical space unless cross is known.
            Self::FitWidth => match axis {
                Axis::Horizontal => space_or_preferred,
                Axis::Vertical => {
                    cross_length.map_or(space_or_preferred, |cl| (cl.get() * ar).min(space))
                }
            },
            // None == preferred size
            Self::None => preferred_length,
            // ScaleDown == min(Contain, None)
            Self::ScaleDown => cross_length
                .map_or(space_or_preferred, |cl| (cl.get() * ar).min(space))
                .min(preferred_length),
        };

        Length::px(length)
    }
}
