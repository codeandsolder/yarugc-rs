// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;
use std::fmt::Display;
use std::num::NonZeroU64;
use std::sync::atomic::AtomicU64;

use crate::core::{CachedResolution, ClassSet, Property, PropertyCache, PropertySet, Selector};

/// A unique identifier for a single [`PropertyStack`].
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct PropertyStackId(pub(crate) NonZeroU64);

/// A cascading set of properties that can be applied to widgets.
///
/// Each layer of the stack consists of a [`Selector`] and a set of properties.
/// When resolving a property, the stack is traversed from top to bottom until
/// a matching selector with the requested property is found.
#[derive(Debug, Default, Clone)]
#[must_use = "Use the constructed property stack."]
pub struct PropertyStack {
    pub(crate) stack: Vec<(Selector, PropertySet)>,
}

// ---

impl PropertyStackId {
    /// Allocates a new, unique `PropertyStackId`.
    #[must_use = "Use the allocated property stack identifier."]
    pub fn next() -> Self {
        static PROPERTY_STACK_ID_COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(crate::util::allocate_id(&PROPERTY_STACK_ID_COUNTER))
    }

    /// Returns the integer value of the `PropertyStackId`.
    #[must_use = "Use the raw stack identifier."]
    pub const fn to_raw(self) -> u64 {
        self.0.get()
    }
}

impl From<PropertyStackId> for u64 {
    fn from(id: PropertyStackId) -> Self {
        id.0.into()
    }
}

impl Display for PropertyStackId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

// ---

impl PropertyStack {
    /// Creates an empty `PropertyStack`.
    pub const fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// Pushes a new entry onto the stack.
    ///
    /// The selector is used to determine whether the entry applies to a given widget based on its class set.
    pub fn push_layer(&mut self, selector: Selector, properties: impl Into<PropertySet>) {
        self.stack.push((selector, properties.into()));
    }

    /// Get the mutable reference a property set for the given index.
    ///
    /// Return [`None`] if the index is out of bounds.
    pub fn get_layer_mut(&mut self, index: usize) -> Option<&mut PropertySet> {
        Some(&mut self.stack.get_mut(index)?.1)
    }

    /// Removes a layer, returning it if the index exists.
    pub fn remove_layer(&mut self, index: usize) -> Option<(Selector, PropertySet)> {
        if index < self.stack.len() {
            Some(self.stack.remove(index))
        } else {
            None
        }
    }

    /// Inserts a layer at the requested position, or returns the uninserted layer.
    ///
    /// # Errors
    /// Returns the selector and properties if the index exceeds the layer count.
    pub fn insert_layer(
        &mut self,
        index: usize,
        selector: Selector,
        properties: impl Into<PropertySet>,
    ) -> Result<(), (Selector, PropertySet)> {
        let layer = (selector, properties.into());
        if index > self.stack.len() {
            return Err(layer);
        }
        self.stack.insert(index, layer);
        Ok(())
    }

    /// Get the property stack layers.
    #[must_use = "Inspect the ordered property layers."]
    pub fn get_layers(&self) -> &[(Selector, PropertySet)] {
        &self.stack
    }

    fn get_prop<P: Property>(&self, index: usize) -> Option<&P> {
        self.stack.get(index)?.1.get::<P>()
    }

