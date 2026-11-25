// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;
use std::fmt::{Debug, Display};

use accesskit::{Action, Node, Role};
use masonry_core::anymore::AnyDebug;
use masonry_core::debug_panic;

use crate::core::keyboard::{Code, Key, NamedKey};
use crate::core::{
    AccessCtx, AccessEvent, ChildrenIds, CursorIcon, EventCtx, LayoutCtx, MeasureCtx, PaintCtx,
    PointerButton, PointerEvent, PointerState, PropertiesMut, PropertiesRef, Property, QueryCtx,
    RegisterCtx, TextEvent, Update, UpdateCtx, Widget, WidgetMut, WidgetPod,
};
use crate::imaging::Painter;
use crate::kurbo::{Affine, Axis, BezPath, Cap, Line, Point, Size, Stroke};
use crate::layout::{AsUnit, LayoutSize, LenReq, Length, SizeDef};
use crate::peniko::Gradient;
use crate::properties::{BackwardColor, ContentColor, ForwardColor, HeatColor, StepInputStyle};
use crate::theme;
use crate::widgets::Label;
use crate::widgets::numeric_grid::{EXACT_GRID_LIMIT, step_sum};

/// How much accidental sliding is allowed to still consider it a simple click.
///
/// 3 logical pixels is great for mouse and trackpad,
/// but a larger value should be used for touch.
const SLIDE_INACTIVE_LENGTH: Length = crate::layout::length!(3.);

// We have three gears with increasing ratios.
const GEAR_1_RATIO: f64 = 1.;
const GEAR_2_RATIO: f64 = 10.;
const GEAR_3_RATIO: f64 = 100.;

// The active gear is chosen based on the distance traveled
const GEAR_1_END: f64 = 100.; // 0 .. 100px
const GEAR_2_END: f64 = 200.; // 100px .. 200px
// Gear 3 is 200px .. infinity

// Calculate total steps at the end of the lower gears
const GEAR_1_END_STEPS: f64 = GEAR_1_RATIO * GEAR_1_END;
const GEAR_2_END_STEPS: f64 = GEAR_2_RATIO.mul_add(GEAR_2_END - GEAR_1_END, GEAR_1_END_STEPS);

/// Defines how many ticks there are per a single step.
///
/// A value of `10` allows for 0.1x speed sliding to still register in ticks.
const TICKS_PER_STEP: i64 = 10;

/// An input widget that steps through values.
///
/// It has increment/decrement buttons for single step movements.
/// For larger changes there is feature-rich pointer control.
/// Click and drag the widget and the value will start changing based on the distance moved.
///
/// There are three different speed zones based on distance:
/// 1. 0 .. 100px moved = 1x speed
/// 2. 100px .. 200px moved = 10x speed
/// 3. 200px .. = 100x speed
///
/// There are two modifier keys to adjust the speed:
/// * Hold `Shift` to increase the speed by 10x
/// * Hold `Alt` to decrease the speed by 10x
///
/// Hold `Ctrl`/`Cmd` to enter into snap mode where the active value
/// will be a multiple of the configured snap value.
///
/// There will be a [`Step`] action when the active value changes due to user input.
///
/// Anything that implements [`Steppable`] can be stepped through.
/// It is already implemented for signed and unsigned integers and floats.
/// For many custom scenarios you can just use an already supported number type
/// and supplement it with a custom display function via [`with_display`].
///
/// [`with_display`]: Self::with_display
pub struct StepInput<T> {
    /// The base value that we're stepping from.
    ///
    /// This is set to the original input value when the widget is initialized,
    /// and is also updated whenever an explicit value update happens.
    /// Ordinary stepping never changes the base, avoiding error accumulation.
    /// Explicit snapping, wrapping and configuration changes can rebase it.
    base: T,
    /// The step size.
    step: T,
    /// The snap interval.
    snap: Option<T>,
    /// The minimum value that can be stepped to.
    min: T,
    /// The maximum value that can be stepped to.
    max: T,

    /// The currently active value.
    value: T,

    /// Ticks determine the active value when combined with the `base` value.
    ///
    /// [`TICKS_PER_STEP`] defines how many ticks are needed for a single `step`.
    ///
    /// While `i64` may not be enough to represent the whole distance from `base` to `min`/`max`,
    /// it is more than enough for UI purposes. Given a person with a ridiculously high
    /// pointer sensitivity that can move 10,000 logical pixels per arm swing. Plus an effective
    /// gear ratio of 10,000,000 ticks per pixel. It would still take that person 3 years
    /// of 24/7 swinging of the arm at a pace of once per second to reach the 63 bit limit.
    ticks: i64,
    /// The displayed value is a projection of the raw tick position onto the snap grid.
    snap_active: bool,
    /// Number of ticks that achieves the `min` value.
    ///
    /// Saturated at `i64::MIN` if reaching `min` from `base` actually requires more.
    min_ticks: i64,
    /// Number of ticks that achieves the `max` value.
    ///
    /// Saturated at `i64::MAX` if reaching `max` from `base` actually requires more.
    max_ticks: i64,

    /// Whether the values should form an infinite circle,
    /// where increasing above `max` results in `min` and
    /// decreasing below `min` results in `max`.
    wrap: bool,

    /// The label displaying the current active value.
    label: Option<WidgetPod<Label>>,
    /// Custom display function override.
    display: Option<DisplayFn<T>>,

    /// The pointer position where the button was pressed down.
    drag_start: Option<Point>,
    /// The pointer position where ticks were last updated.
    slide_last: Option<Point>,
    /// Whether slower slide mode is enabled.
    gearing: SlideGearing,

    /// When hovered, `true` means the backward side and `false` the forward side.
    hover_backward: bool,

    /// Cached label layout x origin.
    label_x_start: f64,
    /// Cached label layout x endpoint.
    label_x_end: f64,
}

#[derive(Default)]
struct SlideGearing {
    slower: bool,
    faster: bool,
}

#[expect(
    clippy::cast_precision_loss,
    reason = "Floating-point step and pointer math necessarily rounds large tick counts"
)]
const fn ticks_as_float(ticks: i64) -> f64 {
    ticks as f64
}

#[expect(
    clippy::cast_sign_loss,
    reason = "The saturating conversion follows a positive count check"
)]
fn nonnegative_steps(value: f64) -> u64 {
    if value.is_nan() || value <= 0. {
        0
    } else {
        value.ceil() as u64
    }
}

// Float Display uses the shortest round-tripping decimal representation in
// fixed notation. Unlike log10, this retains all digits in steps such as
// 0.25 or 2.5, without widening f32 into a longer f64 decimal.
fn decimal_precision(step: impl Display) -> usize {
    step.to_string()
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len())
}

#[derive(Clone, Copy)]
struct SpeedLineGeometry {
    size: Size,
    speed: f64,
    arrow_width: f64,
    arrow_height: f64,
    arrow_edge_pad: f64,
    arrow_offset_active: f64,
    arrow_base_x: f64,
    extra_y_pad: f64,
    backward: bool,
    color: crate::peniko::Color,
}

/// The [`StepInput`] context.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd)]
pub struct StepState<T> {
    /// The currently active value.
    pub value: T,
    /// The step size.
    pub step: T,
    /// The minimum value that can be stepped to.
    pub min: T,
    /// The maximum value that can be stepped to.
    pub max: T,
}

/// Returns the string representation of this value.
///
/// Takes a [`StepState`] and the returned value will be used for display purposes.
pub type DisplayFn<T> = Box<dyn Fn(StepState<T>) -> String>;

/// Data that can be stepped through in a [`StepInput`].
pub trait Steppable: Debug + Copy + Clone + PartialEq + PartialOrd + Send + Sync + 'static {
    /// Whether this value belongs to the valid ordered numeric domain.
    /// Floating implementations must reject NaN and infinities.
    fn is_valid(self) -> bool;

    /// The additive identity of the numeric domain.
    fn zero() -> Self;

    /// Returns `self` with the given number of steps added.
    ///
    /// The addition needs to be saturating and not overflow. The receiver,
    /// step and snap must be valid, with positive step and snap intervals.
    ///
    /// This method is always called on the base value,
    /// so there is no risk of cumulative addings causing errors.
    ///
    /// If `snap` is present then the returned value should be snapped to it.
    /// Which is to say, `result % snap` should be zero. With the exception
    /// of min/max boundaries of the data type being reached.
    #[must_use = "Use the configured value."]
    fn add_steps(&self, step: Self, count: i64, snap: Option<Self>) -> Self;

    /// Returns the next representable snap-grid value in the given direction.
    ///
    /// The interval must be valid and positive. At a numeric domain boundary
    /// the result saturates rather than wrapping or becoming non-finite.
    #[must_use = "Use the next snapped value."]
    fn adjacent_snap(&self, snap: Self, forward: bool) -> Self;

    /// Returns the number of steps required to reach `target` from `self`.
    ///
    /// The `target` can be safely assumed to always be equal or greater than `self`.
    ///
    /// The number of steps needs to reach `target` exactly or go beyond it.
    /// For example if `self` is `5`, `target` is `17`, and `step` is `5`,
    /// then the correct result is `3`.
    ///
    /// If the required steps to reach `target` doesn't fit in `u64`,
    /// then `u64::MAX` must be returned. Do not overflow to `u64::MIN`.
    fn steps_to(&self, target: Self, step: Self) -> u64;

    /// Returns the minimum, greater than zero, step value.
    ///
    /// This is used to validate that both the step and snap are greater than zero.
    /// It is used as the fallback value when a value less than this is encountered.
    fn min_step() -> Self;

    /// Returns the string representation of this value.
    ///
    /// The returned value will be used for display purposes.
    ///
    /// The `min`, `max`, `step` values are available as optional hints.
    /// For example, the default `f64` implementation determines
    /// how many decimal places to show based on the step size.
    fn display(&self, step: Self, min: Self, max: Self) -> String;
}

// --- MARK: IMPL STEPPABLE

/// Allows calling `saturating_add_unsigned` even on unsigned types.
///
/// For example `saturating_add_unsigned!(5u8, 1u8, u) == 6`.
macro_rules! saturating_add_unsigned {
    ($value:expr, $delta:expr, i) => {
        ($value).saturating_add_unsigned($delta)
    };
    ($value:expr, $delta:expr, u) => {
        ($value).saturating_add($delta)
    };
}

/// Allows calling `saturating_sub_unsigned` even on unsigned types.
///
/// For example `saturating_sub_unsigned!(6u8, 1u8, u) == 4`.
macro_rules! saturating_sub_unsigned {
    ($value:expr, $delta:expr, i) => {
        ($value).saturating_sub_unsigned($delta)
    };
    ($value:expr, $delta:expr, u) => {
        ($value).saturating_sub($delta)
    };
}

macro_rules! unsigned_step {
    ($step:expr, i) => {
        $step.cast_unsigned()
    };
    ($step:expr, u) => {
        $step
    };
}

/// Implement `Steppable` for integer types.
///
/// * `$t` == type on which to implement `Steppable` for
/// * `$u` == the unsigned variant of `$t`
/// * `$s` == signedness of `$t`, `u` or `i`
macro_rules! impl_steppable_int {
    ($(($t:ty, $u:ty, $s:tt)),+) => {
        $(
            impl Steppable for $t {
                fn is_valid(self) -> bool { true }

                fn zero() -> Self { 0 }

                fn add_steps(&self, step: Self, count: i64, snap: Option<Self>) -> Self {
                    // Add the steps
                    // * Use unsigned delta so we can go from signed::MIN -> signed::MAX
                    // * Clamp count to the maximum unsigned value to avoid incorrect truncation
                    let magnitude = <$u>::try_from(count.unsigned_abs()).unwrap_or(<$u>::MAX);
                    let delta = unsigned_step!(step, $s).checked_mul(magnitude);
                    let mut value = if count < 0 {
                        match delta {
                            Some(delta) => saturating_sub_unsigned!(self, delta, $s),
                            None => Self::MIN,
                        }
                    } else {
                        match delta {
                            Some(delta) => saturating_add_unsigned!(self, delta, $s),
                            None => Self::MAX,
                        }
                    };

                    // Potential snapping
                    if let Some(snap) = snap {
                        let rem = value.rem_euclid(snap);
                        if rem > 0 {
                            // Check if we are closer to the next step.
                            // If rem >= dist_to_next, we are past the midpoint.
                            let dist_to_next = snap - rem;
                            if rem >= dist_to_next {
                                // Round up: add the missing distance.
                                value = value.saturating_add(dist_to_next);
                            } else {
                                // Round down: subtract the remainder.
                                value = value.saturating_sub(rem);
                            }
                        }
                    }

                    // Return the result
                    value
                }

                fn adjacent_snap(&self, snap: Self, forward: bool) -> Self {
                    let remainder = self.rem_euclid(snap);
                    if forward {
                        self.saturating_add(snap - remainder)
                    } else {
                        self.saturating_sub(if remainder == 0 { snap } else { remainder })
                    }
                }

                fn steps_to(&self, target: Self, step: Self) -> u64 {
                    let delta = self.abs_diff(target);
                    let steps = delta.div_ceil(unsigned_step!(step, $s));
                    u64::try_from(steps).unwrap_or(u64::MAX)
                }

                fn min_step() -> Self {
                    1
                }

                fn display(&self, _step: Self, _min: Self, _max: Self) -> String {
                    format!("{self}")
                }
            }
        )+
    };
}

