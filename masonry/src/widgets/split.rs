// Copyright 2019 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

use accesskit::{ActionData, Node, Role};
use include_doc_path::include_doc_path;
use tracing::{Span, trace_span};

use crate::core::keyboard::{Key, NamedKey};
use crate::core::{
    AccessCtx, AccessEvent, ChildrenIds, CursorIcon, EventCtx, FromDynWidget, LayoutCtx,
    MeasureCtx, NewWidget, NoAction, PaintCtx, PointerButton, PointerButtonEvent, PointerEvent,
    PointerState, PointerUpdate, PropertiesMut, PropertiesRef, QueryCtx, RegisterCtx, TextEvent,
    Update, UpdateCtx, Widget, WidgetId, WidgetMut, WidgetPod,
};
use crate::imaging::Painter;
use crate::kurbo::{Axis, Join, Line, Point, Size, Stroke};
use crate::layout::{AsUnit, LayoutSize, LenReq, Length};
use crate::peniko::Color;
use crate::theme;

/// The split point, specifying how the available space is divided between the two children.
///
/// This always applies to the *available space*, which is the widget's size along the split axis
/// minus the splitter bar thickness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SplitPoint {
    /// Split by a fraction of the available space.
    ///
    /// `0.0` means the first child gets no space, `1.0` means the second child gets no space.
    /// Values outside `0.0..=1.0` are clamped when set.
    Fraction(f64),
    /// Split by an absolute distance from the start.
    FromStart(Length),
    /// Split by an absolute distance from the end.
    FromEnd(Length),
}

impl SplitPoint {
    /// Normalizes fractional positions, clamping infinities and using the
    /// midpoint for NaN. Fixed-length positions are already validated.
    #[must_use = "Use the normalized split point."]
    pub const fn normalized(self) -> Self {
        match self {
            Self::Fraction(value) => Self::Fraction(if value.is_nan() {
                0.5
            } else {
                value.clamp(0.0, 1.0)
            }),
            other => other,
        }
    }
}

#[derive(Clone, Copy)]
struct SplitGeometry {
    space: f64,
    bar: f64,
    min: f64,
    max: f64,
    first: f64,
}

impl SplitGeometry {
    const fn new(total: f64, bar: Length, minimums: (Length, Length), point: SplitPoint) -> Self {
        let bar = bar.get().min(total);
        let space = total - bar;
        let (first, second) = (minimums.0.get(), minimums.1.get());
        let max = (space - second).max(0.0);
        let (min, max) = if first <= max && second <= space {
            (first, max)
        } else {
            // Proportionally compress infeasible minima. Scaling before the
            // sum prevents overflow even for extremely large finite lengths.
            let scale = first.max(second);
            let ratio = if scale > 0.0 {
                (first / scale) / (first / scale + second / scale)
            } else {
                0.5
            };
            let fixed = space * ratio;
            (fixed, fixed)
        };
        let requested = match point {
            SplitPoint::Fraction(fraction) => space * fraction,
            SplitPoint::FromStart(length) => length.get(),
            SplitPoint::FromEnd(length) => space - length.get(),
        };
        Self {
            space,
            bar,
            min,
            max,
            first: requested.clamp(min, max),
        }
    }

    const fn bar_edges(self) -> (f64, f64) {
        (self.first, self.first + self.bar)
    }

    const fn center(self) -> f64 {
        self.bar.mul_add(0.5, self.first)
    }

    const fn hit(self, minimum_area: Length, pos: f64) -> bool {
        let padding = (minimum_area.get() - self.bar).max(0.0);
        let second = self.space - self.first;
        let pad1 = (padding * 0.5).min(self.first);
        let pad2 = (padding - pad1).min(second);
        let pad1 = (padding - pad2).min(self.first);
        let (edge1, edge2) = self.bar_edges();
        pos >= edge1 - pad1 && pos <= edge2 + pad2
    }

    const fn step(self) -> f64 {
        (self.space / 100.0).max(1.0)
    }

    const fn movable(self) -> bool {
        self.min < self.max
    }
}

/// A container containing two other widgets, splitting the area either horizontally or vertically.
///
#[doc = concat!(
    "![Split panel with two labels](",
    include_doc_path!("screenshots/split_columns.png"),
    ")",
)]
pub struct Split<ChildA, ChildB>
where
    ChildA: Widget + ?Sized,
    ChildB: Widget + ?Sized,
{
    axis: Axis,
    chosen_point: SplitPoint,
    min_lengths: (Length, Length),
    bar_thickness: Length,
    min_bar_area: Length,
    solid: bool,
    draggable: bool,
    /// Offset from the bar center to the actual position where the bar was clicked.
    /// This is used to ensure a click without moving is a no-op,
    /// instead of re-centering the bar to the click position.
    click_offset: Option<f64>,
    child1: WidgetPod<ChildA>,
    child2: WidgetPod<ChildB>,
}

