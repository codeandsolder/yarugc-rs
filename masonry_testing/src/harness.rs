// Copyright 2020 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

//! Tools and infrastructure for testing widgets.
//!
//! # Note on testing philosophy
//!
//! `TestHarness` provides two APIs, so to speak:
//!
//! - A high-level API that mimics user interactions.
//! - A low-level API that mirrors the [`RenderRoot`] API.
//!
//! Developers are encouraged to use the former if possible, and write tests as if
//! they were end users interacting with an application.
//!
//! We don't want users to write `harness.pointer_event(left_click(button_position))`,
//! we want them to write `harness.click_on(button)`, so to speak (actual method names differ).
//!
//! When adding features to `TestHarness`, we should always strive to give them such a high-level API.

use std::collections::VecDeque;
use std::fs::File;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::UNIX_EPOCH;

use image::{ImageFormat, Rgba, RgbaImage};
use imaging_vello_cpu::VelloCpuRenderer;
use masonry_core::imaging::record::{Scene, replay_transformed};
use masonry_core::imaging::{ImageRenderer as _, Painter};

use tracing::debug;

use masonry_core::accesskit::{Action, ActionRequest, Node, Role, Tree, TreeId, TreeUpdate};
use masonry_core::anymore::AnyDebug;
use masonry_core::app::{
    RenderRoot, RenderRootOptions, RenderRootSignal, VisualLayerKind, VisualLayerPlan,
    WindowSizePolicy, try_init_test_tracing,
};
use masonry_core::core::keyboard::{Code, Key, KeyState, NamedKey};
use masonry_core::core::{
    CursorIcon, DefaultProperties, ErasedAction, FromDynWidget, Handled, Ime, KeyboardEvent,
    Modifiers, NewWidget, PointerButton, PointerButtonEvent, PointerEvent, PointerId, PointerInfo,
    PointerScrollEvent, PointerState, PointerType, PointerUpdate, ScrollDelta, TextEvent, Widget,
    WidgetId, WidgetMut, WidgetRef, WidgetTag, WindowEvent,
};
use masonry_core::dpi::{LogicalPosition, LogicalSize, PhysicalSize};
use masonry_core::kurbo::{Affine, Point, Rect, Vec2};
use masonry_core::peniko::{Blob, Color};
use masonry_core::util::Duration;

use crate::HarnessError;
use crate::{Record, Recorder};

/// A [`PointerInfo`] for a primary mouse, for testing.
pub const PRIMARY_MOUSE: PointerInfo = PointerInfo {
    pointer_id: Some(PointerId::PRIMARY),
    persistent_device_id: None,
    pointer_type: PointerType::Mouse,
};

// TODO - Add kittest support
// - Being able to check that the tree has an access node
// - Getting a WidgetRef/WidgetMut to a node from a kittest::Queryable
// - Making a debug snapshot of the access tree
// https://github.com/rerun-io/kittest

// TODO - Re-enable doc test.
// Doc test is currently disabled because it depends on a parent crate.

/// A safe headless environment to test widgets in.
///
/// `TestHarness` is a type that simulates a [`RenderRoot`] for testing.
///
/// # Workflow
///
/// One of the main goals of Masonry is to provide primitives that allow application
/// developers to test their app in a convenient and intuitive way.
/// The basic testing workflow is as follows:
///
/// - Create a harness with some widget.
/// - Send events to the widget as if you were a user interacting with a window.
///   (Rewrite passes are handled automatically.)
/// - Check that the state of the widget graph matches what you expect.
///
/// You can do that last part in a few different ways.
/// You can get a [`WidgetRef`] to a specific widget through methods like [`get_widget_with_id`](Self::get_widget_with_id).
/// [`WidgetRef`] implements `Debug`, so you can check the state of an entire tree with something like the [`insta`] crate.
///
/// You can also render the widget tree directly with the [`render`](Self::render) method.
/// Masonry also provides the [`assert_render_snapshot`] macro, which performs snapshot testing on the
/// rendered widget tree automatically.
///
/// # Fidelity
///
/// `TestHarness` tries to act like the normal masonry environment. It will run the same passes as the normal app after every user event and animation.
///
/// Animations can be simulated with the [`animate_ms`](Self::animate_ms) method.
///
/// One minor difference is that paint only happens when the user explicitly calls rendering
/// methods, whereas in a normal applications you could reasonably expect multiple paint calls
/// between eg any two clicks.
///
/// # Example
///
/// ```rust,ignore
/// use insta::assert_debug_snapshot;
///
/// use masonry::core::PointerButton;
/// use masonry::core::Action;
/// use masonry::testing::assert_render_snapshot;
/// use masonry::testing::TestHarness;
/// use masonry::testing::TestWidgetExt;
/// use masonry::theme::default_property_set;
/// use masonry::widgets::Button;
/// # /*
/// #[test]
/// # */
/// fn simple_button() {
///     let widget = Button::new("Hello");
///
///     let mut harness = TestHarness::create(default_property_set(), widget);
///     let button_id = harness.root_id();
///
///     # if false {
///     assert_render_snapshot!(harness, "hello");
///     # }
///
///     assert_eq!(harness.pop_action().unwrap(), None);
///
///     harness.mouse_click_on(button_id);
///     assert_eq!(
///         harness.pop_action().unwrap(),
///         Some((Action::ButtonPressed(Some(PointerButton::Primary)), button_id))
///     );
/// }
///
/// # simple_button();
/// ```
///
/// [`assert_render_snapshot`]: crate::assert_render_snapshot
/// [`insta`]: https://docs.rs/insta/latest/insta/
#[derive(Debug)]
pub struct TestHarness<W: Widget> {
    signal_receiver: mpsc::Receiver<RenderRootSignal>,
    render_root: RenderRoot,
    root_id: WidgetId,
    access_tree: accesskit_consumer::Tree,
    renderer: Option<VelloCpuRenderer>,
    mouse_state: PointerState,
    window_size: PhysicalSize<u32>,
    root_padding: u32,
    background_color: Color,
    panic_on_rewrite_saturation: bool,
    max_screenshot_size: u32,
    action_queue: VecDeque<(ErasedAction, WidgetId)>,
    has_ime_session: bool,
    ime_rect: (LogicalPosition<f64>, LogicalSize<f64>),
    clipboard: String,
    title: String,
    _marker: PhantomData<W>,
}

