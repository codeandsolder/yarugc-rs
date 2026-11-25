// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use accesskit::Role;
use tracing::{Span, trace_span};

use crate::core::{FromDynWidget, NewWidget, NoAction, Widget, WidgetId, WidgetMut, WidgetPod};

/// A radio group container that holds radio buttons.
pub struct RadioGroup {
    pub(crate) child: WidgetPod<dyn Widget>,
    pub(crate) selected_button: Option<WidgetId>,
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
