// Copyright 2020 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

//! The context types that are passed into various widget methods.

#![expect(
    missing_debug_implementations,
    reason = "Not sure if it's worth the effort"
)]

use std::any::TypeId;
use std::collections::hash_map::Entry;

use accesskit::{NodeId, TreeUpdate};
use anymore::AnyDebug;
use dpi::{LogicalPosition, PhysicalPosition};
use parley::{FontContext, LayoutContext};
use tracing::{trace, warn};
use tree_arena::{ArenaMutList, ArenaRef, ArenaRefList};

use crate::app::{MutateCallback, PointerCapture, PointerPress, RenderRootSignal, RenderRootState};
use crate::core::{
    AllowRawMut, BrushIndex, ClassSet, ErasedAction, FromDynWidget, LayerPlacement, LayerType,
    NewWidget, PaintLayerMode, PropertiesMut, PropertiesRef, PropertyArena, PropertyCache,
    ResizeDirection, Widget, WidgetArenaNode, WidgetId, WidgetMut, WidgetPod, WidgetRef,
    WidgetState,
};
use crate::kurbo::{Affine, Axis, Insets, Point, Rect, Size, Vec2};
use crate::layout::{LayoutSize, LenDef, LenReq, Length, SizeDef, UnitPoint};
use crate::passes::layout::{place_widget, resolve_length, resolve_size, run_layout_on};
use crate::peniko::Color;
use crate::util::{ParentLinkedList, get_debug_color};

// Note - Most methods defined in this file revolve around `WidgetState` fields.
// Consider reading `WidgetState` documentation (especially the documented naming scheme)
// before editing context method code.

/// A macro for implementing methods on multiple contexts.
///
/// There are a lot of methods defined on multiple contexts; this lets us only
/// have to write them out once.
macro_rules! impl_context_method {
    ($ty:ty,  { $($method:item)+ } ) => {
        impl $ty { $($method)+ }
    };
    ( $ty:ty, $($more:ty),+, { $($method:item)+ } ) => {
        impl_context_method!($ty, { $($method)+ });
        impl_context_method!($($more),+, { $($method)+ });
    };
}

/// A context provided inside of [`WidgetMut`].
///
/// When you declare a mutable reference type for your widget, methods of this type
/// will have access to a `MutateCtx`. If that method mutates the widget in a way that
/// requires a later pass (for instance, if your widget has a `set_color` method),
/// you will need to signal that change in the pass (eg [`request_render`](MutateCtx::request_render)).
pub struct MutateCtx<'a> {
    pub(crate) global_state: &'a mut RenderRootState,
    pub(crate) parent_widget_state: Option<&'a mut WidgetState>,
    pub(crate) widget_state: &'a mut WidgetState,
    pub(crate) properties: PropertiesMut<'a>,
    pub(crate) children: ArenaMutList<'a, WidgetArenaNode>,
    pub(crate) property_arena: &'a PropertyArena,
}

/// A context provided to the [`Widget::on_action`] method.
pub struct ActionCtx<'a> {
    pub(crate) global_state: &'a mut RenderRootState,
    pub(crate) widget_state: &'a mut WidgetState,
    pub(crate) children: ArenaMutList<'a, WidgetArenaNode>,
    pub(crate) property_arena: &'a PropertyArena,
    pub(crate) is_handled: bool,
}

/// A context provided inside of [`WidgetRef`].
///
/// This context is passed to methods of widgets requiring shared, read-only access.
#[derive(Clone, Copy)]
pub struct QueryCtx<'a> {
    pub(crate) global_state: &'a RenderRootState,
    pub(crate) widget_state: &'a WidgetState,
    pub(crate) properties: PropertiesRef<'a>,
    pub(crate) children: ArenaRefList<'a, WidgetArenaNode>,
    pub(crate) property_arena: &'a PropertyArena,
}

/// A context given when calling another context's `get_raw_mut()` method.
pub struct RawCtx<'a> {
    pub(crate) global_state: &'a mut RenderRootState,
    pub(crate) parent_widget_state: &'a mut WidgetState,
    pub(crate) widget_state: &'a mut WidgetState,
    pub(crate) children: ArenaMutList<'a, WidgetArenaNode>,
    pub(crate) property_arena: &'a PropertyArena,
}

/// A context provided to event-handling [`Widget`] methods.
pub struct EventCtx<'a> {
    pub(crate) global_state: &'a mut RenderRootState,
    pub(crate) widget_state: &'a mut WidgetState,
    pub(crate) children: ArenaMutList<'a, WidgetArenaNode>,
    pub(crate) property_arena: &'a PropertyArena,
    pub(crate) target: WidgetId,
    pub(crate) pointer_press: Option<PointerPress>,
    pub(crate) is_handled: bool,
}

/// A context provided to the [`Widget::register_children`] method.
pub struct RegisterCtx<'a> {
    pub(crate) global_state: &'a mut RenderRootState,
    pub(crate) children: ArenaMutList<'a, WidgetArenaNode>,
    #[cfg(debug_assertions)]
    pub(crate) registered_ids: Vec<WidgetId>,
}

/// A context provided to the [`Widget::update`] method.
pub struct UpdateCtx<'a> {
    pub(crate) global_state: &'a mut RenderRootState,
    pub(crate) widget_state: &'a mut WidgetState,
    pub(crate) children: ArenaMutList<'a, WidgetArenaNode>,
    pub(crate) ancestors: Option<&'a ParentLinkedList<'a>>,
    pub(crate) property_arena: &'a PropertyArena,
}

/// A context provided to [`Widget::measure`] methods.
pub struct MeasureCtx<'a> {
    pub(crate) global_state: &'a mut RenderRootState,
    pub(crate) widget_state: &'a mut WidgetState,
    pub(crate) children: ArenaMutList<'a, WidgetArenaNode>,
    pub(crate) property_arena: &'a PropertyArena,
    pub(crate) auto_length: LenDef,
    pub(crate) context_size: LayoutSize,
    pub(crate) cache_result: bool,
}

/// A context provided to [`Widget::layout`] methods.
pub struct LayoutCtx<'a> {
    pub(crate) global_state: &'a mut RenderRootState,
    pub(crate) widget_state: &'a mut WidgetState,
    pub(crate) children: ArenaMutList<'a, WidgetArenaNode>,
    pub(crate) property_arena: &'a PropertyArena,
}

/// A context provided to the [`Widget::compose`] method.
pub struct ComposeCtx<'a> {
    pub(crate) global_state: &'a mut RenderRootState,
    pub(crate) widget_state: &'a mut WidgetState,
    pub(crate) children: ArenaMutList<'a, WidgetArenaNode>,
    pub(crate) property_arena: &'a PropertyArena,
}

/// A context passed to [`Widget::paint`] method.
pub struct PaintCtx<'a> {
    pub(crate) layer_mode: Option<PaintLayerMode>,
    pub(crate) global_state: &'a mut RenderRootState,
    pub(crate) widget_state: &'a mut WidgetState,
    pub(crate) children: ArenaMutList<'a, WidgetArenaNode>,
}

/// A context passed to [`Widget::accessibility`] method.
pub struct AccessCtx<'a> {
    pub(crate) global_state: &'a mut RenderRootState,
    pub(crate) widget_state: &'a mut WidgetState,
    pub(crate) children: ArenaMutList<'a, WidgetArenaNode>,
    pub(crate) tree_update: &'a mut TreeUpdate,
}

// --- MARK: GETTERS
// Methods for all context types
impl_context_method!(
    MutateCtx<'_>,
    ActionCtx<'_>,
    QueryCtx<'_>,
    EventCtx<'_>,
    UpdateCtx<'_>,
    MeasureCtx<'_>,
    LayoutCtx<'_>,
    ComposeCtx<'_>,
    PaintCtx<'_>,
    AccessCtx<'_>,
    RawCtx<'_>,
    {
        /// The `WidgetId` of the current widget.
        #[must_use = "Use the widget identifier."]
        pub const fn widget_id(&self) -> WidgetId {
            self.widget_state.id
        }

        /// Returns the attached layer created by this widget with the given type `W`.
        ///
        /// See [`MutateCtx::create_attached_layer`] for more details.
        #[must_use = "Use the attached layer identifier if present."]
        pub fn get_attached_layer<W: Widget + ?Sized>(&self) -> Option<WidgetId> {
            self.global_state
                .attached_layers
                .get(&self.widget_id())?
                .get(&TypeId::of::<W>())
                .copied()
        }

        /// Returns the local transform of this widget.
        ///
        /// This transform is used during the mapping of this widget's border-box coordinate space
        /// to the parent's border-box coordinate space.
        ///
        /// This transform is applied before `scroll_translation` and `origin`.
        #[must_use = "Use the local transform."]
        pub const fn transform(&self) -> Affine {
            self.widget_state.transform
        }
    }
);

impl_context_method!(
    MutateCtx<'_>,
    ActionCtx<'_>,
    QueryCtx<'_>,
    EventCtx<'_>,
    UpdateCtx<'_>,
    MeasureCtx<'_>,
    LayoutCtx<'_>,
    ComposeCtx<'_>,
    PaintCtx<'_>,
    AccessCtx<'_>,
    {
        /// Helper method to get a direct reference to a child widget's `WidgetState` from its `WidgetPod`.
        #[track_caller]
        fn get_child_state(&self, child: &WidgetPod<impl Widget + ?Sized>) -> Option<&WidgetState> {
            let Some(node) = self.children.item(child.id()) else {
                debug_panic!(
                    "Widget {} has no registered child {}",
                    self.widget_id(),
                    child.id()
                );
                return None;
            };
            Some(&node.item.state)
        }
    }
);

// TODO - Merge into other block?
impl_context_method!(
    MutateCtx<'_>,
    ActionCtx<'_>,
    EventCtx<'_>,
    UpdateCtx<'_>,
    MeasureCtx<'_>,
    LayoutCtx<'_>,
    ComposeCtx<'_>,
    PaintCtx<'_>,
    AccessCtx<'_>,
    RawCtx<'_>,
    {
        /// Returns a mutable reference to this widget's property cache.
        pub const fn property_cache(&mut self) -> &mut PropertyCache {
            &mut self.widget_state.property_cache
        }
    }
);

