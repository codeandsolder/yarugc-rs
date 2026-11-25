// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::assert_matches;

use accesskit::{ActionRequest, TreeId};

use crate::core::keyboard::{Key, NamedKey};
use crate::core::pointer::{
    PointerButton, PointerButtonEvent, PointerEvent, PointerInfo, PointerType,
};
use crate::core::{
    AccessEvent, NewWidget, PointerId, PointerState, PointerUpdate, TextEvent, Update, Widget,
    WidgetTag,
};
use crate::dpi::PhysicalPosition;
use crate::kurbo::Point;
use crate::layout::AsUnit;
use crate::testing::{
    ModularWidget, Record, TestHarness, TestWidgetExt, assert_any, assert_debug_panics, assert_none,
};
use crate::theme::test_property_set;
use crate::widgets::{Button, ButtonPress, Flex, SizedBox, TextArea};

// POINTER EVENTS

fn create_capture_target() -> ModularWidget<()> {
    ModularWidget::new(())
        .pointer_event_fn(|_, ctx, _, event| {
            if matches!(event, PointerEvent::Down { .. }) {
                ctx.capture_pointer();
            }
        })
        .measure_fn(|_, _, _, _, _, _| 10.px())
}

#[test]
fn pointer_event() {
    let button_tag = WidgetTag::named("button");

    let button = NewWidget::new(Button::with_text("button").record()).with_tag(button_tag);

    let mut harness = TestHarness::create(test_property_set(), button);
    let button_id = harness.get_widget(button_tag).unwrap().id();

    harness.flush_records_of(button_tag).unwrap();
    harness.mouse_move_to(button_id).unwrap();

    let records = harness.take_records_of(button_tag).unwrap();
    assert_any(records, |r| {
        matches!(r, Record::PointerEvent(PointerEvent::Move(_)))
    });
}

#[test]
fn pointer_event_bubbling() {
    let button_tag = WidgetTag::named("button");
    let parent_tag = WidgetTag::named("parent");
    let grandparent_tag = WidgetTag::named("grandparent");

    let button = NewWidget::new(Button::with_text("button").record()).with_tag(button_tag);
    let parent = NewWidget::new(ModularWidget::new_parent(button).record()).with_tag(parent_tag);
    let grandparent =
        NewWidget::new(ModularWidget::new_parent(parent).record()).with_tag(grandparent_tag);

    let mut harness = TestHarness::create(test_property_set(), grandparent);
    let button_id = harness.get_widget(button_tag).unwrap().id();

    harness.flush_records_of(button_tag).unwrap();
    harness.mouse_click_on(button_id, None).unwrap();

    fn is_pointer_down(record: Record) -> bool {
        matches!(record, Record::PointerEvent(PointerEvent::Down { .. }))
    }

    assert_any(
        harness.take_records_of(button_tag).unwrap(),
        is_pointer_down,
    );
    assert_any(
        harness.take_records_of(parent_tag).unwrap(),
        is_pointer_down,
    );
    assert_any(
        harness.take_records_of(grandparent_tag).unwrap(),
        is_pointer_down,
    );
}

#[test]
fn add_child_on_pointer_event() {
    let parent_tag = WidgetTag::named("parent");

    let parent = ModularWidget::new_multi_parent(vec![SizedBox::empty().prepare()])
        .pointer_event_fn(|children, ctx, _, event| {
            if matches!(event, PointerEvent::Down { .. }) {
                children.push(SizedBox::empty().prepare().into_pod());
                ctx.children_changed();
            }
        });
    let parent = NewWidget::new(parent).with_tag(parent_tag);

    let mut harness = TestHarness::create(test_property_set(), parent);
    let parent_id = harness.get_widget(parent_tag).unwrap().id();

    assert_eq!(harness.get_widget(parent_tag).unwrap().children().len(), 1);
    harness.mouse_click_on(parent_id, None).unwrap();
    assert_eq!(harness.get_widget(parent_tag).unwrap().children().len(), 2);
}

#[test]
fn pointer_capture_and_cancel() {
    let target_tag = WidgetTag::named("target");

    let target = create_capture_target();
    let target = NewWidget::new(target).with_tag(target_tag);

    let mut harness = TestHarness::create(test_property_set(), target);

    let target_id = harness.get_widget(target_tag).unwrap().id();

    harness.mouse_move_to(target_id).unwrap();
    harness.mouse_button_press(None);
    assert_eq!(harness.pointer_capture_target_id(), Some(target_id));

    harness.process_pointer_event(&PointerEvent::Cancel(PointerInfo {
        pointer_id: None,
        persistent_device_id: None,
        pointer_type: PointerType::default(),
    }));
    assert_eq!(harness.pointer_capture_target_id(), None);
}

