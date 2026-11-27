// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Opinionated extension traits roughly resembling their equivalently named DOM interfaces.
//!
//! It is used for DOM elements, e.g. created with [`html::span`](`crate::elements::html::span`) to modify the underlying element, such as [`Element::attr`] or [`Element::style`]
//!
//! These traits can also be used as return type of components to allow modifying the underlying DOM element that is returned.
//! For example:
//! ```ignore
//! fn my_div_element_view() -> impl HtmlDivElement<State> {..}
//! ```
//! A lot of the possible attributes are not yet added, if you find something missing for you - please open a PR at <https://github.com/linebender/xilem/pulls>

use std::borrow::Cow;

use wasm_bindgen::JsCast;

use crate::core::anymore::AnyDebug;
use crate::modifiers::{
    Attr, Class, ClassIter, FormValue, Rotate, Scale, ScaleValue, Style, StyleIter,
};
use crate::props::{WithElementProps, WithHtmlInputElementProps};
use crate::{DomNode, DomView, IntoAttributeValue, OptionalAction, Pointer, PointerMsg, events};

macro_rules! event_handler_mixin {
    ($(($event_ty: ident, $fn_name:ident, $event:expr, $web_sys_event_type:ident),)*) => {
    $(
        #[doc = concat!("Add an \"", $event, "\" event handler to this [`Element`].")]
        ///
        /// See [`Element::on`] for more information how to use this.
        // TODO: This would be nice, but although all the events are specified in `web_sys` on the `Element` interface, events such as `dragend` or `reset` link to the more relevant sub interface
        // We *could* add another parameter to the macro to fix this, or probably even not provide these events directly on the `Element` interface
        // ///
        // #[doc = concat!("See <https://developer.mozilla.org/en-US/docs/Web/API/Element/", $event, "_event> for more details")]
        fn $fn_name<Callback, OA>(
            self,
            handler: Callback,
        ) -> events::$event_ty<Self, State, Action, Callback>
        where
            State: 'static,
            Action: 'static,
            OA: OptionalAction<Action> + 'static,
            Callback: Fn(&mut State, web_sys::$web_sys_event_type) -> OA + 'static,
        {
            events::$event_ty::new(self, handler)
        }
    )*
    };
}

