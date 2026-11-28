// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

#![expect(
    missing_debug_implementations,
    reason = "Widgets are not expected to implement Debug"
)]

use std::any::TypeId;

use masonry_core::accesskit::{Node, Role};
use masonry_core::core::{
    AccessCtx, AccessEvent, ActionCtx, ChildrenIds, ComposeCtx, CursorIcon, ErasedAction, EventCtx,
    Layer, LayoutCtx, MeasureCtx, NewWidget, NoAction, PaintCtx, PointerButtonEvent, PointerEvent,
    PropertiesMut, PropertiesRef, QueryCtx, RegisterCtx, TextEvent, Update, UpdateCtx, Widget,
    WidgetId, WidgetPod, WidgetRef, find_widget_under_pointer, pre_paint,
};
use masonry_core::imaging::Painter;
use masonry_core::kurbo::{Axis, Point, Size};
use masonry_core::layout::{LayoutSize, LenReq, Length, SizeDef, UnitPoint};
use tracing::trace_span;

type PointerDownPreviewFn<S> =
    dyn FnMut(&mut S, &mut EventCtx<'_>, &mut PropertiesMut<'_>, &PointerButtonEvent);
type PointerEventFn<S> =
    dyn FnMut(&mut S, &mut EventCtx<'_>, &mut PropertiesMut<'_>, &PointerEvent);
type TextEventFn<S> = dyn FnMut(&mut S, &mut EventCtx<'_>, &mut PropertiesMut<'_>, &TextEvent);
type AccessEventFn<S> = dyn FnMut(&mut S, &mut EventCtx<'_>, &mut PropertiesMut<'_>, &AccessEvent);
type AnimFrameFn<S> = dyn FnMut(&mut S, &mut UpdateCtx<'_>, &mut PropertiesMut<'_>, u64);
type ActionFn<S> =
    dyn FnMut(&mut S, &mut ActionCtx<'_>, &mut PropertiesMut<'_>, &ErasedAction, WidgetId);
