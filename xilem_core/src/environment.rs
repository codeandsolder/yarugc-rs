// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Values accessible throughout the Xilem view tree.

use alloc::boxed::Box;
use core::any::TypeId;
use core::marker::PhantomData;

use anymore::AnyDebug;
use hashbrown::HashMap;
use hashbrown::hash_map::Entry;

use crate::{MessageCtx, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker};

/// A message requesting that a context-consuming view rebuild itself.
#[derive(Debug)]
pub struct Rebuild;

/// A store of resources visible at the current location in the view tree.
///
/// Providers temporarily install their owned resource while traversing descendants.
/// Nested providers of the same type shadow their ancestors and restore them on exit.
/// Inactive resources are retained in their owning view state, not in this map.
///
/// The driver owns one environment and makes it available through
/// [`ViewPathTracker::environment`].
#[derive(Debug, Default)]
pub struct Environment {
    resources: HashMap<TypeId, Box<dyn AnyDebug>>,
}

impl Environment {
    /// Creates an empty environment for a driver.
    #[must_use = "Retain the environment for the lifetime of the driver."]
    pub fn new() -> Self {
        Self::default()
    }

    /// Borrows the closest active resource of the requested type, if any.
    #[must_use = "Inspect the resource visible in the current scope."]
    pub fn get<Res: Resource>(&self) -> Option<&Res> {
        self.resources.get(&TypeId::of::<Res>())?.downcast_ref()
    }

    /// Mutably borrows the closest active resource of the requested type, if any.
    pub fn get_mut<Res: Resource>(&mut self) -> Option<&mut Res> {
        self.resources.get_mut(&TypeId::of::<Res>())?.downcast_mut()
    }

    // Swapping the owned value with the current scope is its own inverse.
    // An empty scope has no map entry, so no inactive slots need reclamation.
    fn swap(&mut self, resource_type: TypeId, value: &mut Option<Box<dyn AnyDebug>>) {
        match (self.resources.entry(resource_type), value.take()) {
            (Entry::Occupied(mut entry), Some(next)) => *value = Some(entry.insert(next)),
            (Entry::Occupied(entry), None) => *value = Some(entry.remove()),
            (Entry::Vacant(entry), Some(next)) => {
                entry.insert(next);
            }
            (Entry::Vacant(_), None) => {}
        }
    }
}

/// Marker trait for values which can be provided to descendant views.
pub trait Resource: AnyDebug {}

// All four lifecycle methods use the same scoped ownership transfer. The driver
// is borrowed independently from the stored resource so descendants can provide
// additional resources and access the rest of the driver during traversal.
fn with_resource_scope<Res: Resource, Driver, R>(
    driver: &mut Driver,
    environment: impl Fn(&mut Driver) -> &mut Environment,
    resource: &mut Option<Box<dyn AnyDebug>>,
    f: impl FnOnce(&mut Driver) -> R,
) -> R {
    let resource_type = TypeId::of::<Res>();
    environment(driver).swap(resource_type, resource);
    let result = f(driver);
    environment(driver).swap(resource_type, resource);
    result
}

/// Makes a resource available to all descendants of a view.
///
/// The initializer runs once when the provider is built. The resource is
/// retained across rebuilds, and nested providers of the same type shadow it.
/// Mutating a resource does not automatically invalidate memoized descendants.
pub fn provides<State, Action, Res, InitialContext, ChildView, Ctx>(
    initial_context: InitialContext,
    child: ChildView,
) -> Provides<State, Action, Res, InitialContext, ChildView>
where
    State: 'static,
    InitialContext: Fn(&mut State) -> Res,
    ChildView: View<State, Action, Ctx>,
    Ctx: ViewPathTracker,
    Res: Resource,
{
    Provides {
        initial_context,
        child,
        phantom: PhantomData,
    }
}

/// The view type for [`provides`].
#[derive(Debug)]
#[must_use = "View values do nothing unless provided to Xilem."]
pub struct Provides<State, Action, Res: Resource, InitialContext, ChildView> {
    initial_context: InitialContext,
    child: ChildView,
    phantom: PhantomData<fn(State, Res) -> Action>,
}

/// Retained state of a resource provider.
#[derive(Debug)]
#[expect(
    unnameable_types,
    reason = "Implementation detail, public because of trait visibility rules"
)]
pub struct ProvidesState<ChildState> {
    child_state: ChildState,
    resource: Option<Box<dyn AnyDebug>>,
}

