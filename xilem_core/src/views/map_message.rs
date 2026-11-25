// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use core::fmt::Debug;
use core::marker::PhantomData;

use crate::{Environment, MessageCtx, MessageResult, Mut, View, ViewMarker, ViewPathTracker};

type ViewSignature<State, ParentAction, ChildAction, Context> =
    fn() -> (State, ParentAction, ChildAction, Context);

/// Maps a child message result into a parent message result.
pub trait MessageMapper<State, ChildAction, ParentAction> {
    /// Transform the result, with mutable access to application state.
    fn map(
        &self,
        state: &mut State,
        environment: &mut Environment,
        result: MessageResult<ChildAction>,
    ) -> MessageResult<ParentAction>;
}

impl<State, ChildAction, ParentAction, F> MessageMapper<State, ChildAction, ParentAction> for F
where
    F: Fn(&mut State, MessageResult<ChildAction>) -> MessageResult<ParentAction>,
{
    fn map(
        &self,
        state: &mut State,
        _environment: &mut Environment,
        result: MessageResult<ChildAction>,
    ) -> MessageResult<ParentAction> {
        self(state, result)
    }
}

/// Adapts an action-only callback, leaving non-action results unchanged.
#[derive(Debug)]
pub struct ActionMapper<F>(F);

impl<State, ChildAction, ParentAction, F> MessageMapper<State, ChildAction, ParentAction>
    for ActionMapper<F>
where
    F: Fn(&mut State, ChildAction) -> ParentAction,
{
    fn map(
        &self,
        state: &mut State,
        _environment: &mut Environment,
        result: MessageResult<ChildAction>,
    ) -> MessageResult<ParentAction> {
        result.map(|action| (self.0)(state, action))
    }
}

/// View type for [`map_message_result`] and [`map_action`]. Most users will want to use `map_action` (the latter).
///
/// This view maps a child [`View<State,ChildAction,_>`] to [`View<State,ParentAction,_>`], whilst allowing the kind of [`MessageResult`] to be changed.
#[must_use = "View values do nothing unless provided to Xilem."]
pub struct MapMessage<V, State, ParentAction, ChildAction, Context, F> {
    pub(crate) map_fn: F,
    pub(crate) child: V,
    pub(crate) phantom: PhantomData<ViewSignature<State, ParentAction, ChildAction, Context>>,
}

impl<V, State, ParentAction, ChildAction, Context, F> Debug
    for MapMessage<V, State, ParentAction, ChildAction, Context, F>
where
    V: Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MapMessage")
            .field("child_action", &core::any::type_name::<ChildAction>())
            .field("parent_action", &core::any::type_name::<ParentAction>())
            .field("child", &self.child)
            .finish_non_exhaustive()
    }
}

/// A view that maps a child [`View<State,ChildAction,_>`] to [`View<State,ParentAction,_>`] while providing mutable access to `State` in the map function.
///
/// This is very similar to the Elm architecture, where the parent view can update state based on the action message from the child view.
///
/// # Examples
///
/// (From the Xilem implementation)
///
/// ```ignore
/// enum CountMessage {
///     Increment,
///     Decrement,
/// }
///
/// fn count_view<T>(count: i32) -> impl WidgetView<T, CountMessage> {
///     flex((
///         label(format!("count: {}", count)),
///         button("+", |_| CountMessage::Increment),
///         button("-", |_| CountMessage::Decrement),
///     ))
/// }
///
/// fn app_logic(count: &mut i32) -> impl WidgetView<i32> {
///     map_action(count_view(*count), |count, message| match message {
///         CountMessage::Increment => *count += 1,
///         CountMessage::Decrement => *count -= 1,
///     })
/// }
/// ```
pub fn map_action<State, ParentAction, ChildAction, Context: ViewPathTracker, V, F>(
    view: V,
    map_fn: F,
) -> MapMessage<V, State, ParentAction, ChildAction, Context, ActionMapper<F>>
where
    State: 'static,
    ParentAction: 'static,
    ChildAction: 'static,
    V: View<State, ChildAction, Context>,
    F: Fn(&mut State, ChildAction) -> ParentAction + 'static,
{
    MapMessage {
        map_fn: ActionMapper(map_fn),
        child: view,
        phantom: PhantomData,
    }
}

/// A view which maps a child [`View<State,ChildAction,_>`] to [`View<State,ParentAction,_>`], whilst allowing the kind of [`MessageResult`] to be changed.
///
/// Changing the [`MessageResult`] kind is mainly useful to intercept child messages.
///
/// This is the more general form of [`map_action`].
/// In most cases, you probably want to use that function.
pub fn map_message_result<State, ParentAction, ChildAction, Context: ViewPathTracker, V, F>(
    view: V,
    map_fn: F,
) -> MapMessage<V, State, ParentAction, ChildAction, Context, F>
where
    State: 'static,
    ParentAction: 'static,
    ChildAction: 'static,
    V: View<State, ChildAction, Context>,
    F: Fn(&mut State, MessageResult<ChildAction>) -> MessageResult<ParentAction> + 'static,
{
    MapMessage {
        map_fn,
        child: view,
        phantom: PhantomData,
    }
}

impl<V, State, ParentAction, ChildAction, F, Context> ViewMarker
    for MapMessage<V, State, ParentAction, ChildAction, Context, F>
{
}
impl<V, State, ParentAction, ChildAction, Context, F> View<State, ParentAction, Context>
    for MapMessage<V, State, ParentAction, ChildAction, Context, F>
where
    V: View<State, ChildAction, Context>,
    State: 'static,
    ParentAction: 'static,
    ChildAction: 'static,
    F: MessageMapper<State, ChildAction, ParentAction> + 'static,
    Context: ViewPathTracker + 'static,
{
    type ViewState = V::ViewState;
    type Element = V::Element;

    fn build(&self, ctx: &mut Context, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        self.child.build(ctx, app_state)
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut Context,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        self.child
            .rebuild(&prev.child, view_state, ctx, element, app_state);
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut Context,
        element: Mut<'_, Self::Element>,
    ) {
        self.child.teardown(view_state, ctx, element);
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageCtx,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<ParentAction> {
        let child_result = self.child.message(view_state, message, element, app_state);
        self.map_fn
            .map(app_state, &mut message.environment, child_result)
    }
}