pub trait Element<State: 'static, Action = ()>:
    Sized + DomView<State, Action, DomNode: DomNode<Props: WithElementProps> + AsRef<web_sys::Element>>
{
    /// Set an attribute for an [`Element`]
    ///
    /// # Examples
    ///
    /// ```
    /// use xilem_web::{interfaces::Element, elements::html::{a, canvas, div, input}};
    /// # fn component() -> impl Element<()> {
    /// div((
    ///     a("a link to an anchor").attr("href", "#anchor"),
    ///     // attribute will only appear if condition is met
    ///     // previous attribute is overwritten (and removed if condition is false)
    ///     a("a link to a new anchor - *maybe*")
    ///         .attr("href", "#anchor")
    ///         .attr("href", true.then_some("#new-anchor")),
    ///     input(()).attr("autofocus", true),
    ///     canvas(()).attr("width", 300)
    /// ))
    /// # }
    /// ```
    fn attr(
        self,
        name: impl Into<Cow<'static, str>>,
        value: impl IntoAttributeValue,
    ) -> Attr<Self, State, Action> {
        Attr::new(self, name.into(), value.into_attr_value())
    }

    /// Add a class to an [`Element`]
    ///
    /// # Examples
    ///
    /// ```
    /// use xilem_web::{interfaces::Element, elements::html::div};
    /// # fn component() -> impl Element<()> {
    /// div(())
    ///     .class("single-class")
    ///     .class(["multiple", "classes"])
    ///     .class(Some("optional-class"))
    /// # }
    /// ```
    fn class<AsClasses: ClassIter>(
        self,
        as_classes: AsClasses,
    ) -> Class<Self, AsClasses, State, Action> {
        Class::new(self, as_classes)
    }

    /// Add a generic event handler to this [`Element`].
    ///
    /// For builtin events such as `onclick` prefer using the specialized event handlers (e.g. [`Element::on_click`])
    ///
    /// # Examples
    ///
    /// ```
    /// use xilem_web::{interfaces::Element, elements::html::div};
    /// # fn component() -> impl Element<()> {
    /// div(()).on("custom-event", |state, event: web_sys::Event| {/* modify `state` */})
    /// # }
    /// ```
    fn on<Event, Callback, OA>(
        self,
        event: impl Into<Cow<'static, str>>,
        handler: Callback,
    ) -> events::OnEvent<Self, State, Action, Event, Callback>
    where
        State: 'static,
        Action: 'static,
        OA: OptionalAction<Action>,
        Callback: Fn(&mut State, Event) -> OA + 'static,
        Event: JsCast + 'static + AnyDebug,
    {
        events::OnEvent::new(self, event, handler)
    }

    /// Add a stateful pointer event (down/move/up/cancel) listener to this [`Element`].
    ///
    /// Pointer ids are captured from the underlying element. A started gesture receives
    /// a cancellation on pointer cancellation, lost capture, or element replacement.
    /// Cancellation reports the last known position and no pressed buttons.
    ///
    /// # Examples
    ///
    /// ```
    /// use xilem_web::{interfaces::Element, elements::html::div, PointerDetails, PointerMsg};
    /// use web_sys::console::log_1;
    ///
    /// # fn component() -> impl Element<()> {
    /// div(()).pointer(|_, pointer_msg| {
    ///     match pointer_msg {
    ///         PointerMsg::Down(PointerDetails { position, button, id, .. }) => {
    ///             log_1(&format!("Down({id}) at {position} and button: {button}").into());
    ///         }
    ///         PointerMsg::Move(PointerDetails { position, button, id, .. }) => {
    ///             log_1(&format!("Move({id}) at {position} and button: {button}").into());
    ///         }
    ///         PointerMsg::Cancel(PointerDetails { id, .. }) => {
    ///             log_1(&format!("Cancel({id})").into());
    ///         }
    ///         PointerMsg::Up(PointerDetails { position, button, id, .. }) => {
    ///             log_1(&format!("Up({id}) at {position} and button: {button}").into());
    ///         }
    ///     };
    /// })
    /// # }
    /// ```
    fn pointer<Callback: Fn(&mut State, PointerMsg)>(
        self,
        handler: Callback,
    ) -> Pointer<Self, State, Action, Callback> {
        crate::pointer::pointer(self, handler)
    }

    /// Defines a unique identifier (ID) which must be unique in the whole document.
    /// Its purpose is to identify the element when linking (using a fragment identifier), scripting, or styling (with CSS).
    ///
    /// See <https://developer.mozilla.org/en-US/docs/Web/API/Element/id> for more details
    fn id(self, value: impl IntoAttributeValue) -> Attr<Self, State, Action> {
        Attr::new(self, Cow::from("id"), value.into_attr_value())
    }

    /// Set the [style](https://developer.mozilla.org/en-US/docs/Web/API/HTMLElement/style) attribute
    ///
    /// # Examples
    ///
    /// ```
    /// use xilem_web::{modifiers::style as s, elements::html::div, interfaces::Element};
    ///
    /// # fn component() -> impl Element<()> {
    /// div(())
    ///     .style([s("display", "flex"), s("align-items", "center")])
    ///     .style(s("justify-content", "center"))
    /// # }
    /// ```
    fn style<AsStyles: StyleIter>(self, styles: AsStyles) -> Style<Self, AsStyles, State, Action> {
        Style::new(self, styles)
    }

    /// Add a `rotate(<radians>rad)` [transform-function](https://developer.mozilla.org/en-US/docs/Web/CSS/transform-function) to the current CSS `transform`
    /// # Examples
    ///
    /// ```
    /// use xilem_web::{modifiers::style as s, interfaces::Element, svg::kurbo::Rect};
    ///
    /// # fn component() -> impl Element<()> {
    /// Rect::from_origin_size((0.0, 10.0), (20.0, 30.0))
    ///     .style(s("transform", "translate(10px, 0)")) // can be combined with untyped `transform`
    ///     .rotate(std::f64::consts::PI / 4.0)
    /// // results in the following html:
    /// // <rect width="20" height="30" x="0.0" y="10.0" style="transform: translate(10px, 0) rotate(0.78539rad);"></rect>
    /// # }
    /// ```
    fn rotate(self, radians: f64) -> Rotate<Self, State, Action> {
        Rotate::new(self, radians)
    }

    /// Add a `scale(<scale>)` [transform-function](https://developer.mozilla.org/en-US/docs/Web/CSS/transform-function) to the current CSS `transform`
    /// # Examples
    ///
    /// ```
    /// use xilem_web::{modifiers::style as s, interfaces::Element, svg::kurbo::Circle};
    ///
    /// # fn component() -> impl Element<()> {
    /// Circle::new((10.0, 20.0), 30.0)
    ///     .style(s("transform", "translate(10px, 0)")) // can be combined with untyped `transform`
    ///     .scale(1.5)
    ///     .scale((1.5, 2.0))
    /// // results in the following html:
    /// // <circle r="30" cy="20" cx="10" style="transform: translate(10px, 0) scale(1.5) scale(1.5, 2);"></circle>
    /// # }
    /// ```
    fn scale(self, scale: impl Into<ScaleValue>) -> Scale<Self, State, Action> {
        Scale::new(self, scale)
    }

    // event list from
    // https://html.spec.whatwg.org/multipage/webappapis.html#idl-definitions
    //
    // I didn't include the events on the window, since we aren't attaching
    // any events to the window in xilem_web
    event_handler_mixin!(
        (OnAbort, on_abort, "abort", Event),
        (OnAuxClick, on_auxclick, "auxclick", PointerEvent),
        (OnBeforeInput, on_beforeinput, "beforeinput", InputEvent),
        (OnBeforeMatch, on_beforematch, "beforematch", Event),
        (OnBeforeToggle, on_beforetoggle, "beforetoggle", Event),
        (OnBlur, on_blur, "blur", FocusEvent),
        (OnCancel, on_cancel, "cancel", Event),
        (OnCanPlay, on_canplay, "canplay", Event),
        (OnCanPlayThrough, on_canplaythrough, "canplaythrough", Event),
        (OnChange, on_change, "change", Event),
        (OnClick, on_click, "click", PointerEvent),
        (OnClose, on_close, "close", Event),
        (OnContextLost, on_contextlost, "contextlost", Event),
        (OnContextMenu, on_contextmenu, "contextmenu", PointerEvent),
        (
            OnContextRestored,
            on_contextrestored,
            "contextrestored",
            Event
        ),
        (OnCopy, on_copy, "copy", Event),
        (OnCueChange, on_cuechange, "cuechange", Event),
        (OnCut, on_cut, "cut", Event),
        (OnDblClick, on_dblclick, "dblclick", MouseEvent),
        (OnDrag, on_drag, "drag", Event),
        (OnDragEnd, on_dragend, "dragend", Event),
        (OnDragEnter, on_dragenter, "dragenter", Event),
        (OnDragLeave, on_dragleave, "dragleave", Event),
        (OnDragOver, on_dragover, "dragover", Event),
        (OnDragStart, on_dragstart, "dragstart", Event),
        (OnDrop, on_drop, "drop", Event),
        (OnDurationChange, on_durationchange, "durationchange", Event),
        (OnEmptied, on_emptied, "emptied", Event),
        (OnEnded, on_ended, "ended", Event),
        (OnError, on_error, "error", Event),
        (OnFocus, on_focus, "focus", FocusEvent),
        (OnFocusIn, on_focusin, "focusin", FocusEvent),
        (OnFocusOut, on_focusout, "focusout", FocusEvent),
        (OnFormData, on_formdata, "formdata", Event),
        (OnInput, on_input, "input", Event),
        (OnInvalid, on_invalid, "invalid", Event),
        (OnKeyDown, on_keydown, "keydown", KeyboardEvent),
        (OnKeyUp, on_keyup, "keyup", KeyboardEvent),
        (OnLoad, on_load, "load", Event),
        (OnLoadedData, on_loadeddata, "loadeddata", Event),
        (OnLoadedMetadata, on_loadedmetadata, "loadedmetadata", Event),
        (OnLoadStart, on_loadstart, "loadstart", Event),
        (OnMouseDown, on_mousedown, "mousedown", MouseEvent),
        (OnMouseEnter, on_mouseenter, "mouseenter", MouseEvent),
        (OnMouseLeave, on_mouseleave, "mouseleave", MouseEvent),
        (OnMouseMove, on_mousemove, "mousemove", MouseEvent),
        (OnMouseOut, on_mouseout, "mouseout", MouseEvent),
        (OnMouseOver, on_mouseover, "mouseover", MouseEvent),
        (OnMouseUp, on_mouseup, "mouseup", MouseEvent),
        (OnPaste, on_paste, "paste", Event),
        (OnPause, on_pause, "pause", Event),
        (OnPlay, on_play, "play", Event),
        (OnPlaying, on_playing, "playing", Event),
        (
            OnPointerCancel,
            on_pointercancel,
            "pointercancel",
            PointerEvent
        ),
        (OnPointerDown, on_pointerdown, "pointerdown", PointerEvent),
        (
            OnPointerEnter,
            on_pointerenter,
            "pointerenter",
            PointerEvent
        ),
        (
            OnPointerLeave,
            on_pointerleave,
            "pointerleave",
            PointerEvent
        ),
        (OnPointerMove, on_pointermove, "pointermove", PointerEvent),
        (OnPointerOut, on_pointerout, "pointerout", PointerEvent),
        (OnPointerOver, on_pointerover, "pointerover", PointerEvent),
        (
            OnPointerRawUpdate,
            on_pointerrawupdate,
            "pointerrawupdate",
            PointerEvent
        ),
        (OnPointerUp, on_pointerup, "pointerup", PointerEvent),
        (OnProgress, on_progress, "progress", Event),
        (OnRateChange, on_ratechange, "ratechange", Event),
        (OnReset, on_reset, "reset", Event),
        (OnScroll, on_scroll, "scroll", Event),
        (OnScrollEnd, on_scrollend, "scrollend", Event),
        (
            OnSecurityPolicyViolation,
            on_securitypolicyviolation,
            "securitypolicyviolation",
            Event
        ),
        (OnSeeked, on_seeked, "seeked", Event),
        (OnSeeking, on_seeking, "seeking", Event),
        (OnSelect, on_select, "select", Event),
        (OnSlotChange, on_slotchange, "slotchange", Event),
        (OnStalled, on_stalled, "stalled", Event),
        (OnSubmit, on_submit, "submit", Event),
        (OnSuspend, on_suspend, "suspend", Event),
        (OnTimeUpdate, on_timeupdate, "timeupdate", Event),
        (OnToggle, on_toggle, "toggle", Event),
        (OnVolumeChange, on_volumechange, "volumechange", Event),
        (OnWaiting, on_waiting, "waiting", Event),
        (OnWheel, on_wheel, "wheel", WheelEvent),
    );

    /// Register a [`web_sys::ResizeObserver`] on this [`Element`].
    ///
    /// See <https://developer.mozilla.org/en-US/docs/Web/API/ResizeObserver/ResizeObserver> for more details
    ///
    /// Note, that this is a different than <https://developer.mozilla.org/en-US/docs/Web/API/Window/resize_event>
    ///
    /// # Examples
    ///
    /// ```
    /// use xilem_web::{interfaces::Element, elements::html::{a, canvas, div, input}};
    ///
    /// fn observe_size() -> impl Element<(f64, f64)> {
    ///     div(()).on_resize(|(width, height): &mut (f64, f64), resize_observer_entry| {
    ///         *width = resize_observer_entry.content_rect().width();
    ///         *height = resize_observer_entry.content_rect().height();
    ///     })
    /// }
    /// ```
    fn on_resize<Callback, OA>(
        self,
        handler: Callback,
    ) -> events::OnResize<Self, State, Action, Callback>
    where
        Action: 'static,
        OA: OptionalAction<Action>,
        Callback: Fn(&mut State, web_sys::ResizeObserverEntry) -> OA + 'static,
        Self::Element: AsRef<web_sys::Element>,
    {
        events::OnResize {
            dom_view: self,
            handler,
            phantom_event_ty: std::marker::PhantomData,
        }
    }
}

