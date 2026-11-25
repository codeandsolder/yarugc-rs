// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::ops::Range;

use masonry::widgets;

use crate::core::{MessageCtx, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker};
use crate::{Pod, ViewCtx, WidgetView};

pub use widgets::ScrollDirection;

/// Default function-pointer handler for a virtual scroll without a custom scroll callback.
pub type ScrollHandler<State, Action> = fn(&mut State, Range<usize>) -> MessageResult<Action>;

/// The view type for [`virtual_scroll`].
///
/// See its documentation for details.
pub struct VirtualScroll<State, Action, ChildrenViews, F, G> {
    phantom: PhantomData<fn(State, ChildrenViews) -> Action>,
    func: F,
    anchor_index: Option<usize>,
    len: usize,
    start_at: f64,
    end_at: f64,
    direction: ScrollDirection,
    scrolling: bool,
    on_scroll: G,
}

/// A (vertical) virtual scrolling View, for Masonry's [`VirtualScroll`](widgets::VirtualScroll).
///
/// Virtual scrolling is a technique to improve performance when scrolling through long lists, by
/// only loading (and therefore laying out, drawing, processing for event handling), the items visible to the user.
///
/// The implementation has some caveats, which are discussed in the documentation of the [underlying widget](widgets::VirtualScroll).
///
/// Whenever this view is rebuilt, all of the loaded children are rebuild.
/// The child creation function is a "component" context, (alike to the usual `app_logic` functions), which means
/// that changing the app's state in this function will *not* cause a rebuild or rerunning of the app
/// logic (this avoids infinite loops).
/// It is correct for `func` to capture, if necessary.
/// However, it also has access to the app's state, so this is unlikely to be needed.
///
/// Arguments:
/// - `len` is the number of children which are supported.
/// - `func` is the component for this element's children.
///   It is provided with the app's state and the index of the child.
///
/// In rare circumstances, the index of the child could be outside of the requested valid range (this is
/// most likely to happen if the valid range changes due to something in `app_logic` updating it - e.g.
/// if a counter which decrements every time a parent component is called is used for the valid range).
/// As such, you should avoid panicking if the index is outside of a range you expect, and you are
/// changing the valid range. We expect this limitation to be lifted in the future.
///
/// For full details, see the documentation on the [view type](VirtualScroll).
pub fn virtual_scroll<State, Action, ChildrenViews, F>(
    len: usize,
    func: F,
) -> VirtualScroll<State, Action, ChildrenViews, F, ScrollHandler<State, Action>>
where
    ChildrenViews: WidgetView<State, Action>,
    F: Fn(&mut State, usize) -> ChildrenViews + 'static,
    State: 'static,
    Action: 'static,
{
    VirtualScroll {
        phantom: PhantomData,
        func,
        anchor_index: None,
        len,
        start_at: 0.,
        end_at: 1.,
        direction: ScrollDirection::TopToBottom,
        scrolling: false,
        on_scroll: private::do_nothing::<State, Action>,
    }
}

/// Component for a [`VirtualScroll`] with unlimited children.
///
/// Arguments:
/// - `func` is the component for this element's children.
///   It is provided with the app's state and the index of the child.
///
/// For full details, see the documentation on the [view type](VirtualScroll).
pub fn unlimited_virtual_scroll<State, Action, ChildrenViews, F>(
    func: F,
) -> VirtualScroll<State, Action, ChildrenViews, F, ScrollHandler<State, Action>>
where
    ChildrenViews: WidgetView<State, Action>,
    F: Fn(&mut State, usize) -> ChildrenViews + 'static,
    State: 'static,
    Action: 'static,
{
    VirtualScroll {
        phantom: PhantomData,
        func,
        anchor_index: None,
        len: usize::MAX,
        start_at: 0.,
        end_at: 1.,
        direction: ScrollDirection::TopToBottom,
        scrolling: false,
        on_scroll: private::do_nothing::<State, Action>,
    }
}

impl<State, Action, ChildrenViews, F, G> VirtualScroll<State, Action, ChildrenViews, F, G> {
    /// Jumps to the child with the specified index.
    ///
    /// Sets the top of the child to the start position of viewport.
    #[must_use = "Use the configured view."]
    pub const fn jump_to(mut self, anchor_index: Option<usize>) -> Self {
        self.anchor_index = anchor_index;
        self
    }

    /// Sets the points (as ratios of width) where the first item starts and
    /// the last item ends in the viewport.
    #[must_use = "Use the configured view."]
    pub const fn start_end(mut self, start_at: f64, end_at: f64) -> Self {
        self.start_at = start_at;
        self.end_at = end_at;
        self
    }

