// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::future::Future;
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinSet;

use super::TaskLifetime;

use crate::ViewCtx;
use crate::core::anymore::AnyDebug;
use crate::core::{
    Environment, MessageCtx, MessageProxy, MessageResult, Mut, NoElement, Resource, View, ViewId,
    ViewMarker, ViewPathTracker,
};

/// Launch a worker which will run until the view is no longer in the tree.
///
/// The bounded request queue uses the supplied nonzero capacity (capped at
/// Tokio's semaphore limit). UI callbacks should use `Sender::try_send` and
/// handle full or closed queues; asynchronous producers may await `send`.
/// The worker is cancelled on teardown or when its view state is dropped.
///
/// `init_future` is given a [`MessageProxy`], which it will store in the future it returns.
/// This `MessageProxy` can be used to send a message to `on_event`, which can then update
/// the app's state.
///
/// For example, this can be used with the time functions in [`tokio::time`].
///
/// Note that this task will not be updated if the view is rebuilt, so `init_future`
/// cannot capture.
// TODO: More thorough documentation.
/// See [`run_once`](crate::core::run_once) for details.
pub fn worker<F, H, M, S, V, State, Action, Fut>(
    capacity: NonZeroUsize,
    init_future: F,
    store_sender: S,
    on_response: H,
) -> impl View<State, Action, ViewCtx, Element = NoElement>
where
    F: Fn(MessageProxy<M>, Receiver<V>) -> Fut + 'static,
    Fut: Future<Output = ()> + Send + 'static,
    S: Fn(&mut State, Sender<V>) + 'static,
    H: Fn(&mut State, M) -> Action + 'static,
    M: AnyDebug + Send + 'static,
    State: 'static,
    Action: 'static,
    V: Send + 'static,
{
    const {
        assert!(
            size_of::<F>() == 0,
            "`worker` will not be ran again when its captured variables are updated.\n\
            To ignore this warning, use `worker_raw`."
        );
    };
    worker_raw(capacity, init_future, store_sender, on_response)
}

/// A version of [`worker`] which can store its message sender in the environment.
///
/// This is an interim solution until we work out nicer
/// interactions with environment values in callbacks and the like.
pub fn env_worker<F, H, M, S, V, Res, State, Action, Fut>(
    capacity: NonZeroUsize,
    init_future: F,
    store_sender: S,
    on_response: H,
) -> impl View<State, Action, ViewCtx, Element = NoElement>
where
    F: Fn(MessageProxy<M>, Receiver<V>) -> Fut + 'static,
    Fut: Future<Output = ()> + Send + 'static,
    S: Fn(&mut State, &mut Res, Sender<V>) + 'static,
    H: Fn(&mut State, M) -> Action + 'static,
    M: AnyDebug + Send + 'static,
    Res: Resource,
    State: 'static,
    Action: 'static,
    V: Send + 'static,
{
    const {
        assert!(
            size_of::<F>() == 0,
            "`worker` will not be ran again when its captured variables are updated.\n\
            To ignore this warning, use `worker_raw`."
        );
    };
    Worker {
        capacity,
        init_future,
        store_sender: move |state: &mut State, environment: &mut Environment, sender: Sender<V>| {
            let Some(resource) = environment.get_mut::<Res>() else {
                return false;
            };
            store_sender(state, resource, sender);
            true
        },
        on_response,
        message: PhantomData,
    }
}

/// Launch a worker which will run until the view is no longer in the tree.
///
/// This is [`worker`] without the capturing rules.
/// See `worker` for full documentation.
pub fn worker_raw<M, V, S, F, H, State, Action, Fut>(
    capacity: NonZeroUsize,
    init_future: F,
    store_sender: S,
    on_response: H,
) -> impl View<State, Action, ViewCtx, Element = NoElement>
where
    // TODO(DJMcNab): Accept app_state here
    F: Fn(MessageProxy<M>, Receiver<V>) -> Fut + 'static,
    Fut: Future<Output = ()> + Send + 'static,
    S: Fn(&mut State, Sender<V>) + 'static,
    H: Fn(&mut State, M) -> Action + 'static,
    M: AnyDebug + Send + 'static,
    State: 'static,
    Action: 'static,
    V: Send + 'static,
{
    Worker {
        capacity,
        init_future,
        on_response,
        store_sender: move |state: &mut State,
                            _environment: &mut Environment,
                            sender: Sender<V>| {
            store_sender(state, sender);
            true
        },
        message: PhantomData,
    }
}

/// The View type for [`worker`], [`env_worker`] and [`worker_raw`]. See its documentation for details.
pub struct Worker<State, Action, F, H, M, S, V> {
    capacity: NonZeroUsize,
    init_future: F,
    store_sender: S,
    on_response: H,
    message: PhantomData<fn(M, V, State) -> Action>,
}

impl<State, Action, F, H, M, S, V> ViewMarker for Worker<State, Action, F, H, M, S, V> {}

impl<State, Action, F, H, M, Fut, S, V> View<State, Action, ViewCtx>
    for Worker<State, Action, F, H, M, S, V>
