// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! The layout pass, which provides the size and position of each widget
//! before any translations applied in [`compose`](crate::passes::compose).
//!
//! The framework helps orchestrate the size computation alongside [`Widget::measure`].
//! The layout size is passed to [`Widget::layout`].

use dpi::LogicalSize;
use tracing::{info_span, trace};
use tree_arena::ArenaMut;
#[cfg(debug_assertions)]
use tree_arena::ArenaMutList;

use crate::app::{RenderRoot, RenderRootSignal, RenderRootState, WindowSizePolicy};
#[cfg(debug_assertions)]
use crate::core::ChildrenIds;
use crate::core::{
    LayoutCtx, MeasureCtx, PropertiesRef, PropertyArena, Widget, WidgetArenaNode, WidgetState,
};
use crate::kurbo::{Axis, Insets, Point, Size};
use crate::layout::{LayoutSize, LenDef, LenReq, Length, MeasurementInputs, SizeDef};
use crate::passes::{enter_span_if, recurse_on_children};
use crate::properties::{BorderWidth, BoxShadow, Dimensions, Padding};
use crate::util::Sanitize;

// --- MARK: COMPUTE SIZE

/// Measures the preferred border-box length of `widget` on the given `axis`.
fn measure_border_box(
    widget: &mut dyn Widget,
    ctx: &mut MeasureCtx<'_>,
    props: &PropertiesRef<'_>,
    axis: Axis,
    len_req: LenReq,
    cross_length: Option<Length>,
) -> Length {
    let cache = ctx.property_cache();
    let border = props.get::<BorderWidth>(cache);
    let padding = props.get::<Padding>(cache);

    let border_and_padding_length = border.length(axis).saturating_add(padding.length(axis));

    // Reduce the border-box length by the border and padding length to get the content-box length.
    let len_req = len_req.reduce(border_and_padding_length);
    let cross_length = cross_length.map(|cross_length| {
        let cross = axis.cross();
        cross_length
            .saturating_sub(border.length(cross))
            .saturating_sub(padding.length(cross))
    });

    // Measure the content-box length.
    let content_length = widget.measure(ctx, props, axis, len_req, cross_length);

    // Add border and padding to the content-box length to return the border-box length.
    content_length.saturating_add(border_and_padding_length)
}

/// Resolves the [`LenDef`] of the provided `axis`.
///
/// Unless `Fixed`, this will result in a [`measure`] invocation.
///
/// [`measure`]: Widget::measure
fn resolve_len_def(
    widget: &mut dyn Widget,
    ctx: &mut MeasureCtx<'_>,
    props: &PropertiesRef<'_>,
    axis: Axis,
    len_def: LenDef,
    cross_length: Option<Length>,
) -> Length {
    let len_req = match len_def {
        LenDef::Fixed(val) => return val,
        LenDef::MinContent => LenReq::MinContent,
        LenDef::MaxContent => LenReq::MaxContent,
        LenDef::FitContent(space) => LenReq::FitContent(space),
    };

    let inputs = MeasurementInputs::new(axis, len_req, cross_length);
    let cached_result = ctx.widget_state.measurement_cache.get(&inputs);

    #[cfg(debug_assertions)]
    let result = {
        // With debug assertions enabled, we will always measure regardless of cache.
        let result = measure_border_box(widget, ctx, props, axis, len_req, cross_length);
        // If the cache did have the result, we verify that it matches.
        if let Some(cached_result) = cached_result {
            if cached_result != result {
                debug_panic!(
                    "Widget '{}' {} measurement for {inputs:?} returned {result} \
                    but cache already had {cached_result}. Widget::measure needs \
                    to always return the same result for the same inputs. If some other data \
                    changed which influenced the results, you need to request layout!",
                    widget.short_type_name(),
                    ctx.widget_id(),
                );
            }
            return cached_result;
        }
        result
    };
    #[cfg(not(debug_assertions))]
    let result = {
        if let Some(cached_result) = cached_result {
            return cached_result;
        }
        measure_border_box(widget, ctx, props, axis, len_req, cross_length)
    };

    if ctx.cache_result {
        ctx.widget_state.measurement_cache.insert(inputs, result);
    }

    result
}

