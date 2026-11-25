// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::{any::TypeId, collections::BTreeMap, mem};

use accesskit::Role;
use include_doc_path::include_doc_path;
use tracing::{Span, trace_span};

use crate::core::{
    ChildrenIds, CollectionWidget, LayoutCtx, MeasureCtx, NewWidget, NoAction, PropertiesRef,
    RegisterCtx, UpdateCtx, UsesProperty, Widget, WidgetId, WidgetMut, WidgetPod,
};
use crate::kurbo::{Axis, Point, Size};
use crate::layout::{AsUnit, LayoutSize, LenDef, LenReq, Length};
use crate::properties::Gap;
use crate::util::debug_panic;

/// A widget that arranges its children in a grid.
///
/// Children are drawn in index order,
/// i.e. each child is drawn on top of the other children with lower indices.
///
#[doc = concat!(
    "![Grid with buttons of various sizes](",
    include_doc_path!("screenshots/grid_with_changed_spacing.png"),
    ")",
)]
#[derive(Default)]
pub struct Grid {
    children: Vec<Child>,
    columns: Vec<GridTrackSize>,
    rows: Vec<GridTrackSize>,
    needs_placing: bool,
}

#[derive(Debug)]
struct Child {
    widget: WidgetPod<dyn Widget>,
    params: GridParams,

    // Recomputed after track or child changes. Unplaced children are stashed.
    cell: Option<(usize, usize)>,
}

/// Describes how a grid track (row/column) should be sized.
#[derive(Clone, Copy, PartialEq)]
pub enum GridTrackSize {
    /// At least the minimum intrinsic size of the content and
    /// at most the maximum intrinsic size of the content.
    Auto,

    /// Minimum intrinsic size of the content.
    MinContent,

    /// Maximum intrinsic size of the content
    MaxContent,

    /// Fit content within the given size.
    FitContent(Length),

    /// Fixed at the given size.
    Fixed(Length),

    /// Fixed at the size that is equal to the size of the grid container
    /// times the given parameter.
    ///
    /// The parameter is in normalized percentage, e.g. 0.3, not 30%.
    Percentage(f64),

    /// Fraction of remaining space.
    Fraction(f64),
}

/// Parameters required when adding an item to a [`Grid`] container.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct GridParams {
    /// Defines which column in the grid the item should start.
    pub col: Option<u16>,

    /// Defines which row in the grid the item should start.
    pub row: Option<u16>,

    /// Defines how many column in the grid the item should occupy.
    pub width: u16,

    /// Defines how many row in the grid the item should occupy.
    pub height: u16,
}

// A summed-area table allows a rectangular vacancy query in constant time.
struct GridOccupancy {
    columns: usize,
    rows: usize,
    cells: Vec<bool>,
    prefix: Vec<usize>,
}

impl GridOccupancy {
    fn new(columns: usize, rows: usize) -> Option<Self> {
        let count = columns.checked_mul(rows)?;
        let prefix_count = columns.checked_add(1)?.checked_mul(rows.checked_add(1)?)?;
        let mut cells = Vec::new();
        let mut prefix = Vec::new();
        cells.try_reserve_exact(count).ok()?;
        prefix.try_reserve_exact(prefix_count).ok()?;
        cells.resize(count, false);
        prefix.resize(prefix_count, 0);
        Some(Self {
            columns,
            rows,
            cells,
            prefix,
        })
    }

    fn place(&mut self, child: &mut Child, (col, row): (usize, usize)) {
        let width = usize::from(child.params.width);
        let height = usize::from(child.params.height);
        if width == 0
            || height == 0
            || col.checked_add(width).is_none_or(|end| end > self.columns)
            || row.checked_add(height).is_none_or(|end| end > self.rows)
        {
            debug_panic!("Child {child:?} with position ({col}, {row}) is out of grid bounds");
            return;
        }
        child.cell = Some((col, row));
        for row in row..row + height {
            let start = row * self.columns + col;
            self.cells[start..start + width].fill(true);
        }
    }

    fn rebuild_prefix(&mut self) {
        let stride = self.columns + 1;
        for (row, cells) in self.cells.chunks_exact(self.columns).enumerate() {
            let mut row_count = 0;
            for (col, &occupied) in cells.iter().enumerate() {
                row_count += usize::from(occupied);
                let index = (row + 1) * stride + col + 1;
                self.prefix[index] = self.prefix[index - stride] + row_count;
            }
        }
    }

    fn find_free(&mut self, params: GridParams, cursor: (usize, usize)) -> Option<(usize, usize)> {
        let width = usize::from(params.width);
        let height = usize::from(params.height);
        if width == 0 || height == 0 {
            return None;
        }
        let last_col = self.columns.checked_sub(width)?;
        let last_row = self.rows.checked_sub(height)?;
        let cols = params.col.map(usize::from);
        let rows = params.row.map(usize::from);
        if cols.is_some_and(|col| col > last_col) || rows.is_some_and(|row| row > last_row) {
            return None;
        }
        self.rebuild_prefix();
        let stride = self.columns + 1;
        let prefix = |col, row| self.prefix[row * stride + col];
        let automatic = cols.is_none() && rows.is_none();
        let first_row = rows.unwrap_or(if automatic { cursor.1 } else { 0 });
        for row in first_row..=rows.unwrap_or(last_row) {
            let first_col = cols.unwrap_or(if automatic && row == cursor.1 {
                cursor.0
            } else {
                0
            });
            for col in first_col..=cols.unwrap_or(last_col) {
                let right = col + width;
                let bottom = row + height;
                let right_count = prefix(right, bottom) - prefix(right, row);
                let left_count = prefix(col, bottom) - prefix(col, row);
                if right_count == left_count {
                    return Some((col, row));
                }
            }
        }
        None
    }
}

