// Copyright 2019 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use accesskit::{ActionRequest, NodeId, TreeId, TreeUpdate};
use dpi::{LogicalPosition, LogicalSize, PhysicalSize};
use kurbo::{Point, Rect, Size};
use parley::fontique::{
    Blob, Collection, CollectionOptions, FallbackKey, FamilyId, FontInfo, Script, SourceCache,
};
use parley::{FontContext, LayoutContext};
use tracing::{debug, info_span, warn};
use tree_arena::{ArenaMut, TreeArena};

use crate::app::VisualLayerPlan;
use crate::app::layer_stack::LayerStack;
use crate::core::{
    AccessCtx, AccessEvent, BrushIndex, CursorIcon, DefaultProperties, ErasedAction, FromDynWidget,
    Handled, Ime, LayerPlacement, LayerType, NewWidget, PointerButton, PointerEvent, PointerInfo,
    PropertyArena, PropertyStack, PropertyStackId, ResizeDirection, TextEvent, Widget, WidgetArena,
    WidgetArenaNode, WidgetId, WidgetMut, WidgetPod, WidgetRef, WidgetState, WidgetTag,
    WidgetTagInner, WindowEvent,
};
use crate::imaging::record::Scene;
use crate::passes::accessibility::run_accessibility_pass;
use crate::passes::action::run_action_pass;
use crate::passes::anim::run_update_anim_pass;
use crate::passes::compose::run_compose_pass;
use crate::passes::event::{
    run_on_access_event_pass, run_on_pointer_event_pass, run_on_text_event_pass,
};
use crate::passes::layout::run_layout_pass;
use crate::passes::mutate::{mutate_widget, run_mutate_pass};
use crate::passes::paint::run_paint_pass;
use crate::passes::update::{
    run_update_disabled_pass, run_update_focus_pass, run_update_focusable_pass,
    run_update_fonts_pass, run_update_pointer_pass, run_update_props_pass, run_update_scroll_pass,
    run_update_stashed_pass, run_update_widget_tree_pass,
};
use crate::passes::{PassTracing, recurse_on_children};
use crate::properties::Dimensions;

/// We ensure that any valid initial IME area is sent to the platform by storing an invalid initial
/// IME area as the `last_sent_ime_area`.
const INVALID_IME_AREA: Rect = Rect::new(f64::NAN, f64::NAN, f64::NAN, f64::NAN);

// --- MARK: STRUCTS

/// The composition root of Masonry.
///
/// This is the entry point for all user events, and the source of all signals to be sent to
/// winit or similar event loop runners, as well as 2D scenes and accessibility information.
///
/// This is also the type that owns the widget tree.
pub struct RenderRoot {
    /// `WidgetPod` handle for the layer stack, which holds the root widget of each layer.
    pub(crate) layer_stack: WidgetPod<LayerStack>,

    /// Property data, including property stacks and per-widget-type default properties.
    pub(crate) property_arena: PropertyArena,

    /// State passed to context types.
    pub(crate) global_state: RenderRootState,

    /// The widget tree; stores widgets and their states.
    pub(crate) widget_arena: WidgetArena,
}

/// The pointer and button that initiated a captured interaction.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PointerPress {
    pub pointer: PointerInfo,
    pub button: Option<PointerButton>,
}

/// Single captured interaction, including the initiating press.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PointerCapture {
    pub target: WidgetId,
    pub press: PointerPress,
}

/// State shared between passes.
pub(crate) struct RenderRootState {
    flags: u8,
    /// Sink for signals to be processed by the event loop.
    pub(crate) signal_sink: Box<dyn FnMut(RenderRootSignal)>,

    /// The accessibility pass creates a wrapper node for the app with `Role::Window`.
    ///
    /// This is the id of that node.
    pub(crate) window_node_id: NodeId,

    /// Whether the window size should be determined by the content or the user.
    pub(crate) size_policy: WindowSizePolicy,

    /// Current size of the window.
    pub(crate) size: PhysicalSize<u32>,

    /// Last mouse position. Updated by `on_pointer_event` pass, used by other passes.
    pub(crate) last_mouse_pos: Option<LogicalPosition<f64>>,

    /// Currently focused widget.
    pub(crate) focused_widget: Option<WidgetId>,

    /// List of ancestors of the currently focused widget.
    pub(crate) focused_path: Vec<WidgetId>,

    /// Widget that will be focused once the `update_focus` pass is run.
    pub(crate) next_focused_widget: Option<WidgetId>,

    /// Most recently clicked/focused widget.
    ///
    /// This is used to pick the focused widget on Tab events.
    pub(crate) focus_anchor: Option<WidgetId>,

    /// Widget which will get text events if no widget is focused.
    pub(crate) focus_fallback: Option<WidgetId>,

    /// Widgets that have requested to be scrolled into view.
    ///
    /// The `WidgetId` is the id of the widget that made the request.
    ///
    /// The `Rect` is the area it wants to be scrolled into view,
    /// in its border-box coordinate space.
    pub(crate) scroll_request_targets: Vec<(WidgetId, Rect)>,

    /// List of ancestors of the currently hovered widget.
    pub(crate) hovered_path: Vec<WidgetId>,

    /// List of ancestors of the currently active widget.
    pub(crate) active_path: Vec<WidgetId>,