/// Resolves the widget's preferred border-box length on the given `axis`.
///
/// `auto_length` specifies the fallback behavior if a widget's dimension is [`Dim::Auto`].
///
/// # Panics
///
/// Panics if a dimension resolves to a non-finite or negative value
/// and debug assertions are enabled. This can happen if the involved numbers are huge,
/// e.g. a logical size of `f64::MAX` scaled by `1.5`.
///
/// [`Dim::Auto`]: crate::layout::Dim::Auto
pub(crate) fn resolve_length(
    global_state: &mut RenderRootState,
    property_arena: &PropertyArena,
    node: ArenaMut<'_, WidgetArenaNode>,
    auto_length: LenDef,
    context_size: LayoutSize,
    axis: Axis,
    cross_length: Option<Length>,
) -> Length {
    // Get the dimensions
    let class_set = &node.item.class_set;
    let cache = &mut node.item.state.property_cache;
    let widget = &mut *node.item.widget;
    let stack = property_arena.get(node.item.state.property_stack_id, widget.type_id());
    let props = PropertiesRef {
        local: &node.item.properties,
        default_map: property_arena
            .default_properties
            .for_widget(widget.type_id()),
        stack,
        class_set,
    };
    let dims = props.get::<Dimensions>(cache);

    // Resolve the dimension on the given axis
    let len_def = dims
        .dim(axis)
        .resolve(context_size.length(axis))
        .unwrap_or(auto_length);

    // Return immediately if we already have a fixed length
    if let LenDef::Fixed(length) = len_def {
        return length;
    }

    // Otherwise fall back to measurement
    let mut children = node.children;
    let mut ctx = MeasureCtx {
        global_state,
        widget_state: &mut node.item.state,
        children: children.reborrow_mut(),
        property_arena,
        auto_length,
        context_size,
        cache_result: true,
    };

    // Resolve the cross dimension in case it's fixed
    let cross_length = cross_length.or_else(|| {
        let cross = axis.cross();
        dims.dim(cross)
            .resolve(context_size.length(cross))
            .and_then(LenDef::fixed)
    });

    // Measure
    resolve_len_def(widget, &mut ctx, &props, axis, len_def, cross_length)
}

/// Resolves the widget's preferred border-box size.
///
/// The returned size will be finite, non-negative, and in logical pixels.
///
/// `auto_size` specifies the fallback behavior if a widget's dimension is [`Dim::Auto`].
///
/// # Panics
///
/// Panics if a dimension resolves to a non-finite or negative value
/// and debug assertions are enabled. This can happen if the involved numbers are huge,
/// e.g. a logical size of `f64::MAX` scaled by `1.5`.
///
/// [`Dim::Auto`]: crate::layout::Dim::Auto
pub(crate) fn resolve_size(
    global_state: &mut RenderRootState,
    property_arena: &PropertyArena,
    node: ArenaMut<'_, WidgetArenaNode>,
    auto_size: SizeDef,
    context_size: LayoutSize,
) -> Size {
    // Currently we only support the common horizontal-tb writing mode,
    // so the assignments are hardcoded here, but the rest of the function adapts.
    let (inline, block) = (Axis::Horizontal, Axis::Vertical);

    // Get the dimensions
    let class_set = &node.item.class_set;
    let cache = &mut node.item.state.property_cache;
    let widget = &mut *node.item.widget;
    let stack = property_arena.get(node.item.state.property_stack_id, widget.type_id());
    let props = PropertiesRef {
        local: &node.item.properties,
        default_map: property_arena
            .default_properties
            .for_widget(widget.type_id()),
        stack,
        class_set,
    };
    let dims = props.get::<Dimensions>(cache);

    // Resolve the dimensions
    let inline_auto = auto_size.dim(inline);
    let inline_def = dims
        .dim(inline)
        .resolve(context_size.length(inline))
        .unwrap_or(inline_auto);
    let block_auto = auto_size.dim(block);
    let block_def = dims
        .dim(block)
        .resolve(context_size.length(block))
        .unwrap_or(block_auto);

    // Return immediately if we already have a fixed size
    let inline_length = inline_def.fixed();
    let block_length = block_def.fixed();
    if let Some(inline_length) = inline_length
        && let Some(block_length) = block_length
    {
        return inline.pack_size(inline_length.get(), block_length.get());
    }

    // Otherwise fall back to measurement
    let mut children = node.children;
    let mut ctx = MeasureCtx {
        global_state,
        widget_state: &mut node.item.state,
        children: children.reborrow_mut(),
        property_arena,
        auto_length: inline_auto,
        context_size,
        cache_result: true,
    };

    let inline_length = inline_length.unwrap_or_else(|| {
        resolve_len_def(widget, &mut ctx, &props, inline, inline_def, block_length)
    });

    // Update the auto length
    ctx.auto_length = block_auto;

    // Even if the inline measurement couldn't cache, the block one might be able to.
    ctx.cache_result = true;

    let block_length = block_length.unwrap_or_else(|| {
        resolve_len_def(
            widget,
            &mut ctx,
            &props,
            block,
            block_def,
            Some(inline_length),
        )
    });

    inline.pack_size(inline_length.get(), block_length.get())
}

