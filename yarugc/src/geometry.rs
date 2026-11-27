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
            clip: corners(transform, Rect::new(0.0, 0.0, 1.0, 1.0))
                .map_or_else(Vec::new, Vec::from),
        }
    }

    /// Apply an additional rectangular clip expressed in target pixels.
    #[must_use = "Use the clipped placement."]
    pub fn clipped(mut self, rect: Rect) -> Self {
        self.clip = intersect_rect(self.clip, Affine::IDENTITY, rect);
        self
    }

    /// Place a subrectangle expressed in the original normalized coordinates.
    /// The original clip is retained, even if the new rectangle extends outside it.
    #[must_use = "Use the subrectangle placement."]
    pub fn subrect(&self, rect: Rect) -> Self {
        let transform = self.transform * rect_transform(rect);
        let clip = if rect.width() > 0.0 && rect.height() > 0.0 {
            corners(transform, Rect::new(0.0, 0.0, 1.0, 1.0))
                .map_or_else(Vec::new, |points| intersect(Vec::from(points), &self.clip))
        } else {
            Vec::new()
        };
        Self { transform, clip }
    }

    /// Move the placement and its clip by the same physical-pixel displacement.
    #[must_use = "Use the translated placement."]
    pub fn translated(&self, offset: Vec2) -> Self {
        let mut clip: Vec<_> = self.clip.iter().map(|p| *p + offset).collect();
        if !has_positive_area(&clip) {
            clip.clear();
        }
        Self {
            transform: self.transform.then_translate(offset),
            clip,
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
        let determinant = self.transform.determinant();
        if width == 0.0
            || height == 0.0
            || !self.transform.is_finite()
            || determinant == 0.0
            || !determinant.is_finite()
        {
            return Vec::new();
        }
        let inverse = self.transform.inverse();
        if !inverse.is_finite() {
            return Vec::new();
        }
        let polygon = intersect_rect(
            self.clip.clone(),
            Affine::IDENTITY,
            Rect::new(0.0, 0.0, width, height),
        );
        let vertex = |p: Point| {
            let uv = inverse * p;
            let vertex = [
                (2.0 * p.x / width - 1.0) as f32,
                (1.0 - 2.0 * p.y / height) as f32,
                uv.x as f32,
                uv.y as f32,
            ];
            vertex.iter().all(|v| v.is_finite()).then_some(vertex)
        };
        let mut result = Vec::new();
        triangulate(&polygon, &mut result, vertex, false);
        result
    }
}

// Reuse the clipped slot and working buffers for a series of glyph subrectangles.
// Clip in slot-local space so the cell bounds and culling use the same coordinates.
pub struct SubrectMesh {
    transform: Affine,
    extent: [f64; 2],
    clip: Vec<Point>,
    bounds: Rect,
    polygon: Vec<Point>,
    scratch: Vec<Point>,
    vertices: Vec<[f32; 4]>,
}

impl SubrectMesh {
    pub(crate) fn new(placement: &Placement, extent: [u32; 2]) -> Option<Self> {
        let extent = extent.map(f64::from);
        let [width, height] = extent;
        let transform = placement.transform;
        let determinant = transform.determinant();
        if determinant == 0.0 || !determinant.is_finite() {
            return None;
        }
        let inverse = transform.inverse();
        if !inverse.is_finite() {
            return None;
        }
        let mut clip = intersect_rect(
            placement.clip.clone(),
            Affine::IDENTITY,
            Rect::new(0.0, 0.0, width, height),
        );
        for p in &mut clip {
            *p = inverse * *p;
        }
        if determinant < 0.0 {
            clip.reverse();
        }
        if !has_positive_area(&clip) {
            return None;
        }
        let first = *clip.first()?;
        let bounds = clip
            .iter()
            .copied()
            .fold(Rect::from_points(first, first), |bounds, point| {
                bounds.union_pt(point)
            });
        Some(Self {
            transform,
            extent,
            clip,
            bounds,
            polygon: Vec::new(),
            scratch: Vec::new(),
            vertices: Vec::new(),
        })
    }

    pub(crate) const fn bounds(&self) -> Rect {
        self.bounds
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "GPU vertex coordinates are f32"
    )]
    pub(crate) fn vertices(&mut self, rect: Rect) -> &[[f32; 4]] {
        self.vertices.clear();
        let Some(edges) = corners(Affine::IDENTITY, rect) else {
            return &self.vertices;
        };
        self.polygon.clear();
        self.polygon.extend_from_slice(&self.clip);
        intersect_in_place(&mut self.polygon, &edges, &mut self.scratch);
        let [width, height] = self.extent;
        let transform = self.transform;
        let vertex = |p: Point| {
            let target = transform * p;
            let v = [
                (2.0 * target.x / width - 1.0) as f32,
                (1.0 - 2.0 * target.y / height) as f32,
                ((p.x - rect.x0) / rect.width()) as f32,
                ((p.y - rect.y0) / rect.height()) as f32,
            ];
            v.iter().all(|v| v.is_finite()).then_some(v)
        };
        triangulate(
            &self.polygon,
            &mut self.vertices,
            vertex,
            transform.determinant() < 0.0,
        );
        &self.vertices
    }
}