    /// The widget and initiating press that currently own pointer capture.
    pub(crate) pointer_capture: Option<PointerCapture>,

    /// Current cursor icon.
    pub(crate) cursor_icon: CursorIcon,

    /// Cache for Parley font data.
    // TODO: move font context out of RenderRootState so that we only have it once per app
    pub(crate) font_context: FontContext,

    /// Cache for Parley text layout data.
    pub(crate) text_layout_context: LayoutContext<BrushIndex>,

    /// List of callbacks that will run in the next `mutate` pass.
    pub(crate) mutate_callbacks: Vec<MutateCallback>,

    /// List of actions that will be handled in the next `action` pass.
    pub(crate) actions: Vec<(ErasedAction, WidgetId)>,

    /// The cursor area last sent to the platform.
    pub(crate) last_sent_ime_area: Rect,

    /// Scene cache for the widget tree.
    pub(crate) scene_cache: HashMap<WidgetId, (Scene, Scene, Scene)>,

    pub(crate) widget_tags: HashMap<WidgetTagInner, WidgetId>,

    /// Map of layers attached to widgets, keyed by the attached widget id, then the type of the layer root.
    pub(crate) attached_layers: HashMap<WidgetId, HashMap<TypeId, WidgetId>>,

    /// Pass tracing configuration, used to skip tracing to limit overhead.
    pub(crate) trace: PassTracing,

    /// Internal state of the widget inspector.
    pub(crate) inspector_state: InspectorState,

    /// DPI scale factor.
    ///
    /// Kurbo coordinates are assumed to be in logical pixels
    pub(crate) scale_factor: f64,
}

type DeferredMutation = Box<dyn FnOnce(WidgetMut<'_, dyn Widget>)>;

pub(crate) struct MutateCallback {
    pub(crate) id: WidgetId,
    pub(crate) callback: DeferredMutation,
}

/// Defines how a window's size is determined.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum WindowSizePolicy {
    /// Measure the content to determine the window size.
    ///
    /// The window size will match the root widget's maximum preferred size.
    Content,
    /// Use the provided window size.
    #[default]
    User,
}

/// Options for creating a [`RenderRoot`].
#[derive(Debug)]
pub struct RenderRootOptions {
    /// Default values that properties will have if not defined per-widget.
    pub default_properties: Arc<DefaultProperties>,

    /// If true, `fontique` will provide access to system fonts
    /// using platform-specific APIs.
    pub use_system_fonts: bool,

    /// Defines how the window size should be determined.
    pub size_policy: WindowSizePolicy,

    /// The size of the window.
    pub size: PhysicalSize<u32>,

    /// The scale factor to use for rendering.
    ///
    /// Useful for high-DPI displays.
    ///
    /// `1.0` is a sensible default.
    pub scale_factor: f64,

    /// Add a font from its raw data for use in tests.
    /// The font is added to the fallback chain for Latin scripts.
    /// This is expected to be used with `use_system_fonts = false`
    /// to ensure rendering is consistent cross-platform.
    ///
    /// We expect to develop a much more fully-featured font API in the future, but
    /// this is necessary for our testing of Masonry.
    pub test_font: Option<Blob<u8>>,
}

/// Objects emitted by the [`RenderRoot`] to signal that something has changed or require external actions.
#[derive(Debug)]
pub enum RenderRootSignal {
    /// A widget has emitted an action.
    Action(ErasedAction, WidgetId),
    /// An IME session has been started.
    StartIme,
    /// The IME session has ended.
    EndIme,
    /// The IME area has been moved.
    ImeMoved(LogicalPosition<f64>, LogicalSize<f64>),
    /// A user interaction has sent something to the clipboard.
    ClipboardStore(String),
    /// The window needs to be redrawn.
    RequestRedraw,
    /// The window should be redrawn for an animation frame. Currently this isn't really different from `RequestRedraw`.
    RequestAnimFrame,
    /// The window should take focus.
    TakeFocus,
    /// The mouse icon should change.
    SetCursor(CursorIcon),
    /// The window should be resized.
    SetSize(PhysicalSize<u32>),
    /// The window title has changed.
    SetTitle(String),
    /// The user has started dragging the window.
    ///
    /// Masonry should send this event when the user presses the left mouse button while hovering a client-side decoration representing the window title bar or similar.
    /// The platform that receives this event should start moving the window around until the mouse button is released.
    DragWindow,
    /// The user has started drag-resizing the window.
    ///
    /// Masonry should send this event when the user presses the left mouse button while hovering a client-side decoration representing a window resize handle.
    /// The platform that receives this event should start resizing the window until the mouse button is released.
    DragResizeWindow(ResizeDirection),
    /// The window should be maximized.
    ToggleMaximized,
    /// The window should be minimized.
    Minimize,
    /// The app should terminate.
    Exit,
    /// The [window system menu] should be shown.
    ///
    /// There are no guarantees as to what the shown menu will contain, or even if a window menu will be drawn at all.
    /// In general, the menu may contain options to maximize, minimize or move the window.
    ///
    /// Some platforms may ignore this.
    ///
    /// See also [the Wayland doc for `xdg_toplevel::show_window_menu`](https://wayland.app/protocols/xdg-shell#xdg_toplevel:request:show_window_menu).
    ///
    /// [Windows system menu]: https://en.wikipedia.org/w/index.php?title=Common_menus_in_Microsoft_Windows&oldid=1285312933#System_menu
    ShowWindowMenu(LogicalPosition<f64>),
    /// The widget picker has selected this widget.
    WidgetSelectedInInspector(WidgetId),
    /// A new [layer] should be created with the widget as root.
    ///
    /// The placement is expressed in window coordinates.
    ///
    /// [layer]: crate::doc::masonry_concepts#layers
    NewLayer(LayerType, NewWidget<dyn Widget>, LayerPlacement),
    /// The layer with the given widget as root should be removed.
    RemoveLayer(WidgetId),
    /// The layer with the given widget as root should be repositioned to the specified point.
    ///
    /// The placement is expressed in window coordinates.
    RepositionLayer(WidgetId, LayerPlacement),
}

