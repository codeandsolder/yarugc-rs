// Copyright 2020 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

use std::{any::TypeId, ops::Range};

use accesskit::{Node, Role};
use tracing::{Span, trace_span};

use crate::core::keyboard::{Key, NamedKey};
use crate::core::{
    AccessCtx, AccessEvent, ActionCtx, ChildrenIds, ComposeCtx, ErasedAction, EventCtx,
    FromDynWidget, LayoutCtx, MeasureCtx, NewWidget, NoAction, PaintCtx, PointerEvent,
    PointerScrollEvent, PropertiesMut, PropertiesRef, Property, RegisterCtx, TextEvent, Update,
    UpdateCtx, UsesProperty, Widget, WidgetId, WidgetMut, WidgetPod,
};
use crate::dpi::{LogicalPosition, PhysicalPosition};
use crate::imaging::Painter;
use crate::kurbo::{Axis, Point, Rect, Size, Vec2};
use crate::layout::{AsUnit, LayoutSize, LenDef, LenReq, Length, SizeDef};
use crate::properties::AutoHideScrollBar;
use crate::widgets::{ScrollBar, ScrollBarMoved};

// TODO: make this configurable or move to theme
const SCROLLBAR_ANIM_OVER_MILLIS: f32 = 300.;
const VISIBILITY_TIMEOUT: u64 = 400_000_000;

// TODO - refactor - see https://github.com/linebender/xilem/issues/366
// TODO - rename "Portal" to "ScrollPortal"?
// TODO - Document which cases need request_layout, request_compose and request_render
// Conceptually, a Portal is a widget giving a restricted view of a child widget
// Imagine a very large widget, and a rect that represents the part of the widget we see

/// A scrolling container with scrollbars and a child widget.
///
/// ## Keyboard and accessibility
///
/// - Exposes an accessibility node with [`accesskit::Role::ScrollView`], including `scroll_x/y`
///   and their ranges.
/// - Handles `accesskit` scroll actions (`ScrollUp`/`ScrollDown`/`ScrollLeft`/`ScrollRight`) by
///   scrolling the viewport.
/// - When this widget is focused, it handles basic keyboard scrolling (arrow keys, PageUp/Down,
///   Home/End) *if the event wasn't already handled by a child*.
///
/// When nested inside another scrolling container, child scroll widgets should call
/// [`EventCtx::set_handled`](crate::core::EventCtx::set_handled) after scrolling to prevent
/// accidental double-scrolling due to event bubbling.
pub struct Portal<W: Widget + ?Sized> {
    child: WidgetPod<W>,
    content_size: Size,
    // TODO - differentiate between the "explicit" viewport pos determined
    // by user input, and the computed viewport pos that may change based
    // on re-layouts
    // TODO - rename
    viewport_pos: Point,
    // TODO - test how it looks like
    constraints: ViewportConstraints,
    scrollbar_horizontal: WidgetPod<ScrollBar>,
    scrollbar_horizontal_visible: bool,
    scrollbar_vertical: WidgetPod<ScrollBar>,
    scrollbar_vertical_visible: bool,
    nanos_since_last_pointer_move: Option<u64>,
}

#[derive(Default)]
struct ViewportConstraints {
    constrain_horizontal: bool,
    constrain_vertical: bool,
    must_fill: bool,
}

// --- MARK: BUILDERS
impl<W: Widget + ?Sized> Portal<W> {
    /// Creates a scrolling container the given child widget.
    #[must_use = "Use the returned value."]
    pub fn new(child: NewWidget<W>) -> Self {
        Self {
            child: child.into_pod(),
            content_size: Size::ZERO,
            viewport_pos: Point::ORIGIN,
            constraints: ViewportConstraints::default(),
            // TODO - remove (TODO: why?)
            scrollbar_horizontal: WidgetPod::new(ScrollBar::new(
                Axis::Horizontal,
                Length::ZERO,
                Length::ZERO,
            )),
            scrollbar_horizontal_visible: false,
            scrollbar_vertical: WidgetPod::new(ScrollBar::new(
                Axis::Vertical,
                Length::ZERO,
                Length::ZERO,
            )),
            scrollbar_vertical_visible: false,
            nanos_since_last_pointer_move: None,
        }
    }

    /// Builder-style method for constraining the child vertically.
    ///
    /// The default is `false`.
    ///
    /// This setting affects how a [`Portal`] lays out its child.
    ///
    /// - When it is `false` (the default), the child does not receive any upper
    ///   bound on its height. The child can be as tall as it wants,
    ///   and the viewport gets moved around to see all of it.
    /// - When it is `true`, the [`Portal`]'s height will be passed down as an upper bound
    ///   on the height of the child. There will be no vertical scrollbar and
    ///   the mouse wheel can't be used to vertically scroll either.
    #[must_use = "Use the returned value."]
    pub const fn constrain_vertical(mut self, constrain: bool) -> Self {
        self.constraints.constrain_vertical = constrain;
        self
    }

    /// Builder-style method for constraining the child horizontally.
    ///
    /// The default is `false`.
    ///
    /// This setting affects how a [`Portal`] lays out its child.
    ///
    /// - When it is `false` (the default), the child does not receive any upper
    ///   bound on its width. The child can be as wide as it wants,
    ///   and the viewport gets moved around to see all of it.
    /// - When it is `true`, the [`Portal`]'s width will be passed down as an upper bound
    ///   on the width of the child. There will be no horizontal scrollbar and
    ///   the mouse wheel can't be used to horizontally scroll either.
    #[must_use = "Use the returned value."]
    pub const fn constrain_horizontal(mut self, constrain: bool) -> Self {
        self.constraints.constrain_horizontal = constrain;
        self
    }

    /// Builder-style method to set whether the child must fill the view.
    ///
    /// If `true`, the child size is guaranteed to be at least the size of the portal.
    #[must_use = "Use the returned value."]
    pub const fn content_must_fill(mut self, must_fill: bool) -> Self {
        self.constraints.must_fill = must_fill;
        self
    }
}

