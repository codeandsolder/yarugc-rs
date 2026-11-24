// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use tracing::info_span;

use crate::app::RenderRoot;
use crate::core::{FromDynWidget, Widget, WidgetId, WidgetMut};
use crate::passes::merge_state_up;

pub(crate) fn mutate_widget<R, W: Widget + FromDynWidget + ?Sized>(
    root: &mut RenderRoot,
    id: WidgetId,
    mutate_fn: impl FnOnce(WidgetMut<'_, W>) -> R,
) -> Option<R> {
    let node = root.widget_arena.nodes.find_mut(id)?;
    // The ancestor path is merged after the callback, so the handle itself
    // need not retain a mutable reference to its parent.
    let root_widget: WidgetMut<'_, W> =
        WidgetMut::from_node(node, &mut root.global_state, &root.property_arena, None)?;
    let _span = info_span!("mutate_widget", name = root_widget.widget.short_type_name()).entered();

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
