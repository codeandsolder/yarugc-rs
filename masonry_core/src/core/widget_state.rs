// Copyright 2018 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;

use kurbo::{Affine, Insets, Point, Rect, Size, Vec2};
use tracing::Span;

use crate::core::{
    ClassSetDiff, PaintLayerMode, PropertyCache, PropertyStackId, WidgetId, WidgetOptions,
};
use crate::layout::MeasurementCache;

// TODO - Reduce WidgetState size.
// See https://github.com/linebender/xilem/issues/706

/// Generic state for all widgets in the hierarchy.
///
/// This struct contains the metadata that passes need to know about widgets and that
/// widgets don't store themselves.
///
/// All context types include a reference to a `WidgetState`, so widgets can query
/// information about their own state and set invalidation flags.
/// Context types for render passes have a shared `&WidgetState`. Context types for
/// other passes have a `&mut WidgetState`.
///
/// In general, the only way to get a `&mut WidgetState` should be in a pass or as part of a
/// [`WidgetMut`]. Both should reborrow the parent widget state and call
/// [`WidgetState::merge_up`] at the end so that invalidations are always bubbled up.
///
/// # Naming scheme
///
/// Some fields follow a naming scheme:
/// - `request_xxx`: this specific widget has requested the xxx pass to run on it.
/// - `needs_xxx`: this widget or a descendant has requested the xxx pass to run on it.
/// - `is_xxx`: this widget is in the xxx state.
/// - `has_xxx`: this widget or a descendant is in the xxx state.
///
/// # Resetting flags
///
/// Generally, the `needs_foobar` and `request_foobar` flags will be reset to
/// false during the "foobar" pass after calling the "foobar" method.
///
/// In principle this shouldn't be a problem because most passes shouldn't be
/// able to request themselves. The exception is the anim pass: an anim frame can
/// (and usually will) request a new anim frame.
///
/// # Zombie flags
///
/// Masonry's passes should be designed to avoid what we'll call "zombie flags".
///
/// Zombie flags are when a pass "foobar" fails to clear `needs_foobar` flags of some
/// widgets by the time it's complete. Even if the pass works correctly, failing to
/// clear the flags means they'll be propagated up in [`WidgetState::merge_up`] by every
/// other pass, which means the widget tree will keep requesting the same passes over
/// and over. Zombie flags are terrible for performance and power-efficiency.
///
/// For example, in previous versions, stashing widgets could sometimes produce zombie
/// flags, because passes such as paint or layout would not run on stashed widgets, but
/// the `needs_paint` and `needs_layout` flags would still propagate up to the parent.
///
/// To avoid zombie flags, all passes should *always* recurse over all children and *never*
/// exit before recursing. The only short-circuit should be the `if !needs_foobar { return }`
/// block at the beginning of the pass. Flags can be used to skip work in the middle of the
/// pass function, but never to skip the `recurse_on_children` call.
///
/// (The exception is the layout pass, which can't recurse on stashed children.)
///
/// [`WidgetMut`]: crate::core::WidgetMut
#[derive(Clone, Debug)]
pub(crate) struct WidgetState {
    pub(crate) id: WidgetId,
    flags: u64,

    // --- LAYOUT ---
    /// The origin (top-left) of the widget's aligned border-box
    /// in the parent's border-box coordinate space.
    ///
    /// Together with `end_point`, these constitute the widget's aligned border-box.
    pub(crate) origin: Point,
    /// The bottom right of the widget's aligned border-box
    /// in the parent's border-box coordinate space.
    ///
    /// Computed from the widget's `origin` and `layout_border_box_size`, with some pixel snapping.
    pub(crate) end_point: Point,
    /// The widget's layout border-box size.
    ///
    /// This is the chosen border-box size with min/max constraints applied.
    ///
    /// It is used to:
    /// * Determine layout cache validity.
    /// * Derive the widget's layout content-box size that will be given to `Widget::layout`.
    /// * Compute `end_point` when the widget is placed to an `origin` by its parent.
    pub(crate) layout_border_box_size: Size,
    /// The insets for converting between content-box and border-box rects.
    ///
    /// Add these insets to the content-box to get the border-box,
    /// and subtract these insets from the border-box to get the content-box.
    ///
    /// These insets are derived from the widget's border and padding properties.
    pub(crate) border_box_insets: Insets,
    /// The insets for converting between border-box and paint-box rects.
    ///
    /// Add these insets to the border-box to get the paint-box,
    /// and subtract these insets from the paint-box to get the border-box.
    ///
    /// In general, these will be zero; the exception is for things like
    /// drop shadows or overflowing text.
    pub(crate) paint_box_insets: Insets,
    /// An axis aligned bounding rect (AABB in 2D),
    /// containing itself and all its descendents in the window's coordinate space.
    ///
    /// This is the union of clipped effective paint-box rects, i.e. the union of
    /// globally transformed aligned border-box rects with paint insets applied.
    pub(crate) bounding_box: Rect,

