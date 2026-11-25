// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use accesskit::Role;
use tracing::{Span, trace_span};

use crate::core::{FromDynWidget, NewWidget, NoAction, Widget, WidgetId, WidgetMut, WidgetPod};
use crate::widgets::RadioButton;

/// A radio group container that holds radio buttons.
///
/// At most one button in the nearest group is selected. If multiple buttons
/// are initially checked, the last one in registration order is selected.
pub struct RadioGroup {
    pub(crate) child: WidgetPod<dyn Widget>,
    selected_button: Option<WidgetId>,
}

impl RadioGroup {
    /// Create a new `RadioGroup`.
    #[must_use = "Use the returned value."]
    pub fn new(child: NewWidget<impl Widget + ?Sized>) -> Self {
        Self {
            child: child.erased().into_pod(),
            selected_button: None,
        }
    }
}

// --- MARK: WIDGETMUT
impl RadioGroup {
    // Apply requests in order and enqueue BOTH sides of the transition.
    // Eagerly checking the new button and deferring only unchecking the old
    // one loses the final selection when a batch selects A, B, then A.
    pub(super) fn apply_selection(mut this: WidgetMut<'_, Self>, id: WidgetId, checked: bool) {
        let group_id = this.id();
        if !this
            .ctx
            .get_descendant::<RadioButton>(id)
            .is_some_and(|button| button.group_id() == Some(group_id))
        {
            return;
        }
        if checked {
            if let Some(previous) = this.widget.selected_button.replace(id)
                && previous != id
            {
                this.ctx
                    .mutate_later::<RadioButton>(previous, move |mut button| {
                        if button.widget.group_id() == Some(group_id) {
                            RadioButton::apply_checked(&mut button, false);
                        }
                    });
            }
        } else if this.widget.selected_button == Some(id) {
            this.widget.selected_button = None;
        }
        this.ctx.mutate_later::<RadioButton>(id, move |mut button| {
            if button.widget.group_id() == Some(group_id) {
                RadioButton::apply_checked(&mut button, checked);
            }
        });
    }

    /// Get mutable reference to the child widget.
    pub fn child_mut<'t, W: Widget + FromDynWidget + ?Sized>(
        this: &'t mut WidgetMut<'_, Self>,
    ) -> Option<WidgetMut<'t, W>> {
        this.ctx.get_mut(this.widget.child.id())
    }
}

// --- MARK: IMPL WIDGET
impl Widget for RadioGroup {
    type Action = NoAction;

    // TODO: navigation shortcuts.

    masonry_core::single_child_container!(child, fill);

    fn accessibility_role(&self) -> Role {
        Role::RadioGroup
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("RadioGroup", id = id.trace())
    }
}

// TODO: make tests.