impl<State, Action, T> Element<State, Action> for T
where
    State: 'static,
    T: DomView<State, Action>,
    <T::DomNode as DomNode>::Props: WithElementProps,
    T::DomNode: AsRef<web_sys::Element>,
{
}

macro_rules! dom_interface {
    ($name:ident : $parent:ident => $target:ident { $($items:tt)* }) => {
        pub trait $name<State: 'static, Action = ()>:
            $parent<State, Action, DomNode: AsRef<web_sys::$target>>
        {
            $($items)*
        }

        impl<State, Action, T> $name<State, Action> for T
        where
            State: 'static,
            T: $parent<State, Action>,
            T::DomNode: AsRef<web_sys::$target>,
        {}
    };
}

dom_interface!(HtmlAnchorElement: HtmlElement => HtmlAnchorElement {
});

dom_interface!(HtmlAreaElement: HtmlElement => HtmlAreaElement {
});

dom_interface!(HtmlAudioElement: HtmlMediaElement => HtmlAudioElement {
});

// #[cfg(feature = "HtmlBaseElement")]
// pub trait HtmlBaseElement<State, Action = ()>:
//     HtmlElement<State, Action, DomNode: AsRef<web_sys::HtmlBaseElement>>
// {
// }

// #[cfg(feature = "HtmlBaseElement")]
// impl<State, Action, T> HtmlBaseElement<State, Action> for T
// where
//    State:  ViewArgument,
//     T: HtmlElement<State, Action>,
//     T::DomNode: AsRef<web_sys::HtmlBaseElement>,
// {
// }