#[test]
fn pointer_capture_is_owned_by_the_initiating_pointer_and_button() {
    let mut harness =
        TestHarness::create(test_property_set(), Button::with_text("capture").prepare());
    let id = harness.root_id();
    harness.mouse_move_to(id).unwrap();
    harness.mouse_button_press(Some(PointerButton::Primary));
    assert_eq!(harness.pointer_capture_target_id(), Some(id));

    // A mouse chord cannot replace the initiating button or produce a click.
    harness.mouse_button_press(Some(PointerButton::Secondary));
    harness.mouse_button_release(Some(PointerButton::Secondary));
    assert_eq!(harness.pointer_capture_target_id(), Some(id));
    assert!(harness.pop_action::<ButtonPress>().unwrap().is_none());

    // An unrelated touch cannot steal capture, move its hover position,
    // terminate the press, or generate a click.
    let other = PointerInfo {
        pointer_id: PointerId::new(42),
        persistent_device_id: None,
        pointer_type: PointerType::Touch,
    };
    let event = PointerButtonEvent {
        pointer: other,
        button: Some(PointerButton::Primary),
        state: PointerState {
            position: PhysicalPosition::new(-100.0, -100.0),
            ..PointerState::default()
        },
    };
    harness.process_pointer_event(&PointerEvent::Down(event.clone()));
    harness.process_pointer_event(&PointerEvent::Move(PointerUpdate {
        pointer: other,
        current: event.state.clone(),
        coalesced: vec![],
        predicted: vec![],
    }));
    harness.process_pointer_event(&PointerEvent::Up(event));
    harness.process_pointer_event(&PointerEvent::Cancel(other));
    assert_eq!(harness.pointer_capture_target_id(), Some(id));
    assert!(harness.get_widget_with_id(id).unwrap().ctx().is_hovered());
    assert!(harness.pop_action::<ButtonPress>().unwrap().is_none());

    harness.mouse_button_release(Some(PointerButton::Primary));
    assert_eq!(harness.pointer_capture_target_id(), None);
    assert_eq!(
        harness.pop_action::<ButtonPress>().unwrap(),
        Some((
            ButtonPress {
                button: Some(PointerButton::Primary)
            },
            id
        )),
    );
}

#[test]
fn pointer_release_uses_current_position_without_intervening_move() {
    let mut harness =
        TestHarness::create(test_property_set(), Button::with_text("capture").prepare());
    let id = harness.root_id();
    harness.mouse_move_to(id).unwrap();
    harness.mouse_button_press(Some(PointerButton::Primary));

    harness.process_pointer_event(&PointerEvent::Up(PointerButtonEvent {
        pointer: PointerInfo {
            pointer_id: Some(PointerId::PRIMARY),
            persistent_device_id: None,
            pointer_type: PointerType::Mouse,
        },
        button: Some(PointerButton::Primary),
        state: PointerState {
            position: PhysicalPosition::new(-100.0, -100.0),
            ..PointerState::default()
        },
    }));
    assert_eq!(harness.pointer_capture_target_id(), None);
    assert!(harness.pop_action::<ButtonPress>().unwrap().is_none());
    assert!(!harness.get_widget_with_id(id).unwrap().ctx().is_hovered());
}

#[test]
fn synthetic_cancel() {
    let target_tag = WidgetTag::named("target");

    let target = create_capture_target();
    let target = NewWidget::new(target.record()).with_tag(target_tag);

    let mut harness = TestHarness::create(test_property_set(), target);

    let target_id = harness.get_widget(target_tag).unwrap().id();

    harness.mouse_move_to(target_id).unwrap();
    harness.mouse_button_press(None);
    assert_eq!(harness.pointer_capture_target_id(), Some(target_id));

    // When we disable a widget with pointer capture, it gets a
    // synthetic PointerCancel event.
    harness.set_disabled(target_tag, true).unwrap();

    let records = harness.take_records_of(target_tag).unwrap();
    assert_any(records, |r| {
        matches!(r, Record::PointerEvent(PointerEvent::Cancel(_)))
    });
}

