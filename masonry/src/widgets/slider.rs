// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;

use accesskit::{ActionData, Node, Orientation, Role};
use include_doc_path::include_doc_path;
use masonry_core::debug_panic;
use tracing::{Span, trace_span};

use crate::core::keyboard::{Key, NamedKey};
use crate::core::pointer::PointerButton;
use crate::core::{
    AccessCtx, AccessEvent, ChildrenIds, EventCtx, LayoutCtx, MeasureCtx, PaintCtx,
    PointerButtonEvent, PointerEvent, PointerUpdate, PrePaintProps, PropertiesMut, PropertiesRef,
    Property, RegisterCtx, TextEvent, Update, UpdateCtx, UsesProperty, Widget, WidgetId, WidgetMut,
    paint_background, paint_box_shadow,
};
use crate::imaging::{Composite, GroupRef, Painter};
use crate::kurbo::{Axis, Circle, Point, Rect, Size, Stroke};
use crate::layout::{LenReq, Length};
use crate::peniko;
use crate::properties::{ThumbColor, ThumbRadius, TrackColor, TrackThickness};
use crate::theme;

/// A widget that allows a user to select a value from a continuous range.
///
#[doc = concat!(
    "![Slider](",
    include_doc_path!("screenshots/slider_initial_state.png"),
    ")",
)]
pub struct Slider {
    state: SliderState,
    drag: Option<SliderDrag>,
}

#[derive(Clone, Copy)]
struct SliderDrag {
    offset: f64,
    last_x: f64,
}

/// Validated slider configuration and selected value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderState {
    min: f64,
    max: f64,
    value: f64,
    step: Option<f64>,
}

/// Invalid slider configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InvalidSliderState {
    /// Bounds must be finite and ordered.
    Range {
        /// Requested lower bound.
        min: f64,
        /// Requested upper bound.
        max: f64,
    },
    /// The selected value must be finite.
    Value(f64),
    /// A step must be finite and strictly positive.
    Step(f64),
}

impl std::fmt::Display for InvalidSliderState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Range { min, max } => write!(
                f,
                "slider bounds must be finite and ordered, got {min}..={max}"
            ),
            Self::Value(value) => write!(f, "slider value must be finite, got {value}"),
            Self::Step(step) => write!(f, "slider step must be finite and positive, got {step}"),
        }
    }
}

impl std::error::Error for InvalidSliderState {}

impl SliderState {
    /// Constructs a validated state, clamping and quantizing the finite value.
    /// Steps are measured from the minimum; both endpoints are always selectable.
    ///
    /// # Errors
    ///
    /// Returns an error for non-finite values, non-finite or reversed bounds,
    /// or a non-positive/non-finite step.
    pub fn try_new(
        min: f64,
        max: f64,
        value: f64,
        step: Option<f64>,
    ) -> Result<Self, InvalidSliderState> {
        if !min.is_finite() || !max.is_finite() || min > max {
            return Err(InvalidSliderState::Range { min, max });
        }
        if !value.is_finite() {
            return Err(InvalidSliderState::Value(value));
        }
        if let Some(step) = step
            && (!step.is_finite() || step <= 0.0)
        {
            return Err(InvalidSliderState::Step(step));
        }
        let mut state = Self {
            min,
            max,
            value: min,
            step,
        };
        state.value = state.quantize(value);
        Ok(state)
    }

    /// Creates a state from known-valid application values.
    ///
    /// # Panics
    ///
    /// Invalid configurations panic with debug assertions enabled. In release
    /// mode they produce a fixed zero-valued slider. For externally supplied
    /// values use [`Self::try_new`] and handle validation errors.
    #[must_use = "Use the configured slider state."]
    pub fn new(min: f64, max: f64, value: f64, step: Option<f64>) -> Self {
        Self::try_new(min, max, value, step).unwrap_or_else(|error| {
            debug_panic!("{error}");
            Self {
                min: 0.0,
                max: 0.0,
                value: 0.0,
                step: None,
            }
        })
    }

    /// The current clamped, quantized value.
    #[must_use = "Inspect the selected value."]
    pub const fn value(self) -> f64 {
        self.value
    }

    fn span(self) -> f64 {
        self.max - self.min
    }