// #[cfg(feature = "HtmlBodyElement")]
// pub trait HtmlBodyElement<State, Action = ()>:
//     HtmlElement<State, Action, DomNode: AsRef<web_sys::HtmlBodyElement>>
// {
// }

// #[cfg(feature = "HtmlBodyElement")]
// impl<State, Action, T> HtmlBodyElement<State, Action> for T
// where
//    State:  ViewArgument,
//     T: HtmlElement<State, Action>,
//     T::DomNode: AsRef<web_sys::HtmlBodyElement>,
// {
// }

dom_interface!(HtmlBrElement: HtmlElement => HtmlBrElement {
});

dom_interface!(HtmlButtonElement: HtmlElement => HtmlButtonElement {
    /// See <https://developer.mozilla.org/en-US/docs/Web/HTML/Attributes/disabled> for more details
    fn disabled(self, disable: bool) -> Attr<Self, State, Action> {
        Attr::new(self, "disabled".into(), disable.into_attr_value())
    }
});

dom_interface!(HtmlCanvasElement: HtmlElement => HtmlCanvasElement {
    /// See <https://developer.mozilla.org/en-US/docs/Web/API/HTMLCanvasElement/width> for more details
    fn width(self, value: u32) -> Attr<Self, State, Action> {
        Attr::new(self, "width".into(), value.into_attr_value())
    }
});

dom_interface!(HtmlDataElement: HtmlElement => HtmlDataElement {
});

dom_interface!(HtmlDataListElement: HtmlElement => HtmlDataListElement {
});

dom_interface!(HtmlDetailsElement: HtmlElement => HtmlDetailsElement {
});

dom_interface!(HtmlDialogElement: HtmlElement => HtmlDialogElement {
});

// #[cfg(feature = "HtmlDirectoryElement")]
// pub trait HtmlDirectoryElement<State, Action = ()>:
//     HtmlElement<State, Action, DomNode: AsRef<web_sys::HtmlDirectoryElement>>
// {
// }

// #[cfg(feature = "HtmlDirectoryElement")]
// impl<State, Action, T> HtmlDirectoryElement<State, Action> for T
// where
//    State:  ViewArgument,
//     T: HtmlElement<State, Action>,
//     T::DomNode: AsRef<web_sys::HtmlDirectoryElement>,
// {
// }

dom_interface!(HtmlDivElement: HtmlElement => HtmlDivElement {
});

dom_interface!(HtmlDListElement: HtmlElement => HtmlDListElement {
});

dom_interface!(HtmlElement: Element => HtmlElement {
});

// #[cfg(feature = "HtmlUnknownElement")]
// pub trait HtmlUnknownElement<State, Action = ()>:
//     HtmlElement<State, Action, DomNode: AsRef<web_sys::HtmlUnknownElement>>
// {
// }

// #[cfg(feature = "HtmlUnknownElement")]
// impl<State, Action, T> HtmlUnknownElement<State, Action> for T
// where
//     State:  ViewArgument,
//     T: HtmlElement<State, Action>,
//     T::DomNode: AsRef<web_sys::HtmlUnknownElement>,
// {
// }

dom_interface!(HtmlEmbedElement: HtmlElement => HtmlEmbedElement {
});

dom_interface!(HtmlFieldSetElement: HtmlElement => HtmlFieldSetElement {
});

// #[cfg(feature = "HtmlFontElement")]
// pub trait HtmlFontElement<State, Action = ()>:
//     HtmlElement<State, Action, DomNode: AsRef<web_sys::HtmlFontElement>>
// {
// }

// #[cfg(feature = "HtmlFontElement")]
// impl<State, Action, T> HtmlFontElement<State, Action> for T
// where
//     State:  ViewArgument,
//     T: HtmlElement<State, Action>,
//     T::DomNode: AsRef<web_sys::HtmlFontElement>,
// {
// }

dom_interface!(HtmlFormElement: HtmlElement => HtmlFormElement {
});

// #[cfg(feature = "HtmlFrameElement")]
// pub trait HtmlFrameElement<State, Action = ()>:
//     HtmlElement<State, Action, DomNode: AsRef<web_sys::HtmlFrameElement>>
// {
// }

// #[cfg(feature = "HtmlFrameElement")]
// impl<State, Action, T> HtmlFrameElement<State, Action> for T
// where
//     State:  ViewArgument,
//     T: HtmlElement<State, Action>,
//     T::DomNode: AsRef<web_sys::HtmlFrameElement>,
// {
// }

// #[cfg(feature = "HtmlFrameSetElement")]
// pub trait HtmlFrameSetElement<State, Action = ()>:
//     HtmlElement<State, Action, DomNode: AsRef<web_sys::HtmlFrameSetElement>>
// {
// }

// #[cfg(feature = "HtmlFrameSetElement")]
// impl<State, Action, T> HtmlFrameSetElement<State, Action> for T
// where
//     State:  ViewArgument,
//     T: HtmlElement<State, Action>,
//     T::DomNode: AsRef<web_sys::HtmlFrameSetElement>,
// {
// }

