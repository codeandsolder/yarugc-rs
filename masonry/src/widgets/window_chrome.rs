// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Shared chrome for native and embedded windows. The host owns window policy.

use accesskit::{Node, Role};

use super::selection_control::{Activation, ActivationKeys};
use crate::core::PropertySet;
use crate::core::{
    AccessCtx, AccessEvent, ArcStr, ChildrenIds, CursorIcon, EventCtx, LayoutCtx, MeasureCtx,
    NewWidget, PaintCtx, PointerButton, PointerEvent, PropertiesMut, PropertiesRef, QueryCtx,
    RegisterCtx, ResizeDirection, TextEvent, Update, UpdateCtx, Widget, WidgetId, WidgetMut,
    WidgetPod,
};
use crate::imaging::Painter;
use crate::kurbo::{Axis, Line, Point, Rect, Size, Stroke};
use crate::layout::{AsUnit, LenReq, Length};
use crate::layout::{LayoutSize, LenDef};
use crate::peniko::Color;
use crate::properties::{ContentColor, LineBreaking};
use crate::widgets::Label;

/// Window controls requested by the chrome. Pointer coordinates are in the
/// containing render root's logical coordinate space, not the moving widget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChromeAction {
    /// Start a title-bar drag.
    BeginMove(Point),
    /// Start dragging an edge or corner.
    BeginResize(ResizeDirection, Point),
    /// Continue the captured drag.
    Drag(Point),
    /// Finish the captured drag at this position.
    EndDrag(Point),
    /// Cancel the captured drag.
    CancelDrag,
    /// Toggle maximization or restore.
    ToggleMaximized,
    /// Toggle minimized state (native windows may only minimize from chrome).
    ToggleMinimized,
    /// Request closing this window (the host may decline).
    Close,
    /// Open the host's window menu.
    ShowMenu(Point),
}

/// Capabilities and presentation state supplied by the window controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChromeState {
    /// Whether to show chrome at all.
    pub visible: bool,
    /// Whether resize handles are available.
    pub resizable: bool,
    /// Whether a minimize control is available.
    pub can_minimize: bool,
    /// Whether a maximize control is available.
    pub can_maximize: bool,
    /// Whether a close control is available.
    pub can_close: bool,
    /// Whether this window is maximized.
    pub maximized: bool,
    /// Whether embedded content is collapsed to its title bar.
    pub minimized: bool,
    /// Whether this window is active.
    pub active: bool,
}

impl Default for ChromeState {
    fn default() -> Self {
        Self {
            visible: true,
            resizable: true,
            can_minimize: true,
            can_maximize: true,
            can_close: true,
            maximized: false,
            minimized: false,
            active: true,
        }
    }
}

/// Logical sizes of the shared chrome. Geometry never depends on title text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChromeMetrics {
    /// Height of the title strip.
    pub title_height: Length,
    /// Hit area reserved on each edge of a resizable window.
    pub resize_border: Length,
    /// Width of each window control.
    pub button_width: Length,
    /// Horizontal padding around the title.
    pub title_padding: Length,
}

impl Default for ChromeMetrics {
    fn default() -> Self {
        Self {
            title_height: crate::layout::length!(30.0),
            resize_border: crate::layout::length!(5.0),
            button_width: crate::layout::length!(34.0),
            title_padding: crate::layout::length!(8.0),
        }
    }
}

/// Colors for application-rendered chrome. Native hosts can keep their own
/// base surface color by selecting a transparent content background.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChromePalette {
    /// Active title bar and frame.
    pub active: Color,
    /// Inactive title bar and frame.
    pub inactive: Color,
    /// Title and icon color.
    pub foreground: Color,
    /// Pointer-hover or keyboard-focus background.
    pub hovered: Color,
    /// Close-button hover background.
    pub close_hovered: Color,
    /// Optional fill under content (normally opaque for embedded windows).
    pub content_background: Option<Color>,
}

impl Default for ChromePalette {
    fn default() -> Self {
        Self {
            active: Color::from_rgb8(45, 50, 59),
            inactive: Color::from_rgb8(33, 36, 43),
            foreground: Color::from_rgb8(230, 233, 239),
            hovered: Color::from_rgb8(65, 72, 83),
            close_hovered: Color::from_rgb8(174, 42, 48),
            content_background: Some(Color::from_rgb8(26, 28, 33)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Minimize,
    Expand,
    Maximize,
    Restore,
    Close,
}

impl Control {
    const fn action(self) -> ChromeAction {
        match self {
            Self::Minimize | Self::Expand => ChromeAction::ToggleMinimized,
            Self::Maximize | Self::Restore => ChromeAction::ToggleMaximized,
            Self::Close => ChromeAction::Close,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Minimize => "Minimize",
            Self::Expand => "Restore minimized window",
            Self::Maximize => "Maximize",
            Self::Restore => "Restore",
            Self::Close => "Close",
        }
    }
}

struct ChromeButton {
    control: Control,
    activation: Activation,
    palette: ChromePalette,
}

impl ChromeButton {
    fn new(control: Control) -> Self {
        Self {
            control,
            activation: Activation::new(),
            palette: ChromePalette::default(),
        }
    }

    fn emit(&self, ctx: &mut EventCtx<'_>) {
        ctx.submit_action::<ChromeAction>(self.control.action());
    }
}

impl Widget for ChromeButton {
    type Action = ChromeAction;

    fn register_children(&mut self, _: &mut RegisterCtx<'_>) {}

    fn measure(
        &mut self,
        _: &mut MeasureCtx<'_>,
        _: &PropertiesRef<'_>,
        _: Axis,
        _: LenReq,
        _: Option<Length>,
    ) -> Length {
        Length::ZERO
    }

    fn layout(&mut self, _: &mut LayoutCtx<'_>, _: &PropertiesRef<'_>, _: Size) {}

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::default()
    }

    fn accepts_focus(&self) -> bool {
        true
    }

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        if Activation::pointer(ctx, event) {
            self.emit(ctx);
        }
    }

    fn on_text_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _: &mut PropertiesMut<'_>,
        event: &TextEvent,
    ) {
        if self
            .activation
            .keyboard_with_keys(ctx, event, ActivationKeys::SpaceOrEnter)
        {
            self.emit(ctx);
        }
    }

    fn on_access_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _: &mut PropertiesMut<'_>,
        event: &AccessEvent,
    ) {
        if Activation::access(ctx, event) {
            self.emit(ctx);
        }
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _: &mut PropertiesMut<'_>, event: &Update) {
        self.activation.update(event);
        if matches!(
            event,
            Update::HoveredChanged(_) | Update::FocusChanged(_) | Update::DisabledChanged(_)
        ) {
            ctx.request_render();
        }
    }

