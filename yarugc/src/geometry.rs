// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

use masonry_core::app::RenderRoot;
use masonry_core::core::WidgetId;
use masonry_core::kurbo::{Affine, Point, Rect, Vec2};

/// Affine placement of a texture's unit square in physical pixels, with a
/// convex clip polygon. This geometry can be retained independently of content.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    transform: Affine,
    clip: Vec<Point>,
}

impl Placement {
    /// Place a texture over a physical-pixel rectangle.
    #[must_use = "Use the placement."]
    pub fn rect(rect: Rect) -> Self {
        let mut placement = Self::from_transform(rect_transform(rect));
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            placement.clip.clear();
        }
        placement
    }

    /// Map normalized texture coordinates to physical pixels.
    #[must_use = "Use the placement."]
    pub fn from_transform(transform: Affine) -> Self {
        Self {
            transform,
            clip: corners(transform, Rect::new(0.0, 0.0, 1.0, 1.0)),
        }
    }

    /// Apply an additional rectangular clip expressed in target pixels.
    #[must_use = "Use the clipped placement."]
    pub fn clipped(mut self, rect: Rect) -> Self {
        self.clip = intersect(self.clip, &corners(Affine::IDENTITY, rect));
        self
    }

    /// Place a subrectangle expressed in the original normalized coordinates.
    /// The original clip is retained, even if the new rectangle extends outside it.
    #[must_use = "Use the subrectangle placement."]
    pub fn subrect(&self, rect: Rect) -> Self {
        let transform = self.transform * rect_transform(rect);
        let clip = intersect(
            corners(transform, Rect::new(0.0, 0.0, 1.0, 1.0)),
            &self.clip,
        );
        Self { transform, clip }
    }

    /// Move the placement and its clip by the same physical-pixel displacement.
    #[must_use = "Use the translated placement."]
    pub fn translated(&self, offset: Vec2) -> Self {
        Self {
            transform: self.transform.then_translate(offset),
            clip: self.clip.iter().map(|p| *p + offset).collect(),
        }
    }

    /// Transform from normalized coordinates to physical pixels.
    #[must_use = "Use the transform."]
    pub const fn transform(&self) -> Affine {
        self.transform
    }

    /// Convex visible polygon in physical pixels, in winding order.
    #[must_use = "Use the clip polygon."]
    pub fn clip_polygon(&self) -> &[Point] {
        &self.clip
    }

    /// Triangulate the visible area, retaining original texture coordinates.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "GPU vertex coordinates are f32"
    )]
    pub(crate) fn vertices(&self, extent: [u32; 2]) -> Vec<[f32; 4]> {
        let [width, height] = extent.map(f64::from);
        if width == 0.0
            || height == 0.0
            || !self.transform.is_finite()
            || self.transform.determinant() == 0.0
            || !self.transform.determinant().is_finite()
        {
            return Vec::new();
        }
        let inverse = self.transform.inverse();
        if !inverse.is_finite() {
            return Vec::new();
        }
        let polygon = intersect(
            self.clip.clone(),
            &corners(Affine::IDENTITY, Rect::new(0.0, 0.0, width, height)),
        );
        if polygon.len() < 3 {
            return Vec::new();
        }
        if polygon
            .iter()
            .any(|p| !p.is_finite() || !(inverse * *p).is_finite())
        {
            return Vec::new();
        }
        let vertex = |p: Point| {
            let uv = inverse * p;
            [
                (2.0 * p.x / width - 1.0) as f32,
                (1.0 - 2.0 * p.y / height) as f32,
                uv.x as f32,
                uv.y as f32,
            ]
        };
        let mut result = Vec::with_capacity((polygon.len() - 2) * 3);
        for pair in polygon[1..].windows(2) {
            result.extend([vertex(polygon[0]), vertex(pair[0]), vertex(pair[1])]);
        }
        result
    }
}

/// Content-box geometry for a widget-backed GPU instrument. Resolve after a
/// structural/layout update; numerical updates need not access the widget tree.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotGeometry {
    placement: Placement,
    logical_size: [f64; 2],
    scale_factor: f64,
}