/// Parameters for creating a [`TestHarness`].
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
#[must_use = "Use the configured harness parameters."]
pub struct TestHarnessParams {
    /// The size of the virtual window the harness renders into for snapshot testing.
    /// Defaults to [`TestHarnessParams::DEFAULT_SIZE`].
    pub window_size: PhysicalSize<u32>,
    /// The background color of the virtual window.
    /// Defaults to [`TestHarnessParams::DEFAULT_BACKGROUND_COLOR`].
    pub background_color: Color,
    /// Extra padding added to screenshots in [`assert_render_snapshot`].
    ///
    /// Currently defaults to zero, but we plan to change this
    /// to [`TestHarnessParams::ROOT_PADDING`] soon.
    /// This is useful:
    ///
    /// 1) For individual widgets, as they will often be designed with content outside their
    ///    border-box (e.g. drop shadows, focus indicators).
    /// 2) For full apps, as it allows (manual) validation that none of the app content is cut off by
    ///    the window border.
    ///
    /// [`assert_render_snapshot`]: crate::assert_render_snapshot
    pub root_padding: u32,
    /// The scale factor widgets are rendered at.
    /// Defaults to 1.0.
    pub scale_factor: f64,
    /// Whether to panic when we detect a loop in [rewrite passes](masonry_core::doc::pass_system#rewrite-passes).
    ///
    /// A loop means a case where the passes keep running because some passes keep
    /// invalidating flags for previous passes.
    pub panic_on_rewrite_saturation: bool,
    /// The largest size a screenshot file is allowed to be in this test.
    /// Defaults to `8KiB`.
    ///
    /// You can use [`TestHarnessParams::KIBIBYTE`] to help set this.
    /// Keeping screenshot files small avoids clones of this repository taking too long.
    /// Masonry testing uses [oxipng] to optimise the size of screenshots.
    pub max_screenshot_size: u32,
}

/// Assert a snapshot of a rendered frame of your app.
///
/// This macro takes a test harness and a name, renders the current state of the app,
/// and stores the rendered image to `<CRATE-ROOT>/screenshots/<TEST-NAME>.png`.
/// This rendering will have extra padding which would not be present in a real app,
/// as documented in [`TestHarnessParams::root_padding`].
///
/// If a screenshot already exists, the rendered value is compared against this screenshot.
/// The assert passes if both are equal; otherwise, a diff file is created.
/// If the test is run again and the new rendered value matches the old screenshot, the diff file is deleted.
///
/// If a screenshot doesn't exist, the assert will fail; the new screenshot is stored as
/// `<CRATE-ROOT>/screenshots/<TEST-NAME>.new.png`, and must be renamed before the assert will pass.
///
/// You can also run tests with the `MASONRY_TEST_BLESS` flag set to `1` to assume all
/// differences are intended and overwrite all the screenshots with new values.
#[macro_export]
macro_rules! assert_render_snapshot {
    ($test_harness:expr, $name:expr) => {{
        let result = $test_harness.check_render_snapshot(env!("CARGO_MANIFEST_DIR"), $name, false);
        assert!(result.is_ok(), "Snapshot assertion failed: {result:?}");
    }};
}

/// Assert a snapshot of a rendered frame of your app, expecting it to fail.
///
/// This macro does essentially the same thing as [`assert_render_snapshot`], but
/// instead of asserting that the rendered frame matches the existing screenshot,
/// it asserts that it does not match.
///
/// This is mostly used internally by Masonry to test that the image diffing does
/// detect changes and regressions.
///
/// This macro is read-only and will not write any new screenshots.
///
/// [`assert_render_snapshot`]: crate::assert_render_snapshot
#[macro_export]
macro_rules! assert_failing_render_snapshot {
    ($test_harness:expr, $name:expr) => {{
        let result = $test_harness.check_render_snapshot(env!("CARGO_MANIFEST_DIR"), $name, true);
        assert!(result.is_ok(), "Snapshot assertion failed: {result:?}");
    }};
}

