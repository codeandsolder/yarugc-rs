// Copyright 2022 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;

use accesskit::{Node, Role};
use include_doc_path::include_doc_path;
use tracing::{Span, trace_span};

use crate::core::keyboard::{Key, KeyState, NamedKey};
use crate::core::{
    AccessCtx, AccessEvent, AllowRawMut, ChildrenIds, EventCtx, LayoutCtx, MeasureCtx, PaintCtx,
    PointerButton, PointerButtonEvent, PointerEvent, PointerUpdate, PropertiesMut, PropertiesRef,
    Property, RawCtx, RegisterCtx, TextEvent, Update, UpdateCtx, UsesProperty, Widget, WidgetId,
    WidgetMut,
};
use crate::imaging::{Composite, GroupRef, Painter};
use crate::kurbo::{Axis, Point, Rect, Size, Stroke};
use crate::layout::{AsUnit, LenReq, Length};
use crate::properties::Collapsible;
use crate::theme;
use crate::widgets::AnimatedF32;

// TODO
// - Rename cursor to oval/rect/bar/grabber/grabbybar
// - Rename progress to something more descriptive
// - Document names
// - Document invariants

/// A scrollbar.
///
#[doc = concat!(
    "![Vertical scrollbar](",
    include_doc_path!("screenshots/scrollbar_default.png"),
    ")",
)]
/// This widget does not directly scroll any content. Instead, it exposes its position via
/// [`cursor_progress`](Self::cursor_progress) and emits [`ScrollBarMoved`] whenever user input
/// changes that position. Parent scroll containers consume the action and synchronize their viewport.
///
/// ## Keyboard and accessibility
///
/// Scrollbars are focusable and support basic keyboard navigation (arrow keys, PageUp/Down,
/// Home/End depending on axis), and expose scroll state in the accessibility tree.
pub struct ScrollBar {
    axis: Axis,
    cursor_progress: f64,
    portal_size: Length,
    content_size: Length,
    focus_owner: Option<WidgetId>,
    pub(crate) opacity: AnimatedF32,
    grab_anchor: Option<f64>,
}

// --- MARK: BUILDERS
impl ScrollBar {
    /// Creates a new scrollbar.
    ///
    /// - `portal_size`: Size of the scrolling container in the relevant axis.
    /// - `content_size`: Size of the child in the relevant axis. Usually exceeds `portal_size` in cases where the scrollbar is visible.
    #[must_use = "Use the returned value."]
    pub const fn new(axis: Axis, portal_size: Length, content_size: Length) -> Self {
        Self {
            axis,
            cursor_progress: 0.0,
            focus_owner: None,
            portal_size,
            content_size,
            opacity: AnimatedF32::stable(1.),
            grab_anchor: None,
        }
    }

    /// Keeps keyboard focus on the specified owner instead of this scrollbar.
    /// Useful in focus-managed popups whose keyboard navigation belongs to the owner.
    #[must_use = "Use the configured scrollbar."]
    pub const fn with_focus_owner(mut self, owner: WidgetId) -> Self {
        self.focus_owner = Some(owner);
        self
    }
}

// --- MARK: METHODS
impl ScrollBar {
    /// Returns how far the scrollbar is from its initial point.
    ///
    /// Values range from 0.0 (beginning) to 1.0 (end).
    #[must_use = "Use the returned value."]
    pub const fn cursor_progress(&self) -> f64 {
        self.cursor_progress
    }

    /// Returns `(cursor_length, empty_space_length)`.
    ///
    /// `cursor_Length` is guaranteed to be at least `min_length`
    /// and the remainder of the layout length is `empty_space_length`.
    fn lengths(&self, layout_size: Size, min_length: f64) -> (f64, f64) {
        let size_ratio = if self.content_size == Length::ZERO {
            1.
        } else {
            self.portal_size.get() / self.content_size.get()
        };
        let size_ratio = size_ratio.clamp(0.0, 1.0);

        let track = layout_size.get_coord(self.axis).max(0.);
        let cursor_length = (size_ratio * track).max(min_length).min(track);
        let empty_space_length = track - cursor_length;

        (cursor_length, empty_space_length)
    }

    fn cursor_rect(&self, layout_size: Size, min_length: f64) -> Rect {
        // TODO - handle invalid sizes
        let (cursor_length, empty_space_length) = self.lengths(layout_size, min_length);

        let cursor_pos_major = self.cursor_progress * empty_space_length;
        let cursor_pos = self.axis.pack_point(cursor_pos_major, 0.0);
        let cursor_size_minor = layout_size.get_coord(self.axis.cross());
        let cursor_size = self.axis.pack_size(cursor_length, cursor_size_minor);

        Rect::from_origin_size(cursor_pos, cursor_size)
    }

