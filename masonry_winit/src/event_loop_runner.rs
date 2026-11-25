// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Debug;
use std::sync::mpsc::Sender;
use std::sync::{Arc, mpsc};

use accesskit_winit::Adapter;
use copypasta::nop_clipboard::NopClipboardContext;
use copypasta::{ClipboardContext, ClipboardProvider};
use masonry_core::app::{
    RenderRoot, RenderRootOptions, RenderRootSignal, VisualLayerKind, WindowSizePolicy,
};
use masonry_core::core::keyboard::{Key, KeyState};
use masonry_core::core::{
    DefaultProperties, ErasedAction, NewWidget, TextEvent, Widget, WindowEvent,
};
use masonry_core::peniko::Color;
use masonry_core::util::Instant;
use masonry_imaging::texture_render::{
    RenderTarget as ImagingRenderTarget, Renderer as ImagingRenderer,
};
use masonry_imaging::{Layer as ImagingLayer, PreparedFrame};
use tracing::{info, info_span, trace};
use ui_events_winit::{WindowEventReducer, WindowEventTranslation};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::error::{EventLoopError, OsError};
use winit::event::WindowEvent as WinitWindowEvent;
use winit::event_loop::{ActiveEventLoop, OwnedDisplayHandle};
use winit::window::{Window as WindowHandle, WindowAttributes, WindowId as HandleId};

use crate::app::{
    AppDriver, DriverCtx, GpuDeviceId, WgpuContext, WgpuLimits, masonry_resize_direction_to_winit,
    winit_ime_to_masonry,
};
use crate::app_driver::WindowId;
use crate::vello_util::{RenderContext, RenderSurface, RenderSurfaceError};

/// The custom event type that we inject into winit's [`EventLoop`](winit::event_loop::EventLoop).
///
/// This represents the types that can be emitted during the event loop, but aren't emitted
/// by winit.
#[derive(Debug)]
pub enum MasonryUserEvent {
    /// An accessibility API emitted an event.
    ///
    /// This is how [`accesskit_winit`] routes its events to us.
    AccessKit(HandleId, accesskit_winit::WindowEvent),
    /// An action was emitted by something other than the widget tree.
    ///
    /// Higher-level GUI frameworks may send these to winit from background threads to wake up the event loop.
    AsyncAction(WindowId, ErasedAction),
    /// A WGPU device became unusable; affected surfaces must be rebuilt.
    #[doc(hidden)]
    GpuDeviceLost(GpuDeviceId),
}

impl From<accesskit_winit::Event> for MasonryUserEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::AccessKit(event.window_id, event.window_event)
    }
}

/// An error creating a window.
#[derive(Debug)]
pub enum WindowCreationError {
    /// The identifier is already assigned to a live or queued window.
    DuplicateId(WindowId),
    /// The platform rejected window creation.
    Platform(OsError),
}

impl core::fmt::Display for WindowCreationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DuplicateId(id) => write!(f, "Window identifier {id:?} is already in use"),
            Self::Platform(err) => write!(f, "Window creation failed: {err}"),
        }
    }
}

impl std::error::Error for WindowCreationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DuplicateId(_) => None,
            Self::Platform(err) => Some(err),
        }
    }
}

/// A container for a window yet to be created.
///
/// This is stored inside [`MasonryState`] and will be created during the `resumed` event.
#[derive(Debug)]
#[must_use = "Pass the configured window to the application."]
pub struct NewWindow {
    /// The id is set by the App, and can be created using the [`WindowId::next()`] method.
    ///
    /// Once the window is created, it can be accessed using this `id` through the
    /// [`DriverCtx::window()`] method.
    pub id: WindowId,
    /// Window attributes for the winit's [`Window`].
    ///
    /// A default attribute can be created using [`Window::default_attributes()`].
    ///
    /// [`Window`]: crate::winit::window::Window
    /// [`Window::default_attributes()`]: crate::winit::window::Window::default_attributes()
    pub attributes: WindowAttributes,
    /// The widget which will take up the entire contents of the new window.
    pub root_widget: NewWidget<dyn Widget>,
    /// The base color of the window.
    pub base_color: Color,
    /// Optional per-window override of the application default properties.
    pub default_properties: Option<Arc<DefaultProperties>>,
}

impl NewWindow {
    /// Creates a new window with an automatically assigned [`WindowId`].
    ///
    /// See the documentation on the fields of this type for details of the parameters.
    pub fn new(attributes: WindowAttributes, root_widget: NewWidget<dyn Widget + 'static>) -> Self {
        Self::new_with_id(WindowId::next(), attributes, root_widget)
    }

    /// Creates a new window with a custom assigned [`WindowId`].
    ///
    /// Use this when you need to specify a unique ID for the window, for example,
    /// for external tracking or state management.
    ///
    /// Every window you create should have a unique id.
    /// You should never recycle ids from previous windows, even after deleting them.
    pub fn new_with_id(
        id: WindowId,
        attributes: WindowAttributes,
        root_widget: NewWidget<dyn Widget + 'static>,
    ) -> Self {
        Self {
            id,
            attributes,
            root_widget,
            base_color: Color::BLACK,
            default_properties: None,
        }
    }

    /// Sets the base color of the new window.
    ///
    /// The base color is the color of the background which all widgets in the window draw on top of.
    /// Masonry's current default theme assumes that this will be a very dark color for sufficient contrast.
    /// This is most useful for apps which want to for example support light mode.
    ///
    /// To change it at runtime, use [`Window::set_base_color`].
    pub const fn with_base_color(mut self, base_color: Color) -> Self {
        self.base_color = base_color;
        self
    }

    /// Sets a window-specific default property set.
    pub fn with_default_properties(mut self, default_properties: Arc<DefaultProperties>) -> Self {
        self.default_properties = Some(default_properties);
        self
    }
}

/// Per-Window state
pub struct Window {
    id: WindowId,
    pub(crate) handle: Arc<WindowHandle>,
    pub(crate) accesskit_adapter: Adapter,
    event_reducer: WindowEventReducer,
    pub(crate) render_root: RenderRoot,
    pub(crate) base_color: Color,
    application_properties: Arc<DefaultProperties>,
    last_anim: Option<Instant>,
    present_mode: wgpu::PresentMode,
    surface_recovery_attempted: bool,
}