#[derive(Clone, Copy)]
struct TrackSizing {
    axis: Axis,
    len_req: LenReq,
    cross_length: Option<Length>,
    gap: Length,
    context_length: Option<Length>,
    is_measuring_only: bool,
}

// --- MARK: BUILDERS
impl Grid {
    /// Creates a new grid with no columns or rows.
    #[must_use = "Use the returned value."]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new grid with the given number of columns and rows,
    /// with each row and column sized with [`GridTrackSize::FRACTION`].
    #[must_use = "Use the returned value."]
    pub fn with_dimensions(columns: usize, rows: usize) -> Self {
        Self {
            columns: vec![GridTrackSize::FRACTION; columns],
            rows: vec![GridTrackSize::FRACTION; rows],
            ..Self::default()
        }
    }

    /// Builder-style method to add a row to the grid.
    #[must_use = "Use the returned value."]
    pub fn with_row(mut self, row: GridTrackSize) -> Self {
        self.rows.push(row);
        self
    }

    /// Builder-style method to add a column to the grid.
    #[must_use = "Use the returned value."]
    pub fn with_column(mut self, column: GridTrackSize) -> Self {
        self.columns.push(column);
        self
    }

    /// Builder-style method to add multiple rows to the grid.
    #[must_use = "Use the configured value."]
    pub fn with_rows(mut self, rows: impl IntoIterator<Item = GridTrackSize>) -> Self {
        self.rows.extend(rows);
        self
    }

    /// Builder-style method to add multiple columns to the grid.
    #[must_use = "Use the configured value."]
    pub fn with_columns(mut self, columns: impl IntoIterator<Item = GridTrackSize>) -> Self {
        self.columns.extend(columns);
        self
    }

    /// Builder-style method to add a child widget.
    ///
    /// # Panics
    ///
    /// If the `params` refer to a row/column that does not exist, an algorithm similar
    /// to CSS grid's would try to assign an appropriate position to it.
    /// If the algorithm fails, `measure`/`layout` pass would panic in debug mode
    /// and silently ignore this child in release mode.
    #[must_use = "Use the returned value."]
    pub fn with(mut self, child: NewWidget<impl Widget + ?Sized>, params: GridParams) -> Self {
        let child = Child::new(child, params);
        self.children.push(child);
        self.needs_placing = true;
        self
    }
}

// --- MARK: METHODS
impl Grid {
    fn assign_cell_to_auto_placed_children(&mut self) {
        if !mem::take(&mut self.needs_placing) {
            return;
        }
        for child in &mut self.children {
            child.cell = None;
        }
        if self.columns.is_empty() || self.rows.is_empty() {
            return;
        }
        let Some(mut occupancy) = GridOccupancy::new(self.columns.len(), self.rows.len()) else {
            debug_panic!("Grid occupancy dimensions overflow or cannot be allocated");
            return;
        };

        // Fully specified items reserve their cells before any automatic placement.
        for child in &mut self.children {
            if let (Some(col), Some(row)) = (child.params.col, child.params.row) {
                occupancy.place(child, (usize::from(col), usize::from(row)));
            }
        }
        let mut cursor = (0, 0);
        for child in &mut self.children {
            if child.params.col.is_some() && child.params.row.is_some() {
                continue;
            }
            if let Some(cell) = occupancy.find_free(child.params, cursor) {
                occupancy.place(child, cell);
                let end = cell.0 + usize::from(child.params.width);
                cursor = (end % occupancy.columns, cell.1 + end / occupancy.columns);
            } else {
                debug_panic!("Child {child:?} could not be placed in the grid");
            }
        }
    }

    fn resolve_track_lengths(
        &mut self,
        sizing: TrackSizing,
        mut compute_length: impl FnMut(
            &mut WidgetPod<dyn Widget + 'static>,
            LenDef,
            LayoutSize,
            Axis,
            Option<Length>,
        ) -> Length,
    ) -> Vec<f64> {
        // This function is a somewhat simplified implementation of track sizing algorithm from
        // the CSS grid spec (https://drafts.csswg.org/css-grid-1/#algo-track-sizing).

        let TrackSizing {
            axis,
            len_req,
            cross_length,
            gap,
            context_length,
            is_measuring_only,
        } = sizing;
        let tracks = match axis {
            Axis::Horizontal => &self.columns,
            Axis::Vertical => &self.rows,
        };

        if tracks.is_empty() {
            return Vec::new();
        }

        // available length for distribution
        // = total_length - gap * (#tracks - 1)
        let context_length = context_length.and_then(|l| {
            Length::try_px(gap.get().mul_add(
                -crate::layout_count(tracks.len().saturating_sub(1)),
                l.get(),
            ))
        });

        let (track_auto_lengths, mut track_lengths) =
            initial_track_lengths(tracks, len_req, context_length);

        let mut multitrack_lengths = BTreeMap::new();

        // step 2: initialize track lengths
        for child in &mut self.children {
            let Some(pos) = child.pos(axis) else {
                continue;
            };
            if child.span(axis) == 1 {
                // if a child spans only a single track, the tracks size accommodates
                // the child if the track size s not fixed.
                let track = pos;
                let auto_length = track_auto_lengths[track];
                if !matches!(auto_length, LenDef::Fixed(_)) {
                    let computed = compute_length(
                        &mut child.widget,
                        auto_length,
                        LayoutSize::NONE,
                        axis,
                        cross_length,
                    );
                    track_lengths[track] = computed.get().max(track_lengths[track]);
                }
            } else {
                let span = child.span(axis);

                let start_track = pos;
                let end_track = start_track + span as usize;

                // if a child spans multiple tracks and any of those is a fractional track, skip this step.
                // any computation involving fractional tracks are done in a separate step at the end.
                let Some((min_length, max_length)) = tracks[start_track..end_track]
                    .iter()
                    .all(|t| !matches!(t, GridTrackSize::Fraction(_)))
                    .then(|| multitrack_lengths.entry((span, pos)).or_insert((0., 0.)))
                else {
                    continue;
                };

                let computed_min = compute_length(
                    &mut child.widget,
                    LenDef::MinContent,
                    LayoutSize::NONE,
                    axis,
                    cross_length,
                );
                let computed_max = compute_length(
                    &mut child.widget,
                    LenDef::MaxContent,
                    LayoutSize::NONE,
                    axis,
                    cross_length,
                );

                *min_length = computed_min.get().max(*min_length);
                *max_length = computed_max.get().max(*max_length);
            }
        }

        // Resolve spanning children in span order, then distribute fractional tracks.
        for ((span, pos), (min_length, max_length)) in multitrack_lengths {
            let end = pos + usize::from(span);
            let lengths = &mut track_lengths[pos..end];
            let available = gap.get().mul_add(
                f64::from(span.saturating_sub(1)),
                lengths.iter().sum::<f64>(),
            );
            distribute_intrinsic(
                &track_auto_lengths[pos..end],
                lengths,
                min_length - available,
                max_length - available,
            );
        }
        distribute_remaining(
            tracks,
            track_lengths,
            len_req,
            context_length,
            is_measuring_only,
        )
    }
}