    fn accessibility_role(&self) -> Role {
        Role::Button
    }

    fn accessibility(&mut self, _: &mut AccessCtx<'_>, _: &PropertiesRef<'_>, node: &mut Node) {
        node.set_label(self.control.label());
        node.add_action(accesskit::Action::Click);
    }

    fn paint(&mut self, ctx: &mut PaintCtx<'_>, _: &PropertiesRef<'_>, painter: &mut Painter<'_>) {
        let rect = ctx.content_box();
        if rect.width() < 12.0 || rect.height() < 12.0 {
            return;
        }
        let color = self.palette.foreground;
        if ctx.is_hovered() || ctx.is_focus_target() {
            let background = if self.control == Control::Close && ctx.is_hovered() {
                self.palette.close_hovered
            } else {
                self.palette.hovered
            };
            painter.fill(rect, background).draw();
        }
        let p = rect.center();
        let stroke = Stroke::new(1.25);
        let mut line = |a, b| {
            painter.stroke(Line::new(a, b), &stroke, color).draw();
        };
        match self.control {
            Control::Minimize | Control::Expand => {
                line(
                    p + crate::kurbo::Vec2::new(-5.0, 0.0),
                    p + crate::kurbo::Vec2::new(5.0, 0.0),
                );
                if self.control == Control::Expand {
                    line(
                        p + crate::kurbo::Vec2::new(0.0, -5.0),
                        p + crate::kurbo::Vec2::new(0.0, 5.0),
                    );
                }
            }
            Control::Close => {
                line(
                    p + crate::kurbo::Vec2::new(-4.0, -4.0),
                    p + crate::kurbo::Vec2::new(4.0, 4.0),
                );
                line(
                    p + crate::kurbo::Vec2::new(-4.0, 4.0),
                    p + crate::kurbo::Vec2::new(4.0, -4.0),
                );
            }
            Control::Maximize | Control::Restore => {
                let box_rect = Rect::from_center_size(p, Size::new(9.0, 9.0));
                if self.control == Control::Restore {
                    painter
                        .stroke(
                            Line::new(
                                (box_rect.x0 + 2.0, box_rect.y0 - 2.0),
                                (box_rect.x1 + 2.0, box_rect.y0 - 2.0),
                            ),
                            &stroke,
                            color,
                        )
                        .draw();
                    painter
                        .stroke(
                            Line::new(
                                (box_rect.x1 + 2.0, box_rect.y0 - 2.0),
                                (box_rect.x1 + 2.0, box_rect.y1 - 2.0),
                            ),
                            &stroke,
                            color,
                        )
                        .draw();
                }
                painter.stroke(box_rect, &stroke, color).draw();
            }
        }
    }
}

/// A reusable framed window with title, optional application header content,
/// independent accessible controls, and a fill-layout content widget.
///
/// Actions bubble to the host. Native controllers start compositor drags;
/// embedded controllers can use the same drag coordinates to update bounds.
pub struct WindowChrome {
    title: WidgetPod<Label>,
    header: Option<WidgetPod<dyn Widget>>,
    content: WidgetPod<dyn Widget>,
    controls: [WidgetPod<ChromeButton>; 3],
    state: ChromeState,
    metrics: ChromeMetrics,
    palette: ChromePalette,
    dragging: bool,
    delegate_drag: bool,
}

impl WindowChrome {
    /// Construct chrome around any content widget.
    #[must_use = "Use the constructed widget."]
    pub fn new(title: impl Into<ArcStr>, content: NewWidget<impl Widget + ?Sized>) -> Self {
        Self {
            title: Label::new(title)
                .prepare()
                .with_props(
                    PropertySet::new()
                        .with(LineBreaking::Clip)
                        .with(ContentColor::new(ChromePalette::default().foreground)),
                )
                .into_pod(),
            header: None,
            content: content.erased().into_pod(),
            controls: [Control::Minimize, Control::Maximize, Control::Close]
                .map(|control| ChromeButton::new(control).prepare().into_pod()),
            state: ChromeState::default(),
            metrics: ChromeMetrics::default(),
            palette: ChromePalette::default(),
            dragging: false,
            delegate_drag: false,
        }
    }

    /// Delegate drag continuation to a native window manager. The chrome
    /// sends the initiating request but does not capture the compositor's pointer.
    #[must_use = "Use the configured chrome."]
    pub const fn with_delegated_drag(mut self) -> Self {
        self.delegate_drag = true;
        self
    }

    /// Insert arbitrary interactive content such as menus or tabs in the title strip.
    #[must_use = "Use the configured chrome."]
    pub fn with_header(mut self, header: NewWidget<impl Widget + ?Sized>) -> Self {
        self.header = Some(header.erased().into_pod());
        self
    }

    /// Set initial host capabilities.
    #[must_use = "Use the configured chrome."]
    pub fn with_state(mut self, state: ChromeState) -> Self {
        if state.minimized {
            self.controls[0] = ChromeButton::new(Control::Expand).prepare().into_pod();
        }
        if state.maximized {
            self.controls[1] = ChromeButton::new(Control::Restore).prepare().into_pod();
        }
        self.state = state;
        self
    }

    /// Set chrome dimensions.
    #[must_use = "Use the configured chrome."]
    pub const fn with_metrics(mut self, metrics: ChromeMetrics) -> Self {
        self.metrics = metrics;
        self
    }

