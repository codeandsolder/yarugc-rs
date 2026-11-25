// Copyright 2019 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

use accesskit::Role;
use tracing::{Span, trace_span};

use crate::core::{
    ArcStr, NewWidget, NoAction, PropertiesMut, StyleProperty, Update, UpdateCtx, Widget, WidgetId,
    WidgetMut, WidgetPod,
};
use crate::parley::style::FontWeight;
use crate::widgets::Label;

/// An `f32` value which can move towards a target value at a linear rate over time.
#[derive(Clone, Debug)]
pub struct AnimatedF32 {
    target: f32,
    // Keep interpolation in f64 to avoid rounding stalling small advances.
    current: f64,
    remaining_millis: f64,
}

impl AnimatedF32 {
    /// Creates a stable value. Non-finite initial values are replaced with zero.
    #[must_use = "Retain the animated value."]
    pub const fn stable(value: f32) -> Self {
        let value = if value.is_finite() { value } else { 0. };
        Self {
            target: value,
            current: value as f64,
            remaining_millis: 0.,
        }
    }

    /// Retargets the animation. Non-finite targets or delays leave it unchanged.
    /// Nonpositive finite delays complete immediately.
    pub fn move_to(&mut self, target: f32, over_millis: f32) {
        if !target.is_finite() || !over_millis.is_finite() {
            tracing::warn!(target, over_millis, "Ignoring invalid animation parameters");
            return;
        }
        self.target = target;
        self.remaining_millis = f64::from(over_millis.max(0.));
        if self.remaining_millis == 0. || self.current.to_bits() == f64::from(target).to_bits() {
            self.current = f64::from(target);
            self.remaining_millis = 0.;
        }
    }

    /// Advances by elapsed milliseconds. Invalid or negative elapsed times are ignored.
    pub fn advance(&mut self, by_millis: f64) -> AnimationStatus {
        if self.remaining_millis <= 0. {
            return AnimationStatus::Completed;
        }
        if !by_millis.is_finite() || by_millis <= 0. {
            return AnimationStatus::Ongoing;
        }
        if by_millis >= self.remaining_millis {
            self.current = f64::from(self.target);
            self.remaining_millis = 0.;
            return AnimationStatus::Completed;
        }
        let fraction = by_millis / self.remaining_millis;
        self.current = (f64::from(self.target) - self.current).mul_add(fraction, self.current);
        self.remaining_millis -= by_millis;
        AnimationStatus::Ongoing
    }

    /// Advances the animation by a frame interval in nanoseconds.
    pub fn advance_nanos(&mut self, nanos: u64) -> AnimationStatus {
        self.advance(crate::util::Duration::from_nanos(nanos).as_secs_f64() * 1000.)
    }

    /// Returns the target value.
    #[must_use = "Use the target value."]
    pub const fn target(&self) -> f32 {
        self.target
    }

    /// Returns the current value.
    #[must_use = "Use the interpolated value."]
    pub const fn value(&self) -> f32 {
        self.current as f32
    }
}

#[cfg(test)]
mod animation_tests {
    use super::*;

    #[test]
    fn retarget_and_complete_without_old_rate_or_overflow() {
        let mut anim = AnimatedF32::stable(-f32::MAX);
        anim.move_to(f32::MAX, 100.);
        assert_eq!(anim.advance(50.), AnimationStatus::Ongoing);
        assert_eq!(anim.value(), 0.);
        anim.move_to(2., 0.);
        assert_eq!(anim.advance(1.), AnimationStatus::Completed);
        assert_eq!(anim.value(), 2.);
        anim.move_to(10., 100.);
        assert_eq!(anim.advance(-5.), AnimationStatus::Ongoing);
        assert_eq!(anim.value(), 2.);
        anim.advance(50.);
        assert_eq!(anim.value(), 6.);
        anim.move_to(4., 50.);
        assert_eq!(anim.advance(50.), AnimationStatus::Completed);
        assert_eq!(anim.value(), 4.);
    }
}

