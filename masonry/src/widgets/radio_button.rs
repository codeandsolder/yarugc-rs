// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;

use accesskit::{Node, Role, Toggled};
use include_doc_path::include_doc_path;
use tracing::{Span, trace, trace_span};

use crate::core::{
    AccessCtx, AccessEvent, ArcStr, ChildrenIds, EventCtx, LayoutCtx, MeasureCtx, NewWidget,
    PaintCtx, PointerButton, PointerButtonEvent, PointerEvent, PropertiesMut, PropertiesRef,
    RegisterCtx, TextEvent, Update, UpdateCtx, UsesProperty, Widget, WidgetId, WidgetMut,
    WidgetPod, keyboard::Key,
};
use crate::imaging::Painter;
use crate::kurbo::{Axis, Circle, Size, Stroke};
use crate::layout::{LenReq, Length};
use crate::properties::{BorderColor, BorderWidth, CheckmarkColor};
use crate::theme;
use crate::widgets::{Label, RadioGroup};

/// A radio button that can be toggled.
///
#[doc = concat!(
    "![Radio button with checked state](",
    include_doc_path!("screenshots/radio_button_hello_checked.png"),
    ")",
)]
///
/// Emits [`RadioButtonSelected`] when an unchecked button is activated.
/// A button outside a group behaves as an independent selection control.
pub struct RadioButton {
    selected: bool,
    // FIXME - Remove label child, have this widget only be a box with a checkmark.
    label: WidgetPod<Label>,
    parent_group: Option<WidgetId>,
}

impl RadioButton {
    /// Create a new `RadioButton` with a text label.
    pub fn new(checked: bool, text: impl Into<ArcStr>) -> Self {
        Self {
            selected: checked,
            label: WidgetPod::new(Label::new(text)),
            parent_group: None,
        }
    }

    /// Create a new `RadioButton` with the given label.
    #[must_use = "Use the returned value."]
    pub fn from_label(checked: bool, label: NewWidget<Label>) -> Self {
        Self {
            selected: checked,
            label: label.into_pod(),
            parent_group: None,
        }
    }
}

// --- MARK: WIDGETMUT
impl RadioButton {
    /// Returns the current selection state.
    #[must_use = "Inspect whether the radio button is checked."]
    pub const fn is_checked(&self) -> bool {
        self.selected
    }

    /// Check or uncheck the button.
    ///
    /// Within a group, selection changes are applied in submission order
    /// during the mutation pass. Selecting a button deselects the previous one.
    pub fn set_checked(this: &mut WidgetMut<'_, Self>, checked: bool) {
        if let Some(parent_id) = this.widget.parent_group {
            let self_id = this.id();
            this.ctx
                .mutate_later::<RadioGroup>(parent_id, move |group| {
                    RadioGroup::apply_selection(group, self_id, checked);
                });
        } else {
            Self::apply_checked(this, checked);
        }
    }

    pub(super) fn apply_checked(this: &mut WidgetMut<'_, Self>, checked: bool) {
        if this.widget.selected != checked {
            this.widget.selected = checked;
            this.ctx.request_render();
        }
    }

    pub(super) const fn group_id(&self) -> Option<WidgetId> {
        self.parent_group
    }

    label_text_accessors!();
}

impl UsesProperty<CheckmarkColor> for RadioButton {}

/// The action type emitted by [`RadioButton`] when it is selected.
///
/// There is no equivalent action for deselection.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct RadioButtonSelected;

// --- MARK: HELPERS
impl RadioButton {
    fn select(&mut self, ctx: &mut EventCtx<'_>) {
        ctx.set_handled();
        if self.selected {
            return;
        }
        if let Some(parent_id) = self.parent_group {
            let self_id = ctx.widget_id();
            ctx.mutate_later::<RadioGroup>(parent_id, move |group| {
                RadioGroup::apply_selection(group, self_id, true);
            });
        } else {
            self.selected = true;
            ctx.request_render();
        }
        ctx.submit_action::<RadioButtonSelected>(RadioButtonSelected);
    }
}

