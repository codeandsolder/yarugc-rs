// Copyright 2023 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::borrow::Cow;
use std::marker::PhantomData;

use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::{AddEventListenerOptions, EventTarget, js_sys};

use crate::core::anymore::AnyDebug;
use crate::core::{MessageCtx, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker};
use crate::{DomView, OptionalAction, ViewCtx};

// Use a distinctive number here, to be able to catch bugs.
// In case the generational-id view path in `View::Message` lead to a wrong view
/// This is a randomly generated 32 bit number - 592908563 in decimal.
const ON_EVENT_VIEW_ID: ViewId = ViewId::new(0x2357_1113);

type EventPhantom<State, Action, Event> = PhantomData<fn() -> (State, Action, Event)>;

/// Wraps a [`View`] `V` and attaches an event listener.
///
/// The event type `Event` should inherit from [`web_sys::Event`]
#[derive(Clone, Debug)]
pub struct OnEvent<V, State, Action, Event, Callback> {
    pub(crate) dom_view: V,
    pub(crate) event: Cow<'static, str>,
    pub(crate) capture: bool,
    pub(crate) passive: bool,
    pub(crate) handler: Callback,
    pub(crate) phantom_event_ty: EventPhantom<State, Action, Event>,
}

impl<V, State, Action, Event, Callback> OnEvent<V, State, Action, Event, Callback>
where
    Event: JsCast + 'static,
{
    pub fn new(dom_view: V, event: impl Into<Cow<'static, str>>, handler: Callback) -> Self {
        Self {
            dom_view,
            event: event.into(),
            passive: true,
            capture: false,
            handler,
            phantom_event_ty: PhantomData,
        }
    }

    /// Whether the event handler should be passive. (default = `true`)
    ///
    /// Passive event handlers can't prevent the browser's default action from
    /// running (otherwise possible with `event.prevent_default()`), which
    /// restricts what they can be used for, but reduces overhead.
    #[must_use = "Use the configured event view."]
    pub const fn passive(mut self, value: bool) -> Self {
        self.passive = value;
        self
    }

    /// Whether the event handler should capture the event *before* being dispatched to any `EventTarget` beneath it in the DOM tree. (default = `false`)
    ///
    /// Events that are bubbling upward through the tree will not trigger a listener designated to use capture.
    /// Event bubbling and capturing are two ways of propagating events that occur in an element that is nested within another element,
    /// when both elements have registered a handle for that event.
    /// The event propagation mode determines the order in which elements receive the event.
    // TODO use similar Nomenclature as gloo (Phase::Bubble/Phase::Capture)?
    #[must_use = "Use the configured event view."]
    pub const fn capture(mut self, value: bool) -> Self {
        self.capture = value;
        self
    }
}

/// Owns the callback and its exact registration on the DOM target.
/// Removing or replacing the view unregisters the old callback even if
/// the DOM node is retained elsewhere.
pub(crate) struct EventListener {
    target: EventTarget,
    event: Cow<'static, str>,
    capture: bool,
    passive: bool,
    callback: Closure<dyn FnMut(web_sys::Event)>,
}

#[derive(Clone, Copy)]
struct ListenerOptions {
    capture: bool,
    passive: bool,
}

impl EventListener {
    pub(crate) fn new(
        target: &EventTarget,
        event: Cow<'static, str>,
        capture: bool,
        passive: bool,
        callback: impl FnMut(web_sys::Event) + 'static,
    ) -> Self {
        let callback = Closure::new(callback);
        let options = AddEventListenerOptions::new();
        options.set_capture(capture);
        options.set_passive(passive);
        target
            .add_event_listener_with_callback_and_add_event_listener_options(
                &event,
                callback.as_ref().unchecked_ref(),
                &options,
            )
            .unwrap_throw();
        Self {
            target: target.clone(),
            event,
            capture,
            passive,
            callback,
        }
    }

    fn matches(&self, target: &EventTarget, event: &str, options: ListenerOptions) -> bool {
        &self.target == target
            && self.event == event
            && self.capture == options.capture
            && self.passive == options.passive
    }
}

