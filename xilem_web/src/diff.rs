// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Basic utility functions for diffing when rebuilding the views with [`View::rebuild`](`crate::core::View::rebuild`)

use std::iter::Peekable;

/// Diffs between two iterators with `Diff` as its [`Iterator::Item`]
pub fn diff_iters<T, I: Iterator<Item = T>>(old: I, new: I) -> DiffIterator<T, I> {
    let next = new.peekable();
    let prev = old.peekable();
    DiffIterator { prev, next }
}

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
/// The [`Iterator::Item`] used in `DiffIterator`.
///
/// `Remove` and `Skip` contain the count of elements that were deleted/skipped
pub enum Diff<T> {
    Add(T),
    Remove(usize),
    Change(T),
    Skip(usize),
}

/// An [`Iterator`] that diffs between two iterators with `Diff` as its [`Iterator::Item`]
pub struct DiffIterator<T, I: Iterator<Item = T>> {
    prev: Peekable<I>,
    next: Peekable<I>,
}

impl<T: PartialEq, I: Iterator<Item = T>> Iterator for DiffIterator<T, I> {
    type Item = Diff<T>;
    fn next(&mut self) -> Option<Self::Item> {
        let mut skip_count = 0;
        while let (Some(new), Some(old)) = (self.next.peek(), self.prev.peek()) {
            if new == old {
                skip_count += 1;
                self.next.next();
                self.prev.next();
                continue;
            }
            if skip_count > 0 {
                return Some(Diff::Skip(skip_count));
            }
            self.prev.next();
            return self.next.next().map(Diff::Change);
        }
        if skip_count > 0 {
            return Some(Diff::Skip(skip_count));
        }
        let mut remove_count = 0;
        while self.prev.next().is_some() {
            remove_count += 1;
        }
        if remove_count > 0 {
            return Some(Diff::Remove(remove_count));
        }
        if let Some(new) = self.next.next() {
            return Some(Diff::Add(new));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{Diff, diff_iters};

    #[test]
    fn preserves_equal_prefix_before_end_and_suffix() {
        assert_eq!(
            diff_iters(vec![1, 2, 3].into_iter(), vec![1, 2, 3].into_iter()).collect::<Vec<_>>(),
            [Diff::Skip(3)]
        );
        assert_eq!(
            diff_iters(vec![1, 2].into_iter(), vec![1, 2, 3].into_iter()).collect::<Vec<_>>(),
            [Diff::Skip(2), Diff::Add(3)]
        );
        assert_eq!(
            diff_iters(vec![1, 2, 3].into_iter(), vec![1, 2].into_iter()).collect::<Vec<_>>(),
            [Diff::Skip(2), Diff::Remove(1)]
        );
        assert_eq!(
            diff_iters(vec![1, 2, 3].into_iter(), vec![1, 4, 3].into_iter()).collect::<Vec<_>>(),
            [Diff::Skip(1), Diff::Change(4), Diff::Skip(1)]
        );
    }
}