// --- MARK: RUN LAYOUT
/// Run [`Widget::layout`] method on the given widget.
/// This will be called by [`LayoutCtx::run_layout`], which is itself called in the parent widget's `layout`.
///
/// The provided `size` will be the given widget's chosen border-box size.
///
/// If the chosen border-box `size` is smaller than what is required to fit the widget's
/// borders and padding, then the `size` will be expanded to meet those constraints.
///
/// The provided `size` must be finite, non-negative, and in logical pixels.
/// Non-finite or negative length will fall back to zero with a logged warning.
///
/// # Panics
///
/// Panics if `size` is non-finite or negative and debug assertions are enabled.
///
/// [`Widget::layout`]: crate::core::Widget::layout
pub(crate) fn run_layout_on(
    global_state: &mut RenderRootState,
    property_arena: &PropertyArena,
    node: ArenaMut<'_, WidgetArenaNode>,
    chosen_size: Size,
) {
    // Ensure the chosen size is sanitized.
    let chosen_size = Size::new(
        chosen_size.width.sanitize("chosen border-box size width"),
        chosen_size.height.sanitize("chosen border-box size height"),
    );

    let mut children = node.children;
    let widget = &mut *node.item.widget;
    let state = &mut node.item.state;
    let properties = &mut node.item.properties;
    let class_set = &node.item.class_set;
    let id = state.id;
    let trace = global_state.trace.enabled(crate::passes::TracePass::Layout);
    let _span = enter_span_if(trace, state);

    // This checks reads `is_explicitly_stashed` instead of `is_stashed` because the latter may be outdated.
    // A widget's `is_explicitly_stashed` flag is controlled by its direct parent.
    // The parent may set this flag during layout, in which case it should avoid calling `run_layout`.
    // Note that, because this check exits before recursing, `run_layout` can only ever be
    // reached for a widget whose parent is not stashed, which means `is_explicitly_stashed`
    // being false is sufficient to know the widget is non-stashed.
    if state.is_explicitly_stashed() {
        debug_panic!(
            "Error in '{}' {}: trying to compute layout of stashed widget.",
            widget.short_type_name(),
            id,
        );
        state.origin = Point::ZERO;
        state.end_point = Point::ZERO;
        state.layout_border_box_size = Size::ZERO;
        return;
    }

    let stack = property_arena.get(state.property_stack_id, widget.type_id());
    let props = PropertiesRef {
        local: properties,
        default_map: property_arena
            .default_properties
            .for_widget(widget.type_id()),
        stack,
        class_set,
    };

    let border_width = props.get::<BorderWidth>(&mut state.property_cache);
    let padding = props.get::<Padding>(&mut state.property_cache);

    // Force the border-box size to be large enough to actually contain the border and padding.
    let border_box_insets = border_width.insets() + padding.insets();
    let minimum_size = border_box_insets.size();
    let border_box_size = minimum_size.max(chosen_size);

    if !state.needs_layout() && state.layout_border_box_size == border_box_size {
        // We reset this to false to mark that the current widget has been visited.
        state.set_request_layout(false);
        return;
    }
    state.layout_border_box_size = border_box_size;

    // TODO - Not everything that has been re-laid out needs to be repainted.
    state.set_needs_paint(true);
    state.set_needs_compose(true);
    state.set_needs_accessibility(true);
    state.set_request_pre_paint(true);
    state.set_request_paint(true);
    state.set_request_post_paint(true);
    state.set_request_compose(true);
    state.set_request_accessibility(true);

    if trace {
        trace!(
            "Computing layout with border-box size {:?}",
            border_box_size
        );
    }

    #[cfg(debug_assertions)]
    let children_ids = mark_layout_children(widget, id, children.reborrow_mut());

    state.paint_box_insets = Insets::ZERO;

    // Compute the insets for deriving the content-box from the border-box
    state.border_box_insets = border_box_insets;

    // Compute the content-box size
    let content_box_size = Size::new(
        (border_box_size.width - minimum_size.width).max(0.0),
        (border_box_size.height - minimum_size.height).max(0.0),
    );

    let mut ctx = LayoutCtx {
        global_state,
        widget_state: state,
        children: children.reborrow_mut(),
        property_arena,
    };

    // Run the widget's layout
    widget.layout(&mut ctx, &props, content_box_size);

    // If children are stashed, the layout pass will not recurse over them.
    // We reset need_layout and request_layout to false directly instead.
    //
    // See the notes about `is_explicitly_stashed` above.
    recurse_on_children(id, widget, children.reborrow_mut(), |node| {
        if node.item.state.is_explicitly_stashed() {
            clear_layout_flags(node);
        }
    });

    // Make sure the paint insets cover the shadow insets
    let shadow = props.get::<BoxShadow>(&mut state.property_cache);
    if shadow.is_visible() {
        let shadow_insets = shadow.get_insets();
        state.paint_box_insets = Insets {
            x0: state.paint_box_insets.x0.max(shadow_insets.x0),
            y0: state.paint_box_insets.y0.max(shadow_insets.y0),
            x1: state.paint_box_insets.x1.max(shadow_insets.x1),
            y1: state.paint_box_insets.y1.max(shadow_insets.y1),
        };
    }

    if trace {
        trace!(
            "Computed layout: border-box={}, first_baseline={}, last_baseline={} insets={:?}",
            border_box_size, state.first_baseline, state.last_baseline, state.paint_box_insets,
        );
    }

    state.set_request_layout(false);
    state.set_needs_layout(false);
    state.set_is_expecting_place_child_call(true);

    #[cfg(debug_assertions)]
    validate_layout_children(widget, state, children, &children_ids);
}