impl Drop for EventListener {
    fn drop(&mut self) {
        if let Err(err) = self.target.remove_event_listener_with_callback_and_bool(
            &self.event,
            self.callback.as_ref().unchecked_ref(),
            self.capture,
        ) {
            web_sys::console::error_1(&err);
        }
    }
}

fn create_event_listener<Event: JsCast + AnyDebug>(
    target: &EventTarget,
    event: &str,
    options: ListenerOptions,
    ctx: &ViewCtx,
) -> EventListener {
    let thunk = ctx.message_thunk();
    EventListener::new(
        target,
        event.to_owned().into(),
        options.capture,
        options.passive,
        move |event| {
            if let Ok(event) = event.dyn_into::<Event>() {
                thunk.push_message(event);
            }
        },
    )
}

mod hidden {
    use super::EventListener;
    #[expect(
        unnameable_types,
        reason = "Implementation detail, public because of trait visibility rules"
    )]
    /// State for the `OnEvent` view.
    pub struct OnEventState<S> {
        pub(crate) child_state: S,
        pub(crate) listener: Option<EventListener>,
    }
}

use hidden::OnEventState;

// These (boilerplatey) functions are there to reduce the boilerplate created by the macro-expansion below.

fn build_event_listener<State, Action, V, Event>(
    element_view: &V,
    event: &str,
    options: ListenerOptions,
    ctx: &mut ViewCtx,
    app_state: &mut State,
) -> (V::Element, OnEventState<V::ViewState>)
where
    State: 'static,
    Action: 'static,
    V: DomView<State, Action>,
    Event: JsCast + 'static + AnyDebug,
{
    // we use a placeholder id here, the id can never change, so we don't need to store it anywhere
    ctx.with_id(ON_EVENT_VIEW_ID, |ctx| {
        let (element, child_state) = element_view.build(ctx, app_state);
        let listener = create_event_listener::<Event>(element.as_ref(), event, options, ctx);
        let state = OnEventState {
            child_state,
            listener: Some(listener),
        };
        (element, state)
    })
}

fn rebuild_event_listener<State, Action, V, Event>(
    element_view: &V,
    prev_element_view: &V,
    mut element: Mut<'_, V::Element>,
    registration: (&str, ListenerOptions),
    state: &mut OnEventState<V::ViewState>,
    ctx: &mut ViewCtx,
    app_state: &mut State,
) where
    State: 'static,
    Action: 'static,
    V: DomView<State, Action>,
    Event: JsCast + 'static + AnyDebug,
{
    let (event, options) = registration;
    ctx.with_id(ON_EVENT_VIEW_ID, |ctx| {
        element_view.rebuild(
            prev_element_view,
            &mut state.child_state,
            ctx,
            element.reborrow_mut(),
            app_state,
        );
        if state
            .listener
            .as_ref()
            .is_some_and(|listener| listener.matches(element.as_ref(), event, options))
        {
            return;
        }
        // Drop the previous registration before creating its replacement.
        drop(state.listener.take());
        state.listener = Some(create_event_listener::<Event>(
            element.as_ref(),
            event,
            options,
            ctx,
        ));
    });
}

fn teardown_event_listener<State, Action, V>(
    element_view: &V,
    element: Mut<'_, V::Element>,
    state: &mut OnEventState<V::ViewState>,
    ctx: &mut ViewCtx,
) where
    State: 'static,
    Action: 'static,
    V: DomView<State, Action>,
{
    state.listener = None;
    ctx.with_id(ON_EVENT_VIEW_ID, |ctx| {
        element_view.teardown(&mut state.child_state, ctx, element);
    });
}

fn message_event_listener<State, Action, V, Event, OA, Callback>(
    element_view: &V,
    state: &mut OnEventState<V::ViewState>,
    message: &mut MessageCtx,
    element: Mut<'_, V::Element>,
    app_state: &mut State,
    handler: &Callback,
) -> MessageResult<Action>
where
    State: 'static,
    Action: 'static,
    V: DomView<State, Action>,
    Event: JsCast + 'static + AnyDebug,
    OA: OptionalAction<Action>,
    Callback: Fn(&mut State, Event) -> OA + 'static,
{
    if message.take_first() != Some(ON_EVENT_VIEW_ID) {
        return MessageResult::Stale;
    }
    if message.remaining_path().is_empty() {
        let Some(event) = message.take_message::<Event>() else {
            return MessageResult::Stale;
        };
        (handler)(app_state, *event)
            .action()
            .map_or(MessageResult::Nop, MessageResult::Action)
    } else {
        element_view.message(&mut state.child_state, message, element, app_state)
    }
}

