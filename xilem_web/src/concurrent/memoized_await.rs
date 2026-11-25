// Copyright 2024 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

use std::future::Future;
use std::marker::PhantomData;

use futures_util::future::{AbortHandle, Abortable};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use wasm_bindgen_futures::spawn_local;

use crate::core::{
    MessageCtx, MessageResult, Mut, NoElement, View, ViewId, ViewMarker, ViewPathTracker,
};
use crate::{OptionalAction, ViewCtx};

type MemoizedPhantom<State, Action, OA, F, FOut> =
    PhantomData<fn() -> (State, Action, OA, F, FOut)>;

/// Await a future returned by `init_future` invoked with the argument `data`, `callback` is called with the output of the future. `init_future` will be invoked again, when `data` changes. Use [`memoized_await`] for construction of this [`View`]
pub struct MemoizedAwait<State, Action, OA, InitFuture, Data, Callback, F, FOut> {
    init_future: InitFuture,
    data: Data,
    callback: Callback,
    debounce_ms: usize,
    reset_debounce_on_update: bool,
    phantom: MemoizedPhantom<State, Action, OA, F, FOut>,
}

impl<State, Action, OA, InitFuture, Data, Callback, F, FOut>
    MemoizedAwait<State, Action, OA, InitFuture, Data, Callback, F, FOut>
where
    FOut: std::fmt::Debug + 'static,
    F: Future<Output = FOut> + 'static,
    InitFuture: Fn(&Data) -> F,
{
    /// Debounce the `init_future` function, when `data` updates,
    /// when `reset_debounce_on_update == false` then this throttles updates each `milliseconds`
    ///
    /// The default for this is `0`
    #[must_use = "Use the configured asynchronous view."]
    pub const fn debounce_ms(mut self, milliseconds: usize) -> Self {
        self.debounce_ms = milliseconds;
        self
    }

    /// When `reset` is `true`, every time `data` updates, the debounce timeout is cleared until `init_future` is invoked.
    /// This is only effective when `debounce > 0`
    ///
    /// The default for this is `true`
    #[must_use = "Use the configured asynchronous view."]
    pub const fn reset_debounce_on_update(mut self, reset: bool) -> Self {
        self.reset_debounce_on_update = reset;
        self
    }

    fn init_future(&self, ctx: &mut ViewCtx, generation: u64) -> AwaitPhase {
        ctx.with_id(ViewId::new(generation), |ctx| {
            let thunk = ctx.message_thunk();
            let future = (self.init_future)(&self.data);
            let (handle, registration) = AbortHandle::new_pair();
            spawn_local(async move {
                // Cancellation is expected when a new request supersedes this one.
                let _ = Abortable::new(
                    async move {
                        thunk.push_message(MemoizedAwaitMessage::<FOut>::Output(future.await));
                    },
                    registration,
                )
                .await;
            });
            AwaitPhase::Running {
                _task: RunningFuture(handle),
            }
        })
    }
}

/// Await a future returned by `init_future` invoked with the argument `data`, `callback` is called with the output of the resolved future. `init_future` will be invoked again, when `data` changes.
///
/// The update behavior can be controlled, by [`debounce_ms`](`MemoizedAwait::debounce_ms`) and [`reset_debounce_on_update`](`MemoizedAwait::reset_debounce_on_update`)
///
/// # Examples
///
/// ```
/// use xilem_web::{core::fork, concurrent::memoized_await, elements::html::div, interfaces::Element};
///
/// fn app_logic(state: &mut i32) -> impl Element<i32> {
///     fork(
///         div(*state),
///         memoized_await(
///             10,
///             |count| std::future::ready(*count),
///             |state: &mut i32, output| *state = output,
///         )
///     )
/// }
/// ```
pub fn memoized_await<State, Action, OA, InitFuture, Data, Callback, F, FOut>(
    data: Data,
    init_future: InitFuture,
    callback: Callback,
) -> MemoizedAwait<State, Action, OA, InitFuture, Data, Callback, F, FOut>
where
    State: 'static,
    Action: 'static,
    Data: PartialEq + 'static,
    FOut: std::fmt::Debug + 'static,
    F: Future<Output = FOut> + 'static,
    InitFuture: Fn(&Data) -> F + 'static,
    OA: OptionalAction<Action> + 'static,
    Callback: Fn(&mut State, FOut) -> OA + 'static,
{
    MemoizedAwait {
        init_future,
        data,
        callback,
        debounce_ms: 0,
        reset_debounce_on_update: true,
        phantom: PhantomData,
    }
}

#[derive(Default)]
#[expect(
    unnameable_types,
    reason = "Implementation detail, public because of trait visibility rules"
)]
pub struct MemoizedAwaitState {
    generation: u64,
    phase: AwaitPhase,
}

#[derive(Default)]
enum AwaitPhase {
    #[default]
    Idle,
    Running {
        _task: RunningFuture,
    },
    Scheduled {
        _timer: ScheduledUpdate,
    },
    Ready,
}

struct RunningFuture(AbortHandle);

