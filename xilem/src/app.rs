// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::iter::Once;
use std::sync::Arc;

use masonry::core::DefaultProperties;
use masonry::peniko::{Blob, Color};
use masonry::theme::{BACKGROUND_COLOR, default_property_set};
use masonry_winit::app::{EventLoopBuilder, MasonryState, MasonryUserEvent, NewWindow, WindowId};
use tokio::runtime::Runtime as TokioRuntime;
use winit::error::EventLoopError;

use crate::core::map_state;
use crate::window_options::WindowCallbacks;
use crate::{MasonryDriver, WidgetView, WindowOptions, WindowView};

/// Error during application startup or execution.
#[derive(Debug)]
pub enum RunError {
    /// Failed to create the async runtime.
    Runtime(std::io::Error),
    /// The window event loop could not be started or run.
    EventLoop(EventLoopError),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime(err) => write!(f, "Could not start async runtime: {err}"),
            Self::EventLoop(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Runtime(err) => Some(err),
            Self::EventLoop(err) => Some(err),
        }
    }
}

impl From<std::io::Error> for RunError {
    fn from(err: std::io::Error) -> Self {
        Self::Runtime(err)
    }
}

impl From<EventLoopError> for RunError {
    fn from(err: EventLoopError) -> Self {
        Self::EventLoop(err)
    }
}

pub(crate) type StartCallback = Box<dyn FnOnce(&mut MasonryState)>;

// TODO - Rename to `AppLauncher` or something.

/// Runtime builder.
#[must_use = "A Xilem app does nothing unless ran."]
pub struct Xilem<State, Logic> {
    state: State,
    logic: Logic,
    runtime: Option<Arc<TokioRuntime>>,
    default_properties: Option<DefaultProperties>,
    default_base_color: Color,
    // Font data to include in loading.
    fonts: Vec<Blob<u8>>,
    // Callback invoked once on startup, after windows creation.
    on_start: Option<StartCallback>,
}

/// State type used by [`Xilem::new_simple`].
pub struct ExitOnClose<S> {
    state: S,
    running: bool,
}

impl<S> AppState for ExitOnClose<S> {
    fn keep_running(&self) -> bool {
        self.running
    }
}

impl<State>
    Xilem<
        ExitOnClose<State>,
        Box<dyn FnMut(&mut ExitOnClose<State>) -> Once<WindowView<ExitOnClose<State>>>>,
    >
{
    /// Create an app builder for a single window app with fixed window attributes
    /// that exits once the window is closed.
    ///
    /// If you want to have multiple windows or change e.g. the window title depending
    /// on the state you should instead use [`Xilem::new`] (which this function wraps).
    pub fn new_simple<View>(
        state: State,
        logic: impl FnMut(&mut State) -> View + 'static,
        window_options: WindowOptions<State>,
    ) -> Self
    where
        View: WidgetView<State>,
    {
        Self::new_simple_inner(state, logic, window_options, None)
    }

    /// Same as [`Self::new_simple`] but allows passing an existing tokio runtime.
    pub fn new_simple_with_tokio<View>(
        state: State,
        logic: impl FnMut(&mut State) -> View + 'static,
        window_options: WindowOptions<State>,
        tokio_rt: Arc<TokioRuntime>,
    ) -> Self
    where
        View: WidgetView<State>,
    {
        Self::new_simple_inner(state, logic, window_options, Some(tokio_rt))
    }

    fn new_simple_inner<View>(
        state: State,
        mut logic: impl FnMut(&mut State) -> View + 'static,
        window_options: WindowOptions<State>,
        tokio_rt: Option<Arc<TokioRuntime>>,
    ) -> Self
    where
        View: WidgetView<State>,
    {
        let window_id = WindowId::next();
        let callbacks = Arc::new(window_options.callbacks);
        Xilem::new_inner(
            ExitOnClose {
                state,
                running: true,
            },
            Box::new(move |ExitOnClose { state, .. }| {
                let callbacks = callbacks.clone();
                let on_close = move |wrapper: &mut ExitOnClose<_>| {
                    wrapper.running = false;
                    if let Some(on_close) = &callbacks.on_close {
                        on_close(&mut wrapper.state);
                    }
                };
                std::iter::once(
                    crate::window(
                        window_id,
                        String::new(),
                        map_state(logic(state), |wrapper: &mut ExitOnClose<_>| {
                            &mut wrapper.state
                        }),
                    )
                    .with_options(|_| WindowOptions {
                        reactive: window_options.reactive.clone(),
                        initial: window_options.initial.clone(),
                        callbacks: WindowCallbacks {
                            on_close: Some(Box::new(on_close)),
                        },
                    }),
                )
            }),
            tokio_rt,
        )
    }
}