fn initial_track_lengths(
    tracks: &[GridTrackSize],
    len_req: LenReq,
    context_length: Option<Length>,
) -> (Vec<LenDef>, Vec<f64>) {
    // step 1: auto lengths and initial lengths of tracks
    tracks
        .iter()
        .map(|t| match *t {
            // conforms to min-content measure of self
            GridTrackSize::Auto | GridTrackSize::MinContent => (LenDef::MinContent, 0.),
            GridTrackSize::MaxContent | GridTrackSize::Fraction(_) => (LenDef::MaxContent, 0.),
            GridTrackSize::FitContent(limit) => (LenDef::FitContent(limit), 0.),
            GridTrackSize::Fixed(length) => (LenDef::Fixed(length), length.get()),
            GridTrackSize::Percentage(percent) => match len_req {
                LenReq::Fit(space) => {
                    let fixed = space.get() * percent;
                    (LenDef::FitContent(fixed.px()), fixed)
                }
                _ => context_length.map_or((LenDef::MinContent, 0.), |length| {
                    let fixed = length.get() * percent;
                    (LenDef::Fixed(fixed.px()), fixed)
                }),
            },
        })
        .unzip()
}

// Distribute the minimum contribution with capped water filling. Each iteration
// either consumes the remaining contribution or permanently saturates a track.
fn distribute_intrinsic(defs: &[LenDef], lengths: &mut [f64], minimum: f64, maximum: f64) {
    let mut remaining = minimum.max(0.);
    for _ in 0..defs.len() {
        if remaining <= 0. {
            break;
        }
        let eligible = |def: &LenDef, length: f64| match def {
            LenDef::MinContent | LenDef::MaxContent => true,
            LenDef::FitContent(limit) => length < limit.get(),
            LenDef::Fixed(_) => false,
        };
        let count = defs
            .iter()
            .zip(lengths.iter())
            .filter(|(def, length)| eligible(def, **length))
            .count();
        if count == 0 {
            break;
        }
        let share = remaining / crate::layout_count(count);
        let mut saturated = false;
        for (def, length) in defs.iter().zip(lengths.iter_mut()) {
            if !eligible(def, *length) {
                continue;
            }
            let addition = match def {
                LenDef::FitContent(limit) => share.min((limit.get() - *length).max(0.)),
                _ => share,
            };
            saturated |= addition < share;
            *length += addition;
            remaining = (remaining - addition).max(0.);
        }
        if !saturated {
            break;
        }
    }
    // Only max-content tracks receive the additional preferred-size contribution.
    let count = defs
        .iter()
        .filter(|def| matches!(def, LenDef::MaxContent))
        .count();
    if count != 0 {
        let share = (maximum - minimum.max(0.) + remaining).max(0.) / crate::layout_count(count);
        for (def, length) in defs.iter().zip(lengths.iter_mut()) {
            if matches!(def, LenDef::MaxContent) {
                *length += share;
            }
        }
    }
}

