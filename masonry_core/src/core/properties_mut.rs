// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::core::{ClassSet, PropertiesRef, Property, PropertyCache, PropertySet, PropertyStack};
use crate::util::AnyMap;

/// Mutable reference to a collection of [properties](Property) that a widget has access to.
///
/// Used by the [`Widget`](crate::core::Widget) trait during most passes.
#[derive(Debug)]
pub struct PropertiesMut<'a> {
    pub(crate) local: &'a mut PropertySet,
    pub(crate) default_map: &'a AnyMap,
    pub(crate) stack: &'a PropertyStack,
    pub(crate) class_set: &'a ClassSet,
}

// TODO - Better document local vs default properties.

impl PropertiesMut<'_> {
    /// Returns true if the widget has a local property of type P.
    #[must_use = "Reports whether the property is explicitly set on the widget."]
    pub fn contains<P: Property>(&self) -> bool {
        self.as_ref().contains::<P>()
    }

    /// Returns value of property P with the standard property precedence.
    pub fn get<P: Property>(&self, cache: &mut PropertyCache) -> &P {
        self.as_ref().get::<P>(cache)
    }

    /// Borrows this collection without granting write access.
    #[must_use = "Use the borrowed property collection."]
    pub const fn as_ref(&self) -> PropertiesRef<'_> {
        PropertiesRef {
            local: self.local,
            default_map: self.default_map,
            stack: self.stack,
            class_set: self.class_set,
        }
    }

    /// Sets local property `P` to given value. Returns the previous value if `P` was already set.
    ///
    /// Does not affect default properties.
    ///
    /// If you're using a `WidgetMut`, call [`WidgetMut::insert_prop`] instead.
    ///
    /// [`WidgetMut::insert_prop`]: crate::core::WidgetMut::insert_prop
    pub fn insert<P: Property>(&mut self, value: P) -> Option<P> {
        self.local.insert(value)
    }

    /// Removes local property `P`. Returns the previous value if `P` was set.
    ///
    /// Does not affect default properties.
    ///
    /// If you're using a `WidgetMut`, call [`WidgetMut::remove_prop`] instead.
    ///
    /// [`WidgetMut::remove_prop`]: crate::core::WidgetMut::remove_prop
    pub fn remove<P: Property>(&mut self) -> Option<P> {
        self.local.remove::<P>()
    }

    /// Returns a mutable reference to the local properties for direct access.
    pub const fn local_properties(&mut self) -> &mut PropertySet {
        self.local
    }
}
