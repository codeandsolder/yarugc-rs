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
    selected: bool,
    highlighted: bool,
}

// --- MARK: BUILDERS
impl SelectorItem {
    /// Creates new selector item with the given text.
    #[must_use = "Use the returned value."]
    pub fn new(text: String) -> Self {
        Self {
            child: WidgetPod::new(Label::new(text)),
            selected: false,
            highlighted: false,
        }
    }

    pub(crate) const fn with_selection(mut self, selected: bool) -> Self {
        self.selected = selected;
        self.highlighted = selected;
        self
    }
}

// --- MARK: WIDGETMUT
impl SelectorItem {
    pub(crate) fn set_highlighted(this: &mut WidgetMut<'_, Self>, highlighted: bool) {
        if this.widget.highlighted != highlighted {
            this.widget.highlighted = highlighted;
            if highlighted {
                this.ctx.add_class("#highlighted");
            } else {
                this.ctx.remove_class("#highlighted");
            }
        }
    }

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
            Update::WidgetAdded if self.highlighted => ctx.add_class("#highlighted"),
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
        Role::ListBoxOption
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        node.add_action(accesskit::Action::Click);
        node.set_selected(self.selected);
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("SelectorItem", id = id.trace())
    }
}