    fn progress_from_mouse_pos(
        &self,
        layout_size: Size,
        min_length: f64,
        anchor: f64,
        mouse_pos: Point,
    ) -> f64 {
        // TODO - handle invalid sizes
        let (cursor_length, empty_space_length) = self.lengths(layout_size, min_length);

        if empty_space_length <= 0. {
            return self.cursor_progress;
        }
        let new_cursor_pos_major = anchor.mul_add(-cursor_length, mouse_pos.get_coord(self.axis));
        let new_cursor_progress = new_cursor_pos_major / empty_space_length;

        new_cursor_progress.clamp(0.0, 1.0)
    }

    const fn scroll_range(&self) -> f64 {
        self.content_size.saturating_sub(self.portal_size).get()
    }

    fn set_cursor_progress(&mut self, new_progress: f64) -> bool {
        let new_progress = if self.scroll_range() <= 0.0 {
            0.0
        } else if new_progress.is_finite() {
            new_progress.clamp(0.0, 1.0)
        } else {
            return false;
        };
        if new_progress.to_bits() == self.cursor_progress.to_bits() {
            false
        } else {
            self.cursor_progress = new_progress;
            true
        }
    }

    fn adjust_by_pixels(&mut self, delta_pixels: f64) -> bool {
        let scroll_range = self.scroll_range();
        if scroll_range <= 1e-12 {
            return false;
        }
        // Translate from a pixel delta in content space to a normalized cursor progress delta.
        self.set_cursor_progress(self.cursor_progress + delta_pixels / scroll_range)
    }

    /// Synchronizes model state without generating a user action. The parent
    /// may use this during layout, or after consuming an input action.
    pub(crate) fn synchronize(&mut self, portal: Length, content: Length, progress: f64) -> bool {
        let sizes_changed = self.portal_size != portal || self.content_size != content;
        self.portal_size = portal;
        self.content_size = content;
        let progress_changed = self.set_cursor_progress(progress);
        sizes_changed || progress_changed
    }

    pub(crate) fn synchronize_raw(
        child: Option<(&mut Self, RawCtx<'_>)>,
        portal: f64,
        content: f64,
        progress: f64,
    ) {
        if let Some((bar, mut ctx)) = child
            && bar.synchronize(Length::px(portal), Length::px(content), progress)
        {
            ctx.request_render();
        }
    }

    fn emit_movement(&self, ctx: &mut EventCtx<'_>) {
        ctx.request_render();
        ctx.submit_action::<ScrollBarMoved>(ScrollBarMoved(self.cursor_progress));
        ctx.set_handled();
    }
}

// --- MARK: WIDGETMUT
impl ScrollBar {
    /// Sets the viewport size, content size and normalized scroll position.
    /// This is a model update and does not emit an action.
    pub fn set_state(
        this: &mut WidgetMut<'_, Self>,
        portal: Length,
        content: Length,
        progress: f64,
    ) {
        let size_changed = this.widget.portal_size != portal || this.widget.content_size != content;
        if this.widget.synchronize(portal, content, progress) {
            if size_changed {
                this.ctx.request_layout();
            } else {
                this.ctx.request_render();
            }
        }
    }
}

/// User-requested scrollbar movement, expressed as normalized progress in 0..=1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollBarMoved(pub f64);

impl UsesProperty<Collapsible> for ScrollBar {}

// --- MARK: IMPL WIDGET
impl Widget for ScrollBar {
    type Action = ScrollBarMoved;

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        match event {
            PointerEvent::Down(PointerButtonEvent {
                button: None | Some(PointerButton::Primary),
                state,
                ..
            }) => {
                if let Some(owner) = self.focus_owner {
                    ctx.set_focus(owner);
                } else {
                    ctx.request_focus();
                }
                ctx.capture_pointer();
                ctx.set_handled();

                let size = ctx.content_box().size();
                let cursor_min_length = theme::SCROLLBAR_MIN_SIZE;
                let cursor_rect = self.cursor_rect(size, cursor_min_length);
                let mouse_pos = ctx.local_position(state.position);
                let mut changed = false;
                if cursor_rect.contains(mouse_pos) {
                    let (c0, c1) = cursor_rect.get_coords(self.axis);
                    let mouse_major = mouse_pos.get_coord(self.axis);
                    self.grab_anchor = Some(if c1 > c0 {
                        ((mouse_major - c0) / (c1 - c0)).clamp(0., 1.)
                    } else {
                        0.5
                    });
                } else {
                    let progress =
                        self.progress_from_mouse_pos(size, cursor_min_length, 0.5, mouse_pos);
                    changed |= self.set_cursor_progress(progress);
                    self.grab_anchor = Some(0.5);
                }
                if changed {
                    self.emit_movement(ctx);
                } else {
                    ctx.request_paint_only();
                }
            }
            PointerEvent::Move(PointerUpdate { current, .. }) => {
                if ctx.is_pointer_capture_target()
                    && let Some(grab_anchor) = self.grab_anchor
                {
                    let size = ctx.content_box().size();
                    let cursor_min_length = theme::SCROLLBAR_MIN_SIZE;
                    let progress = self.progress_from_mouse_pos(
                        size,
                        cursor_min_length,
                        grab_anchor,
                        ctx.local_position(current.position),
                    );
                    if self.set_cursor_progress(progress) {
                        self.emit_movement(ctx);
                    }
                    ctx.set_handled();
                }
            }
            PointerEvent::Up(PointerButtonEvent { state, .. })
                if ctx.is_pointer_capture_target() =>
            {
                if let Some(anchor) = self.grab_anchor.take() {
                    let progress = self.progress_from_mouse_pos(
                        ctx.content_box().size(),
                        theme::SCROLLBAR_MIN_SIZE,
                        anchor,
                        ctx.local_position(state.position),
                    );
                    if self.set_cursor_progress(progress) {
                        self.emit_movement(ctx);
                    }
                }
                ctx.request_paint_only();
                ctx.set_handled();
            }
            PointerEvent::Cancel(..) => {
                self.grab_anchor = None;
                ctx.request_paint_only();
            }
            _ => {}
        }
    }