impl TestHarnessParams {
    /// Default test param values.
    pub const DEFAULT: Self = Self {
        window_size: Self::DEFAULT_SIZE,
        background_color: Self::DEFAULT_BACKGROUND_COLOR,
        root_padding: 0,
        scale_factor: 1.0,
        panic_on_rewrite_saturation: true,
        max_screenshot_size: 8 * Self::KIBIBYTE,
    };

    /// Default canvas size for tests.
    pub const DEFAULT_SIZE: PhysicalSize<u32> = PhysicalSize::new(400, 400);

    /// Default background color for tests.
    pub const DEFAULT_BACKGROUND_COLOR: Color = Color::from_rgb8(0x29, 0x29, 0x29);

    /// Recommended root padding for tests.
    ///
    /// To be set as [`TestHarnessParams::root_padding`]
    pub const ROOT_PADDING: u32 = 5;

    /// One kibibyte. Used in [`TestHarnessParams::max_screenshot_size`].
    pub const KIBIBYTE: u32 = 1024;
}

impl Default for TestHarnessParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TestHarnessParams {
    /// Returns parameters for a test harness with custom dimensions.
    pub fn size_and_padding(window_size: impl Into<PhysicalSize<u32>>, root_padding: u32) -> Self {
        Self {
            window_size: window_size.into(),
            root_padding,
            ..Self::DEFAULT
        }
    }

    /// Builder method to set `window_size`.
    pub fn with_size(self, window_size: impl Into<PhysicalSize<u32>>) -> Self {
        Self {
            window_size: window_size.into(),
            ..self
        }
    }

    /// Builder method to set `background_color`.
    pub const fn with_background(self, background_color: Color) -> Self {
        Self {
            background_color,
            ..self
        }
    }

    /// Builder method to set `root_padding`.
    pub const fn with_padding(self, root_padding: u32) -> Self {
        Self {
            root_padding,
            ..self
        }
    }

    /// Builder method to set `scale_factor`.
    pub const fn with_scale(self, scale_factor: f64) -> Self {
        Self {
            scale_factor,
            ..self
        }
    }

    /// Builder method to set `max_screenshot_size`.
    pub const fn with_max_screenshot_size(self, max_screenshot_size: u32) -> Self {
        Self {
            max_screenshot_size,
            ..self
        }
    }
}

/// Roboto is our current test font.
pub const ROBOTO: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/fonts/roboto/Roboto-Regular.ttf"
));

impl<W: Widget> TestHarness<W> {
    // -- MARK: CREATE
    /// Builds harness with given root widget.
    ///
    /// Window size will be [`TestHarnessParams::DEFAULT_SIZE`].
    /// Background color will be [`TestHarnessParams::DEFAULT_BACKGROUND_COLOR`].
    #[must_use = "Use the constructed test harness."]
    pub fn create(default_props: DefaultProperties, root_widget: NewWidget<W>) -> Self {
        Self::create_with(default_props, root_widget, TestHarnessParams::default())
    }

    /// Builds harness with given root widget and window size.
    pub fn create_with_size(
        default_props: DefaultProperties,
        root_widget: NewWidget<W>,
        window_size: impl Into<PhysicalSize<u32>>,
    ) -> Self {
        Self::create_with(
            default_props,
            root_widget,
            TestHarnessParams::default().with_size(window_size),
        )
    }

    /// Builds harness with given root widget and additional parameters.
    ///
    /// # Panics
    ///
    /// Panics if the widget tree violates a test assertion, including rewrite-pass saturation.
    pub fn create_with(
        default_props: DefaultProperties,
        root_widget: NewWidget<W>,
        params: TestHarnessParams,
    ) -> Self {
        let root_id = root_widget.id();
        let mouse_state = PointerState::default();
        let window_size = params.window_size;

        // If no tracing subscriber has been set before, we set our own. If one has
        // already been set, we get an error which we swallow.
        // Having a default subscriber is helpful for tests; swallowing errors means
        // we don't panic if the user has already set one, or a test creates multiple
        // harnesses.
        let _ = try_init_test_tracing();

        let data = Blob::new(Arc::new(ROBOTO));

        let (signal_sender, signal_receiver) = mpsc::channel::<RenderRootSignal>();

        let dummy_tree_update = TreeUpdate {
            tree_id: TreeId::ROOT,
            nodes: vec![(0.into(), Node::new(Role::Window))],
            tree: Some(Tree {
                root: 0.into(),
                toolkit_name: None,
                toolkit_version: None,
            }),
            focus: 0.into(),
        };
        let mut harness = Self {
            signal_receiver,
            root_id,
            render_root: RenderRoot::new(
                root_widget,
                move |signal| {
                    let _ = signal_sender.send(signal);
                },
                RenderRootOptions {
                    default_properties: Arc::new(default_props),
                    use_system_fonts: false,
                    size_policy: WindowSizePolicy::User,
                    size: window_size,
                    scale_factor: params.scale_factor,
                    test_font: Some(data),
                },
            ),
            access_tree: accesskit_consumer::Tree::new(dummy_tree_update, false),
            renderer: None,
            mouse_state,
            window_size,
            background_color: params.background_color,
            root_padding: params.root_padding,
            panic_on_rewrite_saturation: params.panic_on_rewrite_saturation,
            max_screenshot_size: params.max_screenshot_size,
            action_queue: VecDeque::new(),
            has_ime_session: false,
            ime_rect: Default::default(),
            clipboard: String::new(),
            title: String::new(),
            _marker: PhantomData,
        };

        // Set up the initial state, and clear invalidation flags.
        harness.process_window_event(WindowEvent::EnableAccessTree);
        harness.animate_ms(0);

        let (_, tree_update) = harness.render_root.redraw();
        if let Some(tree_update) = tree_update {
            harness
                .access_tree
                .update_and_process_changes(tree_update, &mut NoOpTreeChangeHandler);
        }

        harness
    }

