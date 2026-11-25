// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use accesskit::{Node, Role};
use tracing::{Span, trace_span};

use super::selection_control::Activation;
use crate::core::keyboard::{Key, NamedKey};
use crate::core::{
    AccessCtx, AccessEvent, EventCtx, LayerType, PointerButton, PointerButtonEvent, PointerEvent,
    PropertiesMut, PropertiesRef, TextEvent, Update, UpdateCtx, Widget, WidgetId, WidgetMut,
    WidgetPod,
};
use crate::kurbo::Point;
use crate::layers::SelectorMenu;
use crate::layout::{AsUnit, UnitPoint};
use crate::theme;
use crate::widgets::Label;

/// A selector which displays a list of options when you click it.
///
/// This is called a "combo box" in some frameworks.
pub struct Selector {
    options: Vec<String>,
    selected_option: Option<usize>,
    child: WidgetPod<Label>,
    activation: Activation,
}

/// The requested option index does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidSelection {
    /// Requested index.
    pub index: usize,
    /// Number of available options.
    pub len: usize,
}

impl std::fmt::Display for InvalidSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "selector index {} outside {} options",
            self.index, self.len
        )
    }
}

impl std::error::Error for InvalidSelection {}

// --- MARK: BUILDERS
impl Selector {
    /// Creates a selector. An empty list has no selected option and cannot open a menu.
    pub fn new(options: Vec<String>) -> Self {
        let selected_option = (!options.is_empty()).then_some(0);
        let text = options.first().cloned().unwrap_or_default();
        Self {
            options,
            selected_option,
            child: WidgetPod::new(Label::new(text)),
            activation: Activation::new(),
        }
    }

    /// Builder method to pre-set the widget's initial option.
    ///
    /// # Errors
    ///
    /// Returns an error if the index is outside the list.
    pub fn with_selected_option(
        mut self,
        selected_option: usize,
    ) -> Result<Self, InvalidSelection> {
        self.option(selected_option)?;
        self.selected_option = Some(selected_option);
        Ok(self)
    }

    /// The selected option index, or None for an empty list.
    #[must_use = "Inspect the selected option."]
    pub const fn selected_option(&self) -> Option<usize> {
        self.selected_option
    }

    fn option(&self, index: usize) -> Result<&str, InvalidSelection> {
        self.options
            .get(index)
            .map(String::as_str)
            .ok_or(InvalidSelection {
                index,
                len: self.options.len(),
            })
    }

    fn update_label(this: &mut WidgetMut<'_, Self>) {
        let text = this
            .widget
            .selected_option
            .and_then(|idx| this.widget.options.get(idx))
            .cloned()
            .unwrap_or_default();
        if let Some(mut child) = Self::child_mut(this) {
            Label::set_text(&mut child, text);
        }
        this.ctx.request_accessibility_update();
    }
}

// --- MARK: WIDGETMUT
impl Selector {
    /// Replaces the list of options, selecting the first if present.
    pub fn set_options(this: &mut WidgetMut<'_, Self>, options: Vec<String>) {
        if let Some(id) = this.ctx.get_attached_layer::<SelectorMenu>() {
            this.ctx.remove_layer(id);
        }
        this.widget.selected_option = (!options.is_empty()).then_some(0);
        this.widget.options = options;
        Self::update_label(this);
    }

    /// Selects the given option, closing any open menu.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the selection if the index is outside the list.
    pub fn select_option(
        this: &mut WidgetMut<'_, Self>,
        selected_option: usize,
    ) -> Result<(), InvalidSelection> {
        this.widget.option(selected_option)?;
        if let Some(id) = this.ctx.get_attached_layer::<SelectorMenu>() {
            this.ctx.remove_layer(id);
        }
        this.widget.selected_option = Some(selected_option);
        Self::update_label(this);
        Ok(())
    }

    pub(crate) fn pick_from_menu(this: &mut WidgetMut<'_, Self>, menu: WidgetId, index: usize) {
        if this.ctx.get_attached_layer::<SelectorMenu>() != Some(menu)
            || this.ctx.is_disabled()
            || this.ctx.is_stashed()
        {
            return;
        }
        let Ok(selected_content) = this.widget.option(index).map(str::to_owned) else {
            return;
        };
        if Self::select_option(this, index).is_ok() {
            this.ctx
                .submit_action::<SelectionChanged>(SelectionChanged {
                    selected_content,
                    index,
                });
        }
    }

    /// Returns a mutable reference to the child label.
    pub fn child_mut<'t>(this: &'t mut WidgetMut<'_, Self>) -> Option<WidgetMut<'t, Label>> {
        this.ctx.get_mut(this.widget.child.id())
    }
}

/// A [`Selector`]'s option was picked.
#[derive(PartialEq, Eq, Debug)]
pub struct SelectionChanged {
    /// The content of the picked selection.
    pub selected_content: String,
    /// The index of the picked selection.
    pub index: usize,
}

// --- MARK: HELPERS