impl<State, Action, Res: Resource, InitialContext, ChildView> ViewMarker
    for Provides<State, Action, Res, InitialContext, ChildView>
{
}

impl<State, Action, Res, InitialContext, Ctx: ViewPathTracker, ChildView> View<State, Action, Ctx>
    for Provides<State, Action, Res, InitialContext, ChildView>
where
    State: 'static,
    InitialContext: Fn(&mut State) -> Res,
    ChildView: View<State, Action, Ctx>,
    Res: Resource,
    Self: 'static,
{
    type Element = ChildView::Element;
    type ViewState = ProvidesState<ChildView::ViewState>;

    fn build(&self, ctx: &mut Ctx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        let mut resource: Option<Box<dyn AnyDebug>> =
            Some(Box::new((self.initial_context)(app_state)));
        let (element, child_state) =
            with_resource_scope::<Res, _, _>(ctx, Ctx::environment, &mut resource, |ctx| {
                self.child.build(ctx, app_state)
            });
        (
            element,
            ProvidesState {
                child_state,
                resource,
            },
        )
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut Ctx,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        with_resource_scope::<Res, _, _>(ctx, Ctx::environment, &mut view_state.resource, |ctx| {
            self.child.rebuild(
                &prev.child,
                &mut view_state.child_state,
                ctx,
                element,
                app_state,
            );
        });
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut Ctx,
        element: Mut<'_, Self::Element>,
    ) {
        with_resource_scope::<Res, _, _>(ctx, Ctx::environment, &mut view_state.resource, |ctx| {
            self.child
                .teardown(&mut view_state.child_state, ctx, element);
        });
        view_state.resource = None;
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageCtx,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        with_resource_scope::<Res, _, _>(
            message,
            |message| &mut message.environment,
            &mut view_state.resource,
            |message| {
                self.child
                    .message(&mut view_state.child_state, message, element, app_state)
            },
        )
    }
}

type ConsumerSignature<State, Res, Action, ChildView> = fn(State, Res) -> (Action, ChildView);

/// Builds a child view using a resource from the closest active provider.
///
/// The callback receives `None` if no ancestor provides the resource, letting
/// it construct an explicit fallback view. It runs on build and rebuild.
/// Mutating a resource does not automatically invalidate memoized descendants.
pub fn with_context<State, Action, Res, Child, ChildView, Ctx>(
    child: Child,
) -> WithContext<State, Action, Res, Child, ChildView>
where
    State: 'static,
    Child: Fn(Option<&mut Res>, &mut State) -> ChildView,
    ChildView: View<State, Action, Ctx>,
    Ctx: ViewPathTracker,
    Res: Resource,
{
    WithContext {
        child,
        phantom: PhantomData,
    }
}

/// The view type for [`with_context`].
#[derive(Debug)]
#[must_use = "View values do nothing unless provided to Xilem."]
pub struct WithContext<State, Action, Res: Resource, Child, ChildView> {
    child: Child,
    phantom: PhantomData<ConsumerSignature<State, Res, Action, ChildView>>,
}

/// Retained state of a resource-consuming view.
#[derive(Debug)]
#[expect(
    unnameable_types,
    reason = "Implementation detail, public because of trait visibility rules"
)]
pub struct WithContextState<ChildState, ChildView> {
    prev: ChildView,
    child_state: ChildState,
}

const WITH_CONTEXT_CHILD: ViewId = ViewId::new(0xc64d_6aeb);

impl<State, Action, Res: Resource, Child, ChildView> ViewMarker
    for WithContext<State, Action, Res, Child, ChildView>
{
}

impl<State, Action, Res, Ctx: ViewPathTracker, Child, ChildView> View<State, Action, Ctx>
    for WithContext<State, Action, Res, Child, ChildView>
