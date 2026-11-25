// Copyright 2025 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use understory_virtual_list::{ScrollAlign, SparsePrefixSumExtentModel, VirtualList};

use crate::core::keyboard::{Key, KeyState, NamedKey};
use crate::core::{
    AccessCtx, AccessEvent, ChildrenIds, ComposeCtx, EventCtx, FromDynWidget, KeyboardEvent,
    LayoutCtx, MeasureCtx, NewWidget, PointerEvent, PointerScrollEvent, PropertiesMut,
    PropertiesRef, RegisterCtx, ScrollDelta, TextEvent, Update, UpdateCtx, Widget, WidgetMut,
    WidgetPod,
};
use crate::dpi::PhysicalPosition;
use crate::kurbo::{Axis, Size};
use crate::layout::{LenDef, LenReq, Length, SizeDef};

mod extent_cache;
use extent_cache::ExtentCache;

/// Request to update the materialized children of a [`VirtualScroll`].
///
/// Use [`VirtualScroll::begin_fetch`] to accept the request before modifying
/// children. A request is accepted at most once and only by its originating
/// widget. A delayed request invalidated by a length change is harmless.
///
/// ```ignore
/// let Some(mut fetch) = VirtualScroll::begin_fetch(&mut scroll, &action) else {
///     return;
/// };
/// for idx in action.removed() {
///     // Tear down any associated application state before removing the widget.
///     let _ = fetch.remove_child(idx);
/// }
/// for idx in action.added() {
///     let label = Label::new(format!("Child {idx}"));
///     let _ = fetch.add_child(idx, NewWidget::new(label).erased());
/// }
/// ```
#[derive(Debug, Clone)]
pub struct VirtualScrollFetchAction {
    request: Arc<FetchRequest>,
}

#[derive(Debug)]
struct FetchRequest {
    old_active: Range<usize>,
    target: Range<usize>,
}

/// Mutable access to the children while applying an accepted fetch request.
///
/// The borrow prevents another fetch or list configuration change while editing.
/// Retained children may also be edited via [`Self::child_mut`].
#[derive(Debug)]
pub struct VirtualScrollFetchMut<'a, 'w> {
    scroll: &'a mut WidgetMut<'w, VirtualScroll>,
}

/// The action type sent by the [`VirtualScroll`] widget when the range of visible children changes as a result of scrolling.
#[derive(Debug)]
pub struct VirtualScrollScrollAction {
    range_in_viewport: Range<usize>,
}

/// The actual action type sent by the [`VirtualScroll`] widget.
///
/// It encapsulates fetching and scrolling actions, as a widget can have only one action type.
#[derive(Debug)]
pub enum VirtualScrollAction {
    /// `VirtualScroll` needs to fetch new children.
    Fetch(VirtualScrollFetchAction),
    /// `VirtualScroll` scrolled onto/out of some children.
    Scroll(VirtualScrollScrollAction),
}

/// Direction of scrolling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(missing_docs, reason = "self explanatory")]
pub enum ScrollDirection {
    TopToBottom,
    BottomToTop,
    LeftToRight,
    RightToLeft,
}

/// The portion of the available main axis used as a virtual scroll viewport.
///
/// Both endpoints are fractions measured in the scroll direction, with zero
/// at the start of the axis and one at the end. An empty viewport is valid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportRange {
    start: f64,
    end: f64,
}

impl ViewportRange {
    /// Uses the full available extent.
    pub const FULL: Self = Self { start: 0., end: 1. };

    /// Constructs an ordered, finite interval within the available extent.
    ///
    /// Returns `None` for non-finite, out-of-bounds or reversed endpoints.
    #[must_use = "Use the validated viewport interval."]
    pub const fn new(start: f64, end: f64) -> Option<Self> {
        if start >= 0. && end <= 1. && start <= end {
            Some(Self { start, end })
        } else {
            None
        }
    }

    /// Start fraction in scroll-direction coordinates.
    #[must_use = "Use the viewport start."]
    pub const fn start(self) -> f64 {
        self.start
    }

    /// End fraction in scroll-direction coordinates.
    #[must_use = "Use the viewport end."]
    pub const fn end(self) -> f64 {
        self.end
    }
}

impl Default for ViewportRange {
    fn default() -> Self {
        Self::FULL
    }
}

/// A virtual scrolling widget.
///
/// Virtual scrolling is a technique to improve performance when scrolling through long lists, by
/// only loading (and therefore laying out, drawing, processing for event handling), the items visible to the user.
///
/// Each child of the virtual scroll widget has an unsigned machine word sized id (i.e. a `usize`), and items are laid out
/// in order of these ids.
/// The widget keeps track of which of these ids are loaded, and requests that more are loaded.
/// The widget requires these ids to be dense (that is, if it has a child with ids 1 and 3, it must have a child
/// with id 2).
///
/// This widget works in close coordinate with the [driver](crate::doc::creating_app#the-driver) to
/// load the children; that is, the driver must provide the children when requested.
/// See [usage](#usage) for more details.
///
/// The Masonry example `virtual_fizzbuzz` shows how to use this widget.
/// It creates an infinitely explorable implementation of the game [Fizz buzz](https://en.wikipedia.org/wiki/Fizz_buzz).
///
/// # Usage
///
/// When you create the virtual scroll, you specify the initial "anchor"; that is an id for which the item will be on-screen.
/// If only a subset of ids are valid, then the valid range of ids widget *must* be set.
///
/// The widget will send a [`VirtualScrollFetchAction`] whenever the children it requires to be loaded (the active children) changes.
/// To handle this, the driver must [add](VirtualScrollFetchMut::add_child) widgets for the ids which are in `target` but not in
/// `old_active`, and [remove](VirtualScrollFetchMut::remove_child) those which are in `old_active` but not in `target`.
/// (`VirtualScroll` does not remove the children itself to enable cleanup by the driver before the
/// children get removed).
/// Accept the action with [`VirtualScroll::begin_fetch`], then use the returned
/// [`VirtualScrollFetchMut`] to update the children. Stale requests are rejected.
/// The docs for [`VirtualScrollFetchAction`] include an example demonstrating this.
///
/// It is invalid to not provide all items requested.
/// For items which have not yet loaded, you should either:
/// 1) Provide a placeholder
/// 2) Restrict the valid range to exclude them
///
/// This widget avoids panicking and infinite loops in these cases, but this widget is not designed to
/// handle them, and so arbitrarily janky behaviour may occur.
///
/// As a special case, it is not possible to have an item with id [`usize::MAX`].
/// This is because of the internal use of exclusive ranges.
///
/// # Caveats
///
/// This widget has been developed as an minimum viable solution, and so there are a number of known issues with it.
/// These are discussed below.
///
/// ## Transforms
///
/// Widgets can be [transformed](WidgetMut::set_transform) arbitrarily from where their parent lays them out.
/// This interacts poorly with virtual scrolling, because an item which would be visible due to its
/// transform can be devirtualised, as its layout rectangle is far enough off-screen.
/// Currently, the virtual scrolling controller ignores this case.
/// The long term plan is for each child to be clipped to a reasonable range around itself.
/// The details of how large this clipping area will be have not been decided.
///
/// This will mean that once this is done, the behaviour with transformed widgets will be consistent but not
/// necessarily intuitive (that is, for a given row on screen, the displayed content will always be the same,
/// but some widgets with transforms might not be visible - in the worst case, completely hidden).
// TODO: Implement this.
///
/// ## Focus
///
/// Currently, this widget does not correctly handle focused child widgets.
/// This means that if (for example) the user is typing in a text box in a virtual scroll, and scrolls down,
/// continuing to type will stop working.
///
/// ## Accessibility
///
/// A proper virtual scrolling list needs accessibility support (such as for scrolling, but
/// also to ensure that focus does not get trapped, that the correct set of items are reported,
/// if/that there are more items following, etc.).
///
/// This widget currently exposes basic scrolling semantics (such as `scroll_y` and
/// `ScrollUp`/`ScrollDown` actions) and handles those actions. However, full accessibility
/// behavior for a virtualized list has not yet been designed, and will be a follow-up.
///
/// ## Scrollbars
///
/// There is not yet any integration with scrollbars for this widget.
/// This is planned; however there is no universally correct scrollbar implementation for virtual scrolling.
/// This widget will support user-provided scrollbar types, through some yet-to-be-determined mechanism.
/// There will also be provided implementations of reasonable scrollbar kinds.
///
/// ## Scroll Gestures
///
/// Like [`Portal`](crate::widgets::Portal), this widget does not handle scroll gestures (i.e. with
/// touch screens).
pub struct VirtualScroll {
    virtual_list: VirtualList<SparsePrefixSumExtentModel<f64>>,
    extent_cache: ExtentCache,

