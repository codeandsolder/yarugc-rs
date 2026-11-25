// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use tracing::{info_span, trace};

use crate::app::{PointerPress, RenderRoot, RenderRootSignal};
use crate::core::keyboard::{Key, KeyState, NamedKey};
use crate::core::{
    AccessEvent, EventCtx, Handled, Ime, PointerButtonEvent, PointerEvent, PointerGestureEvent,
    PointerInfo, PointerScrollEvent, PointerType, PointerUpdate, PropertiesMut, TextEvent, Widget,
    WidgetId,
};
use crate::dpi::{LogicalPosition, PhysicalPosition};
use crate::passes::update::{find_next_focusable, run_update_pointer_pass};
use crate::passes::{enter_span, merge_state_up};

// --- MARK: HELPERS
fn get_pointer_target(
    root: &RenderRoot,
    pointer_pos: Option<LogicalPosition<f64>>,
) -> Option<WidgetId> {
    // See the [pointer capture documentation](../doc/06_masonry_concepts.md#pointer-capture).
    if let Some(capture_target) = root.global_state.pointer_capture_target()
        && root.widget_arena.has(capture_target)
    {
        return Some(capture_target);
    }

    if let Some(pointer_pos) = pointer_pos {
        let pointer_pos = (pointer_pos.x, pointer_pos.y).into();
        return root
            .widget_at_position(pointer_pos)
            .map(|widget| widget.id());
    }

    None
}

/// `true` if this [`PointerEvent`] type is likely to occur every frame.
const fn is_very_frequent(e: &PointerEvent) -> bool {
    matches!(e, PointerEvent::Move(..) | PointerEvent::Scroll { .. })
}

/// Short name for a [`PointerEvent`].
const fn pointer_event_short_name(e: &PointerEvent) -> &'static str {
    match e {
        PointerEvent::Down(..) => "Down",
        PointerEvent::Up(..) => "Up",
        PointerEvent::Move(..) => "Move",
        PointerEvent::Enter(..) => "Enter",
        PointerEvent::Leave(..) => "Leave",
        PointerEvent::Cancel(..) => "Cancel",
        PointerEvent::Scroll(..) => "Scroll",
        PointerEvent::Gesture(..) => "Gesture",
    }
}

