// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;

use kurbo::{Affine, Line, Stroke};
use peniko::{Color, Fill};
use tracing::{info_span, trace};
use tree_arena::ArenaMut;

use crate::app::{RenderRoot, RenderRootState, VisualLayer, VisualLayerKind, VisualLayerPlan};
use crate::core::{
    PaintCtx, PaintLayerMode, PropertiesRef, PropertyArena, WidgetArenaNode, WidgetId,
};
use crate::imaging::record::{Clip, Command, Geometry, Scene};
use crate::imaging::{PaintSink, Painter};
use crate::passes::{enter_span_if, recurse_on_children};
use crate::util::get_debug_color;

struct LayerCollector {
    current_scene: Scene,
    active_clips: Vec<Clip>,
    layers: Vec<VisualLayer>,
    current_owner_id: WidgetId,
    transform: Affine,
}

impl LayerCollector {
    fn new(root_id: WidgetId, transform: Affine) -> Self {
        Self {
            current_scene: Scene::new(),
            active_clips: Vec::new(),
            layers: Vec::new(),
            current_owner_id: root_id,
            transform,
        }
    }

    const fn scene_mut(&mut self) -> &mut Scene {
        &mut self.current_scene
    }

    fn push_clip(&mut self, clip: Clip) {
        self.current_scene.push_clip(clip.clone());
        self.active_clips.push(clip);
    }

    fn pop_clip(&mut self) {
        if self.active_clips.pop().is_some() {
            self.current_scene.pop_clip();
        }
    }

    fn finish_current_layer(&mut self, allow_empty: bool) {
        // A clip stack may span several retained scenes. Each scene must be
        // independently replayable; close inherited clips before publishing it.
        // Do not publish layers containing only clip-stack bookkeeping.
        let has_content = self
            .current_scene
            .commands()
            .iter()
            .any(|command| !matches!(command, Command::PushClip(_) | Command::PopClip));
        if allow_empty || has_content {
            for _ in &self.active_clips {
                self.current_scene.pop_clip();
            }
            self.layers.push(VisualLayer {
                kind: VisualLayerKind::Scene(std::mem::take(&mut self.current_scene)),
                transform: self.transform,
                widget_id: self.current_owner_id,
            });
        } else {
            self.current_scene.clear();
        }
        // The next scene starts under the same inherited clips.
        for clip in &self.active_clips {
            self.current_scene.push_clip(clip.clone());
        }
    }

    fn push_external_layer(&mut self, widget_id: WidgetId, bounds: kurbo::Rect) {
        self.layers.push(VisualLayer {
            kind: VisualLayerKind::External { bounds },
            transform: self.transform,
            widget_id,
        });
    }

    fn into_layers(mut self) -> Vec<VisualLayer> {
        self.finish_current_layer(self.layers.is_empty());
        self.layers
    }
}

/// Re-record only invalidated phases, including their layer declarations.
fn record_widget(
    global_state: &mut RenderRootState,
    property_arena: &PropertyArena,
    scene_cache: &mut HashMap<WidgetId, (Scene, Scene, Scene)>,
    node: ArenaMut<'_, WidgetArenaNode>,
) {
    let state = &mut node.item.state;
    let cache_missing = !scene_cache.contains_key(&state.id);
    if state.is_stashed()
        || !(cache_missing
            || state.request_pre_paint()
            || state.request_paint()
            || state.request_post_paint())
    {
        return;
    }

    let widget = &mut *node.item.widget;
    if global_state.trace.enabled(crate::passes::TracePass::Paint) {
        trace!(
            "Painting widget '{}' {}",
            widget.short_type_name(),
            state.id
        );
    }
    let props = PropertiesRef {
        local: &node.item.properties,
        default_map: property_arena
            .default_properties
            .for_widget(widget.type_id()),
        stack: property_arena.get(state.property_stack_id, widget.type_id()),
        class_set: &node.item.class_set,
    };
    let (pre, content, post) = scene_cache.entry(state.id).or_default();
    let phases = [
        (
            cache_missing || state.request_pre_paint(),
            pre,
            PaintPhase::Pre,
        ),
        (
            cache_missing || state.request_paint(),
            content,
            PaintPhase::Content,
        ),
        (
            cache_missing || state.request_post_paint(),
            post,
            PaintPhase::Post,
        ),
    ];
    let mut ctx = PaintCtx {
        global_state,
        widget_state: state,
        children: node.children,
        layer_mode: None,
    };
    for (requested, scene, phase) in phases {
        if requested {
            scene.clear();
            let sink: &mut dyn PaintSink = scene;
            let mut painter = Painter::new(sink);
            match phase {
                PaintPhase::Pre => widget.pre_paint(&mut ctx, &props, &mut painter),
                PaintPhase::Content => widget.paint(&mut ctx, &props, &mut painter),
                PaintPhase::Post => widget.post_paint(&mut ctx, &props, &mut painter),
            }
            ctx.widget_state.paint_layer_modes[phase as usize] = ctx.layer_mode.take();
        }
    }
}