/// State of the widget inspector. Useful for debugging.
///
/// Widget inspector is WIP. It should get its own standalone documentation.
pub(crate) struct InspectorState {
    pub(crate) is_picking_widget: bool,
    pub(crate) hovered_widget: Option<WidgetId>,
}

impl RenderRoot {
    /// Creates a new `RenderRoot` with the given options.
    ///
    /// The provided root widget will always stay the root widget.
    /// (It cannot be changed later for another widget, only its children can change.)
    /// In case no widget is focused the event pass will target text events
    /// at the child of the root widget if it only has one child.
    ///
    /// Note that this doesn't create a window or start an event loop.
    /// The `masonry` crate doesn't provide a way to do that:
    /// look for `masonry_winit::app::run` instead.
    pub fn new(
        root_widget: NewWidget<impl Widget + ?Sized>,
        signal_sink: impl FnMut(RenderRootSignal) + 'static,
        options: RenderRootOptions,
    ) -> Self {
        let RenderRootOptions {
            default_properties,
            use_system_fonts,
            size_policy,
            size,
            scale_factor,
            test_font,
        } = options;
        let debug_paint = std::env::var("MASONRY_DEBUG_PAINT").is_ok_and(|it| !it.is_empty());

        // LayerStack can't use Dimensions::AUTO because it'll resolve to the window size.
        // Instead we want to always measure LayerStack, so it can measure its base layer.
        let layer_stack = LayerStack::new(root_widget)
            .prepare()
            .with_props(Dimensions::MAX)
            .into_pod();

        let mut root = Self {
            layer_stack,
            global_state: RenderRootState {
                signal_sink: Box::new(signal_sink),
                window_node_id: AccessCtx::next_node_id(),
                size_policy,
                size,
                last_mouse_pos: None,
                focused_widget: None,
                focused_path: Vec::new(),
                next_focused_widget: None,
                focus_anchor: None,
                focus_fallback: None,
                flags: RootFlag::WindowFocused as u8
                    | (u8::from(debug_paint) * RootFlag::DebugPaint as u8),
                scroll_request_targets: Vec::new(),
                hovered_path: Vec::new(),
                active_path: Vec::new(),
                pointer_capture: None,
                cursor_icon: CursorIcon::Default,
                font_context: FontContext {
                    collection: Collection::new(CollectionOptions {
                        system_fonts: use_system_fonts,
                        ..Default::default()
                    }),
                    source_cache: SourceCache::default(),
                },
                text_layout_context: LayoutContext::new(),
                mutate_callbacks: Vec::new(),
                actions: Vec::new(),
                last_sent_ime_area: INVALID_IME_AREA,
                scene_cache: HashMap::new(),
                widget_tags: HashMap::new(),
                attached_layers: HashMap::new(),
                trace: PassTracing::from_env(),
                inspector_state: InspectorState {
                    is_picking_widget: false,
                    hovered_widget: None,
                },
                scale_factor,
            },
            property_arena: PropertyArena::new(default_properties),
            widget_arena: WidgetArena {
                nodes: TreeArena::new(),
            },
        };

        if let Some(test_font_data) = test_font {
            // We don't use `register_fonts` here because that requests a global relayout.
            // However, because we are not yet fully initialised (we are before the below call
            // to `run_rewrite_passes`), `request_layout_all` will panic, as the root hasn't
            // been inserted into the arena yet.
            let families = root
                .global_state
                .font_context
                .collection
                .register_fonts(test_font_data, None);
            // Make sure that all of these fonts are in the fallback chain for the Latin script.
            // <https://en.wikipedia.org/wiki/Script_(Unicode)#Latn>
            root.global_state.font_context.collection.append_fallbacks(
                FallbackKey::new(Script::from_bytes(*b"Latn"), None),
                families.iter().map(|(family, _)| *family),
            );
        }

        // We run a set of passes to initialize the widget tree
        root.run_rewrite_passes();

        root
    }

    /// Returns a mutable reference to the `PropertyArena`.
    pub const fn property_arena(&mut self) -> &mut PropertyArena {
        &mut self.property_arena
    }

