// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;

use accesskit::{Node, Role};
use include_doc_path::include_doc_path;
use tracing::{Span, trace_span};

use crate::core::{
    AccessCtx, ChildrenIds, CollectionWidget, LayoutCtx, MeasureCtx, NewWidget, NoAction, PaintCtx,
    PropertiesRef, RegisterCtx, UpdateCtx, Widget, WidgetId, WidgetMut, WidgetPod,
};
use crate::imaging::Painter;
use crate::kurbo::{Axis, Point, Size};
use crate::layout::{LayoutSize, LenReq, Length, SizeDef};

// TODO - Rename "active" widget to "visible" widget?
// Active already means something else.

/// A widget that displays only one of its children at a time.
///
/// This is useful for switching between multiple views while keeping
/// state loaded, such as in a tab stack.
///
/// The indexed stack acts as a simple container around the active child.
/// If there is no active child, it acts like a leaf node with no content.
#[doc = concat!(
    "![Indexed stack element showing only the fourth element in its children](",
    include_doc_path!("screenshots/indexed_stack_builder_new_widget.png"),
    ")",
)]
#[derive(Default)]
pub struct IndexedStack {
    children: Vec<WidgetPod<dyn Widget>>,
    // Note: active_child must be 0 if there are no children
    active_child: usize,
}

// --- MARK: BUILDERS
impl IndexedStack {
    /// Creates a new stack with no children.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder-style method to add a child widget.
    pub fn with(mut self, child: NewWidget<impl Widget + ?Sized>) -> Self {
        self.children.push(child.erased().into_pod());
        self
    }

    /// Builder-style method to set the active child.
    ///
    /// Index must be a valid index into the stack's children, or 0
    /// if there are no children.
    pub fn with_active_child(mut self, idx: usize) -> Self {
        if self.children.is_empty() {
            assert!(
                idx == 0,
                "Called set_active on empty IndexedStack with non-zero index {idx}"
            );
        } else {
            assert!(
                idx < self.children.len(),
                "Called set_active with invalid index {idx}"
            );
        }

        self.active_child = idx;
        self
    }
}

// --- MARK: METHODS
impl IndexedStack {
    /// Returns the index of the currently active child.
    pub fn active_child(&self) -> usize {
        self.active_child
    }
}

// --- MARK: WIDGETMUT
impl IndexedStack {
    /// Sets the active child, accepting index zero for an empty stack.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid index, leaving the active child unchanged.
    pub fn set_active_child(
        this: &mut WidgetMut<'_, Self>,
        idx: usize,
    ) -> Result<(), crate::core::CollectionError> {
        if idx != 0 || !this.widget.children.is_empty() {
            crate::check_slot(idx, this.widget.children.len())?;
        }
        this.widget.active_child = idx;
        this.ctx.request_layout();
        Ok(())
    }
}

// --- MARK: COLLECTIONWIDGET
impl CollectionWidget<()> for IndexedStack {
    collection_accessors!(child => child.id());

    /// Appends a child widget to the collection.
    fn add(
        this: &mut WidgetMut<'_, Self>,
        child: NewWidget<impl Widget + ?Sized>,
        _params: impl Into<()>,
    ) {
        this.widget.children.push(child.erased().into_pod());
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
        _params: impl Into<()>,
    ) -> Result<(), crate::core::CollectionError> {
        crate::check_insert_slot(idx, this.widget.children.len())?;
        let had_children = !this.widget.children.is_empty();
        this.widget.children.insert(idx, child.erased().into_pod());
        if had_children && this.widget.active_child >= idx {
            // adjust index to keep the same widget active
            this.widget.active_child += 1;
        }
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
        _params: impl Into<()>,
    ) -> Result<(), crate::core::CollectionError> {
        crate::check_slot(idx, this.widget.children.len())?;
        let old_child =
            std::mem::replace(&mut this.widget.children[idx], child.erased().into_pod());
        this.ctx.remove_child(old_child);
        Ok(())
    }