// --- MARK: BUILDERS
impl<ChildA: Widget + ?Sized, ChildB: Widget + ?Sized> Split<ChildA, ChildB> {
    /// Creates a new split panel.
    #[must_use = "Use the returned value."]
    pub fn new(child1: NewWidget<ChildA>, child2: NewWidget<ChildB>) -> Self {
        Self {
            axis: Axis::Horizontal,
            chosen_point: SplitPoint::Fraction(0.5),
            min_lengths: (Length::ZERO, Length::ZERO),
            bar_thickness: 6.px(),
            min_bar_area: 6.px(),
            solid: false,
            draggable: true,
            click_offset: None,
            child1: child1.into_pod(),
            child2: child2.into_pod(),
        }
    }

    /// Builder-style method to set the split axis.
    ///
    /// Horizontal split axis means that the children are left and right.
    /// Vertical split axis means that the children are up and down.
    ///
    /// The default split point is horizontal.
    #[must_use = "Use the returned value."]
    pub const fn split_axis(mut self, axis: Axis) -> Self {
        self.axis = axis;
        self
    }

    /// Builder-style method to set the split point as a fraction of the split axis.
    ///
    /// The value is clamped to `0.0..=1.0`, with NaN selecting the midpoint.
    ///
    /// The default split point is `0.5`.
    #[must_use = "Use the returned value."]
    pub const fn split_fraction(mut self, split_point: f64) -> Self {
        self.chosen_point = SplitPoint::Fraction(split_point).normalized();
        self
    }

    /// Builder-style method to set the split point.
    #[must_use = "Use the returned value."]
    pub const fn split_point(mut self, split_point: SplitPoint) -> Self {
        self.chosen_point = split_point.normalized();
        self
    }

    /// Builder-style method to set the split point as an absolute distance from the start.
    ///
    /// This is the size of the first child along the split axis.
    /// This can be useful when one side should have a stable pixel size, even when the split
    /// container is resized.
    #[must_use = "Use the returned value."]
    pub const fn split_point_from_start(mut self, split_point: Length) -> Self {
        self.chosen_point = SplitPoint::FromStart(split_point);
        self
    }

    /// Builder-style method to set the split point as an absolute distance from the end.
    ///
    /// This is the size of the second child along the split axis.
    /// This can be useful when one side should have a stable pixel size, even when the split
    /// container is resized.
    #[must_use = "Use the returned value."]
    pub const fn split_point_from_end(mut self, split_point: Length) -> Self {
        self.chosen_point = SplitPoint::FromEnd(split_point);
        self
    }

    /// Builder-style method to set the minimum length for both sides of the split axis.
    #[must_use = "Use the returned value."]
    pub const fn min_lengths(mut self, first: Length, second: Length) -> Self {
        self.min_lengths = (first, second);
        self
    }

    /// Builder-style method to set the thickness of the splitter bar.
    ///
    /// The default splitter bar thickness is `6.0`.
    #[must_use = "Use the returned value."]
    pub const fn bar_thickness(mut self, bar_thickness: Length) -> Self {
        self.bar_thickness = bar_thickness;
        self
    }

    /// Builder-style method to set the minimum thickness of the splitter bar area.
    ///
    /// The minimum splitter bar area defines the minimum thickness of the area
    /// where pointer hit detection is done for the splitter bar.
    /// The final hit detection area thickness is either this minimum
    /// or the splitter bar thickness, whichever is greater.
    ///
    /// This can be useful when you want to use a very narrow visual splitter bar,
    /// but don't want to sacrifice user experience by making it hard to click on.
    ///
    /// The default minimum splitter bar area thickness is `6.0`.
    #[must_use = "Use the returned value."]
    pub const fn min_bar_area(mut self, min_bar_area: Length) -> Self {
        self.min_bar_area = min_bar_area;
        self
    }

    /// Builder-style method to set whether the split point can be changed by dragging.
    #[must_use = "Use the returned value."]
    pub const fn draggable(mut self, draggable: bool) -> Self {
        self.draggable = draggable;
        self
    }

    /// Builder-style method to set whether the splitter bar is drawn as a solid rectangle.
    ///
    /// If this is `false` (the default), the bar will be drawn as two parallel lines.
    #[must_use = "Use the returned value."]
    pub const fn solid_bar(mut self, solid: bool) -> Self {
        self.solid = solid;
        self
    }
}

