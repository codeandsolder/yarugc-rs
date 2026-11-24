// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::f64::consts::TAU;

use crate::kurbo::{Point, Rect};
use crate::layout::{Length, UnitPoint};
use crate::peniko::color::{ColorSpaceTag, HueDirection};
use crate::peniko::{ColorStops, ColorStopsSource, Extend};
use crate::peniko::{
    InterpolationAlphaSpace, LinearGradientPosition, RadialGradientPosition, SweepGradientPosition,
};

/// Properties for the supported [`Gradient`] types.
///
/// This mirrors [`peniko::GradientKind`],
/// but uses a layout-invariant representation: instead of saying
/// "The gradient goes from point A to point B", we declare things like
/// "The gradient has angle X", and A and B are computed dynamically from widget layout.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum GradientShape {
    /// Gradient that transitions between two or more colors along a line.
    ///
    /// This is interpreted like [`linear-gradient()`] in CSS.
    ///
    /// [`linear-gradient()`]: https://drafts.csswg.org/css-images-3/#linear-gradient-syntax
    Linear {
        /// The angle defining the gradient line's direction, in radians.
        /// Zero points upwards and positive angles represent clockwise rotation.
        angle: f64,
    },
    /// Gradient that transitions between two or more colors that radiate from an origin.
    ///
    /// This is interpreted like [`radial-gradient()`] in CSS.
    ///
    /// [`radial-gradient()`]: https://drafts.csswg.org/css-images-3/#radial-gradient-syntax
    Radial {
        /// The center of the gradient, relative to the widget's bounding box.
        center: UnitPoint,
        /// The shape and size of the gradient.
        shape: RadialGradientShape,
    },
    /// Gradient that transitions between two or more colors that rotate around a center
    /// point.
    ///
    /// This is similar to [`conic-gradient()`] in CSS **but the values are interpreted differently**.
    /// (Specifically, angle zero points to the right and positive angles rotate counter-clockwise.)
    ///
    /// [`conic-gradient()`]: https://drafts.csswg.org/css-images-4/#conic-gradient-syntax
    Sweep {
        /// The center of the gradient, relative to the widget's bounding box.
        center: UnitPoint,
        /// Start angle of the sweep, in radians.
        /// Zero points to the right and positive angles represent counter-clockwise rotation.
        start_angle: f64,
        /// End angle of the sweep, in radians.
        /// Zero points to the right and positive angles represent counter-clockwise rotation.
        end_angle: f64,
    },
}

/// Definition of a gradient that transitions between two or more colors.
///
/// This mirrors [`peniko::Gradient`],
/// but uses a layout-invariant representation: instead of saying
/// "The gradient goes from point A to point B", we declare things like
/// "The gradient has angle X", and A and B and computed dynamically from widget layout.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    /// Shape and coordinates of the gradient.
    pub shape: GradientShape,
    /// Extend mode.
    pub extend: Extend,
    /// The color space to be used for interpolation.
    ///
    /// The colors in the color stops will be converted to this color space.
    ///
    /// This defaults to [sRGB](ColorSpaceTag::Srgb).
    pub interpolation_cs: ColorSpaceTag,
    /// When interpolating within a cylindrical color space, the direction for the hue.
    ///
    /// This is interpreted as described in [CSS Color Module Level 4 § 12.4].
    ///
    /// [CSS Color Module Level 4 § 12.4]: https://drafts.csswg.org/css-color/#hue-interpolation
    pub hue_direction: HueDirection,
    /// Color stop collection.
    pub stops: ColorStops,
}

/// An enum for different ways a radial gradient can be sized and shaped.
///
/// Matches the `radial-shape` and `radial-extent` parameters
/// of the [`radial-gradient()`] syntax.
///
/// Defaults to `CircleTo(FarthestCorner)`.
///
/// [`radial-gradient()`]: https://drafts.csswg.org/css-images-3/#radial-gradient-syntax
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RadialGradientShape {
    /// A circle defined based on the box size.
    CircleTo(RadialGradientExtent),
    /// A circle with a fixed radius.
    FixedCircle(Length),
    // TODO - Add following and remove #[non_exhaustive]:
    // EllipseTo(RadialGradientExtent),
    // FixedEllipse(f64),
    // EllipsePercentage(f64),
}

/// An enum for different ways a radial gradient can be sized based on the surrounding box.
///
/// Matches the `radial-extent` parameter of the [`radial-gradient()`] syntax.
///
/// [`radial-gradient()`]: https://developer.mozilla.org/en-US/docs/Web/CSS/gradient/radial-gradient#size
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RadialGradientExtent {
    /// Interpreted like CSS `closest-corner` size.
    ClosestCorner,
    /// Interpreted like CSS `closest-side` size.
    ClosestSide,
    /// Interpreted like CSS `farthest-side` size.
    FarthestSide,
    /// Interpreted like CSS `farthest-corner` size.
    FarthestCorner,
}

// ---