    /// The offset of the first baseline relative to the top of the widget's layout border-box.
    ///
    /// In general, this will be `f64::NAN`; the bottom of the widget will be considered
    /// the baseline. Widgets that contain text or controls that expect to be
    /// laid out alongside text can set this as appropriate.
    pub(crate) first_baseline: f64,
    /// The offset of the last baseline relative to the top of the widget's layout border-box.
    ///
    /// In general, this will be `f64::NAN`; the bottom of the widget will be considered
    /// the baseline. Widgets that contain text or controls that expect to be
    /// laid out alongside text can set this as appropriate.
    pub(crate) last_baseline: f64,

    // TODO - Use general Shape
    // Currently Kurbo doesn't really provide a type that lets us
    // efficiently hold an arbitrary shape.
    /// The widget's clip path in the widget's border-box coordinate space.
    ///
    /// This clips the painting of `Widget::paint` and all the painting of children.
    /// It does not clip this widget's `Widget::pre_paint` nor `Widget::post_paint`.
    pub(crate) clip_path: Option<Rect>,

    /// Local transform used during the mapping of this widget's border-box coordinate space
    /// to the parent's border-box coordinate space.
    ///
    /// This transform is applied before `scroll_translation` and `origin`.
    pub(crate) transform: Affine,
    /// Global transform mapping this widget's border-box coordinate space
    /// to the window's coordinate space.
    ///
    /// Computed from all `transform`, `scroll_translation`, and `origin` values
    /// from this widget all the way up to the window.
    ///
    /// Multiply by this to convert from this widget's border-box coordinate space to the window's,
    /// or use the inverse of this transform to go from window's space to this widget's border-box.
    pub(crate) window_transform: Affine,
    /// Translation applied by scrolling, applied after applying `transform` to this widget.
    pub(crate) scroll_translation: Vec2,

    // --- INTERACTIONS ---
    /// The `TypeId` of the widget's `Widget::Action` type.
    pub(crate) action_type: TypeId,

    /// The area of the widget that is being edited by an IME,
    /// in the widget's border-box coordinate space.
    pub(crate) ime_area: Option<Rect>,

    // --- PASSES ---
    /// Cached measurement results.
    pub(crate) measurement_cache: MeasurementCache,

    /// Layer declarations cached alongside pre-paint, paint, and post-paint.
    pub(crate) paint_layer_modes: [Option<PaintLayerMode>; 3],

    // --- STATUS ---

    // --- PROPERTIES ---
    /// The `PropertyStack` assigned to this widget, if any.
    pub(crate) property_stack_id: Option<PropertyStackId>,
    /// Pending class changes to apply during `run_update_props_pass`.
    pub(crate) class_diff: ClassSetDiff,
    /// Cached property stack resolutions for this widget.
    pub(crate) property_cache: PropertyCache,

    // --- DEBUG INFO ---
    /// The typename of the associated widget.
    ///
    /// Used in some guard rails to provide richer error messages when a parent forgets
    /// to iterate over some children.
    pub(crate) trace_span: Span,
    // TODO - Encapsulate this in WidgetStateDebugInfo struct.
    #[cfg(debug_assertions)]
    pub(crate) widget_name: &'static str,
    #[cfg(debug_assertions)]
    pub(crate) action_type_name: &'static str,
}