// --- MARK: METHODS
impl<ChildA: Widget + ?Sized, ChildB: Widget + ?Sized> Split<ChildA, ChildB> {
    const fn geometry(&self, total: f64) -> SplitGeometry {
        SplitGeometry::new(
            total,
            self.bar_thickness,
            self.min_lengths,
            self.chosen_point,
        )
    }

    // Input changes the requested policy only when effective geometry moves.
    // A no-op at a constrained endpoint must not discard the preferred size.
    fn move_split(&mut self, ctx: &mut EventCtx<'_>, geometry: SplitGeometry, first: f64) {
        if !first.is_finite() || !geometry.movable() {
            return;
        }
        let first = first.clamp(geometry.min, geometry.max);
        if first.to_bits() == geometry.first.to_bits() {
            return;
        }
        self.chosen_point = match self.chosen_point {
            SplitPoint::Fraction(_) => SplitPoint::Fraction(first / geometry.space),
            SplitPoint::FromStart(_) => SplitPoint::FromStart(first.px()),
            SplitPoint::FromEnd(_) => SplitPoint::FromEnd((geometry.space - first).px()),
        };
        ctx.request_layout();
    }

    fn drag_to(&mut self, ctx: &mut EventCtx<'_>, state: &PointerState) {
        if let Some(offset) = self.click_offset {
            let geometry = self.geometry(ctx.content_box().size().get_coord(self.axis));
            let pos = ctx.local_position(state.position).get_coord(self.axis);
            let first = geometry.bar.mul_add(-0.5, pos - offset);
            self.move_split(ctx, geometry, first);
        }
        ctx.set_handled();
    }

    /// Returns the color of the splitter bar.
    fn bar_color(&self, ctx: &PaintCtx<'_>) -> Color {
        if !self.draggable || ctx.is_disabled() {
            return theme::ZYNC_700;
        }
        if ctx.is_active() || ctx.is_hovered() || ctx.is_focus_target() {
            theme::ZYNC_600
        } else {
            theme::ZYNC_500
        }
    }

    fn paint_focus_bar(&self, ctx: &PaintCtx<'_>, scene: &mut Painter<'_>) {
        let length = ctx.content_box().size().get_coord(self.axis);
        let (edge1, edge2) = self.geometry(length).bar_edges();

        let mut rect = ctx.border_box();
        rect.set_coords(self.axis, edge1, edge2);
        let rect = rect.inset(2.0);

        let focus_color = theme::FOCUS_COLOR.with_alpha(if ctx.is_active() { 1.0 } else { 0.5 });
        let focus_stroke = Stroke::new(1.0).with_join(Join::Miter);

        scene.stroke(rect, &focus_stroke, focus_color).draw();
    }

    fn paint_solid_bar(&self, ctx: &PaintCtx<'_>, scene: &mut Painter<'_>, color: Color) {
        let length = ctx.content_box().size().get_coord(self.axis);
        let (edge1, edge2) = self.geometry(length).bar_edges();

        let mut rect = ctx.border_box();
        rect.set_coords(self.axis, edge1, edge2);

        scene.fill(rect, color).draw();
    }

    fn paint_stroked_bar(&self, ctx: &PaintCtx<'_>, scene: &mut Painter<'_>, color: Color) {
        let length = ctx.content_box().size().get_coord(self.axis);
        // Set the line width to a third of the splitter bar thickness,
        // because we'll paint two equal lines at the edges.
        let line_width = self.geometry(length).bar / 3.0;
        let line_midpoint = line_width / 2.0;
        let (edge1, edge2) = self.geometry(length).bar_edges();

        let edge1_line_pos = edge1 + line_midpoint;
        let edge2_line_pos = edge2 - line_midpoint;

        let border_box = ctx.border_box();
        let (cross1, cross2) = border_box.get_coords(self.axis.cross());

        let line1_p1 = self.axis.pack_point(edge1_line_pos, cross1);
        let line1_p2 = self.axis.pack_point(edge1_line_pos, cross2);

        let line2_p1 = self.axis.pack_point(edge2_line_pos, cross1);
        let line2_p2 = self.axis.pack_point(edge2_line_pos, cross2);

        let (line1, line2) = (Line::new(line1_p1, line1_p2), Line::new(line2_p1, line2_p2));

        let style = Stroke::new(line_width);
        scene.stroke(line1, &style, color).draw();
        scene.stroke(line2, &style, color).draw();
    }
}