impl Window {
    pub(crate) fn new(
        new_window: NewWindow,
        handle: Arc<WindowHandle>,
        accesskit_adapter: Adapter,
        signal_sender: Sender<(WindowId, RenderRootSignal)>,
        mut options: RenderRootOptions,
    ) -> Self {
        let window_id = new_window.id;
        let application_properties = options.default_properties.clone();
        if let Some(properties) = new_window.default_properties {
            options.default_properties = properties;
        }
        let mut window = Self {
            id: window_id,
            handle,
            accesskit_adapter,
            event_reducer: WindowEventReducer::default(),
            render_root: RenderRoot::new(
                new_window.root_widget,
                move |signal| {
                    if signal_sender.send((window_id, signal)).is_err() {
                        trace!("Dropping signal after the event loop closed");
                    }
                },
                options,
            ),
            base_color: new_window.base_color,
            application_properties,
            last_anim: None,
            present_mode: wgpu::PresentMode::AutoVsync,
            surface_recovery_attempted: false,
        };
        if !window.handle.has_focus() {
            window
                .render_root
                .handle_text_event(&TextEvent::WindowFocusChange(false));
        }
        window
    }

    /// Access the underlying [Winit window](WindowHandle) of this window.
    pub fn handle(&self) -> &winit::window::Window {
        &self.handle
    }

    /// Access the [`RenderRoot`] of this window.
    pub const fn render_root(&mut self) -> &mut RenderRoot {
        &mut self.render_root
    }

    /// Returns the current base color.
    #[must_use = "Inspect the window background."]
    pub const fn base_color(&self) -> Color {
        self.base_color
    }

    /// Changes the window background and schedules a redraw if it changed.
    pub fn set_base_color(&mut self, color: Color) {
        if self.base_color != color {
            self.base_color = color;
            self.handle.request_redraw();
        }
    }

    /// Sets a window-specific property set, or restores the application defaults.
    pub fn set_default_properties(&mut self, properties: Option<Arc<DefaultProperties>>) {
        self.render_root.set_default_properties(
            properties.unwrap_or_else(|| self.application_properties.clone()),
        );
    }

    fn update_size(&mut self, size: PhysicalSize<u32>) {
        if self.render_root.size() != size {
            self.surface_recovery_attempted = false;
            self.render_root
                .handle_window_event(WindowEvent::Resize(size));
        }
    }

    fn sync_geometry(&mut self) {
        let scale = self.handle.scale_factor();
        if self.render_root.scale_factor().to_bits() != scale.to_bits() {
            self.render_root
                .handle_window_event(WindowEvent::Rescale(scale));
        }
        self.update_size(window_size(&self.handle));
    }
}

impl Debug for Window {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Window")
            .field("id", &self.id)
            .field("handle", &self.handle)
            .field("event_reducer", &self.event_reducer)
            .field("render_root", &self.render_root)
            .field("base_color", &self.base_color)
            .finish_non_exhaustive()
    }
}

/// The state of the Masonry application.
///
/// If you run Masonry from an external Winit event loop, create a
/// `MasonryState` via [`MasonryState::new`] and forward events to it via the appropriate method (e.g.,
/// calling [`handle_window_event`](MasonryState::handle_window_event) in [`window_event`](ApplicationHandler::window_event)).
pub struct MasonryState {
    /// The event loop is suspended when the app is e.g. in the background on Android.
    /// We aren't allowed to have any `Surface`s, and we also don't expect to receive any events.
    /// See [`ApplicationHandler::suspended()`] for details.
    is_suspended: bool,
    started: bool,
    render_cx: RenderContext,
    // TODO: Winit doesn't seem to let us create these proxies from within the loop
    // The reasons for this are unclear
    event_loop_proxy: EventLoopProxy,
    #[cfg(feature = "tracy")]
    frame: Option<tracing_tracy::client::Frame>,

    window_id_to_handle_id: HashMap<WindowId, HandleId>,

    surfaces: HashMap<HandleId, RenderSurface<'static>>,
    windows: HashMap<HandleId, Window>,
    /// On Metal, we need to track the state of resize requests to avoid jitter.
    #[cfg(target_os = "macos")]
    resized_window: Option<HandleId>,

    clipboard_cx: Box<dyn ClipboardProvider>,

    signal_receiver: mpsc::Receiver<(WindowId, RenderRootSignal)>,

    signal_sender: Sender<(WindowId, RenderRootSignal)>,
    default_properties: Arc<DefaultProperties>,
    pub(crate) exit: bool,
    /// Windows that are scheduled to be created in the next resumed event.
    new_windows: Vec<NewWindow>,
    need_first_frame: Vec<HandleId>,
    uninitialized_windows: VecDeque<WindowId>,
    suspended_actions: VecDeque<(WindowId, ErasedAction)>,
}

// TODO - Merge into MasonryState?
struct MainState {
    masonry_state: MasonryState,
    app_driver: Box<dyn AppDriver>,
}

/// The type of the event loop used by Masonry.
///
/// This *will* be changed to allow custom event types, but is implemented this way for expedience
pub type EventLoop = winit::event_loop::EventLoop<MasonryUserEvent>;
/// The type of the event loop builder used by Masonry.
///
/// This *will* be changed to allow custom event types, but is implemented this way for expedience
pub type EventLoopBuilder = winit::event_loop::EventLoopBuilder<MasonryUserEvent>;

/// A proxy used to send events to the event loop
pub type EventLoopProxy = winit::event_loop::EventLoopProxy<MasonryUserEvent>;

// --- MARK: RUN

/// Runs the app to completion.
///
/// This is usually one of the last functions called in your main function.
///
/// # Errors
/// Returns an error if the platform event loop cannot be created or run.
pub fn run(
    new_windows: Vec<NewWindow>,
    app_driver: impl AppDriver + 'static,
    default_properties: DefaultProperties,
) -> Result<(), EventLoopError> {
    let event_loop = EventLoop::with_user_event().build()?;

    run_with(event_loop, new_windows, app_driver, default_properties)
}