    /// The range in the id space which is "active", i.e. which the virtual scrolling has decided
    /// are in the range of the viewport and should be shown on screen.
    /// Note that `items` is not necessarily dense in these; that is, if an
    /// item has not been provided by the application, we don't fall over.
    /// This is still an invalid state, but we handle it as well as we can.
    active_range: Range<usize>,

    /// The single outstanding request, identified by its shared allocation.
    pending_fetch: Option<Arc<FetchRequest>>,

    // The strip used by the last layout, independent of what the driver has
    // acknowledged. Scrolling within one large item can change its bounds.
    laid_out_range: Range<usize>,

    items: BTreeMap<usize, WidgetPod<dyn Widget>>,

    anchor_index: usize,
    range_in_viewport: Range<usize>,

    viewport: ViewportRange,
    direction: ScrollDirection,

    scrolling: bool,
}

impl std::fmt::Debug for VirtualScroll {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VirtualScroll")
            .field("virtual_list", &self.virtual_list)
            .field("extent_cache", &self.extent_cache)
            .field("active_range", &self.active_range)
            .field("pending_fetch", &self.pending_fetch)
            .field("laid_out_range", &self.laid_out_range)
            .field("items", &self.items.keys().collect::<Vec<_>>())
            .field("anchor_index", &self.anchor_index)
            .field("range_in_viewport", &self.range_in_viewport)
            .field("viewport", &self.viewport)
            .field("direction", &self.direction)
            .field("scrolling", &self.scrolling)
            .finish()
    }
}

const DEFAULT_MEAN_ITEM_LENGTH: f64 = 180.;

// --- MARK: BUILDERS
impl VirtualScroll {
    /// Creates a new virtual scrolling list.
    ///
    /// The item at `initial_anchor` will have its top aligned with the top of
    /// the scroll area to start with (clamped to the last item if necessary).
    ///
    /// Note that it is not possible to add children before the widget is "live".
    /// This is for simplicity, as the set of the children which should be loaded has
    /// not yet been determined.
    #[must_use = "Use the returned value."]
    pub fn new(initial_anchor: usize, len: usize) -> Self {
        let initial_anchor = initial_anchor.min(len.saturating_sub(1));
        let mut virtual_list = VirtualList::new(
            SparsePrefixSumExtentModel::new(DEFAULT_MEAN_ITEM_LENGTH, len),
            0.,
            0.,
        );
        virtual_list.scroll_to_index(initial_anchor, ScrollAlign::Start);
        Self {
            virtual_list,
            extent_cache: ExtentCache::default(),
            // This range starts intentionally empty, as no items have been loaded.
            active_range: initial_anchor..initial_anchor,
            pending_fetch: None,
            laid_out_range: initial_anchor..initial_anchor,
            items: BTreeMap::default(),
            anchor_index: initial_anchor,
            range_in_viewport: initial_anchor..initial_anchor,
            viewport: ViewportRange::FULL,
            direction: ScrollDirection::TopToBottom,
            scrolling: false,
        }
    }

    /// Sets the number of child ids which are valid.
    #[track_caller]
    #[must_use = "Use the returned value."]
    pub fn with_len(mut self, len: usize) -> Self {
        self.virtual_list.set_len(len);
        self.anchor_index = self.anchor_index.min(len.saturating_sub(1));
        self.virtual_list
            .scroll_to_index(self.anchor_index, ScrollAlign::Start);
        self.active_range = self.anchor_index..self.anchor_index;
        self.laid_out_range = self.active_range.clone();
        self.range_in_viewport = self.active_range.clone();
        self
    }

    /// Sets the viewport interval in scroll-direction coordinates.
    #[must_use = "Use the returned value."]
    pub const fn with_viewport(mut self, viewport: ViewportRange) -> Self {
        self.viewport = viewport;
        self
    }

    /// Sets the direction in which children are laid out.
    #[must_use = "Use the returned value."]
    pub const fn with_direction(mut self, direction: ScrollDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Sets scrolling state.
    ///
    /// Adjusts pixel snapping for animations.
    #[must_use = "Use the returned value."]
    pub const fn with_scrolling(mut self, scrolling: bool) -> Self {
        self.scrolling = scrolling;
        self
    }
}

// --- MARK: METHODS
impl VirtualScroll {
    /// The number of currently active children in this widget.
    ///
    /// This is intended for sanity-checking of higher-level processes (i.e. so that inconsistencies can be caught early).
    #[expect(
        clippy::len_without_is_empty,
        reason = "The only time the VirtualScroll unloads all children is when given an empty valid range."
    )]
    #[must_use = "Use the returned value."]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    // Deltas here are in index-order coordinates. Return whether the clamped
    // offset changed, not merely whether the requested delta was nonzero.
    fn advance_offset(&mut self, delta: f64) -> bool {
        if !delta.is_finite() || delta == 0.0 {
            return false;
        }
        let old = self.virtual_list.scroll_offset();
        self.virtual_list.scroll_by(delta);
        self.virtual_list.clamp_scroll_to_content();
        self.virtual_list.scroll_offset().to_bits() != old.to_bits()
    }

    fn scroll_event(&mut self, ctx: &mut EventCtx<'_>, delta: f64) {
        if self.advance_offset(delta) {
            if self.needs_layout() {
                ctx.request_layout();
            }
            ctx.request_compose();
            ctx.set_handled();
        }
    }

    /// Layout is necessary when the coordinate anchor or materialized strip changes.
    fn needs_layout(&mut self) -> bool {
        self.virtual_list.clamp_scroll_to_content();

        let scroll_offset = self.virtual_list.scroll_offset();
        let offset_of_anchor = self.virtual_list.offset_of(self.anchor_index);
        scroll_offset < offset_of_anchor
            || scroll_offset >= offset_of_anchor + self.virtual_list.extent_of(self.anchor_index)
            || self.virtual_list.materialized_range() != self.laid_out_range
    }

    // Before changing the extent model, capture the actual logical item under
    // the viewport, not the placement anchor from the previous layout.
    fn current_anchor(&mut self) -> (usize, f64) {
        if self.virtual_list.is_empty() {
            return (0, 0.);
        }
        let offset = self.virtual_list.scroll_offset();
        let previous = self.anchor_index.min(self.virtual_list.len() - 1);
        let relative = offset - self.virtual_list.offset_of(previous);
        if relative >= 0. && (relative == 0. || relative < self.virtual_list.extent_of(previous)) {
            return (previous, relative);
        }
        let index = self.virtual_list.index_at_offset(offset);
        (index, (offset - self.virtual_list.offset_of(index)).max(0.))
    }

    fn scroll_offset_from_anchor(&mut self) -> f64 {
        self.virtual_list.scroll_offset() - self.virtual_list.offset_of(self.anchor_index)
    }
}

// -- MARK: IMPL OTHERS
impl VirtualScrollFetchAction {
    /// The range of children ids which were "active" before this change.
    /// That is, the items which the driver wanted to have available, to properly load what it needs.
    /// Note that many of these items will likely still be active even after this event;
    /// only those which aren't also in `target` must be removed.
    #[must_use = "Use the returned value."]
    pub fn old_active(&self) -> &Range<usize> {
        &self.request.old_active
    }

    /// The range of items which are now active.
    ///
    /// Note that many of these items will have previously been active before this event (and so require no action);
    /// only those which weren't also in `old_active` must be added.
    #[must_use = "Use the returned value."]
    pub fn target(&self) -> &Range<usize> {
        &self.request.target
    }

    /// Indices that have left the materialized range.
    #[must_use = "Iterate over the removed indices."]
    pub fn removed(&self) -> impl Iterator<Item = usize> + '_ {
        range_difference(self.old_active(), self.target())
    }

    /// Indices that have entered the materialized range.
    #[must_use = "Iterate over the added indices."]
    pub fn added(&self) -> impl Iterator<Item = usize> + '_ {
        range_difference(self.target(), self.old_active())
    }
}

// The difference of two half-open ranges has at most two contiguous pieces.
fn range_difference(a: &Range<usize>, b: &Range<usize>) -> impl Iterator<Item = usize> {
    (a.start..a.end.min(b.start)).chain(a.start.max(b.end)..a.end)
}