    /// Configure colors for this window's chrome.
    #[must_use = "Use the configured chrome."]
    pub const fn with_palette(mut self, palette: ChromePalette) -> Self {
        self.palette = palette;
        self
    }

    /// Change the chrome palette without replacing content.
    pub fn set_palette(this: &mut WidgetMut<'_, Self>, palette: ChromePalette) {
        this.widget.palette = palette;
        Self::apply_palette(this);
    }

    fn apply_palette(this: &mut WidgetMut<'_, Self>) {
        let palette = this.widget.palette;
        if let Some(mut label) = this.ctx.get_mut::<Label>(this.widget.title.id()) {
            let _ = label.insert_prop(ContentColor::new(palette.foreground));
        }
        for control in &this.widget.controls {
            if let Some(mut button) = this.ctx.get_mut::<ChromeButton>(control.id()) {
                button.widget.palette = palette;
                button.ctx.request_paint_only();
            }
        }
        this.ctx.request_paint_only();
    }

    /// Stable content widget identity, independent of chrome visibility.
    #[must_use = "Use the stable content identifier."]
    pub const fn content_id(&self) -> WidgetId {
        self.content.id()
    }

    /// Test whether a source is one of this chrome's own control widgets.
    /// The host additionally compares the source against the chrome root ID.
    #[must_use = "Check whether the source is a chrome control."]
    pub fn owns_control(&self, source: WidgetId) -> bool {
        self.controls.iter().any(|control| control.id() == source)
    }

    /// Update the title without replacing any content.
    pub fn set_title(this: &mut WidgetMut<'_, Self>, title: impl Into<ArcStr>) {
        if let Some(mut label) = this.ctx.get_mut(this.widget.title.id()) {
            Label::set_text(&mut label, title);
        }
    }

    /// Update host capabilities, active state and maximize state.
    pub fn set_state(this: &mut WidgetMut<'_, Self>, state: ChromeState) {
        if this.widget.state == state {
            return;
        }
        let previous = this.widget.state;
        this.widget.state = state;
        if previous.minimized != state.minimized
            && let Some(mut button) = this
                .ctx
                .get_mut::<ChromeButton>(this.widget.controls[0].id())
        {
            button.widget.control = if state.minimized {
                Control::Expand
            } else {
                Control::Minimize
            };
            button.ctx.request_render();
        }
        if previous.maximized != state.maximized
            && let Some(mut button) = this
                .ctx
                .get_mut::<ChromeButton>(this.widget.controls[1].id())
        {
            button.widget.control = if state.maximized {
                Control::Restore
            } else {
                Control::Maximize
            };
            button.ctx.request_render();
        }
        if (ChromeState {
            active: state.active,
            ..previous
        }) != state
        {
            this.ctx.request_layout();
        }
        this.ctx.request_render();
    }

    fn border(&self) -> f64 {
        if self.state.visible
            && self.state.resizable
            && !self.state.maximized
            && !self.state.minimized
        {
            self.metrics.resize_border.get()
        } else {
            0.0
        }
    }

    fn title_height(&self) -> f64 {
        if self.state.visible {
            self.metrics.title_height.get()
        } else {
            0.0
        }
    }

    fn buttons(&self) -> [bool; 3] {
        [
            self.state.can_minimize,
            self.state.can_maximize,
            self.state.can_close,
        ]
        .map(|enabled| self.state.visible && enabled)
    }

    fn resize_direction(&self, size: Size, pos: Point) -> Option<ResizeDirection> {
        resize_direction(size, pos, self.border())
    }

    fn title_region(&self, size: Size, pos: Point) -> bool {
        self.state.visible
            && pos.x >= self.border()
            && pos.x < size.width - self.border()
            && pos.y >= self.border()
            && pos.y < self.border() + self.title_height()
    }
}

fn resize_direction(size: Size, pos: Point, border: f64) -> Option<ResizeDirection> {
    use ResizeDirection::*;
    if border <= 0.0 || !Rect::from_origin_size(Point::ORIGIN, size).contains(pos) {
        return None;
    }
    let bx = border.min(size.width * 0.5);
    let by = border.min(size.height * 0.5);
    let x = if pos.x < bx {
        -1
    } else if pos.x >= size.width - bx {
        1
    } else {
        0
    };
    let y = if pos.y < by {
        -1
    } else if pos.y >= size.height - by {
        1
    } else {
        0
    };
    match (x, y) {
        (-1, -1) => Some(NorthWest),
        (0, -1) => Some(North),
        (1, -1) => Some(NorthEast),
        (-1, 0) => Some(West),
        (1, 0) => Some(East),
        (-1, 1) => Some(SouthWest),
        (0, 1) => Some(South),
        (1, 1) => Some(SouthEast),
        _ => None,
    }
}