/// Runs the app with the provided event loop to completion.
///
/// # Errors
/// Returns an error if the platform event loop fails.
pub fn run_with(
    // This is passed in mostly to allow configuring the Android app
    event_loop: EventLoop,
    new_windows: Vec<NewWindow>,
    app_driver: impl AppDriver + 'static,
    default_properties: DefaultProperties,
) -> Result<(), EventLoopError> {
    // If no tracing subscriber has been set before, we set our own. If one has
    // already been set, we get an error which we swallow.
    // By now, we're about to take control of the event loop. The user is unlikely
    // to try to set their own subscriber once the event loop has started.
    let _ = masonry_core::app::try_init_tracing();

    let mut main_state = MainState {
        masonry_state: MasonryState::new(
            event_loop.create_proxy(),
            event_loop.owned_display_handle(),
            new_windows,
            default_properties,
        ),
        app_driver: Box::new(app_driver),
    };

    event_loop.run_app(&mut main_state)
}

impl ApplicationHandler<MasonryUserEvent> for MainState {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.masonry_state
            .handle_resumed(event_loop, &mut *self.app_driver);
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        self.masonry_state.handle_suspended(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        handle_id: HandleId,
        event: WinitWindowEvent,
    ) {
        self.masonry_state.handle_window_event(
            event_loop,
            handle_id,
            event,
            self.app_driver.as_mut(),
        );
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: MasonryUserEvent) {
        self.masonry_state
            .handle_user_event(event_loop, event, self.app_driver.as_mut());
    }
}

impl Debug for MasonryState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MasonryState")
            .field("is_suspended", &self.is_suspended)
            .field("started", &self.started)
            .field(
                "gpu_device_ids",
                &self.render_cx.devices.keys().collect::<Vec<_>>(),
            )
            .field("event_loop_proxy", &self.event_loop_proxy)
            .field("window_id_to_handle_id", &self.window_id_to_handle_id)
            .field("surfaces", &self.surfaces)
            .field("windows", &self.windows)
            .field("signal_receiver", &self.signal_receiver)
            .field("signal_sender", &self.signal_sender)
            .field("default_properties", &self.default_properties)
            .field("exit", &self.exit)
            .field("new_windows", &self.new_windows)
            .field("need_first_frame", &self.need_first_frame)
            .field("uninitialized_windows", &self.uninitialized_windows)
            .field("suspended_action_count", &self.suspended_actions.len())
            .finish_non_exhaustive()
    }
}

impl MasonryState {
    /// Creates the Masonry application's composition root.
    ///
    /// - `event_loop_proxy`: a queue provided by [`EventLoop::create_proxy`](winit::event_loop::EventLoop::create_proxy) to send custom events (mostly accessibility) to your event loop.
    /// - `display`: the display of your event loop, provided by [`EventLoop::owned_display_handle`](winit::event_loop::EventLoop::owned_display_handle).
    /// - `new_windows`: the initial list of windows.
    /// - `default_properties`: the default properties for all the widgets of the app.
    pub fn new(
        event_loop_proxy: EventLoopProxy,
        display: OwnedDisplayHandle,
        new_windows: Vec<NewWindow>,
        default_properties: DefaultProperties,
    ) -> Self {
        tracing::debug!(
            backend = ImagingRenderer::BACKEND_NAME,
            "selected Masonry Winit render backend"
        );

        let render_cx = RenderContext::new(display, event_loop_proxy.clone());

        let (signal_sender, signal_receiver) = mpsc::channel::<(WindowId, RenderRootSignal)>();

        let clipboard_cx =
            ClipboardContext::new().map(|cx| -> Box<dyn ClipboardProvider> { Box::new(cx) });
        let clipboard_cx = clipboard_cx.unwrap_or_else(|err| {
            tracing::warn!(?err, "Clipboard unavailable");
            Box::new(NopClipboardContext)
        });

        Self {
            is_suspended: true,
            started: false,
            render_cx,
            event_loop_proxy,
            #[cfg(feature = "tracy")]
            frame: None,
            signal_receiver,

            window_id_to_handle_id: HashMap::new(),
            windows: HashMap::new(),
            surfaces: HashMap::new(),
            #[cfg(target_os = "macos")]
            resized_window: None,

            clipboard_cx,

            signal_sender,
            default_properties: Arc::new(default_properties),
            exit: false,
            new_windows,
            need_first_frame: Vec::new(),
            uninitialized_windows: VecDeque::new(),
            suspended_actions: VecDeque::new(),
        }
    }

    /// Configure how Masonry requests the WGPU device (features and limits).
    ///
    /// This must be called before the first surface/device is created (i.e. before the first
    /// redraw). A good place is [`AppDriver::on_start`].
    ///
    /// `features` is best-effort: Masonry intersects it with `adapter.features()`, so unsupported
    /// feature bits are ignored.
    ///
    /// This method will return `false` if it was called too late and a device had already been
    /// created.
    pub fn set_wgpu_device_options(
        &mut self,
        features: wgpu::Features,
        limits: WgpuLimits,
    ) -> bool {
        self.render_cx.set_wgpu_device_options(features, limits)
    }

    /// Add extra WGPU features to request when creating the device.
    ///
    /// This must be called before the first surface/device is created (i.e. before the first
    /// redraw). A good place is [`AppDriver::on_start`].
    ///
    /// This method will return `false` if it was called too late and a device had already been
    /// created.
    pub fn add_wgpu_features(&mut self, features: wgpu::Features) -> bool {
        self.render_cx.add_wgpu_features(features)
    }

    /// Configure the WGPU limits strategy used when requesting the device.
    ///
    /// This must be called before the first surface/device is created (i.e. before the first
    /// redraw). A good place is [`AppDriver::on_start`].
    ///
    /// This method will return `false` if it was called too late and a device had already been
    /// created.
    pub fn set_wgpu_limits(&mut self, limits: WgpuLimits) -> bool {
        self.render_cx.set_wgpu_limits(limits)
    }