    /// Replace the tree-wide default properties at runtime.
    ///
    /// The default property set is normally fixed at construction. This lets
    /// a host swap it mid-session, e.g. a light/dark theme toggle that
    /// re-colors widgets relying on default `ContentColor` / `Background`.
    ///
    /// This invalidates the computed properties of the entire widget tree,
    /// and calls [`Widget::property_changed`] for every property previously
    /// resolved by each widget.
    pub fn set_default_properties(&mut self, default_properties: Arc<DefaultProperties>) {
        // Mark the whole tree for the update-properties pass: `needs_update_props`
        // so the pass descends to every node, `request_update_props` to enter the
        // cache-eviction branch, and `invalidated` so every cached entry re-fires
        // `property_changed` regardless of its (unchanged) resolved index.
        self.property_arena.default_properties = default_properties;
        self.visit_widget_states(|state| {
            state.set_request_update_props(true);
            state.set_needs_update_props(true);
            state.property_cache.invalidate();
        });

        self.run_rewrite_passes();
    }

    pub(crate) const fn root_id(&self) -> WidgetId {
        self.layer_stack.id()
    }

    fn layer_stack(&self) -> Option<WidgetRef<'_, LayerStack>> {
        self.get_widget(self.root_id())?.downcast()
    }

    fn layer_root_id(&self, layer_idx: usize) -> Option<WidgetId> {
        self.layer_stack()?.layer_root_id(layer_idx)
    }

    /// Returns the root identifiers in layer order (base layer first).
    pub(crate) fn layer_root_ids(&self) -> Vec<WidgetId> {
        self.layer_stack()
            .map_or_else(Vec::new, |stack| stack.layer_root_ids().collect())
    }

    pub(crate) fn root_state(&self) -> Option<&WidgetState> {
        self.widget_arena.get_state(self.root_id())
    }

    pub(crate) fn root_state_mut(&mut self) -> Option<&mut WidgetState> {
        self.widget_arena
            .nodes
            .roots_mut()
            .into_item_mut(self.layer_stack.id())
            .map(|node| &mut node.item.state)
    }

    // --- MARK: WINDOW_EVENT
    /// Handles a window event.
    pub fn handle_window_event(&mut self, event: WindowEvent) -> Handled {
        match event {
            WindowEvent::Rescale(scale_factor) => {
                if self.global_state.scale_factor.to_bits() != scale_factor.to_bits() {
                    self.global_state.scale_factor = scale_factor;
                    // Even if physical size does not change, the logical
                    // constraints do. Content-sized roots must also recompute
                    // their physical extent from the new DPI.
                    self.visit_widget_states(|state| {
                        state.set_request_layout(true);
                        state.set_needs_layout(true);
                    });
                    self.request_render_all();
                    self.run_rewrite_passes();
                }
            }
            WindowEvent::Resize(size) => {
                self.global_state.size = size;
                if let Some(state) = self.root_state_mut() {
                    state.set_request_layout(true);
                    state.set_needs_layout(true);
                }
                self.run_rewrite_passes();
            }
            WindowEvent::AnimFrame(duration) => {
                run_update_anim_pass(self, u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX));
                self.run_rewrite_passes();
            }
            WindowEvent::EnableAccessTree => {
                self.global_state.set_access_tree_active(true);
                // AccessKit expects the next update to have include
                // a description of every single node.
                self.request_access_all();
                self.global_state
                    .emit_signal(RenderRootSignal::RequestRedraw);
            }
            WindowEvent::DisableAccessTree => {
                self.global_state.set_access_tree_active(false);
            }
        }
        Handled::Yes
    }

    // --- MARK: PUB FUNCTIONS
    /// Handles a pointer event.
    pub fn handle_pointer_event(&mut self, event: &PointerEvent) -> Handled {
        let _span = info_span!("pointer_event");
        let handled = run_on_pointer_event_pass(self, event);
        self.run_rewrite_passes();

        handled
    }

    /// Handles a text event.
    pub fn handle_text_event(&mut self, event: &TextEvent) -> Handled {
        let _span = info_span!("text_event");
        let handled = run_on_text_event_pass(self, event);
        run_update_focus_pass(self);

        if matches!(event, TextEvent::Ime(Ime::Enabled)) {
            // Reset the last sent IME area, as the platform reset the IME state and may have
            // forgotten it.
            self.global_state.last_sent_ime_area = INVALID_IME_AREA;
        }
        self.run_rewrite_passes();

        handled
    }

    /// Handles an accesskit event.
    pub fn handle_access_event(&mut self, event: ActionRequest) {
        let _span = info_span!("access_event");
        if event.target_tree != TreeId::ROOT {
            warn!(
                "Received ActionRequest for an unknown tree. {:?}",
                event.target_tree
            );
            return;
        }
        let Ok(id) = event.target_node.0.try_into() else {
            warn!("Received ActionRequest with id 0. This shouldn't be possible.");
            return;
        };
        let event = AccessEvent {
            action: event.action,
            data: event.data,
        };

        run_on_access_event_pass(self, &event, WidgetId(id));
        self.run_rewrite_passes();
    }

    /// Registers all fonts that exist in the given data.
    ///
    /// Returns a list of pairs each containing the family identifier and fonts
    /// added to that family.
    pub fn register_fonts(&mut self, data: Blob<u8>) -> Vec<(FamilyId, Vec<FontInfo>)> {
        let ret = self
            .global_state
            .font_context
            .collection
            .register_fonts(data, None);
        run_update_fonts_pass(self);
        ret
    }

    /// Redraws the window.
    ///
    /// Returns the current visual-layer plan and, if accessibility is active, a tree update.
    pub fn redraw(&mut self) -> (VisualLayerPlan, Option<TreeUpdate>) {
        let visual_layers = self.redraw_visual();
        let tree_update = self.take_accessibility_update();
        (visual_layers, tree_update)
    }

    /// Rebuild layout and record the visual-layer plan without consuming accessibility updates.
    ///
    /// Hosts can call this before a fallible rendering operation, then call
    /// [`Self::take_accessibility_update`] after they have committed the frame.
    #[must_use = "Use the recorded visual layers."]
    pub fn redraw_visual(&mut self) -> VisualLayerPlan {
        self.run_rewrite_passes();
        run_paint_pass(self)
    }

    /// Consume the current accessibility update if the accessibility tree is enabled.
    ///
    /// Unlike [`Self::redraw`], this does not run layout or paint. Call after
    /// layout has been refreshed, and deliver the returned update to the host.
    #[must_use = "Deliver the returned update to the accessibility consumer."]
    pub fn take_accessibility_update(&mut self) -> Option<TreeUpdate> {
        self.global_state
            .access_tree_active()
            .then(|| run_accessibility_pass(self, self.global_state.scale_factor))
    }

    /// Returns the current icon that the mouse should display.
    pub const fn cursor_icon(&self) -> CursorIcon {
        self.global_state.cursor_icon
    }

    // --- MARK: ACCESS WIDGETS
    /// Returns a [`WidgetRef`] to the root widget of the given [layer](crate::doc::masonry_concepts#layers).
    pub fn get_layer_root(&self, layer_idx: usize) -> Option<WidgetRef<'_, dyn Widget>> {
        self.get_widget(self.layer_root_id(layer_idx)?)
    }

    /// Hit-tests the complete layer stack at a window-space point.
    ///
    /// Unlike a test restricted to the base layer, this includes popups and
    /// other overlays in their actual stacking order. Pointer capture is not
    /// considered: this returns the geometrically hit interactive widget.
    #[must_use = "Inspect the widget at this position."]
    pub fn widget_at_position(&self, position: Point) -> Option<WidgetRef<'_, dyn Widget>> {
        self.get_widget(self.root_id())?
            .find_widget_under_pointer(position)
    }

    /// Returns a [`WidgetRef`] to a specific widget.
    pub fn get_widget(&self, id: WidgetId) -> Option<WidgetRef<'_, dyn Widget>> {
        let node = self.widget_arena.nodes.find(id)?;
        Some(WidgetRef::from_node(
            node,
            &self.global_state,
            &self.property_arena,
        ))
    }

    /// Returns a [`WidgetRef`] to the widget with the given tag.
    pub fn get_widget_with_tag<W: Widget + FromDynWidget + ?Sized>(
        &self,
        tag: WidgetTag<W>,
    ) -> Option<WidgetRef<'_, W>> {
        let id = self.global_state.widget_tags.get(&tag.inner)?;
        self.get_widget(*id)?.downcast()
    }

    /// Returns the widget and its ancestors, starting at the widget and ending at the root.
    /// An unknown widget has an empty path.
    pub fn widget_path(&self, id: WidgetId) -> Vec<WidgetId> {
        self.widget_arena
            .nodes
            .get_id_path(id)
            .into_iter()
            .filter_map(|id| std::num::NonZeroU64::new(id).map(WidgetId))
            .collect()
    }

    /// Checks if a widget with the given id is in the tree.
    pub fn has_widget(&self, id: WidgetId) -> bool {
        self.widget_arena.has(id)
    }

    /// Edits the base layer's root widget, returning None for a missing or mismatched widget.
    pub fn edit_base_layer<R, W: Widget + FromDynWidget + ?Sized>(
        &mut self,
        f: impl FnOnce(WidgetMut<'_, W>) -> R,
    ) -> Option<R> {
        self.edit_layer(0, f)
    }

    /// Edits the root of the given layer, if it exists and has the requested type.
    pub fn edit_layer<R, W: Widget + FromDynWidget + ?Sized>(
        &mut self,
        layer_idx: usize,
        f: impl FnOnce(WidgetMut<'_, W>) -> R,
    ) -> Option<R> {
        self.edit_widget(self.layer_root_id(layer_idx)?, f)
    }

    /// Edits a specific widget, if it exists and has the requested type.
    pub fn edit_widget<R, W: Widget + FromDynWidget + ?Sized>(
        &mut self,
        id: WidgetId,
        f: impl FnOnce(WidgetMut<'_, W>) -> R,
    ) -> Option<R> {
        let result = mutate_widget(self, id, f)?;
        self.run_rewrite_passes();
        Some(result)
    }

    /// Edits the widget with the given tag, if it exists and has the requested type.
    pub fn edit_widget_with_tag<R, W: Widget + FromDynWidget + ?Sized>(
        &mut self,
        tag: WidgetTag<W>,
        f: impl FnOnce(WidgetMut<'_, W>) -> R,
    ) -> Option<R> {
        let id = *self.global_state.widget_tags.get(&tag.inner)?;
        self.edit_widget(id, f)
    }

    /// Adds a new layer at the end of the stack, with the given widget as its root, at the given position.
    ///
    /// The placement is expressed in window coordinates.
    pub fn add_layer(&mut self, root: NewWidget<impl Widget + ?Sized>, placement: LayerPlacement) {
        debug!("added layer to stack");
        let _ = mutate_widget(
            self,
            self.root_id(),
            |mut layer_stack: WidgetMut<'_, LayerStack>| {
                LayerStack::add_layer(&mut layer_stack, root, placement);
            },
        );

        self.run_rewrite_passes();
    }

    /// Removes the layer with the given widget as root.
    ///
    /// The base layer cannot be removed.
    ///
    /// # Panics
    ///
    /// Panics in debug mode if the intended layer is the base layer.
    /// An already removed layer is ignored.
    pub fn remove_layer(&mut self, root_id: WidgetId) {
        // A driver may close an attached layer directly. Clear its owner mapping
        // even if no widget requested the removal.
        let mut owner = None;
        self.global_state.attached_layers.retain(|id, layers| {
            let before = layers.len();
            layers.retain(|_, layer| *layer != root_id);
            if layers.len() != before {
                owner = Some(*id);
            }
            !layers.is_empty()
        });
        if let Some(owner) = owner {
            let _ = mutate_widget(self, owner, |mut widget: WidgetMut<'_, dyn Widget>| {
                widget.ctx.request_accessibility_update();
            });
        }
        let _ = mutate_widget(
            self,
            self.root_id(),
            |mut layer_stack: WidgetMut<'_, LayerStack>| {
                LayerStack::remove_layer(&mut layer_stack, root_id);
            },
        );

        self.run_rewrite_passes();
    }

    /// Repositions the layer with the given widget as root.
    ///
    /// The placement is expressed in window coordinates.
    ///
    /// The base layer cannot be repositioned.
    ///
    /// # Panics
    ///
    /// Panics in debug mode if the intended layer is the base layer.
    /// A layer removed before a queued reposition is ignored.
    pub fn reposition_layer(&mut self, root_id: WidgetId, placement: LayerPlacement) {
        let _ = mutate_widget(
            self,
            self.root_id(),
            |mut layer_stack: WidgetMut<'_, LayerStack>| {
                LayerStack::reposition_layer(&mut layer_stack, root_id, placement);
            },
        );

        self.run_rewrite_passes();
    }

    /// Returns the current size of the window.
    pub const fn size(&self) -> PhysicalSize<u32> {
        self.global_state.size
    }

    /// Returns the current DPI scale factor of the window.
    ///
    /// This is the number of physical pixels per logical pixel.
    pub const fn scale_factor(&self) -> f64 {
        self.global_state.scale_factor
    }

    pub(crate) fn get_kurbo_size(&self) -> Size {
        let size = self
            .global_state
            .size
            .to_logical(self.global_state.scale_factor);
        Size::new(size.width, size.height)
    }

    // --- MARK: REWRITE PASSES
    /// Runs all rewrite passes on widget tree.
    ///
    /// Rewrite passes are passes which occur after external events, and
    /// update flags and internal values to a consistent state.
    ///
    /// See the [passes documentation](crate::doc::pass_system) for details.
    pub(crate) fn run_rewrite_passes(&mut self) {
        const REWRITE_PASSES_MAX: usize = 4;

        for _ in 0..REWRITE_PASSES_MAX {
            // Note: this code doesn't do any short-circuiting, because each pass is
            // expected to have its own early exits.
            // Calling a run_xxx_pass should always be very fast if the pass doesn't need to do anything.

            run_mutate_pass(self);
            run_action_pass(self);
            run_update_widget_tree_pass(self);
            run_update_disabled_pass(self);
            run_update_stashed_pass(self);
            run_update_focusable_pass(self);
            run_update_focus_pass(self);
            run_layout_pass(self);
            run_update_scroll_pass(self);
            run_compose_pass(self);
            run_update_pointer_pass(self);
            run_update_props_pass(self);

            if !self.needs_rewrite_passes() {
                break;
            }
        }

        if self.needs_rewrite_passes() {
            warn!(
                "All rewrite passes have run {REWRITE_PASSES_MAX} times, but invalidations are still set"
            );
            // To avoid an infinite loop, we delay re-running the passes until the next frame.
            self.global_state
                .emit_signal(RenderRootSignal::RequestRedraw);
        }

        if self.root_state().is_some_and(WidgetState::needs_anim) {
            self.global_state
                .emit_signal(RenderRootSignal::RequestAnimFrame);
        }

        // We request a redraw if either the render tree or the accessibility
        // tree needs to be rebuilt. Usually both are rebuilt at the same time.
        // A redraw will trigger a rebuild of the accessibility tree.
        if self.root_state().is_some_and(WidgetState::needs_paint) || self.needs_accessibility() {
            self.global_state
                .emit_signal(RenderRootSignal::RequestRedraw);
        }

        if self.global_state.is_ime_active() {
            let Some(state) = self
                .global_state
                .focused_widget
                .and_then(|id| self.widget_arena.get_state(id))
            else {
                self.global_state.set_is_ime_active(false);
                self.global_state.emit_signal(RenderRootSignal::EndIme);
                return;
            };
            let ime_area = state.get_ime_area();
            // Certain desktop environments (primarily KDE on Wayland) re-synchronise IME state
            // with the client (this app) in response to the safe area changing.
            // Our handling of that ultimately results in us sending the safe area again,
            // which causes an infinite loop.
            // We break that loop by not re-sending the same safe area again.
            if self.global_state.last_sent_ime_area != ime_area {
                self.global_state.last_sent_ime_area = ime_area;
                self.global_state
                    .emit_signal(RenderRootSignal::new_ime_moved_signal(ime_area));
            }
        }
    }

    fn visit_widget_states(&mut self, mut visit: impl FnMut(&mut WidgetState)) {
        fn visit_subtree(
            node: ArenaMut<'_, WidgetArenaNode>,
            visit: &mut impl FnMut(&mut WidgetState),
        ) {
            let state = &mut node.item.state;
            visit(state);
            recurse_on_children(state.id, &*node.item.widget, node.children, |child| {
                visit_subtree(child, visit);
            });
        }

        if let Some(root_node) = self.widget_arena.nodes.find_mut(self.root_id()) {
            visit_subtree(root_node, &mut visit);
        }
    }

    pub(crate) fn request_access_all(&mut self) {
        self.visit_widget_states(|state| {
            state.set_needs_accessibility(true);
            state.set_request_accessibility(true);
        });
        self.global_state
            .emit_signal(RenderRootSignal::RequestRedraw);
    }

    pub(crate) fn request_render_all(&mut self) {
        self.visit_widget_states(|state| {
            state.set_needs_paint(true);
            state.set_needs_accessibility(true);
            state.set_request_pre_paint(true);
            state.set_request_paint(true);
            state.set_request_accessibility(true);
            state.set_request_post_paint(true);
        });
        self.global_state
            .emit_signal(RenderRootSignal::RequestRedraw);
    }

    /// Checks whether the given id points to a widget that is "interactive".
    /// i.e. not disabled or stashed.
    /// Only interactive widgets can have text focus or pointer capture.
    pub(crate) fn is_still_interactive(&self, id: WidgetId) -> bool {
        let Some(node) = self.widget_arena.nodes.find(id) else {
            return false;
        };
        let state = &node.item.state;

        !state.is_stashed() && !state.is_disabled()
    }

    /// Returns the [`WidgetId`] of the [focused widget](crate::doc::masonry_concepts#text-focus).
    pub const fn focused_widget(&self) -> Option<WidgetId> {
        self.global_state.focused_widget
    }

    /// Returns the [`WidgetId`] of the widget which [captures pointer events](crate::doc::masonry_concepts#pointer-capture).
    pub const fn pointer_capture_target(&self) -> Option<WidgetId> {
        self.global_state.pointer_capture_target()
    }

    /// Sets the [focused widget](crate::doc::masonry_concepts#text-focus)
    /// and the [focus anchor](crate::doc::masonry_concepts#focus-anchor).
    ///
    /// Returns false if the widget is not found in the tree or can't be focused.
    pub fn focus_on(&mut self, id: Option<WidgetId>) -> bool {
        if let Some(id) = id
            && !self.is_still_interactive(id)
        {
            return false;
        }
        self.global_state.next_focused_widget = id;
        self.global_state.focus_anchor = id;
        self.run_rewrite_passes();
        true
    }

    /// Sets the [focus fallback](crate::doc::masonry_concepts#focus-fallback).
    ///
    /// Returns false if the widget is not found in the tree or can't be focused.
    pub fn set_focus_fallback(&mut self, id: Option<WidgetId>) -> bool {
        if let Some(id) = id
            && !self.is_still_interactive(id)
        {
            return false;
        }
        self.global_state.focus_fallback = id;
        true
    }

    /// Returns true if the widget tree is waiting for an animation frame.
    pub fn needs_anim(&self) -> bool {
        self.root_state().is_some_and(WidgetState::needs_anim)
    }

    /// Returns true if the accessibility tree needs to be rebuilt.
    ///
    /// This will be inhibited if `access_tree_active` is false.
    pub fn needs_accessibility(&self) -> bool {
        self.global_state.access_tree_active()
            && self
                .root_state()
                .is_some_and(WidgetState::needs_accessibility)
    }

    /// Returns `true` if something requires a rewrite pass or a re-render.
    pub fn needs_rewrite_passes(&self) -> bool {
        self.root_state()
            .is_some_and(WidgetState::needs_rewrite_passes)
            || self.global_state.needs_rewrite_passes()
    }

    // TODO - Remove?
    #[doc(hidden)]
    pub fn emit_signal(&mut self, signal: RenderRootSignal) {
        self.global_state.emit_signal(signal);
    }
}