impl_steppable_int!(
    (i8, u8, i),
    (u8, u8, u),
    (i16, u16, i),
    (u16, u16, u),
    (i32, u32, i),
    (u32, u32, u),
    (i64, u64, i),
    (u64, u64, u),
    (isize, usize, i),
    (usize, usize, u)
);

// Snap in the wider floating domain. When a grid index is too large to
// represent exactly, use the remainder instead of overflowing value / snap
// or rounding away significant low bits of the grid index.
fn snap_float(value: f64, snap: f64) -> f64 {
    let magnitude = value.abs();
    let index = magnitude / snap;
    let snapped = if index < EXACT_GRID_LIMIT {
        index.round() * snap
    } else {
        let remainder = magnitude % snap;
        let up = snap - remainder;
        if remainder < up {
            magnitude - remainder
        } else {
            magnitude + up
        }
    };
    snapped.copysign(value)
}

// The signed remainder avoids rounding a tiny negative residue up to an
// entire interval, which rem_euclid can do for floating-point values.
const fn adjacent_grid_by_remainder(value: f64, interval: f64, forward: bool) -> f64 {
    let remainder = value % interval;
    if forward {
        let distance = if remainder < 0.0 {
            -remainder
        } else {
            interval - remainder
        };
        value + distance
    } else {
        let distance = if remainder > 0.0 {
            remainder
        } else {
            interval + remainder
        };
        value - distance
    }
}

fn float_step_count(base: f64, target: f64, step: f64) -> u64 {
    let delta = target - base;
    let count = if delta.is_finite() {
        delta / step
    } else {
        base.mul_add(-0.5, target * 0.5) / (step * 0.5)
    };
    nonnegative_steps(count)
}

/// Implement `Steppable` for float types.
///
/// * `$t` == type on which to implement `Steppable` for
macro_rules! impl_steppable_float {
    ($(($t:ty, $widen:path, $narrow:path)),+) => {
        $(
            impl Steppable for $t {
                fn is_valid(self) -> bool { self.is_finite() }

                fn zero() -> Self { 0.0 }

                fn add_steps(&self, step: Self, count: i64, snap: Option<Self>) -> Self {
                    let value = step_sum($widen(*self), $widen(step), ticks_as_float(count));
                    let value = value.clamp($widen(Self::MIN), $widen(Self::MAX));
                    let value = snap.map_or(value, |snap| snap_float(value, $widen(snap)));
                    $narrow(value.clamp($widen(Self::MIN), $widen(Self::MAX)))
                }

                fn adjacent_snap(&self, snap: Self, forward: bool) -> Self {
                    let value = $widen(*self);
                    let interval = $widen(snap);
                    let direction = if forward { 1.0 } else { -1.0 };
                    let index = value / interval;
                    let candidate = if index.abs() < EXACT_GRID_LIMIT {
                        let nearest = index.round();
                        let grid = $narrow(nearest * interval);
                        let index = if (forward && grid > *self) || (!forward && grid < *self) {
                            nearest
                        } else {
                            nearest + direction
                        };
                        index * interval
                    } else {
                        adjacent_grid_by_remainder(value, interval, forward)
                    };
                    let candidate = $narrow(candidate.clamp($widen(Self::MIN), $widen(Self::MAX)));
                    // A grid interval can be smaller than one representable
                    // increment. Advance by one ULP if rounding erased it.
                    if forward && candidate <= *self {
                        self.next_up().min(Self::MAX)
                    } else if !forward && candidate >= *self {
                        self.next_down().max(Self::MIN)
                    } else {
                        candidate
                    }
                }

                fn steps_to(&self, target: Self, step: Self) -> u64 {
                    float_step_count($widen(*self), $widen(target), $widen(step))
                }

                fn min_step() -> Self {
                    Self::from_bits(1)
                }

                fn display(&self, step: Self, _min: Self, _max: Self) -> String {
                    let precision = decimal_precision(step);
                    format!("{self:.0$}", precision)
                }
            }
        )+
    };
}

const fn narrow_step_float(value: f64) -> f32 {
    value as f32
}

impl_steppable_float!(
    (f32, f64::from, narrow_step_float),
    (f64, std::convert::identity, std::convert::identity)
);

/// Invalid numeric configuration for a step input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidStepInput<T> {
    /// Bounds must be valid and ordered.
    Bounds {
        /// Lower bound.
        min: T,
        /// Upper bound.
        max: T,
    },
    /// The base must be valid and inside the range.
    Base(T),
    /// The step or snap interval must be valid and positive.
    Interval(T),
}

impl<T: Debug> Display for InvalidStepInput<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bounds { min, max } => write!(f, "invalid step input bounds {min:?}..={max:?}"),
            Self::Base(base) => write!(f, "invalid step input base {base:?}"),
            Self::Interval(step) => write!(f, "invalid step input interval {step:?}"),
        }
    }
}

impl<T: Debug> std::error::Error for InvalidStepInput<T> {}

// --- MARK: BUILDERS
impl<T: Steppable> StepInput<T> {
    /// Creates a step input from validated application values.
    ///
    /// # Panics
    ///
    /// Invalid configuration panics in debug builds. Release builds log the
    /// error and normalize invalid fields. Use [`Self::try_new`] for fallible
    /// configuration supplied at runtime.
    #[must_use = "Use the configured widget."]
    pub fn new(base: T, step: T, min: T, max: T) -> Self {
        Self::try_new(base, step, min, max).unwrap_or_else(|error| {
            debug_panic!("{error}");
            let max = if max.is_valid() {
                max
            } else if min.is_valid() {
                min
            } else {
                T::zero()
            };
            let min = if Self::valid_bounds(min, max) {
                min
            } else {
                max
            };
            let base = if base.is_valid() {
                Self::clamp_value(base, min, max)
            } else {
                min
            };
            let step = if Self::valid_interval(step) {
                step
            } else {
                T::min_step()
            };
            Self::from_validated(base, step, min, max)
        })
    }

    /// Creates a step input with a valid ordered range, base and positive step.
    ///
    /// # Errors
    ///
    /// Returns the invalid field if bounds are unordered, base is outside the
    /// range, a value is outside its numeric domain, or step is non-positive.
    pub fn try_new(base: T, step: T, min: T, max: T) -> Result<Self, InvalidStepInput<T>> {
        if !Self::valid_bounds(min, max) {
            return Err(InvalidStepInput::Bounds { min, max });
        }
        if !Self::valid_base(base, min, max) {
            return Err(InvalidStepInput::Base(base));
        }
        if !Self::valid_interval(step) {
            return Err(InvalidStepInput::Interval(step));
        }
        Ok(Self::from_validated(base, step, min, max))
    }

    fn valid_bounds(min: T, max: T) -> bool {
        min.is_valid() && max.is_valid() && min <= max
    }

    fn valid_base(base: T, min: T, max: T) -> bool {
        base.is_valid() && min <= base && base <= max
    }

    fn valid_interval(value: T) -> bool {
        value.is_valid() && value >= T::min_step()
    }

    fn from_validated(base: T, step: T, min: T, max: T) -> Self {
        let mut this = Self {
            base,
            step,
            snap: None,
            min,
            max,
            value: base,
            ticks: 0,
            snap_active: false,
            min_ticks: 0,
            max_ticks: 0,
            wrap: false,
            label: None,
            display: None,
            drag_start: None,
            slide_last: None,
            gearing: SlideGearing::default(),
            hover_backward: false,
            label_x_start: 0.,
            label_x_end: 0.,
        };

        this.calculate_tick_bounds();

        this
    }

    /// Returns the [`StepInput`] with the given `snap` value.
    ///
    /// When the user activates snap mode by holding `Ctrl`/`Cmd`,
    /// then the step value will be snapped to a multiple of this.
    ///
    /// The `snap` value must be greater than zero. Invalid values will be ignored.
    ///
    /// # Panics
    ///
    /// Panics if `snap` is zero or less and debug assertions are enabled.
    #[must_use = "Use the configured value."]
    pub fn with_snap(mut self, snap: T) -> Self {
        if Self::valid_interval(snap) {
            self.snap = Some(snap);
        } else {
            debug_panic!("`snap` must be greater than zero and at least `T::min_step`");
        }
        self
    }

    /// Returns the [`StepInput`] with the given `wrap` value.
    ///
    /// Wrap determines whether the values should form an infinite circle,
    /// where increasing above `max` results in `min` and
    /// decreasing below `min` results in `max`.
    #[must_use = "Use the configured value."]
    pub const fn with_wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    /// Returns the [`StepInput`] with a custom `display` function.
    ///
    /// This `display` function will receive a [`StepState`]
    /// and it will need to return its `String` representation.
    ///
    /// This is useful for showing units (1 ft, 2 GB) or for doing visual-only rounding, etc.
    ///
    /// [`StepState`]: StepState
    #[must_use = "Use the configured value."]
    pub fn with_display<F>(mut self, display: F) -> Self
    where
        F: Fn(StepState<T>) -> String + 'static,
    {
        self.display = Some(Box::new(display));
        self
    }
}

// --- MARK: WIDGETMUT
impl<T: Steppable> StepInput<T> {
    /// Set a new `base` value.
    ///
    /// The new `base` value must be greater or equal to `min`
    /// and less than or equal to `max`. Otherwise it will be ignored.
    ///
    /// Never call this in response to a [`Step`] action as it may cause precision issues.
    /// Only call this when the base value has changed due to reasons other than what
    /// [`StepInput`] already knows about.
    ///
    /// # Panics
    ///
    /// Panics if `base` is less than `min` or greater than `max` and debug assertions are enabled.
    pub fn set_base(this: &mut WidgetMut<'_, Self>, base: T) {
        if this.widget.set_base_impl(base) {
            Self::refresh_display(this);
        }
    }

    /// Set a new `step` value.
    ///
    /// The new `step` value must be greater than zero. Invalid values will be ignored.
    ///
    /// # Panics
    ///
    /// Panics if `step` is zero or less and debug assertions are enabled.
    pub fn set_step(this: &mut WidgetMut<'_, Self>, step: T) {
        if !Self::valid_interval(step) {
            debug_panic!("set_step: `step` >= `T::min_step` assert failed");
            return;
        }
        if this.widget.step != step {
            this.widget.step = step;
            // Only a changed step requires resetting the tick origin.
            Self::set_base(this, this.widget.value);
        }
    }

    /// Set a new `snap` value.
    ///
    /// The new `snap` value must be greater than zero. Invalid values will be ignored.
    /// `None` disables the snap functionality.
    ///
    /// # Panics
    ///
    /// Panics if `snap` is zero or less and debug assertions are enabled.
    pub fn set_snap(this: &mut WidgetMut<'_, Self>, snap: Option<T>) {
        if let Some(snap) = snap
            && !Self::valid_interval(snap)
        {
            debug_panic!("set_snap: `snap` > `T::min_step` assert failed");
            return;
        }
        if this.widget.snap != snap {
            this.widget.finish_snap();
            this.widget.snap = snap;
        }
    }

    /// Set new `min` and `max` bounds.
    ///
    /// `min` must be less or equal to `max`. Invalid values will be ignored.
    ///
    /// # Panics
    ///
    /// Panics if `min` is greater than `max` and debug assertions are enabled.
    pub fn set_bounds(this: &mut WidgetMut<'_, Self>, min: T, max: T) {
        if !Self::valid_bounds(min, max) {
            debug_panic!("set_bounds: `min` <= `max` assert failed");
            return;
        }
        let widget = &mut *this.widget;
        if widget.min == min && widget.max == max {
            return;
        }
        widget.min = min;
        widget.max = max;
        let value = Self::clamp_value(widget.value, min, max);
        // Preserve the selected value when the previous tick origin leaves the
        // range. Only clamp the selection itself when it is out of bounds.
        if widget.snap_active || widget.base < min || widget.base > max || value != widget.value {
            widget.set_base_impl(value);
        } else {
            widget.calculate_tick_bounds();
            if widget.ticks < widget.min_ticks || widget.ticks > widget.max_ticks {
                widget.set_base_impl(value);
            }
        }
        Self::refresh_display(this);
    }

    /// Set a new `wrap` value.
    ///
    /// Wrap determines whether the values should form an infinite circle,
    /// where increasing above `max` results in `min` and
    /// decreasing below `min` results in `max`.
    pub fn set_wrap(this: &mut WidgetMut<'_, Self>, wrap: bool) {
        if this.widget.wrap != wrap {
            this.widget.wrap = wrap;
            this.ctx.request_accessibility_update();
        }
    }

