// Copyright 2023 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Interactivity with pointer events.

use std::marker::PhantomData;

use peniko::kurbo::Point;
use wasm_bindgen::JsCast;
use web_sys::PointerEvent;

use crate::core::{MessageCtx, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker};
use crate::events::EventListener;
use crate::interfaces::Element;
use crate::{DomView, ViewCtx};

// Use a distinctive number here, to be able to catch bugs.
// In case the generational-id view path in `View::Message` lead to a wrong view
/// This is a randomly generated 32 bit number - 305418260 in decimal.
const POINTER_VIEW_ID: ViewId = ViewId::new(0x1234_5014);

/// A view that allows stateful handling of [`PointerEvent`]s with [`PointerMsg`]
///
/// See [`Element::pointer`] for more details how to use this view.
pub struct Pointer<V, T, A, F> {
    child: V,
    callback: F,
    phantom: PhantomData<fn() -> (T, A)>,
}

#[expect(
    unnameable_types,
    reason = "Implementation detail, public because of trait visibility rules"
)]
pub struct PointerState<S> {
    listeners: Option<[EventListener; 3]>,
    child_state: S,
}

#[derive(Debug)]
/// A message representing a pointer event.
pub enum PointerMsg {
    Down(PointerDetails),
    Move(PointerDetails),
    Up(PointerDetails),
}

impl PointerMsg {
    #[must_use = "Use the pointer event detail."]
    pub const fn position(&self) -> Point {
        match self {
            Self::Down(p) | Self::Move(p) | Self::Up(p) => p.position,
        }
    }

    #[must_use = "Use the pointer event detail."]
    pub const fn button(&self) -> i16 {
        match self {
            Self::Down(p) | Self::Move(p) | Self::Up(p) => p.button,
        }
    }

    #[must_use = "Use the pointer event detail."]
    pub const fn id(&self) -> i32 {
        match self {
            Self::Down(p) | Self::Move(p) | Self::Up(p) => p.id,
        }
    }
}

#[derive(Debug)]
/// Details of a pointer event.
pub struct PointerDetails {
    pub id: i32,
    pub button: i16,
    pub position: Point,
}

impl PointerDetails {
    fn from_pointer_event(e: &PointerEvent) -> Self {
        Self {
            id: e.pointer_id(),
            button: e.button(),
            position: Point::new(f64::from(e.client_x()), f64::from(e.client_y())),
        }
    }
}

pub fn pointer<T: 'static, A, F: Fn(&mut T, PointerMsg), V: Element<T, A>>(
    child: V,
    callback: F,
) -> Pointer<V, T, A, F> {
    Pointer {
        child,
        callback,
        phantom: PhantomData,
    }
}

fn build_event_listeners(ctx: &ViewCtx, el: &web_sys::Element) -> [EventListener; 3] {
    type PointerHandler = (&'static str, fn(PointerDetails) -> PointerMsg);
    let kinds: [PointerHandler; 3] = [
        ("pointerdown", PointerMsg::Down),
        ("pointermove", PointerMsg::Move),
        ("pointerup", PointerMsg::Up),
    ];
    kinds.map(|(name, message)| {
        let target = el.clone();
        let thunk = ctx.message_thunk();
        EventListener::new(el.as_ref(), name.into(), false, false, move |event| {
            let Ok(event) = event.dyn_into::<PointerEvent>() else {
                return;
            };
            if name == "pointerdown" {
                // Capture before dispatch, since the callback may replace the node.
                if let Err(error) = target.set_pointer_capture(event.pointer_id()) {
                    web_sys::console::warn_1(&error);
                }
            }
            event.prevent_default();
            event.stop_propagation();
            thunk.push_message(message(PointerDetails::from_pointer_event(&event)));
        })
    })
}

impl<V, State, Action, Callback> ViewMarker for Pointer<V, State, Action, Callback> {}
impl<State, Action, Callback, V> View<State, Action, ViewCtx>
    for Pointer<V, State, Action, Callback>
where
    State: 'static,
    Action: 'static,
    Callback: Fn(&mut State, PointerMsg) -> Action + 'static,
    V: DomView<State, Action, DomNode: AsRef<web_sys::Element>>,
{
    type ViewState = PointerState<V::ViewState>;
    type Element = V::Element;

    fn build(&self, ctx: &mut ViewCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        ctx.with_id(POINTER_VIEW_ID, |ctx| {
            let (element, child_state) = self.child.build(ctx, app_state);
            let el = element.node.as_ref();

            let listeners = build_event_listeners(ctx, el);
            let state = PointerState {
                listeners: Some(listeners),
                child_state,
            };
            (element, state)
        })
    }

    fn rebuild(
        &self,
        prev: &Self,
        state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        mut el: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        ctx.with_id(POINTER_VIEW_ID, |ctx| {
            self.child.rebuild(
                &prev.child,
                &mut state.child_state,
                ctx,
                el.reborrow_mut(),
                app_state,
            );

            if el.flags.was_created() {
                drop(state.listeners.take());
                state.listeners = Some(build_event_listeners(ctx, el.node.as_ref()));
            }
        });
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        element: Mut<'_, Self::Element>,
    ) {
        ctx.with_id(POINTER_VIEW_ID, |ctx| {
            view_state.listeners = None;
            self.child
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
        if message.take_first() != Some(POINTER_VIEW_ID) {
            return MessageResult::Stale;
        }
        if message.remaining_path().is_empty() {
            let Some(msg) = message.take_message() else {
                return MessageResult::Stale;
            };
            MessageResult::Action((self.callback)(app_state, *msg))
        } else {
            self.child
                .message(&mut view_state.child_state, message, element, app_state)
        }
    }
}