impl Widget for WindowChrome {
    type Action = ChromeAction;

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        ctx.register_child(&mut self.title);
        if let Some(header) = &mut self.header {
            ctx.register_child(header);
        }
        for control in &mut self.controls {
            ctx.register_child(control);
        }
        ctx.register_child(&mut self.content);
    }

    fn children_ids(&self) -> ChildrenIds {
        let mut ids = ChildrenIds::from_slice(&[self.title.id()]);
        if let Some(header) = &self.header {
            ids.push(header.id());
        }
        ids.extend(self.controls.iter().map(WidgetPod::id));
        ids.push(self.content.id());
        ids
    }

    fn measure(
        &mut self,
        ctx: &mut MeasureCtx<'_>,
        _: &PropertiesRef<'_>,
        axis: Axis,
        req: LenReq,
        cross: Option<Length>,
    ) -> Length {
        let border = self.border() * 2.0;
        let title = self.title_height();
        let extra = match axis {
            Axis::Horizontal => border,
            Axis::Vertical => border + title,
        };
        let cross_extra = match axis {
            Axis::Horizontal => border + title,
            Axis::Vertical => border,
        };
        let child = if self.state.minimized {
            Length::ZERO
        } else {
            ctx.measure_child(
                &self.content,
                axis,
                req.reduce(extra.px()),
                cross.map(|v| v.saturating_sub(cross_extra.px())),
            )
        };
        let length = child.saturating_add(extra.px());
        if self.state.visible && axis == Axis::Horizontal {
            let controls = self.buttons().into_iter().map(u32::from).sum::<u32>();
            length.max(
                (border
                    + self.metrics.button_width.get() * f64::from(controls)
                    + self.metrics.title_padding.get() * 2.0)
                    .px(),
            )
        } else {
            length
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, _: &PropertiesRef<'_>, size: Size) {
        let border = self.border().min(size.width * 0.5).min(size.height * 0.5);
        let width = (size.width - 2.0 * border).max(0.0);
        let title_height = self
            .title_height()
            .min((size.height - 2.0 * border).max(0.0));
        let content_size = Size::new(width, (size.height - 2.0 * border - title_height).max(0.0));
        ctx.set_stashed(&self.content, self.state.minimized);
        if !self.state.minimized {
            ctx.run_layout(&self.content, content_size);
            ctx.place_child(&self.content, Point::new(border, border + title_height));
        }
        ctx.set_clip_path(size.to_rect() + ctx.border_box_insets());

        let shown = self.buttons();
        let count: u32 = shown.into_iter().map(u32::from).sum();
        let button_width = self
            .metrics
            .button_width
            .get()
            .min(width / (f64::from(count.max(1))));
        let mut end = size.width - border;
        for (control, visible) in self.controls.iter().zip(shown).rev() {
            ctx.set_stashed(control, !visible);
            if visible {
                end -= button_width;
                ctx.run_layout(control, Size::new(button_width, title_height));
                ctx.place_child(control, Point::new(end, border));
            }
        }
        ctx.set_stashed(&self.title, !self.state.visible);
        if let Some(header) = &self.header {
            ctx.set_stashed(header, !self.state.visible);
        }
        if !self.state.visible {
            return;
        }

        let start = (border + self.metrics.title_padding.get()).min(end);
        let free = (end - start - self.metrics.title_padding.get()).max(0.0);
        let header_width = self.header.as_ref().map_or(0.0, |header| {
            ctx.compute_length(
                header,
                LenDef::FitContent(free.px()),
                LayoutSize::maybe(Axis::Vertical, Some(title_height.px())),
                Axis::Horizontal,
                Some(title_height.px()),
            )
            .get()
            .min(free)
        });
        let title_width = free - header_width;
        let label_height = ctx
            .compute_length(
                &self.title,
                LenDef::FitContent(title_height.px()),
                LayoutSize::maybe(Axis::Horizontal, Some(title_width.px())),
                Axis::Vertical,
                Some(title_width.px()),
            )
            .get()
            .min(title_height);
        ctx.run_layout(&self.title, Size::new(title_width, label_height));
        ctx.place_child(
            &self.title,
            Point::new(start, border + (title_height - label_height) * 0.5),
        );
        if let Some(header) = &self.header {
            ctx.run_layout(header, Size::new(header_width, title_height));
            ctx.place_child(header, Point::new(start + title_width, border));
        }
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _: &mut PropertiesMut<'_>, event: &Update) {
        if matches!(event, Update::WidgetAdded) {
            ctx.mutate_self_later(|mut widget| Self::apply_palette(&mut widget));
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx<'_>, _: &PropertiesRef<'_>, painter: &mut Painter<'_>) {
        if !self.state.visible {
            return;
        }
        let size = ctx.content_box().size();
        let border = self.border().min(size.width * 0.5).min(size.height * 0.5);
        let top = (border + self.title_height()).min(size.height);
        let color = if self.state.active {
            self.palette.active
        } else {
            self.palette.inactive
        };
        if let Some(background) = self.palette.content_background {
            painter.fill(size.to_rect(), background).draw();
        }
        painter
            .fill(Rect::new(0.0, 0.0, size.width, top), color)
            .draw();
        if border > 0.0 {
            painter
                .fill(Rect::new(0.0, top, border, size.height), color)
                .draw();
            painter
                .fill(
                    Rect::new(size.width - border, top, size.width, size.height),
                    color,
                )
                .draw();
            painter
                .fill(
                    Rect::new(
                        border,
                        size.height - border,
                        size.width - border,
                        size.height,
                    ),
                    color,
                )
                .draw();
        }
    }

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        match event {
            PointerEvent::Down(event) => {
                if ctx.target() != ctx.widget_id() && !ctx.target_in_child(&self.title) {
                    return;
                }
                let pos = ctx.local_position(event.state.position);
                let root_pos = ctx.local_to_window(pos);
                let size = ctx.content_box().size();
                let action = match event.button {
                    None | Some(PointerButton::Primary) => {
                        if let Some(direction) = self.resize_direction(size, pos) {
                            ChromeAction::BeginResize(direction, root_pos)
                        } else if self.title_region(size, pos) {
                            if event.state.count == 2 && self.state.can_maximize {
                                ChromeAction::ToggleMaximized
                            } else {
                                ChromeAction::BeginMove(root_pos)
                            }
                        } else {
                            return;
                        }
                    }
                    Some(PointerButton::Secondary) if self.title_region(size, pos) => {
                        ChromeAction::ShowMenu(root_pos)
                    }
                    _ => return,
                };
                if !self.delegate_drag
                    && matches!(
                        action,
                        ChromeAction::BeginMove(_) | ChromeAction::BeginResize(..)
                    )
                {
                    self.dragging = true;
                    ctx.capture_pointer();
                }
                ctx.set_handled();
                ctx.submit_action::<ChromeAction>(action);
            }
            PointerEvent::Move(event) if self.dragging && ctx.is_pointer_capture_target() => {
                ctx.set_handled();
                let pos = ctx.local_to_window(ctx.local_position(event.current.position));
                ctx.submit_action::<ChromeAction>(ChromeAction::Drag(pos));
            }
            PointerEvent::Up(event) if self.dragging && ctx.is_pointer_capture_target() => {
                self.dragging = false;
                ctx.set_handled();
                let pos = ctx.local_to_window(ctx.local_position(event.state.position));
                ctx.submit_action::<ChromeAction>(ChromeAction::EndDrag(pos));
            }
            PointerEvent::Cancel(_) if self.dragging => {
                self.dragging = false;
                ctx.submit_action::<ChromeAction>(ChromeAction::CancelDrag);
            }
            _ => {}
        }
    }

    fn get_cursor(&self, ctx: &QueryCtx<'_>, pos: Point) -> CursorIcon {
        use ResizeDirection::*;
        match self.resize_direction(ctx.content_box().size(), ctx.window_to_local(pos)) {
            Some(North | South) => CursorIcon::NsResize,
            Some(East | West) => CursorIcon::EwResize,
            Some(NorthWest | SouthEast) => CursorIcon::NwseResize,
            Some(NorthEast | SouthWest) => CursorIcon::NeswResize,
            None => CursorIcon::Default,
        }
    }

    fn accessibility_role(&self) -> Role {
        Role::GenericContainer
    }
}