impl Selector {
    fn toggle_selector_layer(&self, ctx: &mut EventCtx<'_>) {
        if let Some(id) = ctx.get_attached_layer::<SelectorMenu>() {
            ctx.remove_layer(id);
            return;
        }
        let Some(selected_option) = self.selected_option else {
            return;
        };
        let layer_type = LayerType::Selector {
            options: self.options.clone(),
            selected_option,
        };
        let menu = SelectorMenu::new(ctx.widget_id(), &self.options).prepare();
        let border = ctx.border_box();
        let position = ctx.local_to_window(Point::new(border.x0, border.y1));
        ctx.create_attached_layer(layer_type, menu, position);
    }
}

// --- MARK: IMPL WIDGET
impl Widget for Selector {
    type Action = SelectionChanged;

    // The standard control minimum keeps adjacent widgets aligned.
    masonry_core::single_child_container!(
        child,
        UnitPoint::CENTER,
        min = (
            theme::SELECTOR_MIN_WIDTH.px(),
            theme::BORDERED_WIDGET_HEIGHT.px()
        )
    );

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        // A click on an already open selector closes it on press. Do not
        // capture that press or its release would reopen the menu.
        if matches!(
            event,
            PointerEvent::Down(PointerButtonEvent {
                button: None | Some(PointerButton::Primary),
                ..
            })
        ) && let Some(id) = ctx.get_attached_layer::<SelectorMenu>()
        {
            ctx.remove_layer(id);
            ctx.set_handled();
        } else if self.selected_option.is_some() && Activation::pointer(ctx, event) {
            self.toggle_selector_layer(ctx);
        }
    }

    fn on_text_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &TextEvent,
    ) {
        if matches!(event, TextEvent::WindowFocusChange(false))
            && let Some(id) = ctx.get_attached_layer::<SelectorMenu>()
        {
            ctx.remove_layer(id);
        }
        if let TextEvent::Keyboard(event) = event
            && ctx.is_focus_target()
            && event.state.is_down()
            && !event.repeat
        {
            match event.key {
                Key::Named(NamedKey::Enter) if self.selected_option.is_some() => {
                    self.toggle_selector_layer(ctx);
                    ctx.set_handled();
                }
                Key::Named(NamedKey::Escape) => {
                    if let Some(id) = ctx.get_attached_layer::<SelectorMenu>() {
                        ctx.remove_layer(id);
                        ctx.set_handled();
                    }
                }
                _ => {}
            }
        }
        if self.activation.keyboard(ctx, event) && self.selected_option.is_some() {
            self.toggle_selector_layer(ctx);
        }
    }

    fn on_access_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &AccessEvent,
    ) {
        if self.selected_option.is_some() && Activation::access(ctx, event) {
            ctx.request_focus();
            self.toggle_selector_layer(ctx);
        }
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        self.activation.update(event);
        match event {
            Update::WidgetAdded => {
                ctx.mutate_self_later::<Self>(|mut selector| Self::update_label(&mut selector));
            }
            Update::DisabledChanged(true) | Update::StashedChanged(true) => {
                if let Some(id) = ctx.get_attached_layer::<SelectorMenu>() {
                    ctx.remove_layer(id);
                }
                ctx.request_paint_only();
            }
            Update::HoveredChanged(_)
            | Update::ActiveChanged(_)
            | Update::FocusChanged(_)
            | Update::DisabledChanged(false) => {
                ctx.request_paint_only();
            }
            _ => {}
        }
    }

    fn accessibility_role(&self) -> Role {
        Role::ComboBox
    }

    fn accessibility(
        &mut self,
        ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        if let Some(index) = self.selected_option
            && let Some(value) = self.options.get(index)
        {
            node.add_action(accesskit::Action::Click);
            node.set_value(value.clone());
        }
        let layer = ctx.get_attached_layer::<SelectorMenu>();
        node.set_expanded(layer.is_some());
        if let Some(layer) = layer {
            node.set_controls(vec![layer.into()]);
        }
    }

    fn propagates_pointer_interaction(&self) -> bool {
        false
    }

    fn accepts_focus(&self) -> bool {
        // Selectors can be tab-focused...
        true
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("Selector", id = id.trace())
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::NewWidget;
    use crate::testing::TestHarness;
    use crate::theme::test_property_set;

    fn attached<W: Widget + crate::core::FromDynWidget>(
        harness: &TestHarness<W>,
        id: WidgetId,
    ) -> Option<WidgetId> {
        harness
            .get_widget_with_id(id)?
            .ctx()
            .get_attached_layer::<SelectorMenu>()
    }

    #[test]
    fn empty_and_invalid_selections_never_create_phantom_options() {
        assert!(matches!(
            Selector::new(vec![]).with_selected_option(0),
            Err(InvalidSelection { index: 0, len: 0 })
        ));
        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            Selector::new(vec![]).prepare(),
            (200, 60),
        );
        let id = harness.root_id();
        harness.accessibility_click_on(id);
        assert_eq!(attached(&harness, id), None);
        assert_eq!(harness.root_widget().unwrap().selected_option(), None);
        harness
            .edit_root_widget(|mut selector| {
                assert_eq!(
                    Selector::select_option(&mut selector, 0),
                    Err(InvalidSelection { index: 0, len: 0 })
                );
                Selector::set_options(&mut selector, vec!["A".into(), "B".into()]);
                Selector::select_option(&mut selector, 1).unwrap();
                assert_eq!(
                    Selector::select_option(&mut selector, usize::MAX),
                    Err(InvalidSelection {
                        index: usize::MAX,
                        len: 2
                    })
                );
                assert_eq!(selector.widget.selected_option(), Some(1));
            })
            .unwrap();
        let _ = harness.redraw();
        assert_eq!(harness.access_node(id).unwrap().data().value(), Some("B"));
        harness
            .edit_root_widget(|mut selector| Selector::set_options(&mut selector, vec![]))
            .unwrap();
        let _ = harness.redraw();
        assert_eq!(harness.root_widget().unwrap().selected_option(), None);
        assert!(
            !harness
                .access_node(id)
                .unwrap()
                .data()
                .supports_action(accesskit::Action::Click)
        );
    }

    #[test]
    fn menu_accessibility_and_owner_lifecycle() {
        use crate::widgets::SizedBox;

        let selector = Selector::new(vec!["A".into(), "B".into()]).prepare();
        let id = selector.id();
        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            SizedBox::new(selector).prepare(),
            (200, 200),
        );
        harness.accessibility_click_on(id);
        let first = attached(&harness, id).unwrap();
        let item = harness.get_widget_with_id(first).unwrap().children()[1].id();
        let _ = harness.redraw();
        assert_eq!(
            harness.access_node(id).unwrap().data().is_expanded(),
            Some(true)
        );

        harness.accessibility_click_on(item);
        assert_eq!(
            harness.pop_action::<SelectionChanged>().unwrap(),
            Some((
                SelectionChanged {
                    selected_content: "B".into(),
                    index: 1
                },
                id
            ))
        );
        assert!(harness.get_widget_with_id(first).is_none());
        assert_eq!(attached(&harness, id), None);
        let _ = harness.redraw();
        assert_eq!(harness.access_node(id).unwrap().data().value(), Some("B"));
        assert_eq!(
            harness.access_node(id).unwrap().data().is_expanded(),
            Some(false)
        );

        harness.accessibility_click_on(id);
        let second = attached(&harness, id).unwrap();
        harness
            .edit_widget_with_id(id, |mut widget| {
                Selector::set_options(
                    &mut widget.try_downcast::<Selector>().unwrap(),
                    vec!["C".into()],
                );
            })
            .unwrap();
        assert!(harness.get_widget_with_id(second).is_none());
        harness.accessibility_click_on(item);
        assert!(harness.pop_action_erased().is_none());

        harness.accessibility_click_on(id);
        let third = attached(&harness, id).unwrap();
        harness
            .edit_widget_with_id(id, |mut widget| widget.ctx.set_disabled(true))
            .unwrap();
        assert!(harness.get_widget_with_id(third).is_none());
        harness
            .edit_widget_with_id(id, |mut widget| widget.ctx.set_disabled(false))
            .unwrap();

        harness.accessibility_click_on(id);
        let fourth = attached(&harness, id).unwrap();
        harness
            .edit_root_widget(|mut root| SizedBox::remove_child(&mut root))
            .unwrap();
        assert!(harness.get_widget_with_id(fourth).is_none());
    }

    #[test]
    fn menu_pointer_selection_requires_matching_press_and_release() {
        use crate::widgets::Flex;

        let selector = Selector::new(vec!["A".into(), "B".into()]).prepare();
        let id = selector.id();
        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            Flex::column().with_fixed(selector).prepare(),
            (200, 200),
        );
        harness.mouse_click_on(id, None).unwrap();
        let menu = attached(&harness, id).unwrap();
        let items = harness
            .get_widget_with_id(menu)
            .unwrap()
            .children()
            .iter()
            .map(|item| item.id())
            .collect::<Vec<_>>();
        harness.mouse_move_to(items[0]).unwrap();
        harness.mouse_button_press(None);
        harness.mouse_move_to(items[1]).unwrap();
        harness.mouse_button_release(None);
        assert!(harness.pop_action_erased().is_none());
        assert_eq!(attached(&harness, id), Some(menu));

        harness.mouse_click_on(items[1], None).unwrap();
        assert_eq!(
            harness.pop_action::<SelectionChanged>().unwrap(),
            Some((
                SelectionChanged {
                    selected_content: "B".into(),
                    index: 1
                },
                id
            ))
        );
        assert_eq!(attached(&harness, id), None);

        harness.mouse_click_on(id, None).unwrap();
        assert!(attached(&harness, id).is_some());
        harness.mouse_click_on(id, None).unwrap();
        assert_eq!(attached(&harness, id), None);
    }

    #[test]
    fn with_selected_option() {
        let options = vec!["Blue and black".into(), "White and gold".into()];
        let selector = Selector::new(options).with_selected_option(1).unwrap();
        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            NewWidget::new(selector),
            (200_u32, 60_u32),
        );

        harness
            .edit_root_widget(|mut selector| {
                let label = Selector::child_mut(&mut selector).unwrap();
                assert_eq!(&**label.widget.text(), "White and gold");
            })
            .unwrap();
    }
}