#[test]
fn pointer_capture_suppresses_neighbors() {
    let target_tag = WidgetTag::named("target");
    let other_tag = WidgetTag::named("other");

    let target = create_capture_target();
    let target = NewWidget::new(target).with_tag(target_tag);

    let other = Button::with_text("Hello");
    let other = NewWidget::new(other.record()).with_tag(other_tag);

    let parent = Flex::column()
        .with_fixed(target)
        .with_fixed(other)
        .prepare();

    let mut harness = TestHarness::create(test_property_set(), parent);
    harness.flush_records_of(other_tag).unwrap();

    let target_id = harness.get_widget(target_tag).unwrap().id();
    let other_id = harness.get_widget(other_tag).unwrap().id();

    harness.mouse_move_to(target_id).unwrap();
    harness.mouse_button_press(None);

    assert_eq!(harness.pointer_capture_target_id(), Some(target_id));

    // As long as 'target' is captured, 'other' doesn't get pointer events, even when the cursor is on it.
    harness.mouse_move_to(other_id).unwrap();
    assert_matches!(harness.take_records_of(other_tag).unwrap()[..], []);

    // 'other' is not considered hovered either.
    assert!(!harness.get_widget(other_tag).unwrap().ctx().is_hovered());

    // We end pointer capture.
    harness.mouse_button_release(None);
    assert_eq!(harness.pointer_capture_target_id(), None);

    // Once the capture is released, 'other' should immediately register as hovered.
    assert!(harness.get_widget(other_tag).unwrap().ctx().is_hovered());
}

#[test]
fn try_capture_pointer_on_pointer_move() {
    let widget = ModularWidget::new(())
        .pointer_event_fn(|_, ctx, _, _event| {
            ctx.capture_pointer();
        })
        .prepare();

    let mut harness = TestHarness::create(test_property_set(), widget);

    assert_debug_panics!(
        harness.mouse_move((10.0, 10.0)),
        "event does not allow pointer capture"
    );
}

#[test]
fn try_capture_pointer_on_text_event() {
    let widget = ModularWidget::new(())
        .accepts_focus(true)
        .text_event_fn(|_, ctx, _, _event| {
            ctx.capture_pointer();
        })
        .prepare();

    let mut harness = TestHarness::create(test_property_set(), widget);
    let id = harness.root_id();
    harness.focus_on(Some(id));

    assert_debug_panics!(
        harness.keyboard_type_chars("a"),
        "event does not allow pointer capture"
    );
}

#[test]
fn pointer_cancel_on_window_blur() {
    let target_tag = WidgetTag::named("target");

    let target = create_capture_target();
    let target = NewWidget::new(target.record()).with_tag(target_tag);

    let mut harness = TestHarness::create(test_property_set(), target);

    let target_id = harness.get_widget(target_tag).unwrap().id();

    harness.mouse_move_to(target_id).unwrap();
    harness.mouse_button_press(None);
    assert_eq!(harness.pointer_capture_target_id(), Some(target_id));
    harness.flush_records_of(target_tag).unwrap();

    harness.process_text_event(&TextEvent::WindowFocusChange(false));

    let records = harness.take_records_of(target_tag).unwrap();
    assert_any(records, |r| {
        matches!(r, Record::PointerEvent(PointerEvent::Cancel(..)))
    });
}

#[test]
fn click_anchors_focus() {
    let child_3 = WidgetTag::named("child_3");
    let child_4 = WidgetTag::named("child_4");
    let other = WidgetTag::named("other");

    let parent = Flex::column()
        .with_fixed(NewWidget::new(SizedBox::empty().size(5.px(), 5.px())).with_tag(other))
        .with_fixed(NewWidget::new(Button::with_text("")))
        .with_fixed(NewWidget::new(Button::with_text("")))
        .with_fixed(NewWidget::new(Button::with_text("Click me!")).with_tag(child_3))
        .with_fixed(NewWidget::new(Button::with_text("")).with_tag(child_4))
        .with_fixed(NewWidget::new(Button::with_text("")))
        .prepare();

    let mut harness = TestHarness::create(test_property_set(), parent);

    let child_3_id = harness.get_widget(child_3).unwrap().id();
    let child_4_id = harness.get_widget(child_4).unwrap().id();
    let other_id = harness.get_widget(other).unwrap().id();

    // Clicking a disabled button doesn't focus it.
    harness.set_disabled(child_3, true).unwrap();
    harness.mouse_click_on(child_3_id, None).unwrap();
    assert_eq!(harness.focused_widget_id(), None);

    // But the next tab event focuses its neighbor.
    harness.process_text_event(&TextEvent::key_down(Key::Named(NamedKey::Tab)));
    assert_eq!(harness.focused_widget_id(), Some(child_4_id));

    // TODO - Remove use of mouse_move_to_unchecked after
    // https://github.com/linebender/xilem/issues/1620
    // is resolved.

    // Clicking another non-focusable widget clears focus.
    harness.mouse_move_to_unchecked(other_id).unwrap();
    harness.mouse_button_press(None);
    harness.mouse_button_release(None);
    assert_eq!(harness.focused_widget_id(), None);
}