/// Geometry and content for an embedded tool window.
pub struct ToolWindow {
    chrome: WidgetPod<WindowChrome>,
    state: ChromeState,
    metrics: ChromeMetrics,
    normal_bounds: Rect,
    minimum: Size,
}

/// Invalid requested tool-window geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidToolWindowGeometry;

impl std::fmt::Display for InvalidToolWindowGeometry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("window bounds and minimum dimensions must be finite and nonnegative")
    }
}
impl std::error::Error for InvalidToolWindowGeometry {}

impl ToolWindow {
    /// A window uses exactly the same chrome widget as the native host.
    /// # Errors
    /// Returns an error for nonfinite, reversed, or negative-sized bounds.
    pub fn new(
        mut chrome: NewWidget<WindowChrome>,
        bounds: Rect,
        minimum: Size,
    ) -> Result<Self, InvalidToolWindowGeometry> {
        if !bounds.is_finite()
            || bounds.width() < 0.0
            || bounds.height() < 0.0
            || !minimum.width.is_finite()
            || !minimum.height.is_finite()
            || minimum.width < 0.0
            || minimum.height < 0.0
        {
            return Err(InvalidToolWindowGeometry);
        }
        chrome.widget.state.active = true;
        let state = chrome.widget.state;
        let metrics = chrome.widget.metrics;
        Ok(Self {
            chrome: chrome.into_pod(),
            state,
            metrics,
            normal_bounds: bounds,
            minimum,
        })
    }

    /// The stable identity of this tool window's chrome.
    #[must_use = "Use the tool window identifier."]
    pub const fn id(&self) -> WidgetId {
        self.chrome.id()
    }

    fn bounds(&self, workspace: Size) -> Rect {
        if self.state.maximized && !self.state.minimized {
            return workspace.to_rect();
        }
        if self.state.minimized {
            let title_height = self.metrics.title_height.get();
            let bounds = Rect::from_origin_size(
                self.normal_bounds.origin(),
                Size::new(self.normal_bounds.width(), title_height),
            );
            fit_window(
                bounds,
                Size::new(self.minimum.width, title_height),
                workspace,
            )
        } else {
            fit_window(self.normal_bounds, self.minimum, workspace)
        }
    }
}

fn fit_window(rect: Rect, minimum: Size, workspace: Size) -> Rect {
    let width = rect.width().max(minimum.width).min(workspace.width);
    let height = rect.height().max(minimum.height).min(workspace.height);
    let x = rect.x0.clamp(0.0, (workspace.width - width).max(0.0));
    let y = rect.y0.clamp(0.0, (workspace.height - height).max(0.0));
    Rect::from_origin_size(Point::new(x, y), Size::new(width, height))
}

#[derive(Clone, Copy, Debug)]
enum DragKind {
    Move,
    Resize(ResizeDirection),
}

#[derive(Clone, Copy, Debug)]
struct ToolDrag {
    window: WidgetId,
    start: Point,
    original: Rect,
    kind: DragKind,
}

fn drag_bounds(drag: ToolDrag, position: Point, minimum: Size, workspace: Size) -> Rect {
    let delta = position - drag.start;
    let mut rect = fit_window(drag.original, minimum, workspace);
    match drag.kind {
        DragKind::Move => fit_window(rect + delta, minimum, workspace),
        DragKind::Resize(direction) => {
            use ResizeDirection::*;
            let w = minimum.width.min(workspace.width).min(rect.width());
            let h = minimum.height.min(workspace.height).min(rect.height());
            if matches!(direction, West | NorthWest | SouthWest) {
                rect.x0 = (rect.x0 + delta.x).clamp(0.0, rect.x1 - w);
            }
            if matches!(direction, East | NorthEast | SouthEast) {
                rect.x1 = (rect.x1 + delta.x).clamp(rect.x0 + w, workspace.width);
            }
            if matches!(direction, North | NorthEast | NorthWest) {
                rect.y0 = (rect.y0 + delta.y).clamp(0.0, rect.y1 - h);
            }
            if matches!(direction, South | SouthEast | SouthWest) {
                rect.y1 = (rect.y1 + delta.y).clamp(rect.y0 + h, workspace.height);
            }
            rect
        }
    }
}

/// Application-level requests not owned by embedded window geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToolWorkspaceAction {
    /// The application may approve or reject removal of the window.
    CloseRequested(WidgetId),
    /// The application may show a window menu at this workspace position.
    MenuRequested(WidgetId, Point),
}

/// A base widget with independently movable and resizable embedded windows.
/// Children are laid out in workspace coordinates and painted in z-order.
/// Geometry changes do not replace the content widgets. Use
/// [`Dimensions::STRETCH`](crate::properties::Dimensions::STRETCH) to fill
/// the available workspace, rather than its background's preferred size.
pub struct ToolWorkspace {
    background: WidgetPod<dyn Widget>,
    windows: Vec<ToolWindow>,
    drag: Option<ToolDrag>,
}

