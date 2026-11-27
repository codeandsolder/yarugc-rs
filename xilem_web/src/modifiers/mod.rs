// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! This module contains DOM element modifiers, e.g. to add attributes/classes/styles.
//!
//! A modifier is usually a part/attribute of a [`ViewElement`](crate::core::ViewElement),
//! and has corresponding Views, usually meant to be used in a builder-style.
//!
//! One such example is setting attributes on a DOM element, like this:
//! ```
//! use xilem_web::{interfaces::Element, elements::html::{a, canvas, input}};
//! // ...
//! # use xilem_web::elements::html::div;
//! # fn component() -> impl Element<()> {
//! # div((
//! a("a link to an anchor").attr("href", "#anchor"),
//! // attribute will only appear if condition is met
//! // previous attribute is overwritten (and removed if condition is false)
//! a("a link to a new anchor - *maybe*")
//!     .attr("href", "#anchor")
//!     .attr("href", true.then_some("#new-anchor")),
//! input(()).attr("autofocus", true),
//! canvas(()).attr("width", 300)
//! # ))
//! # }
//! ```
//!
//! These modifiers have to fulfill some properties to be able to be used without unwanted side-effects.
//! As the modifier-views are usually depending on a bound on its `View::Element`, the following needs to be supported:
//!
//! ```
//! use xilem_web::{
//!     core::{frozen, one_of::Either},
//!     interfaces::Element,
//!     elements::html::{div, span},
//!     modifiers::style as s,
//! };
//! // ...
//! # fn component() -> impl Element<()> {
//! # div((
//! // Memoized views may never update their memoized modifiers:
//! frozen(|| div("this will be created only once").class("shadow"))
//!     .class(["text-center", "flex"]),
//! // For some cases be able to read possibly memoized modifiers.
//! // Following results in the style attribute:
//! // `transform: translate(10px, 10px) scale(2.0)` and is updated, when `.scale` changes
//! frozen(|| div("transformed").style(s("transform", "translate(10px, 10px)")))
//!     .scale(2.0),
//! // OneOf/Either views can change their underlying element type, while supporting the same modifier:
//! (if true { Either::A(div("div").class("w-full")) } else { Either::B(span("span")) })
//!     .class("text-center")
//! # ))
//! # }
//! ```
//!
//! They should also aim to produce as little DOM traffic (i.e. js calls to modify the DOM-tree) as possible to be efficient.
mod attribute;
pub use attribute::*;

mod form_value;
pub use form_value::{FormValue, FormValueState};

mod class;
pub use class::*;

mod style;
pub use style::*;

mod overwrite;
pub use overwrite::*;

mod elements;
pub use elements::*;

use crate::{AnyPod, DomNode, Pod, PodFlags, PodMut};

/// A named DOM property that is either set to a value or removed.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum NamedModifier<V> {
    /// Set the property.
    Set(std::borrow::Cow<'static, str>, V),
    /// Remove the property.
    Remove(std::borrow::Cow<'static, str>),
}

impl<V> NamedModifier<V> {
    /// The property name.
    #[must_use = "Use the name to identify the property."]
    pub const fn name(&self) -> &std::borrow::Cow<'static, str> {
        let (Self::Set(name, _) | Self::Remove(name)) = self;
        name
    }

    /// Consume the modifier and return its property name.
    #[must_use = "Use the name to identify the property."]
    pub fn into_name(self) -> std::borrow::Cow<'static, str> {
        let (Self::Set(name, _) | Self::Remove(name)) = self;
        name
    }
}

/// This struct is a container, with the current Element state (e.g. whether it was created/hydrated or generally needs an update), and the modifier itself.
pub struct Modifier<'a, M> {
    pub modifier: &'a mut M,
    pub flags: &'a mut PodFlags,
}

impl<'a, M> Modifier<'a, M> {
    pub const fn new(modifier: &'a mut M, flags: &'a mut PodFlags) -> Self {
        Self { modifier, flags }
    }
}

/// This trait is intended to give access to modifiers of a [`ViewElement`](crate::core::ViewElement).
pub trait WithModifier<M> {
    fn modifier(&mut self) -> Modifier<'_, M>;
}

impl<T, N: DomNode<Props: AsMut<T>>> WithModifier<T> for Pod<N> {
    fn modifier(&mut self) -> Modifier<'_, T> {
        Modifier::new(
            <N::Props as AsMut<T>>::as_mut(&mut self.props),
            &mut self.flags,
        )
    }
}

impl<T, N: DomNode<Props: AsMut<T>>> WithModifier<T> for PodMut<'_, N> {
    fn modifier(&mut self) -> Modifier<'_, T> {
        Modifier::new(<N::Props as AsMut<T>>::as_mut(self.props), self.flags)
    }
}

pub type Children = Vec<AnyPod>;
