// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use accesskit::{Node, Role};
use tracing::{Span, trace_span};

use crate::core::{
    AccessCtx, EventCtx, Layer, NewWidget, NoAction, PointerEvent, PropertiesMut, PropertiesRef,
    Widget, WidgetId, WidgetPod,
};
use crate::layout::UnitPoint;

/// A [`Layer`] representing a simple tooltip showing some content until the mouse moves.
pub struct Tooltip {
    child: WidgetPod<dyn Widget>,
}

// --- MARK: BUILDERS
impl Tooltip {
    /// Creates a new `Tooltip`.
    pub fn new(child: NewWidget<impl Widget + ?Sized>) -> Self {
        Self {
            child: child.erased().into_pod(),
        }
    }
}

// --- MARK: WIDGETMUT
impl Tooltip {
    child_accessors!(child, set_child, child_mut, erased);
}

// --- MARK: IMPL WIDGET
impl Widget for Tooltip {
    type Action = NoAction;

    masonry_core::single_child_container!(child, UnitPoint::CENTER);

    fn accessibility_role(&self) -> Role {
        Role::Tooltip
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        _node: &mut Node,
    ) {
    }

    fn as_layer(&mut self) -> Option<&mut dyn Layer> {
        Some(self)
    }

    fn accepts_pointer_interaction(&self) -> bool {
        false
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("Tooltip", id = id.trace())
    }
}

// --- MARK: IMPL LAYER
impl Layer for Tooltip {
    fn capture_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        let remove_tooltip = matches!(
            event,
            PointerEvent::Down(_)
                | PointerEvent::Up(_)
                | PointerEvent::Move(_)
                | PointerEvent::Leave(_)
        );

        if remove_tooltip {
            ctx.remove_layer(ctx.widget_id());
        }
    }
}