#[track_caller]
fn make_pointer_info(pos: impl Into<Point>, id: u64) -> (PointerInfo, PointerState) {
    let pointer_id = PointerId::new(id).unwrap();
    let mut pointer_state = PointerState::default();

    let Point { x, y } = pos.into();
    let pos = PhysicalPosition { x, y };
    pointer_state.position = pos;

    let pointer_info = PointerInfo {
        pointer_id: Some(pointer_id),
        persistent_device_id: None,
        pointer_type: PointerType::Mouse,
    };

    (pointer_info, pointer_state)
}

#[track_caller]
fn pointer_move(pos: impl Into<Point>, id: u64) -> PointerEvent {
    let (pointer_info, pointer_state) = make_pointer_info(pos, id);
    PointerEvent::Move(PointerUpdate {
        pointer: pointer_info,
        current: pointer_state,
        coalesced: vec![],
        predicted: vec![],
    })
}

// TODO - Implement multi-pointer hover
#[test]
#[ignore]
fn multi_pointers_hover() {
    let button_1_tag = WidgetTag::named("button_1");
    let button_2_tag = WidgetTag::named("button_2");
    let flex_tag = WidgetTag::named("flex");

    let button_1 = NewWidget::new(Button::with_text("Button 1").record()).with_tag(button_1_tag);
    let button_2 = NewWidget::new(Button::with_text("Button 2").record()).with_tag(button_2_tag);

    let flex = NewWidget::new(
        Flex::row()
            .with_fixed(button_1)
            .with_fixed(button_2)
            .record(),
    )
    .with_tag(flex_tag);
    let mut harness = TestHarness::create(test_property_set(), flex);

    let button_1_rect = harness
        .get_widget(button_1_tag)
        .unwrap()
        .ctx()
        .bounding_box();
    let button_2_rect = harness
        .get_widget(button_2_tag)
        .unwrap()
        .ctx()
        .bounding_box();

    // Move pointer 11 over button_1,
    // Verify HoverChange(true), ChildHoveredChanged(true) in flex, and hovered state
    harness.process_pointer_event(&pointer_move(button_1_rect.center(), 11));

    assert_any(harness.take_records_of(button_1_tag).unwrap(), |r| {
        matches!(r, Record::Update(Update::HoveredChanged(true)))
    });
    assert_any(harness.take_records_of(flex_tag).unwrap(), |r| {
        matches!(r, Record::Update(Update::ChildHoveredChanged(true)))
    });
    assert!(harness.get_widget(button_1_tag).unwrap().ctx().is_hovered());
    assert!(harness.get_widget(flex_tag).unwrap().ctx().has_hovered());

    // Move pointer 22 over button_1,
    // Verify no HoverChange(true), no ChildHoveredChanged(true) in flex
    harness.process_pointer_event(&pointer_move(button_1_rect.center(), 22));

    assert_none(harness.take_records_of(button_1_tag).unwrap(), |r| {
        matches!(r, Record::Update(Update::HoveredChanged(_)))
    });
    assert_none(harness.take_records_of(flex_tag).unwrap(), |r| {
        matches!(r, Record::Update(Update::ChildHoveredChanged(_)))
    });

    // Move pointer 22 over button_2,
    // Verify HoverChange(true), and hovered state on button_2
    // Verify no HoverChange(false), no ChildHoveredChanged(false) in flex, and hovered state on button_1
    harness.process_pointer_event(&pointer_move(button_2_rect.center(), 22));

    assert_any(harness.take_records_of(button_2_tag).unwrap(), |r| {
        matches!(r, Record::Update(Update::HoveredChanged(true)))
    });
    assert_none(harness.take_records_of(button_1_tag).unwrap(), |r| {
        matches!(r, Record::Update(Update::HoveredChanged(false)))
    });
    assert_none(harness.take_records_of(flex_tag).unwrap(), |r| {
        matches!(r, Record::Update(Update::ChildHoveredChanged(false)))
    });
    assert!(harness.get_widget(button_2_tag).unwrap().ctx().is_hovered());
    assert!(harness.get_widget(button_1_tag).unwrap().ctx().is_hovered());
    assert!(harness.get_widget(flex_tag).unwrap().ctx().has_hovered());

    // Move pointer 11 outside window,
    // Verify HoverChange(true), and hovered state on button_2
    // no ChildHoveredChanged(false) in flex
    harness.process_pointer_event(&pointer_move((10_000., 10_000.), 11));

    assert_any(harness.take_records_of(button_1_tag).unwrap(), |r| {
        matches!(r, Record::Update(Update::HoveredChanged(false)))
    });
    assert_none(harness.take_records_of(button_2_tag).unwrap(), |r| {
        matches!(r, Record::Update(Update::HoveredChanged(false)))
    });
    assert_none(harness.take_records_of(flex_tag).unwrap(), |r| {
        matches!(r, Record::Update(Update::ChildHoveredChanged(false)))
    });
    assert!(!harness.get_widget(button_1_tag).unwrap().ctx().is_hovered());
    assert!(harness.get_widget(button_2_tag).unwrap().ctx().is_hovered());
    assert!(harness.get_widget(flex_tag).unwrap().ctx().has_hovered());
}

