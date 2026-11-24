// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use crate::core::Selector;

/// Internal type used by Masonry to cache accesses to a widget's properties.
#[derive(Clone, Default, Debug)]
pub struct PropertyCache {
    /// Maps property type IDs to the index of the matching entry in the property stack, if any.
    pub(crate) entries: HashMap<TypeId, Option<usize>>,
    /// User-defined class strings that influenced at least one cached resolution.
    pub(crate) relevant_classes: HashSet<String>,
    /// Which pseudo-class flags influenced at least one cached resolution.
    pub(crate) relevant_is_hovered: bool,
    pub(crate) relevant_is_active: bool,
    pub(crate) relevant_is_disabled: bool,
    pub(crate) relevant_has_focus_target: bool,
    /// Whether the widget's property stack has changed.
    pub(crate) invalidated: bool,
}

/// The result of looking for a cached layer resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CachedResolution {
    Uncached,
    Missing,
    Layer(usize),
}

impl PropertyCache {
    /// Called by `PropertyStack::resolve_cached_mut` when a cache entry is written.
    pub(crate) fn extend_relevant(&mut self, selector: &Selector) {
        self.relevant_classes
            .extend(selector.classes.iter().cloned());
        self.relevant_is_hovered |= selector.is_hovered.is_some();
        self.relevant_is_active |= selector.is_active.is_some();
        self.relevant_is_disabled |= selector.is_disabled.is_some();
        self.relevant_has_focus_target |= selector.has_focus_target.is_some();
    }

    pub(crate) fn cached_resolution(&self, prop_type: TypeId) -> CachedResolution {
        if self.invalidated {
            return CachedResolution::Uncached;
        }
        match self.entries.get(&prop_type) {
            None => CachedResolution::Uncached,
            Some(None) => CachedResolution::Missing,
            Some(Some(index)) => CachedResolution::Layer(*index),
        }
    }

    /// Forget selectors from the old stack, retaining the cached property IDs
    /// until the update pass can notify the widgets that read them.
    pub(crate) fn invalidate(&mut self) {
        self.invalidated = true;
        self.relevant_classes.clear();
        self.relevant_is_hovered = false;
        self.relevant_is_active = false;
        self.relevant_is_disabled = false;
        self.relevant_has_focus_target = false;
    }
}