    /// Set a new custom `display` function.
    ///
    /// This `display` function will receive a [`StepState`]
    /// and it will need to return its `String` representation.
    ///
    /// This is useful for showing units (1 ft, 2 GB) or for doing visual-only rounding, etc.
    ///
    /// [`StepState`]: StepState
    pub fn set_display<F>(this: &mut WidgetMut<'_, Self>, display: F)
    where
        F: Fn(StepState<T>) -> String + 'static,
    {
        this.widget.display = Some(Box::new(display));
        Self::refresh_display(this);
    }

    fn refresh_display(this: &mut WidgetMut<'_, Self>) {
        let display_value = this.widget.display_value(this.widget.value);
        if let Some(label) = &this.widget.label
            && let Some(mut label) = this.ctx.get_mut(label.id())
        {
            Label::set_text(&mut label, display_value);
        }
        this.ctx.request_accessibility_update();
    }
}

// --- MARK: METHODS
impl<T: Steppable> StepInput<T> {
    fn handle_pointer_move(&mut self, ctx: &mut EventCtx<'_>, state: &PointerState) {
        // If we're hovered, highlight the correct side's button.
        if ctx.is_hovered() {
            let content_box_center_x = ctx.content_box().center().x;
            let local_x = ctx.local_position(state.position).x;
            let hover_backward = local_x <= content_box_center_x;
            if hover_backward != self.hover_backward {
                self.hover_backward = hover_backward;
                ctx.request_paint_only();
            }
        }

        // Unless a drag is in progress we can return early.
        if !ctx.is_pointer_capture_target() {
            return;
        }
        let Some(drag_start) = self.drag_start else {
            return;
        };

        // We do slide math in logical pixels, because we don't want
        // the scale factor (i.e. high DPI screens) to affect the speed.
        let pos = state.logical_point();
        let distance = (pos - drag_start).x;

        // Determine the movement delta since the last slide update.
        let distance_last = if let Some(slide_last) = self.slide_last {
            (slide_last - drag_start).x
        } else {
            // We don't start the slide with just any pointer drag,
            // as shaky hands or trackpad use will cause minor inadvertent drag.
            if distance.abs() - SLIDE_INACTIVE_LENGTH.get() <= 0. {
                // Not enough drag to start a slide
                return;
            }
            // We're starting the slide now with a slight jump.
            0.
        };

        // Calculate the steps delta that this latest movement results in.
        let (steps, drift) = Self::steps(distance);
        let (steps_last, _) = Self::steps(distance_last);
        let steps_delta = steps.saturating_sub(steps_last);

        let mut ticks_per_step = TICKS_PER_STEP;

        // Modifier based gearing adjusts the ticks per step.
        self.gearing.faster = state.modifiers.shift();
        self.gearing.slower = state.modifiers.alt();
        if self.gearing.faster {
            ticks_per_step *= 10;
        }
        if self.gearing.slower {
            ticks_per_step /= 10;
        }

        // Calculate the final ticks delta.
        let ticks_delta = steps_delta.saturating_mul(ticks_per_step);

        // Pointer move delta can be very granular and we don't want to lose any of it.
        // We also do rounding in steps calculation and need to account for that.
        // So we save the last update position precisely based on what we actually used.
        self.slide_last = Some(Point::new(pos.x - drift, pos.y));

        // Snap based on modifier
        let snap = if cfg!(target_os = "macos") {
            state.modifiers.meta()
        } else {
            state.modifiers.ctrl()
        };

        // Update the ticks based on the calculated ticks delta.
        let (ticks_unused, value_changed, hit_bounds) = self.update_ticks(ticks_delta, snap);

        // We calculate unused steps with f64 because unused ticks may not divide nicely.
        let steps_unused = ticks_as_float(ticks_unused) / ticks_as_float(ticks_per_step);
        // Used distance here means accounted for, i.e. split between steps and drift.
        let distance_used = Self::distance(ticks_as_float(steps) - steps_unused) + drift;
        let distance_unused = distance - distance_used;

        if hit_bounds && !self.wrap {
            // Once we reach a min/max edge and there is no wrap,
            // we need to start shifting the drag_start position towards the pointer.
            // So that when changing direction we won't be stuck in high gear.
            self.drag_start = Some(Point::new(drag_start.x + distance_unused, drag_start.y));
        } else if hit_bounds && let Some(slide_last) = self.slide_last.as_mut() {
            // Preserve partial movement towards the next wrap, even at slow gear.
            slide_last.x -= distance_unused;
        }

        // If the value was changed, we need to handle it
        if value_changed {
            self.handle_updated_value::<Step<T>>(ctx);
        }

        // Always request paint during slide to update the visual speed indicator.
        ctx.request_paint_only();
    }

    /// Returns the string representation of the given `value`.
    fn display_value(&self, value: T) -> String {
        self.display.as_deref().map_or_else(
            || value.display(self.step, self.min, self.max),
            |display| {
                let state = StepState {
                    value,
                    step: self.step,
                    min: self.min,
                    max: self.max,
                };
                display(state)
            },
        )
    }

    /// Updates the label with the new value and emits a [`Step`] action.
    fn handle_updated_value<A: AnyDebug + Send + From<Step<T>>>(&mut self, ctx: &mut EventCtx<'_>) {
        let display_value = self.display_value(self.value);
        if let Some(label) = self.label.as_mut() {
            ctx.mutate_child_later(label, move |mut label| {
                Label::set_text(&mut label, display_value);
            });
        }
        ctx.request_accessibility_update();
        ctx.submit_action::<A>(Step { value: self.value });
    }

    /// Calculates the active value.
    ///
    /// This can return a value outside the allowed bounds.
    /// Use [`clamp_value`] to deal with that.
    ///
    /// [`clamp_value`]: Self::clamp_value
    fn calculate_value(base: T, step: T, ticks: i64) -> T {
        // We round the ticks for smoother sliding and symmetry when going in both directions.
        let step_count = ticks.saturating_add(TICKS_PER_STEP / 2 * ticks.signum()) / TICKS_PER_STEP;
        base.add_steps(step, step_count, None)
    }

    /// Clamps the given `value` between `min` and `max`.
    fn clamp_value(value: T, min: T, max: T) -> T {
        if value < min {
            min
        } else if value > max {
            max
        } else {
            value
        }
    }

    /// Returns the `steps` as positive ticks.
    fn steps_to_positive_ticks(steps: u64) -> i64 {
        steps
            .saturating_mul(TICKS_PER_STEP.cast_unsigned())
            .min(i64::MAX.cast_unsigned())
            .cast_signed()
    }

    /// Returns the `steps` as negative ticks.
    fn steps_to_negative_ticks(steps: u64) -> i64 {
        steps
            .checked_mul(TICKS_PER_STEP as u64)
            .map_or(i64::MIN, |t| {
                if t >= i64::MIN.cast_unsigned() {
                    i64::MIN
                } else {
                    t.wrapping_neg().cast_signed()
                }
            })
    }

    /// Calculates `min_ticks`/`max_ticks` for reaching `min`/`max` from `base` with `step`.
    ///
    /// Note that these results saturate at their limits, so e.g. `i64::MAX` for `max_ticks`
    /// does not necessarily mean that `max` will be reached.
    fn calculate_tick_bounds(&mut self) {
        let min_steps = self.min.steps_to(self.base, self.step);
        self.min_ticks = Self::steps_to_negative_ticks(min_steps);

        let max_steps = self.base.steps_to(self.max, self.step);
        self.max_ticks = Self::steps_to_positive_ticks(max_steps);
    }

    /// Rebases the tick origin and returns whether the requested value was valid.
    ///
    /// # Panics
    ///
    /// Panics if base is less than `min` or greater than `max` and debug assertions are enabled.
    fn set_base_impl(&mut self, base: T) -> bool {
        if !Self::valid_base(base, self.min, self.max) {
            debug_panic!("set_base: `min` <= `base` <= `max` assert failed");
            return false;
        }
        // We do this even if self.base == base, because some callers expect ticks to be reset.
        self.base = base;
        self.value = base;
        self.ticks = 0;
        self.snap_active = false;
        self.calculate_tick_bounds();
        true
    }

    /// Projects the raw tick position onto the selected grid. Physical
    /// range endpoints remain reachable even if they are off-grid.
    fn set_value(&mut self, ticks: i64, snap: bool) {
        let raw = Self::calculate_value(self.base, self.step, ticks);
        if !raw.is_valid() {
            debug_panic!("Steppable::add_steps returned an invalid value: {raw:?}");
            return;
        }
        let raw = Self::clamp_value(raw, self.min, self.max);
        let value = if let Some(interval) = self.snap.filter(|_| snap)
            && self.min < raw
            && raw < self.max
        {
            raw.add_steps(self.step, 0, Some(interval))
        } else {
            raw
        };
        if value.is_valid() {
            self.value = Self::clamp_value(value, self.min, self.max);
        } else {
            debug_panic!("Steppable::add_steps returned an invalid snapped value: {value:?}");
        }
    }

    /// Once the user leaves snap mode, ordinary steps start from the actual
    /// displayed selection rather than an approximate inverse of the grid.
    fn finish_snap(&mut self) {
        if self.snap_active {
            self.set_base_impl(self.value);
        }
    }

    fn finish_slide(&mut self) {
        self.finish_snap();
        self.slide_last = None;
        self.drag_start = None;
        self.gearing = SlideGearing::default();
    }

    /// Moves to the next snap-grid point (or range endpoint) in the chosen
    /// direction. A discrete snap is an explicit rebase: the next ordinary
    /// step starts from the actual selected value, not an approximate tick.
    fn step_snap(&mut self, forward: bool) -> bool {
        let Some(snap) = self.snap else {
            return if forward {
                self.next_step()
            } else {
                self.prev_step()
            };
        };
        let old_value = self.value;
        let target = if self.wrap && forward && old_value == self.max {
            self.min
        } else if self.wrap && !forward && old_value == self.min {
            self.max
        } else {
            Self::clamp_value(old_value.adjacent_snap(snap, forward), self.min, self.max)
        };
        if target == old_value {
            return false;
        }
        self.set_base_impl(target)
    }

    fn next_snap(&mut self) -> bool {
        self.step_snap(true)
    }

    fn prev_snap(&mut self) -> bool {
        self.step_snap(false)
    }

    /// Updates ticks based on the given `delta`.
    ///
    /// Returns `(unused_delta, value_changed, hit_bounds)`.
    ///
    /// Ticks will be updated to min/max boundaries if wrapping is disabled.
    /// Otherwise the value will be rebased at the wrapping boundary
    /// and ticks will be relative to that new base.
    fn update_ticks(&mut self, delta: i64, snap: bool) -> (i64, bool, bool) {
        const WRAP_COST: u64 = TICKS_PER_STEP.cast_unsigned();
        let snap = snap && self.snap.is_some();
        if !snap {
            self.finish_snap();
        }
        if delta == 0 {
            return (0, false, false);
        }

        let dpos = delta.is_positive();

        let mut new_ticks = self.ticks;
        let mut unused_delta = delta.unsigned_abs();

        loop {
            // Calculate free space before tick bounds
            let space = if dpos {
                (self.max_ticks.cast_unsigned()).wrapping_sub(new_ticks.cast_unsigned())
            } else {
                (new_ticks.cast_unsigned()).wrapping_sub(self.min_ticks.cast_unsigned())
            };
            // If unused delta fits in the space, just use it all
            if unused_delta <= space {
                new_ticks = if dpos {
                    (new_ticks.cast_unsigned()).wrapping_add(unused_delta)
                } else {
                    (new_ticks.cast_unsigned()).wrapping_sub(unused_delta)
                }
                .cast_signed();
                unused_delta = 0;
                break;
            }
            // Otherwise we hit tick bounds
            new_ticks = if dpos { self.max_ticks } else { self.min_ticks };
            unused_delta -= space;
            // Perhaps we can wrap?
            if self.wrap && unused_delta >= WRAP_COST {
                self.base = if dpos { self.min } else { self.max };
                self.calculate_tick_bounds();
                new_ticks = 0;
                unused_delta -= WRAP_COST;
                // After rebasing, every complete cycle returns to this same base.
                // Skip whole cycles instead of iterating once per wrap.
                let span = if dpos {
                    self.max_ticks.unsigned_abs()
                } else {
                    self.min_ticks.unsigned_abs()
                };
                if let Some(cycle) = span.checked_add(WRAP_COST) {
                    unused_delta %= cycle;
                }
            } else {
                break;
            }
        }

        // Convert the unused delta back to signed
        let unused_delta = if dpos {
            unused_delta
        } else {
            unused_delta.wrapping_neg()
        }
        .cast_signed();

        let hit_bounds = unused_delta != 0;

        // Update the ticks
        self.ticks = new_ticks;

        // Calculate the active value change
        let old_value = self.value;
        self.set_value(new_ticks, snap);
        self.snap_active = snap;
        let changed_value = old_value != self.value;

        (unused_delta, changed_value, hit_bounds)
    }

    /// Increments ticks by one step's worth.
    ///
    /// Returns `true` if the active value changed.
    #[inline]
    fn next_step(&mut self) -> bool {
        self.update_ticks(TICKS_PER_STEP, false).1
    }

    /// Decrements ticks by one step's worth.
    ///
    /// Returns `true` if the active value changed.
    #[inline]
    fn prev_step(&mut self) -> bool {
        self.update_ticks(TICKS_PER_STEP.wrapping_neg(), false).1
    }

    /// Returns slide `(speed, forward, backward)` for visual purposes.
    ///
    /// The speed is linear and normalized `[0..1]`.
    ///
    /// * 80% based on distance compared to max gear requirement
    /// * +10% if slower mode is not enabled
    /// * +10% if faster mode is enabled
    ///
    /// Actual step gearing is unlikely to be linear,
    /// but linear works much better for visual cues.
    ///
    /// If the slide is increasing the value then `forward` is `true`.
    /// If the slide is decreasing the value then `backward` is `true`.
    fn visual_speed(&self) -> (f64, bool, bool) {
        let (Some(drag_start), Some(slide_last)) = (self.drag_start, self.slide_last) else {
            return (0., false, false);
        };

        let distance = slide_last.x - drag_start.x;
        let direction = distance.signum();
        let distance = distance.abs();
        if distance == 0. {
            return (0., false, false);
        }

        let mut speed = (distance / GEAR_2_END).clamp(0., 1.) * 0.8;
        if !self.gearing.slower {
            speed += 0.1;
        }
        if self.gearing.faster {
            speed += 0.1;
        }
        (speed, direction > 0., direction < 0.)
    }