pub(super) fn compute_pan_range(mut viewport: Range<f64>, target: Range<f64>) -> Range<f64> {
    // if either range contains the other, the viewport doesn't move
    if target.start <= viewport.start && viewport.end <= target.end {
        return viewport;
    }
    if viewport.start <= target.start && target.end <= viewport.end {
        return viewport;
    }

    // we compute the length that we need to "fit" in our viewport
    let target_width = f64::min(viewport.end - viewport.start, target.end - target.start);
    let viewport_width = viewport.end - viewport.start;

    // Because of the early returns, there are only two cases to consider: we need
    // to move the viewport "left" or "right"
    if viewport.start >= target.start {
        viewport.start = target.end - target_width;
        viewport.end = viewport.start + viewport_width;
    } else {
        viewport.end = target.start + target_width;
        viewport.start = viewport.end - viewport_width;
    }

    viewport
}

// --- MARK: METHODS
impl<W: Widget + ?Sized> Portal<W> {
    fn scrollbar_states(
        &self,
        portal: Size,
        content: Size,
    ) -> [(&WidgetPod<ScrollBar>, f64, f64, f64); 2] {
        [
            (Axis::Horizontal, &self.scrollbar_horizontal),
            (Axis::Vertical, &self.scrollbar_vertical),
        ]
        .map(|(axis, bar)| {
            let viewport = portal.get_coord(axis);
            let total = content.get_coord(axis);
            let range = (total - viewport).max(0.0);
            let progress = if range > 0.0 {
                (self.viewport_pos.get_coord(axis) / range).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (bar, viewport, total, progress)
        })
    }

    fn set_viewport_pos_event_ctx(
        &mut self,
        ctx: &mut EventCtx<'_>,
        portal_size: Size,
        content_size: Size,
        pos: Point,
    ) -> bool {
        let changed = self.set_viewport_pos_raw(portal_size, content_size, pos);
        if changed {
            ctx.request_compose();
            for (bar, viewport, content, progress) in
                self.scrollbar_states(portal_size, content_size)
            {
                ScrollBar::synchronize_raw(ctx.get_raw_mut(bar), viewport, content, progress);
            }
        }
        changed
    }

    fn pan_viewport_by_event_ctx(
        &mut self,
        ctx: &mut EventCtx<'_>,
        portal_size: Size,
        content_size: Size,
        mut delta: Vec2,
    ) -> bool {
        if self.constraints.constrain_horizontal {
            delta.x = 0.0;
        }
        if self.constraints.constrain_vertical {
            delta.y = 0.0;
        }
        if delta.x == 0.0 && delta.y == 0.0 {
            return false;
        }
        self.set_viewport_pos_event_ctx(ctx, portal_size, content_size, self.viewport_pos + delta)
    }

    /// Returns the scrolling "position" of the container.
    #[must_use = "Use the returned value."]
    pub const fn get_viewport_pos(&self) -> Point {
        self.viewport_pos
    }

    // TODO - rename
    fn set_viewport_pos_raw(&mut self, portal_size: Size, content_size: Size, pos: Point) -> bool {
        if !pos.x.is_finite() || !pos.y.is_finite() {
            return false;
        }
        let viewport_max_pos = (content_size - portal_size).max(Size::ZERO);
        let pos = Point::new(
            if self.constraints.constrain_horizontal {
                0.0
            } else {
                pos.x.clamp(0.0, viewport_max_pos.width)
            },
            if self.constraints.constrain_vertical {
                0.0
            } else {
                pos.y.clamp(0.0, viewport_max_pos.height)
            },
        );

        if (pos - self.viewport_pos).hypot2() > 1e-12 {
            self.viewport_pos = pos;
            true
        } else {
            false
        }
    }

    // Note - Rect is in child coordinates
    // TODO - Merge with pan_viewport_to
    // Right now these functions are just different enough to be a pain to merge.
    fn pan_viewport_to_raw(&mut self, portal_size: Size, content_size: Size, target: Rect) -> bool {
        let viewport = Rect::from_origin_size(self.viewport_pos, portal_size);

        let new_pos_x = compute_pan_range(
            viewport.min_x()..viewport.max_x(),
            target.min_x()..target.max_x(),
        )
        .start;
        let new_pos_y = compute_pan_range(
            viewport.min_y()..viewport.max_y(),
            target.min_y()..target.max_y(),
        )
        .start;

        self.set_viewport_pos_raw(portal_size, content_size, Point::new(new_pos_x, new_pos_y))
    }
}

// --- MARK: WIDGETMUT
impl<W: Widget + FromDynWidget + ?Sized> Portal<W> {
    child_accessors!(child, set_child, child_mut, W);

