// Copyright 2020 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;
use std::sync::Arc;

use accesskit::{Node, Role};
use resvg::tiny_skia;
use resvg::usvg::Tree;
use tracing::{Span, trace_span};

use crate::core::{
    AccessCtx, ArcStr, ChildrenIds, LayoutCtx, MeasureCtx, NoAction, PaintCtx, PropertiesRef,
    Property, RegisterCtx, UpdateCtx, UsesProperty, Widget, WidgetId, WidgetMut,
};
use crate::imaging::Painter;
use crate::kurbo::{Affine, Axis, Size};
use crate::layout::{LenReq, Length};
use crate::peniko::{ImageAlphaType, ImageBrush, ImageData, ImageFormat};
use crate::properties::ObjectFit;

use super::image_common::image_widget_methods;

// TODO: Make this a configurable option of the widget
/// The scale that the SVG is native to.
///
/// Used to calculate the preferred logical size of the SVG.
///
/// For example you might have a 256 px icon meant for crisp details on 8K screens.
/// Then the SVG could have an preferred scale of e.g. 4.0,
/// meaning that its preferred logical size is 64px.
/// That way the SVG looks good at any scale and doesn't shift the layout around.
const SVG_SCALE: f64 = 1.0;

// Convert a validated, positive logical extent to an integer raster dimension.
#[expect(
    clippy::cast_sign_loss,
    reason = "Bounds and finiteness are checked before conversion."
)]
fn raster_dimension(extent: f64) -> Option<u32> {
    let rounded = extent.ceil();
    if rounded > 0. && rounded <= f64::from(u32::MAX) {
        Some(rounded as u32)
    } else {
        None
    }
}

/// A widget that renders an SVG.
///
/// You can change the sizing of the SVG with the [`ObjectFit`] property.
pub struct Svg {
    tree: Arc<Tree>,
    rasterized: Option<RasterizedSvg>,
    decorative: bool,
    alt_text: Option<ArcStr>,
}

struct RasterizedSvg {
    brush: ImageBrush,
    logical_size: Size,
    pixel_size: (u32, u32),
    fit: ObjectFit,
}

// --- MARK: BUILDERS
impl Svg {
    /// Creates an SVG drawing widget.
    ///
    /// By default, the SVG will be scaled to fully fit within the container.
    /// ([`ObjectFit::Contain`]).
    #[must_use = "Use the returned value."]
    pub const fn new(tree: Arc<Tree>) -> Self {
        Self {
            tree,
            rasterized: None,
            decorative: false,
            alt_text: None,
        }
    }

    /// Specifies whether the SVG is decorative, meaning it doesn't have meaningful content
    /// and is only for visual presentation.
    ///
    /// If `is_decorative` is `true`, the SVG will be ignored by screen readers.
    #[must_use = "Use the returned value."]
    pub const fn decorative(mut self, is_decorative: bool) -> Self {
        self.decorative = is_decorative;
        self
    }

    /// Sets the text that will describe the SVG to screen readers.
    ///
    /// Users are encouraged to set alt text for the SVG.
    /// If possible, the alt-text should succinctly describe what the SVG represents.
    ///
    /// If the SVG is decorative users should set alt text to `""`.
    /// If it's too hard to describe through text, the alt text should be left unset.
    /// This allows accessibility clients to know that there is no accessible description of the SVG content.
    #[must_use = "Use the configured value."]
    pub fn with_alt_text(mut self, alt_text: impl Into<ArcStr>) -> Self {
        self.alt_text = Some(alt_text.into());
        self
    }
}

// --- MARK: WIDGETMUT
impl Svg {
    /// Sets a new inner SVG.
    pub fn set_tree(this: &mut WidgetMut<'_, Self>, tree: Arc<Tree>) {
        this.widget.tree = tree;
        this.widget.rasterized = None;
        this.ctx.request_layout();
    }

    /// Sets whether the SVG is decorative, meaning it doesn't have meaningful content
    /// and is only for visual presentation.
    ///
    /// See [`Svg::decorative`] for details.
    pub fn set_decorative(this: &mut WidgetMut<'_, Self>, is_decorative: bool) {
        this.widget.decorative = is_decorative;
        this.ctx.request_accessibility_update();
    }