// --- MARK: PROPERTY STACK EDITS
impl RenderRoot {
    /// Replace a [`PropertyStack`] to a new one stack.
    ///
    /// # Panics
    ///
    /// Panics if there is no property stack with the given id in the [`PropertyArena`].
    pub fn replace_property_stack(
        &mut self,
        property_stack_id: PropertyStackId,
        stack: PropertyStack,
    ) {
        let _ = self.property_arena.arena.insert(property_stack_id, stack);

        self.invalidate_property_stack_resolution(property_stack_id);

        self.run_rewrite_passes();
    }

    /// Checks if a property stack with the given id is in the property arena.
    pub fn has_property_stack(&self, property_stack_id: PropertyStackId) -> bool {
        self.property_arena.arena.contains_key(&property_stack_id)
    }

    /// Remove a property stack with the given id in the property arena.
    ///
    /// This also invalidate the computed properties of any widget in the entire tree that is linked to that property stack,
    /// and calls [`Widget::property_changed`] for every property previously
    /// resolved by each widget.
    pub fn remove_property_stack(&mut self, stack_id: PropertyStackId) {
        // Don't proceed any further if nothing has been removed
        if self.property_arena.arena.remove(&stack_id).is_none() {
            return;
        }

        self.invalidate_property_stack_resolution(stack_id);

        self.run_rewrite_passes();
    }