    /// Returns the number of steps needed to cover the given `distance`.
    ///
    /// The given `distance` must be from the drag start position.
    ///
    /// This function will also return the drift from the given `distance`,
    /// in case the `distance` does not precisely convert into integer steps.
    /// Positive drift means that not all the `distance` was used,
    /// negative drift means that more than `distance` was used.
    ///
    /// We use distance based gearing. The bigger the distance, the faster the steps will grow.
    fn steps(distance: f64) -> (i64, f64) {
        let direction = distance.signum();
        let distance = distance.abs();

        // Calculate the number of steps that the given distance converts to.
        // We do this with a progressive system, to ensure the earlier distance
        // is always covered using the lower gears. Not doing this would introduce pointer
        // movement speed into the formula, as jumps in distance would only use the high gear.
        let (start_distance, base_steps, ratio) = if distance <= GEAR_1_END {
            (0., 0., GEAR_1_RATIO)
        } else if distance <= GEAR_2_END {
            (GEAR_1_END, GEAR_1_END_STEPS, GEAR_2_RATIO)
        } else {
            (GEAR_2_END, GEAR_2_END_STEPS, GEAR_3_RATIO)
        };

        let steps_raw = (distance - start_distance).mul_add(ratio, base_steps);
        let steps = steps_raw.round();
        let drift = (steps_raw - steps) * ratio.recip();

        // Return the steps with the correct direction
        ((steps * direction) as i64, drift * direction)
    }

    /// Returns the distance covered by the given `steps`.
    ///
    /// The returned distance is from the drag start position,
    /// so the provided `steps` need to be the total steps from that point.
    ///
    /// This function accounts for distance based gearing.
    /// The bigger the distance, the more steps will be used.
    fn distance(steps: f64) -> f64 {
        let direction = steps.signum();
        let steps = steps.abs();

        let gear_3_steps = (steps - GEAR_2_END_STEPS).max(0.);
        let gear_2_steps = (steps - GEAR_1_END_STEPS - gear_3_steps).max(0.);
        let gear_1_steps = steps - gear_2_steps - gear_3_steps;

        let gear_1_distance = gear_1_steps * GEAR_1_RATIO.recip();
        let gear_2_distance = gear_2_steps * GEAR_2_RATIO.recip();
        let gear_3_distance = gear_3_steps * GEAR_3_RATIO.recip();

        (gear_1_distance + gear_2_distance + gear_3_distance) * direction
    }
}

/// The latest active value of [`StepInput`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Step<T> {
    /// Currently active value.
    pub value: T,
}

// --- MARK: IMPL WIDGET
impl<T: Steppable> Widget for StepInput<T> {
    type Action = Step<T>;

    fn on_access_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &AccessEvent,
    ) {
        if ctx.target() != ctx.widget_id() {
            return;
        }
        let value_changed = match event.action {
            Action::Increment => self.next_step(),
            Action::Decrement => self.prev_step(),
            _ => return,
        };
        ctx.set_handled();

        // If the value was changed, we need to handle it
        if value_changed {
            self.handle_updated_value::<Self::Action>(ctx);
        }
    }

    fn on_text_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &TextEvent,
    ) {
        if !ctx.is_focus_target() {
            return;
        }
        match event {
            TextEvent::Keyboard(ke) => {
                let mut value_changed = false;

                // Snap based on modifier
                let snap = if cfg!(target_os = "macos") {
                    ke.modifiers.meta()
                } else {
                    ke.modifiers.ctrl()
                };

                if !snap {
                    self.finish_snap();
                }

                match ke.key {
                    // Update slide modifier so the visuals can change without pointer movement.
                    Key::Named(NamedKey::Shift) => {
                        let slide_faster = ke.state.is_down();
                        if slide_faster != self.gearing.faster {
                            self.gearing.faster = slide_faster;
                            ctx.request_paint_only();
                        }
                    }
                    // Update slide modifier so the visuals can change without pointer movement.
                    Key::Named(NamedKey::Alt) => {
                        let slide_slower = ke.state.is_down();
                        if slide_slower != self.gearing.slower {
                            self.gearing.slower = slide_slower;
                            ctx.request_paint_only();
                        }
                    }
                    Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowDown) => {
                        if ke.state.is_down() {
                            ctx.set_handled();
                            value_changed = if snap {
                                self.prev_snap()
                            } else {
                                self.prev_step()
                            }
                        }
                    }
                    Key::Named(NamedKey::ArrowRight | NamedKey::ArrowUp) => {
                        if ke.state.is_down() {
                            ctx.set_handled();
                            value_changed = if snap {
                                self.next_snap()
                            } else {
                                self.next_step()
                            }
                        }
                    }
                    Key::Named(NamedKey::Home) => {
                        if ke.state.is_down() {
                            ctx.set_handled();
                            let previous = self.value;
                            self.set_base_impl(self.min);
                            value_changed = self.value != previous;
                        }
                    }
                    Key::Named(NamedKey::End) => {
                        if ke.state.is_down() {
                            ctx.set_handled();
                            let previous = self.value;
                            self.set_base_impl(self.max);
                            value_changed = self.value != previous;
                        }
                    }
                    _ => match ke.code {
                        Code::NumpadSubtract if ke.state.is_down() => {
                            ctx.set_handled();
                            value_changed = if snap {
                                self.prev_snap()
                            } else {
                                self.prev_step()
                            }
                        }
                        Code::NumpadAdd if ke.state.is_down() => {
                            ctx.set_handled();
                            value_changed = if snap {
                                self.next_snap()
                            } else {
                                self.next_step()
                            }
                        }
                        _ => (),
                    },
                }

                // If the value was changed, we need to handle it
                if value_changed {
                    self.handle_updated_value::<Self::Action>(ctx);
                }
            }
            _ => (),
        }
    }

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        match event {
            PointerEvent::Down(pbe)
                if pbe.button.is_none_or(|btn| btn == PointerButton::Primary) =>
            {
                // TODO: Once we get editable input support,
                //       stop starting the slide from the edit area.

                self.finish_slide();
                let pos = pbe.state.logical_point();
                self.drag_start = Some(pos);
                ctx.capture_pointer();
                ctx.request_focus();
                ctx.set_handled();

                // TODO: Lock and hide the pointer during a slide.
                //       https://github.com/linebender/xilem/issues/850
            }
            PointerEvent::Move(pu) => {
                self.handle_pointer_move(ctx, &pu.current);
                if ctx.is_pointer_capture_target() {
                    ctx.set_handled();
                }
            }
            PointerEvent::Cancel(_pi) => {
                self.finish_slide();
                ctx.request_paint_only();
            }
            // We only care about primary button and touch
            PointerEvent::Up(pbe)
                if ctx.is_pointer_capture_target()
                    && pbe.button.is_none_or(|btn| btn == PointerButton::Primary) =>
            {
                ctx.set_handled();
                self.handle_pointer_move(ctx, &pbe.state);
                // Regular click handling happens only if:
                // * There is no slide in progress
                // * The button was previously pressed down on us (active)
                // * The pointer is still on us (hovered)
                if self.drag_start.is_some() && self.slide_last.is_none() && ctx.is_hovered() {
                    let content_box_center_x = ctx.content_box().center().x;
                    let local_x = ctx.local_position(pbe.state.position).x;

                    // Snap based on modifier
                    let snap = if cfg!(target_os = "macos") {
                        pbe.state.modifiers.meta()
                    } else {
                        pbe.state.modifiers.ctrl()
                    };

                    // Update the active value based on which side was clicked
                    let value_changed = if local_x <= content_box_center_x {
                        if snap {
                            self.prev_snap()
                        } else {
                            self.prev_step()
                        }
                    } else if snap {
                        self.next_snap()
                    } else {
                        self.next_step()
                    };

                    // If the value was changed, we need to handle it
                    if value_changed {
                        self.handle_updated_value::<Self::Action>(ctx);
                    }
                }
                self.finish_slide();
                ctx.request_paint_only();
            }
            _ => (),
        }
    }

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        if let Some(label) = self.label.as_mut() {
            ctx.register_child(label);
        }
    }

    fn property_changed(&mut self, ctx: &mut UpdateCtx<'_>, property_type: TypeId) {
        if StepInputStyle::matches(property_type) {
            ctx.request_layout();
        } else if ContentColor::matches(property_type)
            || BackwardColor::matches(property_type)
            || ForwardColor::matches(property_type)
            || HeatColor::matches(property_type)
        {
            ctx.request_paint_only();
        }

        // TODO: Find more elegant way to propagate property to child.
        if ContentColor::matches(property_type) {
            ctx.mutate_self_later::<Self>(|mut this| {
                let prop = *this.get_prop::<ContentColor>();
                let Some(label) = this.widget.label.as_mut() else {
                    return;
                };
                let Some(mut label) = this.ctx.get_mut::<Label>(label.id()) else {
                    return;
                };
                label.insert_prop(prop);
            });
        }
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, props: &mut PropertiesMut<'_>, event: &Update) {
        match event {
            Update::WidgetAdded => {
                let cache = ctx.property_cache();
                let color = props.get::<ContentColor>(cache);

                let display_value = self.display_value(self.value);
                self.label = Some(
                    Label::new(display_value)
                        .accessibility_hidden(true)
                        .prepare()
                        .with_props(*color)
                        .into_pod(),
                );
                ctx.children_changed();
            }
            Update::ActiveChanged(active) => {
                if !active {
                    self.finish_slide();
                }
                ctx.request_paint_only();
            }
            Update::DisabledChanged(true)
            | Update::StashedChanged(true)
            | Update::FocusChanged(false) => {
                self.finish_slide();
                ctx.request_paint_only();
            }
            Update::DisabledChanged(false) | Update::HoveredChanged(_) => {
                ctx.request_paint_only();
            }
            _ => (),
        }
    }

    fn measure(
        &mut self,
        ctx: &mut MeasureCtx<'_>,
        props: &PropertiesRef<'_>,
        axis: Axis,
        len_req: LenReq,
        cross_length: Option<Length>,
    ) -> Length {
        let content_height = theme::BASIC_WIDGET_HEIGHT;

        let cache = ctx.property_cache();
        let style = props.get::<StepInputStyle>(cache);

        let (len_req, min_result) = match len_req {
            LenReq::Min | LenReq::Max => (len_req, Length::ZERO),
            // We always want to use up all offered space but may need even more,
            // so we implement FitContent as space.max(MinContent).
            LenReq::Fit(space) => (LenReq::Min, space),
        };

        let calc_button_length = |axis| match axis {
            Axis::Horizontal => match style {
                StepInputStyle::Basic => {
                    let vertical_space = cross_length.unwrap_or(content_height);
                    let (btn_length, btn_edge_pad) =
                        Self::basic_button_length(vertical_space.get(), None);
                    match len_req {
                        LenReq::Min => 2. * 2.0_f64.mul_add(btn_edge_pad, btn_length),
                        LenReq::Max => 2. * (btn_length * 3.),
                        LenReq::Fit(_) => unreachable!(),
                    }
                }
                StepInputStyle::Flow => {
                    let vertical_space = cross_length.unwrap_or(content_height);
                    let (arrow_width, _, arrow_edge_pad) =
                        Self::flow_button_length(vertical_space.get(), None);
                    match len_req {
                        LenReq::Min => 2. * 2.0_f64.mul_add(arrow_width, arrow_edge_pad),
                        LenReq::Max => 2. * 4.0_f64.mul_add(arrow_width, arrow_edge_pad),
                        LenReq::Fit(_) => unreachable!(),
                    }
                }
            }
            .px(),
            Axis::Vertical => content_height,
        };

        let button_length = calc_button_length(axis);

        let auto_length = match axis {
            Axis::Horizontal => len_req.reduce(button_length).into(),
            Axis::Vertical => len_req.into(),
        };
        let context_size = LayoutSize::maybe(axis.cross(), cross_length);
        let cross = axis.cross();
        let label_cross_length = match cross {
            Axis::Horizontal => cross_length
                .map(|cross_length| cross_length.saturating_sub(calc_button_length(cross))),
            Axis::Vertical => cross_length,
        };
        let label_length = self.label.as_mut().map_or(Length::ZERO, |label| {
            ctx.compute_length(label, auto_length, context_size, axis, label_cross_length)
        });

        let length = match axis {
            Axis::Horizontal => label_length.saturating_add(button_length),
            Axis::Vertical => label_length.max(button_length),
        };

        min_result.max(length)
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, props: &PropertiesRef<'_>, size: Size) {
        let Some(label) = self.label.as_mut() else {
            ctx.clear_baselines();
            return;
        };

        let cache = ctx.property_cache();
        let style = props.get::<StepInputStyle>(cache);

        // Reserve MinContent worth of button space
        let buttons_width = match style {
            StepInputStyle::Basic => {
                let (btn_length, btn_edge_pad) =
                    Self::basic_button_length(size.height, Some(size.width));
                2. * 2.0_f64.mul_add(btn_edge_pad, btn_length)
            }
            StepInputStyle::Flow => {
                let (arrow_width, _, arrow_edge_pad) =
                    Self::flow_button_length(size.height, Some(size.width));
                2. * 2.0_f64.mul_add(arrow_width, arrow_edge_pad)
            }
        };

        let label_space = Size::new((size.width - buttons_width).max(0.), size.height);
        let auto_size = SizeDef::fit(label_space);
        let label_size = ctx.compute_size(label, auto_size, size.into());
        ctx.run_layout(label, label_size);

        let label_origin = Point::new(
            (size.width - label_size.width) * 0.5,
            (size.height - label_size.height) * 0.5,
        );
        ctx.place_child(label, label_origin);

        self.label_x_start = label_origin.x;
        self.label_x_end = label_origin.x + label_size.width;

        ctx.derive_baselines(label);
    }

    fn paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        let cache = ctx.property_cache();
        match props.get::<StepInputStyle>(cache) {
            StepInputStyle::Basic => Self::paint_basic(self, ctx, props, painter),
            StepInputStyle::Flow => Self::paint_flow(self, ctx, props, painter),
        }
    }

    fn accessibility_role(&self) -> Role {
        Role::SpinButton
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        // NOTE: Can't set numeric value/min/max/step because AccessKit only seems to support f64.

        node.set_value(self.display_value(self.value));
        if self.min < self.max {
            if self.wrap || self.value < self.max {
                node.add_action(Action::Increment);
            }
            if self.wrap || self.value > self.min {
                node.add_action(Action::Decrement);
            }
        }

        // TODO: Add support for SetValue when we gain edit support.
        //       Not trivial, because the value needs to satisfy all rules.
        //node.add_action(Action::SetValue);
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::from_iter(self.label.as_ref().map(WidgetPod::id))
    }

    fn accepts_focus(&self) -> bool {
        true
    }

    fn get_cursor(&self, _ctx: &QueryCtx<'_>, _pos: Point) -> CursorIcon {
        CursorIcon::EwResize
    }
}

