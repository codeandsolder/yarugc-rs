// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;

use crate::core::{
    AccessCtx, ArcStr, ChildrenIds, LayoutCtx, MeasureCtx, NewWidget, NoAction, PaintCtx,
    PropertiesRef, RegisterCtx, UpdateCtx, Widget, WidgetId, WidgetMut, WidgetPod,
};
use crate::imaging::Painter;
use crate::kurbo::{Axis, Line, Point, Size, Stroke};
use crate::layout::{AsUnit, LayoutSize, LenDef, LenReq, Length, SizeDef};
use crate::properties::{BorderColor, BorderWidth, Dimensions, Padding};
use crate::widgets::{DisclosureButton, Label};
use crate::{accesskit, theme};

/// Square disclosure button length.
const BUTTON_LENGTH: Length = crate::layout::length!(16.);
/// Padding around the separator line.
const SEPARATOR_PAD: Padding = Padding {
    top: crate::layout::length!(4.),
    left: crate::layout::length!(1.),
    right: crate::layout::length!(1.),
    bottom: Length::ZERO,
};

/// A collapsible panel with a header that contains a child widget.
pub struct CollapsePanel {
    disclosure_button: WidgetPod<DisclosureButton>,
    header_label: WidgetPod<Label>,
    /// The y location of the separator line.
    ///
    /// If it's [`None`], no line will be rendered.
    separator_line_y: Option<f64>,
    child: WidgetPod<dyn Widget>,
}

impl CollapsePanel {
    /// Create a new [`CollapsePanel`] with a header text and a child widget.
    pub fn new(
        collapse: bool,
        header_text: impl Into<ArcStr>,
        child: NewWidget<impl Widget + ?Sized>,
    ) -> Self {
        Self::from_label(collapse, Label::new(header_text).prepare(), child)
    }

    /// Create a new [`CollapsePanel`] with a header label widget and a child widget.
    #[must_use = "Use the returned value."]
    pub fn from_label(
        collapse: bool,
        header_label: NewWidget<Label>,
        child: NewWidget<impl Widget + ?Sized>,
    ) -> Self {
        Self {
            disclosure_button: Self::disclosure_button(collapse, header_label.id()),
            header_label: header_label.into_pod(),
            separator_line_y: None,
            child: child.erased().into_pod(),
        }
    }

    fn disclosure_button(collapse: bool, label: WidgetId) -> WidgetPod<DisclosureButton> {
        DisclosureButton::new(!collapse)
            .with_label(label)
            .prepare()
            .with_props(
                // TODO - Move to DefaultProperties
                Dimensions::fixed(BUTTON_LENGTH, BUTTON_LENGTH),
            )
            .into_pod()
    }
}

// --- MARK: WIDGETMUT
impl CollapsePanel {
    child_accessors!(child, set_child, child_mut, erased);

    /// Set whether or not the panel is collapsed.
    pub fn set_collapsed(this: &mut WidgetMut<'_, Self>, collapsed: bool) {
        if let Some(mut child) = Self::disclosure_button_mut(this) {
            DisclosureButton::set_disclosed(&mut child, !collapsed);
        }
    }

    /// Set the text.
    ///
    /// We enforce this to be an `ArcStr` to make the allocation explicit.
    pub fn set_text(this: &mut WidgetMut<'_, Self>, new_text: ArcStr) {
        if let Some(mut child) = Self::header_label_mut(this) {
            Label::set_text(&mut child, new_text);
        }
    }

    /// Get a mutable reference to the disclosure button.
    pub fn disclosure_button_mut<'t>(
        this: &'t mut WidgetMut<'_, Self>,
    ) -> Option<WidgetMut<'t, DisclosureButton>> {
        this.ctx.get_mut(this.widget.disclosure_button.id())
    }

    /// Get a mutable reference to the label.
    pub fn header_label_mut<'t>(this: &'t mut WidgetMut<'_, Self>) -> Option<WidgetMut<'t, Label>> {
        this.ctx.get_mut(this.widget.header_label.id())
    }
}

