// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::core::keyboard::{Key, NamedKey};
use crate::core::{
    AccessEvent, EventCtx, LayoutCtx, MeasureCtx, PaintCtx, PointerButton, PointerButtonEvent,
    PointerEvent, PrePaintProps, PropertiesRef, TextEvent, Update, WidgetPod, paint_background,
    paint_box_shadow,
};
use crate::imaging::Painter;
use crate::kurbo::{Axis, Cap, Dashes, Insets, Join, Point, Size, Stroke};
use crate::layout::{LayoutSize, LenReq, Length, SizeDef};
use crate::properties::{BorderWidth, Padding};
use crate::theme;
use crate::widgets::Label;

// Shared activation policy for selection controls. Keep the keyboard press
// armed only while the control owns focus, rather than treating any Space
// release as a click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActivationKey {
    Space,
    Enter,
}

#[derive(Clone, Copy)]
pub(super) enum ActivationKeys {
    Space,
    SpaceOrEnter,
}

#[derive(Clone, Copy)]
pub(super) enum ActivationButtons {
    Primary,
    Any,
}

#[derive(Debug)]
pub(super) struct Activation {
    pressed: Option<ActivationKey>,
}

impl Activation {
    pub(super) const fn new() -> Self {
        Self { pressed: None }
    }

    pub(super) fn pointer(ctx: &mut EventCtx<'_>, event: &PointerEvent) -> bool {
        Self::pointer_event(ctx, event, ActivationButtons::Primary).is_some()
    }

    pub(super) fn pointer_event<'e>(
        ctx: &mut EventCtx<'_>,
        event: &'e PointerEvent,
        buttons: ActivationButtons,
    ) -> Option<&'e PointerButtonEvent> {
        let accepts = |event: &PointerButtonEvent| {
            matches!(buttons, ActivationButtons::Any)
                || matches!(event.button, None | Some(PointerButton::Primary))
        };
        match event {
            PointerEvent::Down(event) if accepts(event) => {
                ctx.request_focus();
                ctx.capture_pointer();
                ctx.set_handled();
                None
            }
            PointerEvent::Up(event) if accepts(event) && ctx.is_pointer_capture_target() => {
                ctx.set_handled();
                ctx.is_hovered().then_some(event)
            }
            _ => None,
        }
    }

    pub(super) fn keyboard(&mut self, ctx: &mut EventCtx<'_>, event: &TextEvent) -> bool {
        self.keyboard_with_keys(ctx, event, ActivationKeys::Space)
    }

    pub(super) fn keyboard_with_keys(
        &mut self,
        ctx: &mut EventCtx<'_>,
        event: &TextEvent,
        keys: ActivationKeys,
    ) -> bool {
        if matches!(event, TextEvent::WindowFocusChange(false)) {
            self.pressed = None;
        }
        let TextEvent::Keyboard(event) = event else {
            return false;
        };
        if !ctx.is_focus_target() {
            return false;
        }
        let key = match &event.key {
            Key::Character(c) if c == " " => ActivationKey::Space,
            Key::Named(NamedKey::Enter) if matches!(keys, ActivationKeys::SpaceOrEnter) => {
                ActivationKey::Enter
            }
            _ => return false,
        };
        ctx.set_handled();
        if event.state.is_down() {
            if !event.repeat {
                self.pressed = Some(key);
            }
            false
        } else if self.pressed == Some(key) {
            self.pressed = None;
            true
        } else {
            false
        }
    }

    pub(super) fn access(ctx: &mut EventCtx<'_>, event: &AccessEvent) -> bool {
        if event.action == accesskit::Action::Click && !ctx.is_disabled() && !ctx.is_stashed() {
            ctx.set_handled();
            true
        } else {
            false
        }
    }

    pub(super) const fn update(&mut self, event: &Update) {
        if matches!(
            event,
            Update::FocusChanged(false)
                | Update::DisabledChanged(true)
                | Update::StashedChanged(true)
        ) {
            self.pressed = None;
        }
    }
}

