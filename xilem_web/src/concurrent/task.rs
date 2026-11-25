// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;
use std::rc::Rc;

use futures_channel::oneshot;
use futures_util::FutureExt;
use wasm_bindgen_futures::spawn_local;

use crate::ViewCtx;
use crate::context::MessageThunk;
use crate::core::anymore::AnyDebug;
use crate::core::{MessageCtx, MessageResult, Mut, NoElement, View, ViewMarker};

/// Spawn an async task to update state asynchronously
///
/// The `init_future` function is given a [`TaskProxy`] and a [`ShutdownSignal`].
/// The `TaskProxy` can be used to send a message to `on_event`, which can then update
/// the app's state.
/// The `ShutdownSignal` can be used to detect whether the view has disappeared and
/// a shutdown request has been made. This task uses cooperative shutdown so the future
/// can perform asynchronous cleanup. Messages are rejected once the state is removed.
///
/// Note that this task will not be updated if the view is rebuilt, so `init_future`
/// cannot capture.
pub fn task<M, F, H, State, Action, Fut>(init_future: F, on_event: H) -> Task<F, H, M>
where
    F: Fn(TaskProxy, ShutdownSignal) -> Fut + 'static,
    Fut: Future<Output = ()> + 'static,
    H: Fn(&mut State, M) -> Action + 'static,
    M: AnyDebug,
    State: 'static,
{
    const {
        assert!(
            size_of::<F>() == 0,
            "`task` will not be ran again when its captured variables are updated.\n\
            To ignore this warning, use `task_raw`."
        );
    };
    Task {
        init_future,
        on_event,
        message: PhantomData,
    }
}

/// Spawn a future.
///
/// This is [`task`] without the capturing rules.
/// See `task` for full documentation.
pub fn task_raw<M, F, H, State, Action, Fut>(init_future: F, on_event: H) -> Task<F, H, M>
where
    F: Fn(TaskProxy, ShutdownSignal) -> Fut + 'static,
    Fut: Future<Output = ()> + 'static,
    H: Fn(&mut State, M) -> Action + 'static,
    State: 'static,
{
    Task {
        init_future,
        on_event,
        message: PhantomData,
    }
}

struct AbortHandle {
    abort_tx: oneshot::Sender<()>,
}

impl AbortHandle {
    fn abort(self) {
        let _ = self.abort_tx.send(());
    }
}

pub struct ShutdownSignal {
    shutdown_rx: oneshot::Receiver<()>,
}

impl ShutdownSignal {
    fn new() -> (Self, AbortHandle) {
        let (abort_tx, shutdown_rx) = oneshot::channel();
        (Self { shutdown_rx }, AbortHandle { abort_tx })
    }

    /// Detect whether the view has disappeared and
    /// a shutdown request has been made.
    pub fn should_shutdown(&mut self) -> bool {
        match self.shutdown_rx.try_recv() {
            Ok(Some(())) | Err(oneshot::Canceled) => true,
            Ok(None) => false,
        }
    }

    /// Transform the signal into a future
    /// that resolves if a shutdown request has been made.
    pub fn into_future(self) -> impl Future<Output = ()> {
        self.shutdown_rx.map(|_| ())
    }
}

pub struct Task<F, H, M> {
    init_future: F,
    on_event: H,
    message: PhantomData<fn() -> M>,
}

#[expect(
    unnameable_types,
    reason = "Implementation detail, public because of trait visibility rules"
)]
pub struct TaskState {
    abort_handle: Option<AbortHandle>,
    active: Rc<Cell<bool>>,
}

impl TaskState {
    fn shutdown(&mut self) {
        self.active.set(false);
        if let Some(handle) = self.abort_handle.take() {
            handle.abort();
        }
    }
}

impl Drop for TaskState {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub struct TaskProxy {
    thunk: Rc<MessageThunk>,
    active: Rc<Cell<bool>>,
}

impl TaskProxy {
    pub fn send_message<M>(&self, message: M)
    where
        M: AnyDebug,
    {
        if !self.active.get() {
            return;
        }
        let thunk = Rc::clone(&self.thunk);
        let active = Rc::clone(&self.active);
        spawn_local(async move {
            if active.get() {
                thunk.push_message(message);
            }
        });
    }
}

impl<F, H, M> ViewMarker for Task<F, H, M> {}

impl<State, Action, F, H, M, Fut> View<State, Action, ViewCtx> for Task<F, H, M>
where
    State: 'static,
    Action: 'static,
    F: Fn(TaskProxy, ShutdownSignal) -> Fut + 'static,
    Fut: Future<Output = ()> + 'static,
    H: Fn(&mut State, M) -> Action + 'static,
    M: AnyDebug,
{
    type Element = NoElement;

    type ViewState = TaskState;

    fn build(&self, ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        let thunk = ctx.message_thunk();
        let (shutdown_signal, abort_handle) = ShutdownSignal::new();
        let active = Rc::new(Cell::new(true));
        let view_state = TaskState {
            abort_handle: Some(abort_handle),
            active: Rc::clone(&active),
        };
        let proxy = TaskProxy {
            thunk: Rc::new(thunk),
            active,
        };
        spawn_local((self.init_future)(proxy, shutdown_signal));
        (NoElement, view_state)
    }

    fn rebuild(
        &self,
        _: &Self,
        _: &mut Self::ViewState,
        _: &mut ViewCtx,
        (): Mut<'_, Self::Element>,
        _: &mut State,
    ) {
        // Nothing to do
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        _: &mut ViewCtx,
        (): Mut<'_, Self::Element>,
    ) {
        view_state.shutdown();
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageCtx,
        (): Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        if !view_state.active.get() || !message.remaining_path().is_empty() {
            return MessageResult::Stale;
        }
        let Some(message) = message.take_message::<M>() else {
            return MessageResult::Stale;
        };
        MessageResult::Action((self.on_event)(app_state, *message))
    }
}