    fn progress(self) -> f64 {
        if self.span() == 0.0 {
            0.0
        } else if self.span().is_finite() {
            (self.value - self.min) / self.span()
        } else {
            // Halve before subtracting when finite opposite-sign bounds have
            // a difference larger than the greatest finite float.
            self.min.mul_add(-0.5, self.value * 0.5) / self.min.mul_add(-0.5, self.max * 0.5)
        }
    }

    fn at_progress(self, progress: f64) -> f64 {
        if progress <= 0.0 {
            self.min
        } else if progress >= 1.0 {
            self.max
        } else if self.span().is_finite() {
            progress.mul_add(self.span(), self.min)
        } else {
            self.min.mul_add(1.0 - progress, self.max * progress)
        }
    }

    // Beyond the exact integer range of f64, using an integer grid index can
    // lose the residue entirely. Reduce both operands modulo the step instead.
    fn grid_remainder(self, value: f64, step: f64) -> f64 {
        (value.rem_euclid(step) - self.min.rem_euclid(step)).rem_euclid(step)
    }

    // Round the displacement separately to preserve exact cancellation for
    // decimal UI steps (e.g. -1 + 1000 * 0.001). Use FMA only when the
    // intermediate displacement overflows but the final value may be finite.
    fn grid_at_index(self, index: f64, step: f64) -> f64 {
        let displacement = index * step;
        if displacement.is_finite() {
            self.min + displacement
        } else {
            index.mul_add(step, self.min)
        }
    }

    fn quantize(self, value: f64) -> f64 {
        let value = value.clamp(self.min, self.max);
        let Some(step) = self.step else {
            return value;
        };
        if value <= self.min || value >= self.max {
            return value;
        }
        let index = (value - self.min) / step;
        let grid = if index < EXACT_GRID_LIMIT {
            self.grid_at_index(index.round(), step)
        } else {
            let remainder = self.grid_remainder(value, step);
            if remainder <= step * 0.5 {
                value - remainder
            } else {
                value + (step - remainder)
            }
        }
        .clamp(self.min, self.max);
        if self.max - value <= (grid - value).abs() {
            self.max
        } else {
            grid
        }
    }

    fn update_value(&mut self, value: f64) -> bool {
        if !value.is_finite() {
            return false;
        }
        let value = self.quantize(value);
        if value.to_bits() == self.value.to_bits() {
            return false;
        }
        self.value = value;
        true
    }

    fn increment_candidate(self, count: f64) -> f64 {
        let step = self.step.unwrap_or_else(|| {
            if self.span().is_finite() {
                let increment = self.span() / 100.0;
                if increment > 0.0 {
                    increment
                } else {
                    self.span()
                }
            } else {
                self.max / 100.0 - self.min / 100.0
            }
        });
        if self.step.is_some() {
            let index = (self.value - self.min) / step;
            if index < EXACT_GRID_LIMIT {
                let index = if count < 0.0 && self.value >= self.max {
                    // The maximum is an extra endpoint if it is off-grid.
                    // Find the last grid point strictly below it.
                    let last = index.ceil() - 1.0;
                    let last = if self.grid_at_index(last, step) >= self.max {
                        last - 1.0
                    } else {
                        last
                    };
                    last + 1.0
                } else {
                    index.round()
                };
                return self
                    .grid_at_index(index + count, step)
                    .clamp(self.min, self.max);
            }
            if count < 0.0 && self.value >= self.max {
                let remainder = self.grid_remainder(self.value, step);
                let first = if remainder > 0.0 { remainder } else { step };
                return (count + 1.0)
                    .mul_add(step, self.value - first)
                    .clamp(self.min, self.max);
            }
        }
        count.mul_add(step, self.value).clamp(self.min, self.max)
    }

    fn increment(self, count: f64) -> f64 {
        let value = self.quantize(self.increment_candidate(count));
        if value.to_bits() == self.value.to_bits() {
            if count > 0.0 {
                self.quantize(self.value.next_up().min(self.max))
            } else {
                self.quantize(self.value.next_down().max(self.min))
            }
        } else {
            value
        }
    }
}

const EXACT_GRID_LIMIT: f64 = 4_503_599_627_370_496.0;

