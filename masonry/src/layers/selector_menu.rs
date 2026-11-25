// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use accesskit::Role;
use tracing::{Span, trace_span};

use crate::core::{
    AccessEvent, ChildrenIds, EventCtx, Layer, LayoutCtx, MeasureCtx, NoAction, PointerButton,
    PointerButtonEvent, PointerEvent, PropertiesMut, PropertiesRef, RegisterCtx, UsesProperty,
    Widget, WidgetId, WidgetPod,
};
use crate::kurbo::{Axis, Point, Size};
use crate::layout::{AsUnit, LayoutSize, LenDef, LenReq, Length, SizeDef};
use crate::properties::Gap;
use crate::widgets::{Selector, SelectorItem};

/// A [`Layer`] representing a list of options for a [`Selector`] widget.
pub struct SelectorMenu {
    creator: WidgetId,
    children: Vec<WidgetPod<SelectorItem>>,
    pressed: Option<usize>,
}

impl SelectorMenu {
    pub(crate) fn new(creator: WidgetId, options: &[String]) -> Self {
        Self {
            creator,
            children: options
                .iter()
                .map(|option| SelectorItem::new(option.clone()).prepare().into_pod())
                .collect(),
            pressed: None,
        }
    }

    fn dismiss(&self, ctx: &mut EventCtx<'_>) {
        let id = ctx.widget_id();
        ctx.mutate_later::<Selector>(self.creator, move |mut selector| {
            if selector.ctx.get_attached_layer::<Self>() == Some(id) {
                selector.ctx.remove_layer(id);
            }
        });
    }

    fn target_index(&self, ctx: &EventCtx<'_>) -> Option<usize> {
        self.children
            .iter()
            .position(|child| ctx.target_in_child(child))
    }

    fn pick(&self, ctx: &mut EventCtx<'_>, index: usize) {
        let id = ctx.widget_id();
        ctx.set_focus(self.creator);
        ctx.set_handled();
        ctx.mutate_later::<Selector>(self.creator, move |mut selector| {
            Selector::pick_from_menu(&mut selector, id, index);
        });
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
            PointerEvent::Down(PointerButtonEvent {
                button: None | Some(PointerButton::Primary),
                ..
            }) => {
                self.pressed = self.target_index(ctx);
                ctx.set_handled();
            }
            PointerEvent::Up(PointerButtonEvent {
                button: None | Some(PointerButton::Primary),
                ..
            }) => {
                if let Some(index) = self.pressed.take()
                    && self.target_index(ctx) == Some(index)
                {
                    self.pick(ctx, index);
                }
                ctx.set_handled();
            }
            _ => {}
        }
    }

    fn on_access_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &AccessEvent,
    ) {
        if event.action == accesskit::Action::Click
            && let Some(index) = self.target_index(ctx)
        {
            self.pick(ctx, index);
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
        Role::Menu
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
        match event {
            PointerEvent::Down(PointerButtonEvent { state, .. }) => {
                if !ctx
                    .border_box()
                    .contains(ctx.local_position(state.position))
                {
                    self.pressed = None;
                    self.dismiss(ctx);
                }
            }
            PointerEvent::Up(PointerButtonEvent { state, .. }) => {
                if !ctx
                    .border_box()
                    .contains(ctx.local_position(state.position))
                {
                    self.pressed = None;
                }
            }
            PointerEvent::Cancel(..) => {
                self.pressed = None;
                self.dismiss(ctx);
            }
            _ => {}
        }
    }
}