impl ToolWorkspace {
    /// Construct a workspace over an arbitrary base widget.
    #[must_use = "Use the constructed widget."]
    pub fn new(background: NewWidget<impl Widget + ?Sized>) -> Self {
        Self {
            background: background.erased().into_pod(),
            windows: Vec::new(),
            drag: None,
        }
    }

    /// Add an initial tool window, above the preceding windows.
    #[must_use = "Use the configured workspace."]
    pub fn with_window(mut self, window: ToolWindow) -> Self {
        self.windows.push(window);
        self
    }

    /// Current frontmost window identity.
    #[must_use = "Inspect the frontmost tool window."]
    pub fn active_window(&self) -> Option<WidgetId> {
        self.windows.last().map(ToolWindow::id)
    }

    /// Requested normal bounds, independent of temporary maximize/collapse.
    #[must_use = "Inspect the window geometry."]
    pub fn normal_bounds(&self, id: WidgetId) -> Option<Rect> {
        self.windows
            .iter()
            .find(|entry| entry.id() == id)
            .map(|entry| entry.normal_bounds)
    }

    /// Add a window without rebuilding the workspace.
    pub fn add_window(this: &mut WidgetMut<'_, Self>, window: ToolWindow) {
        this.widget.windows.push(window);
        this.ctx.children_changed();
        Self::sync_active(this);
    }

    /// Remove an approved window. Returns false if it was absent.
    #[must_use = "Check whether the window was removed."]
    pub fn remove_window(this: &mut WidgetMut<'_, Self>, id: WidgetId) -> bool {
        let Some(index) = this
            .widget
            .windows
            .iter()
            .position(|entry| entry.id() == id)
        else {
            return false;
        };
        let entry = this.widget.windows.remove(index);
        if this.widget.drag.is_some_and(|drag| drag.window == id) {
            this.widget.drag = None;
        }
        this.ctx.remove_child(entry.chrome);
        Self::sync_active(this);
        true
    }

    fn sync_active(this: &mut WidgetMut<'_, Self>) {
        let active = this.widget.active_window();
        for entry in &mut this.widget.windows {
            entry.state.active = Some(entry.id()) == active;
            if let Some(mut chrome) = this.ctx.get_mut::<WindowChrome>(entry.id()) {
                WindowChrome::set_state(&mut chrome, entry.state);
            }
        }
    }

    /// Raise a window and keep its descendants' identity and focus.
    pub fn raise_window(this: &mut WidgetMut<'_, Self>, id: WidgetId) {
        let Some(index) = this
            .widget
            .windows
            .iter()
            .position(|entry| entry.id() == id)
        else {
            return;
        };
        if index + 1 != this.widget.windows.len() {
            let entry = this.widget.windows.remove(index);
            this.widget.windows.push(entry);
            this.ctx.children_changed();
        }
        Self::sync_active(this);
    }

    fn apply_action(
        this: &mut WidgetMut<'_, Self>,
        id: WidgetId,
        action: ChromeAction,
        workspace: Size,
    ) {
        if matches!(
            action,
            ChromeAction::BeginMove(_)
                | ChromeAction::BeginResize(..)
                | ChromeAction::ToggleMaximized
                | ChromeAction::ToggleMinimized
        ) {
            Self::raise_window(this, id);
        }
        let Some(entry) = this
            .widget
            .windows
            .iter_mut()
            .find(|entry| entry.id() == id)
        else {
            return;
        };
        match action {
            ChromeAction::BeginMove(position) => {
                if entry.state.maximized {
                    // Restore before moving, keeping the pointer above the restored title strip.
                    let old = entry.bounds(workspace);
                    let relative = ((position.x - old.x0) / old.width().max(1.0)).clamp(0.0, 1.0);
                    entry.state.maximized = false;
                    let restored = entry.bounds(workspace);
                    entry.normal_bounds = fit_window(
                        Rect::from_origin_size(
                            Point::new(
                                position.x - restored.width() * relative,
                                position.y - entry.metrics.title_height.get() * 0.5,
                            ),
                            restored.size(),
                        ),
                        entry.minimum,
                        workspace,
                    );
                }
                this.widget.drag = Some(ToolDrag {
                    window: id,
                    start: position,
                    original: entry.bounds(workspace),
                    kind: DragKind::Move,
                });
            }
            ChromeAction::BeginResize(direction, position)
                if entry.state.resizable && !entry.state.maximized && !entry.state.minimized =>
            {
                this.widget.drag = Some(ToolDrag {
                    window: id,
                    start: position,
                    original: entry.bounds(workspace),
                    kind: DragKind::Resize(direction),
                });
            }
            ChromeAction::Drag(position) | ChromeAction::EndDrag(position) => {
                if let Some(drag) = this.widget.drag.filter(|drag| drag.window == id) {
                    let minimum = if entry.state.minimized {
                        Size::new(entry.minimum.width, entry.metrics.title_height.get())
                    } else {
                        entry.minimum
                    };
                    let bounds = drag_bounds(drag, position, minimum, workspace);
                    if entry.state.minimized {
                        entry.normal_bounds =
                            Rect::from_origin_size(bounds.origin(), entry.normal_bounds.size());
                    } else {
                        entry.normal_bounds = bounds;
                    }
                    if matches!(action, ChromeAction::EndDrag(_)) {
                        this.widget.drag = None;
                    }
                }
            }
            ChromeAction::CancelDrag => {
                if let Some(drag) = this.widget.drag.filter(|drag| drag.window == id) {
                    if entry.state.minimized {
                        entry.normal_bounds = Rect::from_origin_size(
                            drag.original.origin(),
                            entry.normal_bounds.size(),
                        );
                    } else {
                        entry.normal_bounds = drag.original;
                    }
                    this.widget.drag = None;
                }
            }
            ChromeAction::ToggleMaximized if entry.state.can_maximize => {
                entry.state.minimized = false;
                entry.state.maximized = !entry.state.maximized;
            }
            ChromeAction::ToggleMinimized if entry.state.can_minimize => {
                entry.state.minimized = !entry.state.minimized;
            }
            _ => {}
        }
        let state = entry.state;
        if let Some(mut chrome) = this.ctx.get_mut::<WindowChrome>(id) {
            WindowChrome::set_state(&mut chrome, state);
        }
        this.ctx.request_layout();
    }
}