// --- MARK: PAINT STYLES
impl<T: Steppable> StepInput<T> {
    /// Returns `(length, edge_pad)` of a basic button given the `vertical_space`.
    ///
    /// If total horizontal space is know it will help prevent awkwardly large results.
    ///
    /// The `pad` factor determines how much of vertical space is used for padding on one side.
    fn button_length(vertical_space: f64, horizontal_space: Option<f64>, pad: f64) -> (f64, f64) {
        // We want to always leave a bit of space between the button and the content-box edge,
        // so it won't touch the border even when there is no extra padding in the widget.
        // We base both the padding and the button size itself on the available content-box height.
        let mut edge_pad = pad * vertical_space;
        let mut length = 2.0_f64.mul_add(-edge_pad, vertical_space);
        // If we know the total horizontal space, make sure the button length is not over 20% of it.
        // This helps prevent overflow and other awkward results given narrow but tall constraints.
        if let Some(horizontal_space) = horizontal_space {
            let max_length = 0.2 * horizontal_space;
            if length > max_length {
                length = max_length;
                edge_pad = length / 2.0_f64.mul_add(-pad, 1.) * pad;
            }
        }
        (length, edge_pad)
    }

    /// Returns `(length, edge_pad)` of a basic button given the `vertical_space`.
    ///
    /// If total horizontal space is know it will help prevent awkwardly large results.
    #[inline]
    fn basic_button_length(vertical_space: f64, horizontal_space: Option<f64>) -> (f64, f64) {
        Self::button_length(vertical_space, horizontal_space, 0.15)
    }

    /// Returns `(width, height, edge_pad)` of a flow arrow given the `vertical_space`.
    ///
    /// If total horizontal space is know it will help prevent awkwardly large results.
    #[inline]
    fn flow_button_length(vertical_space: f64, horizontal_space: Option<f64>) -> (f64, f64, f64) {
        let (height, edge_pad) = Self::button_length(vertical_space, horizontal_space, 0.1);
        // The arrow is slightly taller than it is wider for artistic reasons.
        let width = height / 1.2;
        (width, height, edge_pad)
    }

    // Paint controls in the basic style.
    fn paint_basic(
        &self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        let cache = ctx.property_cache();
        let color_content = *props.get::<ContentColor>(cache);
        let color_backward = *props.get::<BackwardColor>(cache);
        let color_forward = *props.get::<ForwardColor>(cache);

        let size = ctx.content_box().size();
        let (_, forward, backward) = self.visual_speed();

        let (btn_length, btn_edge_pad) = Self::basic_button_length(size.height, Some(size.width));

        let label_width = self.label_x_end - self.label_x_start;
        let btn_space = ((size.width - label_width) * 0.5).max(0.);

        // Split the horizontal padding to both sides of the button,
        // but constrain the edge padding so we don't go too close nor too far from the edge.
        let btn_x_pad = ((btn_space - btn_length) * 0.5).clamp(btn_edge_pad, btn_length);

        let stroke_width = 0.1 * btn_length;
        let y_center = size.height * 0.5;

        let minus = Line::new(
            Point::new(btn_x_pad, y_center),
            Point::new(btn_x_pad + btn_length, y_center),
        );

        let plus_h = Line::new(
            Point::new(size.width - btn_length - btn_x_pad, y_center),
            Point::new(size.width - btn_x_pad, y_center),
        );
        let plus_v = Line::new(
            Point::new(
                f64::mul_add(btn_length, -0.5, size.width) - btn_x_pad,
                (size.height - btn_length) * 0.5,
            ),
            Point::new(
                f64::mul_add(btn_length, -0.5, size.width) - btn_x_pad,
                (size.height - btn_length).mul_add(-0.5, size.height),
            ),
        );

        // Only one button can have a special color at the same time.
        // Color choice priority is active > hover > none.
        let (minus_color, plus_color) = if ctx.is_active() && backward {
            (&color_backward.0, &color_content.color)
        } else if ctx.is_active() && forward {
            (&color_content.color, &color_forward.0)
        } else if ctx.is_hovered() && self.hover_backward {
            (&color_backward.0, &color_content.color)
        } else if ctx.is_hovered() && !self.hover_backward {
            (&color_content.color, &color_forward.0)
        } else {
            (&color_content.color, &color_content.color)
        };

        let style = Stroke {
            width: stroke_width,
            start_cap: Cap::Butt,
            end_cap: Cap::Butt,
            ..Default::default()
        };
        painter.stroke(minus, &style, *minus_color).draw();
        painter.stroke(plus_h, &style, *plus_color).draw();
        painter.stroke(plus_v, &style, *plus_color).draw();
    }

    fn paint_speed_lines(&self, painter: &mut Painter<'_>, geometry: SpeedLineGeometry) {
        let SpeedLineGeometry {
            size,
            speed,
            arrow_width,
            arrow_height,
            arrow_edge_pad,
            arrow_offset_active,
            arrow_base_x,
            extra_y_pad,
            backward,
            color,
        } = geometry;
        // The outer lines are thinner and shorter.
        let style1 = Stroke {
            width: 0.075 * arrow_height,
            start_cap: Cap::Butt,
            end_cap: Cap::Butt,
            ..Default::default()
        };
        // The inner lines are thicker and longer.
        let style2 = Stroke {
            width: 0.15 * arrow_height,
            start_cap: Cap::Butt,
            end_cap: Cap::Butt,
            ..Default::default()
        };

        // End the outer lines exactly underneath the arrow shoulder tip.
        let style1_x_end = f64::mul_add(arrow_width, 0.11, size.width - arrow_offset_active);
        // Keep the outer line length at 80% of the inner lines.
        let style1_x_start = (style1_x_end - self.label_x_end).mul_add(0.2, self.label_x_end);
        // Keep the outer line just barely following the arrow at the outer edge.
        let style1_y_offset = f64::mul_add(arrow_height, 0.05, style1.width * 0.5);

        // End the inner lines underneath the arrow base.
        let style2_x_end = size.width - arrow_offset_active + arrow_base_x;
        // Start from the content.
        let style2_x_start = self.label_x_end;
        // Keep the inner lines following the base of the arrow, with a slight center gap.
        let style2_y_offset = f64::mul_add(arrow_height, 0.06, style2.width * 0.5);

        // Top outer line.
        let line1_y = extra_y_pad + arrow_edge_pad + style1_y_offset;
        let line1 = Line::new(
            Point::new(style1_x_start, line1_y),
            Point::new(style1_x_end, line1_y),
        );

        // Top inner line.
        let line2_y = 0.5_f64.mul_add(size.height, -style2_y_offset);
        let line2 = Line::new(
            Point::new(style2_x_start, line2_y),
            Point::new(style2_x_end, line2_y),
        );

        // Bottom inner line.
        let line3_y = 0.5_f64.mul_add(size.height, style2_y_offset);
        let line3 = Line::new(
            Point::new(style2_x_start, line3_y),
            Point::new(style2_x_end, line3_y),
        );

        // Bottom outer line.
        let line4_y = size.height - extra_y_pad - arrow_edge_pad - style1_y_offset;
        let line4 = Line::new(
            Point::new(style1_x_start, line4_y),
            Point::new(style1_x_end, line4_y),
        );

        // Outer lines only go up to 80% opacity based on speed.
        let style1_gradient_max = speed as f32 * 0.8;
        let style1_gradient = Gradient::new_linear((style1_x_start, 0.), (style1_x_end, 0.));
        // Inner lines go up to 100% opacity based on speed.
        let style2_gradient_max = speed as f32;
        let style2_gradient = Gradient::new_linear((style2_x_start, 0.), (style2_x_end, 0.));

        // Create the gradients.
        let style1_gradient = style1_gradient.with_stops([
            (0., color.with_alpha(0.)),
            (1., color.with_alpha(style1_gradient_max)),
        ]);
        let style2_gradient = style2_gradient.with_stops([
            (0., color.with_alpha(0.)),
            (1., color.with_alpha(style2_gradient_max)),
        ]);
        // The backwards lines need to be reflected and shifted to the other side.
        let lines_affine = if backward {
            Affine::reflect((0., 0.), (0., 1.))
                .then_translate((self.label_x_end + self.label_x_start, 0.).into())
        } else {
            Affine::IDENTITY
        };

        // Actually paint the lines.
        painter
            .stroke(line1, &style1, &style1_gradient)
            .transform(lines_affine)
            .draw();
        painter
            .stroke(line2, &style2, &style2_gradient)
            .transform(lines_affine)
            .draw();
        painter
            .stroke(line3, &style2, &style2_gradient)
            .transform(lines_affine)
            .draw();
        painter
            .stroke(line4, &style1, &style1_gradient)
            .transform(lines_affine)
            .draw();
    }

    fn flow_arrow(
        size: Size,
        arrow_width: f64,
        arrow_height: f64,
        arrow_edge_pad: f64,
        extra_y_pad: f64,
    ) -> (BezPath, Point, f64) {
        // Define the various points that form a hollow-base arrow facing right.
        let arrow_y_center = 0.5_f64.mul_add(arrow_height, extra_y_pad + arrow_edge_pad);
        let arrow_tip = Point::new(arrow_width, arrow_y_center);
        let arrow_base_x = 0.575 * arrow_width;
        let arrow_base = Point::new(arrow_base_x, arrow_y_center);
        let arrow_top_shoulder = Point::new(0., extra_y_pad + arrow_edge_pad);
        let arrow_bot_shoulder = Point::new(0., size.height - extra_y_pad - arrow_edge_pad);

        // Construct the arrow shape.
        let mut arrow = BezPath::new();
        arrow.move_to(arrow_top_shoulder);
        arrow.line_to(arrow_tip);
        arrow.line_to(arrow_bot_shoulder);
        arrow.line_to(arrow_base);
        arrow.line_to(arrow_top_shoulder);

        (arrow, arrow_tip, arrow_base_x)
    }

