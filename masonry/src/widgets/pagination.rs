// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;

use accesskit::{AriaCurrent, Node, Role};
use tracing::{Span, trace_span};

use crate::core::{
    AccessCtx, ActionCtx, ChildrenIds, ErasedAction, FromDynWidget, LayoutCtx, MeasureCtx,
    MutateCtx, NewWidget, PaintCtx, PointerButton, PropertiesMut, PropertiesRef, RegisterCtx,
    Update, UpdateCtx, Widget, WidgetId, WidgetMut, WidgetPod,
};
use crate::imaging::Painter;
use crate::kurbo::{Axis, Point, Size};
use crate::layout::{LayoutSize, LenReq, Length, SizeDef};
use crate::widgets::{Button, ButtonPress};

/// Pagination for navigating between different page numbers.
pub struct Pagination {
    /// Total number of pages.
    page_count: usize,
    /// The 0-based index of the currently active page.
    active_page: usize,

    /// The maximum number of buttons shown for the first pages.
    buttons_start: u8,
    /// The maximum number of buttons shown for the last pages.
    buttons_end: u8,
    /// The maximum number of buttons shown in total.
    buttons_total: u8,

    /// All the page buttons.
    buttons: Vec<PageButton>,
}

/// One button for a specific page.
struct PageButton {
    widget: WidgetPod<Button>,
    disabled: bool,
    stashed: bool,
    page_idx: usize,
}

/// --- MARK: BUILDERS
impl Pagination {
    /// Creates a new [`Pagination`] with the given `page_count`.
    #[must_use = "Use the returned value."]
    pub const fn new(page_count: usize) -> Self {
        Self {
            page_count,
            active_page: 0,
            buttons_start: 1,
            buttons_end: 1,
            buttons_total: 9,
            buttons: Vec::new(),
        }
    }

    /// Returns the currently active zero-based page, or zero if there are no pages.
    #[must_use = "Use the current page index."]
    pub const fn current_page(&self) -> usize {
        self.active_page
    }

    /// Sets the currently active page.
    ///
    /// This is a 0-based index and defaults to `0`.
    ///
    /// It is clamped to the total page count.
    #[must_use = "Use the returned value."]
    pub fn active_page(mut self, active_page: usize) -> Self {
        self.active_page = if self.page_count == 0 {
            0
        } else {
            active_page.min(self.page_count - 1)
        };
        self
    }

    /// Sets the maximum number of buttons to always show for the first pages.
    ///
    /// This defaults to `1`.
    #[must_use = "Use the returned value."]
    pub const fn buttons_start(mut self, buttons_start: u8) -> Self {
        self.buttons_start = buttons_start;
        self
    }

    /// Sets the maximum number of buttons to always show for the last pages.
    ///
    /// This defaults to `1`.
    #[must_use = "Use the returned value."]
    pub const fn buttons_end(mut self, buttons_end: u8) -> Self {
        self.buttons_end = buttons_end;
        self
    }

    /// Sets the maximum number of buttons shown in total.
    ///
    /// The effective button limit also depends on the total page count.
    ///
    /// This defaults to `9`.
    #[must_use = "Use the returned value."]
    pub const fn buttons_total(mut self, buttons_total: u8) -> Self {
        self.buttons_total = buttons_total;
        self
    }
}

// --- MARK: WIDGETMUT
impl Pagination {
    /// Sets the total page count.
    ///
    /// The active page index is clamped to this new total.
    pub fn set_page_count(this: &mut WidgetMut<'_, Self>, page_count: usize) {
        if this.widget.page_count == page_count {
            return;
        }
        this.widget.page_count = page_count;
        this.widget.active_page = if page_count == 0 {
            0
        } else {
            this.widget.active_page.min(page_count - 1)
        };

        this.widget.sync_buttons(&mut this.ctx);
    }

    /// Sets the currently active page.
    ///
    /// This is a 0-based index.
    ///
    /// It is clamped to the total page count.
    pub fn set_active_page(this: &mut WidgetMut<'_, Self>, active_page: usize) {
        let active_page = active_page.min(this.widget.page_count.saturating_sub(1));
        if this.widget.active_page == active_page {
            return;
        }
        this.widget.active_page = active_page;

        this.widget.sync_buttons(&mut this.ctx);
    }