// The action itself remains the widget's responsibility: radio buttons
// commit selection, whereas switches and checkboxes propose a new value.
macro_rules! activation_methods {
    () => {
        fn on_pointer_event(
            &mut self,
            ctx: &mut $crate::core::EventCtx<'_>,
            _props: &mut $crate::core::PropertiesMut<'_>,
            event: &$crate::core::PointerEvent,
        ) {
            if super::selection_control::Activation::pointer(ctx, event) {
                self.activate(ctx);
            }
        }

        fn on_text_event(
            &mut self,
            ctx: &mut $crate::core::EventCtx<'_>,
            _props: &mut $crate::core::PropertiesMut<'_>,
            event: &$crate::core::TextEvent,
        ) {
            if self.activation.keyboard(ctx, event) {
                self.activate(ctx);
            }
        }

        fn on_access_event(
            &mut self,
            ctx: &mut $crate::core::EventCtx<'_>,
            _props: &mut $crate::core::PropertiesMut<'_>,
            event: &$crate::core::AccessEvent,
        ) {
            if super::selection_control::Activation::access(ctx, event) {
                self.activate(ctx);
            }
        }
    };
}
pub(super) use activation_methods;

// Keep the indicator at its preferred size unless the actual available
// content box is smaller. Reserve the same prefix in layout and painting.
pub(super) const fn indicator_side(size: Size) -> f64 {
    theme::BASIC_WIDGET_HEIGHT
        .get()
        .min(size.width)
        .min(size.height)
        .max(0.0)
}

pub(super) fn indicator_border(side: f64, requested: BorderWidth) -> BorderWidth {
    // Preserve a nonempty interior for the checkmark or radio dot even when
    // a custom theme requests a border wider than the indicator.
    BorderWidth::all(Length::px(requested.width.get().min(side / 3.0)))
}

fn indicator_prefix(side: Length, width: Option<Length>) -> Length {
    let preferred = side.saturating_add(theme::WIDGET_CONTROL_COMPONENT_PADDING);
    width.map_or(preferred, |width| preferred.min(width))
}

pub(super) fn measure(
    label: &WidgetPod<Label>,
    ctx: &mut MeasureCtx<'_>,
    axis: Axis,
    len_req: LenReq,
    cross_length: Option<Length>,
) -> Length {
    // The known cross-axis dimension limits the indicator. An unconstrained
    // axis retains the preferred minimum, rather than shrinking on its own.
    let side = cross_length.map_or(theme::BASIC_WIDGET_HEIGHT, |cross| {
        theme::BASIC_WIDGET_HEIGHT.min(cross)
    });
    let (other_length, cross_space) = match axis {
        Axis::Horizontal => {
            let width = match len_req {
                LenReq::Fit(width) => Some(width),
                _ => None,
            };
            (indicator_prefix(side, width), cross_length)
        }
        Axis::Vertical => (
            Length::ZERO,
            cross_length.map(|width| width.saturating_sub(indicator_prefix(side, Some(width)))),
        ),
    };

    let auto_length = len_req.reduce(other_length).into();
    let context_size = LayoutSize::maybe(axis.cross(), cross_space);
    let label_length = ctx.compute_length(label, auto_length, context_size, axis, cross_space);

    match axis {
        Axis::Horizontal => label_length.saturating_add(other_length),
        Axis::Vertical => label_length.max(side),
    }
}

const FOCUS_RING_OFFSET: f64 = 2.0;
const FOCUS_RING_WIDTH: f64 = 2.0;