/// Boolean widget state stored in a single bitset. Only the propagated
/// invalidation flags merge into ancestor state; local requests and interaction
/// state must remain local to the widget.
#[derive(Clone, Copy, Debug)]
#[repr(u64)]
enum WidgetFlag {
    TransformChanged = 1 << 0,
    AcceptsPointerInteraction = 1 << 1,
    PropagatesPointerInteraction = 1 << 2,
    AcceptsFocus = 1 << 3,
    AcceptsTextInput = 1 << 4,
    IsNew = 1 << 5,
    IsExpectingPlaceChildCall = 1 << 6,
    RequestLayout = 1 << 7,
    NeedsLayout = 1 << 8,
    RequestCompose = 1 << 9,
    NeedsCompose = 1 << 10,
    RequestPrePaint = 1 << 11,
    RequestPaint = 1 << 12,
    RequestPostPaint = 1 << 13,
    NeedsPaint = 1 << 14,
    RequestAccessibility = 1 << 15,
    NeedsAccessibility = 1 << 16,
    RequestAnim = 1 << 17,
    NeedsAnim = 1 << 18,
    NeedsUpdateDisabled = 1 << 19,
    NeedsUpdateStashed = 1 << 20,
    DescendantIsFocusable = 1 << 21,
    NeedsUpdateFocusable = 1 << 22,
    RequestUpdateProps = 1 << 23,
    NeedsUpdateProps = 1 << 24,
    ChildrenChanged = 1 << 25,
    IsExplicitlyDisabled = 1 << 26,
    IsDisabled = 1 << 27,
    IsExplicitlyStashed = 1 << 28,
    IsStashed = 1 << 29,
    HasHovered = 1 << 30,
    IsHovered = 1 << 31,
    HasActive = 1 << 32,
    IsActive = 1 << 33,
    HasFocusTarget = 1 << 34,
}

const PROPAGATED_FLAGS: u64 = WidgetFlag::NeedsLayout as u64
    | WidgetFlag::NeedsCompose as u64
    | WidgetFlag::NeedsPaint as u64
    | WidgetFlag::NeedsAccessibility as u64
    | WidgetFlag::NeedsAnim as u64
    | WidgetFlag::NeedsUpdateDisabled as u64
    | WidgetFlag::NeedsUpdateStashed as u64
    | WidgetFlag::NeedsUpdateFocusable as u64
    | WidgetFlag::NeedsUpdateProps as u64
    | WidgetFlag::ChildrenChanged as u64;
const REWRITE_FLAGS: u64 = WidgetFlag::NeedsLayout as u64
    | WidgetFlag::NeedsCompose as u64
    | WidgetFlag::NeedsUpdateDisabled as u64
    | WidgetFlag::NeedsUpdateStashed as u64
    | WidgetFlag::NeedsUpdateFocusable as u64
    | WidgetFlag::NeedsUpdateProps as u64
    | WidgetFlag::ChildrenChanged as u64;
const INITIAL_FLAGS: u64 = WidgetFlag::AcceptsPointerInteraction as u64
    | WidgetFlag::PropagatesPointerInteraction as u64
    | WidgetFlag::IsNew as u64
    | WidgetFlag::RequestLayout as u64
    | WidgetFlag::NeedsLayout as u64
    | WidgetFlag::RequestCompose as u64
    | WidgetFlag::NeedsCompose as u64
    | WidgetFlag::RequestPrePaint as u64
    | WidgetFlag::RequestPaint as u64
    | WidgetFlag::RequestPostPaint as u64
    | WidgetFlag::NeedsPaint as u64
    | WidgetFlag::RequestAccessibility as u64
    | WidgetFlag::NeedsAccessibility as u64
    | WidgetFlag::RequestAnim as u64
    | WidgetFlag::NeedsAnim as u64
    | WidgetFlag::NeedsUpdateDisabled as u64
    | WidgetFlag::NeedsUpdateStashed as u64
    | WidgetFlag::NeedsUpdateFocusable as u64
    | WidgetFlag::ChildrenChanged as u64;