    fn on_text_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &TextEvent,
    ) {
        let TextEvent::Keyboard(event) = event else {
            return;
        };
        if event.state != KeyState::Down || !ctx.is_focus_target() {
            return;
        }
        let line = 120.0;
        let page = self.portal_size.get();

        let changed = match (&event.key, self.axis) {
            (Key::Named(NamedKey::ArrowUp), Axis::Vertical)
            | (Key::Named(NamedKey::ArrowLeft), Axis::Horizontal) => self.adjust_by_pixels(-line),
            (Key::Named(NamedKey::ArrowDown), Axis::Vertical)
            | (Key::Named(NamedKey::ArrowRight), Axis::Horizontal) => self.adjust_by_pixels(line),
            (Key::Named(NamedKey::PageUp), Axis::Vertical) => self.adjust_by_pixels(-page),
            (Key::Named(NamedKey::PageDown), Axis::Vertical) => self.adjust_by_pixels(page),
            (Key::Named(NamedKey::Home), _) => self.set_cursor_progress(0.),
            (Key::Named(NamedKey::End), _) => self.set_cursor_progress(1.),
            _ => false,
        };
        if changed {
            self.emit_movement(ctx);
        }
    }

    fn on_access_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &AccessEvent,
    ) {
        if !matches!(
            event.action,
            accesskit::Action::ScrollUp
                | accesskit::Action::ScrollDown
                | accesskit::Action::ScrollLeft
                | accesskit::Action::ScrollRight
        ) {
            return;
        }

        let action_matches_axis = matches!(
            (event.action, self.axis),
            (
                accesskit::Action::ScrollUp | accesskit::Action::ScrollDown,
                Axis::Vertical
            ) | (
                accesskit::Action::ScrollLeft | accesskit::Action::ScrollRight,
                Axis::Horizontal
            )
        );
        if !action_matches_axis {
            return;
        }
        let unit = if let Some(accesskit::ActionData::ScrollUnit(unit)) = &event.data {
            *unit
        } else {
            accesskit::ScrollUnit::Item
        };
        let line = 120.0;
        let page = self.portal_size.get();
        let amount = match unit {
            accesskit::ScrollUnit::Item => line,
            accesskit::ScrollUnit::Page => page,
        };
        let signed = match event.action {
            accesskit::Action::ScrollUp | accesskit::Action::ScrollLeft => -amount,
            accesskit::Action::ScrollDown | accesskit::Action::ScrollRight => amount,
            _ => 0.0,
        };

        if self.adjust_by_pixels(signed) {
            self.emit_movement(ctx);
        }
    }

    fn on_anim_frame(
        &mut self,
        ctx: &mut UpdateCtx<'_>,
        props: &mut PropertiesMut<'_>,
        interval: u64,
    ) {
        let cache = ctx.property_cache();
        let collapsible = props.get::<Collapsible>(cache).0;

        if !collapsible {
            self.opacity.move_to(1., 0.);
            ctx.request_paint_only();
            return;
        }

        let result = self.opacity.advance_nanos(interval);
        ctx.request_paint_only();

        if !result.is_completed() {
            ctx.request_anim_frame();
        }
    }

    fn register_children(&mut self, _ctx: &mut RegisterCtx<'_>) {}

    fn property_changed(&mut self, ctx: &mut UpdateCtx<'_>, property_type: TypeId) {
        if Collapsible::matches(property_type) {
            ctx.request_paint_only();
        }
    }

    fn measure(
        &mut self,
        _ctx: &mut MeasureCtx<'_>,
        _props: &PropertiesRef<'_>,
        axis: Axis,
        len_req: LenReq,
        _cross_length: Option<Length>,
    ) -> Length {
        if axis == self.axis {
            // TODO: Consider .max(theme::SCROLLBAR_MIN_SIZE)
            match len_req {
                LenReq::Min | LenReq::Max => self.portal_size,
                LenReq::Fit(space) => space,
            }
        } else {
            let scrollbar_width = theme::SCROLLBAR_WIDTH;
            let cursor_padding = theme::SCROLLBAR_PAD;

            cursor_padding.mul_add(2.0, scrollbar_width).px()
        }
    }

    fn layout(&mut self, _ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, _size: Size) {}

    fn paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        let cache = ctx.property_cache();
        let collapsible = props.get::<Collapsible>(cache).0;

        let opacity = if ctx.is_active() || ctx.is_hovered() {
            1.0
        } else {
            self.opacity.value()
        };
        if opacity < 1. {
            painter.push_fill_clip(ctx.border_box());
            painter.push_group(
                GroupRef::new()
                    .with_composite(Composite::new(crate::peniko::BlendMode::default(), opacity)),
            );
        }

        let radius = theme::SCROLLBAR_RADIUS;
        let edge_width = theme::SCROLLBAR_EDGE_WIDTH;
        let cursor_padding = theme::SCROLLBAR_PAD;
        let cursor_min_length = theme::SCROLLBAR_MIN_SIZE;
        let scrollbar_width = theme::SCROLLBAR_WIDTH;

        let size = ctx.content_box().size();
        let inset_start = if !collapsible || ctx.is_hovered() || self.grab_anchor.is_some() {
            cursor_padding
        } else {
            cursor_padding + scrollbar_width / 2.
        };
        let (leading_x, leading_y) = self.axis.pack_xy(0.0, inset_start);
        let (trailing_x, trailing_y) = self.axis.pack_xy(0.0, cursor_padding);
        let cursor_rect = self
            .cursor_rect(size, cursor_min_length)
            .inset((-leading_x, -leading_y, -trailing_x, -trailing_y))
            .to_rounded_rect(radius);

        painter.fill(cursor_rect, theme::SCROLLBAR_COLOR).draw();
        painter
            .stroke(
                cursor_rect,
                &Stroke::new(edge_width),
                theme::SCROLLBAR_BORDER_COLOR,
            )
            .draw();

        if opacity < 1. {
            painter.pop_group();
            painter.pop_clip();
        }
    }

    fn accessibility_role(&self) -> Role {
        Role::ScrollBar
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        node.set_orientation(match self.axis {
            Axis::Horizontal => accesskit::Orientation::Horizontal,
            Axis::Vertical => accesskit::Orientation::Vertical,
        });

        let scroll_range = self.scroll_range();

        let value = (self.cursor_progress.clamp(0.0, 1.0)) * scroll_range;
        match self.axis {
            Axis::Horizontal => {
                node.set_scroll_x_min(0.0);
                node.set_scroll_x_max(scroll_range);
                node.set_scroll_x(value);
                if self.cursor_progress > 1e-12 {
                    node.add_action(accesskit::Action::ScrollLeft);
                }
                if scroll_range > 0.0 && self.cursor_progress + 1e-12 < 1.0 {
                    node.add_action(accesskit::Action::ScrollRight);
                }
            }
            Axis::Vertical => {
                node.set_scroll_y_min(0.0);
                node.set_scroll_y_max(scroll_range);
                node.set_scroll_y(value);
                if self.cursor_progress > 1e-12 {
                    node.add_action(accesskit::Action::ScrollUp);
                }
                if scroll_range > 0.0 && self.cursor_progress + 1e-12 < 1.0 {
                    node.add_action(accesskit::Action::ScrollDown);
                }
            }
        }
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::new()
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("ScrollBar", id = id.trace())
    }

    fn accepts_focus(&self) -> bool {
        self.focus_owner.is_none()
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        if matches!(
            event,
            Update::DisabledChanged(true) | Update::StashedChanged(true)
        ) {
            self.grab_anchor = None;
        }
        if matches!(event, Update::HoveredChanged(_) | Update::ActiveChanged(_)) {
            ctx.request_paint_only();
        }
    }
}