pub(super) fn layout(
    label: &WidgetPod<Label>,
    ctx: &mut LayoutCtx<'_>,
    props: &PropertiesRef<'_>,
    size: Size,
) {
    let indicator = Length::px(indicator_side(size));
    let prefix = indicator_prefix(indicator, Some(Length::px(size.width))).get();
    let space = Size::new((size.width - prefix).max(0.0), size.height);

    let label_size = ctx.compute_size(label, SizeDef::fit(space), space.into());
    ctx.run_layout(label, label_size);
    ctx.place_child(label, Point::new(prefix, 0.0));
    ctx.derive_baselines(label);

    // The focus/hover ring is drawn outside the border box in pre_paint.
    // Report it for every state, so showing it never requires relayout.
    let cache = ctx.property_cache();
    let insets = props.get::<Padding>(cache).insets() + props.get::<BorderWidth>(cache).insets();
    ctx.set_paint_insets(insets + Insets::uniform(FOCUS_RING_OFFSET + FOCUS_RING_WIDTH / 2.0));

    // An explicit child size or an indivisible glyph may exceed its allocation.
    // Clip to the control border box, preserving pointer interaction in padding.
    if label_size.width > space.width || label_size.height > space.height {
        ctx.set_clip_path(size.to_rect() + insets);
    } else {
        ctx.clear_clip_path();
    }
}

pub(super) fn pre_paint(
    ctx: &mut PaintCtx<'_>,
    props: &PropertiesRef<'_>,
    painter: &mut Painter<'_>,
) {
    let bbox = ctx.border_box();
    let cache = ctx.property_cache();
    let p = PrePaintProps::fetch(props, cache);

    paint_box_shadow(painter, bbox, p.box_shadow, p.corner_radius);
    paint_background(painter, bbox, p.background, p.border_width, p.corner_radius);

    // Paint focus indicator around the entire widget (box + label)
    if ctx.is_focus_target() || ctx.is_hovered() {
        // TODO: Replace this custom implementation with the general paint_border()

        let focus_rect = bbox.inflate(FOCUS_RING_OFFSET, FOCUS_RING_OFFSET);

        let focus_color = p.border_color.color;
        let focus_width = FOCUS_RING_WIDTH;
        let focus_radius = 4.0;

        let focus_stroke = Stroke {
            width: focus_width,
            join: Join::Round,
            miter_limit: 10.0,
            start_cap: Cap::Round,
            end_cap: Cap::Round,
            dash_pattern: Dashes::default(),
            dash_offset: 0.0,
        };
        let focus_path = focus_rect.to_rounded_rect(focus_radius);
        painter
            .stroke(focus_path, &focus_stroke, focus_color)
            .draw();
    }
    // Skip painting the regular border while the check border uses that property
}

// Both selection controls expose the same measurement, layout and background painting.
macro_rules! control_methods {
    () => {
        fn measure(
            &mut self,
            ctx: &mut MeasureCtx<'_>,
            _props: &PropertiesRef<'_>,
            axis: Axis,
            len_req: LenReq,
            cross_length: Option<Length>,
        ) -> Length {
            super::selection_control::measure(&self.label, ctx, axis, len_req, cross_length)
        }

        fn layout(&mut self, ctx: &mut LayoutCtx<'_>, props: &PropertiesRef<'_>, size: Size) {
            super::selection_control::layout(&self.label, ctx, props, size);
        }

        fn pre_paint(
            &mut self,
            ctx: &mut PaintCtx<'_>,
            props: &PropertiesRef<'_>,
            painter: &mut Painter<'_>,
        ) {
            super::selection_control::pre_paint(ctx, props, painter);
        }
    };
}
pub(super) use control_methods;

#[cfg(test)]
mod tests {
    use crate::core::{FromDynWidget, NewWidget, Widget};
    use crate::properties::Dimensions;
    use crate::testing::TestHarness;
    use crate::theme::test_property_set;
    use crate::widgets::{Checkbox, CheckboxToggled, Flex, RadioButton, Switch, SwitchToggled};

    use super::*;

    fn exercise_activation<W>(widget: NewWidget<W>, expected: W::Action)
    where
        W: Widget + FromDynWidget,
        W::Action: PartialEq,
    {
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (100, 40));
        let id = harness.root_id();
        let press = TextEvent::key_down(Key::Character(" ".into()));
        let release = TextEvent::key_up(Key::Character(" ".into()));