where
    State: 'static,
    Child: Fn(Option<&mut Res>, &mut State) -> ChildView,
    ChildView: View<State, Action, Ctx>,
    Res: Resource,
    Self: 'static,
{
    type Element = ChildView::Element;
    type ViewState = WithContextState<ChildView::ViewState, ChildView>;

    fn build(&self, ctx: &mut Ctx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        ctx.with_id(WITH_CONTEXT_CHILD, |ctx| {
            let child_view = (self.child)(ctx.environment().get_mut::<Res>(), app_state);
            let (element, child_state) = child_view.build(ctx, app_state);
            (
                element,
                WithContextState {
                    prev: child_view,
                    child_state,
                },
            )
        })
    }

    fn rebuild(
        &self,
        _: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut Ctx,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        ctx.with_id(WITH_CONTEXT_CHILD, |ctx| {
            let child_view = (self.child)(ctx.environment().get_mut::<Res>(), app_state);
            child_view.rebuild(
                &view_state.prev,
                &mut view_state.child_state,
                ctx,
                element,
                app_state,
            );
            view_state.prev = child_view;
        });
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut Ctx,
        element: Mut<'_, Self::Element>,
    ) {
        ctx.with_id(WITH_CONTEXT_CHILD, |ctx| {
            view_state
                .prev
                .teardown(&mut view_state.child_state, ctx, element);
        });
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageCtx,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        match message.take_first() {
            Some(WITH_CONTEXT_CHILD) => {
                view_state
                    .prev
                    .message(&mut view_state.child_state, message, element, app_state)
            }
            None if message.take_message::<Rebuild>().is_some() => MessageResult::RequestRebuild,
            _ => MessageResult::Stale,
        }
    }
}

type ActionSignature<State, ChildAction, Context, Res, Action> =
    fn(State, ChildAction, Context, Res) -> Action;

/// The view type for [`on_action_with_context`].
#[derive(Debug)]
#[must_use = "View values do nothing unless provided to Xilem."]
pub struct OnActionWithContext<State, Action, Context, OnAction, Res, ChildView, ChildAction> {
    child: ChildView,
    on_action: OnAction,
    phantom: PhantomData<ActionSignature<State, ChildAction, Context, Res, Action>>,
}

/// Operates on a resource when a child returns an action.
///
/// Actions requiring an absent resource are marked stale. Non-action results
/// (including rebuild requests) are forwarded unchanged.
pub fn on_action_with_context<State, Action, Context, OnAction, Res, ChildView, ChildAction>(
    on_action: OnAction,
    child: ChildView,
) -> OnActionWithContext<State, Action, Context, OnAction, Res, ChildView, ChildAction>
where
    State: 'static,
    Context: ViewPathTracker,
    OnActionWithContext<State, Action, Context, OnAction, Res, ChildView, ChildAction>:
        View<State, Action, Context>,
    OnAction: Fn(&mut State, &mut Res, ChildAction) -> Action,
{
    OnActionWithContext {
        child,
        on_action,
        phantom: PhantomData,
    }
}

impl<State, Action, Context, OnAction, Res, ChildView, ChildAction> ViewMarker
    for OnActionWithContext<State, Action, Context, OnAction, Res, ChildView, ChildAction>
{
}

impl<State, Action, Context, OnAction, Res, ChildView, ChildAction> View<State, Action, Context>
    for OnActionWithContext<State, Action, Context, OnAction, Res, ChildView, ChildAction>