    // Paint controls in the flow style.
    fn paint_flow(
        &self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        let cache = ctx.property_cache();
        let color_content = *props.get::<ContentColor>(cache);
        let color_backward = *props.get::<BackwardColor>(cache);
        let color_forward = *props.get::<ForwardColor>(cache);
        let color_heat = *props.get::<HeatColor>(cache);

        let size = ctx.content_box().size();
        let (speed, forward, backward) = self.visual_speed();
        let sliding = forward || backward;

        let (arrow_width, arrow_height, arrow_edge_pad) =
            Self::flow_button_length(size.height, Some(size.width));
        let extra_y_pad = 2.0_f64.mul_add(-arrow_edge_pad, size.height - arrow_height) * 0.5;

        let (arrow, arrow_tip, arrow_base_x) =
            Self::flow_arrow(size, arrow_width, arrow_height, arrow_edge_pad, extra_y_pad);

        // We want the arrow to move within three full widths of itself,
        // i.e. [AWW] -> [WWA] where A is the arrow and W is empty sapce of the same width.
        let arrow_move_range = 3.;
        // Actual available space for arrow movement depends on the label and our total size.
        let label_width = self.label_x_end - self.label_x_start;
        let arrow_space = (size.width - label_width)
            .mul_add(0.5, -arrow_edge_pad)
            .max(0.);
        // Base offset is the stationary arrow location,
        // measured from the outer edge of the widget's content-box.
        let arrow_offset_base = f64::mul_add(arrow_width, arrow_move_range, arrow_edge_pad);
        // Active offset is closer to the edge based on the visual speed factor.
        let arrow_move_active = f64::mul_add(arrow_move_range - 1., -speed, arrow_move_range);
        let arrow_offset_active = f64::mul_add(arrow_width, arrow_move_active, arrow_edge_pad);
        // Keep the offset within available space bounds if possible,
        // and favor overflowing towards the middle content not the widget bounds.
        // Note that arrow_width > arrow_space is possible, so this can't be f64::clamp.
        let arrow_offset_base = arrow_offset_base.min(arrow_space).max(arrow_width);
        let arrow_offset_active = arrow_offset_active.min(arrow_space).max(arrow_width);
        // Only one arrow can be active at a time.
        let (arrow_offset_forward, arrow_offset_backward) = if forward {
            (arrow_offset_active, arrow_offset_base)
        } else {
            (arrow_offset_base, arrow_offset_active)
        };

        // Construct the affines to get the arrows to their correct locations.
        let arrow_affine_forward = Affine::translate((size.width - arrow_offset_forward, 0.));
        let arrow_affine_backward =
            Affine::reflect((0., 0.), (0., 1.)).then_translate((arrow_offset_backward, 0.).into());

        // Radial gradient located at the tip of the arrow.
        // As the speed grows, more of the arrow will start glowing, spreading from its tip.
        let gradient = sliding.then(|| {
            let radius = (1.5 * speed * arrow_height).max(1.) as f32;
            let gradient = Gradient::new_radial(arrow_tip, radius);
            if backward {
                gradient.with_stops([color_heat.0, color_backward.0])
            } else {
                gradient.with_stops([color_heat.0, color_forward.0])
            }
        });

        if sliding && speed > 0. {
            self.paint_speed_lines(
                painter,
                SpeedLineGeometry {
                    size,
                    speed,
                    arrow_width,
                    arrow_height,
                    arrow_edge_pad,
                    arrow_offset_active,
                    arrow_base_x,
                    extra_y_pad,
                    backward,
                    color: if forward {
                        color_forward.0
                    } else {
                        color_backward.0
                    },
                },
            );
        }

        // Paint the backward facing arrow
        if let Some(gradient) = gradient.as_ref().filter(|_| backward) {
            painter
                .fill(&arrow, gradient)
                .transform(arrow_affine_backward)
                .draw();
        } else {
            // Otherwise with a solid color, potentially showing hover status if not sliding.
            let color = if !sliding && self.hover_backward && ctx.is_hovered() {
                &color_backward.0
            } else {
                &color_content.color
            };
            painter
                .fill(&arrow, *color)
                .transform(arrow_affine_backward)
                .draw();
        }

        // Paint the forward facing arrow
        if let Some(gradient) = gradient.as_ref().filter(|_| forward) {
            painter
                .fill(&arrow, gradient)
                .transform(arrow_affine_forward)
                .draw();
        } else {
            // Otherwise with a solid color, potentially showing hover status if not sliding.
            let color = if !sliding && !self.hover_backward && ctx.is_hovered() {
                &color_forward.0
            } else {
                &color_content.color
            };
            painter
                .fill(&arrow, *color)
                .transform(arrow_affine_forward)
                .draw();
        }
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use std::fmt::Display;

    use super::*;
    use crate::core::{NewWidget, PropertySet, WidgetTag};
    use crate::layout::AsUnit;
    use crate::properties::types::CrossAxisAlignment;
    use crate::properties::{Dimensions, Padding};
    use crate::testing::{TestHarness, assert_render_snapshot};
    use crate::theme::test_property_set;
    use crate::widgets::Flex;

    #[test]
    fn model_updates_preserve_selection_and_refresh_display() {
        let widget = StepInput::new(0, 2, 0, 10)
            .with_display(|state| {
                format!(
                    "{}: {}..={} / {}",
                    state.value, state.min, state.max, state.step
                )
            })
            .prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (240, 40));
        let id = harness.root_id();
        harness.focus_on(Some(id));
        for value in [2, 4] {
            harness.process_text_event(&TextEvent::key_down(Key::Named(NamedKey::ArrowRight)));
            assert_eq!(
                harness.pop_action::<Step<i32>>().unwrap(),
                Some((Step { value }, id))
            );
        }
        harness
            .edit_root_widget(|mut widget| {
                StepInput::set_bounds(&mut widget, 3, 8);
                StepInput::set_step(&mut widget, 3);
            })
            .unwrap();
        let _ = harness.redraw();
        let widget = harness.root_widget().unwrap();
        assert_eq!(
            widget.value, 4,
            "a new range must retain an already valid selection"
        );
        let label_id = widget.label.as_ref().unwrap().id();
        let label = harness
            .get_widget_with_id(label_id)
            .unwrap()
            .downcast::<Label>()
            .unwrap();
        assert_eq!(label.text().as_ref(), "4: 3..=8 / 3");
        assert_eq!(
            harness.access_node(id).unwrap().data().value(),
            Some("4: 3..=8 / 3")
        );
        assert!(
            harness.pop_action_erased().is_none(),
            "model updates do not emit user actions"
        );

        harness.process_text_event(&TextEvent::key_down(Key::Named(NamedKey::ArrowRight)));
        assert_eq!(
            harness.pop_action::<Step<i32>>().unwrap(),
            Some((Step { value: 7 }, id))
        );
        harness.process_text_event(&TextEvent::key_down(Key::Named(NamedKey::End)));
        assert_eq!(
            harness.pop_action::<Step<i32>>().unwrap(),
            Some((Step { value: 8 }, id))
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
                .supports_action(Action::Increment)
        );
        harness
            .edit_root_widget(|mut widget| StepInput::set_wrap(&mut widget, true))
            .unwrap();
        let _ = harness.redraw();
        assert!(
            harness
                .access_node(id)
                .unwrap()
                .data()
                .supports_action(Action::Increment)
        );
    }

    #[test]
    fn constants() {
        const {
            // It is signed for dev ergonomics, but conceptually it is unsigned.
            // Zero also doesn't make any sense.
            assert!(TICKS_PER_STEP >= 1);

            // Lower gears must end sooner
            assert!(GEAR_1_END < GEAR_2_END);
            assert!(GEAR_1_END_STEPS < GEAR_2_END_STEPS);
        }

        // These constants must match watch the function returns
        assert_eq!(
            GEAR_1_END_STEPS,
            StepInput::<u64>::steps(GEAR_1_END).0 as f64
        );
        assert_eq!(
            GEAR_2_END_STEPS,
            StepInput::<u64>::steps(GEAR_2_END).0 as f64
        );
    }