#[derive(Clone, Copy)]
struct SliderGeometry {
    size: Size,
    radius: f64,
}

impl SliderGeometry {
    fn new(size: Size, radius: Length) -> Self {
        Self {
            size,
            radius: radius.get().min(size.width.min(size.height) * 0.5),
        }
    }

    const fn travel(self) -> f64 {
        self.radius.mul_add(-2.0, self.size.width).max(0.0)
    }

    fn center(self, progress: f64) -> Point {
        Point::new(
            progress.mul_add(self.travel(), self.radius),
            self.size.height * 0.5,
        )
    }

    fn progress_at(self, x: f64) -> Option<f64> {
        let travel = self.travel();
        (travel > 0.0 && x.is_finite()).then(|| ((x - self.radius) / travel).clamp(0.0, 1.0))
    }
}

impl Slider {
    /// Creates a slider with a finite, ordered range and finite initial value.
    ///
    /// # Panics
    ///
    /// Panics for invalid configurations in debug builds; see [`SliderState::new`].
    #[must_use = "Use the returned value."]
    pub fn new(min: f64, max: f64, value: f64) -> Self {
        Self::from_state(SliderState::new(min, max, value, None))
    }

    /// Creates a slider from an already validated state.
    #[must_use = "Use the configured slider."]
    pub const fn from_state(state: SliderState) -> Self {
        Self { state, drag: None }
    }

    /// Configures the stepping interval, measured from the minimum.
    ///
    /// # Panics
    ///
    /// Panics in debug builds for an invalid step; see [`SliderState::new`].
    #[must_use = "Use the returned value."]
    pub fn with_step(mut self, step: f64) -> Self {
        self.state = SliderState::new(self.state.min, self.state.max, self.state.value, Some(step));
        self
    }

    /// The current slider value.
    #[must_use = "Inspect the selected value."]
    pub const fn value(&self) -> f64 {
        self.state.value()
    }

    fn geometry(ctx: &mut EventCtx<'_>, props: &PropertiesMut<'_>) -> SliderGeometry {
        let size = ctx.content_box().size();
        SliderGeometry::new(size, props.get::<ThumbRadius>(ctx.property_cache()).0)
    }

    fn emit_value(&mut self, ctx: &mut EventCtx<'_>, value: f64) {
        if self.state.update_value(value) {
            ctx.request_render();
            ctx.submit_action::<SliderMoved>(SliderMoved {
                value: self.state.value,
            });
        }
        ctx.set_handled();
    }

    fn drag_to(&mut self, ctx: &mut EventCtx<'_>, geometry: SliderGeometry, x: f64) {
        if let Some(drag) = &mut self.drag
            && drag.last_x.to_bits() != x.to_bits()
            && let Some(progress) = geometry.progress_at(x - drag.offset)
        {
            drag.last_x = x;
            self.emit_value(ctx, self.state.at_progress(progress));
        }
        ctx.set_handled();
    }

    /// Updates the selected value without emitting a user action.
    /// Non-finite values are ignored.
    pub fn set_value(this: &mut WidgetMut<'_, Self>, value: f64) {
        if this.widget.state.update_value(value) {
            this.ctx.request_render();
        }
    }

    /// Atomically updates the range, step and selected value.
    /// Model updates do not emit user actions.
    pub fn set_state(this: &mut WidgetMut<'_, Self>, state: SliderState) {
        if this.widget.state != state {
            this.widget.state = state;
            this.ctx.request_render();
        }
    }
}

impl UsesProperty<TrackThickness> for Slider {}
impl UsesProperty<TrackColor> for Slider {}
impl UsesProperty<ThumbColor> for Slider {}
impl UsesProperty<ThumbRadius> for Slider {}

/// A slider was moved.
#[derive(PartialEq, Debug)]
pub struct SliderMoved {
    /// The new value of the slider.
    pub value: f64,
}

// --- MARK: IMPL WIDGET
impl Widget for Slider {
    type Action = SliderMoved;