    /// Sets the text that will describe the SVG to screen readers.
    ///
    /// See [`Svg::with_alt_text`] for details.
    pub fn set_alt_text(this: &mut WidgetMut<'_, Self>, alt_text: Option<impl Into<ArcStr>>) {
        this.widget.alt_text = alt_text.map(Into::into);
        this.ctx.request_accessibility_update();
    }
}

impl Svg {
    /// Returns the preferred size of the SVG.
    ///
    /// The returned size is in logical pixels.
    ///
    /// This takes into account [`SVG_SCALE`], so rasterization can use a stable
    /// preferred logical size.
    fn preferred_size(&self) -> Size {
        let size = self.tree.size();
        Size::new(
            f64::from(size.width()) / SVG_SCALE,
            f64::from(size.height()) / SVG_SCALE,
        )
    }
}

impl Svg {
    fn rasterize(
        &self,
        logical_size: Size,
        pixel_size: (u32, u32),
        fit: ObjectFit,
    ) -> Option<RasterizedSvg> {
        let (width, height) = pixel_size;
        let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
        let svg_size = self.tree.size();
        let svg_size = Size::new(f64::from(svg_size.width()), f64::from(svg_size.height()));
        let logical_to_pixel = Affine::scale_non_uniform(
            f64::from(width) / logical_size.width,
            f64::from(height) / logical_size.height,
        );
        let coeffs =
            (logical_to_pixel * fit.affine(logical_size.to_rect(), svg_size.to_rect())).as_coeffs();
        let transform = tiny_skia::Transform::from_row(
            coeffs[0] as f32,
            coeffs[1] as f32,
            coeffs[2] as f32,
            coeffs[3] as f32,
            coeffs[4] as f32,
            coeffs[5] as f32,
        );
        resvg::render(&self.tree, transform, &mut pixmap.as_mut());
        let brush = ImageBrush::new(ImageData {
            data: pixmap.take().into(),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::AlphaPremultiplied,
            width,
            height,
        });
        Some(RasterizedSvg {
            brush,
            logical_size,
            pixel_size,
            fit,
        })
    }
}

impl UsesProperty<ObjectFit> for Svg {}

// --- MARK: IMPL WIDGET
impl Widget for Svg {
    type Action = NoAction;

    image_widget_methods!();

    fn layout(&mut self, _ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, _size: Size) {
        self.rasterized = None;
    }

    fn paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        let content_box = ctx.content_box();

        let logical_size = content_box.size();
        let scale = ctx.scale_factor();
        let (Some(width), Some(height)) = (
            raster_dimension(logical_size.width * scale),
            raster_dimension(logical_size.height * scale),
        ) else {
            self.rasterized = None;
            return;
        };
        let cache = ctx.property_cache();
        let fit = *props.get::<ObjectFit>(cache);
        let pixel_size = (width, height);
        if self.rasterized.as_ref().is_none_or(|raster| {
            raster.logical_size != logical_size
                || raster.pixel_size != pixel_size
                || raster.fit != fit
        }) {
            self.rasterized = self.rasterize(logical_size, pixel_size, fit);
        }
        let Some(raster) = self.rasterized.as_ref() else {
            return;
        };

        let transform = Affine::translate(content_box.origin().to_vec2())
            * Affine::scale_non_uniform(
                logical_size.width / f64::from(width),
                logical_size.height / f64::from(height),
            );
        painter.with_fill_clip(content_box, |painter| {
            painter.draw_image(&raster.brush, transform);
        });
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("Svg", id = id.trace())
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use resvg::usvg;
    use resvg::usvg::fontdb::Database;

    use super::*;
    use crate::testing::{ROBOTO, TestHarness, assert_render_snapshot};
    use crate::theme::test_property_set;

    #[test]
    fn empty_tree() {
        let xml = r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#;
        let tree = Tree::from_str(xml, &usvg::Options::default()).unwrap();
        let svg = Svg::new(Arc::new(tree)).prepare();
        let mut harness = TestHarness::create(test_property_set(), svg);
        let _ = harness.render().unwrap();
    }

