// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashSet;

use crate::core::{ClassSet, PseudoClass, PseudoClassFilter};

/// A predicate for matching widgets based on their classes and pseudo-classes.
///
/// This type is mostly used for property resolution.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[must_use = "Use the selector in a property stack."]
pub struct Selector {
    pub(crate) classes: HashSet<String>,
    pub(crate) pseudo: PseudoClassFilter,
}

impl Selector {
    /// Creates an empty `Selector` that matches all widgets.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a `Selector` that matches widgets with all of the specified classes.
    pub fn classes(classes: &[&str]) -> Self {
        let mut selector = Self::new();
        for class in classes {
            selector.classes.insert(class.to_string());
        }
        selector
    }

    /// Builder method for a selector that matches widgets with a specific "hovered" status.
    pub const fn with_hovered(mut self, value: bool) -> Self {
        self.pseudo.set(PseudoClass::Hovered, value);
        self
    }

    /// Builder method for a selector that matches widgets with a specific "active" status.
    pub const fn with_active(mut self, value: bool) -> Self {
        self.pseudo.set(PseudoClass::Active, value);
        self
    }

    /// Builder method for a selector that matches widgets with a specific "disabled" status.
    pub const fn with_disabled(mut self, value: bool) -> Self {
        self.pseudo.set(PseudoClass::Disabled, value);
        self
    }

    /// Builder method for a selector that matches widgets with a specific "focused" status.
    pub const fn with_focused(mut self, value: bool) -> Self {
        self.pseudo.set(PseudoClass::Focused, value);
        self
    }

    /// Checks whether this selector matches a given `ClassSet`.
    pub(crate) fn matches(&self, class_set: &ClassSet) -> bool {
        self.classes.is_subset(&class_set.classes) && self.pseudo.matches(class_set.pseudo)
    }
}
