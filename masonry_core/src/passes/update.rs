// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashSet;

use tracing::{info_span, trace};
use tree_arena::{ArenaMut, ArenaMutList, ArenaRef};
use ui_events::pointer::PointerType;

use crate::app::{RenderRoot, RenderRootSignal, RenderRootState};
use crate::core::{
    ClassSetDiff, CursorIcon, DefaultProperties, Ime, PointerEvent, PointerInfo, PropertiesMut,
    PropertyArena, PropertyCache, PseudoClass, RegisterCtx, TextEvent, Update, UpdateCtx, Widget,
    WidgetArenaNode, WidgetId, WidgetState,
};
use crate::passes::event::{run_on_pointer_event_pass, run_on_text_event_pass};
use crate::passes::{enter_span, enter_span_if, merge_state_up, recurse_on_children};
use crate::properties::core_property_changed;
use crate::util::ParentLinkedList;

// --- MARK: HELPERS
/// Returns the id path starting from the given widget id and ending at the root.
///
/// If `widget_id` is `None`, returns an empty `Vec`.
fn get_id_path(root: &RenderRoot, widget_id: Option<WidgetId>) -> Vec<WidgetId> {
    widget_id.map_or_else(Vec::new, |id| root.widget_path(id))
}

fn is_ancestor_of(root: &RenderRoot, ancestor_id: WidgetId, widget_id: Option<WidgetId>) -> bool {
    let Some(id) = widget_id else {
        return false;
    };
    root.widget_arena
        .nodes
        .get_id_path(id)
        .contains(&ancestor_id.to_raw())
}

/// Make a dummy [`PointerEvent::Cancel`].
fn dummy_pointer_cancel() -> PointerEvent {
    PointerEvent::Cancel(PointerInfo {
        pointer_id: None,
        persistent_device_id: None,
        pointer_type: PointerType::default(),
    })
}

// Construct the same update context for single-node and ancestor-chain passes.
fn update_widget(
    root: &mut RenderRoot,
    target: WidgetId,
    pass_fn: &mut impl FnMut(&mut dyn Widget, &mut UpdateCtx<'_>, &mut PropertiesMut<'_>),
) -> bool {
    let Some(node) = root.widget_arena.nodes.find_mut(target) else {
        return false;
    };

    let children = node.children;
    let widget = &mut *node.item.widget;
    let state = &mut node.item.state;
    let properties = &mut node.item.properties;
    let class_set = &node.item.class_set;
    let stack = root
        .property_arena
        .get(state.property_stack_id, widget.type_id());

    let mut ctx = UpdateCtx {
        global_state: &mut root.global_state,
        widget_state: state,
        children,
        ancestors: None,
        property_arena: &root.property_arena,
    };
    let mut props = PropertiesMut {
        local: properties,
        default_map: root
            .property_arena
            .default_properties
            .for_widget(widget.type_id()),
        stack,
        class_set,
    };
    pass_fn(widget, &mut ctx, &mut props);
    true
}

fn run_targeted_update_pass(
    root: &mut RenderRoot,
    target: Option<WidgetId>,
    mut pass_fn: impl FnMut(&mut dyn Widget, &mut UpdateCtx<'_>, &mut PropertiesMut<'_>),
) {
    let mut current_id = target;
    while let Some(widget_id) = current_id {
        let parent_id = root.widget_arena.parent_of(widget_id);
        if !update_widget(root, widget_id, &mut pass_fn) {
            break;
        }
        merge_state_up(&mut root.widget_arena, widget_id);
        current_id = parent_id;
    }
}

fn run_single_update_pass(
    root: &mut RenderRoot,
    target: Option<WidgetId>,
    mut pass_fn: impl FnMut(&mut dyn Widget, &mut UpdateCtx<'_>, &mut PropertiesMut<'_>),
) {
    let Some(target) = target else {
        return;
    };
    if !update_widget(root, target, &mut pass_fn) {
        return;
    }
    let mut current_id = Some(target);
    while let Some(widget_id) = current_id {
        merge_state_up(&mut root.widget_arena, widget_id);
        current_id = root.widget_arena.parent_of(widget_id);
    }
}

#[derive(Clone, Copy, Debug)]
enum PathStatus {
    Active,
    Hovered,
    Focused,
}

impl PathStatus {
    const fn has(self, state: &WidgetState) -> bool {
        match self {
            Self::Active => state.has_active(),
            Self::Hovered => state.has_hovered(),
            Self::Focused => state.has_focus_target(),
        }
    }

