// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::ops::Range;

use masonry::widgets;

use crate::core::{MessageCtx, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker};
use crate::{Pod, ViewCtx, WidgetView};

pub use widgets::{ScrollDirection, ViewportRange};

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
    viewport: ViewportRange,
    extent_revision: u64,
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
/// Requests invalidated by a change in length are discarded; the child component is
/// called only for indices within the current length.
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
        viewport: ViewportRange::FULL,
        extent_revision: 0,
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
        viewport: ViewportRange::FULL,
        extent_revision: 0,
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

    /// Sets the viewport interval in scroll-direction coordinates.
    #[must_use = "Use the configured view."]
    pub const fn viewport(mut self, viewport: ViewportRange) -> Self {
        self.viewport = viewport;
        self
    }

    /// Changes the revision of the underlying item collection.
    ///
    /// Change this value when replacing or reordering data to discard remembered
    /// size estimates for unmaterialized items. Retained items are remeasured.
    #[must_use = "Use the configured view."]
    pub const fn extent_revision(mut self, revision: u64) -> Self {
        self.extent_revision = revision;
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
            viewport: self.viewport,
            extent_revision: self.extent_revision,
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
            if idx < self.len
                && let Some(widget) = widgets::VirtualScroll::child_mut(element, idx)
            {
                self.rebuild_child(idx, child, ctx, widget, app_state);
            }
        }
    }

    fn rebuild_child(
        &self,
        idx: usize,
        child: &mut private::ChildState<ChildrenViews, ChildrenViews::ViewState>,
        ctx: &mut ViewCtx,
        widget: masonry::core::WidgetMut<'_, ChildrenViews::Widget>,
        app_state: &mut State,
    ) {
        let next = (self.func)(app_state, idx);
        with_child_path(ctx, idx, child.generation, |ctx| {
            next.rebuild(&child.view, &mut child.state, ctx, widget, app_state);
            child.view = next;
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
                .with_viewport(self.viewport)
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
        let len_changed = self.len != prev.len;
        if len_changed {
            widgets::VirtualScroll::set_len(&mut element, self.len);
        }

        if self.anchor_index != prev.anchor_index
            && let Some(idx) = self.anchor_index
        {
            widgets::VirtualScroll::scroll_to(&mut element, idx);
        }

        if self.extent_revision != prev.extent_revision {
            widgets::VirtualScroll::invalidate_extents(&mut element);
        }

        if self.viewport != prev.viewport {
            widgets::VirtualScroll::set_viewport(&mut element, self.viewport);
        }

        let direction_changed = self.direction != prev.direction;
        if direction_changed {
            widgets::VirtualScroll::set_direction(&mut element, self.direction);
        }

        let scrolling_changed = self.scrolling != prev.scrolling;
        if scrolling_changed {
            widgets::VirtualScroll::set_scrolling(&mut element, self.scrolling);
        }

        if let Some(action) = view_state.pending_action.take()
            && let Some(mut fetch) = widgets::VirtualScroll::begin_fetch(&mut element, &action)
        {
            for idx in action.removed() {
                if let Some(mut child) = view_state.children.remove(&idx) {
                    with_child_path(ctx, idx, child.generation, |ctx| {
                        if let Some(child_element) = fetch.child_mut(idx) {
                            child.view.teardown(&mut child.state, ctx, child_element);
                        } else {
                            ctx.remove_action_sources_under_current_path();
                        }
                    });
                }
                let _ = fetch.remove_child(idx);
            }
            // Preserve index order for both new and retained child callbacks.
            for idx in action.target().clone() {
                if let Some(child) = view_state.children.get_mut(&idx) {
                    if let Some(widget) = fetch.child_mut(idx) {
                        self.rebuild_child(idx, child, ctx, widget, app_state);
                    }
                    continue;
                }
                let view = (self.func)(app_state, idx);
                let generation = view_state.next_generation;
                view_state.next_generation = generation.wrapping_add(1);
                let (child, state) =
                    with_child_path(ctx, idx, generation, |ctx| view.build(ctx, app_state));
                if !fetch.add_child(idx, child.new_widget.erased()) {
                    with_child_path(ctx, idx, generation, |ctx| {
                        ctx.remove_action_sources_under_current_path();
                    });
                    continue;
                }
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
            if idx >= self.len {
                return MessageResult::Stale;
            }
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
                    if !element.widget.is_pending_fetch(&action) {
                        return MessageResult::Stale;
                    }
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use masonry::app::RenderRootSignal;

    use crate::core::{DynMessage, Environment};
    use crate::test_support::{render_root, view_context};
    use crate::view::spinner;

    use super::*;

    fn next_fetch(signals: &mpsc::Receiver<RenderRootSignal>) -> widgets::VirtualScrollFetchAction {
        signals
            .try_iter()
            .find_map(|signal| {
                let RenderRootSignal::Action(action, _) = signal else {
                    return None;
                };
                match *action.downcast::<widgets::VirtualScrollAction>().ok()? {
                    widgets::VirtualScrollAction::Fetch(action) => Some(action),
                    widgets::VirtualScrollAction::Scroll(_) => None,
                }
            })
            .unwrap()
    }

    fn message(action: widgets::VirtualScrollFetchAction) -> MessageCtx {
        MessageCtx::new(
            Environment::default(),
            Vec::new(),
            DynMessage::new(widgets::VirtualScrollAction::Fetch(action)),
        )
    }

    #[test]
    fn growing_and_jumping_in_one_rebuild_uses_new_length() {
        let mut ctx = view_context();
        let component = |_: &mut (), _| spinner();
        let original = virtual_scroll::<_, (), _, _>(1, component);
        let grown = virtual_scroll::<_, (), _, _>(100, component).jump_to(Some(80));
        let (pod, mut state) = original.build(&mut ctx, &mut ());
        let (mut root, receive) = render_root(pod.new_widget, (100, 200));
        let first = next_fetch(&receive);
        assert_eq!(
            first.target(),
            &(0..1),
            "initial request covers the single item"
        );
        root.edit_base_layer(
            |mut element: masonry::core::WidgetMut<'_, widgets::VirtualScroll>| {
                assert!(
                    matches!(
                        original.message(
                            &mut state,
                            &mut message(first),
                            element.reborrow_mut(),
                            &mut ()
                        ),
                        MessageResult::RequestRebuild
                    ),
                    "initial request is accepted"
                );
                original.rebuild(&original, &mut state, &mut ctx, element, &mut ());
            },
        )
        .unwrap();
        root.edit_base_layer(|element| {
            grown.rebuild(&original, &mut state, &mut ctx, element, &mut ());
        })
        .unwrap();
        let request = next_fetch(&receive);
        assert!(
            request.target().contains(&80),
            "the new target must be materialized: {:?}",
            request.target()
        );
        assert!(
            request.target().start > 0,
            "jump must leave the initial range"
        );
    }

    #[test]
    fn shrinking_between_message_and_rebuild_never_calls_out_of_range_component() {
        let mut ctx = view_context();
        let mut calls = Vec::new();
        let component = |calls: &mut Vec<usize>, idx| {
            calls.push(idx);
            spinner()
        };
        let original = virtual_scroll::<_, (), _, _>(100, component);
        let smaller = virtual_scroll::<_, (), _, _>(1, component);
        let (pod, mut state) = original.build(&mut ctx, &mut calls);
        let (mut root, receive) = render_root(pod.new_widget, (100, 200));
        let old = next_fetch(&receive);
        assert!(old.target().end > 1);

        root.edit_base_layer(|element| {
            assert!(matches!(
                original.message(&mut state, &mut message(old.clone()), element, &mut calls),
                MessageResult::RequestRebuild,
            ));
        })
        .unwrap();

        root.edit_base_layer(|element| {
            smaller.rebuild(&original, &mut state, &mut ctx, element, &mut calls);
        })
        .unwrap();
        assert!(calls.is_empty());
        assert!(state.children.is_empty());
        let current = next_fetch(&receive);
        assert_eq!(current.target(), &(0..1));

        root.edit_base_layer(
            |mut element: masonry::core::WidgetMut<'_, widgets::VirtualScroll>| {
                assert!(matches!(
                    smaller.message(
                        &mut state,
                        &mut message(current.clone()),
                        element.reborrow_mut(),
                        &mut calls
                    ),
                    MessageResult::RequestRebuild,
                ));
                assert!(matches!(
                    smaller.message(
                        &mut state,
                        &mut message(old),
                        element.reborrow_mut(),
                        &mut calls
                    ),
                    MessageResult::Stale,
                ));
                smaller.rebuild(&smaller, &mut state, &mut ctx, element, &mut calls);
            },
        )
        .unwrap();
        assert_eq!(calls, [0]);
        assert_eq!(state.children.len(), 1);

        root.edit_base_layer(|element| {
            assert!(matches!(
                smaller.message(&mut state, &mut message(current), element, &mut calls),
                MessageResult::Stale,
            ));
        })
        .unwrap();
    }
}