impl QueryCtx<'_> {
    /// Returns a reference to this widget's property cache.
    #[must_use = "Inspect the cached property resolutions."]
    pub const fn property_cache(&self) -> &PropertyCache {
        &self.widget_state.property_cache
    }
}

impl_context_method!(
    MutateCtx<'_>,
    ActionCtx<'_>,
    EventCtx<'_>,
    UpdateCtx<'_>,
    MeasureCtx<'_>,
    LayoutCtx<'_>,
    ComposeCtx<'_>,
    RawCtx<'_>,
    {
        /// Helper method to get a mutable reference to a child widget's `WidgetState` from its `WidgetPod`.
        ///
        /// This one isn't defined for `PaintCtx` and `AccessCtx` because those contexts
        /// can't mutate `WidgetState`.
        #[track_caller]
        fn get_child_state_mut(
            &mut self,
            child: &WidgetPod<impl Widget + ?Sized>,
        ) -> Option<&mut WidgetState> {
            let parent = self.widget_id();
            let Some(node) = self.children.item_mut(child.id()) else {
                debug_panic!("Widget {parent} has no registered child {}", child.id());
                return None;
            };
            Some(&mut node.item.state)
        }
    }
);

// --- MARK: WIDGET_MUT
// Methods to get a child WidgetMut from a parent.
impl MutateCtx<'_> {
    /// Returns a typed mutable handle to a direct child by its identifier.
    /// Returns None when the child is absent or of another widget type.
    ///
    /// A pod created or replaced during this edit may not be registered until
    /// the next widget-tree pass. Such a pod cannot yet be mutated through a context.
    pub fn get_mut<Child: Widget + FromDynWidget + ?Sized>(
        &mut self,
        child: WidgetId,
    ) -> Option<WidgetMut<'_, Child>> {
        WidgetMut::from_node(
            self.children.item_mut(child)?,
            self.global_state,
            self.property_arena,
            Some(self.widget_state),
        )
    }

    /// Returns a shared typed reference to a registered descendant.
    ///
    /// Returns None for a widget outside this subtree, an absent widget,
    /// or a widget of another type.
    #[must_use = "Inspect the descendant if it exists."]
    pub fn get_descendant<W: Widget + FromDynWidget + ?Sized>(&self, id: WidgetId) -> Option<&W> {
        W::from_dyn(&*self.children.find(id)?.item.widget)
    }

    pub(crate) fn reborrow_mut(&mut self) -> MutateCtx<'_> {
        MutateCtx {
            global_state: self.global_state,
            // We don't don't reborrow `parent_widget_state`. This avoids running
            // `merge_up` in `WidgetMut::Drop` multiple times for the same state.
            // It will still be called when the original borrow is dropped.
            parent_widget_state: None,
            widget_state: self.widget_state,
            properties: PropertiesMut {
                local: &mut *self.properties.local,
                default_map: self.properties.default_map,
                stack: self.properties.stack,
                class_set: self.properties.class_set,
            },
            children: self.children.reborrow_mut(),
            property_arena: self.property_arena,
        }
    }

    pub(crate) const fn update_mut(&mut self) -> UpdateCtx<'_> {
        UpdateCtx {
            global_state: self.global_state,
            widget_state: self.widget_state,
            children: self.children.reborrow_mut(),
            ancestors: None,
            property_arena: self.property_arena,
        }
    }
}

// --- MARK: WIDGET_REF
// Methods to get a child WidgetRef from a parent.
impl<'w> QueryCtx<'w> {
    /// Returns a reference to a registered child, or None if it is absent.
    #[must_use = "Use the child reference if it exists."]
    pub fn get(self, child: WidgetId) -> Option<WidgetRef<'w, dyn Widget>> {
        let node = self.children.item(child)?;
        Some(WidgetRef::from_node(
            node,
            self.global_state,
            self.property_arena,
        ))
    }
}

// Methods for all exclusive context types (i.e. those which have exclusive access to the global state).
impl_context_method!(
    MutateCtx<'_>,
    ActionCtx<'_>,
    EventCtx<'_>,
    UpdateCtx<'_>,
    MeasureCtx<'_>,
    LayoutCtx<'_>,
    ComposeCtx<'_>,
    PaintCtx<'_>,
    AccessCtx<'_>,
    RawCtx<'_>,
    {
        /// Returns the Parley contexts needed to build and paint text sections.
        ///
        /// Most users should embed a pre-built label widget
        /// (such as `masonry::widgets::Label`)
        /// as a child for non-interactive text.
        /// These contexts could however be useful for custom text editing, such as for rich text editing.
        ///
        /// Any cached text layouts should be invalidated when receiving [`Update::FontsChanged`].
        ///
        /// [`Update::FontsChanged`]: crate::core::Update::FontsChanged
        pub const fn text_contexts(
            &mut self,
        ) -> (&mut FontContext, &mut LayoutContext<BrushIndex>) {
            (
                &mut self.global_state.font_context,
                &mut self.global_state.text_layout_context,
            )
        }
    }
);

// --- MARK: EVENT HANDLING
impl EventCtx<'_> {
    /// Captures the pointer in the current widget.
    ///
    /// [Pointer capture] is only allowed during a [`Down`] event. It is a logic error to
    /// capture the pointer during any other event.
    ///
    /// A widget normally only receives pointer events when the pointer is inside the widget's
    /// border-box. Pointer capture causes border-box hit checks to be ignored: when the pointer is
    /// captured by a widget, that widget will continue receiving pointer events when the pointer
    /// is outside the widget's border-box. Other widgets the pointer is over will not receive
    /// events. Events that are not marked as handled by the capturing widget, bubble up to the
    /// widget's ancestors, ignoring their border-boxes as well.
    ///
    /// A window supports one captured interaction at a time. During capture,
    /// unrelated pointers and button presses or releases are ignored. An ancestor
    /// may take over capture while handling the same initiating Down event.
    ///
    /// # Releasing the pointer
    ///
    /// Any widget can [`release`] the pointer during any event.
    /// The pointer is automatically released after the initiating button is released
    /// or the captured pointer is cancelled.
    /// A widget holding the pointer capture will be the target of these events.
    ///
    /// If pointer capture is lost for external reasons (the widget is disabled, the window lost focus,
    /// etc), the widget will still get a [`Cancel`] event.
    ///
    /// [Pointer capture]: crate::doc::masonry_concepts#pointer-capture
    /// [`Down`]: ui_events::pointer::PointerEvent::Down
    /// [`Up`]: ui_events::pointer::PointerEvent::Up
    /// [`Cancel`]: ui_events::pointer::PointerEvent::Cancel
    /// [`release`]: Self::release_pointer
    #[track_caller]
    pub fn capture_pointer(&mut self) {
        let id = self.widget_id();
        let Some(press) = self.pointer_press else {
            debug_panic!("capture_pointer - '{id}': event does not allow pointer capture");
            return;
        };
        // TODO: plumb pointer capture through to platform (through winit)
        self.global_state.pointer_capture = Some(PointerCapture { target: id, press });
        self.global_state.set_needs_pointer_pass(true);
    }

    /// Releases the pointer previously [captured] through [`capture_pointer`].
    ///
    /// [captured]: crate::doc::masonry_concepts#pointer-capture
    /// [`capture_pointer`]: EventCtx::capture_pointer
    pub fn release_pointer(&mut self) {
        let id = self.widget_id();
        if self.global_state.pointer_capture_target().is_none() {
            warn!("release_pointer - '{id}': no widget is captured");
            return;
        }
        if self.global_state.pointer_capture_target() != Some(self.widget_state.id) {
            warn!("release_pointer - '{id}': widget does not have pointer capture");
            return;
        }
        self.global_state.pointer_capture = None;
        self.global_state.set_needs_pointer_pass(true);
    }

    /// The widget originally targeted by the event.
    ///
    /// This will be different from [`widget_id`](Self::widget_id) during event bubbling.
    #[must_use = "Use the event target."]
    pub const fn target(&self) -> WidgetId {
        self.target
    }

    /// Whether the original event target belongs to the subtree of a direct child.
    /// This includes events targeted at the child itself.
    #[must_use = "Use the result to route the event to the appropriate child."]
    pub fn target_in_child(&self, child: &WidgetPod<impl Widget + ?Sized>) -> bool {
        self.children.item(child.id()).is_some_and(|node| {
            child.id() == self.target || node.children.find(self.target).is_some()
        })
    }

    /// Hit-tests a direct child's subtree at a physical pointer position,
    /// independently of pointer capture. Honors the child's transform,
    /// clipping, and pointer-interaction policy.
    #[must_use = "Inspect whether the child is hit."]
    pub fn hit_test_child(
        &self,
        child: &WidgetPod<impl Widget + ?Sized>,
        position: PhysicalPosition<f64>,
    ) -> bool {
        let Some(node) = self.children.item(child.id()) else {
            return false;
        };
        let logical = position.to_logical(self.global_state.scale_factor);
        let child = WidgetRef::from_node(node, self.global_state, self.property_arena);
        !child.ctx().is_disabled()
            && child
                .find_widget_under_pointer(Point::new(logical.x, logical.y))
                .is_some()
    }

    /// Converts the given position from the window's coordinate space
    /// to this widget's content-box coordinate space.
    #[must_use = "Use the transformed event position."]
    pub fn local_position(&self, p: PhysicalPosition<f64>) -> Point {
        let LogicalPosition { x, y } = p.to_logical(self.global_state.scale_factor);
        self.window_to_local(Point { x, y })
    }
}