    /// Not applicable.
    fn set_params(
        this: &mut WidgetMut<'_, Self>,
        idx: usize,
        _params: impl Into<()>,
    ) -> Result<(), crate::core::CollectionError> {
        crate::check_slot(idx, this.widget.children.len())
    }

    /// Swaps the index of two children.
    ///
    /// # Errors
    ///
    /// Returns an error if `a` or `b` are out of bounds.
    fn swap(
        this: &mut WidgetMut<'_, Self>,
        a: usize,
        b: usize,
    ) -> Result<(), crate::core::CollectionError> {
        crate::swap_slots(&mut this.widget.children, a, b)?;
        if this.widget.active_child == a {
            this.widget.active_child = b;
        } else if this.widget.active_child == b {
            this.widget.active_child = a;
        }
        this.ctx.children_changed();
        Ok(())
    }

    /// Removes the child at the given index.
    ///
    /// If the active child is removed, the first child in the stack will be selected as active.
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
        if idx == this.widget.active_child {
            // This is valid even if we are removing the last child,
            // since `active_child` must be 0 in that case
            this.widget.active_child = 0;
        } else if this.widget.active_child > idx {
            // correct the index to prevent the active element changing
            this.widget.active_child -= 1;
        }
        this.ctx.remove_child(child);
        Ok(())
    }

    /// Removes all children.
    fn clear(this: &mut WidgetMut<'_, Self>) {
        for child in this.widget.children.drain(..) {
            this.ctx.remove_child(child);
        }
        this.widget.active_child = 0; // 0 is valid for the childrenless case
    }
}