    /// Sets the direction of scrolling.
    #[must_use = "Use the configured view."]
    pub const fn direction(mut self, direction: ScrollDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Sets if the view is in the process of scrolling.
    ///
    /// Helps with animated scrolls where pixel-snapping and font-hinting should be turned off.
    #[must_use = "Use the configured view."]
    pub const fn scrolling(mut self, scrolling: bool) -> Self {
        self.scrolling = scrolling;
        self
    }

    /// Sets the scroll handler.
    ///
    /// A scroll handler enables saving the current index of the scroll view, among others.
    pub fn on_scroll<H>(self, on_scroll: H) -> VirtualScroll<State, Action, ChildrenViews, F, H> {
        VirtualScroll {
            phantom: self.phantom,
            func: self.func,
            anchor_index: self.anchor_index,
            len: self.len,
            start_at: self.start_at,
            end_at: self.end_at,
            direction: self.direction,
            scrolling: self.scrolling,
            on_scroll,
        }
    }
}

mod private {
    use std::{collections::BTreeMap, ops::Range};

    use super::{MessageResult, widgets};

    pub(super) const fn do_nothing<State: 'static, Action>(
        _: &mut State,
        _: Range<usize>,
    ) -> MessageResult<Action> {
        MessageResult::Nop
    }

    #[expect(
        unnameable_types,
        reason = "Not meaningful public API; required to be public due to design of View trait"
    )]
    pub struct VirtualScrollState<View, State> {
        pub(super) pending_action: Option<widgets::VirtualScrollFetchAction>,
        pub(super) children: BTreeMap<usize, ChildState<View, State>>,
        pub(super) next_generation: u64,
    }

    pub(super) struct ChildState<View, State> {
        pub(super) view: View,
        pub(super) state: State,
        pub(super) generation: u64,
    }
}

/// Create the view id used for child views.
const fn view_id_for_index(idx: usize) -> ViewId {
    ViewId::new(idx as _)
}

// An index identifies the slot; a separate generation identifies its incarnation.
fn with_child_path<R>(
    ctx: &mut ViewCtx,
    idx: usize,
    generation: u64,
    f: impl FnOnce(&mut ViewCtx) -> R,
) -> R {
    ctx.with_id(view_id_for_index(idx), |ctx| {
        ctx.with_id(ViewId::new(generation), f)
    })
}

impl<State, Action, ChildrenViews, F, G> VirtualScroll<State, Action, ChildrenViews, F, G>
where
    State: 'static,
    ChildrenViews: WidgetView<State, Action>,
    F: Fn(&mut State, usize) -> ChildrenViews,
{
    fn rebuild_loaded(
        &self,
        state: &mut private::VirtualScrollState<ChildrenViews, ChildrenViews::ViewState>,
        ctx: &mut ViewCtx,
        element: &mut masonry::core::WidgetMut<'_, widgets::VirtualScroll>,
        app_state: &mut State,
    ) {
        for (&idx, child) in &mut state.children {
            self.rebuild_child(idx, child, ctx, element, app_state);
        }
    }

    fn rebuild_child(
        &self,
        idx: usize,
        child: &mut private::ChildState<ChildrenViews, ChildrenViews::ViewState>,
        ctx: &mut ViewCtx,
        element: &mut masonry::core::WidgetMut<'_, widgets::VirtualScroll>,
        app_state: &mut State,
    ) {
        let next = (self.func)(app_state, idx);
        with_child_path(ctx, idx, child.generation, |ctx| {
            if let Some(widget) = widgets::VirtualScroll::child_mut(element, idx) {
                next.rebuild(&child.view, &mut child.state, ctx, widget, app_state);
                child.view = next;
            }
        });
    }
}

impl<State, Action, ChildrenViews, F, G> ViewMarker
    for VirtualScroll<State, Action, ChildrenViews, F, G>
{
}
impl<State, Action, ChildrenViews, F, G> View<State, Action, ViewCtx>
    for VirtualScroll<State, Action, ChildrenViews, F, G>