    // TODO - Reorder methods. Low-level methods should appear later in the doc than high-level methods.

    // --- MARK: PROCESS EVENTS

    /// Sends a [`WindowEvent`] to the simulated window.
    ///
    /// This will run [rewrite passes](masonry_core::doc::pass_system#rewrite-passes) after the event is processed.
    pub fn process_window_event(&mut self, event: WindowEvent) -> Handled {
        let handled = self.render_root.handle_window_event(event);
        self.process_signals();
        handled
    }

    /// Sends a [`PointerEvent`] to the simulated window.
    ///
    /// This will run [rewrite passes](masonry_core::doc::pass_system#rewrite-passes) after the event is processed.
    pub fn process_pointer_event(&mut self, event: &PointerEvent) -> Handled {
        let handled = self.render_root.handle_pointer_event(event);
        self.process_signals();
        handled
    }

    /// Sends a [`TextEvent`] to the simulated window.
    ///
    /// This will run [rewrite passes](masonry_core::doc::pass_system#rewrite-passes) after the event is processed.
    pub fn process_text_event(&mut self, event: &TextEvent) -> Handled {
        let handled = self.render_root.handle_text_event(event);
        self.process_signals();
        handled
    }

    /// Sends an [`ActionRequest`] to the simulated window.
    ///
    /// This will run [rewrite passes](masonry_core::doc::pass_system#rewrite-passes) after the event is processed.
    pub fn process_access_event(&mut self, event: ActionRequest) {
        self.render_root.handle_access_event(event);
        self.process_signals();
    }

    // This should be ran after any operation which runs the rewrite passes
    // (i.e. processing an event, etc.)
    fn process_signals(&mut self) {
        while let Some(signal) = self.signal_receiver.try_iter().next() {
            match signal {
                RenderRootSignal::Action(action, widget_id) => {
                    self.action_queue.push_back((action, widget_id));
                }
                RenderRootSignal::StartIme => {
                    self.has_ime_session = true;
                }
                RenderRootSignal::EndIme => {
                    self.has_ime_session = false;
                }
                RenderRootSignal::ImeMoved(position, size) => {
                    self.ime_rect = (position, size);
                }
                RenderRootSignal::ClipboardStore(text) => {
                    self.clipboard = text;
                }
                RenderRootSignal::SetSize(physical_size) => {
                    self.window_size = physical_size;
                    self.render_root
                        .handle_window_event(WindowEvent::Resize(physical_size));
                }
                RenderRootSignal::SetTitle(title) => {
                    self.title = title;
                }
                RenderRootSignal::RequestRedraw
                | RenderRootSignal::RequestAnimFrame
                | RenderRootSignal::TakeFocus
                | RenderRootSignal::SetCursor(_)
                | RenderRootSignal::DragWindow
                | RenderRootSignal::DragResizeWindow(_)
                | RenderRootSignal::ToggleMaximized
                | RenderRootSignal::Minimize
                | RenderRootSignal::Exit
                | RenderRootSignal::ShowWindowMenu(_)
                | RenderRootSignal::WidgetSelectedInInspector(_) => {}
                RenderRootSignal::NewLayer(_type, root, pos) => {
                    self.render_root.add_layer(root, pos);
                }
                RenderRootSignal::RemoveLayer(root_id) => self.render_root.remove_layer(root_id),
                RenderRootSignal::RepositionLayer(root_id, new_pos) => {
                    self.render_root.reposition_layer(root_id, new_pos);
                }
            }
        }

        assert!(
            !self.panic_on_rewrite_saturation || !self.render_root.needs_rewrite_passes(),
            "Loop detected in rewrite passes"
        );
    }