#[derive(Clone, Copy)]
enum PaintPhase {
    Pre,
    Content,
    Post,
}

// --- MARK: PAINT WIDGET
fn paint_widget(
    global_state: &mut RenderRootState,
    property_arena: &PropertyArena,
    layer_collector: &mut LayerCollector,
    scene_cache: &mut HashMap<WidgetId, (Scene, Scene, Scene)>,
    window_to_layer_transform: &Affine,
    node: ArenaMut<'_, WidgetArenaNode>,
) {
    let mut node = node;
    let _span = enter_span_if(
        global_state.trace.enabled(crate::passes::TracePass::Paint),
        &node.item.state,
    );
    record_widget(
        global_state,
        property_arena,
        scene_cache,
        node.reborrow_mut(),
    );
    let children = node.children;
    let widget = &mut *node.item.widget;
    let state = &mut node.item.state;
    let id = state.id;
    // Still visit stashed subtrees to clear pending flags.
    let is_stashed = state.is_stashed();

    state.set_request_pre_paint(false);
    state.set_request_paint(false);
    state.set_request_post_paint(false);
    state.set_needs_paint(false);

    let paint_layer_mode = if is_stashed {
        PaintLayerMode::Inline
    } else {
        state
            .paint_layer_modes
            .iter()
            .rev()
            .find_map(|mode| *mode)
            .unwrap_or_default()
    };

    if matches!(
        paint_layer_mode,
        PaintLayerMode::IsolatedScene | PaintLayerMode::External
    ) {
        layer_collector.finish_current_layer(false);
    }

    let previous_owner_id = layer_collector.current_owner_id;
    layer_collector.current_owner_id = match paint_layer_mode {
        PaintLayerMode::Inline => previous_owner_id,
        PaintLayerMode::IsolatedScene | PaintLayerMode::External => id,
    };

    let border_box_to_layer_transform = *window_to_layer_transform * state.window_transform;
    let content_box_to_layer_transform =
        border_box_to_layer_transform.pre_translate(state.border_box_translation());
    let paint_as_external = paint_layer_mode == PaintLayerMode::External;
    let has_clip = !is_stashed
        && !paint_as_external
        && append_before_children(
            layer_collector,
            scene_cache,
            state,
            border_box_to_layer_transform,
            content_box_to_layer_transform,
        );

    let parent_state = &mut *state;
    recurse_on_children(id, widget, children, |mut node| {
        // TODO: We could skip painting children outside the parent clip path.
        // There's a few things to consider if we do:
        // - Some widgets can paint outside of their border-box.
        // - Once we implement compositor layers, we may want to paint outside of the clip path anyway in anticipation of user scrolling.
        // - We still want to reset needs_paint and request_paint flags.
        paint_widget(
            global_state,
            property_arena,
            layer_collector,
            scene_cache,
            window_to_layer_transform,
            node.reborrow_mut(),
        );
        parent_state.merge_up(&node.item.state);
    });

    if !is_stashed && !paint_as_external {
        append_after_children(
            global_state,
            layer_collector,
            scene_cache,
            state,
            has_clip,
            border_box_to_layer_transform,
            content_box_to_layer_transform,
        );
    }

    if paint_as_external {
        layer_collector.push_external_layer(
            id,
            border_box_to_layer_transform.transform_rect_bbox(state.border_box()),
        );
    }

    if matches!(
        paint_layer_mode,
        PaintLayerMode::IsolatedScene | PaintLayerMode::External
    ) {
        layer_collector.finish_current_layer(false);
    }

    layer_collector.current_owner_id = previous_owner_id;
}

fn append_before_children(
    collector: &mut LayerCollector,
    scene_cache: &HashMap<WidgetId, (Scene, Scene, Scene)>,
    state: &crate::core::WidgetState,
    border_transform: Affine,
    content_transform: Affine,
) -> bool {
    let Some((pre_scene, scene, _)) = scene_cache.get(&state.id) else {
        debug_panic!(
            "Error in paint pass: scene should have been cached earlier in this function."
        );
        return false;
    };
    collector
        .scene_mut()
        .append_transformed(pre_scene, content_transform);
    if let Some(clip) = state.clip_path {
        collector.push_clip(Clip::Fill {
            transform: border_transform,
            shape: Geometry::Rect(clip),
            fill_rule: Fill::NonZero,
        });
    }
    collector
        .scene_mut()
        .append_transformed(scene, content_transform);
    state.clip_path.is_some()
}

