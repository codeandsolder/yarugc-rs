// Copyright 2018 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

// TODO - Improve the ergonomics of widget layout. The Align widget is a bandaid
// that has several problem; in particular, the fact that Align will pass "loosened"
// size constraints to its child means that "aligning" a widget may actually change
// its computed size. See https://github.com/linebender/xilem/issues/378

use accesskit::{Node, Role};
use include_doc_path::include_doc_path;
use tracing::{Span, trace_span};

use crate::core::{
    AccessCtx, ChildrenIds, LayoutCtx, NewWidget, NoAction, PaintCtx, PropertiesRef, RegisterCtx,
    Widget, WidgetId, WidgetPod,
};
use crate::core::{MeasureCtx, WidgetMut};
use crate::imaging::Painter;
use crate::kurbo::{Axis, Rect, Size};
use crate::layout::{AsUnit, LenReq, Length, SizeDef, UnitPoint};

// TODO - Have child widget type as generic argument

/// A widget that aligns its child.
///
#[doc = concat!(
    "![Right-aligned label](",
    include_doc_path!("screenshots/align_right.png"),
    ")",
)]
pub struct Align {
    alignment: UnitPoint,
    child: WidgetPod<dyn Widget>,
    width_factor: Option<f64>,
    height_factor: Option<f64>,
}

// --- MARK: BUILDERS
impl Align {
    /// Creates widget with alignment.
    ///
    /// Note that the `align` parameter is specified as a `UnitPoint` in
    /// terms of left and right. This is inadequate for bidi-aware layout
    /// and thus the API will change when Masonry gains bidi capability.
    #[must_use = "Use the returned value."]
    pub fn new(align: UnitPoint, child: NewWidget<impl Widget + ?Sized>) -> Self {
        Self {
            alignment: align,
            child: child.erased().into_pod(),
            width_factor: None,
            height_factor: None,
        }
    }

    /// Creates centered widget.
    #[must_use = "Use the returned value."]
    pub fn centered(child: NewWidget<impl Widget + ?Sized>) -> Self {
        Self::new(UnitPoint::CENTER, child)
    }

    /// Creates right-aligned widget.
    #[must_use = "Use the returned value."]
    pub fn right(child: NewWidget<impl Widget + ?Sized>) -> Self {
        Self::new(UnitPoint::RIGHT, child)
    }

    /// Creates left-aligned widget.
    #[must_use = "Use the returned value."]
    pub fn left(child: NewWidget<impl Widget + ?Sized>) -> Self {
        Self::new(UnitPoint::LEFT, child)
    }

    /// Aligns only in the horizontal axis, keeping the child's size in the vertical.
    #[must_use = "Use the returned value."]
    pub fn horizontal(align: UnitPoint, child: NewWidget<impl Widget + ?Sized>) -> Self {
        Self {
            alignment: align,
            child: child.erased().into_pod(),
            width_factor: None,
            height_factor: Some(1.0),
        }
    }

    /// Aligns only in the vertical axis, keeping the child's size in the horizontal.
    #[must_use = "Use the returned value."]
    pub fn vertical(align: UnitPoint, child: NewWidget<impl Widget + ?Sized>) -> Self {
        Self {
            alignment: align,
            child: child.erased().into_pod(),
            width_factor: Some(1.0),
            height_factor: None,
        }
    }
}

// --- MARK: WIDGETMUT
impl Align {
    /// Sets the alignment of the child.
    pub fn set_alignment(this: &mut WidgetMut<'_, Self>, alignment: UnitPoint) {
        if this.widget.alignment != alignment {
            this.widget.alignment = alignment;
            this.ctx.request_layout();
        }
    }

    child_accessors!(child, set_child, child_mut, erased);
}