fn distribute_remaining(
    tracks: &[GridTrackSize],
    track_lengths: Vec<f64>,
    len_req: LenReq,
    context_length: Option<Length>,
    is_measuring_only: bool,
) -> Vec<f64> {
    let extract_bases_with_fr = |track_lengths: Vec<f64>, fr_unit: f64| {
        tracks
            .iter()
            .zip(track_lengths)
            .map(|(track, length)| match track {
                GridTrackSize::Fraction(fr) => {
                    if fr.is_finite() && *fr > 0. {
                        *fr * fr_unit
                    } else {
                        0.
                    }
                }
                _ => length,
            })
            .collect()
    };

    if len_req == LenReq::Min {
        // if the grid is required to be min-content, fractional tracks are sized zero.
        return extract_bases_with_fr(track_lengths, 0.);
    }

    if let Some(context_length) = context_length
        && (matches!(len_req, LenReq::Fit(_)) || !is_measuring_only)
    {
        // if context length is provided and the grid is fit-content sized,
        // distribute the remaining context length after sizing non-fractional tracks
        // between the fractional tracks according to their fractions.
        // if there is no fractional track but there are auto-sized tracks, distribute the space
        // between the auto-sized tracks equally.
        let (occupied_length, total_fr) = tracks
            .iter()
            .zip(&track_lengths)
            .map(|(track, length)| match *track {
                GridTrackSize::Fraction(fr) => {
                    (0., if fr.is_finite() && fr > 0. { fr } else { 0. })
                }
                _ => (*length, 0.),
            })
            .fold((0., 0.), |(a, b), (c, d)| (a + c, b + d));

        let leftover_length = context_length.get() - occupied_length;
        if leftover_length <= 0. {
            return track_lengths;
        }

        if total_fr == 0. {
            let n_auto = tracks
                .iter()
                .filter(|t| matches!(t, GridTrackSize::Auto))
                .count();
            if n_auto != 0 {
                let per_auto_track_ext = leftover_length / crate::layout_count(n_auto);
                tracks
                    .iter()
                    .zip(track_lengths)
                    .map(|(track, length)| match track {
                        GridTrackSize::Auto => length + per_auto_track_ext,
                        _ => length,
                    })
                    .collect()
            } else {
                track_lengths
            }
        } else {
            extract_bases_with_fr(track_lengths, leftover_length / total_fr)
        }
    } else {
        // if context length is not provided or the grid is min/max-content sized,
        // take the maximum of fractional track length from single-track childrens and apply that.
        if let Some(fr_unit) = tracks
            .iter()
            .zip(&track_lengths)
            .filter_map(|(track, length)| match track {
                GridTrackSize::Fraction(fr) if fr.is_finite() && *fr > 0. => Some(length / *fr),
                _ => None,
            })
            .reduce(f64::max)
        {
            extract_bases_with_fr(track_lengths, fr_unit)
        } else {
            track_lengths
        }
    }
}

// --- MARK: IMPL CHILD
impl Child {
    fn new(child: NewWidget<impl Widget + ?Sized>, params: GridParams) -> Self {
        Self {
            widget: child.erased().into_pod(),
            params,

            cell: None,
        }
    }

    /// Returns the number of cells the child's area spans on the given `axis`.
    const fn span(&self, axis: Axis) -> u16 {
        match axis {
            Axis::Horizontal => self.params.width,
            Axis::Vertical => self.params.height,
        }
    }

    fn pos(&self, axis: Axis) -> Option<usize> {
        self.cell.map(|(col, row)| match axis {
            Axis::Horizontal => col,
            Axis::Vertical => row,
        })
    }
}

// --- MARK: IMPL GRIDTRACKSIZE
impl GridTrackSize {
    /// *One (1)* Fraction of remaining space.
    pub const FRACTION: Self = Self::Fraction(1.);
}

impl Default for GridTrackSize {
    fn default() -> Self {
        Self::FRACTION
    }
}

// --- MARK: IMPL GRIDPARAMS
impl GridParams {
    /// Creates grid parameters with the defaulted values.
    ///
    /// It sets the cell span to 1×1.
    #[must_use = "Use the returned value."]
    pub const fn new() -> Self {
        Self {
            col: None,
            row: None,
            width: 1,
            height: 1,
        }
    }

    /// Creates grid parameters for the given position.
    ///
    /// It sets the cell span to 1×1.
    #[must_use = "Use the returned value."]
    pub const fn pos(col: u16, row: u16) -> Self {
        Self::new().with_col(col).with_row(row)
    }

    /// Builder-style method to define at which column in the grid the item should start.
    #[must_use = "Use the returned value."]
    pub const fn with_col(mut self, col: u16) -> Self {
        self.col = Some(col);
        self
    }

    /// Builder-style method to define at which row in the grid the item should start.
    #[must_use = "Use the returned value."]
    pub const fn with_row(mut self, row: u16) -> Self {
        self.row = Some(row);
        self
    }

    /// Builder-style method to define at which column and row in the grid the item should start.
    #[must_use = "Use the returned value."]
    pub const fn with_pos(self, col: u16, row: u16) -> Self {
        self.with_col(col).with_row(row)
    }

    /// Builder-style method to define how many column in the grid the item should occupy.
    ///
    /// # Panics
    ///
    /// When debug assertions are on, panics if the width is zero.
    #[must_use = "Use the configured value."]
    pub fn with_width(mut self, mut width: u16) -> Self {
        if width == 0 {
            debug_panic!(
                "Grid width value should be a positive nonzero number; got {}",
                width
            );
            width = 1;
        }
        self.width = width;
        self
    }

    /// Builder-style method to define how many row in the grid the item should occupy.
    ///
    /// # Panics
    ///
    /// When debug assertions are on, panics if the height is zero.
    #[must_use = "Use the configured value."]
    pub fn with_height(mut self, mut height: u16) -> Self {
        if height == 0 {
            debug_panic!(
                "Grid height value should be a positive nonzero number; got {}",
                height
            );
            height = 1;
        }
        self.height = height;
        self
    }

    /// Builder-style method to define how many column and row in the grid the item should occupy.
    ///
    /// # Panics
    ///
    /// When debug assertions are on, panics if either the width or the height is zero.
    #[must_use = "Use the returned value."]
    pub fn with_span(self, width: u16, height: u16) -> Self {
        self.with_width(width).with_height(height)
    }
}

impl Default for GridParams {
    fn default() -> Self {
        Self::new()
    }
}

