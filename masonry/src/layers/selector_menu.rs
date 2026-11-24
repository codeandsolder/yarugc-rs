// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use accesskit::Role;
use tracing::{Span, trace_span};

use crate::core::{
    AccessEvent, ChildrenIds, CollectionWidget, EventCtx, Layer, LayoutCtx, MeasureCtx, NewWidget,
    NoAction, PointerButton, PointerButtonEvent, PointerEvent, PropertiesMut, PropertiesRef,
    RegisterCtx, TextEvent, Update, UpdateCtx, UsesProperty, Widget, WidgetId, WidgetMut,
    WidgetPod,
};
use crate::kurbo::{Axis, Point, Size};
use crate::layout::{AsUnit, LayoutSize, LenDef, LenReq, Length, SizeDef};
use crate::properties::Gap;
use crate::widgets::{SelectionChanged, Selector};

/// A [`Layer`] representing a list of options for a [`Selector`] widget.
pub struct SelectorMenu {
    creator: WidgetId,
    children: Vec<WidgetPod<dyn Widget>>,
}

// --- MARK: BUILDERS
impl SelectorMenu {
    /// Creates a new empty menu.
    #[must_use = "Use the returned value."]
    pub fn new(creator: WidgetId) -> Self {
        Self {
            creator,
            children: Vec::new(),
        }
    }

    /// Builder-style method to add a child widget.
    #[must_use = "Use the returned value."]
    pub fn with(mut self, child: NewWidget<impl Widget + ?Sized>) -> Self {
        let child = child.erased().into_pod();
        self.children.push(child);
        self
    }
}

impl SelectorMenu {
    fn dismiss(&self, ctx: &mut EventCtx<'_>) {
        let id = ctx.widget_id();
        ctx.remove_layer(id);
        ctx.mutate_later::<Selector>(self.creator, move |selector| {
            if selector.widget.menu_layer_id == Some(id) {
                selector.widget.menu_layer_id = None;
            }
        });
    }
}

// --- MARK: COLLECTIONWIDGET
impl CollectionWidget<()> for SelectorMenu {
    collection_accessors!(child => child.id());

    /// Appends a child widget to the collection.
    fn add(
        this: &mut WidgetMut<'_, Self>,
        child: NewWidget<impl Widget + ?Sized>,
        _params: impl Into<()>,
    ) {
        let child = child.erased().into_pod();
        this.widget.children.push(child);
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
        let child = child.erased().into_pod();
        this.widget.children.insert(idx, child);
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
        let child = child.erased().into_pod();
        let old_child = std::mem::replace(&mut this.widget.children[idx], child);
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
        this.ctx.remove_child(child);
        Ok(())
    }

    /// Removes all children.
    fn clear(this: &mut WidgetMut<'_, Self>) {
        for child in this.widget.children.drain(..) {
            this.ctx.remove_child(child);
        }
    }
}

impl UsesProperty<Gap> for SelectorMenu {}

