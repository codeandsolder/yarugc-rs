// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

#![expect(
    missing_debug_implementations,
    reason = "Widgets are not expected to implement Debug"
)]

use masonry_core::accesskit::{Node, Role};
use masonry_core::core::{
    AccessCtx, FromDynWidget, NewWidget, NoAction, PropertiesRef, Widget, WidgetMut, WidgetPod,
};
use masonry_core::layout::UnitPoint;

/// A basic wrapper widget that can replace its child.
pub struct WrapperWidget {
    child: WidgetPod<dyn Widget>,
}

impl WrapperWidget {
    /// Creates a new `WrapperWidget`.
    ///
    /// The `child` is the initial child widget.
    #[must_use = "Use the wrapped test widget."]
    pub fn new(child: NewWidget<impl Widget>) -> Self {
        Self {
            child: child.erased().into_pod(),
        }
    }

    /// Returns mutable reference to the child widget.
    pub fn child_mut<'t, W: Widget + FromDynWidget + ?Sized>(
        this: &'t mut WidgetMut<'_, Self>,
    ) -> Option<WidgetMut<'t, W>> {
        this.ctx.get_mut(this.widget.child.id())
    }
}

impl WrapperWidget {
    /// Replaces the container's child widget.
    pub fn set_child(this: &mut WidgetMut<'_, Self>, child: impl Widget) {
        // FIXME - Take NewWidget argument
        Self::set_child_pod(this, NewWidget::new(child).erased().into_pod());
    }

    /// Replaces the container's child widget with a `WidgetPod`.
    pub fn set_child_pod(this: &mut WidgetMut<'_, Self>, child: WidgetPod<dyn Widget>) {
        let old_child = std::mem::replace(&mut this.widget.child, child);
        this.ctx.remove_child(old_child);
    }
}

impl Widget for WrapperWidget {
    type Action = NoAction;

    masonry_core::single_child_container!(child, UnitPoint::TOP_LEFT);

    fn accessibility_role(&self) -> Role {
        Role::GenericContainer
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        _node: &mut Node,
    ) {
    }
}