type RegisterChildrenFn<S> = dyn FnMut(&mut S, &mut RegisterCtx<'_>);
type UpdateFn<S> = dyn FnMut(&mut S, &mut UpdateCtx<'_>, &mut PropertiesMut<'_>, &Update);
type PropertyChangeFn<S> = dyn FnMut(&mut S, &mut UpdateCtx<'_>, TypeId);
type MeasureFn<S> = dyn FnMut(
    &mut S,
    &mut MeasureCtx<'_>,
    &PropertiesRef<'_>,
    Axis,
    LenReq,
    Option<Length>,
) -> Length;
type LayoutFn<S> = dyn FnMut(&mut S, &mut LayoutCtx<'_>, &PropertiesRef<'_>, Size);
type ComposeFn<S> = dyn FnMut(&mut S, &mut ComposeCtx<'_>);
type PaintFn<S> = dyn FnMut(&mut S, &mut PaintCtx<'_>, &PropertiesRef<'_>, &mut Painter<'_>);
type RoleFn<S> = dyn Fn(&S) -> Role;
type AccessFn<S> = dyn FnMut(&mut S, &mut AccessCtx<'_>, &PropertiesRef<'_>, &mut Node);
type ChildrenFn<S> = dyn Fn(&S) -> ChildrenIds;

/// A widget that can be constructed from individual functions, builder-style.
///
/// This widget is generic over its state, which is passed in at construction time.
#[must_use = "Use the configured test widget."]
pub struct ModularWidget<S> {
    /// The state passed to all the callbacks of this widget
    pub state: S,
    icon: CursorIcon,
    interaction: u8,
    on_pointer_down_preview: Option<Box<PointerDownPreviewFn<S>>>,
    on_pointer_event: Option<Box<PointerEventFn<S>>>,
    on_text_event: Option<Box<TextEventFn<S>>>,
    on_access_event: Option<Box<AccessEventFn<S>>>,
    on_anim_frame: Option<Box<AnimFrameFn<S>>>,
    on_action: Option<Box<ActionFn<S>>>,
    register_children: Option<Box<RegisterChildrenFn<S>>>,
    update: Option<Box<UpdateFn<S>>>,
    property_change: Option<Box<PropertyChangeFn<S>>>,
    measure: Option<Box<MeasureFn<S>>>,
    layout: Option<Box<LayoutFn<S>>>,
    compose: Option<Box<ComposeFn<S>>>,
    pre_paint: Option<Box<PaintFn<S>>>,
    paint: Option<Box<PaintFn<S>>>,
    post_paint: Option<Box<PaintFn<S>>>,
    role: Option<Box<RoleFn<S>>>,
    access: Option<Box<AccessFn<S>>>,
    children: Option<Box<ChildrenFn<S>>>,
}

impl<S> ModularWidget<S> {
    const ACCEPTS_TEXT_INPUT: u8 = 1 << 3;
    const ACCEPTS_FOCUS: u8 = 1 << 2;
    const PROPAGATES_POINTER_INTERACTION: u8 = 1 << 1;
    const ACCEPTS_POINTER_INTERACTION: u8 = 1 << 0;
    /// Creates a new `ModularWidget`.
    ///
    /// By default none of its methods do anything, and its layout method returns
    /// a static 100x100 size.
    pub fn new(state: S) -> Self {
        Self {
            state,
            icon: CursorIcon::Default,
            interaction: 0b0011,
            on_pointer_down_preview: None,
            on_pointer_event: None,
            on_text_event: None,
            on_access_event: None,
            on_anim_frame: None,
            on_action: None,
            register_children: None,
            update: None,
            property_change: None,
            measure: None,
            layout: None,
            compose: None,
            pre_paint: None,
            paint: None,
            post_paint: None,
            role: None,
            access: None,
            children: None,
        }
    }
}

impl<W: Widget + ?Sized> ModularWidget<WidgetPod<W>> {
    /// Creates a new `ModularWidget` with some methods already set to handle a single child.
    pub fn new_parent(child: NewWidget<W>) -> Self {
        let child = child.into_pod();
        Self::new(child)
            .register_children_fn(move |child, ctx| {
                ctx.register_child(child);
            })
            .measure_fn(move |child, ctx, _props, axis, len_req, cross_length| {
                ctx.measure_child(child, axis, len_req, cross_length)
            })
            .layout_fn(move |child, ctx, _props, size| {
                ctx.layout_child(child, size, UnitPoint::TOP_LEFT);
            })
            .children_fn(|child| ChildrenIds::from_slice(&[child.id()]))
    }
}

impl<W: Widget + ?Sized> ModularWidget<Vec<WidgetPod<W>>> {
    /// Creates a new `ModularWidget` with some methods already set to handle multiple children.
    ///
    /// Layout will just stack all children on the same position and return the size of the largest.
    pub fn new_multi_parent(children: Vec<NewWidget<W>>) -> Self {
        let children = children.into_iter().map(NewWidget::into_pod).collect();
        Self::new(children)
            .register_children_fn(move |children, ctx| {
                for child in children {
                    ctx.register_child(child);
                }
            })
            .measure_fn(move |children, ctx, _props, axis, len_req, cross_length| {
                let auto_length = len_req.into();
                let context_size = LayoutSize::maybe(axis.cross(), cross_length);

                let mut length = Length::ZERO;
                for child in children {
                    if ctx.child_is_stashed(child) {
                        continue;
                    }
                    let child_length =
                        ctx.compute_length(child, auto_length, context_size, axis, cross_length);
                    length = length.max(child_length);
                }

                length
            })
            .layout_fn(move |children, ctx, _props, size| {
                let auto_size = SizeDef::fit(size);
                let context_size = size.into();

                for child in children.iter_mut() {
                    if ctx.child_is_stashed(child) {
                        continue;
                    }
                    let child_size = ctx.compute_size(child, auto_size, context_size);
                    ctx.run_layout(child, child_size);
                    ctx.place_child(child, Point::ZERO);
                }

                let mut placements = children
                    .iter()
                    .filter(|child| !ctx.child_is_stashed(child))
                    .filter_map(|child| ctx.child_placement(child));
                if let Some(first) = placements.next() {
                    let last = placements.next_back().unwrap_or(first);
                    ctx.set_baselines(first.parent_baselines().0, last.parent_baselines().1);
                } else {
                    ctx.clear_baselines();
                }
            })
            .children_fn(|children| children.iter().map(WidgetPod::id).collect())
    }
}

/// Builder methods.
///
/// Each method takes a flag which is then returned by the matching [`Widget`] method.
impl<S> ModularWidget<S> {
    /// See [`Widget::get_cursor`]
    pub const fn cursor_icon(mut self, icon: CursorIcon) -> Self {
        self.icon = icon;
        self
    }

    /// See [`Widget::accepts_pointer_interaction`]
    pub const fn accepts_pointer_interaction(mut self, flag: bool) -> Self {
        self.interaction = if flag {
            self.interaction | Self::ACCEPTS_POINTER_INTERACTION
        } else {
            self.interaction & !Self::ACCEPTS_POINTER_INTERACTION
        };
        self
    }

    /// See [`Widget::propagates_pointer_interaction`]
    pub const fn propagates_pointer_interaction(mut self, flag: bool) -> Self {
        self.interaction = if flag {
            self.interaction | Self::PROPAGATES_POINTER_INTERACTION
        } else {
            self.interaction & !Self::PROPAGATES_POINTER_INTERACTION
        };
        self
    }

    /// See [`Widget::accepts_focus`]
    pub const fn accepts_focus(mut self, flag: bool) -> Self {
        self.interaction = if flag {
            self.interaction | Self::ACCEPTS_FOCUS
        } else {
            self.interaction & !Self::ACCEPTS_FOCUS
        };
        self
    }

    /// See [`Widget::accepts_text_input`]
    pub const fn accepts_text_input(mut self, flag: bool) -> Self {
        self.interaction = if flag {
            self.interaction | Self::ACCEPTS_TEXT_INPUT
        } else {
            self.interaction & !Self::ACCEPTS_TEXT_INPUT
        };
        self
    }
}

/// Builder methods.
///
/// Each method takes a callback that matches the behavior of the matching [`Widget`] method.
impl<S> ModularWidget<S> {
    /// See [`Widget::on_pointer_down_preview`]
    pub fn pointer_down_preview_fn(
        mut self,
        f: impl FnMut(&mut S, &mut EventCtx<'_>, &mut PropertiesMut<'_>, &PointerButtonEvent) + 'static,
    ) -> Self {
        self.on_pointer_down_preview = Some(Box::new(f));
        self
    }

    /// See [`Widget::on_pointer_event`]
    pub fn pointer_event_fn(
        mut self,
        f: impl FnMut(&mut S, &mut EventCtx<'_>, &mut PropertiesMut<'_>, &PointerEvent) + 'static,
    ) -> Self {
        self.on_pointer_event = Some(Box::new(f));
        self
    }

    /// See [`Widget::on_text_event`]
    pub fn text_event_fn(
        mut self,
        f: impl FnMut(&mut S, &mut EventCtx<'_>, &mut PropertiesMut<'_>, &TextEvent) + 'static,
    ) -> Self {
        self.on_text_event = Some(Box::new(f));
        self
    }

    /// See [`Widget::on_access_event`]
    pub fn access_event_fn(
        mut self,
        f: impl FnMut(&mut S, &mut EventCtx<'_>, &mut PropertiesMut<'_>, &AccessEvent) + 'static,
    ) -> Self {
        self.on_access_event = Some(Box::new(f));
        self
    }

    /// See [`Widget::on_anim_frame`]
    pub fn anim_frame_fn(
        mut self,
        f: impl FnMut(&mut S, &mut UpdateCtx<'_>, &mut PropertiesMut<'_>, u64) + 'static,
    ) -> Self {
        self.on_anim_frame = Some(Box::new(f));
        self
    }

    /// See [`Widget::on_action`]
    pub fn action_fn(
        mut self,
        f: impl FnMut(&mut S, &mut ActionCtx<'_>, &mut PropertiesMut<'_>, &ErasedAction, WidgetId)
        + 'static,
    ) -> Self {
        self.on_action = Some(Box::new(f));
        self
    }

    /// See [`Widget::register_children`]
    pub fn register_children_fn(
        mut self,
        f: impl FnMut(&mut S, &mut RegisterCtx<'_>) + 'static,
    ) -> Self {
        self.register_children = Some(Box::new(f));
        self
    }

    /// See [`Widget::update`]
    pub fn update_fn(
        mut self,
        f: impl FnMut(&mut S, &mut UpdateCtx<'_>, &mut PropertiesMut<'_>, &Update) + 'static,
    ) -> Self {
        self.update = Some(Box::new(f));
        self
    }

    /// See [`Widget::property_changed`]
    pub fn property_change_fn(
        mut self,
        f: impl FnMut(&mut S, &mut UpdateCtx<'_>, TypeId) + 'static,
    ) -> Self {
        self.property_change = Some(Box::new(f));
        self
    }

    /// See [`Widget::measure`]
    pub fn measure_fn(
        mut self,
        f: impl FnMut(
            &mut S,
            &mut MeasureCtx<'_>,
            &PropertiesRef<'_>,
            Axis,
            LenReq,
            Option<Length>,
        ) -> Length
        + 'static,
    ) -> Self {
        self.measure = Some(Box::new(f));
        self
    }

    /// See [`Widget::layout`]
    pub fn layout_fn(
        mut self,
        f: impl FnMut(&mut S, &mut LayoutCtx<'_>, &PropertiesRef<'_>, Size) + 'static,
    ) -> Self {
        self.layout = Some(Box::new(f));
        self
    }

    /// See [`Widget::compose`]
    pub fn compose_fn(mut self, f: impl FnMut(&mut S, &mut ComposeCtx<'_>) + 'static) -> Self {
        self.compose = Some(Box::new(f));
        self
    }

    /// See [`Widget::pre_paint`]
    pub fn pre_paint_fn(
        mut self,
        f: impl FnMut(&mut S, &mut PaintCtx<'_>, &PropertiesRef<'_>, &mut Painter<'_>) + 'static,
    ) -> Self {
        self.pre_paint = Some(Box::new(f));
        self
    }

    /// See [`Widget::paint`]
    pub fn paint_fn(
        mut self,
        f: impl FnMut(&mut S, &mut PaintCtx<'_>, &PropertiesRef<'_>, &mut Painter<'_>) + 'static,
    ) -> Self {
        self.paint = Some(Box::new(f));
        self
    }

    /// See [`Widget::post_paint`]
    pub fn post_paint_fn(
        mut self,
        f: impl FnMut(&mut S, &mut PaintCtx<'_>, &PropertiesRef<'_>, &mut Painter<'_>) + 'static,
    ) -> Self {
        self.post_paint = Some(Box::new(f));
        self
    }

    /// See [`Widget::accessibility_role`]
    pub fn role_fn(mut self, f: impl Fn(&S) -> Role + 'static) -> Self {
        self.role = Some(Box::new(f));
        self
    }

    /// See [`Widget::accessibility`]
    pub fn access_fn(
        mut self,
        f: impl FnMut(&mut S, &mut AccessCtx<'_>, &PropertiesRef<'_>, &mut Node) + 'static,
    ) -> Self {
        self.access = Some(Box::new(f));
        self
    }

    /// See [`Widget::children_ids`]
    pub fn children_fn(mut self, children: impl Fn(&S) -> ChildrenIds + 'static) -> Self {
        self.children = Some(Box::new(children));
        self
    }
}

#[deny(clippy::missing_trait_methods)]
impl<S: 'static> Widget for ModularWidget<S> {
    type Action = NoAction;

    fn on_pointer_down_preview(
        &mut self,
        ctx: &mut EventCtx<'_>,
        props: &mut PropertiesMut<'_>,
        event: &PointerButtonEvent,
    ) {
        if let Some(f) = self.on_pointer_down_preview.as_mut() {
            f(&mut self.state, ctx, props, event);
        }
    }

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        if let Some(f) = self.on_pointer_event.as_mut() {
            f(&mut self.state, ctx, props, event);
        }
    }

    fn on_text_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        props: &mut PropertiesMut<'_>,
        event: &TextEvent,
    ) {
        if let Some(f) = self.on_text_event.as_mut() {
            f(&mut self.state, ctx, props, event);
        }
    }

    fn on_access_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        props: &mut PropertiesMut<'_>,
        event: &AccessEvent,
    ) {
        if let Some(f) = self.on_access_event.as_mut() {
            f(&mut self.state, ctx, props, event);
        }
    }

    fn on_anim_frame(
        &mut self,
        ctx: &mut UpdateCtx<'_>,
        props: &mut PropertiesMut<'_>,
        interval: u64,
    ) {
        if let Some(f) = self.on_anim_frame.as_mut() {
            f(&mut self.state, ctx, props, interval);
        }
    }

    fn on_action(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        props: &mut PropertiesMut<'_>,
        action: &ErasedAction,
        source: WidgetId,
    ) {
        if let Some(f) = self.on_action.as_mut() {
            f(&mut self.state, ctx, props, action, source);
        }
    }

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        if let Some(f) = self.register_children.as_mut() {
            f(&mut self.state, ctx);
        }
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, props: &mut PropertiesMut<'_>, event: &Update) {
        if let Some(f) = self.update.as_mut() {
            f(&mut self.state, ctx, props, event);
        }
    }

    fn property_changed(&mut self, ctx: &mut UpdateCtx<'_>, property_type: TypeId) {
        if let Some(f) = self.property_change.as_mut() {
            f(&mut self.state, ctx, property_type);
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
        let Self { state, measure, .. } = self;
        measure
            .as_mut()
            .map(|f| f(state, ctx, props, axis, len_req, cross_length))
            .unwrap_or_default()
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, props: &PropertiesRef<'_>, size: Size) {
        if let Some(f) = self.layout.as_mut() {
            f(&mut self.state, ctx, props, size);
        }
    }

    fn compose(&mut self, ctx: &mut ComposeCtx<'_>) {
        if let Some(f) = self.compose.as_mut() {
            f(&mut self.state, ctx);
        }
    }

    fn accessibility_role(&self) -> Role {
        self.role.as_ref().map_or(Role::Unknown, |f| f(&self.state))
    }

    fn accessibility(
        &mut self,
        ctx: &mut AccessCtx<'_>,
        props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        if let Some(f) = self.access.as_mut() {
            f(&mut self.state, ctx, props, node);
        }
    }

    fn pre_paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        if let Some(f) = self.pre_paint.as_mut() {
            f(&mut self.state, ctx, props, painter);
        } else {
            pre_paint(ctx, props, painter);
        }
    }

    fn paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        if let Some(f) = self.paint.as_mut() {
            f(&mut self.state, ctx, props, painter);
        }
    }

    fn post_paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        if let Some(f) = self.post_paint.as_mut() {
            f(&mut self.state, ctx, props, painter);
        }
    }

    fn children_ids(&self) -> ChildrenIds {
        self.children
            .as_ref()
            .map_or_else(ChildrenIds::new, |f| f(&self.state))
    }

    fn as_layer(&mut self) -> Option<&mut dyn Layer> {
        None
    }

    fn accepts_pointer_interaction(&self) -> bool {
        self.interaction & Self::ACCEPTS_POINTER_INTERACTION != 0
    }

    fn propagates_pointer_interaction(&self) -> bool {
        self.interaction & Self::PROPAGATES_POINTER_INTERACTION != 0
    }

    fn accepts_focus(&self) -> bool {
        self.interaction & Self::ACCEPTS_FOCUS != 0
    }

    fn accepts_text_input(&self) -> bool {
        self.interaction & Self::ACCEPTS_TEXT_INPUT != 0
    }

    fn make_trace_span(&self, id: WidgetId) -> tracing::Span {
        trace_span!("ModularWidget", id = id.trace())
    }

    fn get_debug_text(&self) -> Option<String> {
        None
    }

    fn get_cursor(&self, _ctx: &QueryCtx<'_>, _pos: Point) -> CursorIcon {
        self.icon
    }

    fn find_widget_under_pointer<'c>(
        &'c self,
        ctx: QueryCtx<'c>,
        pos: Point,
    ) -> Option<WidgetRef<'c, dyn Widget>> {
        find_widget_under_pointer(self, ctx, pos)
    }

    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn short_type_name(&self) -> &'static str {
        "ModularWidget"
    }

    fn prepare(self) -> NewWidget<Self>
    where
        Self: Sized,
    {
        NewWidget::new(self)
    }
}
