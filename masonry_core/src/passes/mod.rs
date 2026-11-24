// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Internal passes run by Masonry every frame.
//!
//! See the [passes documentation](crate::doc::pass_system) for more information.
//!
//! This file includes utility functions used by multiple passes.

use tracing::span::EnteredSpan;
use tree_arena::{ArenaMut, ArenaMutList};

use crate::core::{Widget, WidgetArena, WidgetArenaNode, WidgetId, WidgetState};

pub(crate) mod accessibility;
pub(crate) mod action;
pub(crate) mod anim;
pub(crate) mod compose;
pub(crate) mod event;
pub(crate) mod layout;
pub(crate) mod mutate;
pub(crate) mod paint;
pub(crate) mod update;

#[must_use = "Span will be immediately closed if dropped"]
pub(crate) fn enter_span_if(enabled: bool, state: &WidgetState) -> Option<EnteredSpan> {
    enabled.then(|| enter_span(state))
}

#[must_use = "Span will be immediately closed if dropped"]
pub(crate) fn enter_span(state: &WidgetState) -> EnteredSpan {
    state.trace_span.clone().entered()
}

pub(crate) fn recurse_on_children(
    id: WidgetId,
    widget: &dyn Widget,
    mut children: ArenaMutList<'_, WidgetArenaNode>,
    mut callback: impl FnMut(ArenaMut<'_, WidgetArenaNode>),
) {
    let parent_id = id;

    for child_id in widget.children_ids() {
        let Some(node) = children.item_mut(child_id) else {
            let parent_name = widget.short_type_name();
            debug_panic!(
                "Error in '{parent_name}' {parent_id}: cannot find child {child_id} returned by children_ids()"
            );
            continue;
        };

        callback(node);
    }
}

pub(crate) fn merge_state_up(arena: &mut WidgetArena, widget_id: WidgetId) {
    let parent_id = arena.parent_of(widget_id);

    let Some(parent_id) = parent_id else {
        // We've reached the root
        return;
    };

    let Some(parent_node_mut) = arena.nodes.find_mut(parent_id) else {
        return;
    };
    let Some(child_node) = parent_node_mut.children.item(widget_id) else {
        return;
    };

    parent_node_mut.item.state.merge_up(&child_node.item.state);
}

/// Masonry has a significant number of passes which may traverse a significant number of
/// items.
///
/// In most cases, including these elements in traces adds noise and makes operations extremely slow.
/// Because of this, we default these traces to false.
///
/// Using the default tracing filtering mechanism for this would be non-ideal, as it would prevent child
/// spans of the item from running, which may make end-user debugging harder.
///
/// The detailed traces for these passes therefore default to false, but can be enabled using the
/// `MASONRY_TRACE_PASSES` environment variable.
///
/// Note that passes which are bounded by depth (rather than absolute size) are never filtered out here.
///
/// Ideally, we'd cache the spans, which would make a lot (but not all) of this unnecessary.
/// However, each pass uses a different parent span (with the individual pass's name), so it's
/// (at best) non-trivial to make that work.
///
/// We could *maybe* use a global parent span called "Pass", with a name field, but that's extremely ugly.
pub(crate) struct PassTracing(u8);

#[derive(Clone, Copy)]
#[repr(u8)]
pub(crate) enum TracePass {
    UpdateTree = 1 << 0,
    Anim = 1 << 1,
    Layout = 1 << 2,
    Compose = 1 << 3,
    Paint = 1 << 4,
    Access = 1 << 5,
}

impl PassTracing {
    const ALL: u8 = TracePass::UpdateTree as u8
        | TracePass::Anim as u8
        | TracePass::Layout as u8
        | TracePass::Compose as u8
        | TracePass::Paint as u8
        | TracePass::Access as u8;

    pub(crate) const fn enabled(&self, pass: TracePass) -> bool {
        self.0 & pass as u8 != 0
    }

    pub(crate) fn from_env() -> Self {
        let env_var = match std::env::var("MASONRY_TRACE_PASSES") {
            Ok(env_var) => env_var,
            Err(std::env::VarError::NotPresent) => return Self(0),
            Err(std::env::VarError::NotUnicode(value)) => {
                tracing::error!(
                    ?value,
                    "Couldn't parse `MASONRY_TRACE_PASSES` environment variable: Not valid UTF-8",
                );
                return Self(0);
            }
        };

        let env_var = env_var.trim();
        if env_var.eq_ignore_ascii_case("all") {
            return Self(Self::ALL);
        }
        let mut result = Self(0);
        let mut show_help = false;
        let supported_passes = [
            ("update_tree", TracePass::UpdateTree),
            ("anim", TracePass::Anim),
            ("layout", TracePass::Layout),
            ("compose", TracePass::Compose),
            ("paint", TracePass::Paint),
            ("access", TracePass::Access),
        ];
        for input_name in env_var.split(',').map(str::trim) {
            if input_name.eq_ignore_ascii_case("all") {
                tracing::warn!(
                    "`MASONRY_TRACE_PASSES=all` cannot be meaningfully combined with other passes"
                );
                return Self(Self::ALL);
            }
            if let Some((_, pass)) = supported_passes
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(input_name))
            {
                if result.enabled(*pass) {
                    tracing::warn!(
                        pass = input_name,
                        "MASONRY_TRACE_PASSES: Enabled tracing for same pass twice"
                    );
                }
                result.0 |= *pass as u8;
            } else {
                tracing::warn!(pass = input_name, "MASONRY_TRACE_PASSES: Unknown pass");
                show_help = true;
            }
        }
        if show_help {
            let supported_str = supported_passes
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
                .join(", ");
            tracing::warn!(
                "Supported passes for the `MASONRY_TRACE_PASSES` environment variable are {supported_str}"
            );
        }
        result
    }
}