/// A position if the event has one.
const fn try_event_position(event: &PointerEvent) -> Option<PhysicalPosition<f64>> {
    match event {
        PointerEvent::Down(PointerButtonEvent { state, .. })
        | PointerEvent::Up(PointerButtonEvent { state, .. })
        | PointerEvent::Move(PointerUpdate { current: state, .. })
        | PointerEvent::Scroll(PointerScrollEvent { state, .. })
        | PointerEvent::Gesture(PointerGestureEvent { state, .. }) => Some(state.position),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EventRoute {
    Bubble,
    BubbleIncludingDisabled,
    TargetOnly,
}

fn run_event_pass<E>(
    root: &mut RenderRoot,
    target: Option<WidgetId>,
    event: &E,
    route: EventRoute,
    pointer_press: Option<PointerPress>,
    pass_fn: impl FnMut(&mut dyn Widget, &mut EventCtx<'_>, &mut PropertiesMut<'_>, &E),
    trace: bool,
) -> Handled {
    let mut pass_fn = pass_fn;

    let Some(original_target) = target else {
        return Handled::No;
    };
    let mut target_widget_id = Some(original_target);
    let mut is_handled = false;
    while let Some(widget_id) = target_widget_id {
        let parent_id = root.widget_arena.parent_of(widget_id);
        let Some(mut node) = root.widget_arena.nodes.find_mut(widget_id) else {
            break;
        };

        if route == EventRoute::Bubble && node.item.state.is_disabled() {
            // If the widget is disabled, we skip it and bubble to its parent.
            target_widget_id = parent_id;
            continue;
        }

        if !is_handled && (route != EventRoute::TargetOnly || widget_id == original_target) {
            let _span = enter_span(&node.item.state);
            let widget_type_id = node.item.widget.as_ref().type_id();
            let stack = root
                .property_arena
                .get(node.item.state.property_stack_id, widget_type_id);
            let mut ctx = EventCtx {
                global_state: &mut root.global_state,
                widget_state: &mut node.item.state,
                children: node.children.reborrow_mut(),
                property_arena: &root.property_arena,
                target: original_target,
                pointer_press,
                is_handled: false,
            };
            let widget = &mut *node.item.widget;
            if trace {
                trace!(
                    "Widget '{}' {} visited",
                    widget.short_type_name(),
                    widget_id,
                );
            }

            let mut props = PropertiesMut {
                local: &mut node.item.properties,
                default_map: root
                    .property_arena
                    .default_properties
                    .for_widget(widget.type_id()),
                stack,
                class_set: &node.item.class_set,
            };
            pass_fn(widget, &mut ctx, &mut props, event);
            is_handled = ctx.is_handled;
        }

        merge_state_up(&mut root.widget_arena, widget_id);
        target_widget_id = parent_id;
    }

    Handled::from(is_handled)
}

impl PointerPress {
    // Capture owns a single pointer/button interaction. Identity-free cancellation
    // is a window-wide synthetic cancel; an unrelated contact is not.
    fn accepts_event(self, event: &PointerEvent) -> bool {
        let same_pointer = |pointer: &PointerInfo| {
            self.pointer.pointer_id.map_or_else(
                || *pointer == self.pointer,
                |id| pointer.pointer_id == Some(id),
            )
        };
        match event {
            PointerEvent::Down(_) => false,
            PointerEvent::Up(release) => {
                same_pointer(&release.pointer) && self.button == release.button
            }
            PointerEvent::Cancel(pointer) => pointer.pointer_id.is_none() || same_pointer(pointer),
            PointerEvent::Move(PointerUpdate { pointer, .. })
            | PointerEvent::Scroll(PointerScrollEvent { pointer, .. })
            | PointerEvent::Gesture(PointerGestureEvent { pointer, .. })
            | PointerEvent::Enter(pointer)
            | PointerEvent::Leave(pointer) => same_pointer(pointer),
        }
    }
}

// --- MARK: POINTER_EVENT
fn capture_layer_pointer_events(root: &mut RenderRoot, event: &PointerEvent) {
    let layer_ids = root.layer_root_ids();
    for layer_id in layer_ids {
        let Some(mut layer_root) = root.widget_arena.nodes.find_mut(layer_id) else {
            continue;
        };
        let layer_type_id = layer_root.item.widget.as_ref().type_id();
        if let Some(layer) = layer_root.item.widget.as_layer() {
            let stack = root
                .property_arena
                .get(layer_root.item.state.property_stack_id, layer_type_id);
            let mut ctx = EventCtx {
                global_state: &mut root.global_state,
                widget_state: &mut layer_root.item.state,
                children: layer_root.children.reborrow_mut(),
                property_arena: &root.property_arena,
                target: layer_id,
                pointer_press: None,
                is_handled: false,
            };
            let mut props = PropertiesMut {
                local: &mut layer_root.item.properties,
                default_map: root
                    .property_arena
                    .default_properties
                    .for_widget(layer.type_id()),
                stack,
                class_set: &layer_root.item.class_set,
            };

            layer.capture_pointer_event(&mut ctx, &mut props, event);
        }
    }
}

/// See the [passes documentation](crate::doc::pass_system#event-passes).
pub(crate) fn run_on_pointer_event_pass(root: &mut RenderRoot, event: &PointerEvent) -> Handled {
    let _span = info_span!("dispatch_pointer_event").entered();

    trace!(
        "Running ON_POINTER_EVENT pass with {}",
        pointer_event_short_name(event)
    );

    if let Some(capture) = root.global_state.pointer_capture
        && !capture.press.accepts_event(event)
    {
        return Handled::No;
    }

    let event_pos = try_event_position(event).map(|p| p.to_logical(root.global_state.scale_factor));

    let position_changed = event_pos != root.global_state.last_mouse_pos;
    root.global_state.last_mouse_pos = event_pos;
    root.global_state.set_needs_pointer_pass(true);

    // A release may arrive at a different location without an intervening Move
    // (notably on touch devices). Click handlers must see the release position.
    if position_changed
        && root.global_state.pointer_capture.is_some()
        && matches!(event, PointerEvent::Up(_))
    {
        run_update_pointer_pass(root);
    }

    if root.global_state.inspector_state.is_picking_widget
        && matches!(event, PointerEvent::Move(..))
    {
        return Handled::Yes;
    }

    // If the widget picker is active and this is a click event,
    // we select the widget under the mouse and short-circuit the event pass.
    if root.global_state.inspector_state.is_picking_widget
        && matches!(event, PointerEvent::Down { .. })
    {
        let target_widget_id = get_pointer_target(root, event_pos);
        if let Some(target_widget_id) = target_widget_id {
            root.global_state
                .emit_signal(RenderRootSignal::WidgetSelectedInInspector(
                    target_widget_id,
                ));
        }
        root.global_state.inspector_state.is_picking_widget = false;
        root.global_state.inspector_state.hovered_widget = None;
        if let Some(state) = root.root_state_mut() {
            state.set_needs_paint(true);
        }
        return Handled::Yes;
    }

    capture_layer_pointer_events(root, event);

    let target_widget_id = get_pointer_target(root, event_pos);

    if matches!(event, PointerEvent::Down { .. })
        && let Some(target_widget_id) = target_widget_id
    {
        // The next tab event will assign focus around this widget.
        root.global_state.focus_anchor = Some(target_widget_id);

        // If we click outside of the focused widget, we clear the focus.
        if let Some(focused_widget) = root.global_state.focused_widget {
            // Focused_widget isn't ancestor of target_widget_id
            if !root
                .widget_arena
                .nodes
                .get_id_path(target_widget_id)
                .contains(&focused_widget.to_raw())
            {
                root.global_state.next_focused_widget = None;
            }
        }
    }

    let route = if matches!(event, PointerEvent::Cancel { .. }) {
        EventRoute::BubbleIncludingDisabled
    } else {
        EventRoute::Bubble
    };
    let handled = run_event_pass(
        root,
        target_widget_id,
        event,
        route,
        match event {
            PointerEvent::Down(press) => Some(PointerPress {
                pointer: press.pointer,
                button: press.button,
            }),
            _ => None,
        },
        |widget, ctx, props, event| {
            widget.on_pointer_event(ctx, props, event);
        },
        !is_very_frequent(event),
    );

    if matches!(event, PointerEvent::Up { .. } | PointerEvent::Cancel(..)) {
        // Only the initiating release or an applicable cancel can reach this point.
        // If a widget holds the capture,
        // it is notified of the pointer event before the capture is released, so it knows it is
        // about to lose the pointer.
        root.global_state.pointer_capture = None;
        root.global_state.set_needs_pointer_pass(true);
    }

    if !is_very_frequent(event) {
        trace!(
            focused_widget = root.global_state.focused_widget.map(|id| id.0),
            handled = handled.is_handled(),
            "ON_POINTER_EVENT finished",
        );
    }

    handled
}

// IME events belong to one text-input owner. In particular the final Disabled
// event must reach that owner even if it has just become disabled or stashed.
fn dispatch_ime(root: &mut RenderRoot, owner: WidgetId, event: &TextEvent) -> Handled {
    run_event_pass(
        root,
        Some(owner),
        event,
        EventRoute::TargetOnly,
        None,
        |widget, ctx, props, event| widget.on_text_event(ctx, props, event),
        true,
    )
}

pub(crate) fn end_ime_session(root: &mut RenderRoot) {
    if !root.global_state.is_ime_active() {
        return;
    }
    root.global_state.set_is_ime_active(false);
    root.global_state.emit_signal(RenderRootSignal::EndIme);
    if let Some(owner) = root.global_state.focused_widget
        && root.has_widget(owner)
    {
        dispatch_ime(root, owner, &TextEvent::Ime(Ime::Disabled));
    }
}

// --- MARK: TEXT EVENT
/// See the [passes documentation](crate::doc::pass_system#event-passes).
pub(crate) fn run_on_text_event_pass(root: &mut RenderRoot, event: &TextEvent) -> Handled {
    if let TextEvent::WindowFocusChange(focused) = event {
        root.global_state.set_window_focused(*focused);
    } else if !root.global_state.window_focused() {
        return Handled::No;
    }

    if matches!(event, TextEvent::Ime(_)) {
        if root.global_state.is_ime_active()
            && let Some(owner) = root.global_state.focused_widget
            && root.is_still_interactive(owner)
        {
            return dispatch_ime(root, owner, event);
        }
        return Handled::No;
    }

    if matches!(event, TextEvent::WindowFocusChange(false)) {
        end_ime_session(root);
        run_on_pointer_event_pass(
            root,
            &PointerEvent::Cancel(PointerInfo {
                pointer_id: None,
                persistent_device_id: None,
                pointer_type: PointerType::default(),
            }),
        );
    }

    let _span = info_span!("dispatch_text_event").entered();

    trace!("Running ON_TEXT_EVENT pass with {}", event.short_name());

    let target = root.global_state.focused_widget.or_else(|| {
        if let Some(focus_fallback) = root.global_state.focus_fallback
            && root.is_still_interactive(focus_fallback)
        {
            Some(focus_fallback)
        } else {
            None
        }
    });

    let mut handled = run_event_pass(
        root,
        target,
        event,
        EventRoute::Bubble,
        None,
        |widget, ctx, props, event| {
            widget.on_text_event(ctx, props, event);
        },
        true,
    );

    if handled == Handled::No {
        handled = handle_global_shortcut(root, event);
    }

    trace!(
        focused_widget = root.global_state.focused_widget.map(|id| id.0),
        handled = handled.is_handled(),
        "ON_TEXT_EVENT finished",
    );

    handled
}

fn handle_global_shortcut(root: &mut RenderRoot, event: &TextEvent) -> Handled {
    let TextEvent::Keyboard(key) = event else {
        return Handled::No;
    };
    if key.state != KeyState::Down {
        return Handled::No;
    }
    match key.key {
        Key::Named(NamedKey::Tab) => {
            root.global_state.next_focused_widget =
                find_next_focusable(root, !key.modifiers.shift());
        }
        Key::Named(NamedKey::F11) if !key.repeat => {
            root.global_state.inspector_state.is_picking_widget =
                !root.global_state.inspector_state.is_picking_widget;
            root.global_state.inspector_state.hovered_widget = None;
            if let Some(state) = root.root_state_mut() {
                state.set_needs_paint(true);
            }
        }
        Key::Named(NamedKey::F12) if !key.repeat => {
            root.global_state
                .set_debug_paint(!root.global_state.debug_paint());
            if let Some(state) = root.root_state_mut() {
                state.set_needs_paint(true);
            }
        }
        _ => return Handled::No,
    }
    Handled::Yes
}

// --- MARK: ACCESS EVENT
/// See the [passes documentation](crate::doc::pass_system#event-passes).
pub(crate) fn run_on_access_event_pass(
    root: &mut RenderRoot,
    event: &AccessEvent,
    target: WidgetId,
) -> Handled {
    let _span = info_span!("access_event").entered();
    trace!("Running ON_ACCESS_EVENT pass with {}", event.short_name());

    // Accessibility events may be queued against a previous tree revision.
    // A hidden target must not receive an event or activate a visible ancestor.
    let Some(state) = root.widget_arena.get_state(target) else {
        return Handled::No;
    };
    if state.is_stashed() {
        return Handled::No;
    }

    let mut handled = if state.is_disabled() {
        // A disabled node may still be inspected or scrolled into view, but
        // it must not turn a stale activation into a click on its ancestor.
        Handled::No
    } else {
        run_event_pass(
            root,
            Some(target),
            event,
            EventRoute::Bubble,
            None,
            |widget, ctx, props, event| {
                widget.on_access_event(ctx, props, event);
            },
            true,
        )
    };

    // Handle focus events
    match event.action {
        accesskit::Action::Focus if !handled.is_handled() && root.is_still_interactive(target) => {
            root.global_state.next_focused_widget = Some(target);
            handled = Handled::Yes;
        }
        accesskit::Action::Blur
            if !handled.is_handled() && root.global_state.next_focused_widget == Some(target) =>
        {
            root.global_state.next_focused_widget = None;
            handled = Handled::Yes;
        }
        accesskit::Action::ScrollIntoView if !handled.is_handled() => {
            if let Some(widget_state) = root.widget_arena.get_state(target) {
                let rect = widget_state.border_box();
                root.global_state
                    .scroll_request_targets
                    .push((target, rect));
                handled = Handled::Yes;
            }
        }
        _ => {}
    }

    trace!(
        focused_widget = root.global_state.focused_widget.map(|id| id.0),
        handled = handled.is_handled(),
        "ON_ACCESS_EVENT finished",
    );

    handled
}