impl Drop for RunningFuture {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct ScheduledUpdate {
    window: web_sys::Window,
    handle: i32,
    _callback: Closure<dyn FnMut()>,
}

impl Drop for ScheduledUpdate {
    fn drop(&mut self) {
        self.window.clear_timeout_with_handle(self.handle);
    }
}

impl MemoizedAwaitState {
    fn next_generation(&mut self) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect_throw("MemoizedAwait generation exhausted");
    }

    fn schedule_update<FOut: std::fmt::Debug + 'static>(
        &mut self,
        ctx: &mut ViewCtx,
        debounce_duration: usize,
    ) {
        let window = ctx
            .document()
            .default_view()
            .expect_throw("MemoizedAwait requires a window");
        let timeout = i32::try_from(debounce_duration).unwrap_or(i32::MAX);
        ctx.with_id(ViewId::new(self.generation), |ctx| {
            let thunk = ctx.message_thunk();
            let callback = Closure::new(move || {
                thunk.push_message(MemoizedAwaitMessage::<FOut>::ScheduleUpdate);
            });
            let handle = window
                .set_timeout_with_callback_and_timeout_and_arguments_0(
                    callback.as_ref().unchecked_ref(),
                    timeout,
                )
                .unwrap_throw();
            self.phase = AwaitPhase::Scheduled {
                _timer: ScheduledUpdate {
                    window,
                    handle,
                    _callback: callback,
                },
            };
        });
    }
}

#[derive(Debug)]
enum MemoizedAwaitMessage<Output: std::fmt::Debug> {
    Output(Output),
    ScheduleUpdate,
}

impl<State, Action, OA, InitFuture, Data, CB, F, FOut> ViewMarker
    for MemoizedAwait<State, Action, OA, InitFuture, Data, CB, F, FOut>
{
}
impl<State, Action, InitFuture, F, FOut, Data, CB, OA> View<State, Action, ViewCtx>
    for MemoizedAwait<State, Action, OA, InitFuture, Data, CB, F, FOut>
where
    State: 'static,
    Action: 'static,
    OA: OptionalAction<Action> + 'static,
    InitFuture: Fn(&Data) -> F + 'static,
    FOut: std::fmt::Debug + 'static,
    Data: PartialEq + 'static,
    F: Future<Output = FOut> + 'static,
    CB: Fn(&mut State, FOut) -> OA + 'static,
{
    type Element = NoElement;

    type ViewState = MemoizedAwaitState;

    fn build(&self, ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        let mut state = MemoizedAwaitState::default();

        if self.debounce_ms > 0 {
            state.schedule_update::<FOut>(ctx, self.debounce_ms);
        } else {
            state.phase = self.init_future(ctx, state.generation);
        }

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
        let scheduled = matches!(view_state.phase, AwaitPhase::Scheduled { .. });
        let settings_changed = prev.reset_debounce_on_update != self.reset_debounce_on_update
            || prev.debounce_ms != self.debounce_ms;
        let data_changed = prev.data != self.data;
        if matches!(view_state.phase, AwaitPhase::Ready) {
            view_state.phase = self.init_future(ctx, view_state.generation);
            return;
        }
        if scheduled && !settings_changed && (!data_changed || !self.reset_debounce_on_update) {
            // Throttling retains the original timeout; its rebuild reads the latest data.
            return;
        }
        if data_changed || (scheduled && settings_changed) {
            // Invalidate both old future outputs and already-queued timeout messages
            // as soon as a new update is requested, rather than when its future starts.
            view_state.next_generation();
            view_state.phase = AwaitPhase::Idle;
            if self.debounce_ms == 0 {
                view_state.phase = self.init_future(ctx, view_state.generation);
            } else {
                view_state.schedule_update::<FOut>(ctx, self.debounce_ms);
            }
        }
    }

    fn teardown(&self, state: &mut Self::ViewState, _: &mut ViewCtx, (): Mut<'_, Self::Element>) {
        state.phase = AwaitPhase::Idle;
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageCtx,
        (): Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        if message.remaining_path().len() != 1 {
            return MessageResult::Stale;
        }
        let Some(my_id) = message.take_first() else {
            return MessageResult::Stale;
        };

        if my_id.routing_id() == view_state.generation {
            let Some(payload) = message.take_message() else {
                return MessageResult::Stale;
            };
            match *payload {
                MemoizedAwaitMessage::Output(future_output)
                    if matches!(view_state.phase, AwaitPhase::Running { .. }) =>
                {
                    view_state.phase = AwaitPhase::Idle;
                    (self.callback)(app_state, future_output)
                        .action()
                        .map_or(MessageResult::Nop, MessageResult::Action)
                }
                MemoizedAwaitMessage::ScheduleUpdate
                    if matches!(view_state.phase, AwaitPhase::Scheduled { .. }) =>
                {
                    view_state.phase = AwaitPhase::Ready;
                    MessageResult::RequestRebuild
                }
                _ => MessageResult::Stale,
            }
        } else {
            MessageResult::Stale
        }
    }
}