// --- MARK: WIDGETMUT
impl Grid {
    /// Sets the column sizes of the grid.
    pub fn set_columns(this: &mut WidgetMut<'_, Self>, columns: Vec<GridTrackSize>) {
        this.widget.columns = columns;
        this.widget.needs_placing = true;
        this.ctx.request_layout();
    }

    /// Sets the row sizes of the grid.
    pub fn set_rows(this: &mut WidgetMut<'_, Self>, rows: Vec<GridTrackSize>) {
        this.widget.rows = rows;
        this.widget.needs_placing = true;
        this.ctx.request_layout();
    }
}

// --- MARK: COLLECTIONWIDGET
impl CollectionWidget<GridParams> for Grid {
    collection_accessors!(child => child.widget.id());

    /// Appends a child widget to the collection.
    fn add(
        this: &mut WidgetMut<'_, Self>,
        child: NewWidget<impl Widget + ?Sized>,
        params: impl Into<GridParams>,
    ) {
        let child = Child::new(child, params.into());
        this.widget.children.push(child);
        this.widget.needs_placing = true;
        this.ctx.children_changed();
    }

    /// Inserts a child widget at the given index.
    ///
    /// # Errors
    ///
    /// Returns an error if `idx` is larger than the number of children.
    fn insert(
        this: &mut WidgetMut<'_, Self>,
        idx: usize,
        child: NewWidget<impl Widget + ?Sized>,
        params: impl Into<GridParams>,
    ) -> Result<(), crate::core::CollectionError> {
        crate::check_insert_slot(idx, this.widget.children.len())?;
        let child = Child::new(child, params.into());
        this.widget.children.insert(idx, child);
        this.widget.needs_placing = true;
        this.ctx.children_changed();
        Ok(())
    }

    /// Replaces the child widget at the given index.
    ///
    /// # Errors
    ///
    /// Returns an error if `idx` is out of bounds.
    fn set(
        this: &mut WidgetMut<'_, Self>,
        idx: usize,
        child: NewWidget<impl Widget + ?Sized>,
        params: impl Into<GridParams>,
    ) -> Result<(), crate::core::CollectionError> {
        crate::check_slot(idx, this.widget.children.len())?;
        let child = Child::new(child, params.into());
        let old_child = mem::replace(&mut this.widget.children[idx], child);
        this.widget.needs_placing = true;
        this.ctx.remove_child(old_child.widget);
        Ok(())
    }

    /// Sets the child params at the given index.
    ///
    /// # Errors
    ///
    /// Returns an error if `idx` is out of bounds.
    fn set_params(
        this: &mut WidgetMut<'_, Self>,
        idx: usize,
        params: impl Into<GridParams>,
    ) -> Result<(), crate::core::CollectionError> {
        crate::check_slot(idx, this.widget.children.len())?;
        let child = &mut this.widget.children[idx];
        child.params = params.into();
        this.widget.needs_placing = true;
        this.ctx.request_layout();
        Ok(())
    }

    /// Swaps the index of two children.
    ///
    /// Explicit positions follow their children; automatic positions are recomputed.
    ///
    /// # Errors
    ///
    /// Returns an error if `a` or `b` are out of bounds.
    fn swap(
        this: &mut WidgetMut<'_, Self>,
        a: usize,
        b: usize,
    ) -> Result<(), crate::core::CollectionError> {
        crate::check_slot(a, this.widget.children.len())?;
        crate::check_slot(b, this.widget.children.len())?;
        this.widget.children.swap(a, b);
        this.widget.needs_placing = true;
        this.ctx.children_changed();
        Ok(())
    }

    /// Removes the child at the given index.
    ///
    /// # Errors
    ///
    /// Returns an error if `idx` is out of bounds.
    fn remove(
        this: &mut WidgetMut<'_, Self>,
        idx: usize,
    ) -> Result<(), crate::core::CollectionError> {
        crate::check_slot(idx, this.widget.children.len())?;
        let child = this.widget.children.remove(idx);
        this.ctx.remove_child(child.widget);
        this.widget.needs_placing = true;
        Ok(())
    }

    /// Removes all children.
    fn clear(this: &mut WidgetMut<'_, Self>) {
        for child in this.widget.children.drain(..) {
            this.ctx.remove_child(child.widget);
        }
        this.widget.needs_placing = true;
    }
}

impl UsesProperty<Gap> for Grid {}