    /// Sets the maximum number of buttons to always show for the first pages.
    pub fn set_buttons_start(this: &mut WidgetMut<'_, Self>, buttons_start: u8) {
        if this.widget.buttons_start == buttons_start {
            return;
        }
        this.widget.buttons_start = buttons_start;

        this.widget.sync_buttons(&mut this.ctx);
    }

    /// Sets the maximum number of buttons to always show for the last pages.
    pub fn set_buttons_end(this: &mut WidgetMut<'_, Self>, buttons_end: u8) {
        if this.widget.buttons_end == buttons_end {
            return;
        }
        this.widget.buttons_end = buttons_end;

        this.widget.sync_buttons(&mut this.ctx);
    }

    /// Sets the maximum number of buttons shown in total.
    ///
    /// The effective button limit also depends on the total page count.
    pub fn set_buttons_total(this: &mut WidgetMut<'_, Self>, buttons_total: u8) {
        if this.widget.buttons_total == buttons_total {
            return;
        }
        this.widget.buttons_total = buttons_total;

        this.widget.sync_buttons(&mut this.ctx);
    }
}

// --- MARK: METHODS
impl Pagination {
    /// Returns the list of page indices that the pagination buttons are associated with.
    fn derive_button_pages(&self) -> Vec<usize> {
        let button_count = self.page_count.min(self.buttons_total as usize);
        let mut button_pages = Vec::with_capacity(button_count);

        // There are at least button_count distinct pages within button_count - 1
        // steps of the active page, even when it lies at an endpoint.
        let nearby = (1..button_count).flat_map(|offset| {
            [
                self.active_page.checked_add(offset),
                self.active_page.checked_sub(offset),
            ]
            .into_iter()
            .flatten()
        });
        let first = 0..self.page_count.min(usize::from(self.buttons_start));
        let last = (self
            .page_count
            .saturating_sub(usize::from(self.buttons_end))..self.page_count)
            .rev();
        for idx in core::iter::once(self.active_page)
            .chain(first)
            .chain(last)
            .chain(nearby)
        {
            if button_pages.len() == button_count {
                break;
            }
            if idx < self.page_count && !button_pages.contains(&idx) {
                button_pages.push(idx);
            }
        }

        // Make sure the chosen page indices are in ascending order.
        button_pages.sort_unstable();

        button_pages
    }

    /// Synchronizes all the child button widgets with the provided `button_pages`.
    fn sync_buttons<Ctx: SyncCtx>(&mut self, ctx: &mut Ctx) {
        let pages = self.derive_button_pages();
        let old_ids = self.children_ids();
        let mut old = core::mem::take(&mut self.buttons);
        let capacity = usize::from(self.buttons_total);
        let mut visibility_changed = false;

        // Widget identity follows the logical page, not its position in the
        // displayed window. A retained press or queued accessibility action
        // must never be redirected to a different page.
        for page_idx in pages {
            let current = page_idx == self.active_page;
            let button = if let Some(index) =
                old.iter().position(|button| button.page_idx == page_idx)
            {
                let mut button = old.swap_remove(index);
                if button.disabled != current {
                    button.disabled = current;
                    ctx.mutate_child_later(&button.widget, move |mut button| {
                        button.ctx.set_disabled(current);
                        Button::set_aria_current(&mut button, current.then_some(AriaCurrent::Page));
                    });
                }
                if button.stashed {
                    button.stashed = false;
                    ctx.set_stashed(&button.widget, false);
                    visibility_changed = true;
                }
                button
            } else {
                let mut button = Button::with_text((page_idx + 1).to_string());
                if current {
                    button = button.with_aria_current(AriaCurrent::Page);
                }
                PageButton {
                    widget: NewWidget::new(button).disabled(current).into_pod(),
                    disabled: current,
                    stashed: false,
                    page_idx,
                }
            };
            self.buttons.push(button);
        }

        // Keep dormant page identities only while spare capacity remains.
        // Never reassign one to another page.
        old.sort_unstable_by_key(|button| button.page_idx);
        for mut button in old {
            if self.buttons.len() == capacity {
                ctx.remove_child(button.widget);
            } else {
                if !button.stashed {
                    button.stashed = true;
                    ctx.set_stashed(&button.widget, true);
                    visibility_changed = true;
                }
                self.buttons.push(button);
            }
        }
        if old_ids != self.children_ids() {
            ctx.children_changed();
        } else if visibility_changed {
            ctx.request_layout();
        }
    }
}