// --- MARK: IMPL WIDGET
impl Widget for IndexedStack {
    type Action = NoAction;

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        for child in self.children.iter_mut() {
            ctx.register_child(child);
        }
    }

    fn property_changed(&mut self, _ctx: &mut UpdateCtx<'_>, _property_type: TypeId) {}

    fn measure(
        &mut self,
        ctx: &mut MeasureCtx<'_>,
        _props: &PropertiesRef<'_>,
        axis: Axis,
        len_req: LenReq,
        cross_length: Option<Length>,
    ) -> Length {
        if !self.children.is_empty() {
            let auto_length = len_req.into();
            let context_size = LayoutSize::maybe(axis.cross(), cross_length);

            ctx.compute_length(
                &self.children[self.active_child],
                auto_length,
                context_size,
                axis,
                cross_length,
            )
        } else {
            Length::ZERO
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, size: Size) {
        // There's nothing to lay out if we don't have any children
        if self.children.is_empty() {
            return;
        }

        // TODO: move set_stashed to a different layout pass when possible
        for (idx, child) in self.children.iter_mut().enumerate() {
            ctx.set_stashed(child, idx != self.active_child);
        }

        let child_size = ctx.compute_size(
            &self.children[self.active_child],
            SizeDef::fit(size),
            size.into(),
        );
        ctx.run_layout(&self.children[self.active_child], child_size);

        let child_origin = Point::ORIGIN;
        ctx.place_child(&self.children[self.active_child], child_origin);

        ctx.derive_baselines(&self.children[self.active_child]);
    }

    fn paint(
        &mut self,
        _ctx: &mut PaintCtx<'_>,
        _props: &PropertiesRef<'_>,
        _painter: &mut Painter<'_>,
    ) {
    }

    fn accessibility_role(&self) -> Role {
        Role::GenericContainer
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        _node: &mut Node,
    ) {
    }

    fn children_ids(&self) -> ChildrenIds {
        self.children.iter().map(WidgetPod::id).collect()
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("IndexedStack", id = id.trace())
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::properties::Dimensions;
    use crate::testing::{TestHarness, assert_render_snapshot};
    use crate::theme::test_property_set;
    use crate::widgets::Button;

    #[test]
    fn test_indexed_stack_basics() {
        let widget = IndexedStack::new().prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (50, 50));

        assert_render_snapshot!(harness, "indexed_stack_empty");

        harness.edit_root_widget(|mut stack| {
            IndexedStack::add(
                &mut stack,
                Button::with_text("A")
                    .prepare()
                    .with_props(Dimensions::STRETCH),
                (),
            );
        });
        assert_render_snapshot!(harness, "indexed_stack_single");

        harness.edit_root_widget(|mut stack| {
            IndexedStack::add(
                &mut stack,
                Button::with_text("B")
                    .prepare()
                    .with_props(Dimensions::STRETCH),
                (),
            );
            IndexedStack::add(
                &mut stack,
                Button::with_text("C")
                    .prepare()
                    .with_props(Dimensions::STRETCH),
                (),
            );
            IndexedStack::add(
                &mut stack,
                Button::with_text("D")
                    .prepare()
                    .with_props(Dimensions::STRETCH),
                (),
            );
        });
        assert_render_snapshot!(harness, "indexed_stack_single"); // the active child should not change

        harness.edit_root_widget(|mut stack| {
            IndexedStack::set_active_child(&mut stack, 2).unwrap();
        });
        assert_render_snapshot!(harness, "indexed_stack_many_2");
    }

    #[test]
    fn test_widget_removal_and_modification() {
        let widget = IndexedStack::new()
            .with(
                Button::with_text("A")
                    .prepare()
                    .with_props(Dimensions::STRETCH),
            )
            .with(
                Button::with_text("B")
                    .prepare()
                    .with_props(Dimensions::STRETCH),
            )
            .with(
                Button::with_text("C")
                    .prepare()
                    .with_props(Dimensions::STRETCH),
            )
            .with_active_child(1)
            .prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (50, 50));
        // Snapshot with the single widget.
        assert_render_snapshot!(harness, "indexed_stack_initial_builder");

        // Remove the first (inactive) widget
        harness.edit_root_widget(|mut stack| {
            IndexedStack::remove(&mut stack, 0).unwrap();
        });
        assert_render_snapshot!(harness, "indexed_stack_initial_builder"); // Should not be changed

        // Now remove the active widget
        harness.edit_root_widget(|mut stack| {
            IndexedStack::remove(&mut stack, 0).unwrap();
        });
        assert_render_snapshot!(harness, "indexed_stack_builder_removed_widget");

        // Add another widget at the end
        harness.edit_root_widget(|mut stack| {
            IndexedStack::add(
                &mut stack,
                Button::with_text("D")
                    .prepare()
                    .with_props(Dimensions::STRETCH),
                (),
            );
        });
        assert_render_snapshot!(harness, "indexed_stack_builder_removed_widget"); // Should not change

        // Make it active
        harness.edit_root_widget(|mut stack| {
            IndexedStack::set_active_child(&mut stack, 1).unwrap();
        });
        assert_render_snapshot!(harness, "indexed_stack_builder_new_widget");

        // Insert back the first two at the start
        harness.edit_root_widget(|mut stack| {
            IndexedStack::insert(
                &mut stack,
                0,
                Button::with_text("A")
                    .prepare()
                    .with_props(Dimensions::STRETCH),
                (),
            )
            .unwrap();
            IndexedStack::insert(
                &mut stack,
                1,
                Button::with_text("B")
                    .prepare()
                    .with_props(Dimensions::STRETCH),
                (),
            )
            .unwrap();
        });
        assert_render_snapshot!(harness, "indexed_stack_builder_new_widget"); // Should not change

        // Reset original active index
        harness.edit_root_widget(|mut stack| {
            IndexedStack::set_active_child(&mut stack, 1).unwrap();
        });
        assert_render_snapshot!(harness, "indexed_stack_initial_builder");

        // Change the active widget
        harness.edit_root_widget(|mut stack| {
            IndexedStack::set(
                &mut stack,
                1,
                Button::with_text("D")
                    .prepare()
                    .with_props(Dimensions::STRETCH),
                (),
            )
            .unwrap();
        });
        assert_render_snapshot!(harness, "indexed_stack_builder_new_widget");
    }
}