// #[cfg(feature = "HtmlHeadElement")]
// pub trait HtmlHeadElement<State, Action = ()>:
//     HtmlElement<State, Action, DomNode: AsRef<web_sys::HtmlHeadElement>>
// {
// }

// #[cfg(feature = "HtmlHeadElement")]
// impl<State, Action, T> HtmlHeadElement<State, Action> for T
// where
//     State:  ViewArgument,
//     T: HtmlElement<State, Action>,
//     T::DomNode: AsRef<web_sys::HtmlHeadElement>,
// {
// }

dom_interface!(HtmlHeadingElement: HtmlElement => HtmlHeadingElement {
});

dom_interface!(HtmlHrElement: HtmlElement => HtmlHrElement {
});

// #[cfg(feature = "HtmlHtmlElement")]
// pub trait HtmlHtmlElement<State, Action = ()>:
//     HtmlElement<State, Action, DomNode: AsRef<web_sys::HtmlHtmlElement>>
// {
// }

// #[cfg(feature = "HtmlHtmlElement")]
// impl<State, Action, T> HtmlHtmlElement<State, Action> for T
// where
//     State:  ViewArgument,
//     T: HtmlElement<State, Action>,
//     T::DomNode: AsRef<web_sys::HtmlHtmlElement>,
// {
// }

dom_interface!(HtmlIFrameElement: HtmlElement => HtmlIFrameElement {
    /// See <https://developer.mozilla.org/en-US/docs/Web/API/HTMLIFrameElement/src> for more details
    fn src(self, value: impl IntoAttributeValue) -> Attr<Self, State, Action> {
        Attr::new(self, "src".into(), value.into_attr_value())
    }
});

dom_interface!(HtmlImageElement: HtmlElement => HtmlImageElement {
    /// See <https://developer.mozilla.org/en-US/docs/Web/API/HTMLImageElement/src> for more details
    fn src(self, value: impl IntoAttributeValue) -> Attr<Self, State, Action> {
        Attr::new(self, Cow::from("src"), value.into_attr_value())
    }
});

use crate::modifiers::html_input_element;
// #[cfg(feature = "HtmlInputElement")]
pub trait HtmlInputElement<State: 'static, Action = ()>:
    HtmlElement<
        State,
        Action,
        DomNode: DomNode<Props: WithHtmlInputElementProps> + AsRef<web_sys::HtmlInputElement>,
    >
{
    /// Control the live input value, not its default value attribute.
    /// A missing value clears the property. Browser input-type sanitization still
    /// applies; file inputs may only be set to the empty string.
    /// Use the value content attribute for the default value instead.
    fn value(self, value: impl IntoAttributeValue) -> FormValue<Self, State, Action> {
        FormValue::new(self, value)
    }

    /// See <https://developer.mozilla.org/en-US/docs/Web/API/HTMLInputElement/checked> for more details.
    ///
    /// # Examples
    ///
    /// ```
    /// use xilem_web::{interfaces::{Element, HtmlInputElement}, elements::html::input};
    ///
    /// # fn component() -> impl HtmlInputElement<()> {
    /// input(()).type_("checkbox").checked(true) // results in <input type="checkbox" checked></input>
    /// # }
    /// ```
    fn checked(self, checked: bool) -> html_input_element::view::Checked<Self, State, Action> {
        html_input_element::view::Checked::new(self, checked)
    }

    /// See <https://developer.mozilla.org/en-US/docs/Web/API/HTMLInputElement/type> for more details.
    ///
    /// # Examples
    ///
    /// ```
    /// use xilem_web::{interfaces::{Element, HtmlInputElement}, elements::html::input};
    ///
    /// # fn component() -> impl HtmlInputElement<()> {
    /// input(()).type_("radio") // results in <input type="radio"></input>
    /// # }
    /// ```
    fn type_(self, value: impl Into<Cow<'static, str>>) -> Attr<Self, State, Action> {
        Attr::new(self, "type".into(), value.into().into_attr_value())
    }

    /// See <https://developer.mozilla.org/en-US/docs/Web/API/HTMLInputElement/name> for more details.
    ///
    /// # Examples
    ///
    /// ```
    /// use xilem_web::{interfaces::{Element, HtmlInputElement}, elements::html::input};
    ///
    /// # fn component() -> impl HtmlInputElement<()> {
    /// input(()).name("color") // results in <input name="color"></input>
    /// # }
    /// ```
    fn name(self, value: impl Into<Cow<'static, str>>) -> Attr<Self, State, Action> {
        Attr::new(self, "name".into(), value.into().into_attr_value())
    }

    /// See <https://developer.mozilla.org/en-US/docs/Web/API/HTMLInputElement/checked> for more details.
    ///
    /// # Examples
    ///
    /// ```
    /// use xilem_web::{interfaces::{Element, HtmlInputElement}, elements::html::input};
    ///
    /// # fn component() -> impl HtmlInputElement<()> {
    /// input(()).type_("radio").default_checked(true) // results in <input type="radio" checked></input>
    /// # }
    /// ```
    fn default_checked(
        self,
        default_checked: bool,
    ) -> html_input_element::view::DefaultChecked<Self, State, Action> {
        html_input_element::view::DefaultChecked::new(self, default_checked)
    }

    /// See <https://developer.mozilla.org/en-US/docs/Web/API/HTMLInputElement/disabled> for more details.
    ///
    /// # Examples
    ///
    /// ```
    /// use xilem_web::{interfaces::{Element, HtmlInputElement}, elements::html::input};
    ///
    /// # fn component() -> impl HtmlInputElement<()> {
    /// input(()).disabled(true) // results in <input disabled></input>
    /// # }
    /// ```
    fn disabled(self, disabled: bool) -> html_input_element::view::Disabled<Self, State, Action> {
        html_input_element::view::Disabled::new(self, disabled)
    }

    /// See <https://developer.mozilla.org/en-US/docs/Web/API/HTMLInputElement/disabled> for more details.
    ///
    /// # Examples
    ///
    /// ```
    /// use xilem_web::{interfaces::{Element, HtmlInputElement}, elements::html::input};
    ///
    /// # fn component() -> impl HtmlInputElement<()> {
    /// input(()).required(true) // results in <input required></input>
    /// # }
    /// ```
    fn required(self, required: bool) -> html_input_element::view::Required<Self, State, Action> {
        html_input_element::view::Required::new(self, required)
    }

    /// See <https://developer.mozilla.org/en-US/docs/Web/API/HTMLInputElement/multiple> for more details.
    ///
    /// # Examples
    ///
    /// ```
    /// use xilem_web::{interfaces::{Element, HtmlInputElement}, elements::html::input};
    ///
    /// # fn component() -> impl HtmlInputElement<()> {
    /// input(()).multiple(true) // results in <input multiple></input>
    /// # }
    /// ```
    fn multiple(self, multiple: bool) -> html_input_element::view::Multiple<Self, State, Action> {
        html_input_element::view::Multiple::new(self, multiple)
    }
}