impl<V, State, Action, Event, Callback> ViewMarker for OnEvent<V, State, Action, Event, Callback> {}
impl<V, State, Action, Event, Callback, OA> View<State, Action, ViewCtx>
    for OnEvent<V, State, Action, Event, Callback>
where
    State: 'static,
    Action: 'static,
    V: DomView<State, Action>,
    OA: OptionalAction<Action>,
    Callback: Fn(&mut State, Event) -> OA + 'static,
    Event: JsCast + 'static + AnyDebug,
{
    type ViewState = OnEventState<V::ViewState>;

    type Element = V::Element;

    fn build(&self, ctx: &mut ViewCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        build_event_listener::<_, _, _, Event>(
            &self.dom_view,
            &self.event,
            ListenerOptions {
                capture: self.capture,
                passive: self.passive,
            },
            ctx,
            app_state,
        )
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        rebuild_event_listener::<_, _, _, Event>(
            &self.dom_view,
            &prev.dom_view,
            element,
            (
                &self.event,
                ListenerOptions {
                    capture: self.capture,
                    passive: self.passive,
                },
            ),
            view_state,
            ctx,
            app_state,
        );
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        element: Mut<'_, Self::Element>,
    ) {
        teardown_event_listener(&self.dom_view, element, view_state, ctx);
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageCtx,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        message_event_listener(
            &self.dom_view,
            view_state,
            message,
            element,
            app_state,
            &self.handler,
        )
    }
}

macro_rules! event_definitions {
    ($(($ty_name:ident, $event_name:literal, $web_sys_ty:ident)),*) => {
        $(
        pub struct $ty_name<V, State, Action, Callback> {
            pub(crate) dom_view: V,
            pub(crate) capture: bool,
            pub(crate) passive: bool,
            pub(crate) handler: Callback,
            pub(crate) phantom_event_ty: PhantomData<fn() -> (State, Action)>,
        }

        impl<V, State, Action, Callback> ViewMarker for $ty_name<V, State, Action, Callback> {}
        impl<V, State, Action, Callback> $ty_name<V, State, Action, Callback> {
            pub fn new(dom_view: V, handler: Callback) -> Self {
                Self {
                    dom_view,
                    passive: true,
                    capture: false,
                    handler,
                    phantom_event_ty: PhantomData,
                }
            }

            /// Whether the event handler should be passive. (default = `true`)
            ///
            /// Passive event handlers can't prevent the browser's default action from
            /// running (otherwise possible with `event.prevent_default()`), which
            /// restricts what they can be used for, but reduces overhead.
            #[must_use = "Use the configured event view."]
    pub const fn passive(mut self, value: bool) -> Self {
                self.passive = value;
                self
            }

            /// Whether the event handler should capture the event *before* being dispatched to any `EventTarget` beneath it in the DOM tree. (default = `false`)
            ///
            /// Events that are bubbling upward through the tree will not trigger a listener designated to use capture.
            /// Event bubbling and capturing are two ways of propagating events that occur in an element that is nested within another element,
            /// when both elements have registered a handle for that event.
            /// The event propagation mode determines the order in which elements receive the event.
            // TODO use similar Nomenclature as gloo (Phase::Bubble/Phase::Capture)?
            #[must_use = "Use the configured event view."]
    pub const fn capture(mut self, value: bool) -> Self {
                self.capture = value;
                self
            }
        }


        impl<V, State, Action, Callback, OA> View<State, Action, ViewCtx>
            for $ty_name<V, State, Action, Callback>
        where
            State: 'static,
            Action: 'static,
            V: DomView<State, Action>,
            OA: OptionalAction<Action> + 'static,
            Callback: Fn(&mut State, web_sys::$web_sys_ty) -> OA + 'static,
        {
            type ViewState = OnEventState<V::ViewState>;

            type Element = V::Element;

            fn build(&self, ctx: &mut ViewCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
                build_event_listener::<_, _, _, web_sys::$web_sys_ty>(
                    &self.dom_view,
                    $event_name,
                    ListenerOptions { capture: self.capture, passive: self.passive },
                    ctx,
                    app_state
                )
            }

            fn rebuild(
                &self,
                prev: &Self,
                view_state: &mut Self::ViewState,
                ctx: &mut ViewCtx,
                element: Mut<'_, Self::Element>,
                app_state: &mut State
            ) {
                rebuild_event_listener::<_, _, _, web_sys::$web_sys_ty>(
                    &self.dom_view,
                    &prev.dom_view,
                    element,
                    ($event_name, ListenerOptions { capture: self.capture, passive: self.passive }),
                    view_state,
                    ctx,
                    app_state
                );
            }

            fn teardown(
                &self,
                view_state: &mut Self::ViewState,
                ctx: &mut ViewCtx,
                element: Mut<'_, Self::Element>,
            ) {
                teardown_event_listener(&self.dom_view, element, view_state, ctx);
            }

            fn message(
                &self,
                view_state: &mut Self::ViewState,
                 message: &mut MessageCtx,
                 element: Mut<'_, Self::Element>,
                app_state: &mut State,
            ) -> MessageResult<Action> {
                message_event_listener(&self.dom_view, view_state, message, element, app_state, &self.handler)
            }
        }
        )*
    };
}