/// Page change action with the new page index.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PageChanged(pub usize);

// --- MARK: IMPL WIDGET
impl Widget for Pagination {
    type Action = PageChanged;

    fn on_action(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        action: &ErasedAction,
        source: WidgetId,
    ) {
        if let Some(press) = action.downcast_ref::<ButtonPress>()
            && let Some(button) = self
                .buttons
                .iter()
                .find(|button| button.widget.id() == source)
        {
            ctx.set_handled();
            if matches!(press.button, None | Some(PointerButton::Primary))
                && !button.stashed
                && !button.disabled
            {
                self.active_page = button.page_idx;
                self.sync_buttons(ctx);
                ctx.submit_action::<Self::Action>(PageChanged(self.active_page));
            }
        }
    }

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        for button in &mut self.buttons {
            ctx.register_child(&mut button.widget);
        }
    }

    fn property_changed(&mut self, _ctx: &mut UpdateCtx<'_>, _property_type: TypeId) {}

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        match event {
            Update::WidgetAdded => {
                self.sync_buttons(ctx);
            }
            _ => (),
        }
    }

    fn measure(
        &mut self,
        ctx: &mut MeasureCtx<'_>,
        _props: &PropertiesRef<'_>,
        axis: Axis,
        len_req: LenReq,
        cross_length: Option<Length>,
    ) -> Length {
        let mut length = Length::ZERO;

        let context_size = LayoutSize::maybe(axis.cross(), cross_length);

        for button in self.buttons.iter_mut().filter(|button| !button.stashed) {
            let auto_length = match axis {
                Axis::Horizontal => len_req.reduce(length),
                Axis::Vertical => len_req,
            }
            .into();
            let button_length = ctx.compute_length(
                &button.widget,
                auto_length,
                context_size,
                axis,
                cross_length,
            );
            length = match axis {
                Axis::Horizontal => length.saturating_add(button_length),
                Axis::Vertical => length.max(button_length),
            };
        }

        length
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, size: Size) {
        let mut used_width = 0.;
        for button in self.buttons.iter_mut().filter(|button| !button.stashed) {
            let space = Size::new((size.width - used_width).max(0.), size.height);
            let button_size = ctx.compute_size(&button.widget, SizeDef::fit(space), size.into());

            ctx.run_layout(&button.widget, button_size);

            let button_origin = Point::new(used_width, 0.);
            ctx.place_child(&button.widget, button_origin);

            used_width += button_size.width;
        }
    }

    fn paint(
        &mut self,
        _ctx: &mut PaintCtx<'_>,
        _props: &PropertiesRef<'_>,
        _painter: &mut Painter<'_>,
    ) {
    }

    fn accessibility_role(&self) -> Role {
        Role::Navigation
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        _node: &mut Node,
    ) {
        // The current page is announced on the corresponding button.
    }

    fn children_ids(&self) -> ChildrenIds {
        let mut ids = ChildrenIds::with_capacity(self.buttons.len());
        for button in &self.buttons {
            ids.push(button.widget.id());
        }
        ids
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("Pagination", id = id.trace())
    }
}

// --- MARK: SYNCCTX

