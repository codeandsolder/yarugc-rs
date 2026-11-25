// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::core::keyboard::{Key, NamedKey};
use crate::core::{
    AccessEvent, EventCtx, LayoutCtx, MeasureCtx, PaintCtx, PointerButton, PointerButtonEvent,
    PointerEvent, PrePaintProps, PropertiesRef, TextEvent, Update, WidgetPod, paint_background,
    paint_box_shadow,
};
use crate::imaging::Painter;
use crate::kurbo::{Axis, Cap, Dashes, Join, Point, Size, Stroke};
use crate::layout::{LayoutSize, LenReq, Length, SizeDef};
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

pub(super) fn measure(
    label: &WidgetPod<Label>,
    ctx: &mut MeasureCtx<'_>,
    axis: Axis,
    len_req: LenReq,
    cross_length: Option<Length>,
) -> Length {
    let check_side = theme::BASIC_WIDGET_HEIGHT;
    let check_padding = theme::WIDGET_CONTROL_COMPONENT_PADDING;

    let calc_other_length = |axis| match axis {
        Axis::Horizontal => check_side.saturating_add(check_padding),
        Axis::Vertical => Length::ZERO,
    };
    let other_length = calc_other_length(axis);

    let cross = axis.cross();
    let cross_space = cross_length.map(|cross_length| {
        let cross_other_length = calc_other_length(cross);
        cross_length.saturating_sub(cross_other_length)
    });

    let auto_length = len_req.reduce(other_length).into();
    let context_size = LayoutSize::maybe(cross, cross_space);

    let label_length = ctx.compute_length(label, auto_length, context_size, axis, cross_space);

    match axis {
        Axis::Horizontal => label_length.saturating_add(other_length),
        Axis::Vertical => label_length.max(check_side).saturating_add(other_length),
    }
}

pub(super) fn layout(label: &WidgetPod<Label>, ctx: &mut LayoutCtx<'_>, size: Size) {
    let check_side = theme::BASIC_WIDGET_HEIGHT.get();
    let check_padding = theme::WIDGET_CONTROL_COMPONENT_PADDING.get();

    let space = Size::new(
        (size.width - (check_side + check_padding)).max(0.),
        size.height,
    );

    let label_size = ctx.compute_size(label, SizeDef::fit(space), space.into());
    ctx.run_layout(label, label_size);

    let label_origin = Point::new(check_side + check_padding, 0.);
    ctx.place_child(label, label_origin);

    ctx.derive_baselines(label);
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

        let focus_rect = bbox.inflate(2.0, 2.0);

        let focus_color = p.border_color.color;
        let focus_width = 2.0;
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

        fn layout(&mut self, ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, size: Size) {
            super::selection_control::layout(&self.label, ctx, size);
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
    use crate::testing::TestHarness;
    use crate::theme::test_property_set;
    use crate::widgets::{Checkbox, CheckboxToggled, Switch, SwitchToggled};

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
    fn checkbox_and_switch_share_primary_and_keyboard_activation_rules() {
        exercise_activation(
            Checkbox::new(false, "Test").prepare(),
            CheckboxToggled(true),
        );
        exercise_activation(Switch::new(false).prepare(), SwitchToggled(true));
    }
}
