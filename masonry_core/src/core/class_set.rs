// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashSet;

/// Independent widget pseudo-classes, represented as bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum PseudoClass {
    Hovered = 1,
    Active = 2,
    Disabled = 4,
    Focused = 8,
}

/// A mask of pseudo-classes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PseudoClasses(u8);

impl PseudoClasses {
    #[cfg(test)]
    pub(crate) const fn contains(self, class: PseudoClass) -> bool {
        self.0 & class as u8 != 0
    }

    pub(crate) const fn set(&mut self, class: PseudoClass, value: bool) {
        if value {
            self.0 |= class as u8;
        } else {
            self.0 &= !(class as u8);
        }
    }

    pub(crate) const fn union(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub(crate) const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

/// A partial assignment of pseudo-class values, used for both predicates and updates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PseudoClassFilter {
    pub(crate) mask: PseudoClasses,
    values: PseudoClasses,
}

impl PseudoClassFilter {
    pub(crate) const fn set(&mut self, class: PseudoClass, value: bool) {
        self.mask.set(class, true);
        self.values.set(class, value);
    }

    pub(crate) const fn matches(self, actual: PseudoClasses) -> bool {
        actual.0 & self.mask.0 == self.values.0
    }

    pub(crate) const fn apply(self, actual: &mut PseudoClasses) {
        actual.0 = (actual.0 & !self.mask.0) | self.values.0;
    }
}

/// A set of classes and pseudo-classes that can be used for styling widgets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ClassSet {
    pub(crate) classes: HashSet<String>,
    pub(crate) pseudo: PseudoClasses,
}

/// A series of changes applied during the next property update pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ClassSetDiff {
    pub(crate) added: HashSet<String>,
    pub(crate) removed: HashSet<String>,
    pub(crate) pseudo: PseudoClassFilter,
}

impl ClassSet {
    pub(crate) fn add_class(&mut self, class: &str) {
        self.classes.insert(class.to_string());
    }

    pub(crate) fn apply(&mut self, diff: &ClassSetDiff) {
        for class in &diff.added {
            self.add_class(class);
        }
        for class in &diff.removed {
            self.classes.remove(class);
        }
        diff.pseudo.apply(&mut self.pseudo);
    }
}

impl ClassSetDiff {
    pub(crate) fn add(&mut self, class: &str) {
        self.removed.remove(class);
        self.added.insert(class.to_string());
    }

    pub(crate) fn remove(&mut self, class: &str) {
        self.added.remove(class);
        self.removed.insert(class.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_pseudo_class_assignments_preserve_unrelated_bits() {
        let classes = [
            PseudoClass::Hovered,
            PseudoClass::Active,
            PseudoClass::Disabled,
            PseudoClass::Focused,
        ];
        for actual_bits in 0_u8..16 {
            for mask in 0_u8..16 {
                for value_bits in 0_u8..16 {
                    let actual = PseudoClasses(actual_bits);
                    let mut filter = PseudoClassFilter::default();
                    for class in classes {
                        if mask & class as u8 != 0 {
                            filter.set(class, value_bits & class as u8 != 0);
                        }
                    }
                    assert_eq!(
                        filter.matches(actual),
                        actual_bits & mask == value_bits & mask
                    );
                    let mut updated = actual;
                    filter.apply(&mut updated);
                    assert_eq!(updated.0, (actual_bits & !mask) | (value_bits & mask));
                    assert!(filter.matches(updated));
                }
            }
        }
    }
}
