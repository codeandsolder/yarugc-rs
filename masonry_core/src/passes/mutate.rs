// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use tracing::info_span;

use crate::app::RenderRoot;
use crate::core::{FromDynWidget, MutateCtx, PropertiesMut, Widget, WidgetId, WidgetMut};
use crate::passes::merge_state_up;

pub(crate) fn mutate_widget<R, W: Widget + FromDynWidget + ?Sized>(
    root: &mut RenderRoot,
    id: WidgetId,
    mutate_fn: impl FnOnce(WidgetMut<'_, W>) -> R,
) -> Option<R> {
    let node = root.widget_arena.nodes.find_mut(id)?;
    let children = node.children;
    let widget = W::from_dyn_mut(&mut *node.item.widget)?;
    let state = &mut node.item.state;
    let properties = &mut node.item.properties;
    let class_set = &node.item.class_set;
    let id = state.id;
    let stack = root
        .property_arena
        .get(state.property_stack_id, widget.as_dyn().type_id());

    let _span = info_span!("mutate_widget", name = widget.short_type_name()).entered();

    // NOTE - we can set parent_widget_state to None here, because the loop below will merge the
    // states up to the root.

    let root_widget = WidgetMut {
        ctx: MutateCtx {
            global_state: &mut root.global_state,
            parent_widget_state: None,
            widget_state: state,
            properties: PropertiesMut {
                local: properties,
                default_map: root
                    .property_arena
                    .default_properties
                    .for_widget(widget.as_dyn().type_id()),
                stack,
                class_set,
            },
            children,
            property_arena: &root.property_arena,
        },
        widget,
    };

    let result = mutate_fn(root_widget);

    // Merge all state changes up to the root.
    let mut current_id = Some(id);
    while let Some(id) = current_id {
        let parent_id = root.widget_arena.parent_of(id);
        merge_state_up(&mut root.widget_arena, id);
        current_id = parent_id;
    }

    Some(result)
}

/// Apply any deferred mutations (created using `...Ctx::mutate_later`)
///
/// See the [passes documentation](crate::doc::pass_system#the-mutate-pass).
pub(crate) fn run_mutate_pass(root: &mut RenderRoot) {
    let callbacks = std::mem::take(&mut root.global_state.mutate_callbacks);
    for callback in callbacks {
        // A target may have been removed since the callback was queued.
        let _ = mutate_widget(root, callback.id, callback.callback);
    }
}
