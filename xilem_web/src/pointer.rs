// Copyright 2023 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Interactivity with pointer events.

use std::marker::PhantomData;
use std::rc::Rc;

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
    listeners: Option<[EventListener; 5]>,
    registration: Rc<()>,
    active: Vec<PointerDetails>,
    child_state: S,
}

#[derive(Clone, Copy, Debug)]
/// A message representing a pointer event.
pub enum PointerMsg {
    Down(PointerDetails),
    Move(PointerDetails),
    Up(PointerDetails),
    /// A started gesture was cancelled or lost its capture.
    Cancel(PointerDetails),
}

impl PointerMsg {
    /// The details associated with this event.
    #[must_use = "Use the pointer event detail."]
    pub const fn details(&self) -> &PointerDetails {
        match self {
            Self::Down(p) | Self::Move(p) | Self::Up(p) | Self::Cancel(p) => p,
        }
    }

    /// The position in viewport coordinates.
    #[must_use = "Use the pointer event detail."]
    pub const fn position(&self) -> Point {
        self.details().position
    }

    /// The changed button, or -1 if no button changed.
    #[must_use = "Use the pointer event detail."]
    pub const fn button(&self) -> i16 {
        self.details().button
    }

    /// The DOM buttons bitmask, including all simultaneously pressed buttons.
    #[must_use = "Use the pointer event detail."]
    pub const fn buttons(&self) -> u16 {
        self.details().buttons
    }

    /// The pointer identifier.
    #[must_use = "Use the pointer event detail."]
    pub const fn id(&self) -> i32 {
        self.details().id
    }
}

#[derive(Clone, Copy, Debug)]
/// Details of a pointer event.
pub struct PointerDetails {
    pub id: i32,
    pub button: i16,
    /// All currently pressed buttons, using the DOM `MouseEvent.buttons` bitmask.
    pub buttons: u16,
    pub position: Point,
}

impl PointerDetails {
    fn from_pointer_event(e: &PointerEvent) -> Self {
        Self {
            id: e.pointer_id(),
            button: e.button(),
            buttons: e.buttons(),
            position: Point::new(f64::from(e.client_x()), f64::from(e.client_y())),
        }
    }

    const fn cancelled(self) -> PointerMsg {
        PointerMsg::Cancel(Self {
            button: -1,
            buttons: 0,
            ..self
        })
    }
}

// Events queued by a replaced DOM node cannot act on its successor. A
// replacement cancellation, in contrast, is owed to the surviving view.
#[derive(Debug)]
enum PointerDelivery {
    Dom(Rc<()>, PointerMsg),
    Retired(PointerDetails),
}

fn track_pointer(active: &mut Vec<PointerDetails>, event: PointerMsg) -> Option<PointerMsg> {
    let details = *event.details();
    let index = active.iter().position(|p| p.id == details.id);
    match event {
        PointerMsg::Down(_) => {
            if index.is_some() {
                return None;
            }
            active.push(details);
        }
        PointerMsg::Move(_) => {
            if let Some(index) = index {
                active[index] = details;
            }
        }
        PointerMsg::Up(_) => {
            active.remove(index?);
        }
        PointerMsg::Cancel(_) => {
            return Some(active.remove(index?).cancelled());
        }
    }
    Some(event)
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

fn build_event_listeners(
    ctx: &ViewCtx,
    el: &web_sys::Element,
    registration: &Rc<()>,
) -> [EventListener; 5] {
    type PointerHandler = (&'static str, fn(PointerDetails) -> PointerMsg);
    let kinds: [PointerHandler; 5] = [
        ("pointerdown", PointerMsg::Down),
        ("pointermove", PointerMsg::Move),
        ("pointerup", PointerMsg::Up),
        ("pointercancel", PointerMsg::Cancel),
        ("lostpointercapture", PointerMsg::Cancel),
    ];
    kinds.map(|(name, message)| {
        let target = el.clone();
        let registration = Rc::clone(registration);
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
            thunk.push_message(PointerDelivery::Dom(
                Rc::clone(&registration),
                message(PointerDetails::from_pointer_event(&event)),
            ));
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

            let registration = Rc::new(());
            let listeners = build_event_listeners(ctx, el, &registration);
            let state = PointerState {
                listeners: Some(listeners),
                registration,
                active: Vec::new(),
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
                state.registration = Rc::new(());
                state.listeners = Some(build_event_listeners(
                    ctx,
                    el.node.as_ref(),
                    &state.registration,
                ));
                if !state.active.is_empty() {
                    let thunk = ctx.message_thunk();
                    for details in state.active.drain(..) {
                        thunk.enqueue_message(PointerDelivery::Retired(details));
                    }
                }
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
            view_state.active.clear();
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
            let Some(delivery) = message.take_message::<PointerDelivery>() else {
                return MessageResult::Stale;
            };
            let event = match *delivery {
                PointerDelivery::Dom(registration, event) => {
                    if !Rc::ptr_eq(&registration, &view_state.registration) {
                        return MessageResult::Stale;
                    }
                    let Some(event) = track_pointer(&mut view_state.active, event) else {
                        return MessageResult::Stale;
                    };
                    event
                }
                PointerDelivery::Retired(details) => details.cancelled(),
            };
            MessageResult::Action((self.callback)(app_state, event))
        } else {
            self.child
                .message(&mut view_state.child_state, message, element, app_state)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn details(id: i32, buttons: u16) -> PointerDetails {
        PointerDetails {
            id,
            buttons,
            button: -1,
            position: Point::new(f64::from(id), 0.0),
        }
    }

    #[test]
    fn cancellation_is_terminal_and_independent_per_pointer() {
        let mut active = Vec::new();
        let first = details(1, 1);
        let second = details(2, 2);
        assert!(track_pointer(&mut active, PointerMsg::Down(first)).is_some());
        assert!(track_pointer(&mut active, PointerMsg::Down(second)).is_some());
        let moved = PointerDetails {
            position: Point::new(20.0, 30.0),
            buttons: 3,
            ..first
        };
        assert!(track_pointer(&mut active, PointerMsg::Move(moved)).is_some());
        let cancelled = track_pointer(&mut active, PointerMsg::Cancel(first));
        assert!(
            matches!(cancelled, Some(PointerMsg::Cancel(p)) if p.buttons == 0 && p.position == moved.position)
        );
        assert!(track_pointer(&mut active, PointerMsg::Cancel(first)).is_none());
        assert!(track_pointer(&mut active, PointerMsg::Up(first)).is_none());
        assert!(track_pointer(&mut active, PointerMsg::Up(second)).is_some());
        assert!(track_pointer(&mut active, PointerMsg::Cancel(second)).is_none());
        assert!(active.is_empty());
        assert!(track_pointer(&mut active, PointerMsg::Move(first)).is_some());
    }
}