impl SlotGeometry {
    /// Resolve the widget's content box and all ancestor clips, including scroll
    /// containers and transformed clips. Missing, hidden or empty slots return none.
    #[must_use = "Use the resolved slot if visible."]
    pub fn resolve(root: &RenderRoot, id: WidgetId) -> Option<Self> {
        let widget = root.get_widget(id)?;
        let ctx = widget.ctx();
        let rect = ctx.content_box();
        let scale_factor = root.scale_factor();
        if !scale_factor.is_finite() || scale_factor <= 0.0 {
            return None;
        }
        let scale = Affine::scale(scale_factor);
        let transform = scale * ctx.window_transform() * rect_transform(rect);
        let mut placement = Placement::from_transform(transform);
        let size = root.size();
        placement = placement.clipped(Rect::new(
            0.0,
            0.0,
            f64::from(size.width),
            f64::from(size.height),
        ));
        for ancestor in root.widget_path(id) {
            let widget = root.get_widget(ancestor)?;
            let ctx = widget.ctx();
            if ctx.is_stashed() {
                return None;
            }
            if let Some(clip) = ctx.clip_path() {
                placement.clip = intersect(
                    placement.clip,
                    &corners(scale * ctx.window_transform(), clip),
                );
            }
        }
        (placement.clip.len() >= 3 && rect.width() > 0.0 && rect.height() > 0.0).then_some(Self {
            placement,
            logical_size: [rect.width(), rect.height()],
            scale_factor,
        })
    }

    /// Place this slot into its window-local GPU surface.
    #[must_use = "Use the placement."]
    pub const fn placement(&self) -> &Placement {
        &self.placement
    }

    /// Content size in logical units, before any affine transforms.
    #[must_use = "Use the logical dimensions."]
    pub const fn logical_size(&self) -> [f64; 2] {
        self.logical_size
    }

    /// Window DPI scale.
    #[must_use = "Use the DPI scale."]
    pub const fn scale_factor(&self) -> f64 {
        self.scale_factor
    }
}

fn rect_transform(rect: Rect) -> Affine {
    Affine::translate((rect.x0, rect.y0)) * Affine::scale_non_uniform(rect.width(), rect.height())
}

fn corners(transform: Affine, rect: Rect) -> Vec<Point> {
    if !transform.is_finite()
        || !rect.is_finite()
        || rect.width() <= 0.0
        || rect.height() <= 0.0
        || transform.determinant() == 0.0
        || !transform.determinant().is_finite()
    {
        return Vec::new();
    }
    let mut corners = vec![
        transform * Point::new(rect.x0, rect.y0),
        transform * Point::new(rect.x1, rect.y0),
        transform * Point::new(rect.x1, rect.y1),
        transform * Point::new(rect.x0, rect.y1),
    ];
    if transform.determinant() < 0.0 {
        corners.reverse();
    }
    if corners.iter().any(|p| !p.is_finite()) {
        corners.clear();
    }
    corners
}

// Sutherland-Hodgman intersection of two convex polygons with positive winding.
// Unlike an axis-aligned bounding box this preserves rotated and reflected clips.
fn intersect(mut subject: Vec<Point>, clip: &[Point]) -> Vec<Point> {
    if clip.len() < 3 {
        return Vec::new();
    }
    for (a, b) in clip
        .iter()
        .copied()
        .zip(clip.iter().copied().cycle().skip(1))
    {
        let edge = b - a;
        let distance = |p: Point| edge.cross(p - a);
        let Some(mut previous) = subject.last().copied() else {
            break;
        };
        let mut previous_distance = distance(previous);
        let mut output = Vec::new();
        for current in subject {
            let current_distance = distance(current);
            if (previous_distance < 0.0) != (current_distance < 0.0) {
                let t = previous_distance / (previous_distance - current_distance);
                output.push(previous + (current - previous) * t);
            }
            if current_distance >= 0.0 {
                output.push(current);
            }
            previous = current;
            previous_distance = current_distance;
        }
        subject = output;
    }
    subject
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_clipping_preserves_uvs() {
        let placement = Placement::rect(Rect::new(-25.0, 50.0, 25.0, 150.0));
        let vertices = placement.vertices([100, 100]);
        assert_eq!(vertices.len(), 6);
        assert!(vertices.contains(&[-1.0, 0.0, 0.5, 0.0]));
        assert!(vertices.contains(&[-0.5, -1.0, 1.0, 0.5]));
        assert!(
            placement
                .translated(Vec2::new(200.0, 0.0))
                .vertices([100, 100])
                .is_empty()
        );
    }

    #[test]
    fn subrectangle_retains_affine_clip_and_reflection() {
        let placement = Placement::from_transform(
            Affine::translate((50.0, 50.0))
                * Affine::rotate(0.7)
                * Affine::scale_non_uniform(-40.0, 20.0),
        );
        let sub = placement.subrect(Rect::new(-0.5, 0.0, 0.5, 1.0));
        assert!(sub.clip_polygon().len() >= 3);
        for vertex in sub.vertices([100, 100]) {
            assert!((0.5 - 1e-6..=1.0 + 1e-6).contains(&vertex[2]));
        }
    }
}