// --- MARK: IMPL WIDGET
impl Widget for CollapsePanel {
    type Action = NoAction;

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        ctx.register_child(&mut self.disclosure_button);
        ctx.register_child(&mut self.header_label);
        ctx.register_child(&mut self.child);
    }

    fn property_changed(&mut self, _ctx: &mut UpdateCtx<'_>, _property_type: TypeId) {}

    fn measure(
        &mut self,
        ctx: &mut MeasureCtx<'_>,
        props: &PropertiesRef<'_>,
        axis: Axis,
        len_req: LenReq,
        cross_length: Option<Length>,
    ) -> Length {
        let cache = ctx.property_cache();
        let border = props.get::<BorderWidth>(cache);
        let header_x_padding = theme::WIDGET_CONTROL_COMPONENT_PADDING;

        let header_x_padding_length = header_x_padding.saturating_add(header_x_padding);
        let btn_length = BUTTON_LENGTH;

        let separator_height = border
            .width
            .saturating_add(SEPARATOR_PAD.length(Axis::Vertical));

        let space: LenDef = len_req.into();

        let cross = axis.cross();
        let label_cross_space = match cross {
            // If we know the horizontal space, then we can derive the label's horizontal space.
            Axis::Horizontal => cross_length.map(|cross_length| {
                cross_length
                    .saturating_sub(header_x_padding_length)
                    .saturating_sub(btn_length)
            }),
            // Even if we know our vertical space, we don't know the child's height.
            // So we can't provide an accurate height for the label.
            Axis::Vertical => None,
        };
        // We don't give any special context to the label, just our full size
        let label_context_size = LayoutSize::maybe(cross, cross_length);
        let label_auto_length = match axis {
            Axis::Horizontal => space.reduce(header_x_padding_length.saturating_add(btn_length)),
            Axis::Vertical => space,
        };
        let label_length = ctx.compute_length(
            &self.header_label,
            label_auto_length,
            label_context_size,
            axis,
            label_cross_space,
        );

        let header_length = match axis {
            Axis::Horizontal => btn_length
                .saturating_add(label_length)
                .saturating_add(header_x_padding_length),
            Axis::Vertical => btn_length.max(label_length),
        };

        // Collapsed = !Disclosed
        let is_collapsed = ctx
            .get_child(&self.disclosure_button)
            .is_none_or(|button| !button.is_disclosed());

        let child_length = if is_collapsed {
            Length::ZERO
        } else {
            let child_cross_space = match cross {
                // If we know the horizontal space, then that is also the child's horizontal space.
                Axis::Horizontal => cross_length,
                // Even if we know our vertical space, we don't know the header's height.
                // So we can't provide an accurate height for the child.
                Axis::Vertical => None,
            };
            // Child's context size has the same restrictions as child's cross space.
            let child_context_size = LayoutSize::maybe(cross, child_cross_space);
            let child_auto_length = match axis {
                Axis::Horizontal => space,
                Axis::Vertical => space.reduce(header_length.saturating_add(separator_height)),
            };
            ctx.compute_length(
                &self.child,
                child_auto_length,
                child_context_size,
                axis,
                child_cross_space,
            )
        };

        match axis {
            Axis::Horizontal => header_length.max(child_length),
            Axis::Vertical => {
                let mut length = header_length;
                if !is_collapsed {
                    length = length
                        .saturating_add(child_length)
                        .saturating_add(separator_height);
                }
                length
            }
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, props: &PropertiesRef<'_>, size: Size) {
        let cache = ctx.property_cache();
        let border = props.get::<BorderWidth>(cache);
        let header_x_padding = theme::WIDGET_CONTROL_COMPONENT_PADDING;

        let border_width = border.width.get();
        let separator_height = border_width + SEPARATOR_PAD.length(Axis::Vertical).get();

        let button_width = BUTTON_LENGTH.get();
        let header_padding_width = header_x_padding.get();

        // Square button
        let button_size = Size::new(button_width, button_width);
        ctx.run_layout(&self.disclosure_button, button_size);

        let label_auto_size = SizeDef::new(
            LenDef::FitContent(
                (header_padding_width.mul_add(-2., size.width) - button_size.width)
                    .max(0.)
                    .px(),
            ),
            LenDef::FitContent(size.height.px()),
        );
        let label_size = ctx.compute_size(&self.header_label, label_auto_size, size.into());

        ctx.run_layout(&self.header_label, label_size);

        let header_height = button_size.height.max(label_size.height);

        // Center both children in the header, including labels shorter than the button.
        let btn_origin = Point::new(
            header_padding_width,
            (header_height - button_size.height) * 0.5,
        );
        ctx.place_child(&self.disclosure_button, btn_origin);

        let label_origin = Point::new(
            header_padding_width.mul_add(2.0, button_size.width),
            (header_height - label_size.height) * 0.5,
        );
        ctx.place_child(&self.header_label, label_origin);

        // Collapsed = !Disclosed
        let is_collapsed = ctx
            .get_child(&self.disclosure_button)
            .is_none_or(|button| !button.is_disclosed());

        // Only render child if it's not collapsed.
        ctx.set_stashed(&self.child, is_collapsed);

        if is_collapsed {
            self.separator_line_y = None;
        } else {
            let child_space = Size::new(
                size.width,
                (size.height - header_height - separator_height).max(0.),
            );

            let child_auto_size = SizeDef::fit(child_space);
            let child_context_size = child_space.into();

            let child_size = ctx.compute_size(&self.child, child_auto_size, child_context_size);

            ctx.run_layout(&self.child, child_size);

            let child_origin = Point::new(0.0, header_height + separator_height);
            ctx.place_child(&self.child, child_origin);

            self.separator_line_y =
                Some(border_width.mul_add(0.5, header_height + SEPARATOR_PAD.top.get()));
        }

        ctx.derive_baselines(&self.header_label);
    }

    fn paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        if let Some(y) = self.separator_line_y {
            let cache = ctx.property_cache();
            let border_width = *props.get::<BorderWidth>(cache);
            let border_color = *props.get::<BorderColor>(cache);

            let border_box = ctx.border_box();

            // Only paint the line if it would have a positive width
            if SEPARATOR_PAD.length(Axis::Horizontal).get() < border_box.width() {
                let x1 = border_box.x0 + SEPARATOR_PAD.left.get();
                let x2 = border_box.x1 - SEPARATOR_PAD.right.get();
                let line = Line::new((x1, y), (x2, y));
                painter
                    .stroke(
                        line,
                        &Stroke::new(border_width.width.get()),
                        border_color.color,
                    )
                    .draw();
            }
        }
    }

    fn accessibility_role(&self) -> accesskit::Role {
        accesskit::Role::GenericContainer
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        _node: &mut accesskit::Node,
    ) {
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::from_slice(&[
            self.disclosure_button.id(),
            self.header_label.id(),
            self.child.id(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestHarness;
    use crate::theme::test_property_set;
    use crate::widgets::SizedBox;

    #[test]
    fn disclosure_centers_header_and_updates_retained_child_visibility() {
        let header = Label::new("Panel")
            .prepare()
            .with_props(Dimensions::fixed(40.px(), 4.px()));
        let child = SizedBox::empty().prepare();
        let child_id = child.id();
        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            CollapsePanel::from_label(true, header, child).prepare(),
            (160, 80),
        );
        let children = harness.root_widget().unwrap().children();
        let button_id = children[0].id();
        let label_id = children[1].id();
        let button_rect = children[0]
            .ctx()
            .window_transform()
            .transform_rect_bbox(children[0].ctx().border_box());
        let label_rect = children[1]
            .ctx()
            .window_transform()
            .transform_rect_bbox(children[1].ctx().border_box());
        assert!((button_rect.center().y - label_rect.center().y).abs() < 1e-8);
        assert!(children[2].ctx().is_stashed());
        let button = harness.access_node(button_id).unwrap();
        assert_eq!(
            button.data().labelled_by(),
            &[accesskit::NodeId::from(label_id)]
        );
        harness
            .mouse_click_on(button_id, Some(crate::core::PointerButton::Primary))
            .unwrap();
        assert!(
            !harness
                .get_widget_with_id(child_id)
                .unwrap()
                .ctx()
                .is_stashed()
        );
        harness
            .edit_root_widget(|mut panel| CollapsePanel::set_collapsed(&mut panel, true))
            .unwrap();
        assert!(
            harness
                .get_widget_with_id(child_id)
                .unwrap()
                .ctx()
                .is_stashed()
        );
    }
}