    /// Add a property stack to the [`PropertyArena`] and returns its id.
    pub fn insert_property_stack(&mut self, property_stack: PropertyStack) -> PropertyStackId {
        self.property_arena.insert(property_stack)
    }

    /// Mark the node that is linked to this property stack for the update-properties pass: `need_update_props`
    fn invalidate_property_stack_resolution(&mut self, stack_id: PropertyStackId) {
        // Only mark the node that is linked to this property stack for the update-properties pass: `need_update_props`
        fn invalidate_properties_resolution(
            node: ArenaMut<'_, WidgetArenaNode>,
            stack_id: PropertyStackId,
        ) {
            let children = node.children;
            let widget = &mut *node.item.widget;
            let state = &mut node.item.state;

            // We tell the node state there some props update that needs to be done,
            // since we can't tell in advance if its child has the property stack id.
            // If not, the property changes will be not visible when we run the rewrite passes.
            state.set_needs_update_props(true);

            let is_linked_to_p_stack = state.property_stack_id == Some(stack_id);

            if is_linked_to_p_stack {
                state.set_request_update_props(true);
                state.property_cache.invalidate();
            }

            let id = state.id;
            recurse_on_children(id, widget, children, |node| {
                invalidate_properties_resolution(node, stack_id);
            });
        }

        if let Some(root_node) = self.widget_arena.nodes.find_mut(self.root_id()) {
            invalidate_properties_resolution(root_node, stack_id);
        }
    }
}

