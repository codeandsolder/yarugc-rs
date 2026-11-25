// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use accesskit::Role;
use tracing::{Span, trace_span};

use crate::core::{NewWidget, NoAction, Widget, WidgetId, WidgetPod};

/// A pass-through container that hosts exactly one child, which may be replaced dynamically.
///
/// Useful when you need an insertion point for one widget that can be
/// replaced at runtime, without adding layout or chrome of its own.
///
/// Ensure that `Passthrough` has [`Dimensions`] set via props
/// either to [`Dimensions::fixed`] or [`Dimensions::MAX`].
/// Fixed dimensions resolve early and are explicit in intent.
/// Max preferred size of `Passthrough` means that the question of size
/// will get passed through to its child, and doesn't mean that it will
/// necessarily map to the max preferred size of the child.
///
/// # Examples
/// Create a host and later replace its content inside a mutate/edit callback:
/// ```
/// use masonry::core::Widget;
/// use masonry::properties::Dimensions;
/// use masonry::widgets::{Passthrough, Label};
///
/// // Create a host around a label
/// let host = Passthrough::new(Label::new("Hello").prepare()).prepare().with_props(Dimensions::MAX);
///
/// // ... in an edit callback, mutate the widget tree
/// # fn edit(mut host: masonry::core::WidgetMut<'_, Passthrough>) {
/// Passthrough::set_child(&mut host, Label::new("World").prepare());
/// # }
/// ```
///
/// [`Dimensions`]: crate::properties::Dimensions
/// [`Dimensions::fixed`]: crate::properties::Dimensions::fixed
/// [`Dimensions::MAX`]: crate::properties::Dimensions::MAX
pub struct Passthrough {
    inner: WidgetPod<dyn Widget>,
}

// --- MARK: BUILDERS
impl Passthrough {
    /// Creates a new `Passthrough` with the given initial child.
    #[must_use = "Use the returned value."]
    pub fn new(child: NewWidget<impl Widget + ?Sized>) -> Self {
        Self {
            inner: child.erased().into_pod(),
        }
    }
}

// --- MARK: METHODS
impl Passthrough {
    /// Returns the [`WidgetId`] of the hosted child.
    #[must_use = "Use the returned value."]
    pub fn inner_id(&self) -> WidgetId {
        self.inner.id()
    }
}

// --- MARK: WIDGETMUT
impl Passthrough {
    child_accessors!(inner, set_child, child_mut, erased);
}

// --- MARK: IMPL WIDGET
impl Widget for Passthrough {
    type Action = NoAction;

    masonry_core::single_child_container!(inner, fill);

    fn accessibility_role(&self) -> Role {
        Role::GenericContainer
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("Passthrough", id = id.trace())
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::properties::Dimensions;
    use crate::testing::{TestHarness, assert_render_snapshot};
    use crate::theme::test_property_set;
    use crate::widgets::Label;

    #[test]
    fn passthrough_replaces_child() {
        // Start with a label
        let widget = Passthrough::new(Label::new("A").prepare())
            .prepare()
            .with_props(Dimensions::MAX);
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (30, 30));

        assert_render_snapshot!(harness, "passthrough_initial_label_A");

        harness
            .edit_root_widget(|mut host| {
                let mut child = Passthrough::child_mut::<dyn Widget>(&mut host).unwrap();

                // Test that child_mut returns a pointer to the child label
                assert!(child.try_downcast::<Label>().is_some());
            })
            .unwrap();

        // Replace with a label with different text
        harness
            .edit_root_widget(|mut host| {
                Passthrough::set_child(&mut host, Label::new("B").prepare());
            })
            .unwrap();

        assert_render_snapshot!(harness, "passthrough_replaced_label_B");
    }
}