/// Capture the declaration and mark every participating child before layout.
#[cfg(debug_assertions)]
fn mark_layout_children(
    widget: &dyn Widget,
    id: crate::core::WidgetId,
    mut children: ArenaMutList<'_, WidgetArenaNode>,
) -> ChildrenIds {
    let ids = widget.children_ids();
    for &child_id in &ids {
        let Some(child) = children.item_mut(child_id) else {
            debug_panic!(
                "Error in '{}' {id}: child {child_id} is not registered",
                widget.short_type_name()
            );
            continue;
        };
        let state = &mut child.item.state;
        if !state.is_explicitly_stashed() {
            state.set_request_layout(true);
        }
    }
    ids
}

/// Validate that layout visited and placed all non-stashed children.
#[cfg(debug_assertions)]
fn validate_layout_children(
    widget: &dyn Widget,
    state: &WidgetState,
    mut children: ArenaMutList<'_, WidgetArenaNode>,
    old_ids: &ChildrenIds,
) {
    let name = widget.short_type_name();
    let id = state.id;
    let ids = widget.children_ids();
    for &child_id in &ids {
        let Some(child) = children.item_mut(child_id) else {
            debug_panic!("Error in '{name}' {id}: child {child_id} is not registered");
            continue;
        };
        let child_state = &mut child.item.state;
        if child_state.is_explicitly_stashed() {
            child_state.set_request_layout(false);
            continue;
        }
        if child_state.request_layout() {
            debug_panic!(
                "Error in '{name}' {id}: LayoutCtx::run_layout() was not called with child widget '{}' {}.",
                child_state.widget_name,
                child_state.id,
            );
        }
        if child_state.is_expecting_place_child_call() {
            debug_panic!(
                "Error in '{name}' {id}: LayoutCtx::place_child() was not called with child widget '{}' {}.",
                child_state.widget_name,
                child_state.id,
            );
        }
    }
    if *old_ids != ids && !state.children_changed() {
        debug_panic!("Error in '{name}' {id}: children changed during layout pass");
    }
}