impl std::fmt::Debug for RenderRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderRoot")
            .field("property_arena", &self.property_arena)
            .field(
                "widget_arena",
                &format!("<{} widgets>", self.widget_arena.nodes.len()),
            )
            .field("layer_stack", &self.layer_stack)
            .field("global_state", &self.global_state)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for RenderRootState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // TODO - Document fields?
        f.debug_struct("RenderRootState").finish_non_exhaustive()
    }
}

#[derive(Clone, Copy)]
#[repr(u8)]
enum RootFlag {
    WindowFocused = 1 << 0,
    IsImeActive = 1 << 1,
    NeedsPointerPass = 1 << 2,
    AccessTreeActive = 1 << 3,
    DebugPaint = 1 << 4,
}

macro_rules! root_flag_accessors {
    ($get:ident, $set:ident, $flag:ident) => {
        pub(crate) const fn $get(&self) -> bool {
            self.flags & RootFlag::$flag as u8 != 0
        }

        pub(crate) const fn $set(&mut self, value: bool) {
            self.flags =
                (self.flags & !(RootFlag::$flag as u8)) | ((value as u8) * RootFlag::$flag as u8);
        }
    };
}

impl RenderRootState {
    pub(crate) const fn pointer_capture_target(&self) -> Option<WidgetId> {
        match self.pointer_capture {
            Some(capture) => Some(capture.target),
            None => None,
        }
    }

