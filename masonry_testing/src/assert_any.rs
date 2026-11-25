// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::fmt::Debug;

/// Assert that the predicate returns true for at least one item.
///
/// # Panics
///
/// Panics if no item matches, showing the examined values.
#[track_caller]
pub fn assert_any<I: IntoIterator>(iter: I, mut pred: impl FnMut(I::Item) -> bool)
where
    I::Item: Debug,
{
    let mut examined = Vec::new();
    let matched = iter.into_iter().any(|item| {
        examined.push(format!("{item:?}"));
        pred(item)
    });
    assert!(matched, "no item matched predicate in {examined:#?}");
}

/// Assert that the predicate returns true for every item.
///
/// # Panics
///
/// Panics if any item fails the predicate, showing up to six failures.
#[track_caller]
pub fn assert_all<I: IntoIterator>(iter: I, pred: impl FnMut(I::Item) -> bool)
where
    I::Item: Debug,
{
    assert_matches(iter, pred, true);
}

/// Assert that the predicate returns false for every item.
///
/// # Panics
///
/// Panics if any item matches the predicate, showing up to six failures.
#[track_caller]
pub fn assert_none<I: IntoIterator>(iter: I, pred: impl FnMut(I::Item) -> bool)
where
    I::Item: Debug,
{
    assert_matches(iter, pred, false);
}

#[track_caller]
fn assert_matches<I: IntoIterator>(iter: I, mut pred: impl FnMut(I::Item) -> bool, expected: bool)
where
    I::Item: Debug,
{
    let failures: Vec<_> = iter
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let value = format!("{item:?}");
            (pred(item) != expected).then_some((index, value))
        })
        .take(6)
        .collect();
    assert!(
        failures.is_empty(),
        "predicate should return {expected}; mismatches (up to six): {failures:#?}"
    );
}
