// Copyright 2020 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;
use std::f64::consts::PI;

use accesskit::{Node, Role};
use include_doc_path::include_doc_path;
use tracing::{Span, trace_span};

use crate::core::{
    AccessCtx, ChildrenIds, LayoutCtx, MeasureCtx, NoAction, PaintCtx, PropertiesMut,
    PropertiesRef, Property, RegisterCtx, Update, UpdateCtx, UsesProperty, Widget, WidgetId,
};
use crate::imaging::Painter;
use crate::kurbo::{Arc, Axis, Cap, Size, Stroke, Vec2};
use crate::layout::{LenReq, Length};
use crate::properties::{AnimationDuration, TrackColor, TrackThickness};
use crate::theme;

/// An animated spinner widget for showing a loading state.
///
/// You can customize the look of this spinner with the [`TrackColor`], [`TrackThickness`] and the [`AnimationDuration`] properties.
///
#[doc = concat!(
    "![Spinner frame](",
    include_doc_path!("screenshots/spinner_init.png"),
    ")",
)]
/// Non-positive or non-finite animation durations pause rotation.
pub struct Spinner {
    rotation_progress: f64,
}

// --- MARK: DEFAULT
impl Default for Spinner {
    fn default() -> Self {
        Self {
            rotation_progress: 0.0,
        }
    }
}

// --- MARK: BUILDERS
impl Spinner {
    /// Creates a spinner widget
    #[must_use = "Use the returned value."]
    pub fn new() -> Self {
        Self::default()
    }
}

impl UsesProperty<TrackColor> for Spinner {}
impl UsesProperty<TrackThickness> for Spinner {}
impl UsesProperty<AnimationDuration> for Spinner {}

// --- MARK: IMPL WIDGET
impl Widget for Spinner {
    type Action = NoAction;

    fn on_anim_frame(
        &mut self,
        ctx: &mut UpdateCtx<'_>,
        props: &mut PropertiesMut<'_>,
        interval: u64,
    ) {
        let animation_duration_secs = props.get::<AnimationDuration>(ctx.property_cache()).seconds;
        if !animation_duration_secs.is_finite() || animation_duration_secs <= 0. {
            return;
        }
        let frame_secs = crate::util::Duration::from_nanos(interval).as_secs_f64();
        self.rotation_progress = (self.rotation_progress
            + (frame_secs % animation_duration_secs) / animation_duration_secs)
            .fract();
        ctx.request_anim_frame();
        ctx.request_paint_only();
    }

    fn register_children(&mut self, _ctx: &mut RegisterCtx<'_>) {}

    fn property_changed(&mut self, ctx: &mut UpdateCtx<'_>, property_type: TypeId) {
        if AnimationDuration::matches(property_type) {
            ctx.request_anim_frame();
        }
        if TrackColor::matches(property_type) || TrackThickness::matches(property_type) {
            ctx.request_paint_only();
        }
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        match event {
            Update::WidgetAdded => {
                ctx.request_anim_frame();
            }
            _ => (),
        }
    }

    fn measure(
        &mut self,
        _ctx: &mut MeasureCtx<'_>,
        _props: &PropertiesRef<'_>,
        _axis: Axis,
        len_req: LenReq,
        cross_length: Option<Length>,
    ) -> Length {
        match len_req {
            // For preferred length we try to keep a square aspect ratio,
            // and when the cross length is unknown we fall back to the theme's default.
            LenReq::Min | LenReq::Max => cross_length.unwrap_or(theme::BASIC_WIDGET_HEIGHT),
            LenReq::Fit(space) => space,
        }
    }

    fn layout(&mut self, _ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, _size: Size) {}

    fn paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        let content_box = ctx.content_box();
        let radius = content_box.size().min_side() / 2.0;
        let center = content_box.center();
        let sweep_angle = PI * 5.0 / 8.0;

        let cache = ctx.property_cache();
        let colors = props.get::<TrackColor>(cache);
        let thickness = props.get::<TrackThickness>(cache).0.get().min(radius);

        if thickness <= 0.0 {
            return;
        }
        let radii = Vec2::splat(radius - (thickness / 2.0));
        painter
            .stroke(
                Arc::new(center, radii, 0.0, PI * 2.0, 0.0),
                &Stroke::new(thickness).with_caps(Cap::Round),
                colors.inactive,
            )
            .draw();

        painter
            .stroke(
                Arc::new(
                    center,
                    radii,
                    self.rotation_progress * PI * 2.0,
                    sweep_angle,
                    0.0,
                ),
                &Stroke::new(thickness).with_caps(Cap::Round),
                colors.active,
            )
            .draw();
    }