    root_flag_accessors!(window_focused, set_window_focused, WindowFocused);
    root_flag_accessors!(is_ime_active, set_is_ime_active, IsImeActive);
    root_flag_accessors!(needs_pointer_pass, set_needs_pointer_pass, NeedsPointerPass);
    root_flag_accessors!(access_tree_active, set_access_tree_active, AccessTreeActive);
    root_flag_accessors!(debug_paint, set_debug_paint, DebugPaint);

    pub(crate) fn start_ime_session(&mut self) {
        self.set_is_ime_active(true);
        self.last_sent_ime_area = INVALID_IME_AREA;
        self.emit_signal(RenderRootSignal::StartIme);
    }

    /// Sends a signal to the runner of this app, which allows global actions to be triggered by a widget.
    pub(crate) fn emit_signal(&mut self, signal: RenderRootSignal) {
        (self.signal_sink)(signal);
    }

    /// Does something in this state indicate that the rewrite passes need to be reran.
    ///
    /// This is checked in conjunction with [`WidgetState::needs_rewrite_passes`] - if
    /// either returns true, the fixed point loop of the rewrite passes will be run again.
    /// All passes have a fast-path exit check; these together are the union of those checks.
    pub(crate) fn needs_rewrite_passes(&self) -> bool {
        self.needs_pointer_pass()
            || self.focused_widget != self.next_focused_widget
            || !self.mutate_callbacks.is_empty()
            || !self.actions.is_empty()
    }
}

impl RenderRootSignal {
    pub(crate) const fn new_ime_moved_signal(area: Rect) -> Self {
        Self::ImeMoved(
            LogicalPosition {
                x: area.origin().x,
                y: area.origin().y,
            },
            LogicalSize {
                width: area.size().width,
                height: area.size().height,
            },
        )
    }
}