// #[cfg(feature = "HtmlInputElement")]
impl<State, Action, T> HtmlInputElement<State, Action> for T
where
    State: 'static,
    T: HtmlElement<State, Action>,
    T::DomNode: AsRef<web_sys::HtmlInputElement>,
    <T::DomNode as DomNode>::Props: WithHtmlInputElementProps,
{
}

dom_interface!(HtmlLabelElement: HtmlElement => HtmlLabelElement {
    /// The first element in the document with an id attribute matching the value of the for attribute is the labeled control for this label element — if the element with that id is actually a labelable element.
    /// If it is not a labelable element, then the for attribute has no effect.
    /// If there are other elements that also match the id value, later in the document, they are not considered.
    ///
    /// See <https://developer.mozilla.org/en-US/docs/Web/HTML/Element/label#for> for more details
    // TODO different name?
    fn for_(self, value: impl IntoAttributeValue) -> Attr<Self, State, Action> {
        Attr::new(self, Cow::from("for"), value.into_attr_value())
    }
});

dom_interface!(HtmlLegendElement: HtmlElement => HtmlLegendElement {
});

dom_interface!(HtmlLiElement: HtmlElement => HtmlLiElement {
    /// See <https://developer.mozilla.org/en-US/docs/Web/HTML/Element/li#value> for more details
    fn value(self, value: i32) -> Attr<Self, State, Action> {
        Attr::new(self, "value".into(), value.into_attr_value())
    }
});

dom_interface!(HtmlLinkElement: HtmlElement => HtmlLinkElement {
});

dom_interface!(HtmlMapElement: HtmlElement => HtmlMapElement {
});

dom_interface!(HtmlMediaElement: HtmlElement => HtmlMediaElement {
});

dom_interface!(HtmlMenuElement: HtmlElement => HtmlMenuElement {
});

// #[cfg(feature = "HtmlMenuItemElement")]
// pub trait HtmlMenuItemElement<State, Action = ()>:
//     HtmlElement<State, Action, DomNode: AsRef<web_sys::HtmlMenuItemElement>>
// {
// }

// #[cfg(feature = "HtmlMenuItemElement")]
// impl<State, Action, T> HtmlMenuItemElement<State, Action> for T
// where
//     State:  ViewArgument,
//     T: HtmlElement<State, Action>,
//     T::DomNode: AsRef<web_sys::HtmlMenuItemElement>,
// {
// }

// #[cfg(feature = "HtmlMetaElement")]
// pub trait HtmlMetaElement<State, Action = ()>:
//     HtmlElement<State, Action, DomNode: AsRef<web_sys::HtmlMetaElement>>
// {
// }

// #[cfg(feature = "HtmlMetaElement")]
// impl<State, Action, T> HtmlMetaElement<State, Action> for T
// where
//     State:  ViewArgument,
//     T: HtmlElement<State, Action>,
//     T::DomNode: AsRef<web_sys::HtmlMetaElement>,
// {
// }

dom_interface!(HtmlMeterElement: HtmlElement => HtmlMeterElement {
});

dom_interface!(HtmlModElement: HtmlElement => HtmlModElement {
});

dom_interface!(HtmlObjectElement: HtmlElement => HtmlObjectElement {
});

dom_interface!(HtmlOListElement: HtmlElement => HtmlOListElement {
});

dom_interface!(HtmlOptGroupElement: HtmlElement => HtmlOptGroupElement {
});

dom_interface!(HtmlOptionElement: HtmlElement => HtmlOptionElement {
    /// A string representing the value of the `HTMLOptionElement`, i.e. the value attribute of the equivalent `<option>`.
    /// If this is not specified, the value of text is used as the value, e.g. for the associated `<select>` element's value when the form is submitted to the server.
    ///
    /// See <https://developer.mozilla.org/en-US/docs/Web/HTML/Element/option#value> for more details
    fn value(self, value: impl IntoAttributeValue) -> Attr<Self, State, Action> {
        Attr::new(self, Cow::from("value"), value.into_attr_value())
    }

    /// If present, this Boolean attribute indicates that the option is initially selected.
    /// If the `<option>` element is the descendant of a `<select>` element whose multiple attribute is not set, only one single `<option>` of this `<select>` element may have the selected attribute.
    ///
    /// See <https://developer.mozilla.org/en-US/docs/Web/HTML/Element/option#selected> for more details
    fn selected(self, selected: bool) -> Attr<Self, State, Action> {
        Attr::new(self, Cow::from("selected"), selected.into_attr_value())
    }
});

dom_interface!(HtmlOutputElement: HtmlElement => HtmlOutputElement {
});

dom_interface!(HtmlParagraphElement: HtmlElement => HtmlParagraphElement {
});