    fn accessibility_role(&self) -> Role {
        Role::ProgressIndicator
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        _node: &mut Node,
    ) {
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::new()
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("Spinner", id = id.trace())
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use masonry_core::layout::AsUnit;
    use masonry_core::peniko::Color;

    use super::*;
    use crate::core::NewWidget;
    use crate::testing::{TestHarness, assert_render_snapshot};
    use crate::theme::test_property_set;

    #[test]
    fn paused_animation_restarts_and_repaints_changed_properties() {
        let spinner = Spinner::new()
            .prepare()
            .with_props(AnimationDuration { seconds: 0.0 });
        let mut harness = TestHarness::create_with_size(test_property_set(), spinner, (100, 100));
        harness.animate_ms(250);
        assert_eq!(harness.root_widget().unwrap().rotation_progress, 0.0);
        let initial = harness.render().unwrap();
        harness
            .edit_root_widget(|mut spinner| {
                spinner.insert_prop(TrackColor {
                    active: Color::from_rgba8(0xff, 0, 0, 0xff),
                    inactive: Color::from_rgba8(0, 0, 0xff, 0xff),
                });
            })
            .unwrap();
        let recolored = harness.render().unwrap();
        assert!(initial != recolored);
        harness
            .edit_root_widget(|mut spinner| {
                spinner.insert_prop(TrackThickness(0.px()));
            })
            .unwrap();
        assert!(recolored != harness.render().unwrap());
        harness
            .edit_root_widget(|mut spinner| {
                spinner.insert_prop(AnimationDuration { seconds: 1.0 });
            })
            .unwrap();
        harness.animate_ms(250);
        assert_eq!(harness.root_widget().unwrap().rotation_progress, 0.25);
        for seconds in [f64::NAN, f64::INFINITY, -1.0] {
            harness
                .edit_root_widget(|mut spinner| {
                    spinner.insert_prop(AnimationDuration { seconds });
                })
                .unwrap();
            harness.animate_ms(250);
            assert_eq!(harness.root_widget().unwrap().rotation_progress, 0.25);
        }
    }

    #[test]
    fn simple_spinner() {
        let spinner = NewWidget::new(Spinner::new());

        let mut harness = TestHarness::create_with_size(test_property_set(), spinner, (100, 100));
        assert_render_snapshot!(harness, "spinner_init");

        harness.animate_ms(700);
        assert_render_snapshot!(harness, "spinner_700ms");

        harness.animate_ms(400);
        assert_render_snapshot!(harness, "spinner_1100ms");
    }

    #[test]
    fn blue_spinner() {
        let spinner = NewWidget::new(Spinner::new());

        let mut props = test_property_set();
        props.insert::<Spinner, _>(TrackColor {
            active: Color::from_rgba8(0xff, 0xff, 0xff, 0xff),
            inactive: Color::from_rgba8(0x2a, 0x00, 0x96, 0xff),
        });

        let mut harness = TestHarness::create_with_size(props, spinner, (100, 100));
        assert_render_snapshot!(harness, "spinner_blue");
    }

    #[test]
    fn thick_spinner() {
        let spinner = NewWidget::new(Spinner::new());

        let mut props = test_property_set();
        props.insert::<Spinner, _>(TrackThickness(40.px()));

        let mut harness = TestHarness::create_with_size(props, spinner, (100, 100));
        assert_render_snapshot!(harness, "spinner_thick");
    }
}