    #[test]
    fn rejects_nonfinite_configuration_and_accepts_physical_unit_steps() {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(matches!(
                StepInput::try_new(0.0, 1.0, invalid, 1.0),
                Err(InvalidStepInput::Bounds { .. })
            ));
            assert!(matches!(
                StepInput::try_new(0.0, 1.0, -1.0, invalid),
                Err(InvalidStepInput::Bounds { .. })
            ));
            assert!(matches!(
                StepInput::try_new(invalid, 1.0, -1.0, 1.0),
                Err(InvalidStepInput::Base(_))
            ));
            assert!(matches!(
                StepInput::try_new(0.0, invalid, -1.0, 1.0),
                Err(InvalidStepInput::Interval(_))
            ));
            assert!(!StepInput::<f64>::valid_interval(invalid));
        }
        for invalid in [0.0, -0.0, -1.0] {
            assert!(matches!(
                StepInput::try_new(0.0, invalid, -1.0, 1.0),
                Err(InvalidStepInput::Interval(_))
            ));
        }
        assert!(matches!(
            StepInput::try_new(1, 1, 2, 3),
            Err(InvalidStepInput::Base(1))
        ));
        assert!(matches!(
            StepInput::try_new(1, 1, 3, 2),
            Err(InvalidStepInput::Bounds { .. })
        ));
        let tiny = f64::from_bits(1);
        let mut widget = StepInput::try_new(0.0, tiny, 0.0, 8.0 * tiny).unwrap();
        for bits in 1..=8 {
            assert!(
                widget.next_step(),
                "small physical quantities must be step-able"
            );
            assert_eq!(widget.value.to_bits(), bits);
        }
        assert!(!widget.next_step());
        let mut widget = StepInput::try_new(0.0_f32, 1e-15, 0.0, 1e-12).unwrap();
        assert!(widget.next_step());
        assert_eq!(widget.value.to_bits(), 1e-15_f32.to_bits());
    }

    #[test]
    fn floating_steps_saturate_without_losing_representable_cancellation() {
        let max = f64::MAX;
        assert_eq!((-max).add_steps(max, 2, None).to_bits(), max.to_bits());
        assert_eq!(max.add_steps(max, -2, None).to_bits(), (-max).to_bits());
        assert_eq!(max.add_steps(max, 1, None).to_bits(), max.to_bits());
        assert_eq!((-max).add_steps(max, -1, None).to_bits(), (-max).to_bits());
        assert_eq!(
            f32::MAX.add_steps(f32::MAX, 1, None).to_bits(),
            f32::MAX.to_bits()
        );
        assert_eq!((-max).steps_to(max, max), 2);
        assert_eq!((-max).steps_to(max, max * 0.5), 4);

        let mut widget = StepInput::new(-max, max, -max, max);
        assert!(widget.next_step());
        assert_eq!(widget.value.to_bits(), 0.0_f64.to_bits());
        assert!(widget.next_step());
        assert_eq!(widget.value.to_bits(), max.to_bits());
        assert!(!widget.next_step());
    }

    #[test]
    fn drag_snap_retains_raw_movement_and_rebases_on_exit() {
        let make = || StepInput::new(1, 3, -30, 30).with_snap(5);
        let mut segmented = make();
        let mut combined = make();
        for expected in [5, 5, 10, 15] {
            let (unused, _, hit_bounds) = segmented.update_ticks(TICKS_PER_STEP, true);
            assert_eq!(
                (unused, hit_bounds),
                (0, false),
                "snap rounding must not feed back into physical motion"
            );
            assert_eq!(segmented.value, expected);
        }
        combined.update_ticks(4 * TICKS_PER_STEP, true);
        assert_eq!(
            segmented.value, combined.value,
            "event segmentation must not affect a snap drag"
        );
        assert_eq!(segmented.ticks, combined.ticks);
        assert_eq!(segmented.ticks, 4 * TICKS_PER_STEP);
        assert_eq!(
            segmented.update_ticks(0, false),
            (0, false, false),
            "leaving snap mode without motion preserves the selection"
        );
        assert_eq!(segmented.base, 15);
        assert_eq!(segmented.ticks, 0);
        assert!(segmented.prev_step());
        assert_eq!(segmented.value, 12);

        combined.finish_slide();
        assert!(!combined.snap_active);
        assert!(combined.next_step());
        assert_eq!(combined.value, 18);
    }

    #[test]
    fn drag_snap_preserves_off_grid_endpoints_and_wraps() {
        let mut widget = StepInput::new(3, 2, 3, 9).with_snap(5).with_wrap(true);
        widget.update_ticks(3 * TICKS_PER_STEP, true);
        assert_eq!(widget.value, 9, "an off-grid maximum remains reachable");
        widget.update_ticks(TICKS_PER_STEP, true);
        assert_eq!(
            widget.value, 3,
            "wrapping selects the actual off-grid minimum"
        );
        widget.update_ticks(-TICKS_PER_STEP, true);
        assert_eq!(
            widget.value, 9,
            "reverse wrapping selects the actual maximum"
        );
        widget.update_ticks(-3 * TICKS_PER_STEP, true);
        assert_eq!(widget.value, 3);
        widget.finish_slide();
        assert!(widget.next_step());
        assert_eq!(widget.value, 5);
    }

    #[test]
    fn pointer_release_applies_final_sample_and_preserves_snapped_selection() {
        use crate::core::keyboard::Modifiers;
        use crate::core::{PointerButtonEvent, PointerState, PointerUpdate};
        use crate::testing::PRIMARY_MOUSE;

        let widget = StepInput::new(1, 3, -100, 100).with_snap(5).prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (200, 40));
        let id = harness.root_id();
        let snap = if cfg!(target_os = "macos") {
            Modifiers::META
        } else {
            Modifiers::CONTROL
        };
        let state = |x| PointerState {
            position: crate::dpi::PhysicalPosition::new(x, 20.0),
            modifiers: snap,
            ..PointerState::default()
        };
        harness.mouse_move((100., 20.));
        harness.process_pointer_event(&PointerEvent::Down(PointerButtonEvent {
            pointer: PRIMARY_MOUSE,
            button: None,
            state: state(100.),
        }));
        harness.process_pointer_event(&PointerEvent::Move(PointerUpdate {
            pointer: PRIMARY_MOUSE,
            current: state(104.),
            coalesced: vec![],
            predicted: vec![],
        }));
        assert_eq!(
            harness.pop_action::<Step<i32>>().unwrap(),
            Some((Step { value: 15 }, id))
        );
        harness.process_pointer_event(&PointerEvent::Up(PointerButtonEvent {
            pointer: PRIMARY_MOUSE,
            button: None,
            state: state(106.),
        }));
        assert_eq!(
            harness.pop_action::<Step<i32>>().unwrap(),
            Some((Step { value: 20 }, id)),
            "final release sample must not be lost"
        );
        assert_eq!(harness.root_widget().unwrap().base, 20);
        harness.process_text_event(&TextEvent::key_down(Key::Named(NamedKey::ArrowLeft)));
        assert_eq!(
            harness.pop_action::<Step<i32>>().unwrap(),
            Some((Step { value: 17 }, id)),
            "ordinary input starts from snapped selection"
        );
    }

    #[test]
    fn snap_navigation_matches_ordered_grid_and_range_endpoints() {
        for min in -5_i32..=5 {
            for max in min..=5 {
                for snap in 1..=7 {
                    let grid: Vec<_> = (min..=max)
                        .filter(|&value| value == min || value == max || value % snap == 0)
                        .collect();
                    for base in min..=max {
                        for step in [1, 3, 11] {
                            for wrap in [false, true] {
                                let make = || {
                                    StepInput::new(base, step, min, max)
                                        .with_snap(snap)
                                        .with_wrap(wrap)
                                };
                                let expected_next = grid
                                    .iter()
                                    .copied()
                                    .find(|&value| value > base)
                                    .unwrap_or(if wrap { min } else { base });
                                let expected_prev = grid
                                    .iter()
                                    .rev()
                                    .copied()
                                    .find(|&value| value < base)
                                    .unwrap_or(if wrap { max } else { base });
                                let mut next = make();
                                assert_eq!(next.next_snap(), expected_next != base);
                                assert_eq!(
                                    next.value, expected_next,
                                    "forward: {min}..={max}, base {base}, step {step}, snap {snap}, wrap {wrap}"
                                );
                                let mut prev = make();
                                assert_eq!(prev.prev_snap(), expected_prev != base);
                                assert_eq!(
                                    prev.value, expected_prev,
                                    "backward: {min}..={max}, base {base}, step {step}, snap {snap}, wrap {wrap}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn snapped_selection_rebases_normal_steps_and_preserves_float_direction() {
        let mut integer = StepInput::new(1, 3, -10, 20).with_snap(5);
        assert!(integer.next_snap());
        assert_eq!(integer.value, 5);
        assert!(integer.next_step());
        assert_eq!(integer.value, 8);
        assert!(integer.prev_snap());
        assert_eq!(integer.value, 5);
        assert!(integer.prev_step());
        assert_eq!(integer.value, 2);

        let mut float = StepInput::new(-0.27_f32, 0.03, -1.0, 1.0).with_snap(0.1);
        for value in [-0.2_f32, -0.1, 0.0, 0.1, 0.2, 0.3] {
            assert!(float.next_snap());
            assert_eq!(float.value.to_bits(), value.to_bits());
        }
        for value in [0.2_f32, 0.1, 0.0, -0.1, -0.2, -0.3] {
            assert!(float.prev_snap());
            assert_eq!(float.value.to_bits(), value.to_bits());
        }
        let mut extreme = StepInput::new(1e300_f64, 1.0, -f64::MAX, f64::MAX).with_snap(1e-300);
        assert!(extreme.next_snap());
        assert_eq!(extreme.value.to_bits(), 1e300_f64.next_up().to_bits());
        assert!(extreme.prev_snap());
        assert_eq!(extreme.value.to_bits(), 1e300_f64.to_bits());
    }

    #[test]
    fn snapping_and_decimal_display_preserve_numeric_domain() {
        for value in [1e300_f64, -1e300, f64::MAX, f64::MIN] {
            assert_eq!(
                value.add_steps(1.0, 0, Some(1e-300)).to_bits(),
                value.to_bits()
            );
        }
        assert_eq!(0.25_f64.display(0.25, 0.0, 1.0), "0.25");
        assert_eq!(2.5_f64.display(2.5, 0.0, 10.0), "2.5");
        assert_eq!(0.1_f32.display(0.1, 0.0, 1.0), "0.1");
        assert_eq!(
            (-0.25_f64).add_steps(1.0, 0, Some(0.5)).to_bits(),
            (-0.5_f64).to_bits()
        );
        assert_eq!(
            0.25_f64.add_steps(1.0, 0, Some(0.5)).to_bits(),
            0.5_f64.to_bits()
        );
    }

    #[test]
    fn add_steps() {
        fn assert_add<T: Steppable + Display>(
            base: T,
            step: T,
            snap: Option<T>,
            count: i64,
            expected: T,
        ) {
            let result = base.add_steps(step, count, snap);
            if result != expected {
                panic!(
                    "base {base} \
                    step {step} \
                    count {count} \
                    snap {snap:?} \
                    expected {expected} \
                    got {result}"
                );
            }
        }

        // Signed ints
        assert_add::<i8>(-128, 1, None, 0, -128);
        assert_add::<i8>(-128, 1, None, 1, -127);
        assert_add::<i8>(-128, 1, None, 128, 0);
        assert_add::<i8>(-128, 1, None, 129, 1);
        assert_add::<i8>(-128, 1, None, 254, 126);
        assert_add::<i8>(-128, 1, None, 255, 127);
        assert_add::<i8>(-128, 1, None, 1000, 127);
        assert_add::<i8>(-128, 3, None, 10, -98);
        assert_add::<i8>(-10, 3, None, -10, -40);
        assert_add::<i8>(0, 1, None, 1, 1);
        assert_add::<i8>(0, 1, None, 10, 10);
        assert_add::<i8>(0, 1, None, 200, 127);
        assert_add::<i8>(126, 1, None, 1, 127);
        assert_add::<i8>(126, 1, None, 2, 127);

        assert_add::<i8>(-122, 2, Some(10), -2, -128);
        assert_add::<i8>(-122, 2, Some(10), -1, -120);
        assert_add::<i8>(-50, 2, Some(10), -2, -50);
        assert_add::<i8>(-50, 2, Some(10), -3, -60);
        assert_add::<i8>(50, 2, Some(10), 2, 50);
        assert_add::<i8>(50, 2, Some(10), 3, 60);
        assert_add::<i8>(121, 2, Some(10), 1, 120);
        assert_add::<i8>(121, 2, Some(10), 2, 127);

        // Unsigned ints
        assert_add::<u8>(0, 1, None, 0, 0);
        assert_add::<u8>(0, 1, None, 5, 5);
        assert_add::<u8>(0, 1, None, 255, 255);
        assert_add::<u8>(0, 1, None, 1000, 255);
        assert_add::<u8>(100, 1, None, 100, 200);
        assert_add::<u8>(100, 3, None, 10, 130);
        assert_add::<u8>(254, 3, None, 1, 255);
        assert_add::<u8>(255, 1, None, 1, 255);
        assert_add::<u8>(255, 1, None, -1, 254);
        assert_add::<u8>(255, 1, None, -100, 155);
        assert_add::<u8>(255, 1, None, -1000, 0);

        assert_add::<u8>(2, 2, Some(10), 1, 0);
        assert_add::<u8>(50, 2, Some(10), 2, 50);
        assert_add::<u8>(50, 2, Some(10), 3, 60);
        assert_add::<u8>(251, 2, Some(10), 1, 250);
        assert_add::<u8>(251, 2, Some(10), 2, 255);

        // Floats
        assert_add::<f64>(-100., 0.1, None, 100, -90.);
        assert_add::<f64>(-100., 0.0001, None, 100000, -90.);
        assert_add::<f64>(-100., 0.0001, None, 1000000, 0.);
        assert_add::<f64>(-100., 0.0001, None, 10000000, 900.);

        assert_add::<f64>(-50., 0.1, Some(10.), -20, -50.);
        assert_add::<f64>(-50., 0.1, Some(10.), -60, -60.);
        assert_add::<f64>(50., 0.1, Some(10.), 20, 50.);
        assert_add::<f64>(50., 0.1, Some(10.), 60, 60.);
    }

    #[test]
    fn steps_to() {
        fn assert_count<T: Steppable + Display>(base: T, step: T, target: T, expected: u64) {
            let result = base.steps_to(target, step);
            if result != expected {
                panic!("base {base} step {step} target {target} expected {expected} got {result}");
            }
        }

        // Signed ints
        assert_count::<i8>(-128, 1, -120, 8);
        assert_count::<i8>(-128, 1, 0, 128);
        assert_count::<i8>(-128, 1, 1, 129);
        assert_count::<i8>(-128, 1, 127, 255);
        assert_count::<i8>(-128, 3, -120, 3);
        assert_count::<i8>(0, 1, 127, 127);
        assert_count::<i8>(50, 2, 60, 5);

        // Unsigned ints
        assert_count::<u8>(0, 1, 10, 10);
        assert_count::<u8>(0, 2, 10, 5);
        assert_count::<u8>(5, 2, 10, 3);
        assert_count::<u8>(5, 2, 10, 3);
        assert_count::<u8>(5, 2, 5, 0);
        assert_count::<u8>(0, 1, 255, 255);

        assert_count::<u64>(0, 1, u64::MAX, u64::MAX);

        // Floats
        assert_count::<f64>(-5., 0.1, -3.2, 18);
        assert_count::<f64>(-5., 0.1, 5., 100);
        assert_count::<f64>(5., 0.1, 10., 50);

        assert_count::<f64>(f64::MIN, 1., f64::MAX, u64::MAX);
    }

    #[test]
    fn tick_bounds() {
        #[track_caller]
        fn assert_bounds(base: u64, step: u64, min: u64, max: u64, min_ticks: i64, max_ticks: i64) {
            let si = StepInput::new(base, step, min, max);
            assert!(
                si.min_ticks == min_ticks && si.max_ticks == max_ticks,
                "base: {base}, step: {step}, min: {min}, max: {max} \
                expected ({min_ticks}, {max_ticks}) got ({},{})",
                si.min_ticks,
                si.max_ticks,
            );
        }

        // At the time of writing, this test passes with various TICKS_PER_STEP configurations.
        // When only testing with a single value, certain integer boundary scenarios disappear.
        // Thus, when making significant changes to tick bounds calculations, it would be useful
        // to run this test with various TICKS_PER_STEP configurations, e.g. 1, 3, 7, 10.

        const TPS: i64 = TICKS_PER_STEP;

        // Simple bounds at start, middle, end
        assert_bounds(0, 1, 0, 10, 0, 10 * TPS);
        assert_bounds(5, 1, 0, 10, -5 * TPS, 5 * TPS);
        assert_bounds(10, 1, 0, 10, -10 * TPS, 0);

        // Different step values
        assert_bounds(4, 2, 0, 20, -2 * TPS, 8 * TPS);
        assert_bounds(8, 4, 0, 20, -2 * TPS, 3 * TPS);

        // Max steps representable as a signed tick
        let max_steps = i64::MAX / TPS;

        // Max distance is beyond what the signed ticks can contain, ticks clamped at MAX
        assert_bounds(0, 1, 0, u64::MAX, 0, i64::MAX);
        // Max distance is still beyond signed potential, ticks clamped at MAX
        assert_bounds(0, 1, 0, u64::MAX / TPS as u64, 0, i64::MAX);
        // Max distance is near or at the edge of signed potential, depending on TPS
        assert_bounds(0, 1, 0, max_steps as u64, 0, max_steps * TPS);
        // Max distance is just slightly beyond signed potential, ticks clamped at MAX
        assert_bounds(0, 1, 0, max_steps as u64 + 1, 0, i64::MAX);
        // Max distance is guaranteed below signed potential
        assert_bounds(0, 1, 0, max_steps as u64 - 1, 0, (max_steps - 1) * TPS);

        // Min steps representable as a signed tick
        let min_steps = i64::MIN / TPS;
        let min_steps_neg = min_steps.wrapping_neg() as u64;

        // Min distance is beyond what the signed ticks can contain, ticks clamped at MIN
        assert_bounds(u64::MAX, 1, 0, u64::MAX, i64::MIN, 0);
        // Min distance is still beyond signed potential, ticks clamped at MIN
        assert_bounds(
            u64::MAX / TPS as u64,
            1,
            0,
            u64::MAX / TPS as u64,
            i64::MIN,
            0,
        );
        // Min distance is near or at the edge of signed potential, depending on TPS
        assert_bounds(min_steps_neg, 1, 0, min_steps_neg, min_steps * TPS, 0);
        // Min distance is just slightly beyond signed potential, ticks clamped at MIN
        assert_bounds(min_steps_neg + 1, 1, 0, min_steps_neg + 1, i64::MIN, 0);
        // Min distance is guaranteed below signed potential
        assert_bounds(
            min_steps_neg - 1,
            1,
            0,
            min_steps_neg - 1,
            (min_steps + 1) * TPS,
            0,
        );
    }

    #[test]
    fn steps_and_distance() {
        fn assert_sync(steps: f64) {
            // Calculate the distance covered by these steps
            let distance = StepInput::<u64>::distance(steps);

            // Calculate expected steps and drift from the input steps
            let direction = steps.signum();
            let steps = steps.abs();

            let expected_steps = (steps.round() * direction) as i64;
            let expected_drift = if steps > GEAR_2_END_STEPS {
                (steps - steps.round()) / GEAR_3_RATIO
            } else if steps > GEAR_1_END_STEPS {
                (steps - steps.round()) / GEAR_2_RATIO
            } else {
                (steps - steps.round()) / GEAR_1_RATIO
            } * direction;

            // Make sure the steps function meets expectations
            let result = StepInput::<u64>::steps(distance);

            assert!(
                result.0 == expected_steps && (result.1 - expected_drift).abs() <= 1e-10,
                "assert_sync({}) => \
				for {distance} expected ({expected_steps}, {expected_drift}), got {result:?}",
                steps * direction,
            );
        }

        let test_steps = [
            0.0,
            0.01,
            0.1,
            0.4,
            0.6,
            0.99,
            1.0,
            1.1,
            5.01,
            GEAR_1_END_STEPS - 0.1,
            GEAR_1_END_STEPS - 0.01,
            GEAR_1_END_STEPS,
            GEAR_1_END_STEPS + 0.01,
            GEAR_1_END_STEPS + 0.1,
            GEAR_1_END_STEPS + 0.4,
            GEAR_1_END_STEPS + 0.6,
            GEAR_1_END_STEPS + 0.99,
            GEAR_1_END_STEPS + 1.0,
            GEAR_1_END_STEPS + 1.1,
            GEAR_1_END_STEPS + 5.01,
            GEAR_2_END_STEPS - 0.1,
            GEAR_2_END_STEPS - 0.01,
            GEAR_2_END_STEPS,
            GEAR_2_END_STEPS + 0.01,
            GEAR_2_END_STEPS + 0.1,
            GEAR_2_END_STEPS + 0.4,
            GEAR_2_END_STEPS + 0.6,
            GEAR_2_END_STEPS + 0.99,
            GEAR_2_END_STEPS + 1.0,
            GEAR_2_END_STEPS + 1.1,
            GEAR_2_END_STEPS + 5.01,
        ];

        for steps in test_steps {
            assert_sync(steps);
            assert_sync(-steps);
        }
    }

    #[test]
    fn wrapping_large_deltas_are_constant_time() {
        let mut widget = StepInput::new(0_u32, 1, 0, 2).with_wrap(true);
        let cycle = 3 * TICKS_PER_STEP;
        let delta = i64::MAX - (i64::MAX % cycle);
        assert_eq!(widget.update_ticks(delta, false), (0, false, false));
        assert_eq!(widget.value, 0);
        assert_eq!(widget.update_ticks(-delta, false), (0, false, false));
        assert_eq!(widget.value, 0);
    }

    #[test]
    fn update_ticks() {
        #[derive(Debug, Copy, Clone)]
        struct InitialState {
            value: u64,
            step: u64,
            min: u64,
            max: u64,
            wrap: bool,
        }

        #[track_caller]
        fn assert_update(
            initial_state: InitialState,
            delta: i64,
            delta_unused: i64,
            new_value: Option<u64>,
            new_ticks: i64,
        ) {
            let mut si = StepInput::new(
                initial_state.value,
                initial_state.step,
                initial_state.min,
                initial_state.max,
            );
            si.wrap = initial_state.wrap;
            let result = si.update_ticks(delta, false);
            let result_new_value = result.1.then_some(si.value);

            assert!(
                result.0 == delta_unused && result_new_value == new_value && si.ticks == new_ticks,
                "{initial_state:?}, delta: {delta}, \
                expected (got) \
                delta_unused: {delta_unused} ({}), \
                new_value: {new_value:?} ({:?}), \
                new_ticks: {new_ticks} ({})",
                result.0,
                result_new_value,
                si.ticks,
            );
        }

        const TPS: i64 = TICKS_PER_STEP;

        let mut is = InitialState {
            value: 0,
            step: 5,
            min: 0,
            max: 50,
            wrap: false,
        };

        // Test negative unused delta overflow protection
        assert_update(is, i64::MIN, i64::MIN, None, 0);
        // One step back
        assert_update(is, -TPS, -TPS, None, 0);
        // Single tick back
        if TPS >= 3 {
            assert_update(is, -1, -1, None, 0);
        }
        // No real change at all
        assert_update(is, 0, 0, None, 0);
        // No changed value due to rounding down
        if TPS >= 2 {
            assert_update(is, TPS / 2 - 1, 0, None, TPS / 2 - 1);
        }
        // One step forward due to rounding ticks up
        assert_update(is, TPS / 2 + 1, 0, Some(5), TPS / 2 + 1);
        // One step forward due to rounding ticks up
        if TPS >= 2 {
            assert_update(is, TPS - 1, 0, Some(5), TPS - 1);
        }
        // One step forward
        assert_update(is, TPS, 0, Some(5), TPS);
        // One step forward due to rounding ticks down
        if TPS >= 3 {
            assert_update(is, TPS + 1, 0, Some(5), TPS + 1);
        }
        // Two steps forward due to rounding ticks down
        if TPS >= 2 {
            assert_update(is, TPS * 2 - 1, 0, Some(10), TPS * 2 - 1);
        }
        // Two steps forward
        assert_update(is, TPS * 2, 0, Some(10), TPS * 2);
        // Ten steps forward, to the max edge
        assert_update(is, TPS * 10, 0, Some(50), TPS * 10);
        // Ten steps + one tick forward, clamped at max edge
        assert_update(is, TPS * 10 + 1, 1, Some(50), TPS * 10);
        // Elevent steps forward, clamped at max edge
        assert_update(is, TPS * 11, TPS, Some(50), TPS * 10);

        // Test difficult int boundaries.
        // We set the value to the midpoint, so that the distance
        // to min is the signed minimum and that the distance
        // to max is the signed maximum.
        let mid = i64::MAX as u64 + 1;
        is = InitialState {
            value: mid,
            step: 1,
            min: u64::MIN,
            max: u64::MAX,
            wrap: false,
        };

        // Expected value might be off by one due to rounding
        fn rounding(ticks: i64) -> u64 {
            let remainder = (ticks % TPS).unsigned_abs();
            if remainder > 0 && remainder * 2 >= TPS as u64 {
                1
            } else {
                0
            }
        }

        let min_effect = (i64::MIN / TPS).unsigned_abs();

        // Minimum possible distance
        assert_update(is, i64::MIN, 0, Some(mid - min_effect), i64::MIN);
        // One step less than minimum
        assert_update(
            is,
            i64::MIN + TPS,
            0,
            Some(mid - min_effect + 1 - rounding(i64::MIN + TPS)),
            i64::MIN + TPS,
        );
        // Two steps less than minimum
        assert_update(
            is,
            i64::MIN + 2 * TPS,
            0,
            Some(mid - min_effect + 2 - rounding(i64::MIN + 2 * TPS)),
            i64::MIN + 2 * TPS,
        );

        // One step back from mid
        assert_update(is, -TPS, 0, Some(mid - 1), -TPS);
        // One step forward from mid
        assert_update(is, TPS, 0, Some(mid + 1), TPS);

        let max_effect = (i64::MAX / TPS) as u64;

        // Two steps less than maximum
        assert_update(
            is,
            i64::MAX - 2 * TPS,
            0,
            Some(mid + max_effect - 2 + rounding(i64::MAX - 2 * TPS)),
            i64::MAX - 2 * TPS,
        );
        // One step less than maximum
        assert_update(
            is,
            i64::MAX - TPS,
            0,
            Some(mid + max_effect - 1 + rounding(i64::MAX - TPS)),
            i64::MAX - TPS,
        );
        // Maximum possible distance
        assert_update(is, i64::MAX, 0, Some(mid + max_effect), i64::MAX);

        // Test wrapping forward
        is = InitialState {
            value: u64::MAX - 1,
            step: 1,
            min: u64::MIN,
            max: u64::MAX,
            wrap: true,
        };

        // Step to the max edge
        assert_update(is, TPS, 0, Some(u64::MAX), TPS);
        // Step to the max edge plus one tick
        if TPS >= 2 {
            assert_update(is, TPS + 1, 1, Some(u64::MAX), TPS);
        }
        // Wrap around to min edge
        assert_update(is, 2 * TPS, 0, Some(0), 0);
        // Wrap around to min edge plus another step
        assert_update(is, 3 * TPS, 0, Some(1), TPS);

        // Test wrapping backwards
        is = InitialState {
            value: 1,
            step: 1,
            min: u64::MIN,
            max: u64::MAX,
            wrap: true,
        };

        // Step to the min edge
        assert_update(is, -TPS, 0, Some(0), -TPS);
        // Step to the min edge plus one tick
        if TPS >= 2 {
            assert_update(is, -TPS - 1, -1, Some(0), -TPS);
        }
        // Wrap around to max edge
        assert_update(is, -2 * TPS, 0, Some(u64::MAX), 0);
        // Wrap around to max edge plus another step
        assert_update(is, -3 * TPS, 0, Some(u64::MAX - 1), -TPS);

        // Test multi-wrap
        is = InitialState {
            value: 10,
            step: 1,
            min: 0,
            max: 20,
            wrap: true,
        };

        // Wrap around to min edge
        assert_update(is, 11 * TPS, 0, Some(0), 0);
        // Wrap around to min edge 2x
        assert_update(is, 32 * TPS, 0, Some(0), 0);
        // Wrap around to min edge 3x + 2 steps
        assert_update(is, 55 * TPS, 0, Some(2), 2 * TPS);

        // Wrap around to max edge
        assert_update(is, -11 * TPS, 0, Some(20), 0);
        // Wrap around to max edge 2x
        assert_update(is, -32 * TPS, 0, Some(20), 0);
        // Wrap around to max edge 3x + 2 steps
        assert_update(is, -55 * TPS, 0, Some(18), -2 * TPS);
    }

    #[test]
    fn basics() {
        let si = |base, props| {
            StepInput::new(base, 1, 0, usize::MAX)
                .prepare()
                .with_props(props)
        };

        let root = Flex::column()
            .cross_axis_alignment(CrossAxisAlignment::Start)
            .with_fixed(si(0, (StepInputStyle::Basic, Dimensions::MIN)))
            .with_fixed(si(0, (StepInputStyle::Flow, Dimensions::MIN)))
            .with_fixed(si(0, (StepInputStyle::Basic, Dimensions::MAX)))
            .with_fixed(si(0, (StepInputStyle::Flow, Dimensions::MAX)))
            .with_fixed(si(0, (StepInputStyle::Basic, Dimensions::width(100.px()))))
            .with_fixed(si(0, (StepInputStyle::Flow, Dimensions::width(100.px()))))
            .with_fixed(si(500, (StepInputStyle::Basic, Dimensions::MIN)))
            .with_fixed(si(500, (StepInputStyle::Flow, Dimensions::MIN)))
            .with_fixed(si(500, (StepInputStyle::Basic, Dimensions::MAX)))
            .with_fixed(si(500, (StepInputStyle::Flow, Dimensions::MAX)))
            .with_fixed(si(
                500,
                (StepInputStyle::Basic, Dimensions::width(100.px())),
            ))
            .with_fixed(si(500, (StepInputStyle::Flow, Dimensions::width(100.px()))))
            .prepare()
            .with_props(Padding::all(10.px()));

        let mut harness = TestHarness::create_with_size(test_property_set(), root, (150, 525));

        assert_render_snapshot!(harness, "step_input_basics");
    }

    #[test]
    fn speed_lines() {
        let si = |tag| {
            NewWidget::new(StepInput::new(5000, 1, 0, usize::MAX))
                .with_tag(tag)
                .with_props((StepInputStyle::Flow, Dimensions::fixed(250.px(), 85.px())))
        };

        let tag_backward = WidgetTag::unique();
        let tag_forward = WidgetTag::unique();

        let lines_backward = si(tag_backward);
        let lines_forward = si(tag_forward);

        let root = Flex::column()
            .cross_axis_alignment(CrossAxisAlignment::Start)
            .with_fixed(lines_backward)
            .with_fixed(lines_forward)
            .prepare()
            .with_props(Padding::all(10.px()));

        let mut harness = TestHarness::create_with_size(test_property_set(), root, (270, 200));

        harness
            .edit_widget(tag_backward, |mut widget| {
                widget.widget.drag_start = Some(Point::new(10000., 0.));
                widget.widget.slide_last = Some(Point::ORIGIN);
                widget.ctx.request_paint_only();
            })
            .unwrap();

        harness
            .edit_widget(tag_forward, |mut widget| {
                widget.widget.drag_start = Some(Point::new(10000., 0.));
                widget.widget.slide_last = Some(Point::new(20000., 0.));
                widget.ctx.request_paint_only();
            })
            .unwrap();

        assert_render_snapshot!(harness, "step_input_speed_lines");
    }

    #[test]
    fn awkward_layout() {
        let basic = |base, props: PropertySet| {
            let props = props.with(StepInputStyle::Basic);
            StepInput::new(base, 1, 0, usize::MAX)
                .prepare()
                .with_props(props)
        };
        let flow = |base, props: PropertySet| {
            let props = props.with(StepInputStyle::Flow);
            StepInput::new(base, 1, 0, usize::MAX)
                .prepare()
                .with_props(props)
        };

        let root = Flex::column()
            .cross_axis_alignment(CrossAxisAlignment::Start)
            // Below MinContent size to test that controls don't overflow out of the widget.
            .with_fixed(basic(1234, (Dimensions::fixed(50.px(), 30.px())).into()))
            .with_fixed(flow(1234, (Dimensions::fixed(50.px(), 30.px())).into()))
            // Zero padding to test that controls don't touch borders.
            .with_fixed(basic(
                1234,
                (Padding::ZERO, Dimensions::fixed(100.px(), 18.px())).into(),
            ))
            .with_fixed(flow(
                1234,
                (Padding::ZERO, Dimensions::fixed(100.px(), 18.px())).into(),
            ))
            // Extra wide widget to test that controls remain reasonably sized.
            .with_fixed(basic(1234, (Dimensions::fixed(300.px(), 30.px())).into()))
            .with_fixed(flow(1234, (Dimensions::fixed(300.px(), 30.px())).into()))
            // Extra high widget to test that controls remain reasonably sized.
            .with_fixed(basic(1234, (Dimensions::fixed(100.px(), 200.px())).into()))
            .with_fixed(flow(1234, (Dimensions::fixed(100.px(), 200.px())).into()))
            .prepare()
            .with_props(Padding::all(10.px()));

        let mut harness = TestHarness::create_with_size(test_property_set(), root, (320, 650));

        assert_render_snapshot!(harness, "step_input_awkward_layout");
    }
}