    // --- MARK: RESUMED
    /// Delegate method for [`ApplicationHandler::resumed()`].
    pub fn handle_resumed(&mut self, event_loop: &ActiveEventLoop, app_driver: &mut dyn AppDriver) {
        if !self.is_suspended {
            // Short-circuiting since we have already
            // handled the resumed event before this.
            return;
        }

        self.is_suspended = false;

        for new_window in std::mem::take(&mut self.new_windows) {
            let id = new_window.id;
            if let Err(err) = self.create_window(event_loop, new_window) {
                tracing::error!(%err, "Unable to create requested window");
                // A duplicate request is not a failure of the already live window.
                if !self.window_id_to_handle_id.contains_key(&id) {
                    app_driver.on_window_creation_failed(id, &err);
                }
            }
        }

        self.initialize_windows(app_driver);
        if !std::mem::replace(&mut self.started, true) {
            app_driver.on_start(self);
        }

        // Resume is not guaranteed to be followed by a platform redraw.
        for window in self.windows.values_mut() {
            window.surface_recovery_attempted = false;
            window.sync_geometry();
            window.handle.request_redraw();
        }

        self.handle_signals(event_loop, app_driver);
        while !self.exit {
            let Some((id, action)) = self.suspended_actions.pop_front() else {
                break;
            };
            self.handle_user_event(
                event_loop,
                MasonryUserEvent::AsyncAction(id, action),
                app_driver,
            );
        }
    }

    fn initialize_windows(&mut self, app_driver: &mut dyn AppDriver) {
        while let Some(id) = self.uninitialized_windows.pop_front() {
            if let Some(window) = self.window_mut(id) {
                app_driver.on_window_created(id, window);
            }
        }
    }

    // --- MARK: SUSPENDED
    /// Delegate method for [`ApplicationHandler::suspended()`].
    pub fn handle_suspended(&mut self, _event_loop: &ActiveEventLoop) {
        if self.is_suspended {
            // Short-circuiting since we have already
            // handled the suspended event before this.
            return;
        }

        self.is_suspended = true;

        // All surfaces needs to be cleared when suspended.
        // They will be recreated when resumed.
        self.surfaces.clear();
        #[cfg(target_os = "macos")]
        {
            self.resized_window = None;
        }
        for window in self.windows.values_mut() {
            window.last_anim = None;
            window.event_reducer = WindowEventReducer::default();
            window
                .render_root
                .handle_text_event(&TextEvent::WindowFocusChange(false));
        }
    }