impl Gradient {
    /// Creates a gradient with the given shape.
    ///
    /// See also [`Self::new_linear`], [`Self::new_radial`], [`Self::new_radial_with`].
    pub fn new(shape: GradientShape) -> Self {
        Self {
            shape,
            extend: Extend::default(),
            interpolation_cs: ColorSpaceTag::Srgb,
            hue_direction: HueDirection::default(),
            stops: ColorStops::default(),
        }
    }

    /// Creates a [`Linear`](GradientShape::Linear) gradient.
    ///
    /// `angle` is in radians, with zero pointing upwards, and higher values rotating the gradient clockwise.
    /// This matches how [CSS gradients are defined](https://developer.mozilla.org/en-US/docs/Web/CSS/gradient/linear-gradient#angle).
    ///
    /// That is, for an `angle` of 0, the first stop will be at the bottom, and
    /// for an `angle` of [`π/2`](core::f32::consts::FRAC_PI_2) (90°), the first
    /// stop will be aligned with the left edge.
    pub fn new_linear(angle: f64) -> Self {
        let shape = GradientShape::Linear { angle };
        Self::new(shape)
    }

    /// Creates a circular [`Radial`](GradientShape::Radial) gradient extending to the farthest corner.
    ///
    /// See also [`Self::new_radial_with`].
    pub fn new_radial(center: UnitPoint) -> Self {
        let shape = RadialGradientShape::CircleTo(RadialGradientExtent::FarthestCorner);
        let shape = GradientShape::Radial { center, shape };
        Self::new(shape)
    }

    /// Creates a [`Radial`](GradientShape::Radial) gradient.
    pub fn new_radial_with(center: UnitPoint, shape: RadialGradientShape) -> Self {
        let shape = GradientShape::Radial { center, shape };
        Self::new(shape)
    }

    /// Creates a [`Sweep`](GradientShape::Sweep) gradient.
    ///
    /// `start_angle` and `end_angle` are in radians, with zero pointing to the right,
    /// and higher values rotating the gradient counter-clockwise.
    ///
    /// **Warning:** This doesn't match the values CSS uses for
    /// [conic gradients](https://developer.mozilla.org/en-US/docs/Web/CSS/gradient/conic-gradient#composition_of_a_conic_gradient).
    /// We may change how these values are interpreted in the future.
    ///
    /// See also [`Self::new_full_sweep`].
    pub fn new_sweep(center: UnitPoint, start_angle: f64, end_angle: f64) -> Self {
        let shape = GradientShape::Sweep {
            center,
            start_angle,
            end_angle,
        };
        Self::new(shape)
    }

    /// Creates a [`Sweep`](GradientShape::Sweep) gradient doing a full circle.
    ///
    /// `angle` is in radians, with zero pointing to the right,
    /// and higher values rotating the gradient counter-clockwise.
    ///
    /// **Warning:** This doesn't match the values CSS uses for
    /// [conic gradients](https://developer.mozilla.org/en-US/docs/Web/CSS/gradient/conic-gradient#composition_of_a_conic_gradient).
    /// We may change how these values are interpreted in the future.
    pub fn new_full_sweep(center: UnitPoint, angle: f64) -> Self {
        let start_angle = angle;
        let end_angle = angle + TAU;
        let shape = GradientShape::Sweep {
            center,
            start_angle,
            end_angle,
        };
        Self::new(shape)
    }

    /// Builder method to set color stops on the gradient.
    pub fn with_stops(mut self, stops: impl ColorStopsSource) -> Self {
        self.stops.clear();
        stops.collect_stops(&mut self.stops);
        self
    }

    /// Returns gradient brush covering the given Rect.
    ///
    /// This matches the CSS spec for [`linear-gradient()`](https://drafts.csswg.org/css-images-3/#linear-gradient-syntax).
    pub fn get_peniko_gradient_for_rect(&self, rect: Rect) -> peniko::Gradient {
        peniko::Gradient {
            kind: self.shape.get_peniko_kind_for_rect(rect),
            extend: self.extend,
            interpolation_cs: self.interpolation_cs,
            hue_direction: self.hue_direction,
            stops: self.stops.clone(),
            interpolation_alpha_space: InterpolationAlphaSpace::default(),
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "no other way to go from f64 to f32"
)]
impl GradientShape {
    /// Returns gradient coordinates for a gradient covering the given Rect.
    ///
    /// This matches the CSS spec for [`linear-gradient()`](https://drafts.csswg.org/css-images-3/#linear-gradient-syntax).
    pub fn get_peniko_kind_for_rect(&self, rect: Rect) -> peniko::GradientKind {
        match self {
            Self::Linear { angle } => Self::get_peniko_linear_for_rect(*angle, rect),
            Self::Radial { center, shape } => {
                Self::get_peniko_radial_for_rect(*center, *shape, rect)
            }
            Self::Sweep {
                center,
                start_angle,
                end_angle,
            } => Self::get_peniko_sweep_for_rect(*center, *start_angle, *end_angle, rect),
        }
    }

