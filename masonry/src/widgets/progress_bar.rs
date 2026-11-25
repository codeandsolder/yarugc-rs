// Copyright 2019 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;

use accesskit::{Node, Role};
use include_doc_path::include_doc_path;
use tracing::{Span, trace_span};

use crate::core::{
    AccessCtx, ArcStr, ChildrenIds, LayoutCtx, MeasureCtx, NewWidget, NoAction, PaintCtx,
    PrePaintProps, PropertiesMut, PropertiesRef, Property, PropertySet, RegisterCtx, Update,
    UpdateCtx, Widget, WidgetId, WidgetMut, WidgetPod, paint_background, paint_border,
    paint_box_shadow,
};
use crate::imaging::Painter;
use crate::kurbo::{Axis, Size};
use crate::layout::{LayoutSize, LenReq, Length, SizeDef};
use crate::peniko::{Color, Gradient};
use crate::properties::{BarColor, BorderColor, BorderWidth, CornerRadius, LineBreaking};
use crate::theme;
use crate::widgets::Label;

// TODO - NaN probably shouldn't be a meaningful value in our API.

/// A progress bar.
///
#[doc = concat!(
    "![25% progress bar](",
    include_doc_path!("screenshots/progress_bar_25_percent.png"),
    ")",
)]
pub struct ProgressBar {
    /// A value in the range `[0, 1]` inclusive, where 0 is 0% and 1 is 100% complete.
    ///
    /// `None` variant can be used to show a progress bar without a percentage.
    /// NaN is treated as indeterminate; other values are clamped.
    progress: Option<f64>,
    label: WidgetPod<Label>,
}

// --- MARK: BUILDERS
impl ProgressBar {
    /// Creates a new `ProgressBar`.
    ///
    /// The progress value will be clamped to [0, 1].
    ///
    /// A `None` value (or NaN) will show an indeterminate progress bar.
    #[must_use = "Use the returned value."]
    pub fn new(progress: Option<f64>) -> Self {
        let progress = clamp_progress(progress);
        let label_props = PropertySet::one(LineBreaking::Overflow);
        let label = NewWidget::new(Label::new(Self::value(progress)).accessibility_hidden(true))
            .with_props(label_props)
            .into_pod();
        Self { progress, label }
    }
}

// --- MARK: METHODS
impl ProgressBar {
    fn value_accessibility(&self) -> Box<str> {
        self.progress.map_or_else(
            || "progress unspecified".into(),
            |value| format!("{:.0}%", value * 100.).into(),
        )
    }

    fn percent(progress: Option<f64>) -> Option<f64> {
        progress.map(|value| (value * 100.).round_ties_even())
    }

    fn value(progress: Option<f64>) -> ArcStr {
        Self::percent(progress).map_or_else(|| "".into(), |value| format!("{value:.0}%").into())
    }
}

// --- MARK: WIDGETMUT
impl ProgressBar {
    /// Sets the progress displayed by the bar.
    ///
    /// The progress value will be clamped to [0, 1].
    ///
    /// A `None` value (or NaN) will show an indeterminate progress bar.
    pub fn set_progress(this: &mut WidgetMut<'_, Self>, progress: Option<f64>) {
        let progress = clamp_progress(progress);
        if this.widget.progress == progress {
            return;
        }
        let label_changed = Self::percent(this.widget.progress) != Self::percent(progress);
        this.widget.progress = progress;
        if label_changed && let Some(mut label) = this.ctx.get_mut(this.widget.label.id()) {
            Label::set_text(&mut label, Self::value(progress));
        }
        this.ctx.request_paint_only();
        this.ctx.request_accessibility_update();
    }
}

/// Helper to ensure progress is either a number between [0, 1] inclusive, or `None`.
///
/// NaNs are converted to `None`.
fn clamp_progress(progress: Option<f64>) -> Option<f64> {
    let progress = progress?;
    if progress.is_nan() {
        None
    } else {
        Some(progress.max(0.).clamp(0., 1.))
    }
}

