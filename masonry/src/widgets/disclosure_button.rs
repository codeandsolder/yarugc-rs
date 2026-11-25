// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use core::any::TypeId;
use core::f64::consts::FRAC_PI_2;

use super::selection_control::{Activation, ActivationKeys};
use crate::accesskit;
use crate::core::{
    AccessCtx, AccessEvent, ChildrenIds, EventCtx, LayoutCtx, MeasureCtx, NoAction, PaintCtx,
    PropertiesMut, PropertiesRef, RegisterCtx, Update, UpdateCtx, Widget, WidgetId, WidgetMut,
};
use crate::imaging::Painter;
use crate::kurbo::{Affine, Axis, BezPath, Join, Size, Stroke};
use crate::layout::{LenReq, Length};
use crate::palette::css::LIGHT_BLUE;
use crate::peniko::BrushRef;
use crate::properties::ContentColor;

// Default size is a square
const DEFAULT_LENGTH: Length = crate::layout::length!(8.);

/// A triangle button that points towards the right when undisclosed
/// and points towards the bottom when disclosed.
pub struct DisclosureButton {
    is_disclosed: bool,
    activation: Activation,
    label: Option<WidgetId>,
}

impl Default for DisclosureButton {
    fn default() -> Self {
        Self::new(false)
    }
}

// --- MARK: BUILDERS
impl DisclosureButton {
    /// Create a new [`DisclosureButton`] with
    /// an initial disclosed state.
    #[must_use = "Use the returned value."]
    pub const fn new(is_disclosed: bool) -> Self {
        Self {
            is_disclosed,
            activation: Activation::new(),
            label: None,
        }
    }

    /// Associates a separate accessible label with this disclosure control.
    #[must_use = "Use the configured disclosure button."]
    pub const fn with_label(mut self, label: WidgetId) -> Self {
        self.label = Some(label);
        self
    }

    /// Get the disclosed state of the button.
    #[inline]
    #[must_use = "Use the returned value."]
    pub const fn is_disclosed(&self) -> bool {
        self.is_disclosed
    }
}

// --- MARK: WIDGETMUT
impl DisclosureButton {
    /// Change the disclosed state of the button.
    pub fn set_disclosed(this: &mut WidgetMut<'_, Self>, is_disclosed: bool) {
        if this.widget.is_disclosed != is_disclosed {
            this.widget.is_disclosed = is_disclosed;
            this.ctx.request_layout();
        }
    }

    fn activate(&mut self, ctx: &mut EventCtx<'_>) {
        self.is_disclosed = !self.is_disclosed;
        ctx.request_layout();
    }
}

// --- MARK: IMPL WIDGET
impl Widget for DisclosureButton {
    type Action = NoAction;

    super::selection_control::activation_methods!(@pointer_and_keyboard ActivationKeys::SpaceOrEnter);