    /// Returns mutable reference to the horizontal scrollbar.
    pub fn horizontal_scrollbar_mut<'t>(
        this: &'t mut WidgetMut<'_, Self>,
    ) -> Option<WidgetMut<'t, ScrollBar>> {
        this.ctx.get_mut(this.widget.scrollbar_horizontal.id())
    }

    /// Returns mutable reference to the vertical scrollbar.
    pub fn vertical_scrollbar_mut<'t>(
        this: &'t mut WidgetMut<'_, Self>,
    ) -> Option<WidgetMut<'t, ScrollBar>> {
        this.ctx.get_mut(this.widget.scrollbar_vertical.id())
    }

    /// Sets whether to constrain the child horizontally.
    ///
    /// See [`Portal::constrain_horizontal`] for more details.
    pub fn set_constrain_horizontal(this: &mut WidgetMut<'_, Self>, constrain: bool) {
        this.widget.constraints.constrain_horizontal = constrain;
        this.ctx.request_layout();
    }

    /// Sets whether to constrain the child vertically.
    ///
    /// See [`Portal::constrain_vertical`] for more details.
    pub fn set_constrain_vertical(this: &mut WidgetMut<'_, Self>, constrain: bool) {
        this.widget.constraints.constrain_vertical = constrain;
        this.ctx.request_layout();
    }

    /// Sets whether the child's size must be greater than or equal the size of
    /// the `Portal`.
    ///
    /// See [`content_must_fill`] for more details.
    ///
    /// [`content_must_fill`]: Portal::content_must_fill
    pub fn set_content_must_fill(this: &mut WidgetMut<'_, Self>, must_fill: bool) {
        this.widget.constraints.must_fill = must_fill;
        this.ctx.request_layout();
    }

    /// Sets the scrolling "position" of the container.
    ///
    /// A position of zero means no scrolling at all.
    pub fn set_viewport_pos(this: &mut WidgetMut<'_, Self>, position: Point) -> bool {
        let portal_size = this.ctx.content_box().size();
        let content_size = this.widget.content_size;

        let pos_changed = this
            .widget
            .set_viewport_pos_raw(portal_size, content_size, position);
        if pos_changed {
            for (bar, viewport, content, progress) in
                this.widget.scrollbar_states(portal_size, content_size)
            {
                ScrollBar::synchronize_raw(this.ctx.get_raw_mut(bar), viewport, content, progress);
            }
            this.ctx.request_compose();
        }
        pos_changed
    }

    /// Translates the scrolling "position" of the container.
    pub fn pan_viewport_by(this: &mut WidgetMut<'_, Self>, translation: Vec2) -> bool {
        Self::set_viewport_pos(this, this.widget.viewport_pos + translation)
    }

    /// Changes the scrolling "position" of the container so that `target` is scrolled into view.
    ///
    /// `target` is in the child's border-box coordinate space, meaning a target
    /// of `(0, 0, 10, 10)` will scroll an item at the top-left of the child into view.
    pub fn pan_viewport_to(this: &mut WidgetMut<'_, Self>, target: Rect) -> bool {
        let portal_size = this.ctx.content_box().size();
        let viewport = Rect::from_origin_size(this.widget.viewport_pos, portal_size);

        let new_pos_x = compute_pan_range(
            viewport.min_x()..viewport.max_x(),
            target.min_x()..target.max_x(),
        )
        .start;
        let new_pos_y = compute_pan_range(
            viewport.min_y()..viewport.max_y(),
            target.min_y()..target.max_y(),
        )
        .start;

        Self::set_viewport_pos(this, Point::new(new_pos_x, new_pos_y))
    }
}

impl<W: Widget> UsesProperty<AutoHideScrollBar> for Portal<W> {}

// --- MARK: IMPL WIDGET
impl<W: Widget + FromDynWidget + ?Sized> Widget for Portal<W> {
    type Action = NoAction;

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        let cache = ctx.property_cache();
        let auto_hide_scroll_bar = props.get::<AutoHideScrollBar>(cache).0;

        let portal_size = ctx.content_box().size();
        let content_size = self.content_size;