    #[test]
    fn rasterization_uses_physical_scale_without_changing_logical_fit() {
        let xml = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="red"/></svg>"#;
        let tree = Tree::from_str(xml, &usvg::Options::default()).unwrap();
        let svg = Svg::new(Arc::new(tree));
        let logical_size = Size::new(20.0, 10.0);
        let raster = svg
            .rasterize(logical_size, (40, 20), ObjectFit::Contain)
            .unwrap();
        assert_eq!(
            (raster.brush.image.width, raster.brush.image.height),
            (40, 20)
        );
        let data = raster.brush.image.data.as_ref();
        let pixel = |x: usize, y: usize| &data[(y * 40 + x) * 4..(y * 40 + x) * 4 + 4];
        assert_eq!(
            pixel(0, 10),
            [0, 0, 0, 0],
            "contain must letterbox in logical coordinates"
        );
        assert_eq!(pixel(10, 10), [255, 0, 0, 255]);
        assert_eq!(pixel(29, 10), [255, 0, 0, 255]);
        assert_eq!(pixel(30, 10), [0, 0, 0, 0]);
    }

    #[test]
    fn brick_wall() {
        let xml = r###"
            <svg xmlns="http://www.w3.org/2000/svg" width="852" height="320" viewBox="0 0 852 320">
                <defs>
                    <!-- Clip the wall so we can reuse full bricks as edge half-bricks -->
                    <clipPath id="wall-clip">
                        <rect x="10" y="10" width="833" height="310" />
                    </clipPath>

                    <!-- One reusable brick -->
                    <g id="brick">
                        <rect x="0" y="0" width="160" height="70" rx="4" ry="4"
                            fill="#b24a3a" stroke="#8b3429" stroke-width="2" />
                        <text x="80" y="42"
                            text-anchor="middle"
                            font-family="Roboto"
                            font-size="24"
                            fill="#f7e7cf">Masonry</text>
                    </g>
                </defs>

                <!-- Mortar background -->
                <rect x="0" y="0" width="852" height="320" fill="#d8d3cd" />

                <!-- All bricks are clipped to the wall area -->
                <g clip-path="url(#wall-clip)">
                    <!-- Row 1 -->
                    <use href="#brick" x="10"  y="10" />
                    <use href="#brick" x="178" y="10" />
                    <use href="#brick" x="346" y="10" />
                    <use href="#brick" x="514" y="10" />
                    <use href="#brick" x="682" y="10" />

                    <!-- Row 2 (staggered + clipped) -->
                    <use href="#brick" x="-70" y="88" />
                    <use href="#brick" x="98"  y="88" />
                    <use href="#brick" x="266" y="88" />
                    <use href="#brick" x="434" y="88" />
                    <use href="#brick" x="602" y="88" />
                    <use href="#brick" x="770" y="88" />

                    <!-- Row 3 -->
                    <use href="#brick" x="10"  y="166" />
                    <use href="#brick" x="178" y="166" />
                    <use href="#brick" x="346" y="166" />
                    <use href="#brick" x="514" y="166" />
                    <use href="#brick" x="682" y="166" />

                    <!-- Row 4 (staggered + clipped) -->
                    <use href="#brick" x="-70" y="244" />
                    <use href="#brick" x="98"  y="244" />
                    <use href="#brick" x="266" y="244" />
                    <use href="#brick" x="434" y="244" />
                    <use href="#brick" x="602" y="244" />
                    <use href="#brick" x="770" y="244" />
                </g>
                </svg>
        "###;

        let mut fontdb = Database::new();
        fontdb.load_font_data(ROBOTO.into());

        let opts = usvg::Options {
            fontdb: Arc::new(fontdb),
            ..usvg::Options::default()
        };

        let tree = Tree::from_str(xml, &opts).unwrap();
        let svg = Svg::new(Arc::new(tree)).prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), svg, (852, 320));

        assert_render_snapshot!(harness, "svg_brick_wall");
    }
}