where
    Action: 'static,
    F: Fn(MessageProxy<M>, Receiver<V>) -> Fut + 'static,
    V: Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
    S: Fn(&mut State, &mut Environment, Sender<V>) -> bool + 'static,
    H: Fn(&mut State, M) -> Action + 'static,
    M: AnyDebug + Send + 'static,
    State: 'static,
{
    type Element = NoElement;

    type ViewState = TaskLifetime;

    fn build(&self, ctx: &mut ViewCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        let path: Arc<[ViewId]> = ctx.view_path().into();

        let proxy = ctx.proxy();

        let (tx, rx) = tokio::sync::mpsc::channel(self.capacity.get().min(Semaphore::MAX_PERMITS));
        if !(self.store_sender)(app_state, ctx.environment(), tx) {
            return (NoElement, TaskLifetime::default());
        }
        let handle = ctx
            .runtime()
            .spawn((self.init_future)(MessageProxy::new(proxy, path), rx));
        (NoElement, TaskLifetime::new(handle))
    }

    fn rebuild(
        &self,
        _prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        (): Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        if !view_state.is_active() {
            *view_state = self.build(ctx, app_state).1;
        }
    }

    fn teardown(&self, handle: &mut Self::ViewState, _: &mut ViewCtx, (): Mut<'_, Self::Element>) {
        handle.cancel();
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageCtx,
        _element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        if !view_state.is_active() {
            return MessageResult::Stale;
        }
        message
            .take_message::<M>()
            .map_or(MessageResult::Stale, |message| {
                MessageResult::Action((self.on_response)(app_state, *message))
            })
    }
}

/// Process requests concurrently, with a bounded number of owned tasks.
///
/// Closing the input drains accepted requests. Dropping this future cancels
/// all running tasks, rather than detaching them. Responses are delivered in
/// completion order, so include any request identity needed to route them.
/// The handler must represent expected request failures in its response type.
pub async fn process_requests<Request, Response, Handler, Fut>(
    proxy: MessageProxy<Response>,
    mut requests: Receiver<Request>,
    concurrency: NonZeroUsize,
    handle: Handler,
) where
    Request: Send + 'static,
    Response: AnyDebug + Send + 'static,
    Handler: Fn(Request) -> Fut + Send,
    Fut: Future<Output = Response> + Send + 'static,
{
    let mut tasks = JoinSet::new();
    loop {
        while tasks.len() >= concurrency.get() {
            if let Some(Err(error)) = tasks.join_next().await {
                tracing::error!(%error, "worker request task failed");
            }
        }
        let Some(request) = requests.recv().await else {
            break;
        };
        let future = handle(request);
        let proxy = proxy.clone();
        tasks.spawn(async move {
            drop(proxy.message(future.await));
        });
    }
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            tracing::error!(%error, "worker request task failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::time::Duration;

    use crate::core::{ProxyError, RawProxy, SendMessage};

    use super::*;

    #[derive(Debug)]
    struct ClosedProxy;

    impl RawProxy for ClosedProxy {
        fn send_message(&self, _: Arc<[ViewId]>, message: SendMessage) -> Result<(), ProxyError> {
            Err(ProxyError::DriverFinished(message))
        }

        fn dyn_debug(&self) -> &dyn std::fmt::Debug {
            self
        }
    }

    struct RequestGuard(Sender<usize>, usize);

    impl Drop for RequestGuard {
        fn drop(&mut self) {
            let _ = self.0.try_send(self.1);
        }
    }

    #[test]
    fn request_concurrency_is_bounded_and_drop_cancels_children() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (tx, rx) = tokio::sync::mpsc::channel(3);
            let (started_tx, mut started_rx) = tokio::sync::mpsc::channel(3);
            let (dropped_tx, mut dropped_rx) = tokio::sync::mpsc::channel(3);
            for request in 0_usize..3 {
                tx.try_send(request).unwrap();
            }

            let proxy = MessageProxy::<()>::new(Arc::new(ClosedProxy), Arc::from([]));
            let task = TaskLifetime::new(tokio::spawn(process_requests(
                proxy,
                rx,
                NonZeroUsize::MIN.saturating_add(1),
                move |request| {
                    let started = started_tx.clone();
                    let dropped = dropped_tx.clone();
                    async move {
                        let _guard = RequestGuard(dropped, request);
                        started.try_send(request).unwrap();
                        pending::<()>().await;
                    }
                },
            )));
            let mut started = [
                started_rx.recv().await.unwrap(),
                started_rx.recv().await.unwrap(),
            ];
            started.sort_unstable();
            assert_eq!(started, [0, 1]);
            tokio::task::yield_now().await;
            assert!(
                started_rx.try_recv().is_err(),
                "third request must remain queued"
            );

            drop(task);
            let mut dropped = [
                tokio::time::timeout(Duration::from_secs(1), dropped_rx.recv())
                    .await
                    .unwrap()
                    .unwrap(),
                tokio::time::timeout(Duration::from_secs(1), dropped_rx.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            ];
            dropped.sort_unstable();
            assert_eq!(dropped, [0, 1]);
            assert!(
                started_rx.try_recv().is_err(),
                "cancellation must not start queued work"
            );
        });
    }
}