/// The trait [`Xilem::new`] expects to be implemented for the state.
///
/// [`Xilem::new_simple`] does not use this trait implementation.
pub trait AppState {
    /// Returns whether the application should keep running or exit.
    ///
    /// Checked after every state mutation (actions, close requests, etc.).
    fn keep_running(&self) -> bool;
}

impl<State, Logic, WindowIter> Xilem<State, Logic>
where
    State: AppState + 'static,
    Logic: FnMut(&mut State) -> WindowIter + 'static,
    WindowIter: Iterator<Item = WindowView<State>>,
{
    /// Initialize the builder state for your app with an app logic function that returns a window iterator.
    pub fn new(state: State, logic: Logic) -> Self
    where
        State: AppState,
    {
        Self::new_inner(state, logic, None)
    }

    fn new_inner(state: State, logic: Logic, runtime: Option<Arc<TokioRuntime>>) -> Self {
        Self {
            state,
            logic,
            runtime,
            default_properties: None,
            default_base_color: BACKGROUND_COLOR,
            fonts: Vec::new(),
            on_start: None,
        }
    }

    /// Load a font when this `Xilem` is run.
    ///
    /// This is an interim API whilst font lifecycles are determined.
    pub fn with_font(mut self, data: impl Into<Blob<u8>>) -> Self {
        self.fonts.push(data.into());
        self
    }

    // TODO: Find better ways to customize default property set.
    /// Sets default properties of widget tree.
    pub fn with_default_properties(mut self, default_properties: DefaultProperties) -> Self {
        self.default_properties = Some(default_properties);
        self
    }

    // TODO: Find better ways to customize default base color.
    /// Sets default base color of windows.
    pub const fn with_default_base_color(mut self, default_base_color: Color) -> Self {
        self.default_base_color = default_base_color;
        self
    }

    /// Registers a callback to be called once the application has started
    pub fn with_on_start(mut self, callback: impl FnOnce(&mut MasonryState) + 'static) -> Self {
        self.on_start = Some(Box::new(callback));
        self
    }

    /// Run app with custom window attributes.
    ///
    /// # Errors
    /// Returns an error if the async runtime or event loop cannot be created or run.
    pub fn run_in(mut self, mut event_loop: EventLoopBuilder) -> Result<(), RunError> {
        let event_loop = event_loop.build()?;
        let proxy = event_loop.create_proxy();
        let default_properties = self
            .default_properties
            .take()
            .unwrap_or_else(default_property_set);
        let (driver, windows) = self
            .into_driver_and_windows(move |event| proxy.send_event(event).map_err(|err| err.0))?;
        Ok(masonry_winit::app::run_with(
            event_loop,
            windows,
            driver,
            default_properties,
        )?)
    }

    /// Builds the [`MasonryDriver`] and the initial windows.
    ///
    /// The given event sink function sends the given event to the event loop
    /// and returns the given event as an error in case the event loop is stopped.
    ///
    /// # Errors
    /// Returns an I/O error if creating the Tokio runtime fails.
    pub fn into_driver_and_windows(
        self,
        proxy: impl Fn(MasonryUserEvent) -> Result<(), MasonryUserEvent> + Send + Sync + 'static,
    ) -> std::io::Result<(MasonryDriver<State, Logic>, Vec<NewWindow>)> {
        let runtime = match self.runtime {
            Some(runtime) => runtime,
            None => Arc::new(TokioRuntime::new()?),
        };
        Ok(MasonryDriver::new(
            self.state,
            self.logic,
            proxy,
            runtime,
            self.default_base_color,
            self.fonts,
            self.on_start,
        ))
    }
}