// Methods shared by event and action handling.
impl_context_method!(ActionCtx<'_>, EventCtx<'_>, {
    /// Sends a signal to parent widgets to scroll this widget's border-box into view.
    pub fn request_scroll_to_this(&mut self) {
        let rect = self.widget_state.border_box();
        self.global_state
            .scroll_request_targets
            .push((self.widget_state.id, rect));
    }

    /// Sends a signal to parent widgets to scroll the provided `rect` into view.
    ///
    /// The `rect` must be in this widget's content-box coordinate space.
    pub fn request_scroll_to(&mut self, rect: Rect) {
        // Convert from this widget's content-box space to border-box space.
        let rect = rect + self.widget_state.border_box_translation();
        self.global_state
            .scroll_request_targets
            .push((self.widget_state.id, rect));
    }

    /// Sets the event as "handled", which stops its propagation to parent
    /// widgets.
    pub fn set_handled(&mut self) {
        trace!("set_handled");
        self.is_handled = true;
    }

    /// Determines whether the event has been handled.
    #[must_use = "Check whether event propagation was stopped."]
    pub const fn is_handled(&self) -> bool {
        self.is_handled
    }

    /// Requests [text focus].
    ///
    /// Because only one widget can be focused at a time, multiple focus requests
    /// from different widgets during a single event cycle means that the last
    /// widget that requests focus will override the previous requests.
    ///
    /// [text focus]: crate::doc::masonry_concepts#text-focus
    pub fn request_focus(&mut self) {
        trace!("request_focus");
        // We need to send the request even if we're currently focused,
        // because we may have a sibling widget that already requested focus
        // and we have no way of knowing that yet. We need to override that
        // to deliver on the "last focus request wins" promise.
        let id = self.widget_id();
        self.global_state.next_focused_widget = Some(id);
    }

    /// Transfers [text focus] to the widget with the given `WidgetId`.
    ///
    /// [text focus]: crate::doc::masonry_concepts#text-focus
    pub fn set_focus(&mut self, target: WidgetId) {
        trace!("set_focus target={:?}", target);
        self.global_state.next_focused_widget = Some(target);
    }

    /// Gives up [text focus].
    ///
    /// This should only be called by a widget that currently has focus.
    ///
    /// [text focus]: crate::doc::masonry_concepts#text-focus
    pub fn resign_focus(&mut self) {
        trace!("resign_focus");
        if self.has_focus_target() {
            self.global_state.next_focused_widget = None;
        } else {
            warn!(
                "resign_focus can only be called by the currently focused widget {} \
                 or one of its ancestors.",
                self.widget_id()
            );
        }
    }
});

// --- MARK: ACCESSIBILITY
impl AccessCtx<'_> {
    // TODO - We need access to the TreeUpdate to create sub-nodes for text runs,
    // but this seems too powerful. We should figure out another API.
    /// A mutable reference to the global [`TreeUpdate`] object in which all modified/new
    /// accessibility nodes are stored.
    pub const fn tree_update(&mut self) -> &mut TreeUpdate {
        self.tree_update
    }

    /// Returns an id which is guaranteed not to collide with [`WidgetId`]s or with previous ids returned by this function.
    #[must_use = "Assign the new identifier to an accessibility node."]
    pub fn next_node_id() -> NodeId {
        // TODO - Return from a pool disjoint from widget ids.
        WidgetId::next().into()
    }
}

// --- MARK: COMPUTE LENGTH
impl_context_method!(MeasureCtx<'_>, LayoutCtx<'_>, {
    /// Computes the `child`'s preferred border-box [`Length`] on the given `axis`.
    ///
    /// Container widgets usually call this method as part of their [`measure`] logic,
    /// to help them calculate their own length on the given `axis`. They call it as part
    /// of their [`layout`] logic if they have already chosen a length for one axis.
    /// Read [`measure`] and [`layout`] docs for more details about those processes.
    ///
    /// `auto_length` specifies the fallback behavior if the child's dimension is [`Dim::Auto`].
    /// If you're calling this from within [`measure`] then you usually want to derive
    /// this from `len_req`, probably using [`LenReq::reduce`], i.e. you would call
    /// `len_req.reduce(used_space_on_this_axis).into()`. However, if you're calling this
    /// from within [`layout`] then you usually want to use use [`LenDef::FitContent`]
    /// to ask the child to fit inside the available space. Sometimes a different fallback
    /// makes more sense, e.g. `Grid` uses [`LenDef::Fixed`] to fall back to the exact
    /// allocated child area size.
    ///
    /// `context_size` is the size that is used to resolve relative sizes.
    /// For example [`Ratio(0.5)`] will result in half the context size.
    /// This is usually the container widget's content-box size, i.e. excluding borders and padding.
    /// Examples of exceptions include `Grid` which will provide the child's area size,
    /// i.e. the union of cell sizes that the child occupies, and `Portal` which will provide
    /// its viewport size.
    ///
    /// `cross_length` is the length of the cross axis and is critical information for certain
    /// widgets, e.g. for text max advance or to keep an aspect ratio.
    ///
    /// [`measure`]: Widget::measure
    /// [`layout`]: Widget::layout
    /// [`Ratio(0.5)`]: crate::layout::Dim::Ratio
    /// [`Dim::Auto`]: crate::layout::Dim
    /// [`LenReq::reduce`]: crate::layout::LenReq::reduce
    pub fn compute_length(
        &mut self,
        child: &WidgetPod<impl Widget + ?Sized>,
        auto_length: LenDef,
        context_size: LayoutSize,
        axis: Axis,
        cross_length: Option<Length>,
    ) -> Length {
        let parent = self.widget_id();
        let Some(node) = self.children.item_mut(child.id()) else {
            debug_panic!(
                "Widget {parent} cannot measure unregistered child {}",
                child.id()
            );
            return Length::ZERO;
        };
        resolve_length(
            self.global_state,
            self.property_arena,
            node,
            auto_length,
            context_size,
            axis,
            cross_length,
        )
    }
});

// --- MARK: MEASURE
impl MeasureCtx<'_> {
    /// Measures a child within this widget's local content box.
    ///
    /// The child's automatic size follows the current length request and its
    /// relative cross-axis size resolves against the available cross length.
    /// Unlike [`redirect_measurement`](Self::redirect_measurement), this does not
    /// propagate the outer container's measurement context or disable caching.
    pub fn measure_child(
        &mut self,
        child: &WidgetPod<impl Widget + ?Sized>,
        axis: Axis,
        len_req: LenReq,
        cross_length: Option<Length>,
    ) -> Length {
        self.compute_length(
            child,
            len_req.into(),
            LayoutSize::maybe(axis.cross(), cross_length),
            axis,
            cross_length,
        )
    }

    /// Returns the fallback [`LenDef`] of this measurement.
    ///
    /// This is essential information for widgets that want pass through measurements.
    /// That is, this fallback was chosen by widget A for widget B.
    /// Widget B can call this method and then pass it on via [`compute_length`] to widget C.
    /// Widget C will resolve its size with the fallback originally provided by widget A.
    ///
    /// Even easier, though, is to use [`redirect_measurement`].
    ///
    /// Calling `auto_length` will cause the result of this [`measure`] to not be cached.
    /// This is because `auto_length` is not part of the cache key.
    ///
    /// [`measure`]: Widget::measure
    /// [`compute_length`]: Self::compute_length
    /// [`redirect_measurement`]: Self::redirect_measurement
    pub const fn auto_length(&mut self) -> LenDef {
        // We're adding a new variable, auto_length, into the measure function,
        // which is not part of the cache key. Hence, we need to not cache.
        self.cache_result = false;
        self.auto_length
    }

    /// Returns the context size of this measurement.
    ///
    /// This is usually the container widget's content-box size, i.e. excluding borders and padding.
    ///
    /// Examples of exceptions include `Grid` which will provide the child's area size,
    /// i.e. the union of cell sizes that the child occupies, and `Portal` which will provide
    /// its viewport size.
    ///
    /// The context size is used to resolve relative lengths, e.g. a width of [`Ratio(0.5)`]
    /// means half of this space. This resolving is done by Masonry and not manually in [`measure`].
    ///
    /// One or both lengths may be missing if they have not been computed yet,
    /// i.e. when the lengths depend on a child's size.
    ///
    /// This is essential information for widgets that want pass through measurements.
    /// That is, this context size was chosen by widget A for widget B.
    /// Widget B can call this method and then pass it on via [`compute_length`] to widget C.
    /// Widget C will resolve its size in relation to widget A.
    ///
    /// Even easier, though, is to use [`redirect_measurement`].
    ///
    /// Calling `context_size` will cause the result of this [`measure`] to not be cached.
    /// This is because `context_size` is not part of the cache key.
    ///
    /// [`measure`]: Widget::measure
    /// [`Ratio(0.5)`]: crate::layout::Dim::Ratio
    /// [`compute_length`]: Self::compute_length
    /// [`redirect_measurement`]: Self::redirect_measurement
    pub const fn context_size(&mut self) -> LayoutSize {
        // We're adding a new variable, context_size, into the measure function,
        // which is not part of the cache key. Hence, we need to not cache.
        self.cache_result = false;
        self.context_size
    }

    /// Configures whether this [`measure`] result will be cached.
    ///
    /// Masonry will, by default, cache the results of measurement. The cache key is derived
    /// from `axis`, `len_req`, and `cross_length`. If the widget uses any other data to influence
    /// the result of the measurement, then the widget is responsible for requesting layout
    /// when any of that data changes. For properties, this is handled in [`property_changed`].
    /// For any other data you reference, the exact mechanism of detecting changes is up to you.
    /// If you can't detect changes of the referenced data, disable the cache with this method.
    ///
    /// [`measure`]: Widget::measure
    /// [`property_changed`]: Widget::property_changed
    pub const fn cache_result(&mut self, cache_result: bool) {
        self.cache_result = cache_result;
    }

    /// Redirects the measurement request to a `child`.
    ///
    /// This is meant for thin wrapper widgets that want their children measured instead.
    ///
    /// It is a convenience wrapper over [`compute_length`] that automatically configures
    /// `auto_length` and `context_size` to whatever the outer container used. These could also
    /// be manually accessed via [`auto_length`] and [`context_size`] if you're so inclined.
    ///
    /// Calling `redirect_measurement` will cause the result of your [`measure`] to not be cached.
    /// The child's measurement might still be cached, depending on what policy the child chooses.
    /// This is because the redirection introduces new inputs in the form of [`auto_length`]
    /// and [`context_size`] that are not part of the cache key.
    ///
    /// [`measure`]: Widget::measure
    /// [`compute_length`]: Self::compute_length
    /// [`auto_length`]: Self::auto_length
    /// [`context_size`]: Self::context_size
    pub fn redirect_measurement(
        &mut self,
        child: &WidgetPod<impl Widget + ?Sized>,
        axis: Axis,
        cross_length: Option<Length>,
    ) -> Length {
        // We're adding two new variables, auto_length and context_size, into the measure function,
        // which are not part of the cache key. Hence, we need to not cache.
        self.cache_result = false;
        self.compute_length(
            child,
            self.auto_length,
            self.context_size,
            axis,
            cross_length,
        )
    }
}