// --- MARK: CLEAR LAYOUT
// This function is called on stashed widgets and their children
// to set all layout flags to false.
fn clear_layout_flags(node: ArenaMut<'_, WidgetArenaNode>) {
    let children = node.children;
    let widget = &mut *node.item.widget;
    let state = &mut node.item.state;
    let id = state.id;

    // If `needs_layout` is false, no descendant has layout flags set.
    if !state.needs_layout() {
        return;
    }

    state.set_needs_layout(false);
    state.set_request_layout(false);

    recurse_on_children(id, widget, children, |node| {
        clear_layout_flags(node);
    });
}

// --- MARK: PLACE WIDGET
/// Places the child at `origin` in its parent's border-box coordinate space.
pub(crate) fn place_widget(child_state: &mut WidgetState, origin: Point) {
    let end_point = origin + child_state.layout_border_box_size.to_vec2();
    // TODO - Account for display scale in pixel snapping
    // See https://github.com/linebender/xilem/issues/1264
    let origin = origin.round();
    let end_point = end_point.round();

    // TODO - We may want to invalidate in other cases as well
    if origin != child_state.origin {
        child_state.set_transform_changed(true);
        child_state.set_needs_compose(true);
    }
    child_state.origin = origin;
    child_state.end_point = end_point;

    child_state.set_is_expecting_place_child_call(false);
}

// --- MARK: ROOT
/// See the [passes documentation](crate::doc::pass_system#layout-pass).
pub(crate) fn run_layout_pass(root: &mut RenderRoot) {
    if !root.root_state().is_some_and(WidgetState::needs_layout) {
        return;
    }

    let _span = info_span!("layout").entered();
    root.global_state.set_needs_pointer_pass(true);

    let window_size = root.get_kurbo_size();
    let Some(mut root_node) = root.widget_arena.nodes.find_mut(root.root_id()) else {
        return;
    };
    let root_node_size = match root.global_state.size_policy {
        WindowSizePolicy::User => resolve_size(
            &mut root.global_state,
            &root.property_arena,
            root_node.reborrow_mut(),
            SizeDef::fixed(window_size),
            window_size.into(),
        ),
        WindowSizePolicy::Content => resolve_size(
            &mut root.global_state,
            &root.property_arena,
            root_node.reborrow_mut(),
            SizeDef::MAX,
            LayoutSize::NONE,
        ),
    };

    run_layout_on(
        &mut root.global_state,
        &root.property_arena,
        root_node.reborrow_mut(),
        root_node_size,
    );
    place_widget(&mut root_node.item.state, Point::ORIGIN);

    if root.global_state.size_policy == WindowSizePolicy::Content {
        // We use the aligned border-box size, which means that transforms won't affect window size.
        let size = root_node.item.state.border_box().size();
        let new_size =
            LogicalSize::new(size.width, size.height).to_physical(root.global_state.scale_factor);
        if root.global_state.size != new_size {
            root.global_state.size = new_size;
            root.global_state
                .emit_signal(RenderRootSignal::SetSize(new_size));
        }
    }
}