// #[cfg(feature = "HtmlParamElement")]
// pub trait HtmlParamElement<State, Action = ()>:
//     HtmlElement<State, Action, DomNode: AsRef<web_sys::HtmlParamElement>>
// {
// }

// #[cfg(feature = "HtmlParamElement")]
// impl<State, Action, T> HtmlParamElement<State, Action> for T
// where
//     State:  ViewArgument,
//     T: HtmlElement<State, Action>,
//     T::DomNode: AsRef<web_sys::HtmlParamElement>,
// {
// }

dom_interface!(HtmlPictureElement: HtmlElement => HtmlPictureElement {
});

dom_interface!(HtmlPreElement: HtmlElement => HtmlPreElement {
});

dom_interface!(HtmlProgressElement: HtmlElement => HtmlProgressElement {
});

dom_interface!(HtmlQuoteElement: HtmlElement => HtmlQuoteElement {
});

dom_interface!(HtmlScriptElement: HtmlElement => HtmlScriptElement {
});

dom_interface!(HtmlSelectElement: HtmlElement => HtmlSelectElement {
    /// Control the live selected value after reconciling options.
    /// A missing value selects the empty-valued option if present, or clears
    /// selection otherwise. For multiple selections use individual option selection.
    fn value(self, value: impl IntoAttributeValue) -> FormValue<Self, State, Action> {
        FormValue::new(self, value)
    }
});

dom_interface!(HtmlSlotElement: HtmlElement => HtmlSlotElement {
});

dom_interface!(HtmlSourceElement: HtmlElement => HtmlSourceElement {
});

dom_interface!(HtmlSpanElement: HtmlElement => HtmlSpanElement {
});

// #[cfg(feature = "HtmlStyleElement")]
// pub trait HtmlStyleElement<State, Action = ()>:
//     HtmlElement<State, Action, DomNode: AsRef<web_sys::HtmlStyleElement>>
// {
// }

// #[cfg(feature = "HtmlStyleElement")]
// impl<State, Action, T> HtmlStyleElement<State, Action> for T
// where
//     State:  ViewArgument,
//     T: HtmlElement<State, Action>,
//     T::DomNode: AsRef<web_sys::HtmlStyleElement>,
// {
// }

dom_interface!(HtmlTableCaptionElement: HtmlElement => HtmlTableCaptionElement {
});

dom_interface!(HtmlTableCellElement: HtmlElement => HtmlTableCellElement {
});

dom_interface!(HtmlTableColElement: HtmlElement => HtmlTableColElement {
});

dom_interface!(HtmlTableElement: HtmlElement => HtmlTableElement {
});

dom_interface!(HtmlTableRowElement: HtmlElement => HtmlTableRowElement {
});

dom_interface!(HtmlTableSectionElement: HtmlElement => HtmlTableSectionElement {
});

dom_interface!(HtmlTemplateElement: HtmlElement => HtmlTemplateElement {
});

dom_interface!(HtmlTimeElement: HtmlElement => HtmlTimeElement {
});

dom_interface!(HtmlTextAreaElement: HtmlElement => HtmlTextAreaElement {
    /// Control the live textarea value, independently of its default text.
    /// A missing value clears the property.
    fn value(self, value: impl IntoAttributeValue) -> FormValue<Self, State, Action> {
        FormValue::new(self, value)
    }
});

// #[cfg(feature = "HtmlTitleElement")]
// pub trait HtmlTitleElement<State, Action = ()>:
//     HtmlElement<State, Action, DomNode: AsRef<web_sys::HtmlTitleElement>>
// {
// }

// #[cfg(feature = "HtmlTitleElement")]
// impl<State, Action, T> HtmlTitleElement<State, Action> for T
// where
//     State:  ViewArgument,
//     T: HtmlElement<State, Action>,
//     T::DomNode: AsRef<web_sys::HtmlTitleElement>,
// {
// }

dom_interface!(HtmlTrackElement: HtmlElement => HtmlTrackElement {
});

dom_interface!(HtmlUListElement: HtmlElement => HtmlUListElement {
});

dom_interface!(HtmlVideoElement: HtmlMediaElement => HtmlVideoElement {
});

dom_interface!(SvgaElement: SvgGraphicsElement => SvgaElement {
});

dom_interface!(SvgAnimateElement: SvgAnimationElement => SvgAnimateElement {
});

dom_interface!(SvgAnimateMotionElement: SvgAnimationElement => SvgAnimateMotionElement {
});

dom_interface!(SvgAnimateTransformElement: SvgAnimationElement => SvgAnimateTransformElement {
});

dom_interface!(SvgAnimationElement: SvgElement => SvgAnimationElement {
});

dom_interface!(SvgCircleElement: SvgGeometryElement => SvgCircleElement {
});

dom_interface!(SvgClipPathElement: SvgElement => SvgClipPathElement {
});

dom_interface!(SvgComponentTransferFunctionElement: SvgElement => SvgComponentTransferFunctionElement {
});

dom_interface!(SvgDefsElement: SvgGraphicsElement => SvgDefsElement {
});

dom_interface!(SvgDescElement: SvgElement => SvgDescElement {
});

dom_interface!(SvgElement: Element => SvgElement {
    fn fill(self, color: impl Into<peniko::Color>) -> crate::svg::Fill<Self, State, Action> {
        crate::svg::fill(self, color)
    }
    fn stroke(
        self,
        color: impl Into<peniko::Color>,
        style: crate::svg::StrokeStyle,
    ) -> crate::svg::Stroke<Self, State, Action> {
        crate::svg::stroke(self, color, style)
    }
});

dom_interface!(SvgEllipseElement: SvgGeometryElement => SvgEllipseElement {
});

dom_interface!(SvgfeBlendElement: SvgElement => SvgfeBlendElement {
});