        harness
            .mouse_click_on(id, Some(PointerButton::Secondary))
            .unwrap();
        assert!(harness.pop_action_erased().is_none());
        harness
            .mouse_click_on(id, Some(PointerButton::Primary))
            .unwrap();
        assert_eq!(
            harness.pop_action::<W::Action>().unwrap(),
            Some((expected, id))
        );

        // A key release on a newly focused widget must not activate it.
        harness.process_text_event(&release);
        assert!(harness.pop_action_erased().is_none());
        harness.process_text_event(&press);
        harness.focus_on(None);
        harness.focus_on(Some(id));
        harness.process_text_event(&release);
        assert!(harness.pop_action_erased().is_none());

        harness.process_text_event(&press);
        harness
            .edit_root_widget(|mut widget| widget.ctx.set_disabled(true))
            .unwrap();
        harness
            .edit_root_widget(|mut widget| widget.ctx.set_disabled(false))
            .unwrap();
        harness.focus_on(Some(id));
        harness.process_text_event(&release);
        assert!(harness.pop_action_erased().is_none());

        harness.process_text_event(&press);
        harness.process_text_event(&release);
        assert!(harness.pop_action::<W::Action>().unwrap().is_some());
        harness.accessibility_click_on(id);
        assert!(harness.pop_action::<W::Action>().unwrap().is_some());
    }

    #[test]
    fn indicator_geometry_stays_inside_available_content() {
        for (size, expected) in [
            (Size::new(100.0, 40.0), 18.0),
            (Size::new(5.0, 40.0), 5.0),
            (Size::new(40.0, 3.0), 3.0),
            (Size::ZERO, 0.0),
        ] {
            let side = indicator_side(size);
            assert!((side - expected).abs() < 1e-9);
            let border = indicator_border(side, BorderWidth::all(Length::px(100.0)));
            assert!(border.width.get() <= side / 3.0);
            let prefix = indicator_prefix(Length::px(side), Some(Length::px(size.width)));
            assert!(prefix.get() >= side && prefix.get() <= size.width);
        }
    }

    fn exercise_constrained_geometry<W: Widget + FromDynWidget>(widget: NewWidget<W>) {
        let id = widget.id();
        let control = widget.with_props((
            Dimensions::fixed(Length::px(31.0), Length::px(29.0)),
            BorderWidth::all(Length::px(12.0)),
        ));
        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            Flex::row().with_fixed(control).prepare(),
            (100, 100),
        );
        let control = harness.get_widget_with_id(id).unwrap();
        let ctx = control.ctx();
        let label = control.children()[0];
        let position = ctx.window_to_local(
            label
                .ctx()
                .local_to_window(label.ctx().border_box().origin()),
        );
        assert!(position.x >= 0.0 && position.x <= ctx.content_box().width());
        let border = ctx.border_box();
        let paint = ctx.paint_box();
        let expected = border.inflate(3.0, 3.0);
        assert!(
            paint.x0 <= expected.x0
                && paint.y0 <= expected.y0
                && paint.x1 >= expected.x1
                && paint.y1 >= expected.y1
        );
        assert!(ctx.clip_path().is_some());
        harness.focus_on(Some(id));
        assert!(harness.render().is_ok());
    }

    #[test]
    fn constrained_checkbox_and_radio_keep_indicator_and_focus_bounds_valid() {
        let label = || {
            Label::new("Oversized")
                .prepare()
                .with_props(Dimensions::fixed(Length::px(50.0), Length::px(40.0)))
        };
        exercise_constrained_geometry(Checkbox::from_label(true, label()).prepare());
        exercise_constrained_geometry(RadioButton::from_label(true, label()).prepare());
    }

    #[test]
    fn checkbox_and_switch_share_primary_and_keyboard_activation_rules() {
        exercise_activation(
            Checkbox::new(false, "Test").prepare(),
            CheckboxToggled(true),
        );
        exercise_activation(Switch::new(false).prepare(), SwitchToggled(true));
    }
}
