// Copyright 2020 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;

use accesskit::{Node, Role};
use tracing::{Span, trace_span};

use crate::core::{
    AccessCtx, ArcStr, ChildrenIds, LayoutCtx, MeasureCtx, NoAction, PaintCtx, PropertiesRef,
    Property, RegisterCtx, UpdateCtx, UsesProperty, Widget, WidgetId, WidgetMut,
};
use crate::imaging::Painter;
use crate::kurbo::{Axis, Size};
use crate::layout::{LenReq, Length};
use crate::peniko::ImageBrush;
use crate::properties::ObjectFit;

use super::image_common::{image_metadata_methods, image_widget_methods};

// TODO: Make this a configurable option of the widget.
/// The scale that the image is native to.
///
/// Used to calculate the preferred logical size of the image.
///
/// For example you might have a 256 px icon meant for crisp details on 8K screens.
/// Then the image could have an preferred scale of e.g. 4.0,
/// meaning that its preferred logical size is 64px.
/// That way the image looks good at any scale and doesn't shift the layout around.
const IMAGE_SCALE: f64 = 1.0;

// TODO - Resolve name collision between masonry::Image and peniko::Image

/// A widget that renders a bitmap Image.
///
/// The underlying image uses `Arc` for buffer data, making it cheap to clone.
///
/// This currently uses bilinear interpolation, which falls down when the image is
/// larger than its layout size (e.g. it is in a [sized box](super::SizedBox) smaller
/// than the image size).
///
/// You can change the sizing of the image with the [`ObjectFit`] property.
pub struct Image {
    brush: ImageBrush,
    decorative: bool,
    alt_text: Option<ArcStr>,
}

// --- MARK: BUILDERS
impl Image {
    /// Creates an image drawing widget from an image buffer.
    ///
    /// By default, the image will be scaled to fully fit within the container.
    /// ([`ObjectFit::Contain`]).
    #[inline]
    pub fn new(brush: impl Into<ImageBrush>) -> Self {
        Self {
            brush: brush.into(),
            decorative: false,
            alt_text: None,
        }
    }

    /// Specifies whether the image is decorative, meaning it doesn't have meaningful content
    /// and is only for visual presentation.
    ///
    /// If `is_decorative` is `true`, the image will be ignored by screen readers.
    #[must_use = "Use the returned value."]
    pub const fn decorative(mut self, is_decorative: bool) -> Self {
        self.decorative = is_decorative;
        self
    }

    /// Sets the text that will describe the image to screen readers.
    ///
    /// Users are encouraged to set alt text for the image.
    /// If possible, the alt-text should succinctly describe what the image represents.
    ///
    /// If the image is decorative users should set alt text to `""`.
    /// If it's too hard to describe through text, the alt text should be left unset.
    /// This allows accessibility clients to know that there is no accessible description of the image content.
    #[must_use = "Use the configured value."]
    pub fn with_alt_text(mut self, alt_text: impl Into<ArcStr>) -> Self {
        self.alt_text = Some(alt_text.into());
        self
    }
}

// --- MARK: WIDGETMUT
impl Image {
    /// Sets new `ImageBrush`.
    #[inline]
    pub fn set_brush(this: &mut WidgetMut<'_, Self>, brush: impl Into<ImageBrush>) {
        let brush = brush.into();
        if this.widget.brush == brush {
            return;
        }
        let dimensions_changed = this.widget.brush.image.width != brush.image.width
            || this.widget.brush.image.height != brush.image.height;
        this.widget.brush = brush;
        if dimensions_changed {
            this.ctx.request_layout();
        } else {
            this.ctx.request_paint_only();
        }
    }

    image_metadata_methods!();
}

impl Image {
    /// Returns the preferred size of the image.
    ///
    /// The returned size is in logical pixels.
    ///
    /// This takes into account [`IMAGE_SCALE`], so a high-resolution resource can
    /// have a stable preferred logical size.
    fn preferred_size(&self) -> Size {
        Size::new(
            f64::from(self.brush.image.width) / IMAGE_SCALE,
            f64::from(self.brush.image.height) / IMAGE_SCALE,
        )
    }
}

impl UsesProperty<ObjectFit> for Image {}

// --- MARK: IMPL WIDGET
impl Widget for Image {
    type Action = NoAction;

    image_widget_methods!();

    fn layout(&mut self, _ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, _size: Size) {}

    fn paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        let content_box = ctx.content_box();
        let cache = ctx.property_cache();
        let object_fit = props.get::<ObjectFit>(cache);
        // For drawing we want to scale the actual image data lengths, which means
        // we need to avoid using Image::preferred_size which does not match the data.
        let image_size = Size::new(
            f64::from(self.brush.image.width),
            f64::from(self.brush.image.height),
        );
        if image_size.is_empty() || content_box.is_empty() {
            return;
        }
        let transform = object_fit.affine(content_box, image_size.to_rect());

        painter.with_fill_clip(content_box, |painter| {
            painter.draw_image(&self.brush, transform);
        });
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("Image", id = id.trace())
    }
}