// TEXT EVENTS

#[test]
fn text_event() {
    let target_tag = WidgetTag::named("target");

    let target = NewWidget::new(TextArea::new_editable("").record()).with_tag(target_tag);

    let mut harness = TestHarness::create(test_property_set(), target);
    let target_id = harness.get_widget(target_tag).unwrap().id();
    harness.flush_records_of(target_tag).unwrap();

    // The widget isn't focused, it doesn't get text events.
    harness.keyboard_type_chars("A");
    assert_matches!(harness.take_records_of(target_tag).unwrap()[..], []);

    // We focus on the widget, now it gets text events.
    harness.focus_on(Some(target_id));
    harness.keyboard_type_chars("A");
    let records = harness.take_records_of(target_tag).unwrap();
    assert_any(records, |r| matches!(r, Record::TextEvent(_)));
}

#[test]
fn text_event_bubbling() {
    let target_tag = WidgetTag::named("target");
    let parent_tag = WidgetTag::named("parent");
    let grandparent_tag = WidgetTag::named("grandparent");

    let target =
        NewWidget::new(ModularWidget::new(()).accepts_focus(true).record()).with_tag(target_tag);
    let parent = NewWidget::new(ModularWidget::new_parent(target).record()).with_tag(parent_tag);
    let grandparent =
        NewWidget::new(ModularWidget::new_parent(parent).record()).with_tag(grandparent_tag);

    let mut harness = TestHarness::create(test_property_set(), grandparent);
    let target_id = harness.get_widget(target_tag).unwrap().id();

    harness.focus_on(Some(target_id));
    harness.process_text_event(&TextEvent::key_down(Key::Character("A".into())));

    fn is_keyboard_event(record: Record) -> bool {
        matches!(record, Record::TextEvent(TextEvent::Keyboard(_)))
    }

    assert_any(
        harness.take_records_of(target_tag).unwrap(),
        is_keyboard_event,
    );
    assert_any(
        harness.take_records_of(parent_tag).unwrap(),
        is_keyboard_event,
    );
    assert_any(
        harness.take_records_of(grandparent_tag).unwrap(),
        is_keyboard_event,
    );
}

