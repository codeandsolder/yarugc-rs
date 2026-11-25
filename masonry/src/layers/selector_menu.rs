// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use accesskit::{Action, Node, Role};
use tracing::{Span, trace_span};

use crate::core::keyboard::NamedKey;
use crate::core::{
    AccessCtx, AccessEvent, ActionCtx, ChildrenIds, ComposeCtx, ErasedAction, EventCtx, Layer,
    LayoutCtx, MeasureCtx, NoAction, PointerButton, PointerButtonEvent, PointerEvent,
    PointerScrollEvent, PropertiesMut, PropertiesRef, RegisterCtx, Update, UpdateCtx, UsesProperty,
    Widget, WidgetId, WidgetMut, WidgetPod,
};
use crate::dpi::PhysicalPosition;
use crate::kurbo::{Axis, Point, Rect, Size, Vec2};
use crate::layout::{AsUnit, LayoutSize, LenDef, LenReq, Length, SizeDef};
use crate::properties::Gap;
use crate::widgets::{ScrollBar, ScrollBarMoved, Selector, SelectorItem};

/// A [`Layer`] representing a list of options for a [`Selector`] widget.
pub struct SelectorMenu {
    creator: WidgetId,
    children: Vec<WidgetPod<SelectorItem>>,
    scrollbar: WidgetPod<ScrollBar>,
    pressed: Option<usize>,
    highlighted: usize,
    row_bounds: Vec<Rect>,
    viewport_height: f64,
    content_height: f64,
    scroll: f64,
    reveal_highlight: bool,
}

impl SelectorMenu {
    pub(crate) fn new(creator: WidgetId, options: &[String], selected: usize) -> Self {
        Self {
            creator,
            children: options
                .iter()
                .enumerate()
                .map(|(index, option)| {
                    SelectorItem::new(option.clone())
                        .with_selection(index == selected)
                        .prepare()
                        .into_pod()
                })
                .collect(),
            scrollbar: ScrollBar::new(Axis::Vertical, Length::ZERO, Length::ZERO)
                .with_focus_owner(creator)
                .prepare()
                .into_pod(),
            pressed: None,
            highlighted: selected,
            row_bounds: Vec::new(),
            viewport_height: 0.0,
            content_height: 0.0,
            scroll: 0.0,
            reveal_highlight: true,
        }
    }

    pub(crate) fn active_option_id(&self) -> Option<WidgetId> {
        self.children.get(self.highlighted).map(WidgetPod::id)
    }

    fn max_scroll(&self) -> f64 {
        (self.content_height - self.viewport_height).max(0.0)
    }

    fn page_index(&self, forward: bool) -> usize {
        let Some(current) = self.row_bounds.get(self.highlighted) else {
            return self.highlighted;
        };
        let target = current.y0
            + if forward {
                self.viewport_height
            } else {
                -self.viewport_height
            };
        let index = self.row_bounds.partition_point(|rect| rect.y0 < target);
        if forward {
            index
                .max(self.highlighted.saturating_add(1))
                .min(self.children.len().saturating_sub(1))
        } else {
            index.min(self.highlighted.saturating_sub(1))
        }
    }

    fn scroll_to(&mut self, offset: f64) -> bool {
        if !offset.is_finite() {
            return false;
        }
        let next = offset.clamp(0.0, self.max_scroll());
        if next.to_bits() == self.scroll.to_bits() {
            return false;
        }
        self.scroll = next;
        true
    }

    fn reveal(&mut self, rect: Rect) -> bool {
        let offset = if rect.y0 < self.scroll || rect.height() > self.viewport_height {
            rect.y0
        } else if rect.y1 > self.scroll + self.viewport_height {
            rect.y1 - self.viewport_height
        } else {
            self.scroll
        };
        self.scroll_to(offset)
    }