    fn update_descendant(
        self,
        widget: &mut dyn Widget,
        ctx: &mut UpdateCtx<'_>,
        props: &mut PropertiesMut<'_>,
        value: bool,
    ) {
        if self.has(ctx.widget_state) != value {
            let event = match self {
                Self::Active => Update::ChildActiveChanged(value),
                Self::Hovered => Update::ChildHoveredChanged(value),
                Self::Focused => Update::ChildFocusChanged(value),
            };
            widget.update(ctx, props, &event);
            if matches!(self, Self::Focused) {
                ctx.widget_state
                    .class_diff
                    .pseudo
                    .set(PseudoClass::Focused, value);
                ctx.widget_state.set_request_update_props(true);
                ctx.widget_state.set_needs_update_props(true);
            }
        }
        match self {
            Self::Active => ctx.widget_state.set_has_active(value),
            Self::Hovered => ctx.widget_state.set_has_hovered(value),
            Self::Focused => ctx.widget_state.set_has_focus_target(value),
        }
    }
}

// Walk the old path before the new path, bottom-up. Propagating a changed
// node to its ancestors updates common ancestors only when necessary.
fn update_path_status(
    root: &mut RenderRoot,
    previous: &[WidgetId],
    next: &[WidgetId],
    status: PathStatus,
) {
    if previous == next {
        return;
    }
    let next_set: HashSet<_> = next.iter().copied().collect();
    trace!(?status, ?previous, ?next, "Widget status path changed");
    for id in previous.iter().chain(next).copied() {
        let should_update = root
            .widget_arena
            .nodes
            .find(id)
            .is_some_and(|node| status.has(&node.item.state) != next_set.contains(&id));
        if should_update {
            run_targeted_update_pass(root, Some(id), |widget, ctx, props| {
                let value = next_set.contains(&ctx.widget_id());
                status.update_descendant(widget, ctx, props, value);
            });
        }
    }
}

#[derive(Clone, Copy)]
enum PointerStatus {
    Active,
    Hovered,
}

impl PointerStatus {
    fn update(self, root: &mut RenderRoot, previous: Option<WidgetId>, next: Option<WidgetId>) {
        if previous == next {
            return;
        }
        for (target, value) in [(previous, false), (next, true)] {
            run_single_update_pass(root, target, |widget, ctx, props| {
                let state = &mut *ctx.widget_state;
                let (pseudo, event) = match self {
                    Self::Active => {
                        state.set_is_active(value);
                        (PseudoClass::Active, Update::ActiveChanged(value))
                    }
                    Self::Hovered => {
                        state.set_is_hovered(value);
                        (PseudoClass::Hovered, Update::HoveredChanged(value))
                    }
                };
                state.class_diff.pseudo.set(pseudo, value);
                state.set_request_update_props(true);
                state.set_needs_update_props(true);
                widget.update(ctx, props, &event);
            });
        }
    }
}

// --- MARK: TREE
fn update_widget_tree(
    global_state: &mut RenderRootState,
    default_properties: &DefaultProperties,
    property_arena: &PropertyArena,
    node: ArenaMut<'_, WidgetArenaNode>,
    ancestors: Option<&ParentLinkedList<'_>>,
) {
    let mut children = node.children;
    let widget = &mut *node.item.widget;
    let state = &mut node.item.state;
    let properties = &mut node.item.properties;
    let class_set = &node.item.class_set;
    let id = state.id;

    let trace = global_state
        .trace
        .enabled(crate::passes::TracePass::UpdateTree);
    let _span = enter_span_if(trace, state);

    if !state.children_changed() {
        return;
    }

    register_children(global_state, widget, state, children.reborrow_mut());

    if state.is_new() {
        let stack = property_arena.get(state.property_stack_id, widget.type_id());
        let mut ctx = UpdateCtx {
            global_state,
            widget_state: state,
            children: children.reborrow_mut(),
            ancestors,
            property_arena,
        };
        let mut props = PropertiesMut {
            local: properties,
            default_map: default_properties.for_widget(widget.type_id()),
            stack,
            class_set,
        };
        widget.update(&mut ctx, &mut props, &Update::WidgetAdded);
        if trace {
            trace!("{} received Update::WidgetAdded", widget.short_type_name());
        }
        state.set_accepts_pointer_interaction(widget.accepts_pointer_interaction());
        state.set_propagates_pointer_interaction(widget.propagates_pointer_interaction());
        state.set_accepts_focus(widget.accepts_focus());
        state.set_accepts_text_input(widget.accepts_text_input());
        state.trace_span = widget.make_trace_span(state.id);
        state.set_is_new(false);

        if state.children_changed() {
            register_children(global_state, widget, state, children.reborrow_mut());
        }
    }

    // We can recurse on this widget's children, because they have already been added
    // to the arena above.
    let parent_state = state;
    recurse_on_children(id, widget, children, |mut node| {
        let ancestors = ParentLinkedList {
            widget,
            id,
            parent: ancestors,
        };
        update_widget_tree(
            global_state,
            default_properties,
            property_arena,
            node.reborrow_mut(),
            Some(&ancestors),
        );
        parent_state.merge_up(&node.item.state);
    });
}

