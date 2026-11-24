// Copyright 2019 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

use accesskit::{Node, Role};
use tracing::{Span, trace_span};

use crate::core::{
    AccessCtx, ArcStr, ChildrenIds, LayoutCtx, MeasureCtx, NewWidget, NoAction, PaintCtx,
    PropertiesMut, PropertiesRef, RegisterCtx, StyleProperty, Update, UpdateCtx, Widget, WidgetId,
    WidgetMut, WidgetPod,
};
use crate::imaging::Painter;
use crate::kurbo::{Axis, Point, Size};
use crate::layout::{LenReq, Length};
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
        Self {
            label: label.into_pod(),
            weight: AnimatedF32::stable(FontWeight::NORMAL.value()),
        }
    }

    /// Sets the initial font weight for this text.
    #[must_use = "Use the returned value."]
    pub fn with_initial_weight(mut self, weight: f32) -> Self {
        self.weight = AnimatedF32::stable(weight);
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
        this.widget.weight.move_to(target, over_millis);
        this.ctx.request_layout();
        this.ctx.request_anim_frame();
    }
}

// --- MARK: IMPL WIDGET
impl Widget for VariableLabel {
    type Action = NoAction;

    fn accepts_pointer_interaction(&self) -> bool {
        false
    }

    fn update(
        &mut self,
        _ctx: &mut UpdateCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        _event: &Update,
    ) {
    }

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        ctx.register_child(&mut self.label);
    }

    fn on_anim_frame(
        &mut self,
        ctx: &mut UpdateCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        interval: u64,
    ) {
        let result = self.weight.advance_nanos(interval);
        let new_weight = self.weight.value();
        // The ergonomics of child widgets are quite bad - ideally, this wouldn't need a mutate pass, since we
        // can set the required invalidation anyway.
        ctx.mutate_child_later(&self.label, move |mut label| {
            // TODO: Should this be configurable?
            if result.is_completed() {
                Label::set_hint(&mut label, true);
            } else {
                Label::set_hint(&mut label, false);
            }
            Label::insert_style(
                &mut label,
                StyleProperty::FontWeight(FontWeight::new(new_weight)),
            );
        });
        if !result.is_completed() {
            ctx.request_anim_frame();
        }
    }

    fn measure(
        &mut self,
        ctx: &mut MeasureCtx<'_>,
        _props: &PropertiesRef<'_>,
        axis: Axis,
        _len_req: LenReq,
        cross_length: Option<Length>,
    ) -> Length {
        ctx.redirect_measurement(&self.label, axis, cross_length)
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, size: Size) {
        ctx.run_layout(&self.label, size);
        ctx.place_child(&self.label, Point::ORIGIN);
        ctx.derive_baselines(&self.label);
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
        ChildrenIds::from_slice(&[self.label.id()])
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("VariableLabel", id = id.trace())
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    // TODO - Add tests
}