// --- MARK: IMPL WIDGET
impl Widget for ProgressBar {
    type Action = NoAction;

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        ctx.register_child(&mut self.label);
    }

    fn property_changed(&mut self, ctx: &mut UpdateCtx<'_>, property_type: TypeId) {
        if BarColor::matches(property_type)
            || BorderWidth::matches(property_type)
            || BorderColor::matches(property_type)
            || CornerRadius::matches(property_type)
        {
            ctx.request_paint_only();
        }
    }

    fn update(
        &mut self,
        _ctx: &mut UpdateCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        _event: &Update,
    ) {
    }

    fn measure(
        &mut self,
        ctx: &mut MeasureCtx<'_>,
        _props: &PropertiesRef<'_>,
        axis: Axis,
        len_req: LenReq,
        cross_length: Option<Length>,
    ) -> Length {
        // TODO: Move this to theme?
        const DEFAULT_WIDTH: Length = crate::layout::length!(400.);

        let auto_length = len_req.into();
        let context_size = LayoutSize::maybe(axis.cross(), cross_length);

        let label_length =
            ctx.compute_length(&self.label, auto_length, context_size, axis, cross_length);

        let potential_length = match axis {
            Axis::Horizontal => match len_req {
                LenReq::Min | LenReq::Max => DEFAULT_WIDTH,
                LenReq::Fit(space) => space,
            },
            Axis::Vertical => theme::BASIC_WIDGET_HEIGHT,
        };

        // Make sure we always report a length big enough to fit our painting
        potential_length.max(label_length)
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, size: Size) {
        let label_size = ctx.compute_size(&self.label, SizeDef::fit(size), size.into());
        ctx.run_layout(&self.label, label_size);

        let child_origin = ((size - label_size).to_vec2() * 0.5).to_point();
        ctx.place_child(&self.label, child_origin);

        ctx.derive_baselines(&self.label);
    }

    fn pre_paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        let bbox = ctx.border_box();
        let cache = ctx.property_cache();
        let p = PrePaintProps::fetch(props, cache);

        paint_box_shadow(painter, bbox, p.box_shadow, p.corner_radius);
        paint_background(painter, bbox, p.background, p.border_width, p.corner_radius);
        // We need to delay painting the border until after we paint the filled bar area.
    }

    fn paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        let border_box = ctx.border_box();
        let cache = ctx.property_cache();
        let border_width = *props.get::<BorderWidth>(cache);
        let corner_radius = *props.get::<CornerRadius>(cache);
        let border_color = *props.get::<BorderColor>(cache);

        let progress = self.progress.unwrap_or(1.);
        if progress > 0. {
            // The bar width is without the borders.
            let total_border = 2.0 * border_width.width.get();
            if border_box.width() > total_border && border_box.height() > total_border {
                let bg_rect = border_width.bg_rect(border_box, corner_radius);
                let bounds = bg_rect.rect();
                let bar_color = props.get::<BarColor>(cache).0;
                // Paint with a gradient so we get a straight line slice of the rounded rect.
                let gradient = Gradient::new_linear((bounds.x0, 0.), (bounds.x1, 0.)).with_stops([
                    (0., bar_color),
                    (progress as f32, bar_color),
                    (progress as f32, Color::TRANSPARENT),
                    (1., Color::TRANSPARENT),
                ]);

                // Currently bg_rect() gives a rect without borders, so we can use it.
                // However in the future when bg_rect() gets expanded to include borders,
                // we'll need to create a special sans-border rect for this fill.
                painter.fill(bg_rect, &gradient).draw();
            }
        }

        paint_border(
            painter,
            border_box,
            &border_color,
            border_width,
            corner_radius,
        );
    }

    fn accessibility_role(&self) -> Role {
        Role::ProgressIndicator
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        node.set_min_numeric_value(0.0);
        node.set_max_numeric_value(1.0);
        if let Some(value) = self.progress {
            node.set_numeric_value(value);
            node.set_value(Self::value(Some(value)).as_ref());
        }
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::from_slice(&[self.label.id()])
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("ProgressBar", id = id.trace())
    }

    fn get_debug_text(&self) -> Option<String> {
        Some(self.value_accessibility().into())
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{NewWidget, PropertySet};
    use crate::layout::AsUnit;
    use crate::palette;
    use crate::properties::{BorderColor, CornerRadius};
    use crate::testing::{TestHarness, assert_render_snapshot};
    use crate::theme::test_property_set;

    #[test]
    fn subpercentage_updates_repaint_without_layout() {
        use crate::core::WidgetTag;
        use crate::testing::{ModularWidget, Record, TestWidgetExt};

        let tag = WidgetTag::named("progress");
        let parent = WidgetTag::named("progress-parent");
        let child = ProgressBar::new(Some(0.25)).prepare().with_tag(tag);
        let root = ModularWidget::new_parent(child)
            .record()
            .prepare()
            .with_tag(parent);
        let mut harness = TestHarness::create_with_size(test_property_set(), root, (150, 60));
        let id = harness.get_widget(tag).unwrap().id();
        let _ = harness.redraw();
        harness.flush_records_of(parent).unwrap();

        harness
            .edit_widget(tag, |mut bar| {
                ProgressBar::set_progress(&mut bar, Some(0.251))
            })
            .unwrap();
        let _ = harness.redraw();
        let records = harness.take_records_of(parent).unwrap();
        assert!(
            !records
                .iter()
                .any(|record| matches!(record, Record::Layout(_) | Record::Measure(_))),
            "fractional updates retaining the same label must not trigger parent layout: {records:?}"
        );
        assert_eq!(
            harness.access_node(id).unwrap().data().numeric_value(),
            Some(0.251)
        );

        harness
            .edit_widget(tag, |mut bar| {
                ProgressBar::set_progress(&mut bar, Some(0.26))
            })
            .unwrap();
        let _ = harness.redraw();
        assert_eq!(harness.access_node(id).unwrap().data().value(), Some("26%"));
        let label_id = harness.get_widget(tag).unwrap().label.id();
        assert_eq!(
            harness
                .get_widget_with_id(label_id)
                .unwrap()
                .downcast::<Label>()
                .unwrap()
                .text()
                .as_ref(),
            "26%"
        );
    }

    #[test]
    fn symmetric_padding_does_not_shift_the_fill_boundary() {
        use crate::properties::Padding;
        let bar = |padding| {
            ProgressBar::new(Some(0.25)).prepare().with_props((
                Padding::horizontal(padding),
                BorderWidth::all(2.px()),
                BarColor(palette::css::RED),
            ))
        };
        let mut without =
            TestHarness::create_with_size(test_property_set(), bar(Length::ZERO), (150, 60));
        let mut padded =
            TestHarness::create_with_size(test_property_set(), bar(12.px()), (150, 60));
        let original = without.render().unwrap();
        let image = padded.render().unwrap();
        assert!(
            original == image,
            "padding changes text layout space, not the inner-border fill geometry"
        );
    }

    #[test]
    fn indeterminate_progressbar() {
        let widget = NewWidget::new(ProgressBar::new(None));

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (150, 60));

        assert_render_snapshot!(harness, "progress_bar_indeterminate");
    }

    #[test]
    fn _5_percent_styled_progressbar() {
        let widget = ProgressBar::new(Some(0.05)).prepare().with_props((
            CornerRadius::all(50.px()),
            BorderWidth::all(10.px()),
            BorderColor::new(palette::css::PINK),
        ));
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (150, 60));

        assert_render_snapshot!(harness, "progress_bar_5_percent_styled");
    }

    #[test]
    fn _95_percent_styled_progressbar() {
        let widget = ProgressBar::new(Some(0.95)).prepare().with_props((
            CornerRadius::all(50.px()),
            BorderWidth::all(10.px()),
            BorderColor::new(palette::css::PINK),
        ));
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (150, 60));

        assert_render_snapshot!(harness, "progress_bar_95_percent_styled");
    }

    #[test]
    fn _0_percent_progressbar() {
        let widget = NewWidget::new(ProgressBar::new(Some(0.)));
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (150, 60));

        assert_render_snapshot!(harness, "progress_bar_0_percent");
    }

    #[test]
    fn _25_percent_progressbar() {
        let widget = NewWidget::new(ProgressBar::new(Some(0.25)));
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (150, 60));

        assert_render_snapshot!(harness, "progress_bar_25_percent");
    }

    #[test]
    fn _50_percent_progressbar() {
        let widget = NewWidget::new(ProgressBar::new(Some(0.5)));
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (150, 60));

        assert_render_snapshot!(harness, "progress_bar_50_percent");
    }

    #[test]
    fn _75_percent_progressbar() {
        let widget = NewWidget::new(ProgressBar::new(Some(0.75)));
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (150, 60));

        assert_render_snapshot!(harness, "progress_bar_75_percent");
    }

    #[test]
    fn _100_percent_progressbar() {
        let widget = NewWidget::new(ProgressBar::new(Some(1.)));
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (150, 60));

        assert_render_snapshot!(harness, "progress_bar_100_percent");
    }

    #[test]
    fn edit_progressbar() {
        let image_1 = {
            let bar = ProgressBar::new(Some(0.5))
                .prepare()
                .with_props(PropertySet::new().with(BarColor(palette::css::PURPLE)));

            let mut harness = TestHarness::create_with_size(test_property_set(), bar, (60, 20));

            harness.render().unwrap()
        };

        let image_2 = {
            let bar = NewWidget::new(ProgressBar::new(None));

            let mut harness = TestHarness::create_with_size(test_property_set(), bar, (60, 20));

            harness
                .edit_root_widget(|mut bar| {
                    ProgressBar::set_progress(&mut bar, Some(0.5));
                    bar.insert_prop(BarColor(palette::css::PURPLE));
                })
                .unwrap();

            harness.render().unwrap()
        };

        // We don't use assert_eq because we don't want rich assert
        assert!(image_1 == image_2);
    }
}