impl Widget for ToolWorkspace {
    type Action = ToolWorkspaceAction;

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        ctx.register_child(&mut self.background);
        for entry in &mut self.windows {
            ctx.register_child(&mut entry.chrome);
        }
    }

    fn children_ids(&self) -> ChildrenIds {
        core::iter::once(self.background.id())
            .chain(self.windows.iter().map(ToolWindow::id))
            .collect()
    }

    fn measure(
        &mut self,
        ctx: &mut MeasureCtx<'_>,
        _: &PropertiesRef<'_>,
        axis: Axis,
        req: LenReq,
        cross: Option<Length>,
    ) -> Length {
        ctx.measure_child(&self.background, axis, req, cross)
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, _: &PropertiesRef<'_>, size: Size) {
        ctx.run_layout(&self.background, size);
        ctx.place_child(&self.background, Point::ORIGIN);
        for entry in &self.windows {
            let bounds = entry.bounds(size);
            ctx.run_layout(&entry.chrome, bounds.size());
            ctx.place_child(&entry.chrome, bounds.origin());
        }
        ctx.set_clip_path(size.to_rect() + ctx.border_box_insets());
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _: &mut PropertiesMut<'_>, event: &Update) {
        if matches!(event, Update::WidgetAdded) {
            ctx.mutate_self_later(|mut widget| Self::sync_active(&mut widget));
        }
    }

    fn on_pointer_down_preview(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _: &mut PropertiesMut<'_>,
        _: &crate::core::PointerButtonEvent,
    ) {
        if let Some(window) = self
            .windows
            .iter()
            .find(|entry| ctx.target_in_child(&entry.chrome))
        {
            let id = window.id();
            if self.active_window() != Some(id) {
                // Do not change the tree while the initiating click is being routed.
                ctx.mutate_self_later(move |mut widget| Self::raise_window(&mut widget, id));
            }
        }
    }

    fn on_action(
        &mut self,
        ctx: &mut crate::core::ActionCtx<'_>,
        _: &mut PropertiesMut<'_>,
        action: &crate::core::ErasedAction,
        source: WidgetId,
    ) {
        let Some(action) = action.downcast_ref::<ChromeAction>().copied() else {
            return;
        };
        let Some(entry) = self.windows.iter().find(|entry| {
            entry.id() == source
                || ctx
                    .get_child(&entry.chrome)
                    .is_some_and(|chrome| chrome.owns_control(source))
        }) else {
            return;
        };
        let id = entry.id();
        let local = |point| ctx.window_to_local(point);
        let action = match action {
            ChromeAction::BeginMove(point) => ChromeAction::BeginMove(local(point)),
            ChromeAction::BeginResize(direction, point) => {
                ChromeAction::BeginResize(direction, local(point))
            }
            ChromeAction::Drag(point) => ChromeAction::Drag(local(point)),
            ChromeAction::EndDrag(point) => ChromeAction::EndDrag(local(point)),
            ChromeAction::ShowMenu(point) => ChromeAction::ShowMenu(local(point)),
            other => other,
        };
        ctx.set_handled();
        match action {
            ChromeAction::Close => {
                ctx.submit_action::<ToolWorkspaceAction>(ToolWorkspaceAction::CloseRequested(id))
            }
            ChromeAction::ShowMenu(point) => ctx.submit_action::<ToolWorkspaceAction>(
                ToolWorkspaceAction::MenuRequested(id, point),
            ),
            _ => {
                let workspace = ctx.content_box().size();
                ctx.mutate_self_later(move |mut widget| {
                    Self::apply_action(&mut widget, id, action, workspace)
                });
            }
        }
    }

    fn accessibility_role(&self) -> Role {
        Role::GenericContainer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::properties::Dimensions;
    use crate::testing::TestHarness;
    use crate::theme::test_property_set;
    use crate::widgets::{Button, ButtonPress, SizedBox};

    const PRIMARY: Option<PointerButton> = Some(PointerButton::Primary);

    #[test]
    fn edges_have_disjoint_hit_regions_even_on_small_windows() {
        let size = Size::new(100.0, 80.0);
        use ResizeDirection::*;
        for (x, y, expected) in [
            (0., 0., NorthWest),
            (50., 0., North),
            (99., 0., NorthEast),
            (0., 40., West),
            (99., 40., East),
            (0., 79., SouthWest),
            (50., 79., South),
            (99., 79., SouthEast),
        ] {
            assert_eq!(resize_direction(size, Point::new(x, y), 5.), Some(expected));
        }
        assert_eq!(resize_direction(size, Point::new(50., 40.), 5.), None);
        assert_eq!(resize_direction(size, Point::new(-1., 0.), 5.), None);
        assert_eq!(resize_direction(size, Point::new(0., 0.), 0.), None);
        assert_eq!(
            resize_direction(Size::new(4., 4.), Point::new(2., 2.), 5.),
            Some(SouthEast)
        );
    }

    #[test]
    fn chrome_keeps_content_identity_when_visibility_changes() {
        let content = Label::new("content").prepare();
        let content_id = content.id();
        let chrome = WindowChrome::new("Title", content)
            .prepare()
            .with_props(Dimensions::STRETCH);
        let mut harness = TestHarness::create_with_size(test_property_set(), chrome, (240, 160));
        assert_eq!(
            harness
                .get_widget_with_id(content_id)
                .unwrap()
                .ctx()
                .border_box()
                .size(),
            Size::new(230., 120.)
        );
        harness
            .edit_root_widget(|mut chrome| {
                WindowChrome::set_state(
                    &mut chrome,
                    ChromeState {
                        visible: false,
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        assert_eq!(
            harness
                .get_widget_with_id(content_id)
                .unwrap()
                .ctx()
                .border_box()
                .size(),
            Size::new(240., 160.)
        );
        assert_eq!(harness.root_widget().unwrap().content_id(), content_id);
    }

    #[test]
    fn native_drag_delegates_only_initiation_without_pointer_capture() {
        let chrome = WindowChrome::new("Native", Label::new("content").prepare())
            .with_delegated_drag()
            .prepare()
            .with_props(Dimensions::STRETCH);
        let mut harness = TestHarness::create_with_size(test_property_set(), chrome, (240, 160));
        harness.mouse_move((20., 20.));
        harness.mouse_button_press(PRIMARY);
        assert_eq!(
            harness.pop_action::<ChromeAction>().unwrap().unwrap().0,
            ChromeAction::BeginMove(Point::new(20., 20.))
        );
        harness.mouse_move((70., 70.));
        harness.mouse_button_release(PRIMARY);
        assert!(harness.pop_action_erased().is_none());
    }

    fn make_window(title: &str, bounds: Rect) -> (ToolWindow, WidgetId) {
        let content = Button::with_text(title).prepare();
        let id = content.id();
        (
            ToolWindow::new(
                WindowChrome::new(title, content).prepare(),
                bounds,
                Size::new(100., 70.),
            )
            .unwrap(),
            id,
        )
    }

    #[test]
    fn embedded_drag_resize_maximize_and_collapse_preserve_widget_identity() {
        let (window, content) = make_window("Tool", Rect::new(20., 20., 220., 170.));
        let id = window.id();
        let chrome = ToolWorkspace::new(SizedBox::empty().prepare())
            .with_window(window)
            .prepare()
            .with_props(Dimensions::STRETCH);
        let mut harness = TestHarness::create_with_size(test_property_set(), chrome, (400, 300));
        harness.mouse_move((40., 40.));
        harness.mouse_button_press(PRIMARY);
        harness.mouse_move((80., 70.));
        harness.mouse_button_release(PRIMARY);
        assert_eq!(
            harness.root_widget().unwrap().normal_bounds(id),
            Some(Rect::new(60., 50., 260., 200.))
        );
        harness.mouse_move((259., 199.));
        harness.mouse_button_press(PRIMARY);
        harness.mouse_move((399., 299.));
        harness.mouse_button_release(PRIMARY);
        let resized = Rect::new(60., 50., 400., 300.);
        assert_eq!(
            harness.root_widget().unwrap().normal_bounds(id),
            Some(resized)
        );
        let controls = {
            let widget = harness
                .get_widget_with_id(id)
                .unwrap()
                .downcast::<WindowChrome>()
                .unwrap();
            widget.controls.each_ref().map(WidgetPod::id)
        };
        harness.mouse_click_on(controls[1], PRIMARY).unwrap();
        assert_eq!(
            harness
                .get_widget_with_id(id)
                .unwrap()
                .ctx()
                .border_box()
                .size(),
            Size::new(400., 300.)
        );
        harness.mouse_click_on(controls[1], PRIMARY).unwrap();
        assert_eq!(
            harness.root_widget().unwrap().normal_bounds(id),
            Some(resized)
        );
        harness.mouse_click_on(controls[0], PRIMARY).unwrap();
        assert_eq!(
            harness
                .get_widget_with_id(id)
                .unwrap()
                .ctx()
                .border_box()
                .height(),
            30.
        );
        harness.mouse_click_on(controls[0], PRIMARY).unwrap();
        assert_eq!(
            harness
                .get_widget_with_id(id)
                .unwrap()
                .ctx()
                .border_box()
                .size(),
            resized.size()
        );
        assert!(harness.get_widget_with_id(content).is_some());
        harness.mouse_click_on(controls[2], PRIMARY).unwrap();
        assert_eq!(
            harness
                .pop_action::<ToolWorkspaceAction>()
                .unwrap()
                .unwrap()
                .0,
            ToolWorkspaceAction::CloseRequested(id)
        );
        harness
            .edit_root_widget(|mut workspace| {
                assert!(ToolWorkspace::remove_window(&mut workspace, id))
            })
            .unwrap();
        assert!(harness.get_widget_with_id(content).is_none());
    }

    #[test]
    fn clicking_a_consuming_child_raises_its_tool_window_and_preserves_action() {
        let (a, button) = make_window("A", Rect::new(10., 10., 210., 200.));
        let a_id = a.id();
        let (b, _) = make_window("B", Rect::new(250., 10., 390., 200.));
        let b_id = b.id();
        let workspace = ToolWorkspace::new(SizedBox::empty().prepare())
            .with_window(a)
            .with_window(b)
            .prepare()
            .with_props(Dimensions::STRETCH);
        let mut harness = TestHarness::create_with_size(test_property_set(), workspace, (420, 300));
        assert_eq!(harness.root_widget().unwrap().active_window(), Some(b_id));
        harness.mouse_click_on(button, PRIMARY).unwrap();
        assert_eq!(harness.root_widget().unwrap().active_window(), Some(a_id));
        assert_eq!(
            harness.pop_action::<ButtonPress>().unwrap().unwrap().1,
            button
        );
    }

    #[test]
    fn resizing_respects_minimums_and_workspace_bounds() {
        let original = Rect::new(50., 50., 250., 200.);
        let window = Label::new("test").prepare().id();
        let drag = ToolDrag {
            window,
            start: Point::new(50., 50.),
            original,
            kind: DragKind::Resize(ResizeDirection::NorthWest),
        };
        let workspace = Size::new(400., 300.);
        let minimum = Size::new(100., 80.);
        assert_eq!(
            drag_bounds(drag, Point::new(300., 300.), minimum, workspace),
            Rect::new(150., 120., 250., 200.)
        );
        assert_eq!(
            drag_bounds(drag, Point::new(-100., -100.), minimum, workspace),
            Rect::new(0., 0., 250., 200.)
        );
        assert_eq!(
            fit_window(original, minimum, Size::new(40., 30.)),
            Rect::new(0., 0., 40., 30.)
        );
    }
}