/// The child's resolved, unaligned border-box geometry after layout.
#[derive(Debug, Clone, Copy)]
pub struct ChildLayout {
    /// Resolved border-box size in logical pixels.
    pub size: Size,
    /// First and last baselines measured from the top of the layout border box.
    pub baselines: (f64, f64),
}

/// The child's aligned geometry after placement.
#[derive(Debug, Clone, Copy)]
pub struct ChildPlacement {
    /// Origin relative to the parent's content box.
    pub origin: Point,
    /// First and last baselines measured from the top of the aligned border box.
    pub baselines: (f64, f64),
}

impl ChildPlacement {
    /// Baselines relative to the parent content box.
    #[must_use = "Use the parent-relative baselines."]
    pub fn parent_baselines(self) -> (f64, f64) {
        (
            self.origin.y + self.baselines.0,
            self.origin.y + self.baselines.1,
        )
    }
}

// --- MARK: UPDATE LAYOUT
impl LayoutCtx<'_> {
    #[track_caller]
    fn laid_out_child(
        &self,
        child: &WidgetPod<impl Widget + ?Sized>,
        method_name: &str,
    ) -> Option<&WidgetState> {
        let state = self.get_child_state(child)?;
        // In debug builds, request_layout remains true until visited.
        if state.needs_layout() || (cfg!(debug_assertions) && state.request_layout()) {
            debug_panic!(
                "Error in {}: trying to call '{}' with child {} before computing its layout",
                self.widget_id(),
                method_name,
                child.id()
            );
            return None;
        }
        Some(state)
    }

    #[track_caller]
    fn placed_child(
        &self,
        child: &WidgetPod<impl Widget + ?Sized>,
        method_name: &str,
    ) -> Option<&WidgetState> {
        let state = self.laid_out_child(child, method_name)?;
        if state.is_expecting_place_child_call() {
            debug_panic!(
                "Error in {}: trying to call '{}' with child {} before placing it",
                self.widget_id(),
                method_name,
                child.id()
            );
            return None;
        }
        Some(state)
    }

    /// Lays out one child at its preferred size within the given content box.
    ///
    /// Aligns the child in the remaining space and forwards its baselines to
    /// the parent. For oversized children, alignment also controls overflow.
    pub fn layout_child(
        &mut self,
        child: &WidgetPod<impl Widget + ?Sized>,
        size: Size,
        alignment: UnitPoint,
    ) {
        let child_size = self.compute_size(child, SizeDef::fit(size), size.into());
        self.run_layout(child, child_size);
        let remaining = size - child_size;
        let origin = alignment.resolve(Rect::new(0.0, 0.0, remaining.width, remaining.height));
        self.place_child(child, origin);
        self.derive_baselines(child);
    }

    /// Computes the `child`'s preferred border-box size.
    ///
    /// The returned size will be finite, non-negative, and in logical pixels.
    /// An unregistered child produces a debug diagnostic and zero size in release builds.
    ///
    /// Container widgets usually call this method as part of their [`layout`] logic, but
    /// ultimately they can disregard the result and pass a different size to [`run_layout`].
    /// Read [`layout`] docs for more details about that process.
    ///
    /// `auto_size` specifies the fallback behavior if the child has any dimension as [`Dim::Auto`].
    /// Most widgets should use [`SizeDef::fit`] to ask the child to fit inside the
    /// available space. However sometimes a different fallback makes more sense, e.g.
    /// `Grid` uses [`SizeDef::fixed`] to fall back to the exact allocated child area size.
    ///
    /// `context_size` is the size that is used to resolve relative sizes.
    /// For example [`Ratio(0.5)`] will result in half the context size.
    /// This is usually the container widget's content-box size, i.e. excluding borders and padding.
    /// Examples of exceptions include `Grid` which will provide the child's area size,
    /// i.e. the union of cell sizes that the child occupies, and `Portal` which will provide
    /// its viewport size.
    ///
    /// [`layout`]: Widget::layout
    /// [`Ratio(0.5)`]: crate::layout::Dim::Ratio
    /// [`Dim::Auto`]: crate::layout::Dim::Auto
    /// [`run_layout`]: Self::run_layout
    pub fn compute_size(
        &mut self,
        child: &WidgetPod<impl Widget + ?Sized>,
        auto_size: SizeDef,
        context_size: LayoutSize,
    ) -> Size {
        let parent = self.widget_id();
        let Some(node) = self.children.item_mut(child.id()) else {
            debug_panic!(
                "Widget {parent} cannot measure unregistered child {}",
                child.id()
            );
            return Size::ZERO;
        };
        resolve_size(
            self.global_state,
            self.property_arena,
            node,
            auto_size,
            context_size,
        )
    }

    /// Lays out the `child` widget with a chosen border-box `size`.
    ///
    /// Container widgets must call this on every child as part of their [`layout`] method.
    ///
    /// The container widget should usually call [`compute_size`] on the `child`
    /// to get its preferred border-box `size`.
    /// However, ultimately the parent is in control and can choose any `size` for the child.
    ///
    /// If the chosen border-box `size` is smaller than what is required to fit the child's
    /// borders and padding, then the `size` will be expanded to meet those constraints.
    ///
    /// The provided `size` must be finite, non-negative, and in logical pixels.
    /// Non-finite or negative size will fall back to zero with a logged warning.
    ///
    /// # Panics
    ///
    /// Panics if the provided `size` is non-finite or negative and debug assertions are enabled.
    ///
    /// [`layout`]: Widget::layout
    /// [`compute_size`]: Self::compute_size
    pub fn run_layout(&mut self, child: &WidgetPod<impl Widget + ?Sized>, chosen_size: Size) {
        let parent = self.widget_id();
        let Some(mut node) = self.children.item_mut(child.id()) else {
            debug_panic!(
                "Widget {parent} cannot lay out unregistered child {}",
                child.id()
            );
            return;
        };
        run_layout_on(
            self.global_state,
            self.property_arena,
            node.reborrow_mut(),
            chosen_size,
        );
        self.widget_state.merge_up(&node.item.state);
    }

    /// Sets the position of the `child` widget, in this widget's content-box coordinate space.
    ///
    /// Container widgets must call this method with each non-stashed child in their
    /// [`layout`] method, after calling `ctx.run_layout(child, size)`.
    ///
    /// # Panics
    ///
    /// This method will panic if [`LayoutCtx::run_layout`] has not been called yet for the child.
    ///
    /// [`layout`]: Widget::layout
    #[track_caller]
    pub fn place_child(&mut self, child: &WidgetPod<impl Widget + ?Sized>, origin: Point) {
        if self.laid_out_child(child, "place_child").is_none() {
            return;
        }
        // Convert the content-box origin to border-box space before checking it.
        let child_origin = origin + self.widget_state.border_box_translation();
        if !child_origin.x.is_finite() || !child_origin.y.is_finite() {
            debug_panic!(
                "Error in {}: trying to call 'place_child' with child {} with invalid origin {:?}",
                self.widget_id(),
                child.id(),
                origin
            );
            return;
        }
        if let Some(state) = self.get_child_state_mut(child) {
            place_widget(state, child_origin);
        }
    }

    /// Sets explicit paint [`Insets`] for this widget.
    ///
    /// The argument is an [`Insets`] struct that indicates where your widget will overpaint,
    /// relative to its layout content-box, as defined by the `size` given to the widget's
    /// [`layout`] method.
    ///
    /// You are only required to notify of painting that actually overflows the layout border-box.
    /// The insets will still be relative to the content-box, it's just that Masonry doesn't
    /// really need to be notified if you're just painting over your padding or borders.
    ///
    /// You are only required to notify of painting done directly by this widget.
    /// Child widget overdraw needs to be reported by those child widgets themselves.
    ///
    /// [`layout`]: Widget::layout
    pub fn set_paint_insets(&mut self, insets: impl Into<Insets>) {
        let insets = insets.into();
        let insets = Insets::new(
            insets.x0 - self.widget_state.border_box_insets.x0,
            insets.y0 - self.widget_state.border_box_insets.y0,
            insets.x1 - self.widget_state.border_box_insets.x1,
            insets.y1 - self.widget_state.border_box_insets.y1,
        );
        self.widget_state.paint_box_insets = insets.nonnegative();
    }

    /// Sets explicit baselines for this widget.
    ///
    /// The baseline positions are used to align widgets that contain text,
    /// such as buttons, labels, and other controls. It may also be used
    /// by other widgets that are opinionated about how they are aligned
    /// relative to neighbouring text, such as switches or checkboxes.
    ///
    /// The provided values must be the distance from the top of this
    /// widget's content-box to its baseline. If there aren't multiple
    /// baselines then set the same baseline as both `first` and `last`.
    ///
    /// Most container widgets can use [`derive_baselines`] instead.
    /// Multi-child containers should derive their baselines using [`child_placement`].
    ///
    /// [`derive_baselines`]: Self::derive_baselines
    /// [`child_placement`]: Self::child_placement
    pub fn set_baselines(&mut self, first_baseline: f64, last_baseline: f64) {
        self.widget_state.first_baseline = first_baseline + self.widget_state.border_box_insets.y0;
        self.widget_state.last_baseline = last_baseline + self.widget_state.border_box_insets.y0;
    }

    /// Sets explicit baselines for this widget such that they match the child's aligned baselines.
    ///
    /// Most container widgets should use this method to derive their baselines from their child.
    /// More complex containers can use [`child_placement`] in multi-child scenarios.
    ///
    /// # Panics
    ///
    /// This method will panic if [`LayoutCtx::run_layout`] or [`LayoutCtx::place_child`]
    /// have not been called yet for the child.
    ///
    /// [`child_placement`]: Self::child_placement
    #[track_caller]
    pub fn derive_baselines(&mut self, child: &WidgetPod<impl Widget + ?Sized>) {
        if let Some(placement) = self.child_placement(child) {
            let (first, last) = placement.parent_baselines();
            self.set_baselines(first, last);
        } else {
            self.clear_baselines();
        }
    }

    /// Clears explicitly set baseline positions for this widget.
    ///
    /// This results in the effective baseline being the bottom edge of this widget's border-box.
    pub const fn clear_baselines(&mut self) {
        self.widget_state.first_baseline = f64::NAN;
        self.widget_state.last_baseline = f64::NAN;
    }

    /// Returns the insets for converting between content-box and border-box rects.
    ///
    /// Add these insets to the content-box to get the border-box,
    /// and subtract these insets from the border-box to get the content-box.
    #[must_use = "Use the insets to convert between coordinate spaces."]
    pub const fn border_box_insets(&self) -> Insets {
        self.widget_state.border_box_insets
    }

    /// Returns whether this widget needs to call [`LayoutCtx::run_layout`].
    #[must_use = "Check whether layout must be run."]
    pub const fn needs_layout(&self) -> bool {
        self.widget_state.needs_layout()
    }

    /// Returns whether a child of this widget needs to call [`LayoutCtx::run_layout`].
    #[must_use = "Check whether the child requires layout."]
    pub fn child_needs_layout(&self, child: &WidgetPod<impl Widget + ?Sized>) -> bool {
        self.get_child_state(child)
            .is_some_and(WidgetState::needs_layout)
    }

    /// Returns the child's resolved layout size and unaligned baselines.
    ///
    /// Returns None for an absent or not yet laid out child; debug builds
    /// report the invalid layout sequence.
    #[track_caller]
    #[must_use = "Use the child's resolved layout geometry."]
    pub fn child_layout(&self, child: &WidgetPod<impl Widget + ?Sized>) -> Option<ChildLayout> {
        let state = self.laid_out_child(child, "child_layout")?;
        Some(ChildLayout {
            size: state.layout_border_box_size,
            baselines: (state.layout_first_baseline(), state.layout_last_baseline()),
        })
    }

    /// Returns the child's placed origin and aligned baselines.
    ///
    /// The origin is relative to this widget's content box; the baselines
    /// are relative to the child's aligned border box.
    /// Returns None for absent or unplaced children, with a debug diagnostic.
    #[track_caller]
    #[must_use = "Use the child's placed geometry."]
    pub fn child_placement(
        &self,
        child: &WidgetPod<impl Widget + ?Sized>,
    ) -> Option<ChildPlacement> {
        let state = self.placed_child(child, "child_placement")?;
        Some(ChildPlacement {
            origin: state.origin - self.widget_state.border_box_translation(),
            baselines: (
                state.aligned_first_baseline(),
                state.aligned_last_baseline(),
            ),
        })
    }

    /// Sets the widget's clip path in the widget's content-box coordinate space.
    ///
    /// A widget's clip path will have two effects:
    /// - It serves as a mask for painting operations of this widget and its children.
    ///   Note that while all painting done by children will be clipped by this path,
    ///   only the painting done in [`paint`] by this widget itself will be clipped.
    ///   The remaining painting done in [`pre_paint`] and [`post_paint`] will not be clipped.
    /// - Pointer events must be inside this path to reach the widget's children.
    ///
    /// [`paint`]: Widget::paint
    /// [`pre_paint`]: Widget::pre_paint
    /// [`post_paint`]: Widget::post_paint
    pub fn set_clip_path(&mut self, path: Rect) {
        // Translate the clip path to the widget's border-box coordinate space.
        let path = path + self.widget_state.border_box_translation();
        // We intentionally always log this because clip paths are:
        // 1) Relatively rare in the tree
        // 2) An easy potential source of items not being visible when expected
        trace!("set_clip_path {path:?}");
        self.widget_state.clip_path = Some(path);
        // TODO - Updating the clip path may have
        // other knock-on effects we'd need to document.
        self.widget_state.set_request_accessibility(true);
        self.widget_state.set_needs_accessibility(true);
        self.widget_state.set_needs_paint(true);
    }

    /// Removes the widget's clip path.
    ///
    /// See [`LayoutCtx::set_clip_path`] for details.
    pub fn clear_clip_path(&mut self) {
        trace!("clear_clip_path");
        self.widget_state.clip_path = None;
        // TODO - Updating the clip path may have
        // other knock-on effects we'd need to document.
        self.widget_state.set_request_accessibility(true);
        self.widget_state.set_needs_accessibility(true);
        self.widget_state.set_needs_paint(true);
    }
}