impl VirtualScrollScrollAction {
    /// Provides the IDs of the range of children in viewport.
    #[must_use = "Use the returned value."]
    pub const fn range_in_viewport(&self) -> &Range<usize> {
        &self.range_in_viewport
    }
}

impl ScrollDirection {
    const fn axis(self) -> Axis {
        match self {
            Self::TopToBottom | Self::BottomToTop => Axis::Vertical,
            Self::LeftToRight | Self::RightToLeft => Axis::Horizontal,
        }
    }

    const fn is_reverse(self) -> bool {
        matches!(self, Self::BottomToTop | Self::RightToLeft)
    }

    // Backward and forward relative to index order, not screen coordinates.
    const fn access_actions(self) -> (accesskit::Action, accesskit::Action) {
        use accesskit::Action::{ScrollDown, ScrollLeft, ScrollRight, ScrollUp};
        match self {
            Self::TopToBottom => (ScrollUp, ScrollDown),
            Self::BottomToTop => (ScrollDown, ScrollUp),
            Self::LeftToRight => (ScrollLeft, ScrollRight),
            Self::RightToLeft => (ScrollRight, ScrollLeft),
        }
    }

    fn appropriate(self, delta: f64) -> f64 {
        if self.is_reverse() { -delta } else { delta }
    }
}

// --- MARK: WIDGETMUT
impl VirtualScroll {
    /// Returns whether this request is still outstanding for this widget.
    #[must_use = "Check whether this request can be accepted."]
    pub fn is_pending_fetch(&self, action: &VirtualScrollFetchAction) -> bool {
        self.pending_fetch
            .as_ref()
            .is_some_and(|pending| Arc::ptr_eq(pending, &action.request))
    }

    /// Accepts a fetch request and grants scoped access to its children.
    ///
    /// Returns `None` for stale, already accepted, or misrouted requests,
    /// without modifying the widget. Delayed valid requests remain acceptable
    /// even when the viewport has moved; layout will request any newer range.
    pub fn begin_fetch<'a, 'w>(
        this: &'a mut WidgetMut<'w, Self>,
        action: &VirtualScrollFetchAction,
    ) -> Option<VirtualScrollFetchMut<'a, 'w>> {
        if !this.widget.is_pending_fetch(action) {
            return None;
        }
        this.widget.pending_fetch = None;
        this.widget.active_range = action.target().clone();
        this.ctx.request_layout();
        Some(VirtualScrollFetchMut { scroll: this })
    }

    /// Returns a mutable handle if the index contains a registered child widget.
    #[track_caller]
    pub fn child_mut<'t, W: Widget + FromDynWidget + ?Sized>(
        this: &'t mut WidgetMut<'_, Self>,
        idx: usize,
    ) -> Option<WidgetMut<'t, W>> {
        this.ctx.get_mut(this.widget.items.get(&idx)?.id())
    }

    /// Sets the valid number of items.
    ///
    /// That is, the children which the virtual scrolling area will request within.
    /// Runtime equivalent of [`with_len`](Self::with_len).
    pub fn set_len(this: &mut WidgetMut<'_, Self>, len: usize) {
        if this.widget.virtual_list.len() == len {
            return;
        }
        let state = &mut *this.widget;
        let (anchor, _) = state.current_anchor();
        state.virtual_list.set_len(len);
        state.extent_cache.truncate(len);
        if len == 0 || anchor >= len {
            state.anchor_index = len.saturating_sub(1);
            state
                .virtual_list
                .scroll_to_index(state.anchor_index, ScrollAlign::Start);
        }
        if this
            .widget
            .pending_fetch
            .as_ref()
            .is_some_and(|pending| pending.target.end > len)
        {
            this.widget.pending_fetch = None;
        }
        this.ctx.request_layout();
    }

    /// Discards remembered estimates for unmaterialized content.
    ///
    /// Use this when the underlying item collection changes without changing
    /// its length, such as after reordering or replacing all items.
    pub fn invalidate_extents(this: &mut WidgetMut<'_, Self>) {
        this.widget.extent_cache.invalidate();
        this.ctx.request_layout();
    }

    /// Changes both viewport endpoints atomically.
    pub fn set_viewport(this: &mut WidgetMut<'_, Self>, viewport: ViewportRange) {
        if this.widget.viewport != viewport {
            this.widget.viewport = viewport;
            this.ctx.request_layout();
        }
    }

    /// Sets the direction in which children are laid out.
    pub fn set_direction(this: &mut WidgetMut<'_, Self>, direction: ScrollDirection) {
        if this.widget.direction != direction {
            this.widget.direction = direction;
            this.ctx.request_layout();
        }
    }

    /// Sets scrolling state.
    ///
    /// Adjusts pixel snapping for animations.
    pub fn set_scrolling(this: &mut WidgetMut<'_, Self>, scrolling: bool) {
        if this.widget.scrolling != scrolling {
            this.widget.scrolling = scrolling;
            this.ctx.request_compose();
        }
    }

    /// Move the viewport by logical pixels in visual coordinates.
    ///
    /// Positive values move down or right; negative values move up or left.
    /// Returns whether the clamped scroll position changed.
    pub fn scroll_by(this: &mut WidgetMut<'_, Self>, delta: f64) -> bool {
        if !this
            .widget
            .advance_offset(this.widget.direction.appropriate(delta))
        {
            return false;
        }
        if this.widget.needs_layout() {
            this.ctx.request_layout();
        }
        this.ctx.request_compose();
        true
    }

    /// Forcefully aligns the top of the item at `idx` with the top of the
    /// virtual scroll area.
    ///
    /// That is, scroll to the item at `idx`, losing any scroll progress by the user.
    /// An out-of-range index selects the last item, or offset zero in an empty list.
    ///
    /// This method is mostly useful for tests, but can be used outside of tests
    /// (for example, in certain scrollbar schemes).
    pub fn scroll_to(this: &mut WidgetMut<'_, Self>, idx: usize) {
        let idx = idx.min(this.widget.virtual_list.len().saturating_sub(1));
        this.widget.anchor_index = idx;
        this.widget
            .virtual_list
            .scroll_to_index(idx, ScrollAlign::Start);
        this.ctx.request_layout();
    }
}

impl VirtualScrollFetchMut<'_, '_> {
    /// Adds a new child in the accepted target range.
    ///
    /// Returns false if the index is outside the range or already occupied.
    /// An existing registered child is never replaced.
    #[must_use = "Check whether the child was added."]
    pub fn add_child(&mut self, idx: usize, child: NewWidget<dyn Widget>) -> bool {
        if !self.scroll.widget.active_range.contains(&idx) {
            tracing::warn!(idx, "virtual scroll child outside accepted range");
            return false;
        }
        if let std::collections::btree_map::Entry::Vacant(entry) =
            self.scroll.widget.items.entry(idx)
        {
            entry.insert(child.into_pod());
            self.scroll.ctx.children_changed();
            true
        } else {
            tracing::warn!(idx, "virtual scroll child already exists");
            false
        }
    }

    /// Removes an existing child outside the accepted target range.
    ///
    /// Returns false if the child is still active or was never materialized.
    /// Missing children are normal for an incomplete driver response.
    #[must_use = "Check whether the child was removed."]
    pub fn remove_child(&mut self, idx: usize) -> bool {
        if self.scroll.widget.active_range.contains(&idx) {
            tracing::warn!(idx, "cannot remove active virtual scroll child");
            return false;
        }
        if let Some(child) = self.scroll.widget.items.remove(&idx) {
            self.scroll.ctx.remove_child(child);
            true
        } else {
            false
        }
    }

    /// Returns a mutable handle to an existing child, including one being removed.
    pub fn child_mut<W: Widget + FromDynWidget + ?Sized>(
        &mut self,
        idx: usize,
    ) -> Option<WidgetMut<'_, W>> {
        VirtualScroll::child_mut(self.scroll, idx)
    }
}