    fn on_access_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &AccessEvent,
    ) {
        if ctx.target() != ctx.widget_id() || ctx.is_disabled() || ctx.is_stashed() {
            return;
        }
        match event.action {
            accesskit::Action::Click => self.activate(ctx),
            accesskit::Action::Expand if !self.is_disclosed => self.activate(ctx),
            accesskit::Action::Collapse if self.is_disclosed => self.activate(ctx),
            _ => return,
        }
        ctx.set_handled();
    }

    fn register_children(&mut self, _ctx: &mut RegisterCtx<'_>) {}

    fn property_changed(&mut self, ctx: &mut UpdateCtx<'_>, property_type: TypeId) {
        ContentColor::prop_changed(ctx, property_type);
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        self.activation.update(event);
        if matches!(event, Update::FocusChanged(_)) {
            ctx.request_paint_only();
        }
    }

    fn measure(
        &mut self,
        _ctx: &mut MeasureCtx<'_>,
        _props: &PropertiesRef<'_>,
        _axis: Axis,
        len_req: LenReq,
        _cross_length: Option<Length>,
    ) -> Length {
        let length = DEFAULT_LENGTH;

        match len_req {
            LenReq::Min | LenReq::Max => length,
            LenReq::Fit(space) => length.min(space),
        }
    }

    fn layout(&mut self, _ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, _size: Size) {}

    fn paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        let cache = ctx.property_cache();
        let button_color = props.get::<ContentColor>(cache);

        let content_box = ctx.content_box();
        let size = content_box.size();
        let half_size = size * 0.5;

        let mut arrow = BezPath::new();
        arrow.move_to((0.0, -half_size.height));
        arrow.line_to((half_size.width, 0.0));
        arrow.line_to((0.0, half_size.height));

        let mut affine = Affine::translate(content_box.center().to_vec2());

        // Rotate if it's disclosed
        if self.is_disclosed() {
            affine = affine.pre_rotate(FRAC_PI_2);
        }

        painter
            .stroke(
                arrow,
                &Stroke::new(2.0).with_join(Join::Miter),
                button_color.color,
            )
            .transform(affine)
            .draw();

        if ctx.is_focus_target() {
            // TODO: Perhaps change the color of the arrow instead?
            let rect = ctx.border_box().to_rounded_rect(2.0);
            painter
                .stroke(rect, &Stroke::new(1.0), BrushRef::Solid(LIGHT_BLUE))
                .draw();
        }
    }

    fn accessibility_role(&self) -> accesskit::Role {
        accesskit::Role::DisclosureTriangle
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut accesskit::Node,
    ) {
        if let Some(label) = self.label {
            node.push_labelled_by(label.into());
        }
        node.set_expanded(self.is_disclosed);
        node.add_action(accesskit::Action::Click);
        node.add_action(if self.is_disclosed {
            accesskit::Action::Collapse
        } else {
            accesskit::Action::Expand
        });
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::new()
    }

    fn propagates_pointer_interaction(&self) -> bool {
        false
    }

    fn accepts_focus(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::keyboard::{Key, NamedKey};
    use crate::core::{PointerButton, TextEvent};
    use crate::testing::TestHarness;
    use crate::theme::test_property_set;

    #[test]
    fn activation_and_accessibility_follow_disclosure_state() {
        use accesskit::{Action, ActionRequest, TreeId};
        let mut harness =
            TestHarness::create(test_property_set(), DisclosureButton::new(false).prepare());
        let id = harness.root_id();
        let check = |harness: &mut TestHarness<DisclosureButton>, expanded| {
            assert_eq!(harness.root_widget().unwrap().is_disclosed(), expanded);
            let _ = harness.redraw();
            let node = harness.access_node(id).unwrap();
            assert_eq!(node.data().is_expanded(), Some(expanded));
            assert_eq!(node.data().supports_action(Action::Expand), !expanded);
            assert_eq!(node.data().supports_action(Action::Collapse), expanded);
        };
        check(&mut harness, false);
        harness
            .mouse_click_on(id, Some(PointerButton::Secondary))
            .unwrap();
        check(&mut harness, false);
        harness.focus_on(Some(id));
        let enter = Key::Named(NamedKey::Enter);
        harness.process_text_event(&TextEvent::key_up(enter.clone()));
        check(&mut harness, false);
        harness.process_text_event(&TextEvent::key_down(enter.clone()));
        harness.process_text_event(&TextEvent::key_up(enter));
        check(&mut harness, true);
        let request = |action| ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: id.into(),
            data: None,
        };
        harness.process_access_event(request(Action::Expand));
        check(&mut harness, true);
        harness.process_access_event(request(Action::Collapse));
        check(&mut harness, false);
        harness
            .mouse_click_on(id, Some(PointerButton::Primary))
            .unwrap();
        check(&mut harness, true);
        harness
            .edit_root_widget(|mut button| {
                button.ctx.set_disabled(true);
            })
            .unwrap();
        harness.process_access_event(request(Action::Collapse));
        check(&mut harness, true);
        harness
            .edit_root_widget(|mut button| {
                DisclosureButton::set_disclosed(&mut button, false);
            })
            .unwrap();
        check(&mut harness, false);
    }
}