/// The status an animation can be in.
///
/// Generally returned when an animation is advanced, to determine whether.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AnimationStatus {
    /// The animation has finished.
    Completed,
    /// The animation is still running
    Ongoing,
}

impl AnimationStatus {
    /// Return true if animation has finished.
    #[must_use = "Use the returned value."]
    pub const fn is_completed(self) -> bool {
        matches!(self, Self::Completed)
    }
}

/// A widget displaying non-editable text, with a variable [weight](crate::parley::style::FontWeight).
///
/// Ensure that `VariableLabel` has [`Dimensions`] set via props
/// either to [`Dimensions::fixed`] or [`Dimensions::MAX`].
/// Fixed dimensions resolve early and are explicit in intent.
/// Max preferred size of `VariableLabel` means that the question of size
/// will get passed through to its inner label, and doesn't mean that it will
/// necessarily map to the max preferred size of the label.
///
/// [`Dimensions`]: crate::properties::Dimensions
/// [`Dimensions::fixed`]: crate::properties::Dimensions::fixed
/// [`Dimensions::MAX`]: crate::properties::Dimensions::MAX
pub struct VariableLabel {
    label: WidgetPod<Label>,
    weight: AnimatedF32,
    hint_before_animation: Option<bool>,
}

// --- MARK: BUILDERS
impl VariableLabel {
    /// Creates a new variable label from the given text.
    pub fn new(text: impl Into<ArcStr>) -> Self {
        Self::from_label(NewWidget::new(Label::new(text)))
    }

    /// Creates a new variable label from the given label.
    ///
    /// Uses the label's text and style values.
    #[must_use = "Use the returned value."]
    pub fn from_label(label: NewWidget<Label>) -> Self {
        let weight = AnimatedF32::stable(label.widget.font_weight().value());
        Self {
            label: label.into_pod(),
            weight,
            hint_before_animation: None,
        }
    }

    /// Sets the initial font weight for this text.
    #[must_use = "Use the returned value."]
    pub const fn with_initial_weight(mut self, weight: f32) -> Self {
        if weight.is_finite() {
            self.weight = AnimatedF32::stable(weight);
        }
        self
    }
}

// --- MARK: WIDGETMUT
impl VariableLabel {
    /// Returns the underlying label for this widget.
    pub fn label_mut<'t>(this: &'t mut WidgetMut<'_, Self>) -> Option<WidgetMut<'t, Label>> {
        this.ctx.get_mut(this.widget.label.id())
    }

    /// Sets the text of this label.
    pub fn set_text(this: &mut WidgetMut<'_, Self>, new_text: impl Into<ArcStr>) {
        if let Some(mut child) = Self::label_mut(this) {
            Label::set_text(&mut child, new_text);
        }
    }

    /// Sets the weight which this font will target.
    pub fn set_target_weight(this: &mut WidgetMut<'_, Self>, target: f32, over_millis: f32) {
        if !target.is_finite() || !over_millis.is_finite() {
            tracing::warn!(target, over_millis, "Ignoring invalid animation parameters");
            return;
        }
        let state = &mut *this.widget;
        let Some(mut label) = this.ctx.get_mut::<Label>(state.label.id()) else {
            return;
        };
        // A caller may have edited the wrapped label while no animation was running.
        if state.hint_before_animation.is_none() {
            state.weight = AnimatedF32::stable(label.widget.font_weight().value());
        }
        state.weight.move_to(target, over_millis);
        let running = state.weight.remaining_millis > 0.;
        if running {
            if state.hint_before_animation.is_none() {
                state.hint_before_animation = Some(label.widget.hint());
            }
            Label::set_hint(&mut label, false);
        } else {
            Self::apply_weight(&mut label, state.weight.value());
            if let Some(hint) = state.hint_before_animation.take() {
                Label::set_hint(&mut label, hint);
            }
        }
        drop(label);
        if running {
            this.ctx.request_anim_frame();
        }
    }

