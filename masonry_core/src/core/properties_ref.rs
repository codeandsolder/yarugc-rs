// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::core::{ClassSet, Property, PropertyCache, PropertySet, PropertyStack};
use crate::util::AnyMap;

/// Reference to a collection of [properties](Property) that a widget has access to.
///
/// Used by the [`Widget`](crate::core::Widget) trait during rendering passes and in some search methods.
#[derive(Clone, Copy, Debug)]
pub struct PropertiesRef<'a> {
    pub(crate) local: &'a PropertySet,
    pub(crate) default_map: &'a AnyMap,
    pub(crate) stack: &'a PropertyStack,
    pub(crate) class_set: &'a ClassSet,
}

// TODO - Better document local vs default properties.

impl<'a> PropertiesRef<'a> {
    /// Returns true if the widget has a local property of type P.
    #[must_use = "Reports whether the property is explicitly set on the widget."]
    pub fn contains<P: Property>(&self) -> bool {
        self.local.get::<P>().is_some()
    }

    /// Returns the value of property P, caching the applicable stack layer.
    ///
    /// Precedence is local property, matching stack layer, widget default,
    /// then the property's static default.
    pub fn get<P: Property>(&self, cache: &mut PropertyCache) -> &'a P {
        self.resolve(|| self.stack.resolve::<P>(cache, self.class_set))
    }

    /// Resolves a property without changing the cache.
    pub(crate) fn get_without_saving<P: Property>(&self, cache: &PropertyCache) -> &'a P {
        self.resolve(|| {
            self.stack
                .resolve_without_saving::<P>(cache, self.class_set)
        })
    }

    fn resolve<P: Property>(&self, stack_lookup: impl FnOnce() -> Option<&'a P>) -> &'a P {
        self.local
            .get::<P>()
            .or_else(stack_lookup)
            .or_else(|| self.default_map.get::<P>())
            .unwrap_or_else(|| P::static_default())
    }

    /// Returns a reference to the local properties for direct access.
    #[must_use = "Inspect the explicit properties of the widget."]
    pub const fn local_properties(&self) -> &'a PropertySet {
        self.local
    }
}