fn append_after_children(
    global_state: &RenderRootState,
    collector: &mut LayerCollector,
    scene_cache: &HashMap<WidgetId, (Scene, Scene, Scene)>,
    state: &crate::core::WidgetState,
    has_clip: bool,
    border_transform: Affine,
    content_transform: Affine,
) {
    if global_state.debug_paint() {
        paint_debug_geometry(collector.scene_mut(), state, border_transform);
    }
    if has_clip {
        collector.pop_clip();
    }
    if let Some((_, _, post_scene)) = scene_cache.get(&state.id) {
        collector
            .scene_mut()
            .append_transformed(post_scene, content_transform);
    } else {
        debug_panic!(
            "Error in paint pass: scene should have been cached earlier in this function."
        );
    }
    if global_state.inspector_state.hovered_widget == Some(state.id) {
        const HOVER_FILL_COLOR: Color = Color::from_rgba8(60, 60, 250, 100);
        Painter::new(collector.scene_mut())
            .fill(state.border_box(), HOVER_FILL_COLOR)
            .transform(border_transform)
            .draw();
    }
}

fn paint_debug_geometry(
    scene: &mut Scene,
    state: &crate::core::WidgetState,
    border_box_to_layer_transform: Affine,
) {
    const BORDER_WIDTH: f64 = 1.0;
    let color = get_debug_color(state.id.to_raw());
    let rect = state.bounding_box.inset(BORDER_WIDTH / -2.0);
    let border_style = Stroke::new(BORDER_WIDTH);
    let mut painter = Painter::new(scene);
    painter.stroke(rect, &border_style, color).draw();

    let mut draw_baseline = |baseline| {
        let border_box = state.border_box();
        let line = Line::new((border_box.x0, baseline), (border_box.x1, baseline));
        let baseline_style = Stroke::new(1.0).with_dashes(0., [4.0, 4.0]);
        painter
            .stroke(line, &baseline_style, color)
            .transform(border_box_to_layer_transform)
            .draw();
    };
    if state.first_baseline.is_finite() {
        draw_baseline(state.first_baseline);
    }
    // Equal baselines describe the same line, including signed zero.
    if state.last_baseline.is_finite()
        && (!state.first_baseline.is_finite()
            || (state.last_baseline - state.first_baseline).abs() > 0.0)
    {
        draw_baseline(state.last_baseline);
    }
}

// --- MARK: ROOT
/// See the [passes documentation](crate::doc::pass_system#render-passes).
pub(crate) fn run_paint_pass(root: &mut RenderRoot) -> VisualLayerPlan {
    let _span = info_span!("paint").entered();

    // TODO - This is a bit of a hack until we refactor widget tree mutation.
    // This should be removed once remove_child is exclusive to MutateCtx.
    let mut scene_cache = std::mem::take(&mut root.global_state.scene_cache);

    let root_id = root.root_id();
    let layer_root_ids = root.layer_root_ids();

    let mut layers = Vec::new();

    for (idx, &layer_widget_id) in layer_root_ids.iter().enumerate() {
        let Some(state) = root.widget_arena.get_state(layer_widget_id) else {
            continue;
        };
        let transform = if idx == 0 {
            Affine::IDENTITY
        } else {
            state.window_transform
        };

        let mut collector = LayerCollector::new(layer_widget_id, transform);
        paint_layer(
            root,
            &mut collector,
            &mut scene_cache,
            root_id,
            layer_widget_id,
        );
        layers.extend(collector.into_layers());
    }

    root.global_state.scene_cache = scene_cache;

    VisualLayerPlan { layers }
}

/// Paint a single layer's widget subtree into `target_scene`.
///
/// This is a helper that handles the split borrows needed to access
/// `global_state`, `default_properties`, and `widget_arena` simultaneously.
fn paint_layer(
    root: &mut RenderRoot,
    layer_collector: &mut LayerCollector,
    scene_cache: &mut HashMap<WidgetId, (Scene, Scene, Scene)>,
    root_id: WidgetId,
    layer_widget_id: WidgetId,
) {
    // Clear the LayerStack's own paint flags (its paint is a no-op).
    // This is idempotent so safe to call per-layer.
    let Some(root_node) = root.widget_arena.nodes.find_mut(root_id) else {
        return;
    };
    let state = &mut root_node.item.state;
    state.set_request_pre_paint(false);
    state.set_request_paint(false);
    state.set_request_post_paint(false);
    state.set_needs_paint(false);

    // Get the layer child from the same arena handle.
    let Some(layer_node) = root_node.children.into_item_mut(layer_widget_id) else {
        debug_panic!(
            "Error in paint pass: cannot find layer child {layer_widget_id:?} in LayerStack"
        );
        return;
    };

    let window_to_layer_transform = layer_node.item.state.window_transform.inverse();

    paint_widget(
        &mut root.global_state,
        &root.property_arena,
        layer_collector,
        scene_cache,
        &window_to_layer_transform,
        layer_node,
    );
}