// --- MARK: WIDGETMUT
impl<ChildA, ChildB> Split<ChildA, ChildB>
where
    ChildA: Widget + FromDynWidget + ?Sized,
    ChildB: Widget + FromDynWidget + ?Sized,
{
    child_accessors!(child1, set_child1, child1_mut, ChildA);

    child_accessors!(child2, set_child2, child2_mut, ChildB);

    /// Sets the split axis.
    pub fn set_split_axis(this: &mut WidgetMut<'_, Self>, axis: Axis) {
        if this.widget.axis != axis {
            this.widget.axis = axis;
            this.widget.click_offset = None;
            this.ctx.request_layout();
            this.ctx.request_cursor_icon_change();
        }
    }

    /// Sets the split point as a fraction of the split axis.
    ///
    /// The value must be between `0.0` and `1.0`, inclusive.
    /// The default split point is `0.5`.
    pub fn set_split_point(this: &mut WidgetMut<'_, Self>, split_point: SplitPoint) {
        let point = split_point.normalized();
        if this.widget.chosen_point != point {
            this.widget.chosen_point = point;
            this.widget.click_offset = None;
            this.ctx.request_layout();
        }
    }

    /// Sets the minimum lengths for both sides of the split axis.
    pub fn set_min_lengths(this: &mut WidgetMut<'_, Self>, first: Length, second: Length) {
        if this.widget.min_lengths != (first, second) {
            this.widget.min_lengths = (first, second);
            this.ctx.request_layout();
        }
    }

    /// Sets the thickness of the splitter bar.
    ///
    /// The default splitter bar thickness is `6.0`.
    pub fn set_bar_thickness(this: &mut WidgetMut<'_, Self>, bar_thickness: Length) {
        if this.widget.bar_thickness != bar_thickness {
            this.widget.bar_thickness = bar_thickness;
            this.widget.click_offset = None;
            this.ctx.request_layout();
        }
    }

    /// Sets the minimum thickness of the splitter bar area.
    ///
    /// The minimum splitter bar area defines the minimum thickness of the area
    /// where pointer hit detection is done for the splitter bar.
    /// The final hit detection area thickness is either this minimum
    /// or the splitter bar thickness, whichever is greater.
    ///
    /// This can be useful when you want to use a very narrow visual splitter bar,
    /// but don't want to sacrifice user experience by making it hard to click on.
    ///
    /// The default minimum splitter bar area thickness is `6.0`.
    pub fn set_min_bar_area(this: &mut WidgetMut<'_, Self>, min_bar_area: Length) {
        if this.widget.min_bar_area != min_bar_area {
            this.widget.min_bar_area = min_bar_area;
            this.ctx.request_cursor_icon_change();
        }
    }

    /// Sets whether the split point can be changed by dragging.
    pub fn set_draggable(this: &mut WidgetMut<'_, Self>, draggable: bool) {
        if this.widget.draggable != draggable {
            this.widget.draggable = draggable;
            this.widget.click_offset = None;
            this.ctx.set_tab_focusable(draggable);
            this.ctx.request_render();
            this.ctx.request_cursor_icon_change();
        }
    }

    /// Sets whether the splitter bar is drawn as a solid rectangle.
    ///
    /// If this is `false` (the default), the bar will be drawn as two parallel lines.
    pub fn set_bar_solid(this: &mut WidgetMut<'_, Self>, solid: bool) {
        if this.widget.solid != solid {
            this.widget.solid = solid;
            this.ctx.request_paint_only();
        }
    }
}