fn register_children(
    global_state: &mut RenderRootState,
    widget: &mut (dyn Widget + 'static),
    state: &mut WidgetState,
    mut children: ArenaMutList<'_, WidgetArenaNode>,
) {
    state.set_children_changed(false);

    let mut ctx = RegisterCtx {
        global_state,
        children: children.reborrow_mut(),
        #[cfg(debug_assertions)]
        registered_ids: Vec::new(),
    };
    // The widget will call `RegisterCtx::register_child` on all its children,
    // which will add the new widgets to the arena.
    widget.register_children(&mut ctx);

    #[cfg(debug_assertions)]
    {
        let children_ids = widget.children_ids();
        for child_id in ctx.registered_ids {
            if !children_ids.contains(&child_id) {
                debug_panic!(
                    "Error in '{}' {}: method register_children() called \
                    RegisterCtx::register_child() on child {}, which isn't \
                    in the list returned by children_ids()",
                    widget.short_type_name(),
                    state.id,
                    child_id
                );
            }
        }
    }

    #[cfg(debug_assertions)]
    for child_id in widget.children_ids() {
        if !children.has(child_id) {
            debug_panic!(
                "Error in '{}' {}: method register_children() did not call \
                RegisterCtx::register_child() on child {} returned by children_ids()",
                widget.short_type_name(),
                state.id,
                child_id
            );
        }
    }
}

/// See the [passes documentation](crate::doc::pass_system#update-tree-pass).
pub(crate) fn run_update_widget_tree_pass(root: &mut RenderRoot) {
    let _span = info_span!("update_new_widgets").entered();

    // INIT TREE
    if root.layer_stack.incomplete() {
        let mut ctx = RegisterCtx {
            global_state: &mut root.global_state,
            children: root.widget_arena.nodes.roots_mut(),
            #[cfg(debug_assertions)]
            registered_ids: Vec::new(),
        };
        ctx.register_child(&mut root.layer_stack);
    }

    let Some(root_node) = root.widget_arena.nodes.find_mut(root.root_id()) else {
        return;
    };
    update_widget_tree(
        &mut root.global_state,
        &root.property_arena.default_properties,
        &root.property_arena,
        root_node,
        None,
    );
}

// ----------------

fn notify_inherited_state_change(
    global_state: &mut RenderRootState,
    default_properties: &DefaultProperties,
    property_arena: &PropertyArena,
    state: &mut WidgetState,
    children: ArenaMutList<'_, WidgetArenaNode>,
    (widget, properties, class_set): (
        &mut dyn Widget,
        &mut crate::core::PropertySet,
        &crate::core::ClassSet,
    ),
    event: &Update,
) {
    let stack = property_arena.get(state.property_stack_id, widget.type_id());
    let mut ctx = UpdateCtx {
        global_state,
        widget_state: state,
        children,
        ancestors: None,
        property_arena,
    };
    let mut props = PropertiesMut {
        local: properties,
        default_map: default_properties.for_widget(widget.type_id()),
        stack,
        class_set,
    };
    widget.update(&mut ctx, &mut props, event);
}

// --- MARK: DISABLED
/// See the [passes documentation](crate::doc::pass_system#update-passes).
/// See the [disabled status documentation](../doc/06_masonry_concepts.md#disabled).
fn update_disabled_for_widget(
    global_state: &mut RenderRootState,
    default_properties: &DefaultProperties,
    property_arena: &PropertyArena,
    node: ArenaMut<'_, WidgetArenaNode>,
    parent_disabled: bool,
) {
    let mut children = node.children;
    let widget = &mut *node.item.widget;
    let state = &mut node.item.state;
    let properties = &mut node.item.properties;
    let class_set = &node.item.class_set;
    let id = state.id;

    let _span = enter_span(state);

    let disabled = state.is_explicitly_disabled() || parent_disabled;
    if !state.needs_update_disabled() && disabled == state.is_disabled() {
        return;
    }

    if disabled != state.is_disabled() {
        notify_inherited_state_change(
            global_state,
            default_properties,
            property_arena,
            state,
            children.reborrow_mut(),
            (widget, properties, class_set),
            &Update::DisabledChanged(disabled),
        );
        state.set_is_disabled(disabled);
        state.class_diff.pseudo.set(PseudoClass::Disabled, disabled);
        state.set_request_update_props(true);
        state.set_needs_update_props(true);
        state.set_needs_update_focusable(true);
        state.set_request_accessibility(true);
        state.set_needs_accessibility(true);
    }

    state.set_needs_update_disabled(false);

    let parent_state = state;
    recurse_on_children(id, widget, children, |mut node| {
        update_disabled_for_widget(
            global_state,
            default_properties,
            property_arena,
            node.reborrow_mut(),
            disabled,
        );
        parent_state.merge_up(&node.item.state);
    });
}

pub(crate) fn run_update_disabled_pass(root: &mut RenderRoot) {
    let _span = info_span!("update_disabled").entered();

    // If a widget was enabled or disabled, the pointer icon may need to change.
    if root
        .root_state()
        .is_some_and(WidgetState::needs_update_disabled)
    {
        root.global_state.set_needs_pointer_pass(true);
    }

    let Some(root_node) = root.widget_arena.nodes.find_mut(root.root_id()) else {
        return;
    };
    update_disabled_for_widget(
        &mut root.global_state,
        &root.property_arena.default_properties,
        &root.property_arena,
        root_node,
        false,
    );
}

// ----------------

// *Stashed* is for widgets that are no longer "part of the graph". So they can't get keyboard events, don't get painted, etc, but should keep some state.
// Scrolled-out widgets are *not* stashed.

// --- MARK: STASHED
/// See the [passes documentation](crate::doc::pass_system#update-passes).
/// See the [stashed status documentation](../doc/06_masonry_concepts.md#stashed).
fn update_stashed_for_widget(
    global_state: &mut RenderRootState,
    default_properties: &DefaultProperties,
    property_arena: &PropertyArena,
    node: ArenaMut<'_, WidgetArenaNode>,
    parent_stashed: bool,
) {
    let mut children = node.children;
    let widget = &mut *node.item.widget;
    let state = &mut node.item.state;
    let properties = &mut node.item.properties;
    let class_set = &node.item.class_set;
    let id = state.id;

    let _span = enter_span(state);

    let stashed = state.is_explicitly_stashed() || parent_stashed;
    if !state.needs_update_stashed() && stashed == state.is_stashed() {
        return;
    }

    if stashed != state.is_stashed() {
        notify_inherited_state_change(
            global_state,
            default_properties,
            property_arena,
            state,
            children.reborrow_mut(),
            (widget, properties, class_set),
            &Update::StashedChanged(stashed),
        );
        state.set_is_stashed(stashed);
        state.set_needs_update_focusable(true);

        // Items may have been changed while they were stashed in ways that require a
        // relayout and a re-render.
        if !stashed {
            state.set_needs_layout(true);
            state.set_request_layout(true);
            state.set_request_paint(true);
            state.set_needs_paint(true);
            state.set_needs_accessibility(true);
            state.set_request_accessibility(true);
        }
    }

    state.set_needs_update_stashed(false);

    let parent_state = state;
    recurse_on_children(id, widget, children, |mut node| {
        update_stashed_for_widget(
            global_state,
            default_properties,
            property_arena,
            node.reborrow_mut(),
            stashed,
        );
        parent_state.merge_up(&node.item.state);
    });
}

pub(crate) fn run_update_stashed_pass(root: &mut RenderRoot) {
    let _span = info_span!("update_stashed").entered();

    let Some(root_node) = root.widget_arena.nodes.find_mut(root.root_id()) else {
        return;
    };
    update_stashed_for_widget(
        &mut root.global_state,
        &root.property_arena.default_properties,
        &root.property_arena,
        root_node,
        false,
    );
}

// ----------------

// --- MARK: FOCUSABLE

fn update_focusable_for_widget(node: ArenaMut<'_, WidgetArenaNode>) {
    let children = node.children;
    let widget = &mut *node.item.widget;
    let state = &mut node.item.state;
    let id = state.id;

    let _span = enter_span(state);

    if !state.needs_update_focusable() {
        return;
    }

    state.set_descendant_is_focusable(false);

    if state.accepts_focus() && !state.is_disabled() && !state.is_stashed() {
        state.set_descendant_is_focusable(true);
    }

    let parent_state = &mut *state;
    recurse_on_children(id, widget, children, |mut node| {
        update_focusable_for_widget(node.reborrow_mut());

        if node.item.state.descendant_is_focusable() {
            parent_state.set_descendant_is_focusable(true);
        }
    });

    state.set_needs_update_focusable(false);
}

pub(crate) fn run_update_focusable_pass(root: &mut RenderRoot) {
    let _span = info_span!("update_focusable").entered();

    let Some(root_node) = root.widget_arena.nodes.find_mut(root.root_id()) else {
        return;
    };
    update_focusable_for_widget(root_node);
}

pub(crate) fn find_next_focusable(root: &RenderRoot, forward: bool) -> Option<WidgetId> {
    let node = root.widget_arena.nodes.find(root.root_id())?;
    let anchor = root
        .global_state
        .focus_anchor
        .filter(|id| root.has_widget(*id));

    if let Some(anchor) = anchor {
        // The anchor path is ordered from the anchor towards the root. Exclude
        // the root itself before descending it.
        let mut path = root.widget_path(anchor);
        path.pop();
        if let Some(found) = find_first_focusable(node, &path, Some(anchor), forward) {
            return Some(found);
        }
    }

    // Wrap at either end of the focus order.
    find_first_focusable(node, &[], None, forward)
}

fn find_first_focusable(
    node: ArenaRef<'_, WidgetArenaNode>,
    anchor_path: &[WidgetId],
    anchor: Option<WidgetId>,
    forward: bool,
) -> Option<WidgetId> {
    let state = &node.item.state;
    if !state.descendant_is_focusable() {
        return None;
    }

    let is_anchor = anchor == Some(state.id);
    let can_focus =
        state.accepts_focus() && !state.is_disabled() && !state.is_stashed() && !is_anchor;
    let children = node.item.widget.children_ids();

    if forward && anchor_path.is_empty() && can_focus {
        return Some(state.id);
    }

    let candidates = if let Some((anchor_child, remaining)) = anchor_path.split_last() {
        let index = children.iter().position(|id| id == anchor_child)?;
        if let Some(child) = node.children.item(*anchor_child)
            && let Some(found) = find_first_focusable(child, remaining, anchor, forward)
        {
            return Some(found);
        }
        if forward {
            children.get(index + 1..)?
        } else {
            children.get(..index)?
        }
    } else if is_anchor && !forward {
        // Reverse preorder: descendants occur after the anchor, not before it.
        &[]
    } else {
        &children[..]
    };

    let visit =
        |id: &WidgetId| find_first_focusable(node.children.item(*id)?, &[], anchor, forward);
    if forward {
        candidates.iter().find_map(visit)
    } else {
        candidates
            .iter()
            .rev()
            .find_map(visit)
            .or_else(|| can_focus.then_some(state.id))
    }
}

// ----------------

// --- MARK: FOCUS
/// See the [passes documentation](crate::doc::pass_system#update-passes).
/// See the [focus status documentation](../doc/06_masonry_concepts.md#text-focus).
pub(crate) fn run_update_focus_pass(root: &mut RenderRoot) {
    let _span = info_span!("update_focus").entered();
    // If the next-focused widget is disabled, stashed or removed, we set
    // the focused id to None
    if let Some(id) = root.global_state.next_focused_widget
        && !root.is_still_interactive(id)
    {
        root.global_state.next_focused_widget = None;
    }
    // We are more permissive with the anchor.
    if let Some(id) = root.global_state.focus_anchor
        && !root.has_widget(id)
    {
        root.global_state.focus_anchor = None;
    }

    if let Some(id) = root.global_state.focus_fallback
        && !root.is_still_interactive(id)
    {
        root.global_state.focus_fallback = None;
    }

    let prev_focused = root.global_state.focused_widget;
    let was_ime_active = root.global_state.is_ime_active();

    if was_ime_active && prev_focused != root.global_state.next_focused_widget {
        // IME was active, but the next focused widget is going to receive the Ime::Disabled event
        // sent by the platform. Synthesize an `Ime::Disabled` event here and send it to the widget
        // about to be unfocused.

        // HACK: It's not valid to send an event to a non-existent widget, so we check that the "previously"
        // focused widget hasn't just been deleted.
        // This means that if a parent widget was handling IME events, it won't get this event.
        // We know that IME events bubbling is not the correct behaviour, but have chosen to keep it for consistency,
        // as we also are planning to refactor how IME is delivered as we update to use Android View.
        if let Some(prev_focused) = prev_focused
            && root.has_widget(prev_focused)
        {
            run_on_text_event_pass(root, &TextEvent::Ime(Ime::Disabled));
        }

        // Disable the IME, which was enabled specifically for this widget. Note that if the newly
        // focused widget also requires IME, we will request it again - this resets the platform's
        // state, ensuring that partial IME inputs do not "travel" between widgets
        root.global_state.emit_signal(RenderRootSignal::EndIme);

        // Note: handling of the Ime::Disabled event sent above may have changed the next focused
        // widget. In particular, focus may have changed back to the original widget we just
        // disabled IME for.
        //
        // In this unlikely case, the rest of this handler will short-circuit, and IME would not be
        // re-enabled for this widget. Re-enable IME here; the resultant `Ime::Enabled` event sent
        // by the platform will be routed to this widget as it remains the focused widget. We don't
        // handle this as above to avoid loops.
        //
        // First do the disabled, stashed or removed check again.
        if let Some(id) = root.global_state.next_focused_widget
            && !root.is_still_interactive(id)
        {
            root.global_state.next_focused_widget = None;
        }
        if prev_focused == root.global_state.next_focused_widget {
            tracing::warn!(
                id = prev_focused.map(WidgetId::trace),
                "request_focus called whilst handling Ime::Disabled"
            );
            root.global_state.emit_signal(RenderRootSignal::StartIme);
        }
    }

    let next_focused = root.global_state.next_focused_widget;

    // "Focused path" means the focused widget, and all its parents.
    let prev_focused_path = std::mem::take(&mut root.global_state.focused_path);
    let next_focused_path = get_id_path(root, next_focused);

    update_path_status(
        root,
        &prev_focused_path,
        &next_focused_path,
        PathStatus::Focused,
    );

    // Refocus if the focused widget changed.
    if prev_focused != next_focused {
        // We send FocusChange event to widget that lost and the widget that gained focus.
        // We also request accessibility, because build_access_node() depends on the focus state.
        run_single_update_pass(root, prev_focused, |widget, ctx, props| {
            widget.update(ctx, props, &Update::FocusChanged(false));
            ctx.widget_state.set_request_accessibility(true);
            ctx.widget_state.set_needs_accessibility(true);
        });
        run_single_update_pass(root, next_focused, |widget, ctx, props| {
            widget.update(ctx, props, &Update::FocusChanged(true));
            ctx.widget_state.set_request_accessibility(true);
            ctx.widget_state.set_needs_accessibility(true);
        });

        let accepts_text = next_focused
            .and_then(|id| root.widget_arena.get_state(id))
            .is_some_and(WidgetState::accepts_text_input);
        root.global_state.set_is_ime_active(accepts_text);
        if accepts_text {
            root.global_state.emit_signal(RenderRootSignal::StartIme);
        }
    }

    if next_focused.is_some() {
        root.global_state.focus_anchor = next_focused;
    }

    root.global_state.focused_widget = next_focused;
    root.global_state.focused_path = next_focused_path;
}

// ----------------

// --- MARK: SCROLL
// This pass will update scroll positions in cases where a widget has requested to be
// scrolled into view (usually a text input getting text events).
// Each parent that implements scrolling will update its scroll position to ensure the
// child is visible. (If the target area is larger than the parent, the parent will try
// to show the top left of that area.)
/// See the [passes documentation](crate::doc::pass_system#update-passes).
pub(crate) fn run_update_scroll_pass(root: &mut RenderRoot) {
    let _span = info_span!("update_scroll").entered();

    let scroll_request_targets = std::mem::take(&mut root.global_state.scroll_request_targets);
    for (target, rect) in scroll_request_targets {
        // We start with target_rect being in the target's border-box coordinate space.
        let mut target_rect = rect;

        // We run the update pass on the target itself and then its ancestors.
        run_targeted_update_pass(root, Some(target), |widget, ctx, props| {
            // Convert the target_rect from border-box space to content-box space.
            let local_rect = target_rect - ctx.widget_state.border_box_translation();

            let event = Update::RequestPanToChild(local_rect);
            widget.update(ctx, props, &event);

            // TODO - We should run the compose method after this, so
            // translations are updated and the rect passed to parents
            // is more accurate. Until then we don't add scroll_translation
            // at all and only support a single scrolling parent.

            // Before continuing to the parent, we need to convert the target_rect from this
            // widget's border-box coordinate space to the parent's border-box coordinate space.
            let state = &ctx.widget_state;
            target_rect = target_rect + state.origin.to_vec2();
        });
    }
}

// ----------------

// --- MARK: POINTER
/// See the [passes documentation](crate::doc::pass_system#update-passes).
pub(crate) fn run_update_pointer_pass(root: &mut RenderRoot) {
    if !root.global_state.needs_pointer_pass() {
        return;
    }
    let _span = info_span!("update_pointer").entered();

    root.global_state.set_needs_pointer_pass(false);

    let pointer_pos = root
        .global_state
        .last_mouse_pos
        .map(|pos| (pos.x, pos.y).into());

    if root.global_state.inspector_state.is_picking_widget {
        if let Some(pos) = pointer_pos {
            root.global_state.inspector_state.hovered_widget = root
                .get_widget(root.root_id())
                .and_then(|widget| widget.find_widget_under_pointer(pos))
                .map(|widget| widget.id());
        }
        if let Some(state) = root.root_state_mut() {
            state.set_needs_paint(true);
        }
        return;
    }

    // Release pointer capture if target can no longer hold it.
    if let Some(id) = root.global_state.pointer_capture_target
        && !root.is_still_interactive(id)
    {
        // The event pass will set pointer_capture_target to None.
        run_on_pointer_event_pass(root, &dummy_pointer_cancel());
    }

    // -- UPDATE ACTIVE --
    // "Active path" means the widget which is considered active, and all its parents.
    let prev_active_path = std::mem::take(&mut root.global_state.active_path);
    let prev_active_widget = prev_active_path.first().copied();
    let next_active_widget = root.global_state.pointer_capture_target;
    let next_active_path = get_id_path(root, next_active_widget);

    update_path_status(
        root,
        &prev_active_path,
        &next_active_path,
        PathStatus::Active,
    );
    PointerStatus::Active.update(root, prev_active_widget, next_active_widget);

    // -- UPDATE HOVERED --
    let mut next_hovered_widget = pointer_pos.and_then(|pos| {
        root.get_widget(root.root_id())
            .and_then(|widget| widget.find_widget_under_pointer(pos))
            .map(|widget| widget.id())
    });
    // If the pointer is captured, it can either hover its capture target or nothing.
    if let Some(capture_target) = root.global_state.pointer_capture_target
        && next_hovered_widget != Some(capture_target)
    {
        if is_ancestor_of(root, capture_target, next_hovered_widget) {
            next_hovered_widget = Some(capture_target);
        } else {
            next_hovered_widget = None;
        }
    }
    // Check if hovered widget is disabled
    if let Some(id) = next_hovered_widget
        && root
            .widget_arena
            .get_state(id)
            .is_none_or(WidgetState::is_disabled)
    {
        next_hovered_widget = None;
    }

    // "Hovered path" means the widget which is considered hovered, and all its parents.
    let prev_hovered_path = std::mem::take(&mut root.global_state.hovered_path);
    let next_hovered_path = get_id_path(root, next_hovered_widget);
    let prev_hovered_widget = prev_hovered_path.first().copied();

    update_path_status(
        root,
        &prev_hovered_path,
        &next_hovered_path,
        PathStatus::Hovered,
    );
    PointerStatus::Hovered.update(root, prev_hovered_widget, next_hovered_widget);

    // -- UPDATE CURSOR ICON --

    // If the pointer is captured, its icon always reflects the
    // capture target, even when not hovered.
    let icon_source = root
        .global_state
        .pointer_capture_target
        .or(next_hovered_widget);

    let new_icon = icon_source
        .zip(pointer_pos)
        .and_then(|(id, pos)| {
            let widget = root.get_widget(id)?;
            if widget.ctx().is_disabled() {
                return None;
            }
            Some(widget.get_cursor(widget.ctx(), pos))
        })
        .unwrap_or(CursorIcon::Default);

    if root.global_state.cursor_icon != new_icon {
        root.global_state
            .emit_signal(RenderRootSignal::SetCursor(new_icon));
    }

    root.global_state.cursor_icon = new_icon;
    root.global_state.hovered_path = next_hovered_path;
    root.global_state.active_path = next_active_path;
}

// ----------------

// --- MARK: PROPS
/// See the [passes documentation](crate::doc::pass_system#update-passes).
fn update_props_for_widget(
    global_state: &mut RenderRootState,
    property_arena: &PropertyArena,
    node: ArenaMut<'_, WidgetArenaNode>,
) {
    let widget_type_id = node.item.widget.as_ref().type_id();
    let stack = property_arena.get(node.item.state.property_stack_id, widget_type_id);

    let mut children = node.children;
    let widget = &mut *node.item.widget;
    let state = &mut node.item.state;
    let class_set = &mut node.item.class_set;
    let id = state.id;

    if !state.needs_update_props() {
        return;
    }

    // Consume this pass's requests before invoking user callbacks. Requests
    // made by property_changed must survive for the next rewrite pass.
    state.set_needs_update_props(false);
    if state.take_request_update_props() {
        let class_diff = std::mem::take(&mut state.class_diff);
        class_set.apply(&class_diff);

        if cached_props_changed(&class_diff, &state.property_cache) {
            let invalidated = std::mem::take(&mut state.property_cache.invalidated);
            let old_entries = std::mem::take(&mut state.property_cache.entries);
            state.property_cache.clear_dependencies();

            // Rebuild the whole dependency set before notifying widgets.
            // A changed resolution is still a cached dependency even if the
            // next paint/layout does not immediately read the property again.
            let changed: Vec<_> = old_entries
                .into_iter()
                .filter_map(|(type_id, old_index)| {
                    let new_index =
                        stack.resolve_index(class_set, type_id, &mut state.property_cache);
                    (invalidated || new_index != old_index).then_some(type_id)
                })
                .collect();

            for type_id in changed {
                let mut ctx = UpdateCtx {
                    global_state,
                    widget_state: state,
                    children: children.reborrow_mut(),
                    ancestors: None,
                    property_arena,
                };
                core_property_changed(&mut ctx, type_id);
                widget.property_changed(&mut ctx, type_id);
            }
        }
    }

    let parent_state = state;
    recurse_on_children(id, widget, children, |mut node| {
        update_props_for_widget(global_state, property_arena, node.reborrow_mut());
        parent_state.merge_up(&node.item.state);
    });
}

/// Returns `true` if any change in `diff` could affect a cached property resolution
/// tracked in `cache`, or if the cache was invalidated.
fn cached_props_changed(diff: &ClassSetDiff, cache: &PropertyCache) -> bool {
    if cache.invalidated {
        return true;
    }
    if !diff.added.is_disjoint(&cache.relevant_classes) {
        return true;
    }
    if !diff.removed.is_disjoint(&cache.relevant_classes) {
        return true;
    }

    diff.pseudo.mask.intersects(cache.relevant_pseudo)
}

pub(crate) fn run_update_props_pass(root: &mut RenderRoot) {
    let _span = info_span!("update_props").entered();

    if !root
        .root_state()
        .is_some_and(WidgetState::needs_update_props)
    {
        return;
    }

    let Some(root_node) = root.widget_arena.nodes.find_mut(root.root_id()) else {
        return;
    };
    update_props_for_widget(&mut root.global_state, &root.property_arena, root_node);
}

// ----------------

// --- MARK: FONTS
/// See the [passes documentation](crate::doc::pass_system#update-passes).
fn update_fonts_for_widget(
    global_state: &mut RenderRootState,
    default_properties: &DefaultProperties,
    property_arena: &PropertyArena,
    node: ArenaMut<'_, WidgetArenaNode>,
) {
    let mut children = node.children;
    let widget = &mut *node.item.widget;
    let state = &mut node.item.state;
    let properties = &mut node.item.properties;
    let class_set = &node.item.class_set;
    let id = state.id;

    let _span = enter_span(state);

    let stack = property_arena.get(state.property_stack_id, widget.type_id());
    let mut ctx = UpdateCtx {
        global_state,
        widget_state: state,
        children: children.reborrow_mut(),
        ancestors: None,
        property_arena,
    };
    let mut props = PropertiesMut {
        local: properties,
        default_map: default_properties.for_widget(widget.type_id()),
        stack,
        class_set,
    };
    widget.update(&mut ctx, &mut props, &Update::FontsChanged);

    let parent_state = state;
    recurse_on_children(id, widget, children, |mut node| {
        update_fonts_for_widget(
            global_state,
            default_properties,
            property_arena,
            node.reborrow_mut(),
        );
        parent_state.merge_up(&node.item.state);
    });
}

pub(crate) fn run_update_fonts_pass(root: &mut RenderRoot) {
    let _span = info_span!("update_fonts").entered();

    let Some(root_node) = root.widget_arena.nodes.find_mut(root.root_id()) else {
        return;
    };
    update_fonts_for_widget(
        &mut root.global_state,
        &root.property_arena.default_properties,
        &root.property_arena,
        root_node,
    );
}

// ----------------