fn triangulate(
    polygon: &[Point],
    vertices: &mut Vec<[f32; 4]>,
    vertex: impl Fn(Point) -> Option<[f32; 4]>,
    reverse: bool,
) {
    vertices.clear();
    let mut points = polygon.iter().copied();
    let Some(first) = points.next().and_then(&vertex) else {
        return;
    };
    let Some(mut previous) = points.next().and_then(&vertex) else {
        return;
    };
    vertices.reserve((polygon.len() - 2) * 3);
    for point in points {
        let Some(current) = vertex(point) else {
            vertices.clear();
            return;
        };
        if reverse {
            vertices.extend([first, current, previous]);
        } else {
            vertices.extend([first, previous, current]);
        }
        previous = current;
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
        if !scale_factor.is_finite()
            || scale_factor <= 0.0
            || !rect.is_finite()
            || rect.width() <= 0.0
            || rect.height() <= 0.0
        {
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
                placement.clip =
                    intersect_rect(placement.clip, scale * ctx.window_transform(), clip);
            }
        }
        (!placement.clip.is_empty()).then_some(Self {
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

// Positive signed area is a geometric invariant, not an approximate equality
// test: touching or collapsed polygons have no visible interior.
fn has_positive_area(polygon: &[Point]) -> bool {
    if polygon.len() < 3 || polygon.iter().any(|point| !point.is_finite()) {
        return false;
    }
    let origin = polygon[0];
    let twice_area: f64 = polygon[1..]
        .windows(2)
        .map(|pair| (pair[0] - origin).cross(pair[1] - origin))
        .sum();
    twice_area.is_finite() && twice_area > 0.0
}

fn corners(transform: Affine, rect: Rect) -> Option<[Point; 4]> {
    let determinant = transform.determinant();
    if !transform.is_finite()
        || !rect.is_finite()
        || rect.width() <= 0.0
        || rect.height() <= 0.0
        || determinant == 0.0
        || !determinant.is_finite()
    {
        return None;
    }
    let mut corners = [
        transform * Point::new(rect.x0, rect.y0),
        transform * Point::new(rect.x1, rect.y0),
        transform * Point::new(rect.x1, rect.y1),
        transform * Point::new(rect.x0, rect.y1),
    ];
    if determinant < 0.0 {
        corners.reverse();
    }
    has_positive_area(&corners).then_some(corners)
}

fn intersect_rect(subject: Vec<Point>, transform: Affine, rect: Rect) -> Vec<Point> {
    corners(transform, rect).map_or_else(Vec::new, |clip| intersect(subject, &clip))
}

fn push_distinct(points: &mut Vec<Point>, point: Point) {
    if points.last() != Some(&point) {
        points.push(point);
    }
}

// Sutherland-Hodgman intersection of two convex polygons with positive winding.
// Unlike an axis-aligned bounding box this preserves rotated and reflected clips.
// Reuse the two buffers across clipping planes rather than allocate per edge.
fn intersect(mut subject: Vec<Point>, clip: &[Point]) -> Vec<Point> {
    let mut scratch = Vec::new();
    intersect_in_place(&mut subject, clip, &mut scratch);
    subject
}

fn intersect_in_place(subject: &mut Vec<Point>, clip: &[Point], output: &mut Vec<Point>) {
    if subject.is_empty() || clip.len() < 3 {
        subject.clear();
        return;
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
        if !previous_distance.is_finite() {
            subject.clear();
            return;
        }
        output.clear();
        for current in subject.iter().copied() {
            let current_distance = distance(current);
            if !current_distance.is_finite() {
                subject.clear();
                return;
            }
            // Strictly crossing an edge creates a new vertex. A vertex exactly
            // on the edge is already included by the ordinary inside test.
            if (previous_distance < 0.0 && current_distance > 0.0)
                || (previous_distance > 0.0 && current_distance < 0.0)
            {
                let scale = previous_distance.abs().max(current_distance.abs());
                let p = previous_distance / scale;
                let c = current_distance / scale;
                let t = p / (p - c);
                let crossing = previous + (current - previous) * t;
                if !crossing.is_finite() {
                    subject.clear();
                    return;
                }
                push_distinct(output, crossing);
            }
            if current_distance >= 0.0 {
                push_distinct(output, current);
            }
            previous = current;
            previous_distance = current_distance;
        }
        if output.len() > 1 && output.first() == output.last() {
            output.pop();
        }
        std::mem::swap(subject, output);
    }
    if !has_positive_area(subject) {
        subject.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reusable_subrect_mesh_preserves_visible_area_and_uvs() {
        let extent = [100, 100];
        for transform in [
            Affine::new([45.0, 0.0, 0.0, 38.0, -8.0, 8.0]),
            Affine::translate((50.0, 50.0))
                * Affine::rotate(0.7)
                * Affine::scale_non_uniform(80.0, 35.0),
            Affine::translate((50.0, 50.0))
                * Affine::rotate(0.7)
                * Affine::scale_non_uniform(-80.0, 35.0),
            Affine::new([60.0, 18.0, 15.0, 40.0, 10.0, 30.0]),
        ] {
            let placement =
                Placement::from_transform(transform).clipped(Rect::new(0.0, 12.0, 72.0, 70.0));
            let mut mesh = SubrectMesh::new(&placement, extent).unwrap();
            let area = |vertices: &[[f32; 4]]| {
                vertices
                    .chunks_exact(3)
                    .map(|tri| {
                        let [a, b, c] = [tri[0], tri[1], tri[2]]
                            .map(|v| Point::new(f64::from(v[0]), f64::from(v[1])));
                        (b - a).cross(c - a)
                    })
                    .sum::<f64>()
            };
            for i in -3..12 {
                let left = f64::from(i) / 10.0;
                let rect = Rect::new(left, -0.1, left + 0.1, 0.85);
                let reference = placement.subrect(rect).vertices(extent);
                let actual = mesh.vertices(rect);
                assert!((area(actual) - area(&reference)).abs() < 1e-6);
                for &[x, y, u, v] in actual {
                    assert!((-1e-5..=1.0 + 1e-5).contains(&u));
                    assert!((-1e-5..=1.0 + 1e-5).contains(&v));
                    let target = transform
                        * Point::new(
                            f64::from(u).mul_add(rect.width(), rect.x0),
                            f64::from(v).mul_add(rect.height(), rect.y0),
                        );
                    assert!((target.x - (f64::from(x) + 1.0) * 50.0).abs() < 1e-4);
                    assert!((target.y - (1.0 - f64::from(y)) * 50.0).abs() < 1e-4);
                }
            }
        }
        let hidden = Placement::rect(Rect::new(200.0, 0.0, 210.0, 10.0));
        assert!(SubrectMesh::new(&hidden, extent).is_none());
    }

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
    fn edge_and_corner_contact_do_not_produce_visible_geometry() {
        let placement = Placement::rect(Rect::new(0.0, 0.0, 10.0, 10.0));
        for clip in [
            Rect::new(10.0, 2.0, 20.0, 8.0),
            Rect::new(10.0, 10.0, 20.0, 20.0),
            Rect::new(2.0, 10.0, 8.0, 20.0),
        ] {
            let clipped = placement.clone().clipped(clip);
            assert!(clipped.clip_polygon().is_empty());
            assert!(clipped.vertices([100, 100]).is_empty());
        }
        assert!(
            placement
                .translated(Vec2::new(100.0, 0.0))
                .vertices([100, 100])
                .is_empty()
        );
    }

    #[test]
    fn invalid_or_collapsed_rectangles_have_empty_clips() {
        let placement = Placement::rect(Rect::new(0.0, 0.0, 10.0, 10.0));
        for rect in [
            Rect::new(1.0, 0.0, 0.0, 1.0),
            Rect::new(0.0, 1.0, 1.0, 0.0),
            Rect::new(0.0, 0.0, 0.0, 1.0),
            Rect::new(0.0, 0.0, f64::NAN, 1.0),
        ] {
            assert!(placement.subrect(rect).clip_polygon().is_empty());
            assert!(Placement::rect(rect).clip_polygon().is_empty());
        }
        assert!(
            placement
                .translated(Vec2::new(f64::INFINITY, 0.0))
                .clip_polygon()
                .is_empty()
        );
        assert!(
            placement
                .translated(Vec2::new(1e30, 0.0))
                .clip_polygon()
                .is_empty()
        );
    }

    #[test]
    fn clipping_through_existing_vertices_does_not_duplicate_them() {
        let placement = Placement::from_transform(
            Affine::translate((10.0, 10.0)) * Affine::new([2.0, 2.0, -2.0, 2.0, 0.0, 0.0]),
        );
        let clipped = placement.clipped(Rect::new(0.0, 0.0, 10.0, 20.0));
        assert_eq!(clipped.clip_polygon().len(), 3);
        assert_eq!(clipped.vertices([100, 100]).len(), 3);
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
