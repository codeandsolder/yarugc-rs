// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;

use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use crate::ViewCtx;

use crate::core::anymore::AnyDebug;
use crate::core::{
    MessageCtx, MessageProxy, MessageResult, Mut, NoElement, View, ViewId, ViewMarker,
    ViewPathTracker,
};

/// Owns the lifetime of a spawned view task.
///
/// Dropping the view state cancels the task even if normal teardown was skipped.
/// A task that has already finished may still have a message queued in the
/// event loop, so completion alone does not invalidate its route.
#[derive(Debug, Default)]
pub struct TaskLifetime(Option<JoinHandle<()>>);

impl TaskLifetime {
    pub(super) const fn new(handle: JoinHandle<()>) -> Self {
        Self(Some(handle))
    }

    pub(super) const fn is_active(&self) -> bool {
        self.0.is_some()
    }

    pub(super) fn cancel(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

impl Drop for TaskLifetime {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Launch a task which will run until the view is no longer in the tree.
///
/// `init_future` is given a [`MessageProxy`], which it will store in the future it returns.
/// This `MessageProxy` can be used to send a message to `on_event`, which can then update
/// the app's state.
///
/// For example, this can be used with the time functions in `tokio::time` (when enabled by the application).
///
/// The host must keep the supplied runtime alive and driving tasks.
/// The runtime is selected on build and remains fixed until teardown.
///
/// Note that this task will not be updated if the view is rebuilt, so `init_future`
/// cannot capture.
// TODO: More thorough documentation.
/// See [`run_once`](crate::core::run_once) for details.
pub fn task<M, F, H, State, Action, Fut>(
    runtime: &Handle,
    init_future: F,
    on_event: H,
) -> Task<State, Action, F, H, M>
where
    F: Fn(MessageProxy<M>, &mut State) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
    H: Fn(&mut State, M) -> Action + 'static,
    M: AnyDebug + Send + 'static,
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
        runtime: runtime.clone(),
        init_future,
        on_event,
        message: PhantomData,
    }
}

/// Launch a task which will run until the view is no longer in the tree.
///
/// This is [`task`] without the capturing rules.
/// See `task` for full documentation.
pub fn task_raw<M, F, H, State, Action, Fut>(
    runtime: &Handle,
    init_future: F,
    on_event: H,
) -> Task<State, Action, F, H, M>
where
    F: Fn(MessageProxy<M>, &mut State) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
    H: Fn(&mut State, M) -> Action + 'static,
    M: AnyDebug + Send + 'static,
    State: 'static,
{
    Task {
        runtime: runtime.clone(),
        init_future,
        on_event,
        message: PhantomData,
    }
}

/// A view representing a task which will run until the view is no longer in the tree.
pub struct Task<State, Action, F, H, M> {
    runtime: Handle,
    init_future: F,
    on_event: H,
    message: PhantomData<fn(State) -> (Action, M)>,
}

impl<State, Action, F, H, M> ViewMarker for Task<State, Action, F, H, M> {}
impl<State, Action, F, H, M, Fut> View<State, Action, ViewCtx> for Task<State, Action, F, H, M>
where
    State: 'static,
    Action: 'static,
    F: Fn(MessageProxy<M>, &mut State) -> Fut + 'static,
    Fut: Future<Output = ()> + Send + 'static,
    H: Fn(&mut State, M) -> Action + 'static,
    M: AnyDebug + Send + 'static,
{
    type Element = NoElement;

    type ViewState = TaskLifetime;

    fn build(&self, ctx: &mut ViewCtx, state: &mut State) -> (Self::Element, Self::ViewState) {
        let path: Arc<[ViewId]> = ctx.view_path().into();

        let proxy = ctx.proxy();
        let future = {
            let _runtime_context = self.runtime.enter();
            (self.init_future)(MessageProxy::new(proxy, path), state)
        };
        let handle = self.runtime.spawn(future);
        (NoElement, TaskLifetime::new(handle))
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
        join_handle: &mut Self::ViewState,
        _: &mut ViewCtx,
        (): Mut<'_, Self::Element>,
    ) {
        join_handle.cancel();
    }

    fn message(
        &self,
        lifetime: &mut Self::ViewState,
        message: &mut MessageCtx,
        _element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        if !lifetime.is_active() {
            return MessageResult::Stale;
        }
        let Some(message) = message.take_message::<M>() else {
            return MessageResult::Stale;
        };
        MessageResult::Action((self.on_event)(app_state, *message))
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;

    use tokio::sync::oneshot;

    use super::*;

    #[test]
    fn task_runs_on_explicit_host_runtime_without_ambient_context() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let (started_tx, started_rx) = oneshot::channel();
        let mut state = Some(started_tx);
        let mut ctx = crate::test_support::view_context();
        let view = task(
            runtime.handle(),
            |_: MessageProxy<()>, state: &mut Option<oneshot::Sender<()>>| {
                let started = state.take().unwrap();
                let timer = tokio::time::sleep(std::time::Duration::ZERO);
                async move {
                    timer.await;
                    started.send(()).unwrap();
                    pending::<()>().await;
                }
            },
            |_, ()| (),
        );
        let (NoElement, lifetime) = view.build(&mut ctx, &mut state);
        runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(1), started_rx)
                .await
                .unwrap()
                .unwrap();
        });
        assert!(lifetime.is_active(), "The view must own the spawned task");
        drop(lifetime);
    }

    #[test]
    fn dropping_view_state_cancels_task_without_teardown() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (started_tx, started_rx) = oneshot::channel();
            let (dropped_tx, dropped_rx) = oneshot::channel::<()>();
            let handle = tokio::spawn(async move {
                // Dropping the suspended future closes this channel.
                let _lifetime = dropped_tx;
                started_tx.send(()).unwrap();
                pending::<()>().await;
            });
            let lifetime = TaskLifetime::new(handle);
            started_rx.await.unwrap();
            drop(lifetime);
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(1), dropped_rx)
                    .await
                    .unwrap()
                    .is_err(),
                "dropping state must release the suspended task"
            );
        });
    }
}