    fn accepts_focus(&self) -> bool {
        true
    }

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        match event {
            PointerEvent::Down(PointerButtonEvent {
                button: Some(PointerButton::Primary) | None,
                state,
                ..
            }) => {
                let position = ctx.local_position(state.position);
                let geometry = Self::geometry(ctx, props);
                let center = geometry.center(self.state.progress());
                let on_thumb = position.distance(center) <= geometry.radius;
                self.drag = Some(SliderDrag {
                    offset: if on_thumb { position.x - center.x } else { 0.0 },
                    last_x: position.x,
                });
                ctx.request_focus();
                ctx.capture_pointer();
                if !on_thumb && let Some(progress) = geometry.progress_at(position.x) {
                    self.emit_value(ctx, self.state.at_progress(progress));
                }
                ctx.set_handled();
            }
            PointerEvent::Move(PointerUpdate { current, .. })
                if ctx.is_pointer_capture_target() =>
            {
                let position = ctx.local_position(current.position);
                let geometry = Self::geometry(ctx, props);
                self.drag_to(ctx, geometry, position.x);
            }
            PointerEvent::Up(PointerButtonEvent {
                button: Some(PointerButton::Primary) | None,
                state,
                ..
            }) if ctx.is_pointer_capture_target() => {
                let position = ctx.local_position(state.position);
                let geometry = Self::geometry(ctx, props);
                self.drag_to(ctx, geometry, position.x);
                self.drag = None;
            }
            PointerEvent::Cancel(..) => self.drag = None,
            _ => {}
        }
    }

    fn on_text_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &TextEvent,
    ) {
        let TextEvent::Keyboard(event) = event else {
            return;
        };
        if !ctx.is_focus_target() || event.state.is_up() {
            return;
        }
        let count = if event.modifiers.shift() { 10.0 } else { 1.0 };
        let value = match &event.key {
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowDown) => self.state.increment(-count),
            Key::Named(NamedKey::ArrowRight | NamedKey::ArrowUp) => self.state.increment(count),
            Key::Named(NamedKey::PageDown) => self.state.increment(-10.0),
            Key::Named(NamedKey::PageUp) => self.state.increment(10.0),
            Key::Named(NamedKey::Home) => self.state.min,
            Key::Named(NamedKey::End) => self.state.max,
            _ => return,
        };
        self.emit_value(ctx, value);
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        match event {
            Update::DisabledChanged(true)
            | Update::StashedChanged(true)
            | Update::FocusChanged(false) => {
                self.drag = None;
                ctx.request_render();
            }
            Update::FocusChanged(true) | Update::HoveredChanged(_) | Update::ActiveChanged(_) => {
                ctx.request_render();
            }
            _ => {}
        }
    }

    fn on_access_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &AccessEvent,
    ) {
        if ctx.target() != ctx.widget_id() {
            return;
        }
        let value = match event.action {
            accesskit::Action::Increment => self.state.increment(1.0),
            accesskit::Action::Decrement => self.state.increment(-1.0),
            accesskit::Action::SetValue => match &event.data {
                Some(ActionData::NumericValue(value)) => *value,
                Some(ActionData::Value(value)) => {
                    let Ok(value) = value.parse() else {
                        return;
                    };
                    value
                }
                _ => return,
            },
            _ => return,
        };
        self.emit_value(ctx, value);
    }

    fn register_children(&mut self, _ctx: &mut RegisterCtx<'_>) {}

    fn property_changed(&mut self, ctx: &mut UpdateCtx<'_>, property_type: TypeId) {
        TrackThickness::prop_changed(ctx, property_type);
        ThumbColor::prop_changed(ctx, property_type);
        ThumbRadius::prop_changed(ctx, property_type);
        if TrackColor::matches(property_type) {
            ctx.request_paint_only();
        }
    }

    fn measure(
        &mut self,
        ctx: &mut MeasureCtx<'_>,
        props: &PropertiesRef<'_>,
        axis: Axis,
        len_req: LenReq,
        _cross_length: Option<Length>,
    ) -> Length {
        match axis {
            Axis::Horizontal => match len_req {
                // TODO: Move this 100. to theme?
                LenReq::Min | LenReq::Max => crate::layout::length!(100.),
                LenReq::Fit(space) => space,
            },
            Axis::Vertical => {
                let cache = ctx.property_cache();
                let thumb_radius = props.get::<ThumbRadius>(cache);
                let track_thickness = props.get::<TrackThickness>(cache);

                let thumb_length = thumb_radius.0.saturating_add(thumb_radius.0);
                let track_length = track_thickness.0;
                let padding_length = theme::WIDGET_CONTROL_COMPONENT_PADDING;

                thumb_length
                    .max(track_length)
                    .saturating_add(padding_length)
            }
        }
    }

    fn layout(&mut self, _ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, _size: Size) {}

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

        if ctx.is_focus_target() || ctx.is_hovered() {
            // TODO: Replace this custom implementation with the general paint_border()

            let focus_rect = bbox.inset(2.);

            let focus_color = p.border_color.color;
            let focus_width = 2.;
            let focus_radius = 4.;

            let focus_path = focus_rect.to_rounded_rect(focus_radius);
            let focus_stroke = Stroke::new(focus_width).with_miter_limit(10.);

            painter
                .stroke(focus_path, &focus_stroke, focus_color)
                .draw();
        }
    }

    fn paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        // Get parameters and resolve colors

        let cache = ctx.property_cache();
        let track_color = props.get::<TrackColor>(cache);
        let thumb_color = props.get::<ThumbColor>(cache).0;
        let track_thickness = props.get::<TrackThickness>(cache).0.get();
        let thumb_radius = props.get::<ThumbRadius>(cache).0;
        let thumb_border_width: f64 = if ctx.is_active() {
            2.5
        } else if ctx.is_hovered() {
            1.5
        } else {
            2.
        };

        // Calculate geometry based on state
        let size = ctx.content_box().size();
        if size.width <= 0.0 || size.height <= 0.0 {
            return;
        }
        let geometry = SliderGeometry::new(size, thumb_radius);
        let track_thickness = track_thickness.min(size.height);
        let track_y = (size.height - track_thickness) / 2.0;
        let border_box = ctx.border_box();

        // TODO: replace with proper disabled colors
        // Push semitransparent layer if disabled
        if ctx.is_disabled() {
            const DISABLED_ALPHA: f32 = 0.4;
            // Paint through a semitransparent isolated group when disabled.
            painter.push_fill_clip(border_box);
            painter.push_group(
                GroupRef::new()
                    .with_composite(Composite::new(peniko::BlendMode::default(), DISABLED_ALPHA)),
            );
        }

        let center = geometry.center(self.state.progress());
        if track_thickness > 0.0 {
            let track_rect = Rect::new(0., track_y, size.width, track_y + track_thickness)
                .to_rounded_rect(track_thickness / 2.);
            let fraction = (center.x / size.width).clamp(0.0, 1.0) as f32;
            let gradient = peniko::Gradient::new_linear((0., 0.), (size.width, 0.)).with_stops([
                (0., track_color.active),
                (fraction, track_color.active),
                (fraction, track_color.inactive),
                (1., track_color.inactive),
            ]);
            painter.fill(track_rect, &gradient).draw();
        }

        if geometry.radius > 0.0 {
            let border_width = thumb_border_width.min(geometry.radius);
            let thumb_circle = Circle::new(center, geometry.radius - border_width / 2.);
            painter.fill(thumb_circle, thumb_color).draw();
            painter
                .stroke(thumb_circle, &Stroke::new(border_width), track_color.active)
                .draw();
        }

        // Pop the semitransparent layer
        if ctx.is_disabled() {
            painter.pop_group();
            painter.pop_clip();
        }
    }

    fn accessibility_role(&self) -> Role {
        Role::Slider
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        node.set_orientation(Orientation::Horizontal);
        node.set_value(self.state.value.to_string());
        node.set_numeric_value(self.state.value);
        node.set_min_numeric_value(self.state.min);
        node.set_max_numeric_value(self.state.max);
        if let Some(step) = self.state.step {
            node.set_numeric_value_step(step);
        }
        if self.state.min < self.state.max {
            node.add_action(accesskit::Action::SetValue);
        }
        if self.state.value < self.state.max {
            node.add_action(accesskit::Action::Increment);
        }
        if self.state.value > self.state.min {
            node.add_action(accesskit::Action::Decrement);
        }
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::new()
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("Slider", id = id.trace())
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::TextEvent;
    use crate::kurbo::Point;
    use crate::testing::{TestHarness, assert_render_snapshot};
    use crate::theme::test_property_set;

    #[test]
    fn state_validation_and_endpoint_quantization() {
        assert_eq!(
            SliderState::try_new(2.0, 1.0, 1.0, None),
            Err(InvalidSliderState::Range { min: 2.0, max: 1.0 })
        );
        assert!(
            matches!(SliderState::try_new(0.0, 1.0, f64::NAN, None), Err(InvalidSliderState::Value(value)) if value.is_nan())
        );
        for step in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            assert!(
                matches!(SliderState::try_new(0.0, 1.0, 0.0, Some(step)), Err(InvalidSliderState::Step(invalid)) if invalid.to_bits() == step.to_bits())
            );
        }
        let mut state = SliderState::try_new(3.0, 13.0, 8.0, Some(8.0)).unwrap();
        assert_eq!(state.value(), 11.0, "grid is anchored to the minimum");
        assert!(state.update_value(state.increment(1.0)));
        assert_eq!(state.value(), 13.0, "maximum remains selectable off-grid");
        assert!(state.update_value(state.increment(-1.0)));
        assert_eq!(
            state.value(),
            11.0,
            "decrement from a non-grid maximum selects the preceding grid point"
        );
        assert!(
            !state.update_value(10.0),
            "quantizing to the same value is not a change"
        );
        assert!(
            !state.update_value(f64::NAN),
            "invalid input cannot corrupt the value"
        );
        assert!(state.update_value(state.min));
        assert_eq!(state.value(), 3.0, "minimum remains selectable");
    }

    #[test]
    fn extreme_finite_ranges_and_small_steps_remain_finite() {
        let mut state = SliderState::new(-f64::MAX, f64::MAX, 0.0, None);
        assert_eq!(
            state.progress(),
            0.5,
            "opposite-sign extremes must not overflow progress"
        );
        for progress in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let value = state.at_progress(progress);
            assert!(
                value.is_finite(),
                "interpolation at {progress} must be finite"
            );
            state.update_value(value);
            assert!((state.progress() - progress).abs() < 1e-12);
        }
        let tiny = f64::from_bits(1);
        let mut state = SliderState::new(0.0, 8.0 * tiny, 0.0, Some(tiny));
        for index in 1..=8 {
            assert!(
                state.update_value(state.increment(1.0)),
                "subnormal steps must not be suppressed"
            );
            assert_eq!(state.value(), f64::from_bits(index));
        }
        let fixed = SliderState::new(5.0, 5.0, 99.0, None);
        assert_eq!(fixed.progress(), 0.0);
        assert_eq!(fixed.increment(1.0), 5.0);
    }

    #[test]
    fn decimal_grid_preserves_zero_and_sub_ulp_steps_can_advance() {
        let state = SliderState::new(-1.0, 1.0, 0.0, Some(0.001));
        assert_eq!(
            state.value().to_bits(),
            0.0_f64.to_bits(),
            "decimal grid retains exact zero crossing"
        );
        let mut state = SliderState::new(1e20, 1e20 + 1e6, 1e20, Some(1.0));
        let next = state.increment(1.0);
        assert!(
            state.update_value(next),
            "a step smaller than the local ULP must still advance to the next representable value"
        );
        assert!(state.value() > 1e20, "the selected value must advance");
        assert!(
            state.update_value(state.increment(-1.0)),
            "sub-ULP steps can also move backward"
        );
        assert_eq!(
            state.value().to_bits(),
            1e20_f64.to_bits(),
            "decrement returns to the minimum"
        );
    }

    #[test]
    fn model_changes_refresh_accessibility_without_emitting_actions() {
        let widget = Slider::new(0.0, 100.0, 25.0).prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (200, 32));
        let id = harness.root_id();
        let _ = harness.redraw();
        harness
            .edit_root_widget(|mut slider| {
                Slider::set_state(&mut slider, SliderState::new(10.0, 30.0, 25.0, Some(5.0)));
            })
            .unwrap();
        let _ = harness.redraw();
        let node = harness.access_node(id).unwrap();
        assert_eq!(node.data().min_numeric_value(), Some(10.0));
        assert_eq!(node.data().max_numeric_value(), Some(30.0));
        assert_eq!(node.data().numeric_value_step(), Some(5.0));
        assert_eq!(node.data().numeric_value(), Some(25.0));
        assert!(
            harness.pop_action_erased().is_none(),
            "model changes do not submit actions"
        );
        harness.focus_on(Some(id));
        harness.process_text_event(&TextEvent::key_down(Key::Named(NamedKey::End)));
        assert_eq!(
            harness.pop_action::<SliderMoved>().unwrap(),
            Some((SliderMoved { value: 30.0 }, id))
        );
        harness.process_text_event(&TextEvent::key_down(Key::Named(NamedKey::End)));
        assert!(
            harness.pop_action_erased().is_none(),
            "repeated endpoint activation is a no-op"
        );
        let _ = harness.redraw();
        assert!(
            !harness
                .access_node(id)
                .unwrap()
                .data()
                .supports_action(accesskit::Action::Increment)
        );
    }

    #[test]
    fn thumb_geometry_is_invertible_and_fits_compact_allocations() {
        for size in [
            Size::new(200.0, 32.0),
            Size::new(5.0, 3.0),
            Size::new(1.0, 1.0),
            Size::ZERO,
        ] {
            let geometry = SliderGeometry::new(size, Length::px(7.0));
            for progress in [0.0, 0.25, 0.5, 1.0] {
                let center = geometry.center(progress);
                assert!(center.x >= geometry.radius && center.x <= size.width - geometry.radius);
                assert!(center.y >= geometry.radius && center.y <= size.height - geometry.radius);
                if geometry.travel() > 0.0 {
                    assert!((geometry.progress_at(center.x).unwrap() - progress).abs() < 1e-12);
                } else {
                    assert!(geometry.progress_at(center.x).is_none());
                }
            }
        }
        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            Slider::new(5.0, 5.0, 5.0).prepare(),
            (2, 2),
        );
        let _ = harness.redraw();
        assert_eq!(harness.root_widget().unwrap().value(), 5.0);
    }

    #[test]
    fn slider_initial_state() {
        let widget = Slider::new(0.0, 100.0, 25.0).prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (200, 32));

        assert_render_snapshot!(harness, "slider_initial_state");
    }

    #[test]
    fn slider_drag_interaction() {
        let widget = Slider::new(0.0, 100.0, 25.0).prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (200, 32));
        let slider_id = harness.root_id();

        assert_render_snapshot!(harness, "slider_drag_initial_at_25");

        // 1. Move the mouse to the thumb position (25%) BEFORE clicking.
        harness.mouse_move(Point::new(53.5, 16.0));

        // 2. Press the mouse button.
        // This should not emit an action because the value does not change.
        harness.mouse_button_press(None);
        assert!(harness.pop_action::<SliderMoved>().unwrap().is_none());

        // 3. Move to the new position (75%).
        harness.mouse_move(Point::new(146.5, 16.0));

        assert_eq!(
            harness.pop_action::<SliderMoved>().unwrap(),
            Some((SliderMoved { value: 75.0 }, slider_id))
        );
        assert_render_snapshot!(harness, "slider_drag_to_75");

        // Release the mouse
        harness.mouse_button_release(None);
        assert_render_snapshot!(harness, "slider_drag_released_at_75");
    }

    #[test]
    fn slider_keyboard_interaction() {
        let widget = Slider::new(0.0, 100.0, 50.0).with_step(10.0).prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (200, 32));
        let slider_id = harness.root_id();

        harness.focus_on(Some(slider_id));
        assert_render_snapshot!(harness, "slider_keyboard_focused");

        harness.process_text_event(&TextEvent::key_down(Key::Named(NamedKey::ArrowRight)));
        harness.process_text_event(&TextEvent::key_up(Key::Named(NamedKey::ArrowRight)));

        assert_eq!(
            harness.pop_action::<SliderMoved>().unwrap(),
            Some((SliderMoved { value: 60.0 }, slider_id))
        );
        assert_render_snapshot!(harness, "slider_keyboard_moved");
    }

    #[test]
    fn slider_disabled_state() {
        let mut widget = Slider::new(0.0, 100.0, 50.0).prepare();
        widget.options.disabled = true;
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (200, 32));

        assert_render_snapshot!(harness, "slider_disabled");
        assert!(harness.pop_action::<SliderMoved>().unwrap().is_none());
    }
}