        match *event {
            PointerEvent::Scroll(PointerScrollEvent { delta, .. }) => {
                let scale_factor = ctx.scale_factor();
                let line_px = PhysicalPosition {
                    x: 120.0 * scale_factor,
                    y: 120.0 * scale_factor,
                };
                let page_px = PhysicalPosition {
                    x: portal_size.width * scale_factor,
                    y: portal_size.height * scale_factor,
                };
                let delta_px = delta.to_pixel_delta(line_px, page_px);
                let LogicalPosition { x, y } = delta_px.to_logical::<f64>(scale_factor);
                let mut delta = -Vec2 { x, y };

                // Ignore scroll deltas in directions that are constrained
                if self.constraints.constrain_horizontal {
                    delta.x = 0.;
                }
                if self.constraints.constrain_vertical {
                    delta.y = 0.;
                }

                if self.pan_viewport_by_event_ctx(ctx, portal_size, content_size, delta) {
                    ctx.set_handled();
                }
            }
            PointerEvent::Move(_) if auto_hide_scroll_bar => {
                let f = |mut bar: WidgetMut<'_, ScrollBar>| {
                    if bar.widget.opacity.value() == 0. {
                        bar.widget.opacity.move_to(1., SCROLLBAR_ANIM_OVER_MILLIS);
                        bar.ctx.request_anim_frame();
                    }
                };
                ctx.mutate_child_later(&self.scrollbar_horizontal, f);
                ctx.mutate_child_later(&self.scrollbar_vertical, f);

                self.nanos_since_last_pointer_move = Some(0);
                ctx.request_anim_frame();
            }
            _ => (),
        }
    }

    fn on_text_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &TextEvent,
    ) {
        let portal_size = ctx.content_box().size();
        let content_size = self.content_size;
        let target = ctx.target();
        let scrollbar_target =
            target == self.scrollbar_vertical.id() || target == self.scrollbar_horizontal.id();

        if let TextEvent::Keyboard(event) = event
            && event.state.is_down()
            // Avoid scrolling the portal when the focused widget is one of its scrollbars.
            // Scrollbars are focusable for keyboard users, and in that case they should own
            // the arrow/page/home/end keys.
            && !scrollbar_target
        {
            let line = 120.0;
            let page_y = portal_size.height;

            let mut did_scroll = false;
            match &event.key {
                Key::Named(NamedKey::PageDown) => {
                    did_scroll |= self.pan_viewport_by_event_ctx(
                        ctx,
                        portal_size,
                        content_size,
                        Vec2::new(0.0, page_y),
                    );
                }
                Key::Named(NamedKey::PageUp) => {
                    did_scroll |= self.pan_viewport_by_event_ctx(
                        ctx,
                        portal_size,
                        content_size,
                        Vec2::new(0.0, -page_y),
                    );
                }
                Key::Named(NamedKey::ArrowDown) => {
                    did_scroll |= self.pan_viewport_by_event_ctx(
                        ctx,
                        portal_size,
                        content_size,
                        Vec2::new(0.0, line),
                    );
                }
                Key::Named(NamedKey::ArrowUp) => {
                    did_scroll |= self.pan_viewport_by_event_ctx(
                        ctx,
                        portal_size,
                        content_size,
                        Vec2::new(0.0, -line),
                    );
                }
                Key::Named(NamedKey::ArrowRight) => {
                    did_scroll |= self.pan_viewport_by_event_ctx(
                        ctx,
                        portal_size,
                        content_size,
                        Vec2::new(line, 0.0),
                    );
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    did_scroll |= self.pan_viewport_by_event_ctx(
                        ctx,
                        portal_size,
                        content_size,
                        Vec2::new(-line, 0.0),
                    );
                }
                Key::Named(NamedKey::Home) => {
                    did_scroll |= self.set_viewport_pos_event_ctx(
                        ctx,
                        portal_size,
                        content_size,
                        Point::new(0.0, 0.0),
                    );
                }
                Key::Named(NamedKey::End) => {
                    let scroll_range = (content_size - portal_size).max(Size::ZERO);
                    did_scroll |= self.set_viewport_pos_event_ctx(
                        ctx,
                        portal_size,
                        content_size,
                        Point::new(scroll_range.width, scroll_range.height),
                    );
                }
                _ => {}
            }
            if did_scroll {
                ctx.set_handled();
            }
        }
    }

    fn on_access_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &AccessEvent,
    ) {
        let portal_size = ctx.content_box().size();
        let content_size = self.content_size;
        let target = ctx.target();
        let scrollbar_target =
            target == self.scrollbar_vertical.id() || target == self.scrollbar_horizontal.id();

        if !scrollbar_target
            && matches!(
                event.action,
                accesskit::Action::ScrollUp
                    | accesskit::Action::ScrollDown
                    | accesskit::Action::ScrollLeft
                    | accesskit::Action::ScrollRight
            )
        {
            let unit = if let Some(accesskit::ActionData::ScrollUnit(unit)) = &event.data {
                *unit
            } else {
                accesskit::ScrollUnit::Item
            };
            let line = 120.0;
            let amount = match unit {
                accesskit::ScrollUnit::Item => line,
                accesskit::ScrollUnit::Page => match event.action {
                    accesskit::Action::ScrollLeft | accesskit::Action::ScrollRight => {
                        portal_size.width
                    }
                    _ => portal_size.height,
                },
            };

            let delta = match event.action {
                accesskit::Action::ScrollUp => Vec2::new(0.0, -amount),
                accesskit::Action::ScrollDown => Vec2::new(0.0, amount),
                accesskit::Action::ScrollLeft => Vec2::new(-amount, 0.0),
                accesskit::Action::ScrollRight => Vec2::new(amount, 0.0),
                _ => Vec2::ZERO,
            };

            if self.pan_viewport_by_event_ctx(ctx, portal_size, content_size, delta) {
                ctx.set_handled();
            }
        }
    }

    fn on_action(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        action: &ErasedAction,
        source: WidgetId,
    ) {
        let Some(movement) = action.downcast_ref::<ScrollBarMoved>() else {
            return;
        };
        let portal_size = ctx.content_box().size();
        let content_size = self.content_size;
        let range = (content_size - portal_size).max(Size::ZERO);
        let position = if source == self.scrollbar_horizontal.id() {
            Point::new(movement.0 * range.width, self.viewport_pos.y)
        } else if source == self.scrollbar_vertical.id() {
            Point::new(self.viewport_pos.x, movement.0 * range.height)
        } else {
            return;
        };
        self.set_viewport_pos_raw(portal_size, content_size, position);
        for (bar, viewport, content, progress) in self.scrollbar_states(portal_size, content_size) {
            ScrollBar::synchronize_raw(ctx.get_raw_mut(bar), viewport, content, progress);
        }
        ctx.request_compose();
        ctx.set_handled();
    }

    fn on_anim_frame(
        &mut self,
        ctx: &mut UpdateCtx<'_>,
        props: &mut PropertiesMut<'_>,
        interval: u64,
    ) {
        let cache = ctx.property_cache();
        let auto_hide_scroll_bar = props.get::<AutoHideScrollBar>(cache).0;

        match self.nanos_since_last_pointer_move.take() {
            None => {
                let rest_opacity = if auto_hide_scroll_bar { 0. } else { 1. };
                let f = move |bar: WidgetMut<'_, ScrollBar>| {
                    bar.widget.opacity.move_to(rest_opacity, 0.);
                };
                ctx.mutate_child_later(&self.scrollbar_horizontal, f);
                ctx.mutate_child_later(&self.scrollbar_vertical, f);
            }
            Some(mut since_last_move) if auto_hide_scroll_bar => {
                since_last_move = since_last_move.saturating_add(interval);

                if since_last_move >= VISIBILITY_TIMEOUT {
                    let f = |mut bar: WidgetMut<'_, ScrollBar>| {
                        bar.widget.opacity.move_to(0., SCROLLBAR_ANIM_OVER_MILLIS);
                        bar.ctx.request_anim_frame();
                    };
                    ctx.mutate_child_later(&self.scrollbar_horizontal, f);
                    ctx.mutate_child_later(&self.scrollbar_vertical, f);
                } else {
                    self.nanos_since_last_pointer_move = Some(since_last_move);
                    ctx.request_anim_frame();
                }
            }
            Some(since_last_move) => self.nanos_since_last_pointer_move = Some(since_last_move),
        }
    }

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        ctx.register_child(&mut self.child);
        ctx.register_child(&mut self.scrollbar_horizontal);
        ctx.register_child(&mut self.scrollbar_vertical);
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        match event {
            Update::RequestPanToChild { child, rect } if *child == self.child.id() => {
                let portal_size = ctx.content_box().size();
                let content_size = self.content_size;

                self.pan_viewport_to_raw(
                    portal_size,
                    content_size,
                    *rect + self.viewport_pos.to_vec2().round(),
                );
                ctx.request_compose();

                for (bar, viewport, content, progress) in
                    self.scrollbar_states(portal_size, content_size)
                {
                    ScrollBar::synchronize_raw(ctx.get_raw_mut(bar), viewport, content, progress);
                }
            }
            _ => {}
        }
    }

    fn property_changed(&mut self, ctx: &mut UpdateCtx<'_>, property_type: TypeId) {
        if AutoHideScrollBar::matches(property_type) {
            ctx.request_anim_frame();
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
        match len_req {
            LenReq::Min => Length::ZERO,
            LenReq::Max => {
                let context_size = LayoutSize::maybe(axis.cross(), cross_length);
                let auto_length = len_req.into();

                let cross = axis.cross();
                let cross_space = cross_length.filter(|_| match cross {
                    Axis::Horizontal => self.constraints.constrain_horizontal,
                    Axis::Vertical => self.constraints.constrain_vertical,
                });

                ctx.compute_length(&self.child, auto_length, context_size, axis, cross_space)
            }
            LenReq::Fit(space) => space,
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, size: Size) {
        let auto_size = SizeDef::new(
            if self.constraints.constrain_horizontal {
                LenDef::FitContent(size.width.px())
            } else {
                LenDef::MaxContent
            },
            if self.constraints.constrain_vertical {
                LenDef::FitContent(size.height.px())
            } else {
                LenDef::MaxContent
            },
        );
        let content_size = {
            let child_size = ctx.compute_size(&self.child, auto_size, size.into());
            if self.constraints.must_fill {
                child_size.max(size)
            } else {
                child_size
            }
        };
        ctx.run_layout(&self.child, content_size);
        self.content_size = content_size;

        // TODO - document better
        // Recompute the portal offset for the new layout
        self.set_viewport_pos_raw(size, content_size, self.viewport_pos);
        for (bar, viewport, content, progress) in self.scrollbar_states(size, content_size) {
            ScrollBar::synchronize_raw(ctx.get_raw_mut(bar), viewport, content, progress);
        }

        ctx.set_clip_path(size.to_rect());

        ctx.place_child(&self.child, Point::ZERO);

        self.scrollbar_horizontal_visible =
            !self.constraints.constrain_horizontal && size.width < content_size.width;
        self.scrollbar_vertical_visible =
            !self.constraints.constrain_vertical && size.height < content_size.height;

        ctx.set_stashed(
            &self.scrollbar_horizontal,
            !self.scrollbar_horizontal_visible,
        );
        if self.scrollbar_horizontal_visible {
            let scrollbar_size =
                ctx.compute_size(&self.scrollbar_horizontal, SizeDef::fit(size), size.into());
            ctx.run_layout(&self.scrollbar_horizontal, scrollbar_size);
            ctx.place_child(
                &self.scrollbar_horizontal,
                Point::new(0.0, size.height - scrollbar_size.height),
            );
        }

        ctx.set_stashed(&self.scrollbar_vertical, !self.scrollbar_vertical_visible);
        if self.scrollbar_vertical_visible {
            let scrollbar_size =
                ctx.compute_size(&self.scrollbar_vertical, SizeDef::fit(size), size.into());
            ctx.run_layout(&self.scrollbar_vertical, scrollbar_size);
            ctx.place_child(
                &self.scrollbar_vertical,
                Point::new(size.width - scrollbar_size.width, 0.0),
            );
        }
    }

    fn compose(&mut self, ctx: &mut ComposeCtx<'_>) {
        ctx.set_child_scroll_translation(
            &self.child,
            Vec2::new(-self.viewport_pos.x, -self.viewport_pos.y),
        );
    }

    fn paint(
        &mut self,
        _ctx: &mut PaintCtx<'_>,
        _props: &PropertiesRef<'_>,
        _painter: &mut Painter<'_>,
    ) {
    }

    fn accessibility_role(&self) -> Role {
        Role::ScrollView
    }

    fn accessibility(
        &mut self,
        ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        node.set_clips_children();

        let portal_size = ctx.content_box().size();
        let content_size = self.content_size;
        let scroll_range = (content_size - portal_size).max(Size::ZERO);

        let can_scroll_x = !self.constraints.constrain_horizontal && scroll_range.width > 1e-12;
        let can_scroll_y = !self.constraints.constrain_vertical && scroll_range.height > 1e-12;

        if can_scroll_x {
            node.set_scroll_x_min(0.0);
            node.set_scroll_x_max(scroll_range.width);
            node.set_scroll_x(self.viewport_pos.x);
            if self.viewport_pos.x > 1e-12 {
                node.add_action(accesskit::Action::ScrollLeft);
            }
            if self.viewport_pos.x + 1e-12 < scroll_range.width {
                node.add_action(accesskit::Action::ScrollRight);
            }
        }

        if can_scroll_y {
            node.set_scroll_y_min(0.0);
            node.set_scroll_y_max(scroll_range.height);
            node.set_scroll_y(self.viewport_pos.y);
            if self.viewport_pos.y > 1e-12 {
                node.add_action(accesskit::Action::ScrollUp);
            }
            if self.viewport_pos.y + 1e-12 < scroll_range.height {
                node.add_action(accesskit::Action::ScrollDown);
            }
        }

        if can_scroll_y && !can_scroll_x {
            node.set_orientation(accesskit::Orientation::Vertical);
        } else if can_scroll_x && !can_scroll_y {
            node.set_orientation(accesskit::Orientation::Horizontal);
        }

        node.add_child_action(accesskit::Action::ScrollIntoView);
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::from_slice(&[
            self.child.id(),
            self.scrollbar_vertical.id(),
            self.scrollbar_horizontal.id(),
        ])
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("Portal", id = id.trace())
    }

    fn accepts_focus(&self) -> bool {
        !(self.constraints.constrain_horizontal && self.constraints.constrain_vertical)
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::WidgetTag;
    use crate::core::keyboard::{Key, NamedKey};
    use crate::layout::AsUnit;
    use crate::properties::Dimensions;
    use crate::testing::{ModularWidget, TestHarness, assert_render_snapshot};
    use crate::theme::test_property_set;
    use crate::widgets::{Button, Flex, SizedBox};

    fn button(
        text: &'static str,
        top_pad: f64,
        tag: Option<WidgetTag<Button>>,
    ) -> NewWidget<ModularWidget<WidgetPod<Button>>> {
        let btn = NewWidget::new(Button::with_text(text))
            .with_tag(tag.unwrap_or_else(WidgetTag::unique))
            .with_props(Dimensions::fixed(70.px(), 40.px()));

        ModularWidget::new_parent(btn)
            .measure_fn(move |child, ctx, _props, axis, len_req, cross_length| {
                let auto_length = len_req.into();
                let context_size = LayoutSize::maybe(axis.cross(), cross_length);

                let other = match axis {
                    Axis::Horizontal => Length::ZERO,
                    Axis::Vertical => top_pad.px(),
                };

                ctx.compute_length(child, auto_length, context_size, axis, cross_length)
                    .saturating_add(other)
            })
            .layout_fn(move |child, ctx, _props, size| {
                let child_size = ctx.compute_size(child, SizeDef::fit(size), size.into());
                ctx.run_layout(child, child_size);
                // We don't place it at (0,0) to test that stacked-origin translation works.
                // Because if we were at (0,0) it would be effectively the same as no parent.
                ctx.place_child(child, Point::new(0., top_pad));
            })
            .prepare()
    }

    #[test]
    fn button_list() {
        let button_3 = WidgetTag::named("button-3");
        let button_13 = WidgetTag::named("button-13");

        let widget = Portal::new(NewWidget::new(
            Flex::column()
                .with_fixed(button("Item 1", 0., None))
                .with_fixed(button("Item 2", 20., None))
                .with_fixed(button("Item 3", 20., Some(button_3)))
                .with_fixed(button("Item 4", 20., None))
                .with_fixed(button("Item 5", 20., None))
                .with_fixed(button("Item 6", 20., None))
                .with_fixed(button("Item 7", 20., None))
                .with_fixed(button("Item 8", 20., None))
                .with_fixed(button("Item 9", 20., None))
                .with_fixed(button("Item 10", 20., None))
                .with_fixed(button("Item 11", 20., None))
                .with_fixed(button("Item 12", 20., None))
                .with_fixed(button("Item 13", 20., Some(button_13)))
                .with_fixed(button("Item 14", 20., None))
                .with_fixed_spacer(10.px()),
        ))
        .prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (400, 400));

        harness
            .edit_root_widget(|mut portal| {
                Portal::set_viewport_pos(&mut portal, Point::new(0.0, 130.0))
            })
            .unwrap();
        assert_render_snapshot!(harness, "portal_button_list_scrolled");

        harness.scroll_into_view(harness.get_widget(button_3).unwrap().id());
        assert_render_snapshot!(harness, "portal_button_list_scroll_to_item_3");

        harness.scroll_into_view(harness.get_widget(button_13).unwrap().id());
        assert_render_snapshot!(harness, "portal_button_list_scroll_to_item_13");
    }

    #[test]
    fn scroll_into_view() {
        let button_tag = WidgetTag::named("hidden-button");

        let widget = Portal::new(
            Flex::column()
                .with_fixed_spacer(500.px())
                .with_fixed(NewWidget::new(Button::with_text("Fully visible")).with_tag(button_tag))
                .with_fixed_spacer(500.px())
                .prepare(),
        )
        .prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (200, 200));
        let button_id = harness.get_widget(button_tag).unwrap().id();

        harness.scroll_into_view(button_id);
        assert_render_snapshot!(harness, "portal_scrolled_button_into_view");
    }

    #[test]
    fn autohidden_scrollbar() {
        let widget = Portal::new(Flex::column().with_fixed_spacer(1000.px()).prepare())
            .prepare()
            .with_props(AutoHideScrollBar(true));

        let mut default_props = test_property_set();
        default_props.insert::<ScrollBar, _>(crate::properties::Collapsible(true));

        let mut harness = TestHarness::create_with_size(default_props, widget, (200, 200));

        assert_render_snapshot!(harness, "portal_scrollbar_autohidden");

        harness.mouse_move((150., 50.));
        harness.animate_ms(300);
        assert_render_snapshot!(harness, "portal_scrollbar_shown_on_mouse_jiggle");
    }

    #[test]
    fn portal_accessibility_node_exposes_scroll() {
        let portal_tag = WidgetTag::named("portal");
        let content = SizedBox::empty().size(300.px(), 300.px()).prepare();
        let portal = NewWidget::new(Portal::new(content)).with_tag(portal_tag);

        let mut harness = TestHarness::create_with_size(test_property_set(), portal, (100, 100));
        let _ = harness.render().unwrap();

        let portal_id = harness.get_widget(portal_tag).unwrap().id();
        let node = harness.access_node(portal_id).unwrap();

        assert_eq!(node.data().role(), Role::ScrollView);
        assert!(node.data().supports_action(accesskit::Action::ScrollDown));
        assert!(!node.data().supports_action(accesskit::Action::ScrollUp));
        assert!(
            node.data()
                .child_supports_action(accesskit::Action::ScrollIntoView)
        );
        assert_eq!(node.data().scroll_y_min(), Some(0.0));
        assert!(node.data().scroll_y_max().is_some());
        assert_eq!(node.data().scroll_y(), Some(0.0));

        harness
            .edit_widget(portal_tag, |mut portal| {
                Portal::pan_viewport_by(&mut portal, Vec2::new(0., 99999.));
            })
            .unwrap();
        harness.render().unwrap();

        let node = harness.access_node(portal_id).unwrap();
        assert!(!node.data().supports_action(accesskit::Action::ScrollDown));
        assert!(node.data().supports_action(accesskit::Action::ScrollUp));
    }

    #[test]
    fn portal_keyboard_scroll_updates_access_tree() {
        let portal_tag = WidgetTag::named("portal");
        let content = SizedBox::empty().size(300.px(), 300.px()).prepare();
        let portal = NewWidget::new(Portal::new(content)).with_tag(portal_tag);

        let mut harness = TestHarness::create_with_size(test_property_set(), portal, (100, 100));
        let _ = harness.render().unwrap();

        let portal_id = harness.get_widget(portal_tag).unwrap().id();
        harness.focus_on(Some(portal_id));

        harness.process_text_event(&TextEvent::key_down(Key::Named(NamedKey::PageDown)));
        let _ = harness.render().unwrap();

        let node = harness.access_node(portal_id).unwrap();
        assert!(node.data().scroll_y().unwrap_or(0.0) > 0.0);
    }

    #[test]
    fn nested_portals_reveal_transformed_descendant_from_current_scroll_positions() {
        use crate::kurbo::Affine;
        use crate::properties::Gap;

        let target = WidgetTag::named("nested-scroll-target");
        let inner = WidgetTag::named("inner-portal");
        let inner_content = Flex::column()
            .with_fixed_spacer(300.px())
            .with_fixed(
                SizedBox::empty()
                    .size(40.px(), 30.px())
                    .prepare()
                    .with_transform(Affine::translate((0.0, 20.0)))
                    .with_tag(target),
            )
            .with_fixed_spacer(300.px())
            .prepare()
            .with_props(Gap::new(Length::ZERO));
        let outer_content = Flex::column()
            .with_fixed_spacer(260.px())
            .with_fixed(
                Portal::new(inner_content)
                    .constrain_horizontal(true)
                    .prepare()
                    .with_props(Dimensions::fixed(80.px(), 120.px()))
                    .with_tag(inner),
            )
            .with_fixed_spacer(400.px())
            .prepare()
            .with_props(Gap::new(Length::ZERO));
        let root = Portal::new(outer_content)
            .constrain_horizontal(true)
            .prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), root, (120, 100));
        harness
            .edit_root_widget(|mut portal| {
                Portal::set_viewport_pos(&mut portal, Point::new(0.0, 80.0));
            })
            .unwrap();
        harness
            .edit_widget(inner, |mut portal| {
                Portal::set_viewport_pos(&mut portal, Point::new(0.0, 50.0));
            })
            .unwrap();

        let target_id = harness.get_widget(target).unwrap().id();
        for _ in 0..2 {
            harness.scroll_into_view(target_id);
            let inner_position = harness.get_widget(inner).unwrap().get_viewport_pos();
            let outer_position = harness.root_widget().unwrap().get_viewport_pos();
            assert!(
                (inner_position.y - 230.0).abs() < 1e-6,
                "inner viewport must account for target transform: {inner_position:?}"
            );
            assert!(
                (outer_position.y - 280.0).abs() < 1e-6,
                "outer viewport must reveal the region after inner scrolling: {outer_position:?}"
            );
            let widget = harness.get_widget(target).unwrap();
            let rect = widget
                .ctx()
                .window_transform()
                .transform_rect_bbox(widget.ctx().border_box());
            assert!(
                rect.y0 >= 0.0 && rect.y1 <= 100.0,
                "target must be visible through both portals: {rect:?}"
            );
        }

        // The outer scrollbar is already visible; it is not part of the scrollable content.
        let bar = harness.root_widget().unwrap().children()[1].id();
        harness.scroll_into_view(bar);
        let position = harness.root_widget().unwrap().get_viewport_pos();
        assert!(
            (position.y - 280.0).abs() < 1e-6,
            "scrollbar reveal must not scroll its owner's content"
        );
    }

    #[test]
    fn scrollbar_action_is_consumed_and_progress_resynchronizes_after_resize() {
        let content = SizedBox::empty().size(80.px(), 600.px()).prepare();
        let widget = Portal::new(content).constrain_horizontal(true).prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (100, 200));
        let bar = harness.root_widget().unwrap().children()[1];
        let bar_id = bar.id();
        let end = bar.ctx().local_to_window(Point::new(
            bar.ctx().content_box().width() / 2.0,
            bar.ctx().content_box().height() - 1.0,
        ));
        harness.mouse_move(end);
        harness.mouse_button_press(None);
        harness.mouse_button_release(None);
        assert!(harness.pop_action_erased().is_none());
        assert!((harness.root_widget().unwrap().get_viewport_pos().y - 400.0).abs() < 1e-6);
        assert!(
            (harness
                .get_widget_with_id(bar_id)
                .unwrap()
                .downcast::<ScrollBar>()
                .unwrap()
                .cursor_progress()
                - 1.0)
                .abs()
                < 1e-9
        );

        harness.process_window_event(crate::core::WindowEvent::Resize((100, 400).into()));
        assert!((harness.root_widget().unwrap().get_viewport_pos().y - 200.0).abs() < 1e-6);
        let bar = harness
            .get_widget_with_id(bar_id)
            .unwrap()
            .downcast::<ScrollBar>()
            .unwrap();
        assert!((bar.cursor_progress() - 1.0).abs() < 1e-9);
        harness.process_window_event(crate::core::WindowEvent::Resize((100, 700).into()));
        let bar = harness
            .get_widget_with_id(bar_id)
            .unwrap()
            .downcast::<ScrollBar>()
            .unwrap();
        assert!(bar.ctx().is_stashed());
        assert!(bar.cursor_progress().abs() < 1e-9);
        assert!(harness.root_widget().unwrap().get_viewport_pos().y.abs() < 1e-9);
    }

    #[test]
    fn single_axis_scroll_keeps_both_scrollbar_progress_values_finite() {
        let top = WidgetTag::named("scroll-top");
        let bottom = WidgetTag::named("scroll-bottom");
        let content = Flex::column()
            .with_fixed(NewWidget::new(SizedBox::empty().size(20.px(), 20.px())).with_tag(top))
            .with_fixed_spacer(400.px())
            .with_fixed(NewWidget::new(SizedBox::empty().size(20.px(), 20.px())).with_tag(bottom))
            .prepare();
        let portal = Portal::new(content)
            .constrain_horizontal(true)
            .content_must_fill(true)
            .prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), portal, (100, 100));
        harness
            .edit_root_widget(|mut portal| {
                assert!(Portal::set_viewport_pos(&mut portal, Point::new(0.0, 80.0)));
                assert_eq!(
                    Portal::horizontal_scrollbar_mut(&mut portal)
                        .unwrap()
                        .widget
                        .cursor_progress(),
                    0.0
                );
                let progress = Portal::vertical_scrollbar_mut(&mut portal)
                    .unwrap()
                    .widget
                    .cursor_progress();
                assert!(progress > 0.0 && progress < 1.0);
            })
            .unwrap();
        let bottom_id = harness.get_widget(bottom).unwrap().id();
        harness.scroll_into_view(bottom_id);
        harness
            .edit_root_widget(|mut portal| {
                assert_eq!(
                    Portal::horizontal_scrollbar_mut(&mut portal)
                        .unwrap()
                        .widget
                        .cursor_progress(),
                    0.0
                );
                assert_eq!(
                    Portal::vertical_scrollbar_mut(&mut portal)
                        .unwrap()
                        .widget
                        .cursor_progress(),
                    1.0
                );
            })
            .unwrap();
        let top_id = harness.get_widget(top).unwrap().id();
        harness.scroll_into_view(top_id);
        harness
            .edit_root_widget(|mut portal| {
                assert_eq!(
                    Portal::horizontal_scrollbar_mut(&mut portal)
                        .unwrap()
                        .widget
                        .cursor_progress(),
                    0.0
                );
                assert_eq!(
                    Portal::vertical_scrollbar_mut(&mut portal)
                        .unwrap()
                        .widget
                        .cursor_progress(),
                    0.0
                );
            })
            .unwrap();
    }

    // Helper function for panning tests
    fn make_range(repr: &str) -> Range<f64> {
        let repr = &repr[repr.find('_').unwrap()..];

        let start = repr.find('x').unwrap();
        let end = repr[start..].find('_').unwrap() + start;

        assert!(repr[end..].chars().all(|c| c == '_'));

        (start as f64)..(end as f64)
    }

    #[test]
    fn test_pan_to_same() {
        let initial_range = make_range("_______xxxx_____");
        let target_range = make_range(" _______xxxx_____");
        let result_range = make_range(" _______xxxx_____");

        assert_eq!(compute_pan_range(initial_range, target_range), result_range);
    }

    #[test]
    fn test_pan_to_smaller() {
        let initial_range = make_range("_____xxxxxxxx___");
        let target_range = make_range(" _______xxxx_____");
        let result_range = make_range(" _____xxxxxxxx___");

        assert_eq!(compute_pan_range(initial_range, target_range), result_range);
    }

    #[test]
    fn test_pan_to_larger() {
        let initial_range = make_range("_______xxxx_____");
        let target_range = make_range(" _____xxxxxxxx___");
        let result_range = make_range(" _______xxxx_____");

        assert_eq!(compute_pan_range(initial_range, target_range), result_range);
    }

    #[test]
    fn test_pan_to_left() {
        let initial_range = make_range("_______xxxx_____");
        let target_range = make_range(" ____xx__________");
        let result_range = make_range(" ____xxxx________");

        assert_eq!(compute_pan_range(initial_range, target_range), result_range);
    }

    #[test]
    fn test_pan_to_left_intersects() {
        let initial_range = make_range("_______xxxxx____");
        let target_range = make_range(" ____xxxx________");
        let result_range = make_range(" ____xxxxx_______");

        assert_eq!(compute_pan_range(initial_range, target_range), result_range);
    }

    #[test]
    fn test_pan_to_left_larger() {
        let initial_range = make_range("__________xx____");
        let target_range = make_range(" ____xxxx________");
        let result_range = make_range(" ______xx________");

        assert_eq!(compute_pan_range(initial_range, target_range), result_range);
    }

    #[test]
    fn test_pan_to_left_larger_intersects() {
        let initial_range = make_range("_______xx_______");
        let target_range = make_range(" ____xxxx________");
        let result_range = make_range(" ______xx________");

        assert_eq!(compute_pan_range(initial_range, target_range), result_range);
    }

    #[test]
    fn test_pan_to_right() {
        let initial_range = make_range("_____xxxx_______");
        let target_range = make_range(" __________xx____");
        let result_range = make_range(" ________xxxx____");

        assert_eq!(compute_pan_range(initial_range, target_range), result_range);
    }

    #[test]
    fn test_pan_to_right_intersects() {
        let initial_range = make_range("____xxxxx_______");
        let target_range = make_range(" ________xxxx____");
        let result_range = make_range(" _______xxxxx____");

        assert_eq!(compute_pan_range(initial_range, target_range), result_range);
    }

    #[test]
    fn test_pan_to_right_larger() {
        let initial_range = make_range("____xx__________");
        let target_range = make_range(" ________xxxx____");
        let result_range = make_range(" ________xx______");

        assert_eq!(compute_pan_range(initial_range, target_range), result_range);
    }

    #[test]
    fn test_pan_to_right_larger_intersects() {
        let initial_range = make_range("_______xx_______");
        let target_range = make_range(" ________xxxx____");
        let result_range = make_range(" ________xx______");

        assert_eq!(compute_pan_range(initial_range, target_range), result_range);
    }
}