impl ComposeCtx<'_> {
    /// Sets the scroll translation for the child widget.
    ///
    /// The translation is applied on top of the position from [`LayoutCtx::place_child`].
    ///
    /// The given translation may be quantized so the child's final position
    /// stays pixel-perfect.
    pub fn set_child_scroll_translation(
        &mut self,
        child: &WidgetPod<impl Widget + ?Sized>,
        translation: Vec2,
    ) {
        self.set_animated_child_scroll_translation(child, translation.round());
    }

    /// Sets the scroll translation for the child widget.
    ///
    /// The translation is applied on top of the position from [`LayoutCtx::place_child`].
    ///
    /// Unlike [`Self::set_child_scroll_translation`], doesn't perform pixel-snapping.
    /// This method should be used for intermediary scroll values during scroll animations.
    pub fn set_animated_child_scroll_translation(
        &mut self,
        child: &WidgetPod<impl Widget + ?Sized>,
        translation: Vec2,
    ) {
        if !translation.x.is_finite() || !translation.y.is_finite() {
            debug_panic!(
                "Widget {} cannot set child {} scroll translation to {:?}",
                self.widget_id(),
                child.id(),
                translation
            );
            return;
        }
        if let Some(child) = self.get_child_state_mut(child)
            && translation != child.scroll_translation
        {
            child.scroll_translation = translation;
            child.set_transform_changed(true);
            child.set_needs_compose(true);
        }
    }
}

// --- MARK: GET GEOMETRY
// Methods on all context types except MeasureCtx and LayoutCtx
// These methods access geometry resolved during layout and compose.
impl_context_method!(
    MutateCtx<'_>,
    ActionCtx<'_>,
    QueryCtx<'_>,
    EventCtx<'_>,
    UpdateCtx<'_>,
    ComposeCtx<'_>,
    PaintCtx<'_>,
    AccessCtx<'_>,
    {
        /// Returns the aligned content-box rect of this widget
        /// in this widget's content-box coordinate space.
        #[must_use = "Use the content-box geometry."]
        pub fn content_box(&self) -> Rect {
            let translation = self.widget_state.border_box_translation();
            self.widget_state.content_box() - translation
        }

        /// Returns the aligned border-box rect of this widget
        /// in this widget's content-box coordinate space.
        #[must_use = "Use the border-box geometry."]
        pub fn border_box(&self) -> Rect {
            let translation = self.widget_state.border_box_translation();
            self.widget_state.border_box() - translation
        }

        /// Returns the aligned paint-box rect of this widget
        /// in this widget's content-box coordinate space.
        ///
        /// Covers the area we expect to be invalidated when the widget is painted.
        #[must_use = "Use the paint bounds."]
        pub fn paint_box(&self) -> Rect {
            let translation = self.widget_state.border_box_translation();
            self.widget_state.paint_box() - translation
        }

        /// Returns the widget's bounding-box rect in the window's coordinate space.
        ///
        /// It contains this widget and all of its descendents.
        ///
        /// This is the union of clipped effective paint-box rects, i.e. the union of
        /// globally transformed aligned border-box rects with paint insets applied.
        ///
        /// See [bounding box documentation] for more details.
        ///
        /// [bounding box documentation]: crate::doc::masonry_concepts#bounding-box
        #[must_use = "Use the window-space bounds."]
        pub const fn bounding_box(&self) -> Rect {
            self.widget_state.bounding_box
        }

        /// Returns the first baseline relative to the top of the widget's aligned content-box.
        #[must_use = "Use the first baseline."]
        pub fn first_baseline(&self) -> f64 {
            let border_box_baseline = self.widget_state.aligned_first_baseline();
            border_box_baseline - self.widget_state.border_box_insets.y0
        }

        /// Returns the last baseline relative to the top of the widget's aligned content-box.
        #[must_use = "Use the last baseline."]
        pub fn last_baseline(&self) -> f64 {
            let border_box_baseline = self.widget_state.aligned_last_baseline();
            border_box_baseline - self.widget_state.border_box_insets.y0
        }

        /// The clip path of the widget, if any was set.
        ///
        /// The returned clip path will be in this widget's content-box coordinate space.
        ///
        /// For more information, see
        /// [`LayoutCtx::set_clip_path`](crate::core::LayoutCtx::set_clip_path).
        #[must_use = "Use the clip geometry if present."]
        pub fn clip_path(&self) -> Option<Rect> {
            // Translate the clip path to the widget's content-box coordinate space.
            let translation = self.widget_state.border_box_translation();
            self.widget_state.clip_path.map(|path| path - translation)
        }

        /// Returns the [`Vec2`] for translating between this widget's
        /// content-box and border-box coordinate spaces.
        ///
        /// Add this [`Vec2`] to translate from content-box to border-box,
        /// and subtract this [`Vec2`] to translate from border-box to content-box.
        #[must_use = "Use the content-to-border translation."]
        pub const fn border_box_translation(&self) -> Vec2 {
            self.widget_state.border_box_translation()
        }

        /// Returns the global transform mapping this widget's content-box coordinate space
        /// to the window's coordinate space.
        ///
        /// Computed from all `transform`, `scroll_translation`, and `origin` values
        /// from this widget all the way up to the window.
        ///
        /// Multiply by this to convert from this widget's content-box coordinate space to the window's,
        /// or use the inverse of this transform to go from window's space to this widget's content-box.
        #[must_use = "Use the window-space transform."]
        pub fn window_transform(&self) -> Affine {
            let translation = self.widget_state.border_box_translation();
            self.widget_state
                .window_transform
                .pre_translate(translation)
        }

        /// Converts the `point` from the window's coordinate space
        /// to this widget's content-box coordinate space.
        #[must_use = "Use the point in local coordinates."]
        pub fn window_to_local(&self, point: Point) -> Point {
            let to_border_box = self.widget_state.window_transform.inverse();
            let to_content_box = -self.widget_state.border_box_translation();
            to_border_box.then_translate(to_content_box) * point
        }

        /// Converts the `point` from this widget's content-box coordinate space
        /// to the window's coordinate space.
        ///
        /// The returned point is relative to the window's content area; it excludes window chrome.
        #[must_use = "Use the point in window coordinates."]
        pub fn local_to_window(&self, point: Point) -> Point {
            let translation = self.widget_state.border_box_translation();
            self.widget_state.window_transform * (point + translation)
        }

        /// Returns the DPI scaling factor.
        ///
        /// This can be useful for loading image resources meant for a specific scale.
        #[must_use = "Use the DPI scale factor."]
        pub const fn scale_factor(&self) -> f64 {
            self.global_state.scale_factor
        }
    }
);

