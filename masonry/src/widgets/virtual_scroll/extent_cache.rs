// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use understory_virtual_list::{SparsePrefixSumExtentModel, VirtualList};

use crate::kurbo::{Axis, Size};

use super::DEFAULT_MEAN_ITEM_LENGTH;

// SparsePrefixSumExtentModel counts known entries in each prefix query.
// Bound the history independently of the logical list length, but never
// discard a currently measured item.
const HISTORY_CAPACITY: usize = 1024;

#[derive(Debug, Default)]
pub(super) struct ExtentCache {
    geometry: Option<(Axis, Size)>,
    measured: BTreeMap<usize, f64>,
    truncated: bool,
}

impl ExtentCache {
    // Old indices must not reappear if a list shrinks and grows before layout.
    pub(super) fn truncate(&mut self, len: usize) {
        let previous_len = self.measured.len();
        self.measured.retain(|&index, _| index < len);
        self.truncated |= previous_len != self.measured.len();
    }

    // Historical extents are estimates, not promises about unmaterialized
    // content. The next layout will remeasure the registered children.
    pub(super) fn invalidate(&mut self) {
        self.geometry = None;
    }

    pub(super) fn sync(
        &mut self,
        list: &mut VirtualList<SparsePrefixSumExtentModel<f64>>,
        geometry: (Axis, Size),
        anchor: usize,
        live: impl Iterator<Item = (usize, f64)> + Clone,
    ) {
        let geometry_changed = self.geometry != Some(geometry);
        self.geometry = Some(geometry);
        if geometry_changed {
            self.measured.clear();
        }

        let mut changes = Vec::new();
        let mut live_count = 0;
        for (index, extent) in live.clone() {
            live_count += 1;
            if self
                .measured
                .get(&index)
                .is_none_or(|old| old.to_bits() != extent.to_bits())
            {
                self.measured.insert(index, extent);
                changes.push((index, extent));
            }
        }

        let excess = self
            .measured
            .len()
            .saturating_sub(HISTORY_CAPACITY.max(live_count));
        if excess != 0 {
            let live: BTreeSet<_> = live.map(|(index, _)| index).collect();
            let mut dormant: Vec<_> = self
                .measured
                .keys()
                .copied()
                .filter(|index| !live.contains(index))
                .collect();
            // Prefer measurements near the current viewport, not necessarily
            // those with the lowest indices or most recent insertion time.
            dormant.sort_unstable_by_key(|&index| (Reverse(index.abs_diff(anchor)), index));
            for index in dormant.into_iter().take(excess) {
                self.measured.remove(&index);
            }
        }

        if !geometry_changed && excess == 0 && changes.is_empty() && !self.truncated {
            return;
        }
        self.truncated = false;

        // Compute the mean without overflowing a sum of finite extents.
        let mut mean = 0.;
        for (count, &extent) in self.measured.values().enumerate() {
            mean += (extent - mean) / crate::layout_count(count + 1);
        }
        let old_default = list.model().default_extent();
        let default_extent = if mean > 0. {
            mean
        } else if geometry_changed || self.measured.is_empty() {
            // Zero-length rows are valid, but unknown rows must not all be
            // estimated at zero (which can materialize the entire list).
            DEFAULT_MEAN_ITEM_LENGTH
        } else {
            old_default
        };
        let default_changed = default_extent.to_bits() != old_default.to_bits();

        // The model stores overrides, not observations. In a uniform list
        // every measured row equals the default, so prefix queries need no
        // sparse entries at all. Changing the default may change which
        // observations need an override, so rebuild in that case.
        let rebuild = geometry_changed
            || excess != 0
            || default_changed
            || changes
                .iter()
                .any(|(_, extent)| extent.to_bits() == default_extent.to_bits());
        if !rebuild && changes.is_empty() {
            return;
        }
        let model = list.model_mut();
        if rebuild {
            model.rebuild(
                self.measured
                    .iter()
                    .filter(|(_, extent)| extent.to_bits() != default_extent.to_bits())
                    .map(|(&index, &extent)| (index, extent)),
                &|extent| *extent,
            );
        } else {
            for (index, extent) in changes {
                model.set_extent(index, extent);
            }
        }
        if default_changed {
            model.set_default_extent(default_extent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_overrides_match_observations_after_default_changes() {
        let mut list = VirtualList::new(SparsePrefixSumExtentModel::new(180., 10), 100., 0.);
        let mut cache = ExtentCache::default();
        let geometry = (Axis::Vertical, Size::new(100., 100.));

        let cases: &[&[(usize, f64)]] = &[
            &[(0, 20.), (1, 20.), (2, 20.)],
            &[(3, 40.), (6, 0.)],
            &[(1, 40.), (3, 20.)],
            &[(0, 0.), (1, 0.), (2, 0.), (3, 0.), (6, 0.)],
            &[(0, 30.), (1, 30.), (2, 30.), (3, 30.), (6, 30.)],
            &[(2, 50.), (3, 10.)],
            &[(2, 30.), (3, 30.)],
        ];
        for observations in cases {
            cache.sync(&mut list, geometry, 0, observations.iter().copied());
            let mut expected_offset = 0.;
            for index in 0..list.len() {
                let expected_extent = cache
                    .measured
                    .get(&index)
                    .copied()
                    .unwrap_or(list.model().default_extent());
                assert!((list.extent_of(index) - expected_extent).abs() < 1e-10);
                assert!((list.offset_of(index) - expected_offset).abs() < 1e-8);
                expected_offset += expected_extent;
            }
            assert!((list.total_extent() - expected_offset).abs() < 1e-8);
        }
    }

    #[test]
    fn history_is_bounded_and_invalidated_with_geometry_or_length() {
        let mut list = VirtualList::new(SparsePrefixSumExtentModel::new(180., 10_000), 100., 0.);
        let mut cache = ExtentCache::default();
        let geometry = (Axis::Vertical, Size::new(100., 100.));
        cache.sync(&mut list, geometry, 0, [(0, 10.), (1, 30.)].into_iter());
        cache.sync(&mut list, geometry, 2, [(2, 60.)].into_iter());
        assert_eq!(list.extent_of(0), 10.);
        assert_eq!(list.extent_of(1), 30.);
        assert_eq!(list.extent_of(2), 60.);
        assert!((list.model().default_extent() - 100. / 3.).abs() < 1e-10);

        let far = 2000;
        cache.sync(
            &mut list,
            geometry,
            far,
            (far..far + HISTORY_CAPACITY).map(|index| (index, 80.)),
        );
        assert_eq!(cache.measured.len(), HISTORY_CAPACITY);
        assert!(!cache.measured.contains_key(&0));
        assert!(cache.measured.contains_key(&far));

        list.set_len(far + 1);
        cache.truncate(far + 1);
        list.set_len(10_000);
        cache.sync(&mut list, geometry, far, [(far, 20.)].into_iter());
        assert_eq!(cache.measured.len(), 1);
        assert_eq!(list.extent_of(far), 20.);
        assert_eq!(list.model().default_extent(), 20.);

        cache.sync(
            &mut list,
            (Axis::Horizontal, geometry.1),
            0,
            [(0, 90.)].into_iter(),
        );
        assert_eq!(cache.measured.len(), 1);
        assert_eq!(list.extent_of(far), 90.);
        assert_eq!(list.extent_of(0), 90.);
    }
}