    fn apply_weight(label: &mut WidgetMut<'_, Label>, value: f32) {
        let weight = FontWeight::new(value);
        if label.widget.font_weight() != weight {
            let _ = Label::insert_style(label, StyleProperty::FontWeight(weight));
        }
    }
}

// --- MARK: IMPL WIDGET
impl Widget for VariableLabel {
    type Action = NoAction;

    fn accepts_pointer_interaction(&self) -> bool {
        false
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        if matches!(event, Update::WidgetAdded) {
            let weight = self.weight.value();
            ctx.mutate_child_later(&self.label, move |mut label| {
                Self::apply_weight(&mut label, weight);
            });
        }
    }

    masonry_core::single_child_container!(label, fill);

    fn on_anim_frame(
        &mut self,
        ctx: &mut UpdateCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        interval: u64,
    ) {
        if self.hint_before_animation.is_none() {
            return;
        }
        let result = self.weight.advance_nanos(interval);
        let weight = self.weight.value();
        let hint = if result.is_completed() {
            self.hint_before_animation.take()
        } else {
            ctx.request_anim_frame();
            None
        };
        ctx.mutate_child_later(&self.label, move |mut label| {
            Self::apply_weight(&mut label, weight);
            if let Some(hint) = hint {
                Label::set_hint(&mut label, hint);
            }
        });
    }

    fn accessibility_role(&self) -> Role {
        Role::GenericContainer
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("VariableLabel", id = id.trace())
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestHarness;
    use crate::theme::test_property_set;

    #[test]
    fn initial_weight_and_animation_update_the_actual_label() {
        let child = Label::new("variable")
            .with_style(StyleProperty::FontWeight(FontWeight::new(500.)))
            .with_hint(false)
            .prepare();
        let child_id = child.id();
        let widget = VariableLabel::from_label(child).with_initial_weight(600.);
        let mut harness =
            TestHarness::create_with_size(test_property_set(), widget.prepare(), (200, 40));
        let check = |harness: &TestHarness<VariableLabel>, weight, hint| {
            let child = harness.get_widget_with_id(child_id).unwrap();
            let child = child.downcast::<Label>().unwrap();
            assert_eq!(child.font_weight(), FontWeight::new(weight));
            assert_eq!(child.hint(), hint);
        };
        check(&harness, 600., false);
        harness
            .edit_root_widget(|mut widget| {
                VariableLabel::set_target_weight(&mut widget, 800., 100.);
            })
            .unwrap();
        harness.animate_ms(50);
        check(&harness, 700., false);
        harness
            .edit_root_widget(|mut widget| {
                VariableLabel::set_target_weight(&mut widget, 350., 0.);
            })
            .unwrap();
        check(&harness, 350., false);
        harness.animate_ms(100);
        check(&harness, 350., false);
    }

    #[test]
    fn animation_starts_from_wrapped_style_and_restores_hinting() {
        let child = Label::new("variable")
            .with_style(StyleProperty::FontWeight(FontWeight::new(500.)))
            .prepare();
        let child_id = child.id();
        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            VariableLabel::from_label(child).prepare(),
            (200, 40),
        );
        harness
            .edit_root_widget(|mut widget| {
                VariableLabel::set_target_weight(&mut widget, 700., 100.);
            })
            .unwrap();
        harness.animate_ms(50);
        let child = harness.get_widget_with_id(child_id).unwrap();
        let child = child.downcast::<Label>().unwrap();
        assert_eq!(child.font_weight(), FontWeight::new(600.));
        assert!(!child.hint());
        harness.animate_ms(50);
        let child = harness.get_widget_with_id(child_id).unwrap();
        let child = child.downcast::<Label>().unwrap();
        assert_eq!(child.font_weight(), FontWeight::new(700.));
        assert!(child.hint());
    }
}