    pub(crate) fn navigate(this: &mut WidgetMut<'_, Self>, key: NamedKey) {
        let last = this.widget.children.len().saturating_sub(1);
        let previous = this.widget.highlighted;
        let next = match key {
            NamedKey::ArrowDown => previous.saturating_add(1).min(last),
            NamedKey::ArrowUp => previous.saturating_sub(1),
            NamedKey::PageUp => this.widget.page_index(false),
            NamedKey::PageDown => this.widget.page_index(true),
            NamedKey::Home => 0,
            NamedKey::End => last,
            _ => return,
        };
        if previous == next {
            return;
        }
        this.widget.highlighted = next;
        for (index, highlighted) in [(previous, false), (next, true)] {
            if let Some(child) = this.widget.children.get(index)
                && let Some(mut item) = this.ctx.get_mut::<SelectorItem>(child.id())
            {
                SelectorItem::set_highlighted(&mut item, highlighted);
            }
        }
        if let Some(rect) = this.widget.row_bounds.get(next).copied() {
            if this.widget.reveal(rect) {
                this.ctx.request_compose();
            }
        } else {
            this.widget.reveal_highlight = true;
        }
        this.ctx.request_accessibility_update();
        let menu = this.id();
        let option = this.widget.active_option_id();
        this.ctx
            .mutate_later::<Selector>(this.widget.creator, move |mut selector| {
                Selector::set_active_option(&mut selector, menu, option);
            });
    }

    pub(crate) fn commit_highlight(this: &mut WidgetMut<'_, Self>) {
        let menu = this.id();
        let index = this.widget.highlighted;
        this.ctx
            .mutate_later::<Selector>(this.widget.creator, move |mut selector| {
                Selector::pick_from_menu(&mut selector, menu, index);
            });
    }

    fn dismiss(&self, ctx: &mut EventCtx<'_>) {
        let id = ctx.widget_id();
        ctx.mutate_later::<Selector>(self.creator, move |mut selector| {
            if selector.ctx.get_attached_layer::<Self>() == Some(id) {
                selector.ctx.remove_layer(id);
            }
        });
    }

    fn measure_rows(&mut self, ctx: &mut LayoutCtx<'_>, width: f64, height: f64, gap: f64) -> f64 {
        let size = Size::new(width, height);
        let auto_size = SizeDef::new(LenDef::Fixed(width.px()), LenDef::MaxContent);
        self.row_bounds.clear();
        self.row_bounds.reserve(self.children.len());
        let mut y_offset = 0.0;
        for child in &self.children {
            let child_size = ctx.compute_size(child, auto_size, size.into());
            self.row_bounds.push(Rect::from_origin_size(
                Point::new(0.0, y_offset),
                child_size,
            ));
            y_offset += child_size.height + gap;
        }
        (y_offset - gap).max(0.0)
    }