// --- MARK: IMPL WIDGET
impl Widget for Align {
    type Action = NoAction;

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        ctx.register_child(&mut self.child);
    }

    fn measure(
        &mut self,
        ctx: &mut MeasureCtx<'_>,
        _props: &PropertiesRef<'_>,
        axis: Axis,
        len_req: LenReq,
        cross_length: Option<Length>,
    ) -> Length {
        let child_length = ctx.measure_child(&self.child, axis, len_req, cross_length);

        let factor = match axis {
            Axis::Horizontal => self.width_factor,
            Axis::Vertical => self.height_factor,
        };
        let default = if let LenReq::Fit(space) = len_req {
            space
        } else {
            child_length
        };
        let mut length = factor.map_or(default, |factor| (child_length.get() * factor).px());

        // Never return a length larger than the bounds
        if let LenReq::Fit(space) = len_req {
            length = length.min(space);
        }

        length
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, size: Size) {
        let child_size = ctx.compute_size(&self.child, SizeDef::fit(size), size.into());
        ctx.run_layout(&self.child, child_size);

        let extra_width = size.width - child_size.width;
        let extra_height = size.height - child_size.height;
        let child_origin = self
            .alignment
            .resolve(Rect::new(0., 0., extra_width, extra_height));
        ctx.place_child(&self.child, child_origin);

        ctx.derive_baselines(&self.child);
    }

    fn paint(
        &mut self,
        _ctx: &mut PaintCtx<'_>,
        _props: &PropertiesRef<'_>,
        _painter: &mut Painter<'_>,
    ) {
    }

    fn accessibility_role(&self) -> Role {
        Role::GenericContainer
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        _node: &mut Node,
    ) {
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::from_slice(&[self.child.id()])
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("Align", id = id.trace())
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::WidgetTag;
    use crate::layout::AsUnit;
    use crate::palette;
    use crate::peniko::color::AlphaColor;
    use crate::properties::{Background, BorderColor, BorderWidth, Dimensions};
    use crate::testing::{TestHarness, assert_render_snapshot};
    use crate::theme::test_property_set;
    use crate::widgets::{Label, SizedBox};

    #[test]
    fn centered() {
        let widget = Align::centered(Label::new("hello").prepare()).prepare();

        let mut harness = TestHarness::create(test_property_set(), widget);

        assert_render_snapshot!(harness, "align_centered");
    }

    #[test]
    fn right() {
        let widget = Align::right(Label::new("hello").prepare()).prepare();

        let mut harness = TestHarness::create(test_property_set(), widget);

        assert_render_snapshot!(harness, "align_right");
    }

    #[test]
    fn left() {
        let widget = Align::left(Label::new("hello").prepare()).prepare();

        let mut harness = TestHarness::create(test_property_set(), widget);

        assert_render_snapshot!(harness, "align_left");
    }

    #[test]
    fn oversized() {
        let align_tag = WidgetTag::unique();

        let child = SizedBox::empty().prepare().with_props((
            Dimensions::fixed(100.px(), 100.px()),
            Background::Color(AlphaColor::from_rgba8(127, 0, 0, 127)),
        ));
        let align = NewWidget::new(Align::new(UnitPoint::CENTER, child))
            .with_tag(align_tag)
            .with_props((
                Dimensions::fixed(50.px(), 50.px()),
                BorderWidth::all(2.px()),
                BorderColor::new(palette::css::BLACK),
            ));
        let root = Align::centered(align).prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), root, (200, 200));

        assert_render_snapshot!(harness, "align_oversized_center");

        harness
            .edit_widget(align_tag, |mut align| {
                Align::set_alignment(&mut align, UnitPoint::TOP_LEFT);
            })
            .unwrap();
        assert_render_snapshot!(harness, "align_oversized_top_left");

        harness
            .edit_widget(align_tag, |mut align| {
                Align::set_alignment(&mut align, UnitPoint::BOTTOM_RIGHT);
            })
            .unwrap();
        assert_render_snapshot!(harness, "align_oversized_bottom_right");
    }
}