    pub(crate) fn create_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        mut new_window: NewWindow,
    ) -> Result<(), WindowCreationError> {
        if self.window_id_to_handle_id.contains_key(&new_window.id)
            || self
                .new_windows
                .iter()
                .any(|window| window.id == new_window.id)
        {
            return Err(WindowCreationError::DuplicateId(new_window.id));
        }

        if self.is_suspended {
            // Wait until resumed before creating the windows.
            self.new_windows.push(new_window);

            return Ok(());
        }

        // TODO: move this check to modification of base_color once winit exposes window transparency state
        if !new_window.attributes.transparent && new_window.base_color.components[3] < 1. {
            tracing::warn!(
                window_id = ?new_window.id,
                "New window with non-opaque base color doesn't support transparency - \
                you should call `.with_transparent(true)` on the new window's `WindowAttributes`."
            );
        }

        let visible = new_window.attributes.visible;
        // We always create the window as invisible so that we can
        // render the first frame before showing it to avoid flashing.
        let handle = event_loop
            .create_window(std::mem::take(&mut new_window.attributes).with_visible(false))
            .map_err(WindowCreationError::Platform)?;
        if visible {
            // We defer the rendering of the first frame to the handle_signals method because
            // we want to handle any signals caused by the initial layout before we render.
            self.need_first_frame.push(handle.id());
        }

        let adapter =
            Adapter::with_event_loop_proxy(event_loop, &handle, self.event_loop_proxy.clone());
        let handle = Arc::new(handle);

        let handle_id = handle.id();
        let scale_factor = handle.scale_factor();

        let size = window_size(&handle);

        let window = Window::new(
            new_window,
            handle,
            adapter,
            self.signal_sender.clone(),
            RenderRootOptions {
                default_properties: self.default_properties.clone(),
                use_system_fonts: true,
                size_policy: WindowSizePolicy::User,
                size,
                scale_factor,
                test_font: None,
            },
        );

        tracing::debug!(window_id = window.id.trace(), handle=?handle_id, "creating window");
        self.uninitialized_windows.push_back(window.id);
        self.window_id_to_handle_id.insert(window.id, handle_id);
        self.windows.insert(handle_id, window);
        Ok(())
    }

    pub(crate) fn close_window(&mut self, window_id: WindowId) -> bool {
        tracing::debug!(window_id = window_id.trace(), "closing window");
        self.uninitialized_windows.retain(|id| *id != window_id);
        self.suspended_actions.retain(|(id, _)| *id != window_id);
        let Some(handle_id) = self.window_id_to_handle_id.remove(&window_id) else {
            let count = self.new_windows.len();
            self.new_windows.retain(|window| window.id != window_id);
            return self.new_windows.len() != count;
        };
        self.surfaces.remove(&handle_id);
        #[cfg(target_os = "macos")]
        if self.resized_window == Some(handle_id) {
            self.resized_window = None;
        }
        self.need_first_frame.retain(|id| *id != handle_id);
        let Some(window) = self.windows.remove(&handle_id) else {
            return false;
        };
        // On some systems (notably KDE Wayland) the IME state survives window closure.
        window.handle.set_ime_allowed(false);
        true
    }

    // --- MARK: REDRAW
    fn redraw(&mut self, handle_id: HandleId, app_driver: &mut dyn AppDriver) {
        let _span = info_span!("redraw");

        let Some(window) = self.windows.get_mut(&handle_id) else {
            return;
        };
        let size = window.render_root.size();
        if size.width == 0 || size.height == 0 {
            // Surface can't have a dimension of zero, remove the stale surface to save memory.
            self.surfaces.remove(&handle_id);
            return;
        }

        let Some(surface) =
            Self::prepare_surface(&mut self.render_cx, &mut self.surfaces, window, app_driver)
        else {
            return;
        };

        let now = Instant::now();
        // TODO: this calculation uses wall-clock time of the paint call, which
        // potentially has jitter.
        //
        // See https://github.com/linebender/druid/issues/85 for discussion.
        let last = window.last_anim.take();
        let elapsed = last.map(|t| now.duration_since(t)).unwrap_or_default();

        window
            .render_root
            .handle_window_event(WindowEvent::AnimFrame(elapsed));

        // If this animation will continue, store the time.
        // If a new animation starts, then it will have zero reported elapsed time.
        let animation_continues = window.render_root.needs_anim();
        window.last_anim = animation_continues.then_some(now);

        #[cfg(target_os = "macos")]
        if self.resized_window == Some(handle_id) {
            self.render_cx.on_window_resize_state_change(surface, true);
        }

        let (visual_layers, tree_update) = window.render_root.redraw();
        let overlays: Vec<_> = visual_layers
            .overlay_layers()
            .filter_map(|layer| match &layer.kind {
                VisualLayerKind::Scene(scene) => Some(ImagingLayer {
                    scene,
                    transform: layer.transform,
                }),
                VisualLayerKind::External { .. } => None,
            })
            .collect();
        let size = window.render_root.size();
        let empty_scene = masonry_core::imaging::record::Scene::new();
        let root_scene = visual_layers
            .root_layer()
            .and_then(|layer| match &layer.kind {
                VisualLayerKind::Scene(scene) => Some(scene),
                VisualLayerKind::External { .. } => None,
            })
            .unwrap_or(&empty_scene);
        let frame = PreparedFrame::new(
            size.width,
            size.height,
            window.render_root.scale_factor(),
            window.base_color,
            root_scene,
            &overlays,
        );
        let outcome = Self::render(surface, window, frame, &mut self.render_cx);
        match outcome {
            FrameOutcome::Presented => window.surface_recovery_attempted = false,
            FrameOutcome::Skipped => {}
            FrameOutcome::Resize(size) => {
                window.update_size(size);
                window.handle.request_redraw();
            }
            FrameOutcome::SurfaceLost => {
                self.surfaces.remove(&handle_id);
                if !std::mem::replace(&mut window.surface_recovery_attempted, true) {
                    window.handle.request_redraw();
                }
            }
        }
        #[cfg(feature = "tracy")]
        drop(self.frame.take());
        if let Some(tree_update) = tree_update {
            window.accesskit_adapter.update_if_active(|| tree_update);
        }
    }

    fn prepare_surface<'a>(
        render_cx: &mut RenderContext,
        surfaces: &'a mut HashMap<HandleId, RenderSurface<'static>>,
        window: &Window,
        app_driver: &mut dyn AppDriver,
    ) -> Option<&'a mut RenderSurface<'static>> {
        use std::collections::hash_map::Entry;

        let id = window.handle.id();
        let size = window.render_root.size();
        if surfaces
            .get(&id)
            .is_some_and(|surface| render_cx.devices[&surface.dev_id].is_lost())
        {
            surfaces.remove(&id);
        }
        let surface = match surfaces.entry(id) {
            Entry::Occupied(entry) => {
                let surface = entry.into_mut();
                if surface.config.width != size.width || surface.config.height != size.height {
                    render_cx.resize_surface(surface, size.width, size.height);
                }
                surface
            }
            Entry::Vacant(entry) => {
                match create_surface(render_cx, window.handle.clone(), size, window.present_mode) {
                    Ok(surface) => entry.insert(surface),
                    Err(err) => {
                        tracing::error!(%err, "Cannot create render surface");
                        return None;
                    }
                }
            }
        };

        // The device might have been allocated during an earlier failed
        // surface creation. Notify at first successful use, not allocation.
        let device_handle = render_cx.devices.get_mut(&surface.dev_id)?;
        if device_handle.is_lost() {
            return None;
        }
        if !device_handle.ready_notified {
            device_handle.ready_notified = true;
            app_driver.on_wgpu_ready(&WgpuContext {
                id: surface.dev_id,
                instance: &render_cx.instance,
                adapter: &device_handle.adapter,
                device: &device_handle.device,
                queue: &device_handle.queue,
            });
        }
        Some(surface)
    }

    // --- MARK: RENDER
    fn render(
        surface: &mut RenderSurface<'_>,
        window: &Window,
        frame: PreparedFrame<'_>,
        render_cx: &mut RenderContext,
    ) -> FrameOutcome {
        let surface_texture = match Self::acquire_surface_texture(surface, window, render_cx) {
            Ok(texture) => texture,
            Err(outcome) => return outcome,
        };

        let Some(handle) = render_cx.devices.get_mut(&surface.dev_id) else {
            return FrameOutcome::SurfaceLost;
        };
        if handle.is_lost() {
            return FrameOutcome::SurfaceLost;
        }
        let device = &handle.device;
        let queue = &handle.queue;
        let _render_span = tracing::info_span!(
            "Rendering Masonry window",
            backend = ImagingRenderer::BACKEND_NAME
        )
        .entered();
        if let Err(err) = handle.renderer.render_to_texture(
            ImagingRenderTarget {
                adapter: &handle.adapter,
                device,
                queue,
                texture: &surface.target_texture,
                view: &surface.target_view,
            },
            frame,
        ) {
            tracing::error!(
                backend = ImagingRenderer::BACKEND_NAME,
                "Couldn't render Masonry content into target texture: {err}"
            );
            return FrameOutcome::Skipped;
        }
        Self::present_surface(surface, surface_texture, &window.handle, device, queue);
        FrameOutcome::Presented
    }

    fn acquire_surface_texture(
        surface: &mut RenderSurface<'_>,
        window: &Window,
        render_cx: &RenderContext,
    ) -> Result<wgpu::SurfaceTexture, FrameOutcome> {
        use wgpu::CurrentSurfaceTexture;

        if surface.needs_reconfigure {
            render_cx.configure_surface(surface);
        }
        let mut reconfigured = false;
        loop {
            match surface.surface.get_current_texture() {
                CurrentSurfaceTexture::Success(texture) => return Ok(texture),
                CurrentSurfaceTexture::Suboptimal(texture) => {
                    // Reconfigure only once the acquired texture is no longer alive.
                    surface.needs_reconfigure = true;
                    return Ok(texture);
                }
                CurrentSurfaceTexture::Outdated if !reconfigured => {
                    let size = window_size(&window.handle);
                    if size.width == 0 || size.height == 0 || size != window.render_root.size() {
                        return Err(FrameOutcome::Resize(size));
                    }
                    // Same dimensions: reconfigure without allocating an intermediate texture.
                    render_cx.configure_surface(surface);
                    reconfigured = true;
                }
                CurrentSurfaceTexture::Timeout => {
                    window.handle.request_redraw();
                    return Err(FrameOutcome::Skipped);
                }
                CurrentSurfaceTexture::Occluded => return Err(FrameOutcome::Skipped),
                CurrentSurfaceTexture::Lost => return Err(FrameOutcome::SurfaceLost),
                other => {
                    tracing::error!(?other, reconfigured, "Couldn't acquire swap chain texture");
                    return Err(FrameOutcome::Skipped);
                }
            }
        }
    }

    fn present_surface(
        surface: &RenderSurface<'_>,
        surface_texture: wgpu::SurfaceTexture,
        window: &WindowHandle,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        // Copy the new surface content to the surface.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Surface Blit"),
        });
        surface.blitter.copy(
            device,
            &mut encoder,
            &surface.target_view,
            &surface_texture
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default()),
        );
        queue.submit([encoder.finish()]);
        window.pre_present_notify();
        queue.present(surface_texture);
        {
            let _render_poll_span =
                tracing::info_span!("Waiting for GPU to finish rendering").entered();
            if let Err(err) = device.poll(wgpu::PollType::wait_indefinitely()) {
                tracing::error!(?err, "GPU polling failed");
            }
        }
    }

    // --- MARK: WINDOW_EVENT
    /// Delegate method for [`ApplicationHandler::window_event()`].
    pub fn handle_window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        handle_id: HandleId,
        event: WinitWindowEvent,
        app_driver: &mut dyn AppDriver,
    ) {
        if self.is_suspended {
            tracing::warn!(
                ?event,
                "Got window event whilst suspended or before window created"
            );
            return;
        }

        let Some(window) = self.windows.get_mut(&handle_id) else {
            tracing::warn!(
                ?event,
                "Got window event for unknown window {:?}",
                handle_id
            );
            return;
        };

        let _span = info_span!("window_event", window_id = window.id.trace()).entered();
        #[cfg(feature = "tracy")]
        if self.frame.is_none() {
            self.frame = Some(tracing_tracy::client::non_continuous_frame!("Masonry"));
        }
        window
            .accesskit_adapter
            .process_event(&window.handle, &event);

        Self::translate_input(window, &event, self.clipboard_cx.as_mut());

        // On macOS, interactive resizing needs special presentation behavior on Metal to avoid
        // visible jitter.
        #[cfg(target_os = "macos")]
        match event {
            WinitWindowEvent::Resized(_)
            | WinitWindowEvent::RedrawRequested
            | WinitWindowEvent::Moved(_) => {}
            _ => {
                if let Some(surface) = self
                    .resized_window
                    .take()
                    .and_then(|id| self.surfaces.get_mut(&id))
                {
                    self.render_cx.on_window_resize_state_change(surface, false);
                }
            }
        }

        match event {
            WinitWindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                window
                    .render_root
                    .handle_window_event(WindowEvent::Rescale(scale_factor));
                window.update_size(window_size(&window.handle));
            }
            WinitWindowEvent::RedrawRequested => {
                self.redraw(handle_id, app_driver);
            }
            WinitWindowEvent::CloseRequested => {
                app_driver.on_close_requested(window.id, &mut DriverCtx::new(self, event_loop));
            }
            WinitWindowEvent::Resized(size) => {
                #[cfg(target_os = "macos")]
                {
                    if let Some(previous) = self.resized_window.replace(handle_id)
                        && previous != handle_id
                        && let Some(surface) = self.surfaces.get_mut(&previous)
                    {
                        self.render_cx.on_window_resize_state_change(surface, false);
                    }
                    if let Some(surface) = self.surfaces.get_mut(&handle_id) {
                        self.render_cx.on_window_resize_state_change(surface, true);
                    }
                }

                window.update_size(size);
            }
            WinitWindowEvent::Ime(ime) => {
                let ime = winit_ime_to_masonry(ime);
                window.render_root.handle_text_event(&TextEvent::Ime(ime));
            }
            WinitWindowEvent::Focused(new_focus) => {
                window
                    .render_root
                    .handle_text_event(&TextEvent::WindowFocusChange(new_focus));
            }
            WinitWindowEvent::Occluded(false) => {
                window.sync_geometry();
                window.surface_recovery_attempted = false;
                window.handle.request_redraw();
            }
            _ => (),
        }

        self.handle_signals(event_loop, app_driver);
    }

    fn translate_input(
        window: &mut Window,
        event: &WinitWindowEvent,
        clipboard: &mut dyn ClipboardProvider,
    ) {
        if !matches!(
            event,
            WinitWindowEvent::KeyboardInput {
                is_synthetic: true,
                ..
            }
        ) && let Some(wet) = window
            .event_reducer
            .reduce(window.handle.scale_factor(), event)
        {
            match wet {
                WindowEventTranslation::Keyboard(k) => {
                    // TODO - Detect in Masonry code instead
                    let action_mod = if cfg!(target_os = "macos") {
                        k.modifiers.meta()
                    } else {
                        k.modifiers.ctrl()
                    };
                    if let Key::Character(c) = &k.key
                        && c.as_str().eq_ignore_ascii_case("v")
                        && action_mod
                        && k.state == KeyState::Down
                    {
                        match clipboard.get_contents() {
                            Ok(text) => {
                                window
                                    .render_root
                                    .handle_text_event(&TextEvent::ClipboardPaste(text));
                            }
                            Err(error) => {
                                tracing::warn!(?error, "Unable to read clipboard contents");
                            }
                        }
                    } else {
                        window
                            .render_root
                            .handle_text_event(&TextEvent::Keyboard(k));
                    }
                }
                WindowEventTranslation::Pointer(p) => {
                    window.render_root.handle_pointer_event(&p);
                }
            }
        }
    }

    // --- MARK: USER_EVENT
    /// Delegate method for [`ApplicationHandler::user_event()`].
    pub fn handle_user_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        event: MasonryUserEvent,
        app_driver: &mut dyn AppDriver,
    ) {
        if self.exit {
            event_loop.exit();
            return;
        }
        match event {
            MasonryUserEvent::GpuDeviceLost(dev_id) => self.handle_device_lost(dev_id, app_driver),
            MasonryUserEvent::AccessKit(handle_id, event) => {
                let Some(window) = self.windows.get_mut(&handle_id) else {
                    tracing::debug!(handle = ?handle_id, "Dropping accesskit event for closed window");
                    return;
                };
                match event {
                    accesskit_winit::WindowEvent::InitialTreeRequested => {
                        window
                            .render_root
                            .handle_window_event(WindowEvent::EnableAccessTree);
                    }
                    accesskit_winit::WindowEvent::ActionRequested(action_request) => {
                        if !self.is_suspended {
                            window.render_root.handle_access_event(action_request);
                        }
                    }
                    accesskit_winit::WindowEvent::AccessibilityDeactivated => {
                        window
                            .render_root
                            .handle_window_event(WindowEvent::DisableAccessTree);
                    }
                }
            }
            MasonryUserEvent::AsyncAction(id, action) => {
                if self.is_suspended {
                    if self.window_id_to_handle_id.contains_key(&id)
                        || self.new_windows.iter().any(|window| window.id == id)
                    {
                        self.suspended_actions.push_back((id, action));
                    }
                    return;
                }
                if self
                    .handle_id(id)
                    .is_none_or(|handle| !self.windows.contains_key(&handle))
                {
                    tracing::debug!(?id, "Dropping async action for closed window");
                    return;
                }
                app_driver.on_async_action(id, &mut DriverCtx::new(self, event_loop), action);
            }
        }

        self.handle_signals(event_loop, app_driver);
    }

    fn handle_device_lost(&mut self, dev_id: GpuDeviceId, app_driver: &mut dyn AppDriver) {
        if !self
            .render_cx
            .devices
            .get(&dev_id)
            .is_some_and(crate::vello_util::DeviceHandle::is_lost)
        {
            return;
        }
        self.surfaces.retain(|id, surface| {
            if surface.dev_id != dev_id {
                return true;
            }
            if let Some(window) = self.windows.get_mut(id) {
                window.surface_recovery_attempted = false;
                if !self.is_suspended {
                    window.handle.request_redraw();
                }
            }
            false
        });
        if let Some(device) = self.render_cx.devices.remove(&dev_id)
            && device.ready_notified
        {
            app_driver.on_wgpu_lost(dev_id);
        }
    }

    // --- MARK: SIGNALS
    fn handle_signals(&mut self, event_loop: &ActiveEventLoop, app_driver: &mut dyn AppDriver) {
        if self.exit {
            event_loop.exit();
            return;
        }
        if self.is_suspended {
            return;
        }

        let mut need_redraw = HashSet::<HandleId>::new();

        while !self.exit {
            self.initialize_windows(app_driver);
            let Some((window_id, signal)) = self.signal_receiver.try_iter().next() else {
                break;
            };

            self.handle_signal(window_id, signal, event_loop, app_driver, &mut need_redraw);
        }

        if self.exit {
            event_loop.exit();
        } else {
            self.schedule_redraws(need_redraw, app_driver);
        }
    }

    fn handle_signal(
        &mut self,
        window_id: WindowId,
        signal: RenderRootSignal,
        event_loop: &ActiveEventLoop,
        app_driver: &mut dyn AppDriver,
        need_redraw: &mut HashSet<HandleId>,
    ) {
        let Some(handle_id) = self.window_id_to_handle_id.get(&window_id) else {
            tracing::warn!(id = ?window_id, signal = ?signal, "Got a signal for an unknown window");
            return;
        };

        let Some(window) = self.windows.get_mut(handle_id) else {
            return;
        };
        let handle = &window.handle;

        match signal {
            RenderRootSignal::Action(action, widget_id) => {
                trace!(
                    "Action {:?} on widget {:?}",
                    (*action).type_name(),
                    widget_id
                );
                app_driver.on_action(
                    window_id,
                    &mut DriverCtx::new(self, event_loop),
                    widget_id,
                    action,
                );
            }
            RenderRootSignal::StartIme => {
                handle.set_ime_allowed(true);
            }
            RenderRootSignal::EndIme => {
                handle.set_ime_allowed(false);
            }
            RenderRootSignal::ImeMoved(position, size) => {
                handle.set_ime_cursor_area(position, size);
            }
            RenderRootSignal::ClipboardStore(text) => {
                if let Err(err) = self.clipboard_cx.set_contents(text) {
                    tracing::warn!(?err, "Unable to write clipboard contents");
                }
            }
            RenderRootSignal::RequestRedraw | RenderRootSignal::RequestAnimFrame => {
                need_redraw.insert(*handle_id);
            }
            RenderRootSignal::TakeFocus => {
                handle.focus_window();
            }
            RenderRootSignal::SetCursor(cursor) => {
                handle.set_cursor(cursor);
            }
            RenderRootSignal::SetSize(size) => {
                if let Some(applied) = handle.request_inner_size(size) {
                    window.update_size(applied);
                }
            }
            RenderRootSignal::SetTitle(title) => {
                handle.set_title(&title);
            }
            RenderRootSignal::DragWindow => {
                // TODO - Handle return value?
                let _ = handle.drag_window();
            }
            RenderRootSignal::DragResizeWindow(direction) => {
                // TODO - Handle return value?
                let direction = masonry_resize_direction_to_winit(direction);
                let _ = handle.drag_resize_window(direction);
            }
            RenderRootSignal::ToggleMaximized => {
                handle.set_maximized(!handle.is_maximized());
            }
            RenderRootSignal::Minimize => {
                handle.set_minimized(true);
            }
            RenderRootSignal::Exit => {
                self.exit = true;
            }
            RenderRootSignal::ShowWindowMenu(position) => {
                handle.show_window_menu(position);
            }
            RenderRootSignal::WidgetSelectedInInspector(widget_id) => {
                Self::report_inspector_selection(window, widget_id);
            }
            RenderRootSignal::NewLayer(_type, root, pos) => {
                window.render_root.add_layer(root, pos);
            }
            RenderRootSignal::RemoveLayer(root_id) => window.render_root.remove_layer(root_id),
            RenderRootSignal::RepositionLayer(root_id, new_pos) => {
                window.render_root.reposition_layer(root_id, new_pos);
            }
        }
    }

    fn report_inspector_selection(window: &Window, widget_id: masonry_core::core::WidgetId) {
        let Some(widget) = window.render_root.get_widget(widget_id) else {
            return;
        };
        let widget_name = widget.short_type_name();
        let display_name = widget.get_debug_text().map_or_else(
            || widget_name.into(),
            |debug_text| format!("{widget_name}<{debug_text}>"),
        );
        info!("Widget selected in inspector: {widget_id} - {display_name}");
    }

    fn schedule_redraws(&mut self, need_redraw: HashSet<HandleId>, app_driver: &mut dyn AppDriver) {
        // If an app creates a visible window, we firstly create it as invisible
        // and then render the first frame before making it visible to avoid flashing.
        for handle_id in std::mem::take(&mut self.need_first_frame) {
            self.redraw(handle_id, app_driver);
            if let Some(window) = self.windows.get_mut(&handle_id) {
                window.handle.set_visible(true);
            }
        }

        // If we're processing a lot of actions, we may have a lot of pending redraws.
        // We batch them up to avoid redundant requests.
        for handle_id in need_redraw {
            if let Some(window) = self.windows.get(&handle_id) {
                window.handle.request_redraw();
            }
        }
    }

    fn handle_id(&self, window_id: WindowId) -> Option<HandleId> {
        self.window_id_to_handle_id.get(&window_id).copied()
    }

    pub(crate) fn window_mut(&mut self, window_id: WindowId) -> Option<&mut Window> {
        self.windows.get_mut(&self.handle_id(window_id)?)
    }

    /// Returns true if app is currently suspended.
    ///
    /// See [`ApplicationHandler::suspended()`] for details.
    ///
    /// Short version: "suspended" is a notion that mostly applies to web and mobile apps,
    /// usually when the app is about to be put in cache.
    /// Suspended apps have no surfaces and receive no events.
    #[must_use = "Check whether the app is suspended."]
    pub const fn is_suspended(&self) -> bool {
        self.is_suspended
    }

    // TODO: Remove this method.
    // It's currently used to call register_fonts and set_focus_fallback.
    #[doc(hidden)]
    pub fn roots(&mut self) -> impl Iterator<Item = &mut RenderRoot> {
        self.windows
            .values_mut()
            .map(|window| &mut window.render_root)
    }

    /// Returns the device identity of a live window surface, if it has one.
    #[must_use = "Identify the GPU owning the window surface."]
    pub fn gpu_device_id(&self, window_id: WindowId) -> Option<GpuDeviceId> {
        let surface = self.surfaces.get(&self.handle_id(window_id)?)?;
        (!self.render_cx.devices.get(&surface.dev_id)?.is_lost()).then_some(surface.dev_id)
    }

    /// Returns the configured presentation mode of a live surface.
    ///
    /// Automatic modes remain automatic; WGPU chooses the concrete mode.
    #[must_use = "Inspect the configured presentation mode."]
    pub fn present_mode(&self, window_id: WindowId) -> Option<wgpu::PresentMode> {
        let surface = self.surfaces.get(&self.handle_id(window_id)?)?;
        self.gpu_device_id(window_id)?;
        Some(surface.config.present_mode)
    }

    /// Sets how frames are presented to the user.
    ///
    /// This affects what users commonly know as "vertical sync".
    /// Returns false if the window is unknown or its live surface does not support the mode.
    /// A mode selected before surface creation is retained as a preference; if unavailable
    /// on a new or recovered surface, a supported fallback is used.
    pub fn set_present_mode(
        &mut self,
        window_id: WindowId,
        present_mode: wgpu::PresentMode,
    ) -> bool {
        let Some(handle_id) = self.handle_id(window_id) else {
            return false;
        };
        let Some(window) = self.windows.get_mut(&handle_id) else {
            return false;
        };
        if let Some(surface) = self.surfaces.get_mut(&handle_id)
            && !self.render_cx.devices[&surface.dev_id].is_lost()
            && !self.render_cx.set_present_mode(surface, present_mode)
        {
            return false;
        }
        if window.present_mode != present_mode {
            window.present_mode = present_mode;
            window.handle.request_redraw();
        }
        true
    }
}

#[derive(Debug, PartialEq, Eq)]
enum FrameOutcome {
    Presented,
    Skipped,
    Resize(PhysicalSize<u32>),
    SurfaceLost,
}

fn window_size(handle: &WindowHandle) -> PhysicalSize<u32> {
    // https://github.com/rust-windowing/winit/issues/2308
    #[cfg(target_os = "ios")]
    {
        handle.outer_size()
    }
    #[cfg(not(target_os = "ios"))]
    {
        handle.inner_size()
    }
}

fn create_surface(
    render_cx: &mut RenderContext,
    handle: Arc<WindowHandle>,
    size: PhysicalSize<u32>,
    present_mode: wgpu::PresentMode,
) -> Result<RenderSurface<'static>, RenderSurfaceError> {
    render_cx.create_surface(handle, size.width, size.height, present_mode)
}

#[cfg(all(test, target_os = "linux"))]
mod lifecycle_tests;