#[test]
fn text_event_fallback() {
    let target_tag = WidgetTag::named("target");
    let other_tag = WidgetTag::named("other");

    let target = NewWidget::new(TextArea::new_editable("").record()).with_tag(target_tag);
    let other = NewWidget::new(TextArea::new_editable("")).with_tag(other_tag);
    let parent = Flex::row().with_fixed(target).with_fixed(other).prepare();

    let mut harness = TestHarness::create(test_property_set(), parent);
    let target_id = harness.get_widget(target_tag).unwrap().id();
    let other_id = harness.get_widget(other_tag).unwrap().id();
    harness.flush_records_of(target_tag).unwrap();
    harness.set_focus_fallback(Some(target_id));

    harness.focus_on(Some(other_id));
    assert_matches!(harness.take_records_of(target_tag).unwrap()[..], []);

    // If a widget is set as focus fallback, that widget gets text events when no widget is focused.
    harness.focus_on(None);
    harness.keyboard_type_chars("A");
    let records = harness.take_records_of(target_tag).unwrap();
    assert_any(records, |r| matches!(r, Record::TextEvent(_)));

    // Unless it's disabled.
    harness.set_disabled(target_tag, true).unwrap();
    harness.flush_records_of(target_tag).unwrap();
    harness.keyboard_type_chars("A");
    assert_matches!(harness.take_records_of(target_tag).unwrap()[..], []);
}

#[test]
fn tab_focus() {
    let child_1 = WidgetTag::named("child_1");
    let child_2 = WidgetTag::named("child_2");
    let child_3 = WidgetTag::named("child_3");
    let child_4 = WidgetTag::named("child_4");
    let child_5 = WidgetTag::named("child_5");

    let parent = Flex::column()
        .with_fixed(NewWidget::new(Button::with_text("")).with_tag(child_1))
        .with_fixed(NewWidget::new(Button::with_text("")).with_tag(child_2))
        .with_fixed(NewWidget::new(Button::with_text("")).with_tag(child_3))
        .with_fixed(NewWidget::new(Button::with_text("")).with_tag(child_4))
        .with_fixed(NewWidget::new(Button::with_text("")).with_tag(child_5))
        .prepare();

    let mut harness = TestHarness::create(test_property_set(), parent);

    let child_1_id = harness.get_widget(child_1).unwrap().id();
    let child_2_id = harness.get_widget(child_2).unwrap().id();
    let child_3_id = harness.get_widget(child_3).unwrap().id();
    let child_4_id = harness.get_widget(child_4).unwrap().id();
    let child_5_id = harness.get_widget(child_5).unwrap().id();

    assert_eq!(harness.focused_widget_id(), None);

    // Tab moves focus to the next focusable widget in the tree.
    harness.focus_on(Some(child_2_id));
    harness.press_tab_key(false);
    assert_eq!(harness.focused_widget_id(), Some(child_3_id));

    // Shift+Tab moves focus to the previous focusable widget in the tree.
    harness.focus_on(Some(child_4_id));
    harness.press_tab_key(true);
    assert_eq!(harness.focused_widget_id(), Some(child_3_id));

    // When nothing is focused, Tab focuses the first focusable widget in the tree.
    harness.focus_on(None);
    harness.press_tab_key(false);
    assert_eq!(harness.focused_widget_id(), Some(child_1_id));

    // When nothing is focused, Shift+Tab focuses the last focusable widget in the tree.
    harness.focus_on(None);
    harness.press_tab_key(true);
    assert_eq!(harness.focused_widget_id(), Some(child_5_id));
}

// ACCESS EVENTS

#[test]
fn access_event_bubbling() {
    let target_tag = WidgetTag::named("target");
    let parent_tag = WidgetTag::named("parent");
    let grandparent_tag = WidgetTag::named("grandparent");

    let target = NewWidget::new(ModularWidget::new(()).record()).with_tag(target_tag);
    let parent = NewWidget::new(ModularWidget::new_parent(target).record()).with_tag(parent_tag);
    let grandparent =
        NewWidget::new(ModularWidget::new_parent(parent).record()).with_tag(grandparent_tag);

    let mut harness = TestHarness::create(test_property_set(), grandparent);
    let target_id = harness.get_widget(target_tag).unwrap().id();

    // Send random event
    harness.process_access_event(ActionRequest {
        action: accesskit::Action::Click,
        target_tree: TreeId::ROOT,
        target_node: target_id.into(),
        data: None,
    });

    fn is_access_click(record: Record) -> bool {
        matches!(
            record,
            Record::AccessEvent(AccessEvent {
                action: accesskit::Action::Click,
                data: None
            })
        )
    }

    assert_any(
        harness.take_records_of(target_tag).unwrap(),
        is_access_click,
    );
    assert_any(
        harness.take_records_of(parent_tag).unwrap(),
        is_access_click,
    );
    assert_any(
        harness.take_records_of(grandparent_tag).unwrap(),
        is_access_click,
    );
}