    // --- MARK: RENDER
    /// Renders the window into an image and updates the `accesskit_consumer` tree.
    ///
    /// The returned image contains a bitmap (an array of pixels) as an 8-bits-per-channel RGB image.
    /// The returned image has padding of the [`TestHarnessParams::root_padding`] this harness
    /// was created with on all sides.
    ///
    /// If you just want to run the rendering passes and get the render data without
    /// rastering it into an image, use [`Self::redraw`] instead.
    ///
    /// # Errors
    ///
    /// Returns an error for overflowing dimensions, rendering failure or invalid pixels.
    pub fn render(&mut self) -> Result<RgbaImage, HarnessError> {
        let (visual_layers, _tree_update) = self.redraw();

        if std::env::var("SKIP_RENDER_TESTS").is_ok_and(|it| !it.is_empty()) {
            return Ok(RgbaImage::from_pixel(1, 1, Rgba([255, 255, 255, 255])));
        }

        let (width, height) = {
            // Avoid having a zero-sized image.
            let padding = self
                .root_padding
                .checked_mul(2)
                .ok_or(HarnessError::ImageDimensions)?;
            let width = self
                .window_size
                .width
                .max(1)
                .checked_add(padding)
                .ok_or(HarnessError::ImageDimensions)?;
            let height = self
                .window_size
                .height
                .max(1)
                .checked_add(padding)
                .ok_or(HarnessError::ImageDimensions)?;
            (width, height)
        };

        let mut full_scene = Scene::new();
        {
            let (width, height) = { (f64::from(width), f64::from(height)) };
            let mut painter = Painter::new(&mut full_scene);
            painter.fill_rect(Rect::new(0.0, 0.0, width, height), self.background_color);

            let root_transform =
                Affine::translate((f64::from(self.root_padding), f64::from(self.root_padding)))
                    * Affine::scale(self.render_root.scale_factor());

            for layer in &visual_layers.layers {
                if let VisualLayerKind::Scene(scene) = &layer.kind {
                    replay_transformed(scene, &mut full_scene, root_transform * layer.transform);
                }
            }
        }

        let renderer = self
            .renderer
            .get_or_insert_with(|| VelloCpuRenderer::new(1, 1));
        let image = renderer.render_source(&mut full_scene, width, height)?;
        RgbaImage::from_vec(image.width, image.height, image.data)
            .ok_or(HarnessError::InvalidPixels)
    }

    /// Returns render root of the harness
    pub const fn render_root(&mut self) -> &mut RenderRoot {
        &mut self.render_root
    }

    /// Redraws the window.
    ///
    /// If you want to get a bitmap image of the contents, use [`Self::render`] instead.
    pub fn redraw(&mut self) -> (VisualLayerPlan, Option<TreeUpdate>) {
        let (visual_layers, tree_update) = self.render_root.redraw();
        if let Some(update) = &tree_update {
            self.access_tree
                .update_and_process_changes(update.clone(), &mut NoOpTreeChangeHandler);
        }

        (visual_layers, tree_update)
    }

    /// Returns a reference to the current state of the accessibility tree.
    pub const fn access_tree(&self) -> &accesskit_consumer::Tree {
        &self.access_tree
    }