dom_interface!(SvgfeColorMatrixElement: SvgElement => SvgfeColorMatrixElement {
});

dom_interface!(SvgfeComponentTransferElement: SvgElement => SvgfeComponentTransferElement {
});

dom_interface!(SvgfeCompositeElement: SvgElement => SvgfeCompositeElement {
});

dom_interface!(SvgfeConvolveMatrixElement: SvgElement => SvgfeConvolveMatrixElement {
});

dom_interface!(SvgfeDiffuseLightingElement: SvgElement => SvgfeDiffuseLightingElement {
});

dom_interface!(SvgfeDisplacementMapElement: SvgElement => SvgfeDisplacementMapElement {
});

dom_interface!(SvgfeDistantLightElement: SvgElement => SvgfeDistantLightElement {
});

dom_interface!(SvgfeDropShadowElement: SvgElement => SvgfeDropShadowElement {
});

dom_interface!(SvgfeFloodElement: SvgElement => SvgfeFloodElement {
});

dom_interface!(SvgfeFuncAElement: SvgComponentTransferFunctionElement => SvgfeFuncAElement {
});

dom_interface!(SvgfeFuncBElement: SvgComponentTransferFunctionElement => SvgfeFuncBElement {
});

dom_interface!(SvgfeFuncGElement: SvgComponentTransferFunctionElement => SvgfeFuncGElement {
});

dom_interface!(SvgfeFuncRElement: SvgComponentTransferFunctionElement => SvgfeFuncRElement {
});

dom_interface!(SvgfeGaussianBlurElement: SvgElement => SvgfeGaussianBlurElement {
});

dom_interface!(SvgfeImageElement: SvgElement => SvgfeImageElement {
});

dom_interface!(SvgfeMergeElement: SvgElement => SvgfeMergeElement {
});

dom_interface!(SvgfeMergeNodeElement: SvgElement => SvgfeMergeNodeElement {
});

dom_interface!(SvgfeMorphologyElement: SvgElement => SvgfeMorphologyElement {
});

dom_interface!(SvgfeOffsetElement: SvgElement => SvgfeOffsetElement {
});

dom_interface!(SvgfePointLightElement: SvgElement => SvgfePointLightElement {
});

dom_interface!(SvgfeSpecularLightingElement: SvgElement => SvgfeSpecularLightingElement {
});

dom_interface!(SvgfeSpotLightElement: SvgElement => SvgfeSpotLightElement {
});

dom_interface!(SvgfeTileElement: SvgElement => SvgfeTileElement {
});

dom_interface!(SvgfeTurbulenceElement: SvgElement => SvgfeTurbulenceElement {
});

dom_interface!(SvgFilterElement: SvgElement => SvgFilterElement {
});

dom_interface!(SvgForeignObjectElement: SvgGraphicsElement => SvgForeignObjectElement {
});

dom_interface!(SvggElement: SvgGraphicsElement => SvggElement {
});

dom_interface!(SvgGeometryElement: SvgGraphicsElement => SvgGeometryElement {
});

dom_interface!(SvgGradientElement: SvgElement => SvgGradientElement {
});

dom_interface!(SvgGraphicsElement: SvgElement => SvgGraphicsElement {
});

dom_interface!(SvgImageElement: SvgGraphicsElement => SvgImageElement {
});

dom_interface!(SvgLinearGradientElement: SvgGradientElement => SvgLinearGradientElement {
});

dom_interface!(SvgLineElement: SvgGeometryElement => SvgLineElement {
});

dom_interface!(SvgMarkerElement: SvgElement => SvgMarkerElement {
});

dom_interface!(SvgMaskElement: SvgElement => SvgMaskElement {
});

dom_interface!(SvgMetadataElement: SvgElement => SvgMetadataElement {
});

dom_interface!(SvgmPathElement: SvgElement => SvgmPathElement {
});

dom_interface!(SvgPathElement: SvgGeometryElement => SvgPathElement {
});

dom_interface!(SvgPatternElement: SvgElement => SvgPatternElement {
});

dom_interface!(SvgPolygonElement: SvgGeometryElement => SvgPolygonElement {
});

dom_interface!(SvgPolylineElement: SvgGeometryElement => SvgPolylineElement {
});

dom_interface!(SvgRectElement: SvgGeometryElement => SvgRectElement {
});

dom_interface!(SvgScriptElement: SvgElement => SvgScriptElement {
});

dom_interface!(SvgSetElement: SvgAnimationElement => SvgSetElement {
});

dom_interface!(SvgStopElement: SvgElement => SvgStopElement {
});

dom_interface!(SvgStyleElement: SvgElement => SvgStyleElement {
});

dom_interface!(SvgSwitchElement: SvgGraphicsElement => SvgSwitchElement {
});

dom_interface!(SvgSymbolElement: SvgElement => SvgSymbolElement {
});

dom_interface!(SvgTextContentElement: SvgGraphicsElement => SvgTextContentElement {
});

dom_interface!(SvgTextPathElement: SvgTextContentElement => SvgTextPathElement {
});

dom_interface!(SvgTextPositioningElement: SvgTextContentElement => SvgTextPositioningElement {
});

dom_interface!(SvgtSpanElement: SvgTextPositioningElement => SvgtSpanElement {
});

dom_interface!(SvgViewElement: SvgElement => SvgViewElement {
});

dom_interface!(SvgRadialGradientElement: SvgGradientElement => SvgRadialGradientElement {
});

dom_interface!(SvgsvgElement: SvgGraphicsElement => SvgsvgElement {
});

dom_interface!(SvgTextElement: SvgTextPositioningElement => SvgTextElement {
});

dom_interface!(SvgTitleElement: SvgElement => SvgTitleElement {
});

dom_interface!(SvgUseElement: SvgGraphicsElement => SvgUseElement {
});