    fn scrollbar_progress(&self) -> f64 {
        let maximum = self.max_scroll();
        if maximum > 0.0 {
            self.scroll / maximum
        } else {
            0.0
        }
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
                if self.pressed.is_some() {
                    ctx.capture_pointer();
                }
                ctx.set_focus(self.creator);
                ctx.set_handled();
            }
            PointerEvent::Up(PointerButtonEvent {
                button: None | Some(PointerButton::Primary),
                state,
                ..
            }) => {
                if let Some(index) = self.pressed.take()
                    && ctx.is_pointer_capture_target()
                    && ctx.is_hovered()
                    && self
                        .children
                        .get(index)
                        .is_some_and(|child| ctx.hit_test_child(child, state.position))
                {
                    self.pick(ctx, index);
                }
                ctx.set_handled();
            }
            PointerEvent::Scroll(PointerScrollEvent { delta, .. }) => {
                let scale = ctx.scale_factor();
                let line = PhysicalPosition::new(0.0, 120.0 * scale);
                let page = PhysicalPosition::new(0.0, self.viewport_height * scale);
                let delta = delta.to_pixel_delta(line, page).to_logical::<f64>(scale).y;
                if self.scroll_to(self.scroll - delta) {
                    ctx.request_compose();
                    ctx.request_accessibility_update();
                }
                ctx.set_handled();
            }
            _ => {}
        }
    }

    fn on_action(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        action: &ErasedAction,
        source: WidgetId,
    ) {
        if source == self.scrollbar.id()
            && let Some(movement) = action.downcast_ref::<ScrollBarMoved>()
        {
            if self.scroll_to(movement.0 * self.max_scroll()) {
                ctx.request_compose();
                ctx.request_accessibility_update();
            }
            ctx.set_handled();
        }
    }

    fn on_access_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &AccessEvent,
    ) {
        match event.action {
            Action::Click => {
                if let Some(index) = self.target_index(ctx) {
                    self.pick(ctx, index);
                }
            }
            Action::ScrollUp | Action::ScrollDown => {
                let amount = if event.action == Action::ScrollUp {
                    -self.viewport_height
                } else {
                    self.viewport_height
                };
                if self.scroll_to(self.scroll + amount) {
                    ctx.request_compose();
                    ctx.request_accessibility_update();
                }
                ctx.set_handled();
            }
            _ => {}
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

        let gap_length = gap.gap.get();
        self.content_height = self.measure_rows(ctx, size.width, size.height, gap_length);
        let show_bar = self.content_height > size.height && size.height > 0.0 && size.width > 0.0;
        let scrollbar_width = if show_bar {
            let preferred = ctx
                .compute_size(&self.scrollbar, SizeDef::fit(size), size.into())
                .width;
            preferred.min(size.width / 2.0)
        } else {
            0.0
        };
        if show_bar {
            self.content_height =
                self.measure_rows(ctx, size.width - scrollbar_width, size.height, gap_length);
        }
        for (child, rect) in self.children.iter().zip(&self.row_bounds) {
            ctx.run_layout(child, rect.size());
            ctx.place_child(child, rect.origin());
        }
        if self.viewport_height.to_bits() != size.height.to_bits() {
            self.reveal_highlight = true;
        }
        self.viewport_height = size.height;
        ctx.set_clip_path(size.to_rect());
        self.scroll_to(self.scroll);
        if self.reveal_highlight {
            self.reveal_highlight = false;
            if let Some(rect) = self.row_bounds.get(self.highlighted).copied() {
                self.reveal(rect);
            }
        }
        ctx.set_stashed(&self.scrollbar, !show_bar);
        ScrollBar::synchronize_raw(
            ctx.get_raw_mut(&self.scrollbar),
            self.viewport_height,
            self.content_height,
            self.scrollbar_progress(),
        );
        if show_bar {
            ctx.run_layout(&self.scrollbar, Size::new(scrollbar_width, size.height));
            ctx.place_child(
                &self.scrollbar,
                Point::new(size.width - scrollbar_width, 0.0),
            );
        }
        ctx.clear_baselines();
    }

    fn compose(&mut self, ctx: &mut ComposeCtx<'_>) {
        for child in &self.children {
            ctx.set_child_scroll_translation(child, Vec2::new(0.0, -self.scroll));
        }
        ScrollBar::synchronize_raw(
            ctx.get_raw_mut(&self.scrollbar),
            self.viewport_height,
            self.content_height,
            self.scrollbar_progress(),
        );
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        if let Update::RequestPanToChild { child, rect } = event
            && *child != self.scrollbar.id()
        {
            // Compose rounds the row translation, so undo that exact translation.
            if self.reveal(*rect + Vec2::new(0.0, self.scroll.round())) {
                ctx.request_compose();
                ctx.request_accessibility_update();
            }
        }
    }

    fn accessibility_role(&self) -> Role {
        Role::ListBox
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        if let Some(item) = self.children.get(self.highlighted) {
            node.set_active_descendant(item.id().into());
        }
        node.set_clips_children();
        node.set_orientation(accesskit::Orientation::Vertical);
        let maximum = self.max_scroll();
        if maximum > 0.0 {
            node.set_scroll_y_min(0.0);
            node.set_scroll_y_max(maximum);
            node.set_scroll_y(self.scroll);
            if self.scroll > 0.0 {
                node.add_action(Action::ScrollUp);
            }
            if self.scroll < maximum {
                node.add_action(Action::ScrollDown);
            }
            node.add_child_action(Action::ScrollIntoView);
        }
    }

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        for child in &mut self.children {
            ctx.register_child(child);
        }
        ctx.register_child(&mut self.scrollbar);
    }

    fn children_ids(&self) -> ChildrenIds {
        self.children
            .iter()
            .map(WidgetPod::id)
            .chain(std::iter::once(self.scrollbar.id()))
            .collect()
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