// --- MARK: GET STATUS

// Methods on all context types
// Access status information (hovered/pointer captured/disabled/etc).
impl_context_method!(
    MutateCtx<'_>,
    ActionCtx<'_>,
    QueryCtx<'_>,
    EventCtx<'_>,
    UpdateCtx<'_>,
    MeasureCtx<'_>,
    LayoutCtx<'_>,
    ComposeCtx<'_>,
    PaintCtx<'_>,
    AccessCtx<'_>,
    {
        /// The "hovered" status of a widget.
        ///
        /// A widget is "hovered" when a pointer is hovering over its border-box.
        /// Widgets will often change their appearance as a visual indication that they
        /// will respond to pointer (usually mouse) interaction.
        ///
        /// If the pointer is [captured], then only the capturing widget can have hovered
        /// status. If the pointer is captured but not hovering over the captured
        /// widget, then no widget has the hovered status.
        ///
        /// [captured]: crate::doc::masonry_concepts#pointer-capture
        #[must_use = "Inspect the widget's hover state."]
        pub const fn is_hovered(&self) -> bool {
            self.widget_state.is_hovered()
        }

        /// Whether this widget or any of its descendants are hovered.
        ///
        /// To check if only this specific widget is hovered use [`is_hovered`](Self::is_hovered).
        #[must_use = "Inspect hover state in the subtree."]
        pub const fn has_hovered(&self) -> bool {
            self.widget_state.has_hovered()
        }

        /// The "active" status of a widget.
        ///
        /// A widget is "active" when the user is "actively" interacting with it.
        /// Currently, a widget is determined to be active when it has [captured] a pointer,
        /// but this may change in the future to account for e.g. keyboard interactions.
        ///
        /// [captured]: crate::doc::masonry_concepts#pointer-capture
        #[must_use = "Inspect the widget's active state."]
        pub const fn is_active(&self) -> bool {
            self.widget_state.is_active()
        }

        /// Whether this widget or any of its descendants are active.
        ///
        /// To check if only this specific widget is active use [`is_active`](Self::is_active).
        #[must_use = "Inspect active state in the subtree."]
        pub const fn has_active(&self) -> bool {
            self.widget_state.has_active()
        }

        /// Whether a pointer is [captured] by this widget.
        ///
        /// The pointer will usually be the mouse. In future versions, this
        /// function will take a pointer id as input to test a specific pointer.
        ///
        /// [captured]: crate::doc::masonry_concepts#pointer-capture
        #[must_use = "Check whether this widget captures the pointer."]
        pub fn is_pointer_capture_target(&self) -> bool {
            self.global_state.pointer_capture_target() == Some(self.widget_state.id)
        }

        /// The [text focus] status of a widget.
        ///
        /// The focused widget is the one that receives keyboard events.
        ///
        /// Returns `true` if this specific widget is focused.
        /// To check if any descendants are focused use [`has_focus_target`].
        ///
        /// [text focus]: crate::doc::masonry_concepts#text-focus
        /// [`has_focus_target`]: Self::has_focus_target
        #[must_use = "Check whether this widget has focus."]
        pub fn is_focus_target(&self) -> bool {
            self.global_state.focused_widget == Some(self.widget_id())
        }

        /// Whether this widget or any of its descendants are focused.
        ///
        /// To check if only this specific widget is focused use [`is_focus_target`](Self::is_focus_target).
        #[must_use = "Check whether this subtree contains focus."]
        pub const fn has_focus_target(&self) -> bool {
            self.widget_state.has_focus_target()
        }

        /// Whether the window is focused.
        #[must_use = "Check the window's focus state."]
        pub const fn is_window_focused(&self) -> bool {
            self.global_state.window_focused()
        }

        /// The widget, if any, which has [pointer capture].
        ///
        /// The pointer will usually be the mouse. In future versions, this
        /// function will take a pointer id as input to test a specific pointer.
        ///
        /// [pointer capture]: crate::doc::masonry_concepts#pointer-capture
        #[must_use = "Use the pointer capture target if present."]
        pub const fn pointer_capture_target_id(&self) -> Option<WidgetId> {
            self.global_state.pointer_capture_target()
        }

        /// The widget, if any, which has [text focus].
        ///
        /// The focused widget is the one that receives keyboard events.
        ///
        /// [text focus]: crate::doc::masonry_concepts#text-focus
        #[must_use = "Use the focused widget identifier if present."]
        pub const fn focus_target_id(&self) -> Option<WidgetId> {
            self.global_state.focused_widget
        }

        /// Whether this widget gets pointer events and hovered status.
        ///
        /// See [`Widget::accepts_pointer_interaction()`] for details.
        #[must_use = "Check whether this widget accepts pointer input."]
        pub const fn accepts_pointer_interaction(&self) -> bool {
            self.widget_state.accepts_pointer_interaction()
        }

        /// Whether children of this widget get pointer events and hovered status.
        ///
        /// See [`Widget::propagates_pointer_interaction()`] for details.
        #[must_use = "Check whether this widget permits pointer input to children."]
        pub const fn propagates_pointer_interaction(&self) -> bool {
            self.widget_state.propagates_pointer_interaction()
        }

        /// Whether this widget gets text focus.
        ///
        /// See [`Widget::accepts_focus()`] for details.
        #[must_use = "Check whether this widget accepts keyboard focus."]
        pub const fn accepts_focus(&self) -> bool {
            self.widget_state.accepts_focus()
        }

        /// Whether this widget gets IME events.
        ///
        /// See [`Widget::accepts_text_input()`] for details.
        #[must_use = "Check whether this widget accepts IME input."]
        pub const fn accepts_text_input(&self) -> bool {
            self.widget_state.accepts_text_input()
        }

        /// Whether this widget is [disabled].
        ///
        /// Returns `true` if this widget or any of its ancestors is explicitly disabled.
        /// To make this widget explicitly disabled use [`set_disabled`].
        ///
        /// [disabled]: crate::doc::masonry_concepts#disabled
        /// [`set_disabled`]: EventCtx::set_disabled
        #[must_use = "Inspect the effective disabled state."]
        pub const fn is_disabled(&self) -> bool {
            self.widget_state.is_disabled()
        }

        /// Whether this widget is [stashed].
        ///
        /// [stashed]: crate::doc::masonry_concepts#stashed
        #[must_use = "Inspect the effective stashed state."]
        pub const fn is_stashed(&self) -> bool {
            self.widget_state.is_stashed()
        }

        /// Returns whether the given child is [stashed].
        ///
        /// The result of this function is not affected by `set_stashed` until the next
        /// [`update_stashed`] pass.
        ///
        /// [stashed]: crate::doc::masonry_concepts#stashed
        /// [`update_stashed`]: crate::doc::pass_system#update-passes
        #[must_use = "Check whether the child is stashed."]
        pub fn child_is_stashed(&self, child: &WidgetPod<impl Widget + ?Sized>) -> bool {
            self.get_child_state(child)
                .is_none_or(WidgetState::is_stashed)
        }
    }
);