// --- MARK: IMPL WIDGET
impl Widget for VirtualScroll {
    type Action = VirtualScrollAction;

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        if let PointerEvent::Scroll(PointerScrollEvent { delta, .. }) = event {
            let content_box = ctx.content_box();
            let scale_factor = ctx.scale_factor();
            let line_px = PhysicalPosition {
                x: 120.0 * scale_factor,
                y: 120.0 * scale_factor,
            };
            let page_px = PhysicalPosition {
                x: content_box.width() * scale_factor,
                y: content_box.height() * scale_factor,
            };

            let delta_px = delta
                .to_pixel_delta(line_px, page_px)
                .to_logical::<f64>(scale_factor);
            // Keep pixel gestures on their actual axis. A traditional vertical
            // line wheel can also operate a horizontal list without a trackpad.
            let visual_delta = match self.direction.axis() {
                Axis::Vertical => -delta_px.y,
                Axis::Horizontal
                    if delta_px.x == 0.0 && matches!(delta, ScrollDelta::LineDelta(..)) =>
                {
                    -delta_px.y
                }
                Axis::Horizontal => -delta_px.x,
            };
            self.scroll_event(ctx, self.direction.appropriate(visual_delta));
        }
    }

    fn on_text_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &TextEvent,
    ) {
        const DELTA_LINE: f64 = 20.;
        let page = self.virtual_list.viewport_extent();

        // To get to this state, you currently need to press "tab" to focus this widget in the
        // example.
        let TextEvent::Keyboard(KeyboardEvent {
            state: KeyState::Down,
            key: Key::Named(key),
            ..
        }) = event
        else {
            return;
        };

        // For vertical layouts, PageDown/ArrowDown scroll forward and PageUp/ArrowUp scroll back.
        // For horizontal layouts, PageDown/ArrowRight scroll forward and PageUp/ArrowLeft scroll back.
        // In both cases "forward" means increasing scroll offset (towards end of list).
        // For BottomToTop and RightToLeft, direction.appropriate() negates the delta so that
        // the arrow key which moves visually "forward" still increases the scroll offset correctly.
        let delta = match (key, self.direction.axis()) {
            (NamedKey::PageDown, _) => Some((page, true)),
            (NamedKey::PageUp, _) => Some((-page, true)),
            (NamedKey::ArrowDown, Axis::Vertical) | (NamedKey::ArrowRight, Axis::Horizontal) => {
                Some((DELTA_LINE, false))
            }
            (NamedKey::ArrowUp, Axis::Vertical) | (NamedKey::ArrowLeft, Axis::Horizontal) => {
                Some((-DELTA_LINE, false))
            }
            _ => None,
        };
        if let Some((delta, direct)) = delta {
            self.scroll_event(
                ctx,
                if direct {
                    delta
                } else {
                    self.direction.appropriate(delta)
                },
            );
        }
    }

    fn on_access_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &AccessEvent,
    ) {
        let (backward, forward) = self.direction.access_actions();
        let sign = if event.action == backward {
            -1.
        } else if event.action == forward {
            1.
        } else {
            return;
        };
        let delta = match event.data {
            Some(accesskit::ActionData::ScrollUnit(accesskit::ScrollUnit::Page)) => {
                self.virtual_list.viewport_extent()
            }
            _ => self.virtual_list.extent_of(self.anchor_index),
        };

        self.scroll_event(ctx, sign * delta);
    }

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        for child in self.items.values_mut() {
            ctx.register_child(child);
        }
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        if let Update::RequestPanToChild { rect, .. } = event {
            let length = ctx.content_box().size().get_coord(self.direction.axis());
            let viewport = if self.direction.is_reverse() {
                (1.0 - self.viewport.end()) * length..(1.0 - self.viewport.start()) * length
            } else {
                self.viewport.start() * length..self.viewport.end() * length
            };
            let target = rect.get_coords(self.direction.axis());
            let new_pos = super::compute_pan_range(viewport.clone(), target.0..target.1).start;
            let delta = self.direction.appropriate(new_pos - viewport.start);
            if self.advance_offset(delta) {
                if self.needs_layout() {
                    ctx.request_layout();
                }
                ctx.request_compose();
            }
        }
    }

    fn measure(
        &mut self,
        _ctx: &mut MeasureCtx<'_>,
        _props: &PropertiesRef<'_>,
        _axis: Axis,
        len_req: LenReq,
        _cross_length: Option<Length>,
    ) -> Length {
        // Our preferred size is a const square in logical pixels.
        //
        // It is not clear that a data-derived result would be better.
        // We definitely can't load all the children to calculate our unclipped size.
        //
        // If we would base it on the currently loaded items, then the preferred size
        // would fluctuate all over the place. The UI experience would be miserable,
        // with our viewport size frequently changing as the user is scrolling.
        //
        // Perhaps it would be worth it to always keep some first N items in memory and
        // derive our preferred size always from those. That way it would be much more stable.
        // We could also detect if we have a defined size via props and then unload those items.
        // Still, we would run into complexities with ensuring they are loaded in time for measure.
        //
        // So, for now, we just use a simple O(1) default.
        const DEFAULT_LENGTH: Length = crate::layout::length!(100.);

        match len_req {
            LenReq::Min | LenReq::Max => DEFAULT_LENGTH,
            LenReq::Fit(space) => space,
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, size: Size) {
        ctx.set_clip_path(size.to_rect());

        let (anchor, intra_item_offset) = self.current_anchor();
        let maximum =
            (self.virtual_list.total_extent() - self.virtual_list.viewport_extent()).max(0.);
        let at_end = maximum > 0. && self.virtual_list.scroll_offset() >= maximum;

        // Measure registered children under the current constraints. The
        // core already caches child measurement; only changed main-axis
        // extents should invalidate the virtual list's prefix/strip caches.
        let axis = self.direction.axis();
        let auto_size = SizeDef::fit(size).with(axis, LenDef::MaxContent);
        let len = self.virtual_list.len();
        let sizes: Vec<_> = self
            .items
            .iter()
            .map(|(&index, child)| {
                // Pending driver teardown may leave out-of-range children registered.
                (index < len).then(|| ctx.compute_size(child, auto_size, size.into()))
            })
            .collect();
        let live = self
            .items
            .keys()
            .copied()
            .zip(sizes.iter())
            .filter_map(|(index, size)| size.map(|size| (index, size.get_coord(axis))));
        self.extent_cache
            .sync(&mut self.virtual_list, (axis, size), anchor, live);

        let main_axis_length = size.get_coord(self.direction.axis());
        let start_at = self.viewport.start() * main_axis_length;
        let end_at = self.viewport.end() * main_axis_length;
        self.virtual_list.set_viewport_extent(end_at - start_at);
        self.virtual_list.set_overscan(
            main_axis_length + start_at,
            main_axis_length.mul_add(3., -end_at),
        );

        let offset = if at_end {
            (self.virtual_list.total_extent() - self.virtual_list.viewport_extent()).max(0.)
        } else {
            self.virtual_list.offset_of(anchor)
                + intra_item_offset.min(self.virtual_list.extent_of(anchor))
        };
        self.virtual_list.set_scroll_offset(offset);
        self.virtual_list.clamp_scroll_to_content();

        self.anchor_index = self
            .virtual_list
            .viewport_range()
            .start
            .min(len.saturating_sub(1));

        // Keep one outstanding request until the driver accepts it. Frame count
        // is not a timeout: delayed drivers may legitimately take many redraws.
        let materialized = self.virtual_list.materialized_range();
        self.laid_out_range = materialized.clone();
        if self.pending_fetch.is_none() && self.active_range != materialized {
            let request = Arc::new(FetchRequest {
                old_active: self.active_range.clone(),
                target: materialized.clone(),
            });
            self.pending_fetch = Some(Arc::clone(&request));
            ctx.submit_action::<VirtualScrollAction>(VirtualScrollAction::Fetch(
                VirtualScrollFetchAction { request },
            ));
        }

        // place children
        let offset_of_anchor = self.virtual_list.offset_of(self.anchor_index);
        for ((idx, child), child_size) in self.items.iter_mut().zip(sizes) {
            let child_size = child_size.filter(|_| materialized.contains(idx));
            ctx.set_stashed(child, child_size.is_none());
            if let Some(child_size) = child_size {
                ctx.run_layout(child, child_size);
                let pos = self.virtual_list.offset_of(*idx) - offset_of_anchor;
                let placed_pos = if self.direction.is_reverse() {
                    -pos - self.virtual_list.extent_of(*idx)
                } else {
                    pos
                };
                ctx.place_child(child, self.direction.axis().pack_point(placed_pos, 0.));
            }
        }
    }

    fn compose(&mut self, ctx: &mut ComposeCtx<'_>) {
        let content_length = ctx.content_box().size().get_coord(self.direction.axis());
        let offset = self
            .viewport
            .start()
            .mul_add(-content_length, self.scroll_offset_from_anchor());
        let offset = if self.direction.is_reverse() {
            content_length - self.direction.appropriate(offset)
        } else {
            -self.direction.appropriate(offset)
        };
        let translation = self.direction.axis().pack_vec2(offset, 0.);
        for (_, child) in self.items.range_mut(self.laid_out_range.clone()) {
            if self.scrolling {
                ctx.set_animated_child_scroll_translation(child, translation);
            } else {
                ctx.set_child_scroll_translation(child, translation);
            }
        }

        let range_in_viewport = self.virtual_list.viewport_range();
        if std::mem::replace(&mut self.range_in_viewport, range_in_viewport.clone())
            != range_in_viewport
        {
            ctx.submit_action::<VirtualScrollAction>(VirtualScrollAction::Scroll(
                VirtualScrollScrollAction { range_in_viewport },
            ));
        }
    }

    fn accessibility_role(&self) -> accesskit::Role {
        accesskit::Role::ScrollView
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut accesskit::Node,
    ) {
        node.set_clips_children();
        node.set_orientation(match self.direction.axis() {
            Axis::Horizontal => accesskit::Orientation::Horizontal,
            Axis::Vertical => accesskit::Orientation::Vertical,
        });
        // Use the same bounds as VirtualList::clamp_scroll_to_content. Unlike a
        // visible index range, these distinguish partial visibility at the end.
        let maximum =
            (self.virtual_list.total_extent() - self.virtual_list.viewport_extent()).max(0.);
        let offset = self.virtual_list.scroll_offset().clamp(0., maximum);
        let (minimum, maximum_visual, position) = if self.direction.is_reverse() {
            (-maximum, 0., -offset)
        } else {
            (0., maximum, offset)
        };

        match self.direction.axis() {
            Axis::Horizontal => {
                node.set_scroll_x_min(minimum);
                node.set_scroll_x_max(maximum_visual);
                node.set_scroll_x(position);
            }
            Axis::Vertical => {
                node.set_scroll_y_min(minimum);
                node.set_scroll_y_max(maximum_visual);
                node.set_scroll_y(position);
            }
        }

        let (backward, forward) = self.direction.access_actions();
        if offset > 0. {
            node.add_action(backward);
        }
        if offset < maximum {
            node.add_action(forward);
        }
        node.add_child_action(accesskit::Action::ScrollIntoView);
    }

    fn children_ids(&self) -> ChildrenIds {
        self.items.values().map(WidgetPod::id).collect()
    }

    fn accepts_text_input(&self) -> bool {
        false
    }

    fn accepts_focus(&self) -> bool {
        // Our focus behaviour is not carefully designed.
        // There are a few things to consider:
        // - We want this widget to accept e.g. pagedown events, even when there is no focusable child
        // - We want the keyboard focus to be able to "escape" the virtual list, rather than be trapped.
        // See also the caveat in the main docs for this widget.
        // This is true for now to allow PageDown events to be handled.
        true
    }

    // TODO: Optimise using binary search?
    // fn find_widget_under_pointer(..);

    fn get_debug_text(&self) -> Option<String> {
        Some(format!("{self:#?}"))
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use crate::core::{NewWidget, Widget, WidgetId, WidgetMut};
    use crate::kurbo::Vec2;
    use crate::parley::StyleProperty;
    use crate::testing::{TestHarness, assert_render_snapshot};
    use crate::theme::test_property_set;
    use crate::widgets::Label;

    use super::*;

    fn next_fetch(harness: &mut TestHarness<VirtualScroll>) -> VirtualScrollFetchAction {
        loop {
            let (action, _) = harness
                .pop_action::<VirtualScrollAction>()
                .unwrap()
                .unwrap();
            if let VirtualScrollAction::Fetch(action) = action {
                return action;
            }
        }
    }

    fn fixed_size_driver(
        action: VirtualScrollFetchAction,
        mut scroll: WidgetMut<'_, VirtualScroll>,
    ) {
        use crate::layout::AsUnit;
        use crate::widgets::SizedBox;

        let mut fetch = VirtualScroll::begin_fetch(&mut scroll, &action).unwrap();
        for idx in action.removed() {
            assert!(fetch.remove_child(idx));
        }
        for idx in action.added() {
            assert!(fetch.add_child(
                idx,
                SizedBox::empty().size(50.px(), 50.px()).prepare().erased()
            ));
        }
    }

    #[test]
    fn viewport_range_is_finite_ordered_and_atomic() {
        for (start, end) in [
            (f64::NAN, 1.),
            (0., f64::INFINITY),
            (f64::NEG_INFINITY, 0.),
            (-0.1, 1.),
            (0., 1.1),
            (0.75, 0.25),
        ] {
            assert!(ViewportRange::new(start, end).is_none());
        }
        assert!(ViewportRange::new(0.5, 0.5).is_some());
        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            VirtualScroll::new(0, 100)
                .with_viewport(ViewportRange::new(0.1, 0.2).unwrap())
                .prepare(),
            (100, 200),
        );
        let id = harness.root_id();
        drive_to_fixpoint(&mut harness, id, fixed_size_driver);
        harness
            .edit_root_widget(|mut scroll| {
                VirtualScroll::set_viewport(&mut scroll, ViewportRange::new(0.75, 1.).unwrap());
            })
            .unwrap();
        drive_to_fixpoint(&mut harness, id, fixed_size_driver);
        assert_eq!(
            harness
                .root_widget()
                .unwrap()
                .virtual_list
                .viewport_extent(),
            50.
        );
        assert_eq!(
            harness.root_widget().unwrap().viewport,
            ViewportRange::new(0.75, 1.).unwrap()
        );
    }

    #[test]
    fn keyboard_and_accessibility_scroll_follow_visual_direction() {
        use accesskit::{Action, ActionData, ActionRequest, ScrollUnit, TreeId};

        let cases = [
            (
                ScrollDirection::TopToBottom,
                NamedKey::ArrowDown,
                NamedKey::ArrowUp,
                Action::ScrollDown,
                Action::ScrollUp,
                1.,
            ),
            (
                ScrollDirection::BottomToTop,
                NamedKey::ArrowUp,
                NamedKey::ArrowDown,
                Action::ScrollUp,
                Action::ScrollDown,
                -1.,
            ),
            (
                ScrollDirection::LeftToRight,
                NamedKey::ArrowRight,
                NamedKey::ArrowLeft,
                Action::ScrollRight,
                Action::ScrollLeft,
                1.,
            ),
            (
                ScrollDirection::RightToLeft,
                NamedKey::ArrowLeft,
                NamedKey::ArrowRight,
                Action::ScrollLeft,
                Action::ScrollRight,
                -1.,
            ),
        ];
        for (direction, forward_key, back_key, forward, back, sign) in cases {
            let mut harness = TestHarness::create_with_size(
                test_property_set(),
                VirtualScroll::new(0, 10)
                    .with_direction(direction)
                    .prepare(),
                (200, 200),
            );
            let id = harness.root_id();
            drive_to_fixpoint(&mut harness, id, fixed_size_driver);
            let _ = harness.redraw();
            let node = harness.access_node(id).unwrap();
            assert!(node.data().supports_action(forward));
            assert!(!node.data().supports_action(back));

            harness.focus_on(Some(id));
            harness.process_text_event(&TextEvent::key_down(Key::Named(forward_key)));
            assert_eq!(
                harness.root_widget().unwrap().virtual_list.scroll_offset(),
                20.
            );
            let _ = harness.redraw();
            let node = harness.access_node(id).unwrap();
            let (position, minimum, maximum) = match direction.axis() {
                Axis::Vertical => (
                    node.data().scroll_y(),
                    node.data().scroll_y_min(),
                    node.data().scroll_y_max(),
                ),
                Axis::Horizontal => (
                    node.data().scroll_x(),
                    node.data().scroll_x_min(),
                    node.data().scroll_x_max(),
                ),
            };
            assert_eq!(position, Some(sign * 20.));
            assert_eq!(minimum, Some(if sign < 0. { -300. } else { 0. }));
            assert_eq!(maximum, Some(if sign < 0. { 0. } else { 300. }));
            assert!(node.data().supports_action(back));
            harness.process_text_event(&TextEvent::key_down(Key::Named(back_key)));
            assert_eq!(
                harness.root_widget().unwrap().virtual_list.scroll_offset(),
                0.
            );

            harness.process_access_event(ActionRequest {
                action: forward,
                target_tree: TreeId::ROOT,
                target_node: id.into(),
                data: Some(ActionData::ScrollUnit(ScrollUnit::Page)),
            });
            drive_to_fixpoint(&mut harness, id, fixed_size_driver);
            assert_eq!(
                harness.root_widget().unwrap().virtual_list.scroll_offset(),
                200.
            );

            harness.process_text_event(&TextEvent::key_down(Key::Named(NamedKey::PageDown)));
            drive_to_fixpoint(&mut harness, id, fixed_size_driver);
            assert_eq!(
                harness.root_widget().unwrap().virtual_list.scroll_offset(),
                300.
            );
            let _ = harness.redraw();
            let node = harness.access_node(id).unwrap();
            assert!(node.data().supports_action(back));
            assert!(!node.data().supports_action(forward));
        }
    }

    #[test]
    fn nested_wheel_scrolls_outer_portal_only_at_inner_boundary() {
        use crate::core::WidgetTag;
        use crate::layout::AsUnit;
        use crate::properties::{Dimensions, Gap};
        use crate::widgets::{Flex, Portal};

        let inner = WidgetTag::named("virtual-scroll-child");
        let content = Flex::column()
            .with_fixed(
                VirtualScroll::new(0, 4)
                    .prepare()
                    .with_tag(inner)
                    .with_props(Dimensions::fixed(100.px(), 100.px())),
            )
            .with_fixed_spacer(300.px())
            .prepare()
            .with_props(Gap::new(Length::ZERO));
        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            Portal::new(content).constrain_horizontal(true).prepare(),
            (100, 100),
        );
        for _ in 0..64 {
            let Some((action, _)) = harness.pop_action::<VirtualScrollAction>().unwrap() else {
                break;
            };
            if let VirtualScrollAction::Fetch(fetch) = action {
                harness
                    .edit_widget(inner, |scroll| fixed_size_driver(fetch, scroll))
                    .unwrap();
            }
        }
        assert!(
            harness.pop_action_erased().is_none(),
            "materialization must converge"
        );
        harness.mouse_move((40.0, 40.0));
        harness.mouse_wheel(Vec2::new(0.0, -50.0));
        assert_eq!(
            harness
                .get_widget(inner)
                .unwrap()
                .virtual_list
                .scroll_offset(),
            50.0
        );
        assert_eq!(harness.root_widget().unwrap().get_viewport_pos().y, 0.0);

        harness.mouse_wheel(Vec2::new(0.0, -100.0));
        assert_eq!(
            harness
                .get_widget(inner)
                .unwrap()
                .virtual_list
                .scroll_offset(),
            100.0
        );
        assert_eq!(harness.root_widget().unwrap().get_viewport_pos().y, 0.0);

        harness.mouse_wheel(Vec2::new(0.0, -50.0));
        assert_eq!(
            harness
                .get_widget(inner)
                .unwrap()
                .virtual_list
                .scroll_offset(),
            100.0
        );
        assert_eq!(harness.root_widget().unwrap().get_viewport_pos().y, 50.0);
    }

    #[test]
    fn reveal_materialized_item_uses_current_visual_coordinates_in_all_directions() {
        for direction in [
            ScrollDirection::TopToBottom,
            ScrollDirection::BottomToTop,
            ScrollDirection::LeftToRight,
            ScrollDirection::RightToLeft,
        ] {
            let mut harness = TestHarness::create_with_size(
                test_property_set(),
                VirtualScroll::new(0, 20)
                    .with_direction(direction)
                    .prepare(),
                (200, 200),
            );
            let id = harness.root_id();
            drive_to_fixpoint(&mut harness, id, fixed_size_driver);
            let child = harness.root_widget().unwrap().items[&5].id();
            for _ in 0..2 {
                harness.scroll_into_view(child);
                drive_to_fixpoint(&mut harness, id, fixed_size_driver);
                let scroll = harness.root_widget().unwrap().virtual_list.scroll_offset();
                assert!(
                    (scroll - 100.0).abs() < 1e-6,
                    "{direction:?}: reveal offset should be 100, got {scroll}"
                );
            }
        }
    }

    #[test]
    fn intra_item_scroll_fetches_newly_materialized_neighbors() {
        use crate::layout::AsUnit;
        use crate::widgets::SizedBox;

        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            VirtualScroll::new(0, 100).prepare(),
            (100, 100),
        );
        let id = harness.root_id();
        let driver = |action: VirtualScrollFetchAction,
                      mut scroll: WidgetMut<'_, VirtualScroll>| {
            let mut fetch = VirtualScroll::begin_fetch(&mut scroll, &action).unwrap();
            for idx in action.removed() {
                assert!(fetch.remove_child(idx));
            }
            for idx in action.added() {
                let height = if idx == 0 { 1000.px() } else { 50.px() };
                assert!(fetch.add_child(
                    idx,
                    SizedBox::empty().size(50.px(), height).prepare().erased()
                ));
            }
        };
        drive_to_fixpoint(&mut harness, id, driver);
        assert_eq!(
            harness.root_widget().unwrap().active_range,
            0..1,
            "large first item initially covers the materialized strip"
        );
        harness
            .edit_root_widget(|mut scroll| {
                assert!(VirtualScroll::scroll_by(&mut scroll, 800.0));
            })
            .unwrap();
        let request = next_fetch(&mut harness);
        assert!(
            request.target().contains(&1),
            "newly materialized neighbor must be requested without crossing the anchor"
        );
        assert_eq!(
            harness.root_widget().unwrap().anchor_index,
            0,
            "scroll remains inside the first item"
        );
        harness
            .edit_root_widget(|scroll| driver(request, scroll))
            .unwrap();
        drive_to_fixpoint(&mut harness, id, driver);
        assert!(
            harness.root_widget().unwrap().items.contains_key(&1),
            "neighbor is materialized before reaching it"
        );
    }

    #[test]
    fn initial_and_runtime_jumps_clamp_to_valid_items() {
        let mut initial = VirtualScroll::new(usize::MAX, 20).with_len(10);
        assert_eq!(
            initial.anchor_index, 9,
            "builder anchor must track the last valid item"
        );
        assert_eq!(
            initial.scroll_offset_from_anchor(),
            0.,
            "builder anchor and offset agree"
        );
        let mut harness =
            TestHarness::create_with_size(test_property_set(), initial.prepare(), (100, 200));
        let id = harness.root_id();
        drive_to_fixpoint(&mut harness, id, fixed_size_driver);
        harness
            .edit_root_widget(|mut scroll| {
                VirtualScroll::scroll_to(&mut scroll, usize::MAX);
                assert_eq!(scroll.widget.anchor_index, 9, "jump anchor is clamped");
                assert_eq!(
                    scroll.widget.scroll_offset_from_anchor(),
                    0.,
                    "jump offset matches anchor"
                );
            })
            .unwrap();
        drive_to_fixpoint(&mut harness, id, fixed_size_driver);
        assert_eq!(
            harness.root_widget().unwrap().range_in_viewport.end,
            10,
            "last item is visible"
        );
        harness
            .edit_root_widget(|mut scroll| {
                VirtualScroll::set_len(&mut scroll, 0);
                VirtualScroll::scroll_to(&mut scroll, usize::MAX);
                assert_eq!(
                    scroll.widget.anchor_index, 0,
                    "empty list has no nonzero anchor"
                );
                assert_eq!(
                    scroll.widget.scroll_offset_from_anchor(),
                    0.,
                    "empty list offset is zero"
                );
            })
            .unwrap();
        drive_to_fixpoint(&mut harness, id, fixed_size_driver);
        assert!(
            harness.root_widget().unwrap().range_in_viewport.is_empty(),
            "empty list has no visible items"
        );
    }

    #[test]
    fn changing_extent_estimates_preserves_the_actual_scroll_anchor() {
        use crate::layout::AsUnit;
        use crate::widgets::SizedBox;

        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            VirtualScroll::new(0, 1000).prepare(),
            (100, 200),
        );
        let id = harness.root_id();
        let driver = |action: VirtualScrollFetchAction,
                      mut scroll: WidgetMut<'_, VirtualScroll>| {
            let mut fetch = VirtualScroll::begin_fetch(&mut scroll, &action).unwrap();
            for index in action.removed() {
                assert!(fetch.remove_child(index));
            }
            for index in action.added() {
                let height = if index < 15 { 50. } else { 120. };
                assert!(
                    fetch.add_child(
                        index,
                        SizedBox::empty()
                            .size(50.px(), height.px())
                            .prepare()
                            .erased()
                    )
                );
            }
        };
        drive_to_fixpoint(&mut harness, id, driver);
        harness
            .edit_root_widget(|mut scroll| {
                assert!(VirtualScroll::scroll_by(&mut scroll, 1025.));
            })
            .unwrap();
        drive_to_fixpoint(&mut harness, id, driver);
        harness
            .edit_root_widget(|mut scroll| {
                let (index, offset) = scroll.widget.current_anchor();
                assert_eq!(index, 20);
                assert!((offset - 25.).abs() < 1e-8);
                VirtualScroll::invalidate_extents(&mut scroll);
            })
            .unwrap();
        drive_to_fixpoint(&mut harness, id, driver);
        harness
            .edit_root_widget(|mut scroll| {
                let (index, offset) = scroll.widget.current_anchor();
                assert_eq!(index, 20);
                assert!((offset - 25.).abs() < 1e-8);
            })
            .unwrap();
    }

    #[test]
    fn ending_scroll_animation_resnaps_existing_child() {
        use crate::kurbo::Point;

        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            VirtualScroll::new(0, 10).prepare(),
            (200, 200),
        );
        let id = harness.root_id();
        drive_to_fixpoint(&mut harness, id, fixed_size_driver);
        let child_id = harness.root_widget().unwrap().items[&0].id();
        let origin = |harness: &TestHarness<VirtualScroll>| {
            harness
                .get_widget_with_id(child_id)
                .unwrap()
                .ctx()
                .window_transform()
                * Point::ORIGIN
        };
        let original = origin(&harness);
        harness
            .edit_root_widget(|mut scroll| {
                VirtualScroll::set_scrolling(&mut scroll, true);
                assert!(VirtualScroll::scroll_by(&mut scroll, 0.25));
            })
            .unwrap();
        assert_eq!(origin(&harness), original - Vec2::new(0., 0.25));
        harness
            .edit_root_widget(|mut scroll| {
                VirtualScroll::set_scrolling(&mut scroll, false);
            })
            .unwrap();
        assert_eq!(origin(&harness), original);
    }

    #[test]
    fn range_differences_match_set_membership() {
        for a_start in 0..8 {
            for a_end in a_start..8 {
                for b_start in 0..8 {
                    for b_end in b_start..8 {
                        let a = a_start..a_end;
                        let b = b_start..b_end;
                        let expected = a.clone().filter(|idx| !b.contains(idx)).collect::<Vec<_>>();
                        assert_eq!(range_difference(&a, &b).collect::<Vec<_>>(), expected);
                    }
                }
            }
        }
        assert_eq!(
            range_difference(
                &(usize::MAX - 3..usize::MAX),
                &(usize::MAX - 2..usize::MAX - 1)
            )
            .collect::<Vec<_>>(),
            vec![usize::MAX - 3, usize::MAX - 1],
        );
    }

    #[test]
    fn fetch_identity_rejects_wrong_widget_and_replay() {
        let make_harness = || {
            TestHarness::create_with_size(
                test_property_set(),
                VirtualScroll::new(0, 100).prepare(),
                (100, 200),
            )
        };
        let mut first = make_harness();
        let mut second = make_harness();
        let action = next_fetch(&mut first);
        let other = next_fetch(&mut second);
        assert_eq!(action.old_active(), other.old_active());
        assert_eq!(action.target(), other.target());

        second
            .edit_root_widget(|mut scroll| {
                assert!(VirtualScroll::begin_fetch(&mut scroll, &action).is_none());
                assert!(scroll.widget.is_pending_fetch(&other));
            })
            .unwrap();

        first
            .edit_root_widget(|mut scroll| {
                assert!(VirtualScroll::begin_fetch(&mut scroll, &other).is_none());
                let mut fetch = VirtualScroll::begin_fetch(&mut scroll, &action).unwrap();
                for idx in action.added() {
                    assert!(fetch.add_child(idx, Label::new(idx.to_string()).prepare().erased()));
                }
                assert!(VirtualScroll::begin_fetch(&mut scroll, &action).is_none());
            })
            .unwrap();

        // A distinct widget may have the exact same ranges, but its own request
        // must still be usable after rejection of the foreign one.
        second
            .edit_root_widget(|mut scroll| {
                let mut fetch = VirtualScroll::begin_fetch(&mut scroll, &other).unwrap();
                for idx in other.added() {
                    assert!(fetch.add_child(idx, Label::new(idx.to_string()).prepare().erased()));
                }
            })
            .unwrap();
    }

    #[test]
    fn pending_fetch_survives_redraws_and_is_invalidated_by_shrink() {
        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            VirtualScroll::new(0, 100).prepare(),
            (100, 200),
        );
        let old = next_fetch(&mut harness);
        assert!(old.target().end > 1);
        for _ in 0..20 {
            harness
                .edit_root_widget(|mut scroll| scroll.ctx.request_render())
                .unwrap();
            let _ = harness.redraw();
            assert!(harness.root_widget().unwrap().is_pending_fetch(&old));
        }
        harness
            .edit_root_widget(|mut scroll| {
                VirtualScroll::set_len(&mut scroll, 1);
                assert!(!scroll.widget.is_pending_fetch(&old));
                assert!(VirtualScroll::begin_fetch(&mut scroll, &old).is_none());
            })
            .unwrap();
        let new = next_fetch(&mut harness);
        assert_eq!(new.target(), &(0..1));
        harness
            .edit_root_widget(|mut scroll| {
                let mut fetch = VirtualScroll::begin_fetch(&mut scroll, &new).unwrap();
                assert!(!fetch.add_child(1, Label::new("invalid").prepare().erased()));
                assert!(fetch.add_child(0, Label::new("valid").prepare().erased()));
                assert!(!fetch.remove_child(0));
            })
            .unwrap();
        assert_eq!(harness.root_widget().unwrap().len(), 1);

        harness
            .edit_root_widget(|mut scroll| {
                VirtualScroll::set_len(&mut scroll, 100);
                assert!(VirtualScroll::begin_fetch(&mut scroll, &old).is_none());
            })
            .unwrap();
        let grown = next_fetch(&mut harness);
        harness
            .edit_root_widget(|mut scroll| {
                assert!(VirtualScroll::begin_fetch(&mut scroll, &old).is_none());
                assert!(scroll.widget.is_pending_fetch(&grown));
                let mut fetch = VirtualScroll::begin_fetch(&mut scroll, &grown).unwrap();
                for idx in grown.added() {
                    assert!(fetch.add_child(idx, Label::new(idx.to_string()).prepare().erased()));
                }
            })
            .unwrap();
    }

    #[test]
    fn delayed_driver_can_revisit_stashed_items_and_duplicate_add_preserves_identity() {
        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            VirtualScroll::new(0, 1000).prepare(),
            (100, 200),
        );
        let id = harness.root_id();
        drive_to_fixpoint(&mut harness, id, |action, mut scroll| {
            let mut fetch = VirtualScroll::begin_fetch(&mut scroll, &action).unwrap();
            for idx in action.removed() {
                assert!(fetch.remove_child(idx));
            }
            for idx in action.added() {
                assert!(fetch.add_child(idx, Label::new(idx.to_string()).prepare().erased()));
                if idx == 0 {
                    let original = fetch.scroll.widget.items[&idx].id();
                    assert!(!fetch.add_child(idx, Label::new("duplicate").prepare().erased()));
                    assert_eq!(fetch.scroll.widget.items[&idx].id(), original);
                    assert!(!fetch.remove_child(idx));
                }
            }
        });
        let original = harness.root_widget().unwrap().items[&0].id();

        // Leave the fetch request queued while the viewport jumps away and
        // back. The original item remains registered across both layouts.
        harness
            .edit_root_widget(|mut scroll| VirtualScroll::scroll_to(&mut scroll, 500))
            .unwrap();
        assert!(
            harness
                .get_widget_with_id(original)
                .unwrap()
                .ctx()
                .is_stashed()
        );
        harness
            .edit_root_widget(|mut scroll| VirtualScroll::scroll_to(&mut scroll, 0))
            .unwrap();
        assert!(
            !harness
                .get_widget_with_id(original)
                .unwrap()
                .ctx()
                .is_stashed()
        );
        assert_eq!(harness.root_widget().unwrap().items[&0].id(), original);
    }

    #[test]
    fn sensible_driver() {
        let widget = VirtualScroll::new(0, usize::MAX).prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (100, 200));
        let virtual_scroll_id = harness.root_id();
        fn driver(action: VirtualScrollFetchAction, mut scroll: WidgetMut<'_, VirtualScroll>) {
            let mut fetch = VirtualScroll::begin_fetch(&mut scroll, &action).unwrap();
            for idx in action.removed() {
                let _ = fetch.remove_child(idx);
            }
            for idx in action.added() {
                assert!(
                    fetch.add_child(
                        idx,
                        NewWidget::new(
                            Label::new(format!("{idx}")).with_style(StyleProperty::FontSize(30.)),
                        )
                        .erased(),
                    )
                );
            }
        }

        drive_to_fixpoint(&mut harness, virtual_scroll_id, driver);
        assert_render_snapshot!(harness, "virtual_scroll_basic");
        harness
            .edit_root_widget(|mut scroll| {
                VirtualScroll::scroll_to(&mut scroll, 100);
            })
            .unwrap();
        drive_to_fixpoint(&mut harness, virtual_scroll_id, driver);
        assert_render_snapshot!(harness, "virtual_scroll_moved");
        harness.mouse_move_to(virtual_scroll_id).unwrap();
        harness.mouse_wheel(Vec2 { x: 0., y: 25. });
        drive_to_fixpoint(&mut harness, virtual_scroll_id, driver);
        assert_render_snapshot!(harness, "virtual_scroll_scrolled");
    }

    // The driver intentionally violates the contract; verify that sparse items
    // do not prevent the scroll implementation from converging.
    fn sparse_driver_gaps(step: usize, remainder: usize, target: usize) {
        let widget = VirtualScroll::new(0, usize::MAX).prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (100, 200));
        let virtual_scroll_id = harness.root_id();
        let driver = |action: VirtualScrollFetchAction,
                      mut scroll: WidgetMut<'_, VirtualScroll>| {
            let mut fetch = VirtualScroll::begin_fetch(&mut scroll, &action).unwrap();
            for idx in action.removed() {
                let _ = fetch.remove_child(idx);
            }
            for idx in action.added() {
                if idx % step == remainder {
                    assert!(
                        fetch.add_child(
                            idx,
                            NewWidget::new(
                                Label::new(format!("{idx}"))
                                    .with_style(StyleProperty::FontSize(30.)),
                            )
                            .erased(),
                        )
                    );
                }
            }
        };
        drive_to_fixpoint(&mut harness, virtual_scroll_id, driver);
        harness
            .edit_root_widget(|mut scroll| VirtualScroll::scroll_to(&mut scroll, target))
            .unwrap();
        drive_to_fixpoint(&mut harness, virtual_scroll_id, driver);
        harness.mouse_move_to(virtual_scroll_id).unwrap();
        harness.mouse_wheel(Vec2 { x: 0., y: 200. });
        drive_to_fixpoint(&mut harness, virtual_scroll_id, driver);
    }

    #[test]
    fn small_gaps() {
        sparse_driver_gaps(2, 0, 100);
    }

    #[test]
    fn big_gaps() {
        sparse_driver_gaps(100, 1, 200);
    }

    #[test]
    /// We shouldn't panic or loop if the driver is very poorly written (doesn't set `valid_range` correctly)
    /// Note that we don't test rendering in this case, because this is a driver which breaks our contract.
    fn degenerate_driver() {
        let widget = VirtualScroll::new(0, usize::MAX).prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (100, 200));
        let virtual_scroll_id = harness.root_id();
        fn driver(action: VirtualScrollFetchAction, mut scroll: WidgetMut<'_, VirtualScroll>) {
            let mut fetch = VirtualScroll::begin_fetch(&mut scroll, &action).unwrap();
            for idx in action.removed() {
                let _ = fetch.remove_child(idx);
            }
            for idx in action.added() {
                if idx < 5 {
                    assert!(
                        fetch.add_child(
                            idx,
                            NewWidget::new(
                                Label::new(format!("{idx}"))
                                    .with_style(StyleProperty::FontSize(30.)),
                            )
                            .erased(),
                        )
                    );
                }
            }
        }

        drive_to_fixpoint(&mut harness, virtual_scroll_id, driver);
        harness
            .edit_root_widget(|mut scroll| {
                VirtualScroll::scroll_to(&mut scroll, 200);
            })
            .unwrap();
        drive_to_fixpoint(&mut harness, virtual_scroll_id, driver);
        harness.mouse_move_to(virtual_scroll_id).unwrap();
        harness.mouse_wheel(Vec2 { x: 0., y: 200. });
        drive_to_fixpoint(&mut harness, virtual_scroll_id, driver);
    }

    #[test]
    /// If there's a maximum to the valid range, we should behave in a sensible way.
    fn limited_down() {
        const MAX: usize = 10;
        let widget = VirtualScroll::new(100, MAX).prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (100, 200));
        let virtual_scroll_id = harness.root_id();
        fn driver(action: VirtualScrollFetchAction, mut scroll: WidgetMut<'_, VirtualScroll>) {
            let mut fetch = VirtualScroll::begin_fetch(&mut scroll, &action).unwrap();
            for idx in action.removed() {
                let _ = fetch.remove_child(idx);
            }
            for idx in action.added() {
                assert!(
                    idx < MAX,
                    "Virtual Scroll controller should never request an invalid id. Requested {idx}"
                );
                assert!(
                    fetch.add_child(
                        idx,
                        NewWidget::new(
                            Label::new(idx.to_string()).with_style(StyleProperty::FontSize(30.)),
                        )
                        .erased(),
                    )
                );
            }
        }

        drive_to_fixpoint(&mut harness, virtual_scroll_id, driver);
        // We are scrolled down as far as possible. This is hard to write a convincing code test for,
        // so validate it with code.
        assert_render_snapshot!(harness, "virtual_scroll_limited_up_bottom");
        let (original_range, original_scroll) = {
            let widget = harness.root_widget().unwrap();
            tracing::debug!(widget = ?(&*widget));
            assert_eq!(
                widget.range_in_viewport.end, MAX,
                "Virtual Scroll controller should fix the last item to the end of the viewport"
            );
            (
                widget.active_range.clone(),
                widget.virtual_list.scroll_offset(),
            )
        };

        harness.mouse_move_to(virtual_scroll_id).unwrap();
        harness.mouse_wheel(Vec2 { x: 0., y: 40. });
        drive_to_fixpoint(&mut harness, virtual_scroll_id, driver);
        {
            let widget = harness.root_widget().unwrap();
            tracing::debug!(widget = ?(&*widget));
            assert_ne!(widget.range_in_viewport.end, MAX);
            assert_eq!(widget.active_range, original_range);
        }
        harness.mouse_wheel(Vec2 { x: 0., y: -45. });
        drive_to_fixpoint(&mut harness, virtual_scroll_id, driver);

        assert_render_snapshot!(harness, "virtual_scroll_limited_up_bottom");
        {
            let widget = harness.root_widget().unwrap();
            assert_eq!(widget.range_in_viewport.end, MAX);
            assert_eq!(
                widget.virtual_list.scroll_offset(),
                original_scroll,
                "Should be scrolled as far as possible (which is the same as we originally were)"
            );
        }
    }

    fn drive_to_fixpoint(
        harness: &mut TestHarness<VirtualScroll>,
        virtual_scroll_id: WidgetId,
        mut f: impl FnMut(VirtualScrollFetchAction, WidgetMut<'_, VirtualScroll>),
    ) {
        let mut iteration = 0;
        let mut old_active = None;
        loop {
            iteration += 1;
            if iteration > 1000 {
                panic!("Took too long to reach fixpoint");
            }
            let Some((action, id)) = harness.pop_action::<VirtualScrollAction>().unwrap() else {
                break;
            };
            let VirtualScrollAction::Fetch(action) = action else {
                continue;
            };
            assert_eq!(
                id, virtual_scroll_id,
                "Only widget in tree should give action"
            );
            if let Some(old_active) = old_active.take() {
                assert_eq!(action.old_active(), &old_active);
            }
            old_active = Some(action.target().clone());
            assert_ne!(
                action.target(),
                action.old_active(),
                "Shouldn't have sent an update if the target hasn't changed"
            );

            harness
                .edit_root_widget(|scroll| {
                    f(action, scroll);
                })
                .unwrap();
        }
    }
}