/// Collections of context methods required to sync buttons.
trait SyncCtx {
    fn request_layout(&mut self);
    fn children_changed(&mut self);
    fn set_stashed(&mut self, child: &WidgetPod<impl Widget + ?Sized>, stashed: bool);
    fn remove_child(&mut self, child: WidgetPod<impl Widget + ?Sized>);
    fn mutate_child_later<W: Widget + FromDynWidget + ?Sized>(
        &mut self,
        child: &WidgetPod<W>,
        f: impl FnOnce(WidgetMut<'_, W>) + Send + 'static,
    );
}

// These contexts expose identical mutation operations. Keep the adapter in one place.
macro_rules! impl_sync_ctx {
    ($($ctx:ident),+ $(,)?) => {
        $(
            impl SyncCtx for $ctx<'_> {
                fn request_layout(&mut self) {
                    self.request_layout();
                }

                fn children_changed(&mut self) {
                    self.children_changed();
                }

                fn set_stashed(&mut self, child: &WidgetPod<impl Widget + ?Sized>, stashed: bool) {
                    self.set_stashed(child, stashed);
                }

                fn remove_child(&mut self, child: WidgetPod<impl Widget + ?Sized>) {
                    self.remove_child(child);
                }

                fn mutate_child_later<W: Widget + FromDynWidget + ?Sized>(
                    &mut self,
                    child: &WidgetPod<W>,
                    f: impl FnOnce(WidgetMut<'_, W>) + Send + 'static,
                ) {
                    self.mutate_child_later(child, f);
                }
            }
        )+
    };
}

impl_sync_ctx!(MutateCtx, ActionCtx, UpdateCtx);

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_selection_matches_priority_order() {
        // Independent oracle: rank all pages by priority, then select the first N.
        for count in 0..=16 {
            for active in 0..=count {
                for total in 0..=8 {
                    for start in 0..=4 {
                        for end in 0..=4 {
                            let pg = Pagination::new(count)
                                .active_page(active)
                                .buttons_total(total)
                                .buttons_start(start)
                                .buttons_end(end);
                            let mut expected: Vec<_> = (0..count).collect();
                            expected.sort_unstable_by_key(|&idx| {
                                if idx == pg.active_page {
                                    (0, 0, false)
                                } else if idx < usize::from(start) {
                                    (1, idx, false)
                                } else if idx >= count.saturating_sub(usize::from(end)) {
                                    (2, count - idx, false)
                                } else {
                                    (3, idx.abs_diff(pg.active_page), idx < pg.active_page)
                                }
                            });
                            expected.truncate(usize::from(total));
                            expected.sort_unstable();
                            assert_eq!(pg.derive_button_pages(), expected);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn retained_buttons_relayout_when_visible_count_changes() {
        use crate::core::WidgetTag;
        use crate::properties::types::{CrossAxisAlignment, MainAxisAlignment};
        use crate::testing::TestHarness;
        use crate::theme::test_property_set;
        use crate::widgets::Flex;

        let tag = WidgetTag::unique();
        let pagination = NewWidget::new(Pagination::new(3)).with_tag(tag);
        let root = Flex::row()
            .with_fixed(pagination)
            .main_axis_alignment(MainAxisAlignment::Start)
            .cross_axis_alignment(CrossAxisAlignment::Start);
        let mut harness =
            TestHarness::create_with_size(test_property_set(), NewWidget::new(root), (400, 80));
        let original = harness.get_widget(tag).unwrap();
        let width = original.ctx().border_box().width();
        let ids = original.inner().children_ids();
        let _ = harness.redraw();

        harness
            .edit_widget(tag, |mut widget| {
                Pagination::set_page_count(&mut widget, 1);
            })
            .unwrap();
        let _ = harness.redraw();
        let shrunk = harness.get_widget(tag).unwrap();
        assert!(shrunk.ctx().border_box().width() < width);
        assert_eq!(shrunk.inner().children_ids(), ids);
        assert_eq!(
            harness
                .access_node(shrunk.id())
                .unwrap()
                .child_ids()
                .count(),
            1
        );

        harness
            .edit_widget(tag, |mut widget| {
                Pagination::set_page_count(&mut widget, 3);
            })
            .unwrap();
        let _ = harness.redraw();
        let restored = harness.get_widget(tag).unwrap();
        assert_eq!(restored.ctx().border_box().width(), width);
        assert_eq!(restored.inner().children_ids(), ids);
        assert_eq!(
            harness
                .access_node(restored.id())
                .unwrap()
                .child_ids()
                .count(),
            3
        );
    }

    #[test]
    fn page_identity_and_accessibility_survive_window_changes() {
        use crate::testing::TestHarness;
        use crate::theme::test_property_set;
        use accesskit::{Action, ActionRequest, TreeId};

        let pagination = Pagination::new(10)
            .buttons_start(0)
            .buttons_end(0)
            .buttons_total(3)
            .active_page(2);
        let mut harness = TestHarness::create(test_property_set(), pagination.prepare());
        let root_id = harness.root_id();
        let ids = harness.root_widget().unwrap().inner().children_ids();
        let _ = harness.redraw();
        assert_eq!(
            harness.access_node(ids[1]).unwrap().data().aria_current(),
            Some(AriaCurrent::Page)
        );

        // A secondary click on another page does not navigate.
        harness.mouse_move_to(ids[2]).unwrap();
        harness.mouse_button_press(Some(PointerButton::Secondary));
        harness.mouse_button_release(Some(PointerButton::Secondary));
        assert!(harness.pop_action::<PageChanged>().unwrap().is_none());

        // Retain the press on page 1 while the visible window changes.
        harness.mouse_move_to(ids[0]).unwrap();
        harness.mouse_button_press(Some(PointerButton::Primary));
        harness
            .edit_root_widget(|mut widget| Pagination::set_active_page(&mut widget, 3))
            .unwrap();
        harness.mouse_button_release(Some(PointerButton::Primary));
        assert!(harness.pop_action::<PageChanged>().unwrap().is_none());
        let new_ids = harness.root_widget().unwrap().inner().children_ids();
        assert_eq!(new_ids[0], ids[1]);
        assert_eq!(new_ids[1], ids[2]);
        assert!(!new_ids.contains(&ids[0]));
        let _ = harness.redraw();
        assert!(harness.access_node(ids[0]).is_none());
        assert_eq!(
            harness.access_node(ids[1]).unwrap().data().aria_current(),
            None
        );
        assert_eq!(
            harness.access_node(ids[2]).unwrap().data().aria_current(),
            Some(AriaCurrent::Page)
        );
        let click = |id: WidgetId| ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: id.into(),
            data: None,
        };
        harness.process_access_event(click(ids[0]));
        assert!(harness.pop_action::<PageChanged>().unwrap().is_none());
        harness.process_access_event(click(ids[1]));
        assert_eq!(
            harness.pop_action::<PageChanged>().unwrap(),
            Some((PageChanged(2), root_id))
        );
        assert_eq!(harness.root_widget().unwrap().current_page(), 2);
    }

    #[test]
    fn page_selection_handles_usize_max() {
        let pg = Pagination::new(usize::MAX)
            .active_page(usize::MAX)
            .buttons_total(5);
        assert_eq!(
            pg.derive_button_pages(),
            vec![
                0,
                usize::MAX - 4,
                usize::MAX - 3,
                usize::MAX - 2,
                usize::MAX - 1
            ]
        );
    }

    #[test]
    fn button_pages() {
        let pg = Pagination::new(0);
        assert_eq!(pg.derive_button_pages(), vec![]);

        let pg = Pagination::new(1);
        assert_eq!(pg.derive_button_pages(), vec![0]);

        let pg = Pagination::new(2);
        assert_eq!(pg.derive_button_pages(), vec![0, 1]);

        let pg = Pagination::new(10)
            .buttons_total(0)
            .buttons_start(1)
            .buttons_end(1)
            .active_page(5);
        assert_eq!(pg.derive_button_pages(), vec![]);

        let pg = Pagination::new(10)
            .buttons_total(1)
            .buttons_start(1)
            .buttons_end(1)
            .active_page(5);
        assert_eq!(pg.derive_button_pages(), vec![5]);

        let pg = Pagination::new(10)
            .buttons_total(2)
            .buttons_start(1)
            .buttons_end(1)
            .active_page(5);
        assert_eq!(pg.derive_button_pages(), vec![0, 5]);

        let pg = Pagination::new(10)
            .buttons_total(3)
            .buttons_start(1)
            .buttons_end(1)
            .active_page(5);
        assert_eq!(pg.derive_button_pages(), vec![0, 5, 9]);

        let pg = Pagination::new(10)
            .buttons_total(5)
            .buttons_start(1)
            .buttons_end(1)
            .active_page(5);
        assert_eq!(pg.derive_button_pages(), vec![0, 4, 5, 6, 9]);

        let pg = Pagination::new(10)
            .buttons_total(5)
            .buttons_start(2)
            .buttons_end(2)
            .active_page(5);
        assert_eq!(pg.derive_button_pages(), vec![0, 1, 5, 8, 9]);

        let pg = Pagination::new(10)
            .buttons_total(5)
            .buttons_start(0)
            .buttons_end(2)
            .active_page(3);
        assert_eq!(pg.derive_button_pages(), vec![2, 3, 4, 8, 9]);

        let pg = Pagination::new(10)
            .buttons_total(5)
            .buttons_start(1)
            .buttons_end(1)
            .active_page(9);
        assert_eq!(pg.derive_button_pages(), vec![0, 6, 7, 8, 9]);

        let pg = Pagination::new(10)
            .buttons_total(20)
            .buttons_start(1)
            .buttons_end(1)
            .active_page(9);
        assert_eq!(pg.derive_button_pages(), vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }
}
