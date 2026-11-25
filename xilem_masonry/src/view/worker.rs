// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use crate::ViewCtx;
use crate::core::anymore::AnyDebug;
use crate::core::{
    Environment, MessageCtx, MessageProxy, MessageResult, Mut, NoElement, Resource, View, ViewId,
    ViewMarker, ViewPathTracker,
};

// TODO: Update generic variable names to be more .

/// Launch a task which will run until the view is no longer in the tree.
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
    init_future: F,
    store_sender: S,
    on_response: H,
) -> impl View<State, Action, ViewCtx, Element = NoElement>
where
    F: Fn(MessageProxy<M>, UnboundedReceiver<V>) -> Fut + 'static,
    Fut: Future<Output = ()> + Send + 'static,
    S: Fn(&mut State, UnboundedSender<V>) + 'static,
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
    Worker {
        init_future,
        store_sender: move |state: &mut State,
                            _environment: &mut Environment,
                            sender: UnboundedSender<V>| {
            store_sender(state, sender);
            true
        },
        on_response,
        message: PhantomData,
    }
}

/// A version of [`worker`] which can store its message sender in the environment.
///
/// This is an interim solution until we work out nicer
/// interactions with environment values in callbacks and the like.
pub fn env_worker<F, H, M, S, V, Res, State, Action, Fut>(
    init_future: F,
    store_sender: S,
    on_response: H,
) -> impl View<State, Action, ViewCtx, Element = NoElement>
where
    F: Fn(MessageProxy<M>, UnboundedReceiver<V>) -> Fut + 'static,
    Fut: Future<Output = ()> + Send + 'static,
    S: Fn(&mut State, &mut Res, UnboundedSender<V>) + 'static,
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
        init_future,
        store_sender: move |state: &mut State,
                            environment: &mut Environment,
                            sender: UnboundedSender<V>| {
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
    init_future: F,
    store_sender: S,
    on_response: H,
) -> impl View<State, Action, ViewCtx, Element = NoElement>
where
    // TODO(DJMcNab): Accept app_state here
    F: Fn(MessageProxy<M>, UnboundedReceiver<V>) -> Fut + 'static,
    Fut: Future<Output = ()> + Send + 'static,
    S: Fn(&mut State, UnboundedSender<V>) + 'static,
    H: Fn(&mut State, M) -> Action + 'static,
    M: AnyDebug + Send + 'static,
    State: 'static,
    Action: 'static,
    V: Send + 'static,
{
    Worker {
        init_future,
        on_response,
        store_sender: move |state: &mut State,
                            _environment: &mut Environment,
                            sender: UnboundedSender<V>| {
            store_sender(state, sender);
            true
        },
        message: PhantomData,
    }
}

/// The View type for [`worker`], [`env_worker`] and [`worker_raw`]. See its documentation for details.
pub struct Worker<State, Action, F, H, M, S, V> {
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
    F: Fn(MessageProxy<M>, UnboundedReceiver<V>) -> Fut + 'static,
    V: Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
    S: Fn(&mut State, &mut Environment, UnboundedSender<V>) -> bool + 'static,
    H: Fn(&mut State, M) -> Action + 'static,
    M: AnyDebug + Send + 'static,
    State: 'static,
{
    type Element = NoElement;

    type ViewState = Option<JoinHandle<()>>;

    fn build(&self, ctx: &mut ViewCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        let path: Arc<[ViewId]> = ctx.view_path().into();

        let proxy = ctx.proxy();

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        if !(self.store_sender)(app_state, ctx.environment(), tx) {
            return (NoElement, None);
        }
        let handle = ctx
            .runtime()
            .spawn((self.init_future)(MessageProxy::new(proxy, path), rx));
        (NoElement, Some(handle))
    }

    fn rebuild(
        &self,
        _prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        (): Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        if view_state.is_none() {
            *view_state = self.build(ctx, app_state).1;
        }
    }

    fn teardown(&self, handle: &mut Self::ViewState, _: &mut ViewCtx, (): Mut<'_, Self::Element>) {
        if let Some(handle) = handle.take() {
            handle.abort();
        }
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageCtx,
        _element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        if view_state.is_none() {
            return MessageResult::Stale;
        }
        message
            .take_message::<M>()
            .map_or(MessageResult::Stale, |message| {
                MessageResult::Action((self.on_response)(app_state, *message))
            })
    }
}
