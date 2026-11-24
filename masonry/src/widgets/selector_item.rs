// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use accesskit::{Node, Role};
use tracing::{Span, trace_span};

use crate::core::{
    AccessCtx, NoAction, PropertiesMut, PropertiesRef, Update, UpdateCtx, Widget, WidgetId,
    WidgetMut, WidgetPod,
};
use crate::layout::UnitPoint;
use crate::widgets::Label;

/// An option in a [`SelectorMenu`](crate::layers::SelectorMenu).
pub struct SelectorItem {
    child: WidgetPod<Label>,
}

// --- MARK: BUILDERS
impl SelectorItem {
    /// Creates new selector item with the given text.
    pub fn new(text: String) -> Self {
        Self {
            child: WidgetPod::new(Label::new(text)),
        }
    }
}

// --- MARK: WIDGETMUT
impl SelectorItem {
    /// Returns a mutable reference to the child.
    pub fn child_mut<'t>(this: &'t mut WidgetMut<'_, Self>) -> Option<WidgetMut<'t, Label>> {
        this.ctx.get_mut(this.widget.child.id())
    }
}

// --- MARK: IMPL WIDGET
impl Widget for SelectorItem {
    type Action = NoAction;

    masonry_core::single_child_container!(child, UnitPoint::CENTER);

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        match event {
            Update::HoveredChanged(_)
            | Update::ActiveChanged(_)
            | Update::FocusChanged(_)
            | Update::DisabledChanged(_) => {
                ctx.request_paint_only();
            }
            _ => {}
        }
    }

    fn accessibility_role(&self) -> Role {
        Role::MenuItem
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        node.add_action(accesskit::Action::Click);
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("SelectorItem", id = id.trace())
    }
}