    // A single reverse traversal implements the cascade for typed reads and
    // the type-erased update pass. Only layers defining the property can affect
    // its resolution, including layers whose selectors currently don't match.
    fn find_matching<'a, T>(
        &'a self,
        classes: &ClassSet,
        mut lookup: impl FnMut(&'a PropertySet) -> Option<T>,
        mut observe: impl FnMut(&Selector),
    ) -> Option<(usize, T)> {
        self.stack
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, (selector, properties))| {
                let value = lookup(properties)?;
                observe(selector);
                selector.matches(classes).then_some((index, value))
            })
    }

    pub(crate) fn resolve_index(
        &self,
        classes: &ClassSet,
        prop_type: TypeId,
        cache: &mut PropertyCache,
    ) -> Option<usize> {
        let index = self
            .find_matching(
                classes,
                |props| props.map.as_raw().contains_key(&prop_type).then_some(()),
                |selector| cache.extend_relevant(selector),
            )
            .map(|(index, ())| index);
        cache.entries.insert(prop_type, index);
        index
    }

    fn cached_or_resolve<'a, P: Property>(
        &'a self,
        cached: CachedResolution,
        resolve: impl FnOnce() -> Option<&'a P>,
    ) -> Option<&'a P> {
        match cached {
            CachedResolution::Missing => None,
            CachedResolution::Layer(index) => self.get_prop::<P>(index).or_else(resolve),
            CachedResolution::Uncached => resolve(),
        }
    }

    pub(crate) fn resolve<P: Property>(
        &self,
        cache: &mut PropertyCache,
        classes: &ClassSet,
    ) -> Option<&P> {
        let prop_type = TypeId::of::<P>();
        self.cached_or_resolve(cache.cached_resolution(prop_type), || {
            let found = self.find_matching(classes, PropertySet::get::<P>, |selector| {
                cache.extend_relevant(selector);
            });
            cache
                .entries
                .insert(prop_type, found.as_ref().map(|(index, _)| *index));
            found.map(|(_, value)| value)
        })
    }

    pub(crate) fn resolve_without_saving<P: Property>(
        &self,
        cache: &PropertyCache,
        classes: &ClassSet,
    ) -> Option<&P> {
        self.cached_or_resolve(cache.cached_resolution(TypeId::of::<P>()), || {
            self.find_matching(classes, PropertySet::get::<P>, |_| {})
                .map(|(_, value)| value)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{PropertiesMut, PropertiesRef};
    use crate::util::AnyMap;

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct Value(u32);
    impl Property for Value {
        fn static_default() -> &'static Self {
            &Self(0)
        }
    }

    #[derive(Clone, Debug, Default)]
    struct Other;
    impl Property for Other {
        fn static_default() -> &'static Self {
            &Self
        }
    }

    #[test]
    fn all_access_paths_resolve_the_same_cascade() {
        let mut stack = PropertyStack::new();
        stack.push_layer(Selector::new(), Value(2));
        stack.push_layer(Selector::classes(&["selected"]), Value(3));
        stack.push_layer(Selector::classes(&["unrelated"]).with_hovered(true), Other);
        let mut defaults = AnyMap::new();
        defaults.insert(Value(1));
        let mut local = PropertySet::one(Value(4));
        let mut classes = ClassSet::default();
        classes.add_class("selected");
        let mut cache = PropertyCache::default();

        let mut props = PropertiesMut {
            local: &mut local,
            default_map: &defaults,
            stack: &stack,
            class_set: &classes,
        };
        assert_eq!(props.get::<Value>(&mut cache), &Value(4));
        assert!(cache.entries.is_empty());
        assert_eq!(props.remove::<Value>(), Some(Value(4)));
        assert_eq!(props.get::<Value>(&mut cache), &Value(3));
        assert_eq!(
            cache.cached_resolution(TypeId::of::<Value>()),
            CachedResolution::Layer(1)
        );
        assert!(cache.relevant_classes.contains("selected"));
        assert!(!cache.relevant_classes.contains("unrelated"));
        assert!(
            !cache
                .relevant_pseudo
                .contains(crate::core::PseudoClass::Hovered)
        );

        let readonly = props.as_ref();
        assert_eq!(readonly.get_without_saving::<Value>(&cache), &Value(3));
        assert_eq!(
            stack.resolve_index(&classes, TypeId::of::<Value>(), &mut cache),
            Some(1)
        );

        let mut cache = PropertyCache::default();
        classes.classes.clear();
        let props = PropertiesRef {
            local: &local,
            default_map: &defaults,
            stack: &stack,
            class_set: &classes,
        };
        assert_eq!(props.get_without_saving::<Value>(&cache), &Value(2));
        assert!(cache.entries.is_empty());
        assert_eq!(props.get::<Value>(&mut cache), &Value(2));
        assert!(cache.relevant_classes.contains("selected"));

        let empty = PropertyStack::new();
        cache.invalidate();
        let props = PropertiesRef {
            stack: &empty,
            ..props
        };
        assert_eq!(props.get::<Value>(&mut cache), &Value(1));
        assert!(cache.relevant_classes.is_empty());
        let empty_defaults = AnyMap::new();
        let props = PropertiesRef {
            default_map: &empty_defaults,
            ..props
        };
        assert_eq!(props.get::<Value>(&mut cache), &Value(0));
    }

    #[test]
    fn invalid_layer_positions_preserve_stack_and_return_input() {
        let mut stack = PropertyStack::new();
        let invalid = stack.insert_layer(1, Selector::new(), Value(7));
        let (selector, properties) = invalid.unwrap_err();
        assert_eq!(properties.get::<Value>(), Some(&Value(7)));
        assert!(stack.get_layers().is_empty());

        assert!(stack.insert_layer(0, selector, properties).is_ok());
        assert!(stack.remove_layer(1).is_none());
        assert!(stack.get_layer_mut(1).is_none());
        assert_eq!(stack.get_layers().len(), 1);
        let (_, properties) = stack.remove_layer(0).unwrap();
        assert_eq!(properties.get::<Value>(), Some(&Value(7)));
        assert!(stack.get_layers().is_empty());
    }

    #[test]
    fn invalidation_rechecks_previously_missing_and_removed_layers() {
        let mut stack = PropertyStack::new();
        let classes = ClassSet::default();
        let mut cache = PropertyCache::default();
        assert_eq!(stack.resolve::<Value>(&mut cache, &classes), None);
        assert_eq!(
            cache.cached_resolution(TypeId::of::<Value>()),
            CachedResolution::Missing
        );
        stack.push_layer(Selector::new(), Value(9));
        cache.invalidate();
        assert_eq!(
            stack.resolve_without_saving::<Value>(&cache, &classes),
            Some(&Value(9))
        );
        assert_eq!(
            stack.resolve::<Value>(&mut cache, &classes),
            Some(&Value(9))
        );
        cache.invalidated = false;
        assert_eq!(
            cache.cached_resolution(TypeId::of::<Value>()),
            CachedResolution::Layer(0)
        );

        stack.remove_layer(0);
        cache.invalidate();
        assert_eq!(stack.resolve::<Value>(&mut cache, &classes), None);
        cache.invalidated = false;
        assert_eq!(
            cache.cached_resolution(TypeId::of::<Value>()),
            CachedResolution::Missing
        );
    }
}