where
    State: 'static,
    Context: ViewPathTracker,
    Res: Resource,
    Self: 'static,
    ChildView: View<State, ChildAction, Context>,
    OnAction: Fn(&mut State, &mut Res, ChildAction) -> Action,
{
    type Element = ChildView::Element;
    type ViewState = ChildView::ViewState;

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
    ) -> MessageResult<Action> {
        match self.child.message(view_state, message, element, app_state) {
            MessageResult::Action(action) => {
                let Some(resource) = message.environment.get_mut::<Res>() else {
                    return MessageResult::Stale;
                };
                MessageResult::Action((self.on_action)(app_state, resource, action))
            }
            MessageResult::RequestRebuild => MessageResult::RequestRebuild,
            MessageResult::Nop => MessageResult::Nop,
            MessageResult::Stale => MessageResult::Stale,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;
    use alloc::vec;
    use alloc::vec::Vec;
    use core::cell::RefCell;

    use super::*;
    use crate::docs::Fake;
    use crate::{DynMessage, NoElement, fork, run_once};

    #[derive(Debug)]
    struct Counter(u32);
    impl Resource for Counter {}

    type Observation = (Option<u32>, Vec<ViewId>);

    #[derive(Clone)]
    struct Probe(Rc<RefCell<Vec<Observation>>>);

    impl Probe {
        fn record(&self, ctx: &mut Fake) {
            let value = ctx.environment().get::<Counter>().map(|counter| counter.0);
            self.0.borrow_mut().push((value, ctx.view_path().to_vec()));
        }
    }

    impl ViewMarker for Probe {}
    impl View<(), u32, Fake> for Probe {
        type Element = NoElement;
        type ViewState = ();

        fn build(&self, ctx: &mut Fake, (): &mut ()) -> (NoElement, ()) {
            self.record(ctx);
            (NoElement, ())
        }

        fn rebuild(&self, _: &Self, (): &mut (), ctx: &mut Fake, (): (), (): &mut ()) {
            self.record(ctx);
        }

        fn teardown(&self, (): &mut (), ctx: &mut Fake, (): ()) {
            self.record(ctx);
        }

        fn message(
            &self,
            (): &mut (),
            message: &mut MessageCtx,
            (): (),
            (): &mut (),
        ) -> MessageResult<u32> {
            message
                .take_message::<u32>()
                .map_or(MessageResult::Stale, |message| {
                    MessageResult::Action(*message)
                })
        }
    }

    #[test]
    fn scoped_resources_shadow_and_restore_across_lifecycle() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let increment = |(): &mut (), resource: &mut Counter, amount: u32| {
            resource.0 += amount;
            resource.0
        };
        let view = provides(
            |(): &mut ()| Counter(1),
            fork(
                provides(
                    |(): &mut ()| Counter(10),
                    on_action_with_context(increment, Probe(events.clone())),
                ),
                on_action_with_context(increment, Probe(events.clone())),
            ),
        );
        let mut ctx = Fake::default();
        let (NoElement, mut state) = view.build(&mut ctx, &mut ());
        assert_eq!(
            *events.borrow(),
            vec![
                (Some(10), vec![ViewId::new(0)]),
                (Some(1), vec![ViewId::new(1)])
            ]
        );
        assert!(ctx.environment().get::<Counter>().is_none());

        for (index, amount, expected) in [(0, 5_u32, 15), (1, 2, 3)] {
            let mut message = MessageCtx::new(
                core::mem::take(ctx.environment()),
                vec![ViewId::new(index)],
                DynMessage::new(amount),
            );
            assert!(
                matches!(view.message(&mut state, &mut message, (), &mut ()), MessageResult::Action(value) if value == expected)
            );
            *ctx.environment() = message.finish().0;
            assert!(ctx.environment().get::<Counter>().is_none());
        }

        events.borrow_mut().clear();
        view.rebuild(&view, &mut state, &mut ctx, (), &mut ());
        assert_eq!(
            *events.borrow(),
            vec![
                (Some(15), vec![ViewId::new(0)]),
                (Some(3), vec![ViewId::new(1)])
            ]
        );
        assert!(ctx.environment().get::<Counter>().is_none());

        events.borrow_mut().clear();
        view.teardown(&mut state, &mut ctx, ());
        assert_eq!(
            *events.borrow(),
            vec![
                (Some(3), vec![ViewId::new(1)]),
                (Some(15), vec![ViewId::new(0)])
            ]
        );
        assert!(ctx.environment().get::<Counter>().is_none());
        assert!(ctx.view_path().is_empty());
    }

    #[test]
    fn missing_resource_has_explicit_fallback_and_stale_actions() {
        let mut context = Fake::default();
        let consumer = with_context::<Vec<Option<u32>>, (), Counter, _, _, Fake>(
            |resource: Option<&mut Counter>, state: &mut Vec<Option<u32>>| {
                state.push(resource.map(|counter| counter.0));
                run_once(|| {})
            },
        );
        let mut app_state = Vec::new();
        let (NoElement, mut state) =
            View::<Vec<Option<u32>>, (), Fake>::build(&consumer, &mut context, &mut app_state);
        consumer.rebuild(&consumer, &mut state, &mut context, (), &mut app_state);
        assert_eq!(app_state, vec![None, None]);

        let handler = on_action_with_context(
            |(): &mut (), counter: &mut Counter, action| counter.0 + action,
            Probe(Rc::new(RefCell::new(Vec::new()))),
        );
        let (NoElement, mut handler_state) = handler.build(&mut context, &mut ());
        let mut message = MessageCtx::new(Environment::new(), vec![], DynMessage::new(7_u32));
        assert!(matches!(
            handler.message(&mut handler_state, &mut message, (), &mut ()),
            MessageResult::Stale
        ));
    }
}