// --- MARK: UPDATE FLAGS
impl_context_method!(
    MutateCtx<'_>,
    ActionCtx<'_>,
    EventCtx<'_>,
    UpdateCtx<'_>,
    RawCtx<'_>,
    {
        /// Requests a [`paint`](crate::core::Widget::paint) and an
        /// [`accessibility`](crate::core::Widget::accessibility) pass.
        pub fn request_render(&mut self) {
            trace!("request_render");
            self.widget_state.set_request_pre_paint(true);
            self.widget_state.set_request_paint(true);
            self.widget_state.set_request_post_paint(true);
            self.widget_state.set_needs_paint(true);
            self.widget_state.set_needs_accessibility(true);
            self.widget_state.set_request_accessibility(true);
        }

        /// Requests a paint pass for the [`pre_paint`](crate::core::Widget::pre_paint) method.
        pub fn request_pre_paint(&mut self) {
            trace!("request_pre_paint");
            self.widget_state.set_request_pre_paint(true);
            self.widget_state.set_needs_paint(true);
        }

        /// Requests a paint pass, specifically for the [`paint`] method.
        ///
        /// Unlike [`request_render`], this does not request an [`accessibility`] pass
        /// or a call to [`pre_paint`] or [`post_paint`].
        ///
        /// Use `request_render` unless you're sure those aren't needed.
        ///
        /// [`paint`]: crate::core::Widget::paint
        /// [`request_render`]: Self::request_render
        /// [`accessibility`]: crate::core::Widget::accessibility
        /// [`pre_paint`]: crate::core::Widget::post_paint
        /// [`post_paint`]: crate::core::Widget::post_paint
        pub fn request_paint_only(&mut self) {
            trace!("request_paint_only");
            self.widget_state.set_request_paint(true);
            self.widget_state.set_needs_paint(true);
        }

        /// Requests a paint pass for the [`post_paint`](crate::core::Widget::post_paint) method.
        pub fn request_post_paint(&mut self) {
            trace!("request_post_paint");
            self.widget_state.set_request_post_paint(true);
            self.widget_state.set_needs_paint(true);
        }

        /// Requests an [`accessibility`](crate::core::Widget::accessibility) pass.
        ///
        /// This doesn't request a [`paint`](crate::core::Widget::paint) pass.
        /// If you want to request both an accessibility pass and a paint pass,
        /// use [`request_render`](Self::request_render).
        pub fn request_accessibility_update(&mut self) {
            trace!("request_accessibility_update");
            self.widget_state.set_needs_accessibility(true);
            self.widget_state.set_request_accessibility(true);
        }

        /// Requests a [`layout`] pass.
        ///
        /// Call this method if the widget has changed in a way that requires a layout pass.
        ///
        /// [`layout`]: crate::core::Widget::layout
        pub fn request_layout(&mut self) {
            trace!("request_layout");
            self.widget_state.set_request_layout(true);
            self.widget_state.set_needs_layout(true);
        }

        /// Requests that this widget's [`compose`] method be called.
        ///
        /// The compose pass is often cheaper than the layout pass,
        /// because it can only transform individual widgets' position.
        /// Use this when widget-owned state read by [`compose`] changes,
        /// such as a scroll offset applied to a child during compose.
        ///
        /// [`compose`]: crate::core::Widget::compose
        pub fn request_compose(&mut self) {
            trace!("request_compose");
            self.widget_state.set_needs_compose(true);
            self.widget_state.set_request_compose(true);
        }

        /// Requests an animation frame.
        pub fn request_anim_frame(&mut self) {
            trace!("request_anim_frame");
            self.widget_state.set_request_anim(true);
            self.widget_state.set_needs_anim(true);
        }

        /// Notifies Masonry that the cursor returned by [`Widget::get_cursor`] has changed.
        ///
        /// This is mostly meant for cases where the cursor changes even if the pointer doesn't
        /// move, because the nature of the widget has changed somehow.
        pub fn request_cursor_icon_change(&mut self) {
            trace!("request_cursor_icon_change");
            self.global_state.set_needs_pointer_pass(true);
        }

        /// Indicates that your children have changed.
        ///
        /// Widgets must call this method after adding a new child.
        ///
        /// This method will also call [`request_layout`](Self::request_layout).
        pub fn children_changed(&mut self) {
            trace!("children_changed");
            self.widget_state.set_children_changed(true);
            self.widget_state.set_needs_update_focusable(true);
            self.request_layout();
        }

        /// Indicates that a child is about to be removed from the tree.
        ///
        /// Container widgets should avoid dropping `WidgetPod`s. Instead, they should
        /// pass them to this method.
        ///
        /// This method will also call [`children_changed`](Self::children_changed).
        pub fn remove_child(&mut self, child: WidgetPod<impl Widget + ?Sized>) {
            fn remove_node(
                global_state: &mut RenderRootState,
                parent_id: WidgetId,
                node: ArenaRef<'_, WidgetArenaNode>,
            ) {
                let state = &node.item.state;

                // Visit the ownership tree, not the container's declaration.
                for id in node.child_ids() {
                    if let Some(child) = node.children.item(id) {
                        remove_node(global_state, state.id, child);
                    }
                }

                if let Some(tag) = node.item.tag {
                    global_state.widget_tags.remove(&tag);
                }

                // If we remove the focus anchor, its parent becomes the anchor.
                if global_state.focus_anchor == Some(state.id) {
                    global_state.focus_anchor = Some(parent_id);
                }

                global_state.scene_cache.remove(&state.id);

                if let Some(layers) = global_state.attached_layers.remove(&state.id) {
                    for (_, layer_id) in layers {
                        global_state.emit_signal(RenderRootSignal::RemoveLayer(layer_id));
                    }
                }
            }

            let id = child.id();
            // A newly created pod may be removed before the registration pass.
            if let Some(node) = self.children.item(id) {
                remove_node(self.global_state, self.widget_state.id, node);
                let _ = self.children.remove(id);
            }

            self.children_changed();
            drop(child);
        }

        /// Sets the disabled state for this widget.
        ///
        /// Setting this to `false` does not mean a widget is not still disabled; for instance it may
        /// still be disabled by an ancestor. See [`is_disabled`] for more information.
        ///
        /// [`is_disabled`]: EventCtx::is_disabled
        pub const fn set_disabled(&mut self, disabled: bool) {
            if self.widget_state.is_explicitly_disabled() != disabled {
                self.widget_state.set_needs_update_disabled(true);
                self.widget_state.set_is_explicitly_disabled(disabled);
            }
        }

        /// Sets the local transform for this widget.
        ///
        /// This maps this widget's border-box coordinate space
        /// to the parent's border-box coordinate space.
        ///
        /// It behaves similarly as CSS transforms.
        pub const fn set_transform(&mut self, transform: Affine) {
            self.widget_state.transform = transform;
            self.widget_state.set_transform_changed(true);
            self.widget_state.set_needs_compose(true);
        }

        /// Adds a string to this widget's [class set].
        ///
        /// Changes will be applied in the next update pass and may affect property resolution.
        ///
        /// [class]: crate::doc::masonry_concepts#classes
        pub fn add_class(&mut self, class: &str) {
            self.widget_state.class_diff.add(class);
            self.widget_state.set_request_update_props(true);
            self.widget_state.set_needs_update_props(true);
        }

        /// Removes a string from this widget's [class set].
        ///
        /// Changes will be applied in the next update pass and may affect property resolution.
        ///
        /// [class]: crate::doc::masonry_concepts#classes
        pub fn remove_class(&mut self, class: &str) {
            self.widget_state.class_diff.remove(class);
            self.widget_state.set_request_update_props(true);
            self.widget_state.set_needs_update_props(true);
        }
    }
);