// FIXME - remove cfg?
#[cfg(not(target_arch = "wasm32"))]
// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::NewWidget;
    use crate::peniko::{ImageAlphaType, ImageData, ImageFormat};
    use crate::testing::{TestHarness, assert_render_snapshot};
    use crate::theme::test_property_set;

    /// Painting an empty image shouldn't crash.
    #[test]
    fn empty_paint() {
        // TODO - Blob::empty() function?
        // TODO: Does Vello promise this is supported?
        let brush = ImageData {
            data: Vec::new().into(),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: 0,
            height: 0,
        };

        let image_widget = NewWidget::new(Image::new(brush));
        let mut harness = TestHarness::create(test_property_set(), image_widget);
        let _ = harness.render().unwrap();
    }

    #[test]
    fn tall_paint() {
        let brush = ImageData {
            // This could have a more concise chain, but previously used versions either
            // had unreadable formatting or used `rustfmt::skip`, which broke formatting
            // across large parts of the file.
            data: [
                [255, 255, 255, 255],
                [000, 000, 000, 255],
                [000, 000, 000, 255],
                [255, 255, 255, 255],
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .into(),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: 2,
            height: 2,
        };
        let image_widget = NewWidget::new(Image::new(brush));

        let mut harness =
            TestHarness::create_with_size(test_property_set(), image_widget, (40, 60));
        assert_render_snapshot!(harness, "image_tall_paint");
    }

    #[test]
    fn same_size_image_updates_do_not_relayout_the_parent() {
        use crate::core::WidgetTag;
        use crate::kurbo::Point;
        use crate::layout::SizeDef;
        use crate::testing::ModularWidget;
        use std::cell::Cell;
        use std::rc::Rc;

        let image = |width: u32, color: [u8; 4]| ImageData {
            data: color.repeat(usize::try_from(width).unwrap()).into(),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width,
            height: 1,
        };
        let original = image(1, [255, 0, 0, 255]);
        let tag = WidgetTag::unique();
        let layouts = Rc::new(Cell::new(0));
        let count = layouts.clone();
        let parent =
            ModularWidget::new_parent(Image::new(original.clone()).prepare().with_tag(tag))
                .layout_fn(move |child, ctx, _, size| {
                    count.set(count.get() + 1);
                    let child_size = ctx.compute_size(child, SizeDef::fit(size), size.into());
                    ctx.run_layout(child, child_size);
                    ctx.place_child(child, Point::ORIGIN);
                });
        let mut harness = TestHarness::create(test_property_set(), parent.prepare());
        let before = harness.render().unwrap();
        let initial_layouts = layouts.get();
        harness
            .edit_widget(tag, |mut widget| Image::set_brush(&mut widget, original))
            .unwrap();
        assert_eq!(layouts.get(), initial_layouts);
        harness
            .edit_widget(tag, |mut widget| {
                Image::set_brush(&mut widget, image(1, [0, 0, 255, 255]))
            })
            .unwrap();
        assert_eq!(layouts.get(), initial_layouts);
        assert!(harness.render().unwrap() != before);
        harness
            .edit_widget(tag, |mut widget| {
                Image::set_brush(&mut widget, image(2, [0, 0, 255, 255]))
            })
            .unwrap();
        assert!(layouts.get() > initial_layouts);
    }

    #[test]
    fn edit_image() {
        let brush = ImageData {
            data: vec![255; 4 * 8 * 8].into(),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: 8,
            height: 8,
        };

        let render_1 = {
            let image_widget = NewWidget::new(Image::new(brush.clone()));

            let mut harness =
                TestHarness::create_with_size(test_property_set(), image_widget, (40, 60));

            harness.render().unwrap()
        };

        let render_2 = {
            let other_brush = ImageData {
                data: vec![10; 4 * 8 * 8].into(),
                format: ImageFormat::Rgba8,
                alpha_type: ImageAlphaType::Alpha,
                width: 8,
                height: 8,
            };
            let image_widget = NewWidget::new(Image::new(other_brush));

            let mut harness =
                TestHarness::create_with_size(test_property_set(), image_widget, (40, 60));

            harness
                .edit_root_widget(|mut image| {
                    Image::set_brush(&mut image, brush);
                })
                .unwrap();

            harness.render().unwrap()
        };

        // TODO - Use Kompari instead
        // We don't use assert_eq because we don't want rich assert
        assert!(render_1 == render_2);
    }

    #[test]
    fn layout() {
        let brush = ImageData {
            data: vec![255; 4 * 8 * 8].into(),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: 8,
            height: 8,
        };
        let image_widget = NewWidget::new(Image::new(brush.clone()));
        let mut harness =
            TestHarness::create_with_size(test_property_set(), image_widget, (100, 50));

        // Contain.
        harness
            .edit_root_widget(|mut image| {
                image.insert_prop(ObjectFit::Contain);
            })
            .unwrap();
        assert_render_snapshot!(harness, "image_layout_contain");

        // Cover.
        harness
            .edit_root_widget(|mut image| {
                image.insert_prop(ObjectFit::Cover);
            })
            .unwrap();
        assert_render_snapshot!(harness, "image_layout_cover");

        // FitHeight.
        harness
            .edit_root_widget(|mut image| {
                image.insert_prop(ObjectFit::FitHeight);
            })
            .unwrap();
        assert_render_snapshot!(harness, "image_layout_fitheight");

        // FitWidth.
        harness
            .edit_root_widget(|mut image| {
                image.insert_prop(ObjectFit::FitWidth);
            })
            .unwrap();
        assert_render_snapshot!(harness, "image_layout_fitwidth");

        // None.
        harness
            .edit_root_widget(|mut image| {
                image.insert_prop(ObjectFit::None);
            })
            .unwrap();
        assert_render_snapshot!(harness, "image_layout_none");

        // ScaleDown.
        harness
            .edit_root_widget(|mut image| {
                image.insert_prop(ObjectFit::ScaleDown);
            })
            .unwrap();
        assert_render_snapshot!(harness, "image_layout_scaledown");

        // Stretch.
        harness
            .edit_root_widget(|mut image| {
                image.insert_prop(ObjectFit::Stretch);
            })
            .unwrap();
        assert_render_snapshot!(harness, "image_layout_stretch");
    }
}