    fn get_peniko_linear_for_rect(angle: f64, rect: Rect) -> peniko::GradientKind {
        // The CSS spec gives this formula for the gradient line length:
        // `abs(W * sin(A)) + abs(H * cos(A))`
        // https://drafts.csswg.org/css-images-3/#linear-gradient-syntax

        let size = rect.size();
        let sin_a = angle.sin();
        let cos_a = angle.cos();
        let gradient_line_length = (size.width * sin_a).abs() + (size.height * cos_a).abs();

        let center = rect.center();
        let x = sin_a * gradient_line_length / 2.0;
        let y = cos_a * gradient_line_length / 2.0;

        let start = Point::new(center.x - x, center.y - y);
        let end = Point::new(center.x + x, center.y + y);

        LinearGradientPosition { start, end }.into()
    }

    fn get_peniko_radial_for_rect(
        center: UnitPoint,
        shape: RadialGradientShape,
        rect: Rect,
    ) -> peniko::GradientKind {
        let center = center.resolve(rect);
        let radius = Self::get_gradient_radius(rect, center, shape);

        RadialGradientPosition {
            start_center: center,
            start_radius: 0.,
            end_center: center,
            end_radius: radius.max(0.0).min(f64::from(f32::MAX)) as f32,
        }
        .into()
    }

    fn get_gradient_radius(rect: Rect, center: Point, shape: RadialGradientShape) -> f64 {
        let extent = match shape {
            RadialGradientShape::CircleTo(extent) => extent,
            RadialGradientShape::FixedCircle(radius) => return radius.get(),
        };

        // Corners are the Cartesian product of the two x and y edge
        // coordinates. The nearest/farthest corner therefore uses the
        // nearest/farthest edge independently on each axis.
        let left = (center.x - rect.x0).abs();
        let right = (center.x - rect.x1).abs();
        let top = (center.y - rect.y0).abs();
        let bottom = (center.y - rect.y1).abs();
        let near_x = left.min(right);
        let near_y = top.min(bottom);
        let far_x = left.max(right);
        let far_y = top.max(bottom);

        match extent {
            RadialGradientExtent::ClosestCorner => near_x.hypot(near_y),
            RadialGradientExtent::ClosestSide => near_x.min(near_y),
            RadialGradientExtent::FarthestSide => far_x.max(far_y),
            RadialGradientExtent::FarthestCorner => far_x.hypot(far_y),
        }
    }

    fn get_peniko_sweep_for_rect(
        center: UnitPoint,
        start_angle: f64,
        end_angle: f64,
        rect: Rect,
    ) -> peniko::GradientKind {
        let center = center.resolve(rect);

        // Keep the renderer's native angle convention; Vello currently culls
        // sweeps outside the 0..2*PI range.

        SweepGradientPosition {
            center,
            start_angle: start_angle as f32,
            end_angle: end_angle as f32,
        }
        .into()
    }
}

impl Default for RadialGradientShape {
    fn default() -> Self {
        Self::CircleTo(RadialGradientExtent::FarthestCorner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radial_extents_match_brute_force_for_arbitrary_centers() {
        let rects = [
            Rect::new(10.0, 20.0, 110.0, 80.0),
            Rect::new(110.0, 80.0, 10.0, 20.0),
            Rect::new(0.0, 0.0, 0.0, 0.0),
        ];
        for rect in rects {
            for x in [-100.0, 10.0, 50.0, 110.0, 200.0] {
                for y in [-100.0, 20.0, 50.0, 80.0, 200.0] {
                    let center = Point::new(x, y);
                    let corners = [
                        Point::new(rect.x0, rect.y0),
                        Point::new(rect.x1, rect.y0),
                        Point::new(rect.x0, rect.y1),
                        Point::new(rect.x1, rect.y1),
                    ]
                    .map(|corner| center.distance(corner));
                    let sides = [
                        (x - rect.x0).abs(),
                        (x - rect.x1).abs(),
                        (y - rect.y0).abs(),
                        (y - rect.y1).abs(),
                    ];
                    let expected = [
                        (
                            RadialGradientExtent::ClosestCorner,
                            corners.into_iter().fold(f64::INFINITY, f64::min),
                        ),
                        (
                            RadialGradientExtent::FarthestCorner,
                            corners.into_iter().fold(0.0, f64::max),
                        ),
                        (
                            RadialGradientExtent::ClosestSide,
                            sides.into_iter().fold(f64::INFINITY, f64::min),
                        ),
                        (
                            RadialGradientExtent::FarthestSide,
                            sides.into_iter().fold(0.0, f64::max),
                        ),
                    ];
                    for (extent, radius) in expected {
                        let actual = GradientShape::get_gradient_radius(
                            rect,
                            center,
                            RadialGradientShape::CircleTo(extent),
                        );
                        assert!(
                            (actual - radius).abs() <= f64::EPSILON * radius.max(1.0),
                            "{rect:?}, {center:?}, {extent:?}: {actual} != {radius}"
                        );
                    }
                }
            }
        }
    }
}