event_definitions!(
    (OnAbort, "abort", Event),
    (OnAuxClick, "auxclick", PointerEvent),
    (OnBeforeInput, "beforeinput", InputEvent),
    (OnBeforeMatch, "beforematch", Event),
    (OnBeforeToggle, "beforetoggle", Event),
    (OnBlur, "blur", FocusEvent),
    (OnCancel, "cancel", Event),
    (OnCanPlay, "canplay", Event),
    (OnCanPlayThrough, "canplaythrough", Event),
    (OnChange, "change", Event),
    (OnClick, "click", PointerEvent),
    (OnClose, "close", Event),
    (OnContextLost, "contextlost", Event),
    (OnContextMenu, "contextmenu", PointerEvent),
    (OnContextRestored, "contextrestored", Event),
    (OnCopy, "copy", Event),
    (OnCueChange, "cuechange", Event),
    (OnCut, "cut", Event),
    (OnDblClick, "dblclick", MouseEvent),
    (OnDrag, "drag", Event),
    (OnDragEnd, "dragend", Event),
    (OnDragEnter, "dragenter", Event),
    (OnDragLeave, "dragleave", Event),
    (OnDragOver, "dragover", Event),
    (OnDragStart, "dragstart", Event),
    (OnDrop, "drop", Event),
    (OnDurationChange, "durationchange", Event),
    (OnEmptied, "emptied", Event),
    (OnEnded, "ended", Event),
    (OnError, "error", Event),
    (OnFocus, "focus", FocusEvent),
    (OnFocusIn, "focusin", FocusEvent),
    (OnFocusOut, "focusout", FocusEvent),
    (OnFormData, "formdata", Event),
    (OnInput, "input", Event),
    (OnInvalid, "invalid", Event),
    (OnKeyDown, "keydown", KeyboardEvent),
    (OnKeyUp, "keyup", KeyboardEvent),
    (OnLoad, "load", Event),
    (OnLoadedData, "loadeddata", Event),
    (OnLoadedMetadata, "loadedmetadata", Event),
    (OnLoadStart, "loadstart", Event),
    (OnMouseDown, "mousedown", MouseEvent),
    (OnMouseEnter, "mouseenter", MouseEvent),
    (OnMouseLeave, "mouseleave", MouseEvent),
    (OnMouseMove, "mousemove", MouseEvent),
    (OnMouseOut, "mouseout", MouseEvent),
    (OnMouseOver, "mouseover", MouseEvent),
    (OnMouseUp, "mouseup", MouseEvent),
    (OnPaste, "paste", Event),
    (OnPause, "pause", Event),
    (OnPlay, "play", Event),
    (OnPlaying, "playing", Event),
    (OnPointerCancel, "pointercancel", PointerEvent),
    (OnPointerDown, "pointerdown", PointerEvent),
    (OnPointerEnter, "pointerenter", PointerEvent),
    (OnPointerLeave, "pointerleave", PointerEvent),
    (OnPointerMove, "pointermove", PointerEvent),
    (OnPointerOut, "pointerout", PointerEvent),
    (OnPointerOver, "pointerover", PointerEvent),
    (OnPointerRawUpdate, "pointerrawupdate", PointerEvent),
    (OnPointerUp, "pointerup", PointerEvent),
    (OnProgress, "progress", Event),
    (OnRateChange, "ratechange", Event),
    (OnReset, "reset", Event),
    (OnScroll, "scroll", Event),
    (OnScrollEnd, "scrollend", Event),
    (OnSecurityPolicyViolation, "securitypolicyviolation", Event),
    (OnSeeked, "seeked", Event),
    (OnSeeking, "seeking", Event),
    (OnSelect, "select", Event),
    (OnSlotChange, "slotchange", Event),
    (OnStalled, "stalled", Event),
    (OnSubmit, "submit", Event),
    (OnSuspend, "suspend", Event),
    (OnTimeUpdate, "timeupdate", Event),
    (OnToggle, "toggle", Event),
    (OnVolumeChange, "volumechange", Event),
    (OnWaiting, "waiting", Event),
    (OnWheel, "wheel", WheelEvent)
);