// --- MARK: IMPL WIDGET
impl<ChildA, ChildB> Widget for Split<ChildA, ChildB>
where
    ChildA: Widget + ?Sized,
    ChildB: Widget + ?Sized,
{
    type Action = NoAction;

    fn accepts_focus(&self) -> bool {
        self.draggable
    }

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        if !self.draggable {
            return;
        }
        match event {
            PointerEvent::Down(PointerButtonEvent {
                button: None | Some(PointerButton::Primary),
                state,
                ..
            }) => {
                let pos = ctx.local_position(state.position).get_coord(self.axis);
                let geometry = self.geometry(ctx.content_box().size().get_coord(self.axis));
                if geometry.movable() && geometry.hit(self.min_bar_area, pos) {
                    ctx.set_handled();
                    ctx.capture_pointer();
                    ctx.request_focus();
                    self.click_offset = Some(pos - geometry.center());
                }
            }
            PointerEvent::Move(PointerUpdate { current, .. })
                if ctx.is_pointer_capture_target() =>
            {
                self.drag_to(ctx, current);
            }
            PointerEvent::Up(PointerButtonEvent { state, .. })
                if ctx.is_pointer_capture_target() =>
            {
                self.drag_to(ctx, state);
                self.click_offset = None;
            }
            PointerEvent::Cancel(..) => self.click_offset = None,
            _ => {}
        }
    }

    fn on_text_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &TextEvent,
    ) {
        if ctx.is_disabled() || !ctx.is_focus_target() || !self.draggable {
            return;
        }

        let TextEvent::Keyboard(key_event) = event else {
            return;
        };
        if !key_event.state.is_down() {
            return;
        }

        let geometry = self.geometry(ctx.content_box().size().get_coord(self.axis));
        let delta = geometry.step()
            * if key_event.modifiers.shift() {
                10.0
            } else {
                1.0
            };
        let first = match key_event.key {
            Key::Named(NamedKey::ArrowLeft) if self.axis == Axis::Horizontal => {
                geometry.first - delta
            }
            Key::Named(NamedKey::ArrowRight) if self.axis == Axis::Horizontal => {
                geometry.first + delta
            }
            Key::Named(NamedKey::ArrowUp) if self.axis == Axis::Vertical => geometry.first - delta,
            Key::Named(NamedKey::ArrowDown) if self.axis == Axis::Vertical => {
                geometry.first + delta
            }
            Key::Named(NamedKey::Home) => geometry.min,
            Key::Named(NamedKey::End) => geometry.max,
            _ => return,
        };
        self.move_split(ctx, geometry, first);
        ctx.set_handled();
    }

    fn on_access_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &AccessEvent,
    ) {
        if ctx.is_disabled() || !self.draggable || ctx.target() != ctx.widget_id() {
            return;
        }
        let geometry = self.geometry(ctx.content_box().size().get_coord(self.axis));
        let first = match event.action {
            accesskit::Action::Increment => geometry.first + geometry.step(),
            accesskit::Action::Decrement => geometry.first - geometry.step(),
            accesskit::Action::SetValue => match &event.data {
                Some(ActionData::NumericValue(value)) => *value,
                Some(ActionData::Value(value)) => {
                    let Ok(value) = value.parse() else {
                        return;
                    };
                    value
                }
                _ => return,
            },
            _ => return,
        };
        self.move_split(ctx, geometry, first);
        ctx.set_handled();
    }

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        ctx.register_child(&mut self.child1);
        ctx.register_child(&mut self.child2);
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        match event {
            Update::FocusChanged(false)
            | Update::DisabledChanged(true)
            | Update::StashedChanged(true)
            | Update::ActiveChanged(false) => {
                self.click_offset = None;
                ctx.request_paint_only();
            }
            Update::FocusChanged(true)
            | Update::HoveredChanged(_)
            | Update::ActiveChanged(true)
            | Update::DisabledChanged(false) => {
                ctx.request_paint_only();
            }
            _ => {}
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
        if let LenReq::Fit(space) = len_req {
            // We always want to use up all offered space
            if axis == self.axis {
                // Don't go below the bar thickness, which we always want to paint.
                return space.max(self.bar_thickness);
            }
            return space;
        }
        // Both children can share the same auto length, because it'll be either Min or MaxContent.
        let auto_length = len_req.into();

        let cross = axis.cross();
        let (child1_cross_space, child2_cross_space) = cross_length
            .map(|cross_length| {
                // We need to split the cross length if it's our split axis
                if cross == self.axis {
                    let geometry = self.geometry(cross_length.get());
                    (geometry.first.px(), (geometry.space - geometry.first).px())
                } else {
                    (cross_length, cross_length)
                }
            })
            .unzip();
        let child1_context_size = LayoutSize::maybe(cross, child1_cross_space);
        let child2_context_size = LayoutSize::maybe(cross, child2_cross_space);

        let child1_length = ctx.compute_length(
            &self.child1,
            auto_length,
            child1_context_size,
            axis,
            child1_cross_space,
        );
        let child2_length = ctx.compute_length(
            &self.child2,
            auto_length,
            child2_context_size,
            axis,
            child2_cross_space,
        );

        if axis == self.axis {
            child1_length
                .saturating_add(child2_length)
                .saturating_add(self.bar_thickness)
        } else {
            child1_length.max(child2_length)
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, size: Size) {
        let geometry = self.geometry(size.get_coord(self.axis));
        let cross_space = size.get_coord(self.axis.cross());
        let child1_split_space = geometry.first;
        let child2_split_space = geometry.space - geometry.first;

        let child1_size = self.axis.pack_size(child1_split_space, cross_space);
        let child2_size = self.axis.pack_size(child2_split_space, cross_space);

        ctx.run_layout(&self.child1, child1_size);
        ctx.run_layout(&self.child2, child2_size);

        // Top-left align both children.
        let child1_origin = Point::ORIGIN;
        let child2_origin = self.axis.pack_point(child1_split_space + geometry.bar, 0.);
        ctx.place_child(&self.child1, child1_origin);
        ctx.place_child(&self.child2, child2_origin);
    }

    fn paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        _props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        // TODO - Paint differently if the bar is draggable and hovered.
        let geometry = self.geometry(ctx.content_box().size().get_coord(self.axis));
        if geometry.bar <= 0.0 {
            return;
        }
        let bar_color = self.bar_color(ctx);
        if self.solid {
            self.paint_solid_bar(ctx, painter, bar_color);
        } else {
            self.paint_stroked_bar(ctx, painter, bar_color);
        }

        if ctx.is_focus_target() && self.draggable && geometry.movable() && !ctx.is_disabled() {
            self.paint_focus_bar(ctx, painter);
        }
        // TODO: Child painting should probably be clipped, in such a way that
        //       one child won't overflow across the split bar onto the other child.
        //       Although that will only happen if we are sized below our MinContent.
    }

    fn get_cursor(&self, ctx: &QueryCtx<'_>, pos: Point) -> CursorIcon {
        let length = ctx.content_box().size().get_coord(self.axis);
        let local_pos = ctx.window_to_local(pos).get_coord(self.axis);
        let geometry = self.geometry(length);
        let is_bar_area_hovered = geometry.hit(self.min_bar_area, local_pos);

        if self.draggable
            && !ctx.is_disabled()
            && geometry.movable()
            && (self.click_offset.is_some() || is_bar_area_hovered)
        {
            match self.axis {
                Axis::Horizontal => CursorIcon::EwResize,
                Axis::Vertical => CursorIcon::NsResize,
            }
        } else {
            CursorIcon::Default
        }
    }

    fn accessibility_role(&self) -> Role {
        Role::Splitter
    }

    fn accessibility(
        &mut self,
        ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        let geometry = self.geometry(ctx.content_box().size().get_coord(self.axis));

        node.set_orientation(match self.axis {
            Axis::Horizontal => accesskit::Orientation::Horizontal,
            Axis::Vertical => accesskit::Orientation::Vertical,
        });
        node.set_value(geometry.first.to_string());
        node.set_numeric_value(geometry.first);
        node.set_min_numeric_value(geometry.min);
        node.set_max_numeric_value(geometry.max);
        node.set_numeric_value_step(geometry.step());

        if self.draggable && !ctx.is_disabled() && geometry.movable() {
            node.add_action(accesskit::Action::SetValue);
            if geometry.first < geometry.max {
                node.add_action(accesskit::Action::Increment);
            }
            if geometry.first > geometry.min {
                node.add_action(accesskit::Action::Decrement);
            }
        }
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::from_slice(&[self.child1.id(), self.child2.id()])
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("Split", id = id.trace())
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{TextEvent, WindowEvent};
    use crate::dpi::PhysicalSize;
    use crate::properties::Padding;
    use crate::testing::{TestHarness, assert_render_snapshot};
    use crate::theme::test_property_set;
    use crate::widgets::Label;

    #[test]
    fn geometry_respects_allocations_and_infeasible_minima() {
        for total in [0.0, 2.0, 6.0, 56.0, 206.0, f64::MAX] {
            for minimums in [
                (80.px(), 20.px()),
                (0.px(), 100.px()),
                (Length::ZERO, Length::ZERO),
                (Length::px(f64::MAX), Length::px(f64::MAX)),
            ] {
                for point in [
                    SplitPoint::Fraction(0.0),
                    SplitPoint::Fraction(0.8),
                    SplitPoint::Fraction(1.0),
                    SplitPoint::FromStart(50.px()),
                    SplitPoint::FromEnd(30.px()),
                ] {
                    let geometry = SplitGeometry::new(total, 6.px(), minimums, point);
                    assert!(
                        geometry.min >= 0.0
                            && geometry.max <= geometry.space
                            && geometry.min <= geometry.first
                            && geometry.first <= geometry.max,
                        "invalid split bounds for total {total}, minima {minimums:?}, point {point:?}"
                    );
                    let (start, end) = geometry.bar_edges();
                    assert!(
                        start >= 0.0 && end <= total && end >= start,
                        "bar must fit inside allocation"
                    );
                    assert!((geometry.space + geometry.bar - total).abs() <= total * f64::EPSILON);
                }
            }
        }
        let geometry =
            SplitGeometry::new(56.0, 6.px(), (80.px(), 20.px()), SplitPoint::Fraction(0.8));
        assert_eq!(
            geometry.first.to_bits(),
            40.0_f64.to_bits(),
            "infeasible minima compress proportionally"
        );
        assert!(!geometry.movable());
    }

    #[test]
    fn resize_preserves_requested_policy_and_compact_geometry() {
        use crate::properties::Dimensions;
        use crate::widgets::SizedBox;
        let widget = Split::new(SizedBox::empty().prepare(), SizedBox::empty().prepare())
            .min_lengths(80.px(), 20.px())
            .split_fraction(0.8)
            .prepare()
            .with_props(Dimensions::FIT);
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (56, 40));
        let id = harness.root_id();
        let check = |harness: &TestHarness<Split<SizedBox, SizedBox>>, expected: [f64; 2]| {
            for (child, expected) in harness
                .root_widget()
                .unwrap()
                .children()
                .iter()
                .zip(expected)
            {
                assert!(
                    (child.ctx().border_box().width() - expected).abs() < 1e-8,
                    "actual pane width differs from expected {expected}"
                );
            }
        };
        check(&harness, [40.0, 10.0]);
        let _ = harness.redraw();
        assert!(
            !harness
                .access_node(id)
                .unwrap()
                .data()
                .supports_action(accesskit::Action::SetValue)
        );
        harness.process_window_event(WindowEvent::Resize(PhysicalSize::new(2, 40)));
        check(&harness, [0.0, 0.0]);
        let _ = harness.redraw();
        harness.process_window_event(WindowEvent::Resize(PhysicalSize::new(206, 40)));
        check(&harness, [160.0, 40.0]);
        assert_eq!(
            harness.root_widget().unwrap().chosen_point,
            SplitPoint::Fraction(0.8),
            "constraint must not overwrite preferred point"
        );
    }

    #[test]
    fn drag_release_and_focusability_follow_current_state() {
        use crate::testing::PRIMARY_MOUSE;
        use accesskit::{Action, ActionRequest, TreeId};
        let widget = Split::new(
            Label::new("First").prepare(),
            Label::new("Second").prepare(),
        )
        .draggable(false)
        .prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (150, 100));
        let id = harness.root_id();
        let focus_action = |action| ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: id.into(),
            data: None,
        };
        let _ = harness.redraw();
        assert!(
            !harness
                .access_node(id)
                .unwrap()
                .data()
                .supports_action(Action::Focus)
        );
        harness.process_access_event(focus_action(Action::Focus));
        assert_eq!(
            harness.focused_widget_id(),
            None,
            "stale accessibility focus must not restore a noninteractive tab stop"
        );
        harness
            .edit_root_widget(|mut split| Split::set_draggable(&mut split, true))
            .unwrap();
        let _ = harness.redraw();
        assert!(
            harness
                .access_node(id)
                .unwrap()
                .data()
                .supports_action(Action::Focus)
        );
        harness.mouse_move((75.0, 20.0));
        harness.mouse_button_press(Some(PointerButton::Secondary));
        assert_eq!(
            harness.pointer_capture_target_id(),
            None,
            "secondary press cannot start dragging"
        );
        harness.mouse_button_release(Some(PointerButton::Secondary));
        harness.mouse_button_press(None);
        assert_eq!(harness.pointer_capture_target_id(), Some(id));
        harness.process_pointer_event(&PointerEvent::Up(PointerButtonEvent {
            pointer: PRIMARY_MOUSE,
            button: None,
            state: PointerState {
                position: crate::dpi::PhysicalPosition::new(105.0, 20.0),
                ..PointerState::default()
            },
        }));
        assert!(
            (harness.root_widget().unwrap().children()[0]
                .ctx()
                .border_box()
                .width()
                - 102.0)
                .abs()
                < 1e-8,
            "final release position must move the bar"
        );
        assert_eq!(harness.focused_widget_id(), Some(id));
        harness
            .edit_root_widget(|mut split| Split::set_draggable(&mut split, false))
            .unwrap();
        let _ = harness.redraw();
        assert_eq!(harness.focused_widget_id(), None);
        let node = harness.access_node(id).unwrap();
        assert!(!node.data().supports_action(Action::Focus));
        assert!(!node.data().supports_action(Action::SetValue));
    }

    #[test]
    fn columns() {
        #[rustfmt::skip]
        let widget = Split::new(
            Label::new("Hello").prepare(),
            Label::new("World").prepare(),
        ).split_axis(Axis::Horizontal).draggable(false).prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (150, 150));

        assert_render_snapshot!(harness, "split_columns");
    }

    #[test]
    fn rows() {
        #[rustfmt::skip]
        let widget = Split::new(
            Label::new("Hello").prepare(),
            Label::new("World").prepare(),
        ).split_axis(Axis::Vertical).draggable(false).prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (150, 150));

        assert_render_snapshot!(harness, "split_rows");
    }

    #[test]
    fn edit_splitter() {
        let image_1 = {
            let widget = Split::new(Label::new("Hello").prepare(), Label::new("World").prepare())
                .split_fraction(0.3)
                .min_lengths(40.px(), 10.px())
                .bar_thickness(12.px())
                .draggable(true)
                .solid_bar(true)
                .prepare();

            let mut harness =
                TestHarness::create_with_size(test_property_set(), widget, (100, 100));

            harness.render().unwrap()
        };

        let image_2 = {
            let widget =
                Split::new(Label::new("Hello").prepare(), Label::new("World").prepare()).prepare();

            let mut harness =
                TestHarness::create_with_size(test_property_set(), widget, (100, 100));

            harness
                .edit_root_widget(|mut splitter| {
                    Split::set_split_point(&mut splitter, SplitPoint::Fraction(0.3));
                    Split::set_min_lengths(&mut splitter, 40.px(), 10.px());
                    Split::set_bar_thickness(&mut splitter, 12.px());
                    Split::set_draggable(&mut splitter, true);
                    Split::set_bar_solid(&mut splitter, true);
                })
                .unwrap();

            harness.render().unwrap()
        };

        // We don't use assert_eq because we don't want rich assert
        assert!(image_1 == image_2);
    }

    #[test]
    fn drag_moves_split_point() {
        let widget =
            Split::new(Label::new("Hello").prepare(), Label::new("World").prepare()).prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (150, 100));

        let child1_initial_width = {
            let root = harness.root_widget().unwrap();
            root.children()[0].ctx().border_box().size().width
        };

        // Initial bar center with default settings:
        // split_space = 150 - 6 = 144, child1 = 72, bar center = 72 + 3 = 75.
        harness.mouse_move(Point::new(75.0, 10.0));
        harness.mouse_button_press(None);
        harness.mouse_move(Point::new(105.0, 10.0));
        harness.mouse_button_release(None);

        let (child1_width, child2_width) = {
            let root = harness.root_widget().unwrap();
            let children = root.children();
            (
                children[0].ctx().border_box().size().width,
                children[1].ctx().border_box().size().width,
            )
        };

        assert!(child1_width > child1_initial_width);
        assert!((child1_width - 102.0).abs() < 0.01);
        assert!((child2_width - 42.0).abs() < 0.01);
    }

    #[test]
    fn keyboard_moves_split_point() {
        let widget =
            Split::new(Label::new("Hello").prepare(), Label::new("World").prepare()).prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (150, 100));

        let root_id = harness.root_id();
        harness.focus_on(Some(root_id));

        let child1_initial_width = {
            let root = harness.root_widget().unwrap();
            root.children()[0].ctx().border_box().size().width
        };

        harness.process_text_event(&TextEvent::key_down(Key::Named(NamedKey::ArrowRight)));

        let child1_width = {
            let root = harness.root_widget().unwrap();
            root.children()[0].ctx().border_box().size().width
        };

        assert!(child1_width > child1_initial_width);
    }

    fn check_pixel_size_on_resize(point: SplitPoint, child_index: usize) {
        let widget = Split::new(Label::new("Hello").prepare(), Label::new("World").prepare())
            .split_point(point)
            .prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (200, 100));
        let check_width = |harness: &TestHarness<Split<Label, Label>>| {
            let root = harness.root_widget().unwrap();
            let width = root.children()[child_index].ctx().border_box().size().width;
            assert!((width - 50.0).abs() < 0.01);
        };
        check_width(&harness);
        harness.process_window_event(WindowEvent::Resize(PhysicalSize::new(300, 100)));
        check_width(&harness);
    }

    #[test]
    fn from_start_keeps_pixel_size_on_resize() {
        check_pixel_size_on_resize(SplitPoint::FromStart(50.px()), 0);
    }

    #[test]
    fn from_end_keeps_pixel_size_on_resize() {
        check_pixel_size_on_resize(SplitPoint::FromEnd(50.px()), 1);
    }

    #[test]
    fn fraction_clamps_when_set() {
        let widget = Split::new(
            Label::new("Hello").prepare().with_props(Padding::ZERO),
            Label::new("World").prepare().with_props(Padding::ZERO),
        )
        .prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (150, 100));

        harness
            .edit_root_widget(|mut split| {
                Split::set_split_point(&mut split, SplitPoint::Fraction(2.0));
            })
            .unwrap();
        let child1_width = {
            let root = harness.root_widget().unwrap();
            root.children()[0].ctx().border_box().size().width
        };
        assert!((child1_width - 144.0).abs() < 0.01);

        harness
            .edit_root_widget(|mut split| {
                Split::set_split_point(&mut split, SplitPoint::Fraction(-1.0));
            })
            .unwrap();
        let child1_width = {
            let root = harness.root_widget().unwrap();
            root.children()[0].ctx().border_box().size().width
        };
        assert!((child1_width - 0.0).abs() < 0.01);
    }
}
