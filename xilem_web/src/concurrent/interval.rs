// Copyright 2024 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

use std::marker::PhantomData;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::core::{MessageCtx, MessageResult, Mut, NoElement, View, ViewMarker};
use crate::{OptionalAction, ViewCtx};

/// Start an interval which invokes `callback` every `ms` milliseconds
pub struct Interval<Callback, State, Action> {
    ms: u32,
    callback: Callback,
    phantom: PhantomData<fn() -> (State, Action)>,
}

/// Start an interval which invokes `callback` every `ms` milliseconds
///
/// Currently, when `ms` changes, the previous interval is cleared, and starts with the new interval.
/// This default behavior may change in the future, and may even be configurable.
///
/// # Examples
///
/// ```
/// use xilem_web::{core::fork, concurrent::interval, elements::html::div, interfaces::Element};
///
/// fn timer(seconds: &mut u32) -> impl Element<u32> {
///     fork(
///         div(format!("{seconds} seconds have passed, since creating this view")),
///         interval(
///             1000, // in ms, when this changes, the interval is reset
///             |seconds: &mut u32| *seconds += 1,
///         )
///     )
/// }
/// ```
///
/// Browser delays above the signed 32-bit millisecond limit are clamped.
pub fn interval<State, Action, OA, Callback>(
    ms: u32,
    callback: Callback,
) -> Interval<Callback, State, Action>
where
    State: 'static,
    Action: 'static,
    OA: OptionalAction<Action> + 'static,
    Callback: Fn(&mut State) -> OA + 'static,
{
    Interval {
        ms,
        callback,
        phantom: PhantomData,
    }
}

#[expect(
    unnameable_types,
    reason = "Implementation detail, public because of trait visibility rules"
)]
pub struct IntervalState {
    registration: Option<IntervalRegistration>,
    generation: u64,
}

struct IntervalRegistration {
    window: web_sys::Window,
    handle: i32,
    _callback: Closure<dyn FnMut()>,
}

impl Drop for IntervalRegistration {
    fn drop(&mut self) {
        self.window.clear_interval_with_handle(self.handle);
    }
}

#[derive(Debug)]
struct Tick(u64);

impl IntervalState {
    fn restart(&mut self, ctx: &ViewCtx, ms: u32) {
        self.registration = None;
        self.generation = self.generation.wrapping_add(1);
        let Some(window) = ctx.document().default_view() else {
            web_sys::console::warn_1(
                &"Cannot start an interval in a document without a window".into(),
            );
            return;
        };
        let thunk = ctx.message_thunk();
        let generation = self.generation;
        let callback = Closure::new(move || thunk.push_message(Tick(generation)));
        let delay = i32::try_from(ms).unwrap_or(i32::MAX);
        match window.set_interval_with_callback_and_timeout_and_arguments_0(
            callback.as_ref().unchecked_ref(),
            delay,
        ) {
            Ok(handle) => {
                self.registration = Some(IntervalRegistration {
                    window,
                    handle,
                    _callback: callback,
                });
            }
            Err(error) => {
                web_sys::console::warn_2(&"Could not start browser interval".into(), &error);
            }
        }
    }
}

impl<Callback, State, Action> ViewMarker for Interval<Callback, State, Action> {}

impl<State, Action, Callback, OA> View<State, Action, ViewCtx> for Interval<Callback, State, Action>
where
    State: 'static,
    Action: 'static,
    OA: OptionalAction<Action> + 'static,
    Callback: Fn(&mut State) -> OA + 'static,
{
    type Element = NoElement;

    type ViewState = IntervalState;

    fn build(&self, ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        let mut state = IntervalState {
            registration: None,
            generation: 0,
        };
        state.restart(ctx, self.ms);
        (NoElement, state)
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        (): Mut<'_, Self::Element>,
        _: &mut State,
    ) {
        if prev.ms != self.ms {
            view_state.restart(ctx, self.ms);
        }
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        _: &mut ViewCtx,
        (): Mut<'_, Self::Element>,
    ) {
        view_state.registration = None;
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageCtx,
        (): Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        if view_state.registration.is_none()
            || message
                .maybe_take_message::<Tick>(|tick| tick.0 == view_state.generation)
                .is_none()
        {
            return MessageResult::Stale;
        }
        (self.callback)(app_state)
            .action()
            .map_or(MessageResult::Nop, MessageResult::Action)
    }
}