// --- MARK: OTHER METHODS
// Methods on mutable context types
impl_context_method!(
    MutateCtx<'_>,
    ActionCtx<'_>,
    EventCtx<'_>,
    UpdateCtx<'_>,
    MeasureCtx<'_>,
    LayoutCtx<'_>,
    ComposeCtx<'_>,
    RawCtx<'_>,
    {
        // TODO - Remove from MeasureCtx/LayoutCtx/ComposeCtx
        /// Marks child widget as stashed.
        ///
        /// If `stashed` is true, the child will not be painted or listed in the accessibility tree.
        ///
        /// This will *not* trigger a layout pass.
        pub fn set_stashed(&mut self, child: &WidgetPod<impl Widget + ?Sized>, stashed: bool) {
            let Some(child_state) = self.get_child_state_mut(child) else {
                return;
            };
            // Stashing is generally a property derived from the parent widget's state
            // (rather than set imperatively), so it is likely to be set as part of passes.
            // Therefore, we avoid re-running the update_stashed_pass in most cases.
            if child_state.is_explicitly_stashed() != stashed {
                child_state.set_is_explicitly_stashed(stashed);
                child_state.set_needs_update_stashed(true);
                self.widget_state.set_needs_update_stashed(true);
            }
        }

        /// Queues a typed callback for this widget.
        ///
        /// Callbacks run in submission order during the mutate pass and are skipped
        /// if the target is absent or does not have the requested type.
        pub fn mutate_self_later<W: Widget + FromDynWidget + ?Sized>(
            &mut self,
            f: impl FnOnce(WidgetMut<'_, W>) + Send + 'static,
        ) {
            self.mutate_later(self.widget_id(), f);
        }

        /// Queues a typed callback for the given child widget.
        ///
        /// The same ordering and target validation as [`Self::mutate_later`] apply.
        pub fn mutate_child_later<W: Widget + FromDynWidget + ?Sized>(
            &mut self,
            child: &WidgetPod<W>,
            f: impl FnOnce(WidgetMut<'_, W>) + Send + 'static,
        ) {
            self.mutate_later(child.id(), f);
        }

        /// Queues a typed callback for the widget with the given id.
        ///
        /// Callbacks run in submission order during the mutate pass and are skipped
        /// if the target is absent or does not have the requested type.
        pub fn mutate_later<W: Widget + FromDynWidget + ?Sized>(
            &mut self,
            target: WidgetId,
            f: impl FnOnce(WidgetMut<'_, W>) + Send + 'static,
        ) {
            self.global_state.mutate_callbacks.push(MutateCallback {
                id: target,
                callback: Box::new(|mut widget| {
                    if let Some(widget) = widget.try_downcast() {
                        f(widget);
                    }
                }),
            });
        }

        /// Returns a shared typed reference to a registered child, if present.
        /// This does not borrow the child's mutable state or propagate pass flags.
        #[must_use = "Inspect the child if it exists."]
        pub fn get_child<Child: Widget + FromDynWidget + ?Sized>(
            &self,
            child: &WidgetPod<Child>,
        ) -> Option<&Child> {
            Child::from_dyn(&*self.children.item(child.id())?.item.widget)
        }

        /// Returns direct mutable reference to the stored child, and a context handle for that child.
        ///
        /// This context lets you set pass flags for the child widget, which may be tricky.
        /// In general, you should avoid setting the flags of a pass that runs before the
        /// pass you're currently in.
        /// Not doing so might lead to performance cliffs and, hypothetically, panics.
        ///
        /// This method is an escape hatch for cases where a parent widget completely
        /// controls their child, but needs it to be a separate widget for user interaction to
        /// behave as expected.
        /// As such, the child widget must opt-in using the `AllowRawMut` trait.
        ///
        /// See [pass documentation](crate::doc::pass_system) for the pass order.
        pub fn get_raw_mut<Child: Widget + FromDynWidget + AllowRawMut + ?Sized>(
            &mut self,
            child: &WidgetPod<Child>,
        ) -> Option<(&mut Child, RawCtx<'_>)> {
            let node = self.children.item_mut(child.id())?;
            let widget = Child::from_dyn_mut(&mut *node.item.widget)?;
            let ctx = RawCtx {
                global_state: self.global_state,
                parent_widget_state: self.widget_state,
                widget_state: &mut node.item.state,
                children: node.children,
                property_arena: self.property_arena,
            };
            Some((widget, ctx))
        }

        /// Submits an action, which indicates that this widget requires something be handled
        /// by the application, such as user input.
        ///
        /// The `Action` type parameter should always be the `Self::Action` associated type
        /// of the widget you're calling this method from.
        /// Masonry will validate this, and this method may panic if this isn't the case.
        ///
        /// For further details see [`ErasedAction`].
        pub fn submit_action<Action: AnyDebug + Send>(&mut self, action: impl Into<Action>) {
            trace!("submit_action");
            let action = action.into();
            if action.type_id() != self.widget_state.action_type {
                #[cfg(debug_assertions)]
                let expected_type = self.widget_state.action_type_name;
                #[cfg(not(debug_assertions))]
                let expected_type = "<Self as Widget>::Action";

                debug_panic!(
                    "Trying to emit action of incorrect type `{}`. Expected type is `{}`.",
                    action.type_name(),
                    expected_type,
                );
                return;
            }
            self.global_state
                .actions
                .push((Box::new(action), self.widget_state.id));
        }

        /// Submits a type-erased action.
        ///
        /// Unlike [`Self::submit_action`], this method lets you submit an action with an
        /// arbitrary type, which may not match `Self::Action`.
        /// This may act as an escape hatch in some situations.
        ///
        /// For further details see [`ErasedAction`].
        pub fn submit_untyped_action(&mut self, action: ErasedAction) {
            trace!("submit_untyped_action");
            self.global_state
                .actions
                .push((action, self.widget_state.id));
        }

        /// Sets the IME cursor area in the widget's content-box coordinate space.
        ///
        /// When this widget is [focused] and [accepts text input], the reported IME area is sent
        /// to the platform. The area can be used by the platform to, for example, place a
        /// candidate box near that area, while ensuring the area is not obscured.
        ///
        /// If no IME area is set, then Masonry will use the widget's aligned border-box rect.
        ///
        /// [focused]: EventCtx::request_focus
        /// [accepts text input]: Widget::accepts_text_input
        pub fn set_ime_area(&mut self, ime_area: Rect) {
            let translation = self.widget_state.border_box_translation();
            self.widget_state.ime_area = Some(ime_area + translation);
        }

        /// Removes the IME cursor area.
        ///
        /// See [`set_ime_area`] for more details.
        ///
        /// [`set_ime_area`]: Self::set_ime_area
        pub const fn clear_ime_area(&mut self) {
            self.widget_state.ime_area = None;
        }

        /// Sets the contents of the platform clipboard.
        ///
        /// For example, text widgets should call this for "cut" and "copy" user interactions.
        /// Note that we currently don't support the "Primary" selection buffer on X11/Wayland.
        pub fn set_clipboard(&mut self, contents: String) {
            trace!("set_clipboard");
            self.global_state
                .emit_signal(RenderRootSignal::ClipboardStore(contents));
        }

        /// Starts a window drag.
        ///
        /// Moves the window with the left mouse button until the button is released.
        pub fn drag_window(&mut self) {
            trace!("drag_window");
            self.global_state.emit_signal(RenderRootSignal::DragWindow);
        }

        /// Starts a window resize.
        ///
        /// Resizes the window with the left mouse button until the button is released.
        pub fn drag_resize_window(&mut self, direction: ResizeDirection) {
            trace!("drag_resize_window");
            self.global_state
                .emit_signal(RenderRootSignal::DragResizeWindow(direction));
        }

        /// Toggles the maximized state of the window.
        pub fn toggle_maximized(&mut self) {
            trace!("toggle_maximized");
            self.global_state
                .emit_signal(RenderRootSignal::ToggleMaximized);
        }

        /// Minimizes the window.
        pub fn minimize(&mut self) {
            trace!("minimize");
            self.global_state.emit_signal(RenderRootSignal::Minimize);
        }

        /// Exits the application.
        pub fn exit(&mut self) {
            trace!("exit");
            self.global_state.emit_signal(RenderRootSignal::Exit);
        }

        /// Shows the window menu at a specified position.
        pub fn show_window_menu(&mut self, position: LogicalPosition<f64>) {
            trace!("show_window_menu");
            self.global_state
                .emit_signal(RenderRootSignal::ShowWindowMenu(position));
        }

        /// Creates a new [layer] at a specified `position`.
        ///
        /// The placement is expressed in window coordinates.
        ///
        /// # Panics
        ///
        /// If [`W::as_layer()`](Widget::as_layer) returns `None`.
        ///
        /// [layer]: crate::doc::masonry_concepts#layers
        pub fn create_layer<W: Widget + ?Sized>(
            &mut self,
            layer_type: LayerType,
            mut fallback_widget: NewWidget<W>,
            placement: LayerPlacement,
        ) {
            trace!("create_layer");

            if fallback_widget.widget.as_layer().is_none() {
                debug_panic!(
                    "cannot create layer of type {} - `Widget::as_layer()` returned None",
                    fallback_widget.widget.short_type_name()
                );
                return;
            }

            self.global_state.emit_signal(RenderRootSignal::NewLayer(
                layer_type,
                fallback_widget.erased(),
                placement,
            ));
        }

        /// Creates a new [layer] at a specified `position`, and ties it to the current widget.
        ///
        /// The layer will be removed when the current widget is removed from the tree.
        /// A widget can only have one layer of any given type `W` at a time:
        /// if a widget creates two layers of the same type, the old layer will be removed.
        ///
        /// The placement is expressed in window coordinates.
        ///
        /// # Panics
        ///
        /// If [`W::as_layer()`](Widget::as_layer) returns `None`.
        ///
        /// [layer]: crate::doc::masonry_concepts#layers
        pub fn create_attached_layer<W: Widget + ?Sized>(
            &mut self,
            layer_type: LayerType,
            mut fallback_widget: NewWidget<W>,
            placement: LayerPlacement,
        ) {
            trace!("create_attached_layer");

            if fallback_widget.widget.as_layer().is_none() {
                debug_panic!(
                    "cannot create layer of type {} - `Widget::as_layer()` returned None",
                    fallback_widget.widget.short_type_name()
                );
                return;
            }

            // FIXME - Add LayerId type and use that instead.
            let layer_id = fallback_widget.id;

            let layers = self
                .global_state
                .attached_layers
                .entry(self.widget_id())
                .or_default();
            if let Some(prev) = layers.insert(TypeId::of::<W>(), layer_id) {
                self.global_state
                    .emit_signal(RenderRootSignal::RemoveLayer(prev));
            }
            self.widget_state.set_request_accessibility(true);
            self.widget_state.set_needs_accessibility(true);
            self.global_state.emit_signal(RenderRootSignal::NewLayer(
                layer_type,
                fallback_widget.erased(),
                placement,
            ));
        }

        /// Removes the layer with the specified widget as root.
        pub fn remove_layer(&mut self, root_widget_id: WidgetId) {
            trace!("remove_layer");
            self.global_state
                .emit_signal(RenderRootSignal::RemoveLayer(root_widget_id));

            let Some(layers) = self.global_state.attached_layers.get_mut(&self.widget_id()) else {
                return;
            };

            let layer_type_id = layers
                .iter()
                .find(|(_, id)| **id == root_widget_id)
                .map(|(type_id, _)| *type_id);
            if let Some(type_id) = layer_type_id {
                layers.remove(&type_id);
                self.widget_state.set_request_accessibility(true);
                self.widget_state.set_needs_accessibility(true);
            }
            if layers.is_empty() {
                self.global_state.attached_layers.remove(&self.widget_id());
            }
        }
    }
);

impl_context_method!(
    MutateCtx<'_>,
    EventCtx<'_>,
    UpdateCtx<'_>,
    ComposeCtx<'_>,
    {
        /// Updates the placement policy of an existing layer.
        pub fn reposition_layer(&mut self, root_widget_id: WidgetId, placement: LayerPlacement) {
            self.global_state
                .emit_signal(RenderRootSignal::RepositionLayer(root_widget_id, placement));
        }
    }
);

impl UpdateCtx<'_> {
    /// Returns the nearest ancestor of this widget that is of type `W`, along with its `WidgetId`.
    ///
    /// This should only be called during [`WidgetAdded`](crate::core::Update::WidgetAdded).
    /// Calling it in other contexts will always return `None`.
    // FIXME: We should move this method out of `UpdateCtx` to avoid having this disclaimer.
    #[must_use = "Use the matching ancestor if present."]
    pub fn nearest_ancestor<W: Widget>(&self) -> Option<(&W, WidgetId)> {
        let mut list = self.ancestors;
        while let Some(node) = list {
            if let Some(widget) = W::from_dyn(node.widget) {
                return Some((widget, node.id));
            }
            list = node.parent;
        }
        None
    }
}

impl RegisterCtx<'_> {
    /// Registers a child widget.
    ///
    /// Container widgets should call this on all their children in
    /// their implementation of [`Widget::register_children`].
    pub fn register_child(&mut self, child: &mut WidgetPod<impl Widget + ?Sized>) {
        let Some(NewWidget {
            widget,
            id,
            options,
            properties,
            property_stack_id,
            classes,
            tag,
            action_type,
            #[cfg(debug_assertions)]
            action_type_name,
        }) = child.take_inner()
        else {
            return;
        };

        #[cfg(debug_assertions)]
        {
            self.registered_ids.push(id);
        }

        let state = WidgetState::new(
            id,
            widget.short_type_name(),
            options,
            action_type,
            property_stack_id,
            #[cfg(debug_assertions)]
            action_type_name,
        );

        // Keep the widget registered even if a duplicate tag is supplied in
        // a release build. Only the original owner retains the tag.
        let tag = tag.filter(|tag| match self.global_state.widget_tags.entry(*tag) {
            Entry::Vacant(entry) => {
                entry.insert(id);
                true
            }
            Entry::Occupied(_) => {
                debug_panic!("Tag '{tag}' already exists in the widget tree");
                false
            }
        });

        let node = WidgetArenaNode {
            widget: widget.as_box_dyn(),
            state,
            properties,
            tag,
            class_set: ClassSet {
                classes,
                ..ClassSet::default()
            },
        };
        self.children.insert(id, node);
    }
}

impl Drop for RawCtx<'_> {
    fn drop(&mut self) {
        self.parent_widget_state.merge_up(self.widget_state);
    }
}

// --- MARK: DEBUG PAINT
impl PaintCtx<'_> {
    /// Controls how this widget subtree is recorded in the current paint pass.
    ///
    /// Each paint phase caches its most recent declaration until it is run again.
    /// If multiple phases declare a mode, the last in painter order wins.
    /// Without a declaration the subtree is inline.
    pub const fn set_paint_layer_mode(&mut self, mode: PaintLayerMode) {
        self.layer_mode = Some(mode);
    }

    /// Whether debug paint is enabled.
    ///
    /// If this property is set, your widget may draw additional debug information
    /// (such as the position of the text baseline).
    /// These should normally use the [debug color][Self::debug_color] for this widget.
    /// Please note that when debug painting is enabled, each widget's layout boundaries are
    /// outlined by Masonry, so you should avoid duplicating that.
    ///
    /// Debug paint can be enabled by setting the environment variable `MASONRY_DEBUG_PAINT`.
    #[must_use = "Check whether debug painting is enabled."]
    pub const fn debug_paint_enabled(&self) -> bool {
        self.global_state.debug_paint()
    }

    /// A color used for debug painting in this widget.
    ///
    /// This is normally used to paint additional debugging information
    /// when debug paint is enabled, see [`Self::debug_paint_enabled`].
    #[must_use = "Use the assigned debug color."]
    pub fn debug_color(&self) -> Color {
        get_debug_color(self.widget_id().to_raw())
    }
}