impl AllowRawMut for ScrollBar {}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::keyboard::{Key, NamedKey};
    use crate::core::{NewWidget, TextEvent};
    use crate::properties::Dimensions;
    use crate::testing::{TestHarness, assert_render_snapshot};
    use crate::theme::test_property_set;

    fn centered_scrollbar(
        axis: Axis,
        size: (u32, u32),
        before: &str,
        after: &str,
    ) -> TestHarness<ScrollBar> {
        let widget =
            NewWidget::new(ScrollBar::new(axis, 200.px(), 600.px())).with_props(Dimensions::FIT);
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, size);
        let scrollbar_id = harness.root_id();
        assert_render_snapshot!(harness, before);
        assert!(harness.pop_action_erased().is_none());
        harness.mouse_click_on(scrollbar_id, None).unwrap();
        let (action, source) = harness.pop_action::<ScrollBarMoved>().unwrap().unwrap();
        assert_eq!(source, scrollbar_id);
        assert!((action.0 - 0.5).abs() < 1e-9);
        assert_render_snapshot!(harness, after);
        harness
    }

    #[test]
    fn simple_scrollbar() {
        let mut harness = centered_scrollbar(
            Axis::Vertical,
            (50, 200),
            "scrollbar_default",
            "scrollbar_middle",
        );
        harness.mouse_button_press(None);
        harness.mouse_move(Point::new(30.0, 150.0));

        assert_render_snapshot!(harness, "scrollbar_down");

        harness.mouse_move(Point::new(30.0, 300.0));
        assert_render_snapshot!(harness, "scrollbar_bottom");
    }

    #[test]
    fn collapsible_scrollbar() {
        let widget = NewWidget::new(ScrollBar::new(Axis::Vertical, 200.px(), 600.px()))
            .with_props(Collapsible(true))
            .with_props(Dimensions::FIT);

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (20, 200));

        assert_render_snapshot!(harness, "scrollbar_collapsed");
    }

    #[test]
    fn horizontal_scrollbar() {
        centered_scrollbar(
            Axis::Horizontal,
            (200, 50),
            "scrollbar_horizontal",
            "scrollbar_horizontal_middle",
        );
    }

    #[test]
    fn model_updates_do_not_emit_actions_and_empty_range_clears_progress() {
        let widget = ScrollBar::new(Axis::Vertical, 100.px(), 300.px())
            .prepare()
            .with_props(Dimensions::FIT);
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (20, 100));
        let id = harness.root_id();
        harness
            .edit_root_widget(|mut bar| {
                ScrollBar::set_state(&mut bar, 100.px(), 300.px(), 0.75);
            })
            .unwrap();
        assert!(harness.pop_action_erased().is_none());
        harness.focus_on(Some(id));
        harness.process_text_event(&TextEvent::key_down(Key::Named(NamedKey::End)));
        assert_eq!(
            harness.pop_action::<ScrollBarMoved>().unwrap(),
            Some((ScrollBarMoved(1.0), id))
        );
        harness
            .edit_root_widget(|mut bar| {
                ScrollBar::set_state(&mut bar, 300.px(), 300.px(), f64::NAN);
            })
            .unwrap();
        assert!(harness.pop_action_erased().is_none());
        assert!(harness.root_widget().unwrap().cursor_progress().abs() < 1e-12);
        let _ = harness.redraw();
        assert!(
            !harness
                .access_node(id)
                .unwrap()
                .data()
                .supports_action(accesskit::Action::ScrollDown)
        );
        harness
            .mouse_click_on(id, Some(PointerButton::Secondary))
            .unwrap();
        assert!(harness.pop_action_erased().is_none());
    }

    #[test]
    fn keyboard_scroll_updates_access_tree() {
        let widget = NewWidget::new(ScrollBar::new(Axis::Vertical, 200.px(), 600.px()))
            .with_props(Dimensions::FIT);
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (50, 200));
        let _ = harness.render().unwrap();

        let scrollbar_id = harness.root_id();
        harness.focus_on(Some(scrollbar_id));

        harness.process_text_event(&TextEvent::key_down(Key::Named(NamedKey::ArrowDown)));
        let _ = harness.render().unwrap();

        let node = harness.access_node(scrollbar_id).unwrap();
        assert!(node.data().scroll_y().unwrap_or(0.0) > 0.0);
    }

    // TODO - Add "portal larger than content" test

    // TODO - Add WidgetMut tests
}