// --- MARK: IMPL WIDGET
impl Widget for Grid {
    type Action = NoAction;

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        for child in &mut self.children {
            ctx.register_child(&mut child.widget);
        }
    }

    fn property_changed(&mut self, ctx: &mut UpdateCtx<'_>, property_type: TypeId) {
        Gap::prop_changed(ctx, property_type);
    }

    fn measure(
        &mut self,
        ctx: &mut MeasureCtx<'_>,
        props: &PropertiesRef<'_>,
        axis: Axis,
        len_req: LenReq,
        cross_length: Option<Length>,
    ) -> Length {
        let cache = ctx.property_cache();
        let gap = props.get::<Gap>(cache);

        let gap_length = gap.gap.get();

        self.assign_cell_to_auto_placed_children();

        let track_lengths = self.resolve_track_lengths(
            TrackSizing {
                axis,
                len_req,
                cross_length,
                gap: gap.gap,
                context_length: ctx.context_size().length(axis),
                is_measuring_only: true,
            },
            |child, auto_length, context_size, axis, cross_length| {
                ctx.compute_length(child, auto_length, context_size, axis, cross_length)
            },
        );

        if track_lengths.is_empty() {
            return 0.px();
        }

        let computed = Length::px(gap_length.mul_add(
            crate::layout_count(track_lengths.len().saturating_sub(1)),
            track_lengths.into_iter().sum::<f64>(),
        ));

        match len_req {
            LenReq::Fit(space) => computed.min(space),
            _ => computed,
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, props: &PropertiesRef<'_>, size: Size) {
        let cache = ctx.property_cache();
        let gap = props.get::<Gap>(cache);

        let gap_length = gap.gap.get();

        self.assign_cell_to_auto_placed_children();

        let mut resolve_track = |grid: &mut Self, axis: Axis| {
            grid.resolve_track_lengths(
                TrackSizing {
                    axis,
                    len_req: LenReq::Fit(size.get_coord(axis).px()),
                    cross_length: Some(size.get_coord(axis.cross()).px()),
                    gap: gap.gap,
                    context_length: Some(size.get_coord(axis).px()),
                    is_measuring_only: false,
                },
                |child, auto_length, context_size, axis, cross_length| {
                    ctx.compute_length(child, auto_length, context_size, axis, cross_length)
                },
            )
            .into_iter()
            .scan(0., |offset, length| {
                Some(mem::replace(offset, *offset + length + gap_length))
            })
            .collect::<Vec<_>>()
        };

        let col_offsets = resolve_track(self, Axis::Horizontal);
        let row_offsets = resolve_track(self, Axis::Vertical);

        let mut baselines = None::<(usize, usize, f64, usize, usize, f64)>;

        for child in &mut self.children {
            ctx.set_stashed(&child.widget, child.cell.is_none());
            let Some((col, row)) = child.cell else {
                continue;
            };
            let width = child.params.width as usize;
            let height = child.params.height as usize;

            if col + width > self.columns.len() || row + height > self.rows.len() {
                continue;
            }

            let size = Size::new(
                col_offsets
                    .get(col + width)
                    .map_or(size.width, |o| o - gap_length)
                    - col_offsets[col],
                row_offsets
                    .get(row + height)
                    .map_or(size.height, |o| o - gap_length)
                    - row_offsets[row],
            )
            .max(Size::ZERO);

            ctx.run_layout(&child.widget, size);

            let origin = Point::new(col_offsets[col], row_offsets[row]);
            ctx.place_child(&child.widget, origin);

            let Some(placement) = ctx.child_placement(&child.widget) else {
                continue;
            };
            let (first_baseline, last_baseline) = placement.parent_baselines();
            let last_row = row + height;

            baselines = Some(match baselines {
                None => (col, row, first_baseline, col, last_row, last_baseline),
                Some((mut col0, mut row0, mut base0, mut col1, mut row1, mut base1)) => {
                    if (row, col) < (row0, col0) {
                        (col0, row0, base0) = (col, row, first_baseline);
                    }
                    if (last_row, col) > (row1, col1) {
                        (col1, row1, base1) = (col, last_row, last_baseline);
                    }
                    (col0, row0, base0, col1, row1, base1)
                }
            });
        }

        if let Some((_, _, first_baseline, _, _, last_baseline)) = baselines {
            ctx.set_baselines(first_baseline, last_baseline);
        } else {
            ctx.clear_baselines();
        }
    }

    fn accessibility_role(&self) -> Role {
        Role::GenericContainer
    }

    fn children_ids(&self) -> ChildrenIds {
        self.children
            .iter()
            .map(|child| child.widget.id())
            .collect()
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("Grid", id = id.trace())
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::AsUnit;
    use crate::palette;
    use crate::properties::types::CrossAxisAlignment;
    use crate::properties::{Background, Dimensions, Padding};
    use crate::testing::{TestHarness, assert_render_snapshot};
    use crate::theme::test_property_set;
    use crate::widgets::{Button, Flex, Label};

    #[test]
    fn intrinsic_growth_respects_fixed_tracks_and_caps() {
        let defs = [LenDef::Fixed(10.px()), LenDef::FitContent(20.px())];
        let mut lengths = [10., 5.];
        distribute_intrinsic(&defs, &mut lengths, 100., 200.);
        assert_eq!(lengths, [10., 20.]);
        let mut fixed = [10.];
        distribute_intrinsic(&[LenDef::Fixed(10.px())], &mut fixed, 100., 200.);
        assert_eq!(fixed, [10.]);
        let mut max = [10.];
        distribute_intrinsic(&[LenDef::MaxContent], &mut max, 30., 80.);
        assert_eq!(max, [90.]);
    }

    #[test]
    fn placement_checks_bounds_and_explicit_reservations() {
        let mut occupancy = GridOccupancy::new(4, 3).unwrap();
        let mut child = Child::new(
            Label::new("reserved").prepare(),
            GridParams::pos(0, 0).with_span(2, 2),
        );
        occupancy.place(&mut child, (0, 0));
        assert_eq!(
            occupancy.find_free(GridParams::new().with_span(2, 2), (0, 0)),
            Some((2, 0))
        );
        assert_eq!(
            occupancy.find_free(GridParams::new().with_width(5), (0, 0)),
            None
        );
        assert_eq!(
            occupancy.find_free(GridParams::new().with_col(u16::MAX), (0, 0)),
            None
        );
        assert_eq!(
            occupancy.find_free(GridParams::new().with_row(2), (3, 2)),
            Some((0, 2))
        );
        assert_eq!(
            occupancy.find_free(
                GridParams {
                    height: 0,
                    ..GridParams::new()
                },
                (0, 0)
            ),
            None
        );
    }

    #[test]
    fn test_grid_basics() {
        // Start with a 1x1 grid
        let widget = Grid::with_dimensions(1, 1)
            .with(Button::with_text("A").prepare(), GridParams::pos(0, 0))
            .prepare()
            .with_props(Dimensions::STRETCH);
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (200, 200));
        // Snapshot with the single widget.
        assert_render_snapshot!(harness, "grid_initial_1x1");

        // Expand it to a 4x4 grid
        harness
            .edit_root_widget(|mut grid| {
                Grid::set_columns(&mut grid, vec![GridTrackSize::FRACTION; 4]);
            })
            .unwrap();
        assert_render_snapshot!(harness, "grid_expanded_4x1");

        harness
            .edit_root_widget(|mut grid| {
                Grid::set_rows(&mut grid, vec![GridTrackSize::FRACTION; 4]);
            })
            .unwrap();
        assert_render_snapshot!(harness, "grid_expanded_4x4");

        // Add a widget that takes up more than one horizontal cell
        harness
            .edit_root_widget(|mut grid| {
                Grid::add(
                    &mut grid,
                    Button::with_text("B").prepare(),
                    GridParams::pos(1, 0).with_span(3, 1),
                );
            })
            .unwrap();
        assert_render_snapshot!(harness, "grid_with_horizontal_widget");

        // Add a widget that takes up more than one vertical cell
        harness
            .edit_root_widget(|mut grid| {
                Grid::add(
                    &mut grid,
                    Button::with_text("C").prepare(),
                    GridParams::pos(0, 1).with_span(1, 3),
                );
            })
            .unwrap();
        assert_render_snapshot!(harness, "grid_with_vertical_widget");

        // Add a widget that takes up more than one horizontal and vertical cell
        harness
            .edit_root_widget(|mut grid| {
                Grid::add(
                    &mut grid,
                    Button::with_text("D").prepare(),
                    GridParams::pos(1, 1).with_span(2, 2),
                );
            })
            .unwrap();
        assert_render_snapshot!(harness, "grid_with_2x2_widget");

        // Change the gap
        harness
            .edit_root_widget(|mut grid| {
                grid.insert_prop(Gap::new(7.px()));
            })
            .unwrap();
        assert_render_snapshot!(harness, "grid_with_changed_spacing");
    }

    #[test]
    fn test_widget_removal_and_modification() {
        let widget = Grid::with_dimensions(2, 2)
            .with(Button::with_text("A").prepare(), GridParams::pos(0, 0))
            .prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (200, 200));
        // Snapshot with the single widget.
        assert_render_snapshot!(harness, "grid_initial_2x2");

        // Now remove the widget
        harness
            .edit_root_widget(|mut grid| {
                Grid::remove(&mut grid, 0).unwrap();
            })
            .unwrap();
        assert_render_snapshot!(harness, "grid_2x2_with_removed_widget");

        // Add it back
        harness
            .edit_root_widget(|mut grid| {
                Grid::add(
                    &mut grid,
                    Button::with_text("A").prepare(),
                    GridParams::pos(0, 0),
                );
            })
            .unwrap();
        assert_render_snapshot!(harness, "grid_initial_2x2"); // Should be back to the original state

        // Test replacement
        harness
            .edit_root_widget(|mut grid| {
                Grid::remove(&mut grid, 0).unwrap();
                Grid::add(
                    &mut grid,
                    Button::with_text("X").prepare(),
                    GridParams::pos(0, 0),
                );
            })
            .unwrap();
        harness
            .edit_root_widget(|mut grid| {
                Grid::set(
                    &mut grid,
                    0,
                    Button::with_text("A").prepare(),
                    GridParams::pos(0, 0),
                )
                .unwrap();
            })
            .unwrap();
        assert_render_snapshot!(harness, "grid_initial_2x2"); // Should be back to the original state

        // Change the grid params to position it on the other corner
        harness
            .edit_root_widget(|mut grid| {
                Grid::set_params(&mut grid, 0, GridParams::pos(1, 1)).unwrap();
            })
            .unwrap();
        assert_render_snapshot!(harness, "grid_moved_2x2_1");

        // Now make it take up the entire grid
        harness
            .edit_root_widget(|mut grid| {
                Grid::set_params(&mut grid, 0, GridParams::pos(0, 0).with_span(2, 2)).unwrap();
            })
            .unwrap();
        assert_render_snapshot!(harness, "grid_moved_2x2_2");
    }

    #[test]
    fn test_widget_order() {
        let widget = Grid::with_dimensions(2, 2)
            .with(Button::with_text("A").prepare(), GridParams::pos(0, 0))
            .prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (200, 200));
        // Snapshot with the single widget.
        assert_render_snapshot!(harness, "grid_initial_2x2");

        // Order sets the draw order, so draw a widget over A by adding it after
        harness
            .edit_root_widget(|mut grid| {
                Grid::add(
                    &mut grid,
                    Button::with_text("B").prepare(),
                    GridParams::pos(0, 0),
                );
            })
            .unwrap();
        assert_render_snapshot!(harness, "grid_2x2_with_overlapping_b");

        // Draw a widget under the others by putting it at index 0
        // Make it wide enough to see it stick out, with half of it under A and B.
        harness
            .edit_root_widget(|mut grid| {
                Grid::insert(
                    &mut grid,
                    0,
                    Button::with_text("C").prepare(),
                    GridParams::pos(0, 0).with_span(2, 1),
                )
                .unwrap();
            })
            .unwrap();
        assert_render_snapshot!(harness, "grid_2x2_with_overlapping_c");
    }

    #[test]
    fn grid_baselines() {
        let grid = Grid::with_dimensions(3, 3)
            .with(
                Label::new("A\nB").prepare().with_props((
                    Padding::from_vh(0.px(), 0.px()),
                    Background::Color(palette::css::ORANGE),
                )),
                GridParams::pos(1, 0),
            )
            .with(
                Label::new("C\nD").prepare().with_props((
                    Padding::from_vh(8.px(), 0.px()),
                    Background::Color(palette::css::DARK_BLUE),
                )),
                GridParams::pos(0, 0).with_span(1, 2),
            )
            .with(
                Label::new("E\nF").prepare().with_props((
                    Padding::from_vh(16.px(), 0.px()),
                    Background::Color(palette::css::DARK_SALMON),
                )),
                GridParams::pos(2, 0).with_span(1, 3),
            )
            .with(
                Label::new("G\nH").prepare().with_props((
                    Padding::from_vh(24.px(), 0.px()),
                    Background::Color(palette::css::DARK_SLATE_BLUE),
                )),
                GridParams::pos(1, 1).with_span(1, 2),
            )
            .prepare()
            .with_props(Dimensions::width(80.px()));

        let root = Flex::row()
            .cross_axis_alignment(CrossAxisAlignment::FirstBaseline)
            .with_fixed(Label::new("Out").prepare())
            .with_fixed(grid)
            .prepare()
            .with_props(Padding::all(10.px()));

        let mut harness = TestHarness::create_with_size(test_property_set(), root, (150, 200));

        assert_render_snapshot!(harness, "grid_baselines_first");

        harness
            .edit_root_widget(|mut root| {
                Flex::set_cross_axis_alignment(&mut root, CrossAxisAlignment::LastBaseline);
            })
            .unwrap();

        assert_render_snapshot!(harness, "grid_baselines_last");
    }

    #[test]
    fn grid_nonuniform() {
        let widget = Grid::new()
            .with_columns([
                GridTrackSize::MinContent,
                GridTrackSize::MaxContent,
                GridTrackSize::Fraction(1.),
            ])
            .with_rows([
                GridTrackSize::Auto,
                GridTrackSize::Fixed(30.px()),
                GridTrackSize::Percentage(0.2),
                GridTrackSize::Fraction(2.),
                GridTrackSize::Fraction(1.),
            ])
            .with(
                Label::new("Min Content")
                    .prepare()
                    .with_props(Background::Color(palette::css::CHOCOLATE.with_alpha(0.5))),
                GridParams::new(),
            )
            .with(
                Label::new("Max Content")
                    .prepare()
                    .with_props(Background::Color(palette::css::OLIVE.with_alpha(0.5))),
                GridParams::new(),
            )
            .with(
                Label::new("1×3")
                    .prepare()
                    .with_props(Background::Color(palette::css::ORANGE_RED.with_alpha(0.5))),
                GridParams::pos(2, 0).with_height(3),
            )
            .with(
                Label::new("20px")
                    .prepare()
                    .with_props(Background::Color(palette::css::MAGENTA.with_alpha(0.5))),
                GridParams::new().with_col(1),
            )
            .with(
                Label::new("30%")
                    .prepare()
                    .with_props(Background::Color(palette::css::SEA_GREEN.with_alpha(0.5))),
                GridParams::new(),
            )
            .with(
                Label::new("2×2")
                    .prepare()
                    .with_props(Background::Color(palette::css::AQUAMARINE.with_alpha(0.5))),
                GridParams::pos(1, 2).with_span(2, 2),
            )
            .with(
                Label::new("2fr")
                    .prepare()
                    .with_props(Background::Color(palette::css::PURPLE.with_alpha(0.5))),
                GridParams::new(),
            )
            .with(
                Label::new("1fr")
                    .prepare()
                    .with_props(Background::Color(palette::css::GOLD.with_alpha(0.5))),
                GridParams::new().with_col(2),
            )
            .prepare()
            .with_props(Gap::new(5.px()))
            .with_props(Dimensions::STRETCH);

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (300, 300));
        assert_render_snapshot!(harness, "grid_nonuniform");
    }

    #[test]
    fn grid_fitcontent_overflow() {
        let widget = Grid::new()
            .with_columns([
                GridTrackSize::MinContent,
                GridTrackSize::FitContent(50.px()),
                GridTrackSize::MinContent,
            ])
            .with_rows([GridTrackSize::Fixed(30.px()); 2])
            .with(
                Label::new("30px")
                    .prepare()
                    .with_props(Dimensions::width(30.px()))
                    .with_props(Background::Color(palette::css::CHOCOLATE.with_alpha(0.5))),
                GridParams::pos(0, 0),
            )
            .with(
                Label::new("40px")
                    .prepare()
                    .with_props(Dimensions::width(40.px()))
                    .with_props(Background::Color(palette::css::BISQUE.with_alpha(0.5))),
                GridParams::pos(1, 0),
            )
            .with(
                Label::new("20px")
                    .prepare()
                    .with_props(Dimensions::width(20.px()))
                    .with_props(Background::Color(palette::css::LIME_GREEN.with_alpha(0.5))),
                GridParams::pos(2, 0),
            )
            .with(
                Label::new("160px")
                    .prepare()
                    .with_props(Dimensions::width(160.px()))
                    .with_props(Background::Color(palette::css::PURPLE.with_alpha(0.5))),
                GridParams::pos(0, 1).with_width(3),
            )
            .prepare()
            .with_props(Gap::new(5.px()))
            .with_props(Dimensions::MIN);

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (300, 300));
        assert_render_snapshot!(harness, "grid_fitcontent_overflow");
    }
}