pub struct OnResize<V, State, Action, Callback> {
    pub(crate) dom_view: V,
    pub(crate) handler: Callback,
    pub(crate) phantom_event_ty: PhantomData<fn() -> (State, Action)>,
}

pub struct OnResizeState<VState> {
    child_state: VState,
    #[expect(
        dead_code,
        reason = "Closures are retained so they can be called by environment"
    )]
    callback: Closure<dyn FnMut(js_sys::Array)>,
    observer: web_sys::ResizeObserver,
}

impl<V, State, Action, Callback> ViewMarker for OnResize<V, State, Action, Callback> {}
impl<State, Action, OA, Callback, V> View<State, Action, ViewCtx>
    for OnResize<V, State, Action, Callback>
where
    State: 'static,
    Action: 'static,
    OA: OptionalAction<Action>,
    Callback: Fn(&mut State, web_sys::ResizeObserverEntry) -> OA + 'static,
    V: DomView<State, Action, DomNode: AsRef<web_sys::Element>>,
{
    type Element = V::Element;

    type ViewState = OnResizeState<V::ViewState>;

    fn build(&self, ctx: &mut ViewCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        ctx.with_id(ON_EVENT_VIEW_ID, |ctx| {
            let thunk = ctx.message_thunk();
            let callback = Closure::new(move |entries: js_sys::Array| {
                if let Ok(entry) = entries.at(0).dyn_into::<web_sys::ResizeObserverEntry>() {
                    thunk.push_message(entry);
                }
            });

            let observer =
                web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()).unwrap_throw();
            let (element, child_state) = self.dom_view.build(ctx, app_state);
            observer.observe(element.as_ref());

            let state = OnResizeState {
                child_state,
                callback,
                observer,
            };

            (element, state)
        })
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        ctx.with_id(ON_EVENT_VIEW_ID, |ctx| {
            self.dom_view.rebuild(
                &prev.dom_view,
                &mut view_state.child_state,
                ctx,
                element.reborrow_mut(),
                app_state,
            );
            if element.flags.was_created() {
                view_state.observer.disconnect();
                view_state.observer.observe(element.as_ref());
            }
        });
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        element: Mut<'_, Self::Element>,
    ) {
        ctx.with_id(ON_EVENT_VIEW_ID, |ctx| {
            view_state.observer.disconnect();
            self.dom_view
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
        if message.take_first() != Some(ON_EVENT_VIEW_ID) {
            return MessageResult::Stale;
        }
        if message.remaining_path().is_empty() {
            let Some(event) = message.take_message::<web_sys::ResizeObserverEntry>() else {
                return MessageResult::Stale;
            };
            (self.handler)(app_state, *event)
                .action()
                .map_or(MessageResult::Nop, MessageResult::Action)
        } else {
            self.dom_view
                .message(&mut view_state.child_state, message, element, app_state)
        }
    }
}