impl Widget for SelectorMenu {
    type Action = NoAction;

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        match event {
            PointerEvent::Up(PointerButtonEvent {
                button: None | Some(PointerButton::Primary),
                ..
            }) => {
                let self_id = ctx.widget_id();

                let Some(index) = self
                    .children
                    .iter()
                    .position(|child| ctx.target_in_child(child))
                else {
                    return;
                };

                // FIXME - This might be subject to TOCTOU.
                // Fix this once associated layers are implemented.
                ctx.mutate_later::<Selector>(self.creator, move |mut selector| {
                    if selector.widget.menu_layer_id != Some(self_id) {
                        return;
                    }
                    let Some(selected_content) = selector.widget.options.get(index).cloned() else {
                        return;
                    };

                    selector
                        .ctx
                        .submit_action::<SelectionChanged>(SelectionChanged {
                            selected_content,
                            index,
                        });
                    selector.ctx.remove_layer(self_id);
                    selector.widget.menu_layer_id = None;
                    Selector::select_option(&mut selector, index);
                });
            }
            _ => (),
        }
    }

    // TODO - Handle text selection
    // (e.g. if options are A, B and C, typing "C" should set selected option to 2)
    fn on_text_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &TextEvent,
    ) {
        match event {
            TextEvent::WindowFocusChange(false) => {
                self.dismiss(ctx);
            }
            _ => (),
        }
    }

    // TODO
    fn on_access_event(
        &mut self,
        _ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        _event: &AccessEvent,
    ) {
    }

    fn on_anim_frame(
        &mut self,
        _ctx: &mut UpdateCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        _interval: u64,
    ) {
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        // FIXME - This might be subject to TOCTOU.
        // Fix this once associated layers are implemented.
        if matches!(event, Update::WidgetAdded) {
            let id = ctx.widget_id();
            ctx.mutate_later::<Selector>(self.creator, move |selector| {
                selector.widget.menu_layer_id = Some(id);
            });
        }
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

        let (len_req, min_result) = match len_req {
            LenReq::Min | LenReq::Max => (len_req, Length::ZERO),
            LenReq::Fit(space) => (LenReq::Min, space),
        };

        let auto_length = len_req.into();
        let context_size = LayoutSize::maybe(axis.cross(), cross_length);

        let mut length = Length::ZERO;
        for child in &mut self.children {
            let child_length =
                ctx.compute_length(child, auto_length, context_size, axis, cross_length);
            match axis {
                Axis::Horizontal => length = length.max(child_length),
                Axis::Vertical => length = length.saturating_add(child_length),
            }
        }

        if axis == Axis::Vertical {
            let gap_count = crate::layout_count(self.children.len().saturating_sub(1));
            let total_gap = gap_count * gap_length;
            length = length.saturating_add(total_gap.px());
        }

        min_result.max(length)
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, props: &PropertiesRef<'_>, size: Size) {
        let cache = ctx.property_cache();
        let gap = props.get::<Gap>(cache);

        if self.children.is_empty() {
            ctx.clear_baselines();
            return;
        }
        let gap_count = crate::layout_count(self.children.len().saturating_sub(1));
        let gap_length = gap.gap.get();
        let total_child_vertical_space = gap_length.mul_add(-gap_count, size.height);
        let child_vertical_space =
            total_child_vertical_space / crate::layout_count(self.children.len());

        let width_def = LenDef::FitContent(size.width.px());
        let height_def = LenDef::FitContent(child_vertical_space.max(0.).px());
        let auto_size = SizeDef::new(width_def, height_def);
        let context_size = size.into();

        let mut y_offset = 0.0;
        for child in &mut self.children {
            let child_size = ctx.compute_size(child, auto_size, context_size);
            ctx.run_layout(child, child_size);
            ctx.place_child(child, Point::new(0.0, y_offset));

            y_offset += child_size.height + gap_length;
        }

        let mut placements = self
            .children
            .iter()
            .filter_map(|child| ctx.child_placement(child));
        if let Some(first) = placements.next() {
            let last = placements.next_back().unwrap_or(first);
            ctx.set_baselines(first.parent_baselines().0, last.parent_baselines().1);
        } else {
            ctx.clear_baselines();
        }
    }

    fn accessibility_role(&self) -> Role {
        Role::GenericContainer
    }

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        for child in &mut self.children {
            ctx.register_child(child);
        }
    }

    fn children_ids(&self) -> ChildrenIds {
        self.children.iter().map(WidgetPod::id).collect()
    }

    fn as_layer(&mut self) -> Option<&mut dyn Layer> {
        Some(self)
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("SelectorMenu", id = id.trace())
    }
}

// --- MARK: IMPL LAYER
impl Layer for SelectorMenu {
    fn capture_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        let remove_this = match event {
            PointerEvent::Down(PointerButtonEvent { state, .. }) => {
                let local_pos = ctx.local_position(state.position);

                !ctx.border_box().contains(local_pos)
            }
            PointerEvent::Cancel(..) => true,
            _ => false,
        };

        if remove_this {
            // FIXME - This might be subject to TOCTOU.
            // Fix this once associated layers are implemented.
            self.dismiss(ctx);
        }
    }
}