    /// Returns a reference to the current value of a node of the accessibility tree.
    pub fn access_node(&self, id: WidgetId) -> Option<accesskit_consumer::Node<'_>> {
        // AccessKit 0.35 exposes the local ID through its public u128 conversion.
        // The harness owns a single root tree, so match the full local ID by walking
        // the exposed nodes rather than constructing a private NodeId.
        let mut pending = vec![self.access_tree.state().root()];
        while let Some(node) = pending.pop() {
            if u128::from(node.id()) >> 64 == u128::from(id.to_raw()) {
                return Some(node);
            }
            pending.extend(node.children());
        }
        None
    }

    // --- MARK: EVENT HELPERS

    /// Move an internal mouse state, and send a [`Move`](PointerEvent::Move) event to the window.
    ///
    /// The position is in logical coordinates.
    pub fn mouse_move(&mut self, pos: impl Into<Point>) {
        let Point { x, y } = pos.into();
        let scale_factor = self.render_root.scale_factor();
        self.mouse_state.position = LogicalPosition { x, y }.to_physical(scale_factor);
        self.mouse_state.scale_factor = scale_factor;

        debug!("Harness mouse moved to {x}, {y}");

        self.process_pointer_event(&PointerEvent::Move(PointerUpdate {
            pointer: PRIMARY_MOUSE,
            current: self.mouse_state.clone(),
            coalesced: vec![],
            predicted: vec![],
        }));
    }

    /// Sends a [`Down`](PointerEvent::Down) event to the window.
    pub fn mouse_button_press(&mut self, button: Option<PointerButton>) {
        if let Some(button) = button {
            self.mouse_state.buttons.insert(button);
        }
        self.process_pointer_event(&PointerEvent::Down(PointerButtonEvent {
            pointer: PRIMARY_MOUSE,
            button,
            state: self.mouse_state.clone(),
        }));
    }

    /// Sends an [`Up`](PointerEvent::Up) event to the window.
    pub fn mouse_button_release(&mut self, button: Option<PointerButton>) {
        if let Some(button) = button {
            self.mouse_state.buttons.remove(button);
        }
        self.process_pointer_event(&PointerEvent::Up(PointerButtonEvent {
            pointer: PRIMARY_MOUSE,
            button,
            state: self.mouse_state.clone(),
        }));
    }

    /// Sends a [`Scroll`](PointerEvent::Scroll) event to the window.
    ///
    /// The delta is in logical pixels.
    pub fn mouse_wheel(&mut self, Vec2 { x, y }: Vec2) {
        let scale_factor = self.render_root.scale_factor();
        self.process_pointer_event(&PointerEvent::Scroll(PointerScrollEvent {
            pointer: PRIMARY_MOUSE,
            delta: ScrollDelta::PixelDelta(LogicalPosition { x, y }.to_physical(scale_factor)),
            state: self.mouse_state.clone(),
        }));
    }

    /// Moves the mouse to an interactive widget, then presses and releases the button.
    ///
    /// # Errors
    ///
    /// Returns an error if the widget cannot be reached.
    #[track_caller]
    pub fn mouse_click_on(
        &mut self,
        id: WidgetId,
        button: Option<PointerButton>,
    ) -> Result<(), HarnessError> {
        self.mouse_move_to(id)?;
        self.mouse_button_press(button);
        self.mouse_button_release(button);
        Ok(())
    }

    /// Moves the mouse to the center of an interactive and visible widget.
    ///
    /// # Errors
    ///
    /// Returns an error if the widget is absent, stashed, non-interactive, or obscured.
    pub fn mouse_move_to(&mut self, id: WidgetId) -> Result<(), HarnessError> {
        let widget = self
            .get_widget_with_id(id)
            .ok_or(HarnessError::MissingWidget(id))?;
        let center = widget.ctx().window_transform() * widget.ctx().border_box().center();
        if widget.ctx().is_stashed() || !widget.ctx().accepts_pointer_interaction() {
            return Err(HarnessError::UnreachableWidget(id));
        }
        if self
            .render_root
            .get_layer_root(0)
            .and_then(|root| root.find_widget_under_pointer(center))
            .map(|widget| widget.id())
            != Some(id)
        {
            return Err(HarnessError::UnreachableWidget(id));
        }
        self.mouse_move(center);
        Ok(())
    }

    /// Moves to a widget's center without testing pointer interactivity or occlusion.
    ///
    /// # Errors
    ///
    /// Returns an error if the widget is absent or stashed.
    pub fn mouse_move_to_unchecked(&mut self, id: WidgetId) -> Result<(), HarnessError> {
        let widget = self
            .get_widget_with_id(id)
            .ok_or(HarnessError::MissingWidget(id))?;
        if widget.ctx().is_stashed() {
            return Err(HarnessError::UnreachableWidget(id));
        }
        let center = widget.ctx().window_transform() * widget.ctx().border_box().center();
        self.mouse_move(center);
        Ok(())
    }

    /// Tries to get the target widget into the viewport.
    ///
    /// This will send an accesskit [`ScrollIntoView`] action to the widget,
    /// which will usually send [`RequestPanToChild`] events to the widget's parents.
    /// If the widget is hidden because it's "scrolled away", this should make it visible again.
    ///
    /// [`RequestPanToChild`]: masonry_core::core::Update::RequestPanToChild
    /// [`ScrollIntoView`]: masonry_core::accesskit::Action::ScrollIntoView
    #[track_caller]
    pub fn scroll_into_view(&mut self, id: WidgetId) {
        self.render_root.handle_access_event(ActionRequest {
            action: Action::ScrollIntoView,
            target_tree: TreeId::ROOT,
            target_node: id.to_raw().into(),
            data: None,
        });
    }

    /// Clicks the given widget.
    ///
    /// This will send an accesskit [`Click`] action to the widget,
    /// which may have different behavior compared to a normal mouse click.
    ///
    /// Unlike [`Self::mouse_click_on`], the event will be routed to the target widget
    /// even if it's not visible, or hidden behind other widgets.
    ///
    /// # Panics
    ///
    /// - If the widget is not found in the tree.
    ///
    /// [`Click`]: masonry_core::accesskit::Action::Click
    #[track_caller]
    pub fn accessibility_click_on(&mut self, id: WidgetId) {
        self.render_root.handle_access_event(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: id.to_raw().into(),
            data: None,
        });
    }

    // TODO - Handle complicated IME
    // TODO - Mock Winit keyboard events
    /// Sends a [`TextEvent`] for each character in the given string.
    pub fn keyboard_type_chars(&mut self, text: &str) {
        // For each character
        for c in text.split("").filter(|s| !s.is_empty()) {
            let event = TextEvent::Ime(Ime::Commit(c.to_string()));
            self.render_root.handle_text_event(&event);
        }
        self.process_signals();
    }

    /// Sends a [`TextEvent`] representing the user pressing the `Tab` key, either with or without the `Shift` key pressed.
    pub fn press_tab_key(&mut self, shift: bool) {
        let modifiers = if shift {
            Modifiers::SHIFT
        } else {
            Modifiers::empty()
        };
        let event = TextEvent::Keyboard(KeyboardEvent {
            state: KeyState::Down,
            key: Key::Named(NamedKey::Tab),
            code: Code::Unidentified,
            modifiers,
            ..KeyboardEvent::default()
        });
        self.render_root.handle_text_event(&event);
        self.process_signals();
    }

    /// Sets the [focused widget](masonry_core::doc::masonry_concepts#text-focus)
    /// and the [focus anchor](masonry_core::doc::masonry_concepts#focus-anchor).
    ///
    /// # Panics
    ///
    /// If the widget is not found in the tree or can't be focused.
    #[track_caller]
    pub fn focus_on(&mut self, id: Option<WidgetId>) {
        assert!(
            self.render_root.focus_on(id),
            "Cannot focus widget {id:?}: absent, stashed or disabled"
        );
        self.process_signals();
    }

    /// Sets the [focus fallback](masonry_core::doc::masonry_concepts#focus-fallback).
    ///
    /// # Panics
    ///
    /// Panics if the target cannot be focused.
    pub fn set_focus_fallback(&mut self, id: Option<WidgetId>) {
        assert!(
            self.render_root.set_focus_fallback(id),
            "Cannot set focus fallback to {id:?}"
        );
    }

    /// Runs an animation pass on the widget tree.
    pub fn animate_ms(&mut self, ms: u64) {
        self.render_root
            .handle_window_event(WindowEvent::AnimFrame(Duration::from_millis(ms)));
        self.process_signals();
    }

    /// Helper method to directly enable/disable a widget.
    pub fn set_disabled(&mut self, widget: WidgetTag<impl Widget>, disabled: bool) -> Option<()> {
        self.edit_widget(widget, |mut target| {
            target.ctx.set_disabled(disabled);
        })
    }

    // --- MARK: GETTERS

    /// Returns the base-layer widget if it is registered with the expected type.
    pub fn root_widget(&self) -> Option<WidgetRef<'_, W>> {
        self.render_root
            .get_layer_root(0)
            .and_then(|root| root.downcast())
    }

    /// Returns the identifier assigned to the base widget at construction.
    pub const fn root_id(&self) -> WidgetId {
        self.root_id
    }

    /// Replace the tree-wide default properties at runtime.
    ///
    /// Mirrors [`RenderRoot::set_default_properties`].
    pub fn set_default_properties(&mut self, default_properties: Arc<DefaultProperties>) {
        self.render_root.set_default_properties(default_properties);
    }

    /// Returns a registered widget with the given id, if present.
    pub fn get_widget_with_id(&self, id: WidgetId) -> Option<WidgetRef<'_, dyn Widget>> {
        self.render_root.get_widget(id)
    }

    /// Returns a registered widget with the given tag and type, if present.
    pub fn get_widget<W2: Widget + FromDynWidget + ?Sized>(
        &self,
        tag: WidgetTag<W2>,
    ) -> Option<WidgetRef<'_, W2>> {
        self.render_root.get_widget_with_tag(tag)
    }

    /// Drains events recorded by the given widget, if present.
    pub fn take_records_of<W2: Widget>(&self, tag: WidgetTag<Recorder<W2>>) -> Option<Vec<Record>> {
        Some(self.get_widget(tag)?.inner().recording().drain())
    }

    /// Flushes events recorded by the given widget, if present.
    pub fn flush_records_of<W2: Widget>(&self, tag: WidgetTag<Recorder<W2>>) -> Option<()> {
        self.get_widget(tag)?.inner().recording().clear();
        Some(())
    }

    /// Returns a [`WidgetRef`] to the [focused widget](masonry_core::doc::masonry_concepts#text-focus).
    pub fn focused_widget(&self) -> Option<WidgetRef<'_, dyn Widget>> {
        self.render_root
            .get_layer_root(0)?
            .find_widget_by_id(self.render_root.focused_widget()?)
    }

    /// Returns the id of the [focused widget](masonry_core::doc::masonry_concepts#text-focus).
    pub const fn focused_widget_id(&self) -> Option<WidgetId> {
        self.render_root.focused_widget()
    }

    /// Returns a [`WidgetRef`] to the widget which [captures pointer events](masonry_core::doc::masonry_concepts#pointer-capture).
    pub fn pointer_capture_target(&self) -> Option<WidgetRef<'_, dyn Widget>> {
        self.render_root
            .get_widget(self.render_root.pointer_capture_target()?)
    }

    /// Returns the id of the widget which [captures pointer events](masonry_core::doc::masonry_concepts#pointer-capture).
    // TODO - This is kinda redundant with the above
    pub const fn pointer_capture_target_id(&self) -> Option<WidgetId> {
        self.render_root.pointer_capture_target()
    }

    /// Calls the provided visitor on every widget in the widget tree.
    pub fn inspect_widgets(&self, mut f: impl FnMut(WidgetRef<'_, dyn Widget>)) {
        fn inspect(
            widget: WidgetRef<'_, dyn Widget>,
            f: &mut impl FnMut(WidgetRef<'_, dyn Widget>),
        ) {
            f(widget);
            for child in widget.children() {
                inspect(child, f);
            }
        }

        if let Some(root) = self.render_root.get_layer_root(0) {
            inspect(root, &mut f);
        }
    }

    /// Returns a [`WidgetMut`] to the root widget.
    ///
    /// Because of how `WidgetMut` works, it can only be passed to a user-provided callback.
    pub fn edit_root_widget<R>(&mut self, f: impl FnOnce(WidgetMut<'_, W>) -> R) -> Option<R> {
        let ret = self.render_root.edit_base_layer(f);
        self.process_signals();
        ret
    }

    /// Returns a [`WidgetMut`] to a specific widget.
    ///
    /// Because of how `WidgetMut` works, it can only be passed to a user-provided callback.
    pub fn edit_widget_with_id<R>(
        &mut self,
        id: WidgetId,
        f: impl FnOnce(WidgetMut<'_, dyn Widget>) -> R,
    ) -> Option<R> {
        let ret = self.render_root.edit_widget(id, f);
        self.process_signals();
        ret
    }

    /// Returns a [`WidgetMut`] to the widget with the given tag.
    ///
    /// Because of how `WidgetMut` works, it can only be passed to a user-provided callback.
    #[track_caller]
    pub fn edit_widget<R, W2: Widget + FromDynWidget + ?Sized>(
        &mut self,
        tag: WidgetTag<W2>,
        f: impl FnOnce(WidgetMut<'_, W2>) -> R,
    ) -> Option<R> {
        let ret = self.render_root.edit_widget_with_tag(tag, f);
        self.process_signals();
        ret
    }

    /// Pops and downcasts the oldest action, preserving an unexpected action as an error.
    ///
    /// # Errors
    ///
    /// Returns the original action and widget identifier if its type does not match.
    pub fn pop_action<T: AnyDebug>(
        &mut self,
    ) -> Result<Option<(T, WidgetId)>, (ErasedAction, WidgetId)> {
        let Some((action, widget)) = self.pop_action_erased() else {
            return Ok(None);
        };
        action
            .downcast()
            .map(|action| Some((*action, widget)))
            .map_err(|action| (action, widget))
    }

    /// Pops the oldest [`ErasedAction`] emitted by the widget tree.
    pub fn pop_action_erased(&mut self) -> Option<(ErasedAction, WidgetId)> {
        self.action_queue.pop_front()
    }

    /// Returns the app's current cursor icon.
    ///
    /// The cursor icon is the icon that would be displayed to indicate the mouse
    /// position in a visual environment.
    pub const fn cursor_icon(&self) -> CursorIcon {
        self.render_root.cursor_icon()
    }

    /// Returns whether the app has an IME session in progress.
    ///
    /// This usually means that a widget which [accepts text input](Widget::accepts_text_input) is focused.
    pub const fn has_ime_session(&self) -> bool {
        self.has_ime_session
    }

    /// Returns the rectangle of the IME session.
    ///
    /// This is usually the effective border-box rectangle of the focused widget.
    pub const fn ime_rect(&self) -> (LogicalPosition<f64>, LogicalSize<f64>) {
        self.ime_rect
    }

    /// Returns the contents of the emulated clipboard.
    ///
    /// This is an empty string by default.
    pub fn clipboard_contents(&self) -> String {
        self.clipboard.clone()
    }

    /// Returns the size of the simulated window.
    pub const fn window_size(&self) -> PhysicalSize<u32> {
        self.window_size
    }

    /// Returns the title of the simulated window.
    pub fn title(&self) -> String {
        self.title.clone()
    }

    // --- MARK: SNAPSHOT

    /// Renders the current widget tree and writes a uniquely named temporary PNG file.
    ///
    /// # Errors
    ///
    /// Returns rendering, time or file errors.
    pub fn save_render_snapshot(&mut self) -> Result<PathBuf, HarnessError> {
        let image = self.render()?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_nanos();
        let pid = std::process::id();
        let path =
            std::env::temp_dir().join(format!("masonry-{pid}-{timestamp:032x}-screenshot.png"));
        let mut file = File::create_new(&path)?;
        image.write_to(&mut file, ImageFormat::Png)?;
        debug!("Screenshot saved to {}", path.display());
        Ok(path)
    }

    /// Compares the rendered widget tree with a reference snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error for a mismatch, unexpected match, missing reference, or file/rendering failure.
    #[doc(hidden)]
    pub fn check_render_snapshot(
        &mut self,
        manifest_dir: &str,
        test_name: &str,
        expect_failure: bool,
    ) -> Result<(), HarnessError> {
        if std::env::var("SKIP_RENDER_TESTS").is_ok_and(|it| !it.is_empty()) {
            let _ = self.redraw();
            return Ok(());
        }
        let image = self.render()?;
        crate::screenshots::compare_snapshot(
            image,
            Path::new(manifest_dir),
            test_name,
            expect_failure,
            self.max_screenshot_size,
            std::env::var_os("MASONRY_TEST_BLESS").is_some_and(|it| !it.is_empty()),
        )
    }
}

struct NoOpTreeChangeHandler;

impl accesskit_consumer::TreeChangeHandler for NoOpTreeChangeHandler {
    fn node_added(&mut self, _node: &accesskit_consumer::Node<'_>) {}

    fn node_updated(
        &mut self,
        _old_node: &accesskit_consumer::Node<'_>,
        _new_node: &accesskit_consumer::Node<'_>,
    ) {
    }

    fn focus_moved(
        &mut self,
        _old_node: Option<&accesskit_consumer::Node<'_>>,
        _new_node: Option<&accesskit_consumer::Node<'_>>,
    ) {
    }

    fn node_removed(&mut self, _node: &accesskit_consumer::Node<'_>) {}
}
