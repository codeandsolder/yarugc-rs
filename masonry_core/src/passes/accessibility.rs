// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use accesskit::{Node, NodeId, Role, Tree, TreeId, TreeUpdate};
use kurbo::Rect;
use tracing::{info_span, trace};
use tree_arena::ArenaMut;

use crate::app::{RenderRoot, RenderRootState};
use crate::core::{
    AccessCtx, DefaultProperties, PropertiesRef, PropertyArena, Widget, WidgetArenaNode,
};
use crate::passes::{enter_span_if, recurse_on_children};

// --- MARK: BUILD TREE
fn build_accessibility_tree(
    global_state: &mut RenderRootState,
    default_properties: &DefaultProperties,
    property_arena: &PropertyArena,
    tree_update: &mut TreeUpdate,
    node: ArenaMut<'_, WidgetArenaNode>,
    scale_factor: Option<f64>,
) {
    let mut children = node.children;
    let widget = &mut *node.item.widget;
    let state = &mut node.item.state;
    let properties = &mut node.item.properties;
    let class_set = &node.item.class_set;
    let id = state.id;
    let _span = enter_span_if(
        global_state.trace.enabled(crate::passes::TracePass::Access),
        state,
    );

    if !state.needs_accessibility() {
        return;
    }

    if state.request_accessibility() && !state.is_stashed() {
        if global_state.trace.enabled(crate::passes::TracePass::Access) {
            trace!(
                "Building accessibility node for widget '{}' {}",
                widget.short_type_name(),
                id,
            );
        }

        let stack = property_arena.get(state.property_stack_id, widget.type_id());
        let mut ctx = AccessCtx {
            global_state,
            widget_state: state,
            children: children.reborrow_mut(),
            tree_update,
        };
        let mut node = build_access_node(widget, &ctx, scale_factor);
        let props = PropertiesRef {
            local: properties,
            default_map: default_properties.for_widget(widget.type_id()),
            stack,
            class_set,
        };
        widget.accessibility(&mut ctx, &props, &mut node);

        let id: NodeId = ctx.widget_state.id.into();
        if ctx
            .global_state
            .trace
            .enabled(crate::passes::TracePass::Access)
        {
            trace!("Built node {} with role={:?}", id.0, node.role());
        }
        ctx.tree_update.nodes.push((id, node));
    }

    state.set_request_accessibility(false);
    state.set_needs_accessibility(false);

    let parent_state = state;
    recurse_on_children(id, widget, children, |mut node| {
        build_accessibility_tree(
            global_state,
            default_properties,
            property_arena,
            tree_update,
            node.reborrow_mut(),
            None,
        );
        parent_state.merge_up(&node.item.state);
    });
}

// --- MARK: BUILD NODE
fn build_access_node(widget: &dyn Widget, ctx: &AccessCtx<'_>, scale_factor: Option<f64>) -> Node {
    let mut node = Node::new(widget.accessibility_role());
    node.set_bounds(to_accesskit_rect(ctx.widget_state.border_box()));

    let mut local_transform = ctx.widget_state.compose_local_transform();

    // TODO - Remove once Masonry uses physical coordinates.
    // See https://github.com/linebender/xilem/issues/1264
    if let Some(scale_factor) = scale_factor {
        local_transform = local_transform.pre_scale(scale_factor);
    }
    node.set_transform(accesskit::Affine::new(local_transform.as_coeffs()));

    node.set_children(
        widget
            .children_ids()
            .into_iter()
            .filter_map(|id| {
                let child = ctx.children.item(id)?;
                (!child.item.state.is_stashed()).then_some(id.into())
            })
            .collect::<Vec<NodeId>>(),
    );

    if ctx.is_stashed() {
        debug_panic!("build_access_node called for stashed widget");
    }

    // Note - The values returned by these methods can be modified by other passes.
    // When that happens, the other pass should set flags to request an accessibility pass.
    if ctx.is_disabled() {
        node.set_disabled();
    }
    if ctx.widget_state.clip_path.is_some() {
        node.set_clips_children();
    }
    if ctx.accepts_focus() && !ctx.is_disabled() {
        node.add_action(accesskit::Action::Focus);
    }
    if ctx.is_focus_target() {
        node.add_action(accesskit::Action::Blur);
    }

    node
}

const fn to_accesskit_rect(r: Rect) -> accesskit::Rect {
    accesskit::Rect::new(r.x0, r.y0, r.x1, r.y1)
}

// --- MARK: ROOT
/// See the [passes documentation](crate::doc::pass_system#render-passes).
pub(crate) fn run_accessibility_pass(root: &mut RenderRoot, scale_factor: f64) -> TreeUpdate {
    let _span = info_span!("accessibility").entered();

    let mut tree_update = TreeUpdate {
        tree_id: TreeId::ROOT,
        nodes: vec![],
        tree: Some(Tree {
            root: root.global_state.window_node_id,
            toolkit_name: Some("Masonry".to_string()),
            toolkit_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
        focus: root
            .global_state
            .focused_widget
            .map_or(root.global_state.window_node_id, Into::into),
    };

    // TODO: make root node type customizable to support Dialog/AlertDialog roles
    let mut window_node = Node::new(Role::Window);
    let root_id = root.root_id();
    if let Some(root_node) = root.widget_arena.nodes.find_mut(root_id) {
        build_accessibility_tree(
            &mut root.global_state,
            &root.property_arena.default_properties,
            &root.property_arena,
            &mut tree_update,
            root_node,
            Some(scale_factor),
        );
        window_node.set_children(vec![root_id.into()]);
    } else {
        tree_update.focus = root.global_state.window_node_id;
    }
    tree_update
        .nodes
        .push((root.global_state.window_node_id, window_node));

    tree_update
}