macro_rules! widget_flag_accessors {
    ($(#[$meta:meta])* $get:ident, $set:ident, $flag:ident) => {
        $(#[$meta])*
        pub(crate) const fn $get(&self) -> bool {
            self.flags & WidgetFlag::$flag as u64 != 0
        }

        pub(crate) const fn $set(&mut self, value: bool) {
            self.flags = (self.flags & !(WidgetFlag::$flag as u64))
                | ((value as u64) * WidgetFlag::$flag as u64);
        }
    };
}

impl WidgetState {
    widget_flag_accessors!(
        /// The effective compose transform has changed because `transform`, `scroll_translation`, or
        /// `origin` has changed.
        transform_changed,
        set_transform_changed,
        TransformChanged
    );
    widget_flag_accessors!(
        /// Tracks whether widget gets pointer events.
        /// Should be immutable after `WidgetAdded` event.
        accepts_pointer_interaction,
        set_accepts_pointer_interaction,
        AcceptsPointerInteraction
    );
    widget_flag_accessors!(
        /// Tracks whether children of this widget get pointer events.
        /// Should be immutable after `WidgetAdded` event.
        propagates_pointer_interaction,
        set_propagates_pointer_interaction,
        PropagatesPointerInteraction
    );
    widget_flag_accessors!(
        /// Tracks whether widget gets text focus.
        /// Should be immutable after `WidgetAdded` event.
        accepts_focus,
        set_accepts_focus,
        AcceptsFocus
    );
    widget_flag_accessors!(
        /// Tracks whether widget is eligible for IME events.
        /// Should be immutable after `WidgetAdded` event.
        accepts_text_input,
        set_accepts_text_input,
        AcceptsTextInput
    );
    widget_flag_accessors!(
        /// `WidgetAdded` hasn't been sent to this widget yet.
        is_new,
        set_is_new,
        IsNew
    );
    widget_flag_accessors!(
        /// A flag used to track and debug missing calls to `place_child`.
        is_expecting_place_child_call,
        set_is_expecting_place_child_call,
        IsExpectingPlaceChildCall
    );
    widget_flag_accessors!(
        /// This widget explicitly requested layout
        request_layout,
        set_request_layout,
        RequestLayout
    );
    widget_flag_accessors!(
        /// The `compose` method must be called on this widget
        request_compose,
        set_request_compose,
        RequestCompose
    );
    widget_flag_accessors!(
        /// The `compose` pass must run on this widget.
        needs_compose,
        set_needs_compose,
        NeedsCompose
    );
    widget_flag_accessors!(
        /// The `pre_paint` method must be called on this widget
        request_pre_paint,
        set_request_pre_paint,
        RequestPrePaint
    );
    widget_flag_accessors!(
        /// The `paint` method must be called on this widget
        request_paint,
        set_request_paint,
        RequestPaint
    );
    widget_flag_accessors!(
        /// The `post_paint` method must be called on this widget
        request_post_paint,
        set_request_post_paint,
        RequestPostPaint
    );
    widget_flag_accessors!(
        /// A painting method must be called on this widget or a descendant
        needs_paint,
        set_needs_paint,
        NeedsPaint
    );
    widget_flag_accessors!(
        /// The `accessibility` method must be called on this widget
        request_accessibility,
        set_request_accessibility,
        RequestAccessibility
    );
    widget_flag_accessors!(
        /// The `accessibility` method must be called on this widget or a descendant
        needs_accessibility,
        set_needs_accessibility,
        NeedsAccessibility
    );
    widget_flag_accessors!(
        /// An animation must run on this widget
        request_anim,
        set_request_anim,
        RequestAnim
    );
    widget_flag_accessors!(
        /// An animation must run on this widget or a descendant
        needs_anim,
        set_needs_anim,
        NeedsAnim
    );
    widget_flag_accessors!(
        /// This widget or a descendant changed its `is_explicitly_disabled` value
        needs_update_disabled,
        set_needs_update_disabled,
        NeedsUpdateDisabled
    );
    widget_flag_accessors!(
        /// This widget or a descendant changed its `is_explicitly_stashed` value
        needs_update_stashed,
        set_needs_update_stashed,
        NeedsUpdateStashed
    );
    widget_flag_accessors!(
        /// This widget or a descendant has `accepts_focus == true`
        descendant_is_focusable,
        set_descendant_is_focusable,
        DescendantIsFocusable
    );
    widget_flag_accessors!(
        /// A focusable widget was added, removed, stashed, disabled, etc.
        needs_update_focusable,
        set_needs_update_focusable,
        NeedsUpdateFocusable
    );
    widget_flag_accessors!(
        /// This widget has pending property changes.
        request_update_props,
        set_request_update_props,
        RequestUpdateProps
    );
    widget_flag_accessors!(
        /// This widget or a descendant has pending property changes.
        needs_update_props,
        set_needs_update_props,
        NeedsUpdateProps
    );
    widget_flag_accessors!(children_changed, set_children_changed, ChildrenChanged);
    widget_flag_accessors!(
        /// This widget has been disabled.
        is_explicitly_disabled,
        set_is_explicitly_disabled,
        IsExplicitlyDisabled
    );
    widget_flag_accessors!(
        /// This widget or an ancestor has been disabled.
        is_disabled,
        set_is_disabled,
        IsDisabled
    );
    widget_flag_accessors!(
        /// This widget has been stashed.
        is_explicitly_stashed,
        set_is_explicitly_stashed,
        IsExplicitlyStashed
    );
    widget_flag_accessors!(
        /// This widget or an ancestor has been stashed.
        is_stashed,
        set_is_stashed,
        IsStashed
    );
    widget_flag_accessors!(
        /// In the hovered path, starting from window and ending at the hovered widget.
        /// Descendants of the hovered widget are not in the hovered path.
        has_hovered,
        set_has_hovered,
        HasHovered
    );
    widget_flag_accessors!(
        /// This specific widget is hovered.
        is_hovered,
        set_is_hovered,
        IsHovered
    );
    widget_flag_accessors!(
        /// In the active path, starting from window and ending at the active widget.
        /// Descendants of the active widget are not in the active path.
        has_active,
        set_has_active,
        HasActive
    );
    widget_flag_accessors!(
        /// This specific widget is active.
        is_active,
        set_is_active,
        IsActive
    );
    widget_flag_accessors!(
        /// In the focused path, starting from window and ending at the focused widget.
        /// Descendants of the focused widget are not in the focused path.
        has_focus_target,
        set_has_focus_target,
        HasFocusTarget
    );

    pub(crate) fn new(
        id: WidgetId,
        widget_name: &'static str,
        options: WidgetOptions,
        action_type: TypeId,
        property_stack_id: Option<PropertyStackId>,
        #[cfg(debug_assertions)] action_type_name: &'static str,
    ) -> Self {
        Self {
            id,
            flags: INITIAL_FLAGS
                | ((options.disabled as u64) * WidgetFlag::IsExplicitlyDisabled as u64),

            origin: Point::ORIGIN,
            end_point: Point::ORIGIN,
            layout_border_box_size: Size::ZERO,
            border_box_insets: Insets::ZERO,
            paint_box_insets: Insets::ZERO,
            bounding_box: Rect::ZERO,
            first_baseline: f64::NAN,
            last_baseline: f64::NAN,
            clip_path: Option::default(),
            transform: options.transform,
            window_transform: Affine::IDENTITY,
            scroll_translation: Vec2::ZERO,

            action_type,
            ime_area: None,

            measurement_cache: MeasurementCache::new(),
            paint_layer_modes: [None; 3],

            property_stack_id,
            class_diff: ClassSetDiff::default(),
            property_cache: PropertyCache::default(),

            trace_span: Span::none(),
            #[cfg(debug_assertions)]
            widget_name,
            #[cfg(debug_assertions)]
            action_type_name,
        }
    }

    /// Consumes the pending property update request.
    pub(crate) const fn take_request_update_props(&mut self) -> bool {
        let requested = self.request_update_props();
        self.set_request_update_props(false);
        requested
    }

    /// Updates state to incorporate state changes from a child.
    ///
    /// This method is idempotent and can be called multiple times.
    pub(crate) fn merge_up(&mut self, child_state: &Self) {
        if child_state.needs_layout() {
            self.measurement_cache.clear();
        }
        self.flags |= child_state.flags & PROPAGATED_FLAGS;
    }

    // TODO: Add WidgetState::add_diff method that merges a ClassSetDiff into the WidgetState's class_diff.

    /// Returns `true` if this widget or a descendant explicitly requested layout.
    pub(crate) const fn needs_layout(&self) -> bool {
        self.flags & WidgetFlag::NeedsLayout as u64 != 0
    }

    /// Sets the flag for whether this widget or a descendant explicitly requested layout.
    ///
    /// If set to `true` this also clears the measurement cache.
    pub(crate) fn set_needs_layout(&mut self, needs_layout: bool) {
        if needs_layout {
            self.measurement_cache.clear();
        }
        self.flags = (self.flags & !(WidgetFlag::NeedsLayout as u64))
            | ((needs_layout as u64) * WidgetFlag::NeedsLayout as u64);
    }

    /// Returns the widget's aligned content-box in the widget's border-box coordinate space.
    pub(crate) fn content_box(&self) -> Rect {
        let border_box = self.border_box();
        let x0 = border_box.x0 + self.border_box_insets.x0;
        let y0 = border_box.y0 + self.border_box_insets.y0;
        let x1 = (border_box.x1 - self.border_box_insets.x1).max(x0);
        let y1 = (border_box.y1 - self.border_box_insets.y1).max(y0);
        Rect::new(x0, y0, x1, y1)
    }

    /// Returns the widget's aligned border-box in the widget's border-box coordinate space.
    pub(crate) fn border_box(&self) -> Rect {
        (self.end_point - self.origin).to_size().to_rect()
    }

    /// Returns the widget's aligned paint-box in the widget's border-box coordinate space.
    pub(crate) fn paint_box(&self) -> Rect {
        self.border_box() + self.paint_box_insets
    }

    /// Returns the [`Vec2`] for translating between this widget's
    /// content-box and border-box coordinate spaces.
    ///
    /// Add this [`Vec2`] to translate from content-box to border-box,
    /// and subtract this [`Vec2`] to translate from border-box to content-box.
    pub(crate) const fn border_box_translation(&self) -> Vec2 {
        Vec2::new(self.border_box_insets.x0, self.border_box_insets.y0)
    }

    /// Returns this widget's total local transform needed for composing.
    ///
    /// This maps from this widget's border-box coordinate space into its
    /// parent's border-box coordinate space. It includes the widget's local
    /// transform, scroll translation, and origin.
    pub(crate) fn compose_local_transform(&self) -> Affine {
        // The translation needs to be applied after the local transform so scrolling
        // and layout origin are in the transformed coordinate space, similar to CSS.
        let local_translation = self.scroll_translation + self.origin.to_vec2();
        self.transform.then_translate(local_translation)
    }

    /// Returns the first baseline relative to the top of the widget's layout border-box.
    pub(crate) const fn layout_first_baseline(&self) -> f64 {
        if self.first_baseline.is_nan() {
            self.layout_border_box_size.height
        } else {
            self.first_baseline
        }
    }

    /// Returns the last baseline relative to the top of the widget's layout border-box.
    pub(crate) const fn layout_last_baseline(&self) -> f64 {
        if self.last_baseline.is_nan() {
            self.layout_border_box_size.height
        } else {
            self.last_baseline
        }
    }

    /// Returns the first baseline relative to the top of the widget's aligned border-box.
    pub(crate) fn aligned_first_baseline(&self) -> f64 {
        if self.first_baseline.is_nan() {
            self.end_point.y - self.origin.y
        } else {
            self.first_baseline
        }
    }

    /// Returns the last baseline relative to the top of the widget's aligned border-box.
    pub(crate) fn aligned_last_baseline(&self) -> f64 {
        if self.last_baseline.is_nan() {
            self.end_point.y - self.origin.y
        } else {
            self.last_baseline
        }
    }

    /// Returns the area being edited by an IME, in the window's coordinate space.
    ///
    /// If no explicit `ime_area` has been defined this will return the effective border-box area.
    pub(crate) fn get_ime_area(&self) -> Rect {
        // Note: this returns sensible values for a widget that is translated and/or rescaled.
        // Other transformations like rotation may produce weird IME areas.
        self.window_transform
            .transform_rect_bbox(self.ime_area.unwrap_or_else(|| self.border_box()))
    }

    /// Returns the result of intersecting the widget's clip path (if any) with the given rect.
    ///
    /// Both the argument and the result are in window coordinates.
    ///
    /// Returns `None` if the given rect is clipped out.
    pub(crate) fn clip_child(&self, child_rect: Rect) -> Option<Rect> {
        self.clip_path.map_or(Some(child_rect), |clip_path| {
            let clip_path_global = self.window_transform.transform_rect_bbox(clip_path);
            if clip_path_global.overlaps(child_rect) {
                Some(clip_path_global.intersect(child_rect))
            } else {
                None
            }
        })
    }

    pub(crate) const fn needs_rewrite_passes(&self) -> bool {
        self.flags & REWRITE_FLAGS != 0
    }
}