#[test]
fn accessibility_focus() {
    let child_2 = WidgetTag::named("child_2");
    let child_3 = WidgetTag::named("child_3");

    let parent = Flex::column()
        .with_fixed(NewWidget::new(Button::with_text("")))
        .with_fixed(NewWidget::new(Button::with_text("")).with_tag(child_2))
        .with_fixed(NewWidget::new(Button::with_text("")).with_tag(child_3))
        .with_fixed(NewWidget::new(Button::with_text("")))
        .prepare();

    let mut harness = TestHarness::create(test_property_set(), parent);
    let child_2_id = harness.get_widget(child_2).unwrap().id();
    let child_3_id = harness.get_widget(child_3).unwrap().id();

    // Send focus event
    harness.process_access_event(ActionRequest {
        action: accesskit::Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: child_3_id.into(),
        data: None,
    });
    assert_eq!(harness.focused_widget_id(), Some(child_3_id));

    // Send blur event with incorrect id
    harness.process_access_event(ActionRequest {
        action: accesskit::Action::Blur,
        target_tree: TreeId::ROOT,
        target_node: child_2_id.into(),
        data: None,
    });
    assert_eq!(harness.focused_widget_id(), Some(child_3_id));

    // Send blur event with correct id
    harness.process_access_event(ActionRequest {
        action: accesskit::Action::Blur,
        target_tree: TreeId::ROOT,
        target_node: child_3_id.into(),
        data: None,
    });
    assert_eq!(harness.focused_widget_id(), None);
}

#[test]
fn downcast_untyped_action() {
    let target_tag = WidgetTag::named("arbitrary_submitter");

    #[derive(Debug)]
    struct ArbitraryAction;

    let arbitrary_submitter = ModularWidget::new(()).pointer_event_fn(|_, ctx, _, _| {
        ctx.submit_untyped_action(Box::new(ArbitraryAction));
    });

    let widget = NewWidget::new(arbitrary_submitter).with_tag(target_tag);

    let mut harness = TestHarness::create(test_property_set(), widget);
    let target_id = harness.get_widget(target_tag).unwrap().id();

    harness.mouse_move_to(target_id).unwrap();

    assert_matches!(
        harness.pop_action::<ArbitraryAction>().unwrap(),
        Some((ArbitraryAction, _))
    );
}

#[test]
fn accessibility_requests_for_removed_widgets_are_ignored() {
    let tag = WidgetTag::named("removed-button");
    let child = NewWidget::new(Button::with_text("")).with_tag(tag);
    let mut harness =
        TestHarness::create(test_property_set(), NewWidget::new(SizedBox::new(child)));
    let id = harness.get_widget(tag).unwrap().id();
    harness
        .edit_root_widget(|mut root| SizedBox::remove_child(&mut root))
        .unwrap();
    for action in [
        accesskit::Action::Focus,
        accesskit::Action::Blur,
        accesskit::Action::ScrollIntoView,
        accesskit::Action::Click,
    ] {
        harness.process_access_event(ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: id.into(),
            data: None,
        });
    }
    assert_eq!(harness.focused_widget_id(), None);
    assert!(harness.get_widget_with_id(id).is_none());
}

#[test]
fn accessibility_activation_rejects_hidden_and_disabled_targets_without_bubbling() {
    let child = NewWidget::new(Button::with_text(""));
    let id = child.id();
    let parent = ModularWidget::new_parent(child)
        .access_event_fn(|_, ctx, _, event| {
            if event.action == accesskit::Action::Click {
                ctx.submit_untyped_action(Box::new(()));
            }
        })
        .prepare();
    let mut harness = TestHarness::create(test_property_set(), parent);

    harness.accessibility_click_on(id);
    assert!(
        harness
            .pop_action::<crate::widgets::ButtonPress>()
            .unwrap()
            .is_some()
    );
    assert!(harness.pop_action::<()>().unwrap().is_some());

    harness
        .edit_widget_with_id(id, |mut child| child.ctx.set_disabled(true))
        .unwrap();
    harness.accessibility_click_on(id);
    assert!(harness.pop_action_erased().is_none());

    harness
        .edit_widget_with_id(id, |mut child| child.ctx.set_disabled(false))
        .unwrap();
    harness
        .edit_root_widget(|mut parent| {
            parent.ctx.set_stashed(&parent.widget.state, true);
        })
        .unwrap();
    harness.accessibility_click_on(id);
    assert!(harness.pop_action_erased().is_none());
}