// --- MARK: IMPL WIDGET
impl Widget for RadioButton {
    type Action = RadioButtonSelected;

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        match event {
            PointerEvent::Down(PointerButtonEvent {
                button: None | Some(PointerButton::Primary),
                ..
            }) => {
                ctx.request_focus();
                ctx.capture_pointer();
                ctx.set_handled();
                trace!("RadioButton {:?} pressed", ctx.widget_id());
            }
            PointerEvent::Up(PointerButtonEvent {
                button: None | Some(PointerButton::Primary),
                ..
            }) if ctx.is_active() && ctx.is_hovered() => {
                trace!("RadioButton {:?} released", ctx.widget_id());
                self.select(ctx);
            }
            _ => (),
        }
    }

    fn on_text_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &TextEvent,
    ) {
        match event {
            TextEvent::Keyboard(event) if event.state.is_up() => {
                if matches!(&event.key, Key::Character(c) if c == " ") {
                    self.select(ctx);
                }
            }
            _ => (),
        }
    }

    fn propagates_pointer_interaction(&self) -> bool {
        false
    }

    fn accepts_focus(&self) -> bool {
        // RadioButton can be tab-focused...
        true
    }

    fn on_access_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &AccessEvent,
    ) {
        match event.action {
            accesskit::Action::Click => {
                self.select(ctx);
            }
            _ => {}
        }
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        match event {
            Update::WidgetAdded => {
                self.parent_group = ctx.nearest_ancestor::<RadioGroup>().map(|(_, id)| id);
                if let Some(parent_id) = self.parent_group
                    && self.selected
                {
                    let self_id = ctx.widget_id();
                    ctx.mutate_later::<RadioGroup>(parent_id, move |group| {
                        RadioGroup::apply_selection(group, self_id, true);
                    });
                }
            }
            Update::HoveredChanged(_)
            | Update::ActiveChanged(_)
            | Update::FocusChanged(_)
            | Update::DisabledChanged(_) => {
                ctx.request_paint_only();
            }

            _ => {}
        }
    }

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        ctx.register_child(&mut self.label);
    }

    fn property_changed(&mut self, ctx: &mut UpdateCtx<'_>, property_type: TypeId) {
        CheckmarkColor::prop_changed(ctx, property_type);
    }

    super::selection_control::control_methods!();

    fn paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        let cache = ctx.property_cache();
        let border_color = *props.get::<BorderColor>(cache);
        let border_width = *props.get::<BorderWidth>(cache);
        let brush = *props.get::<CheckmarkColor>(cache);

        let side = theme::BASIC_WIDGET_HEIGHT.get();
        let check_size = Size::new(side, side);

        let border_circle = Circle::new(
            check_size.to_rect().center(),
            (side - border_width.width.get()) * 0.5,
        );

        // Paint the radio button border
        let border_stroke = Stroke::new(border_width.width.get());
        painter
            .stroke(border_circle, &border_stroke, border_color.color)
            .draw();

        // Paint the checkmark if checked
        if self.selected {
            // TODO: Create a prop for ellipse size. Default: 50% of border size
            let check_circle = Circle::new(check_size.to_rect().center(), side * 0.25);
            painter.fill(check_circle, brush.color).draw();
        }
    }

    fn accessibility_role(&self) -> Role {
        Role::RadioButton
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        node.add_action(accesskit::Action::Click);
        if self.selected {
            node.set_toggled(Toggled::True);
        } else {
            node.set_toggled(Toggled::False);
        }
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::from_slice(&[self.label.id()])
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("RadioButton", id = id.trace())
    }

    fn get_debug_text(&self) -> Option<String> {
        if self.selected {
            Some("(X)".to_string())
        } else {
            Some("( )".to_string())
        }
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{CollectionWidget, StyleProperty, WidgetTag};
    use crate::properties::ContentColor;
    use crate::testing::{TestHarness, assert_render_snapshot};
    use crate::theme::{ACCENT_COLOR, default_property_set};
    use crate::widgets::Flex;

    fn check_state(harness: &mut TestHarness<RadioGroup>, ids: &[WidgetId], expected: &[bool]) {
        let _ = harness.redraw();
        for (&id, &checked) in ids.iter().zip(expected) {
            let node = harness.access_node(id).unwrap();
            assert_eq!(
                node.data().toggled(),
                Some(if checked {
                    Toggled::True
                } else {
                    Toggled::False
                })
            );
            let button = harness.get_widget_with_id(id).unwrap();
            assert_eq!(
                button.downcast::<RadioButton>().unwrap().is_checked(),
                checked
            );
        }
        assert!(harness.pop_action_erased().is_none());
    }

    #[test]
    fn initial_and_batched_selection_are_exclusive() {
        let a = RadioButton::new(true, "A").prepare();
        let b = RadioButton::new(true, "B").prepare();
        let ids = [a.id(), b.id()];
        let group = RadioGroup::new(Flex::row().with_fixed(a).with_fixed(b).prepare()).prepare();
        let mut harness = TestHarness::create_with_size(default_property_set(), group, (200, 40));
        check_state(&mut harness, &ids, &[false, true]);

        harness
            .edit_root_widget(|mut group| {
                let mut row = RadioGroup::child_mut::<Flex>(&mut group).unwrap();
                for id in [ids[0], ids[1], ids[0]] {
                    let mut button = row.ctx.get_mut::<RadioButton>(id).unwrap();
                    RadioButton::set_checked(&mut button, true);
                }
            })
            .unwrap();
        check_state(&mut harness, &ids, &[true, false]);

        // Selecting a removed child must not clear the live selection.
        harness
            .edit_root_widget(|mut group| {
                let mut row = RadioGroup::child_mut::<Flex>(&mut group).unwrap();
                {
                    let mut b = row.ctx.get_mut::<RadioButton>(ids[1]).unwrap();
                    RadioButton::set_checked(&mut b, true);
                }
                Flex::remove(&mut row, 1).unwrap();
            })
            .unwrap();
        check_state(&mut harness, &ids[..1], &[true]);
    }

    #[test]
    fn initially_selected_button_is_unchecked_when_another_is_activated() {
        let a = RadioButton::new(true, "A").prepare();
        let b = RadioButton::new(false, "B").prepare();
        let ids = [a.id(), b.id()];
        let group = RadioGroup::new(Flex::row().with_fixed(a).with_fixed(b).prepare()).prepare();
        let mut harness = TestHarness::create_with_size(default_property_set(), group, (200, 40));
        check_state(&mut harness, &ids, &[true, false]);

        harness
            .mouse_click_on(ids[1], Some(PointerButton::Secondary))
            .unwrap();
        check_state(&mut harness, &ids, &[true, false]);
        harness.accessibility_click_on(ids[1]);
        assert_eq!(
            harness.pop_action::<RadioButtonSelected>().unwrap(),
            Some((RadioButtonSelected, ids[1]))
        );
        check_state(&mut harness, &ids, &[false, true]);
        harness.accessibility_click_on(ids[1]);
        check_state(&mut harness, &ids, &[false, true]);
    }

    #[test]
    fn simple_radio_button() {
        let radio_tag = WidgetTag::unique();
        let widget = NewWidget::new(RadioButton::new(false, "Hello")).with_tag(radio_tag);
        let widget = NewWidget::new(RadioGroup::new(widget));

        let mut harness = TestHarness::create_with_size(default_property_set(), widget, (100, 40));
        let radio_id = harness.get_widget(radio_tag).unwrap().id();

        assert_render_snapshot!(harness, "radio_button_hello_unchecked");

        assert!(harness.pop_action_erased().is_none());

        harness.mouse_move_to(radio_id).unwrap();

        assert_render_snapshot!(harness, "radio_button_hello_hovered");

        harness.mouse_click_on(radio_id, None).unwrap();
        assert_eq!(
            harness.pop_action::<RadioButtonSelected>().unwrap(),
            Some((RadioButtonSelected, radio_id))
        );

        assert_render_snapshot!(harness, "radio_button_hello_checked");

        harness
            .edit_root_widget(|mut group| {
                let mut radio = RadioGroup::child_mut::<RadioButton>(&mut group).unwrap();
                RadioButton::set_checked(&mut radio, false);
            })
            .unwrap();

        // The radio button should be unchecked again, but still hovered
        assert_render_snapshot!(harness, "radio_button_hello_hovered");

        harness.focus_on(None);
        harness.press_tab_key(false);
        assert_eq!(harness.focused_widget().map(|w| w.id()), Some(radio_id));
    }

    #[test]
    fn radio_button_focus_indicator() {
        use crate::properties::types::MainAxisAlignment;

        let radio = NewWidget::new(RadioButton::new(true, "Focus test"));
        let radio_id = radio.id();
        let group = NewWidget::new(RadioGroup::new(radio));

        let root = NewWidget::new(
            Flex::row()
                .with_fixed(group)
                .main_axis_alignment(MainAxisAlignment::Center),
        );
        let mut harness = TestHarness::create_with_size(default_property_set(), root, (120, 40));

        harness.focus_on(Some(radio_id));
        assert_render_snapshot!(harness, "radio_button_focus_focused");
    }

    #[test]
    fn edit_radio_button() {
        let image_1 = {
            let label = Label::new("The quick brown fox jumps over the lazy dog")
                .with_style(StyleProperty::FontSize(20.0));
            let label = NewWidget::new(label).with_props(ContentColor::new(ACCENT_COLOR));
            let radio = NewWidget::new(RadioButton::from_label(true, label));
            let group = NewWidget::new(RadioGroup::new(radio));

            let mut harness =
                TestHarness::create_with_size(default_property_set(), group, (50, 50));

            harness.render().unwrap()
        };

        let image_2 = {
            let radio = NewWidget::new(RadioButton::new(false, "Hello world"));
            let group = NewWidget::new(RadioGroup::new(radio));

            let mut harness =
                TestHarness::create_with_size(default_property_set(), group, (50, 50));

            harness
                .edit_root_widget(|mut group| {
                    let mut radio = RadioGroup::child_mut::<RadioButton>(&mut group).unwrap();
                    RadioButton::set_checked(&mut radio, true);
                    RadioButton::set_text(
                        &mut radio,
                        ArcStr::from("The quick brown fox jumps over the lazy dog"),
                    );

                    let mut label = RadioButton::label_mut(&mut radio).unwrap();
                    label.insert_prop(ContentColor::new(ACCENT_COLOR));
                    Label::insert_style(&mut label, StyleProperty::FontSize(20.0));
                })
                .unwrap();

            harness.render().unwrap()
        };

        // We don't use assert_eq because we don't want rich assert
        assert!(image_1 == image_2);
    }
}