where
    State: 'static,
    Action: 'static,
    ChildrenViews: WidgetView<State, Action>,
    F: Fn(&mut State, usize) -> ChildrenViews + 'static,
    G: Fn(&mut State, Range<usize>) -> MessageResult<Action> + Send + Sync + 'static,
{
    type Element = Pod<widgets::VirtualScroll>;

    type ViewState = private::VirtualScrollState<ChildrenViews, ChildrenViews::ViewState>;

    fn build(&self, ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        // TODO: How does the anchor interact with Xilem?
        // Setting that seems like an imperative action?
        let pod = Pod::new(
            widgets::VirtualScroll::new(self.anchor_index.unwrap_or(0), self.len)
                .with_start_end(self.start_at, self.end_at)
                .with_direction(self.direction)
                .with_scrolling(self.scrolling),
        );
        ctx.record_action_source(pod.new_widget.id());
        (
            pod,
            private::VirtualScrollState {
                pending_action: None,
                children: BTreeMap::default(),
                next_generation: 0,
            },
        )
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        if self.anchor_index != prev.anchor_index
            && let Some(idx) = self.anchor_index
        {
            widgets::VirtualScroll::scroll_to(&mut element, idx);
        }

        let len_changed = self.len != prev.len;
        if len_changed {
            widgets::VirtualScroll::set_len(&mut element, self.len);
        }

        let start_at = self.start_at.to_bits() != prev.start_at.to_bits();
        if start_at {
            widgets::VirtualScroll::set_start(&mut element, self.start_at);
        }

        let end_at = self.end_at.to_bits() != prev.end_at.to_bits();
        if end_at {
            widgets::VirtualScroll::set_end(&mut element, self.end_at);
        }

        let direction_changed = self.direction != prev.direction;
        if direction_changed {
            widgets::VirtualScroll::set_direction(&mut element, self.direction);
        }

        let scrolling_changed = self.scrolling != prev.scrolling;
        if scrolling_changed {
            widgets::VirtualScroll::set_scrolling(&mut element, self.scrolling);
        }

        if let Some(action) = view_state.pending_action.take() {
            widgets::VirtualScroll::will_handle_action(&mut element, &action);
            for idx in action.old_active().clone() {
                if !action.target().contains(&idx) {
                    if let Some(mut child) = view_state.children.remove(&idx) {
                        with_child_path(ctx, idx, child.generation, |ctx| {
                            if let Some(child_element) =
                                widgets::VirtualScroll::child_mut(&mut element, idx)
                            {
                                child.view.teardown(&mut child.state, ctx, child_element);
                            } else {
                                ctx.remove_action_sources_under_current_path();
                            }
                        });
                    }
                    widgets::VirtualScroll::remove_child(&mut element, idx);
                }
            }
            // Preserve index order for both new and retained child callbacks.
            for idx in action.target().clone() {
                if let Some(child) = view_state.children.get_mut(&idx) {
                    self.rebuild_child(idx, child, ctx, &mut element, app_state);
                    continue;
                }
                let view = (self.func)(app_state, idx);
                let generation = view_state.next_generation;
                view_state.next_generation = generation.wrapping_add(1);
                let (child, state) =
                    with_child_path(ctx, idx, generation, |ctx| view.build(ctx, app_state));
                widgets::VirtualScroll::add_child(&mut element, idx, child.new_widget.erased());
                view_state.children.insert(
                    idx,
                    private::ChildState {
                        view,
                        state,
                        generation,
                    },
                );
            }
        } else {
            self.rebuild_loaded(view_state, ctx, &mut element, app_state);
        }
        debug_assert_eq!(
            element.widget.len(),
            view_state.children.len(),
            "VirtualScroll: Child added outside of the control of Xilem."
        );
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
    ) {
        for (&idx, child) in &mut view_state.children {
            with_child_path(ctx, idx, child.generation, |ctx| {
                if let Some(child_element) = widgets::VirtualScroll::child_mut(&mut element, idx) {
                    child.view.teardown(&mut child.state, ctx, child_element);
                } else {
                    ctx.remove_action_sources_under_current_path();
                }
            });
        }
        ctx.teardown_action_source(element.ctx.widget_id());
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageCtx,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        if let Some(first) = message.take_first() {
            let Ok(idx) = usize::try_from(first.routing_id()) else {
                return MessageResult::Stale;
            };
            let Some(target) = view_state.children.get_mut(&idx) else {
                return MessageResult::Stale;
            };
            if message.take_first() != Some(ViewId::new(target.generation)) {
                return MessageResult::Stale;
            }
            let Some(child) = widgets::VirtualScroll::child_mut(&mut element, idx) else {
                return MessageResult::Stale;
            };
            return target
                .view
                .message(&mut target.state, message, child, app_state);
        }
        if let Some(action) = message.take_message::<widgets::VirtualScrollAction>() {
            match *action {
                widgets::VirtualScrollAction::Fetch(action) => {
                    // TODO: We should be able to rebuild here (we have the element)
                    // but we currently can't make a `ViewCtx`
                    view_state.pending_action = Some(action);
                    // We need rebuild to be called now.
                    MessageResult::RequestRebuild
                }
                widgets::VirtualScrollAction::Scroll(action) => {
                    (self.on_scroll)(app_state, action.range_in_viewport().clone())
                }
            }
        } else {
            tracing::error!(?message, "Wrong message type in VirtualScroll::message");
            MessageResult::Stale
        }
    }
}
