// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use masonry_core::util::IdAllocator;
use std::fmt::Debug;
use std::hash::Hash;
use std::num::NonZeroU64;

use masonry_core::app::RenderRoot;
use masonry_core::core::{ErasedAction, WidgetId};
use tracing::field::DisplayValue;
use winit::event_loop::ActiveEventLoop;

use crate::app::MasonryState;
use crate::event_loop_runner::{NewWindow, Window, WindowCreationError};

/// A unique and persistent identifier for a window.
///
/// [`MasonryState`] internally maps these to winit window ids ([`winit::window::WindowId`]).
/// Applications should only use this struct and not be concerned with the winit window ids.
/// When the application is suspended and resumed this id will stay the same, while the
/// winit window id will change.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct WindowId(pub(crate) NonZeroU64);

impl WindowId {
    /// Allocate a new, unique `WindowId`.
    pub fn next() -> Self {
        static WINDOW_ID_COUNTER: IdAllocator = IdAllocator::new();
        Self(WINDOW_ID_COUNTER.allocate())
    }

    /// A serialized representation of the `WindowId` for debugging purposes.
    #[must_use = "Use the identifier in a trace."]
    pub fn trace(self) -> DisplayValue<NonZeroU64> {
        tracing::field::display(self.0)
    }
}

/// Context for the [`AppDriver`] trait.
#[derive(Debug)]
pub struct DriverCtx<'a> {
    state: &'a mut MasonryState,
    event_loop: &'a ActiveEventLoop,
}

impl<'a> DriverCtx<'a> {
    pub(crate) const fn new(state: &'a mut MasonryState, event_loop: &'a ActiveEventLoop) -> Self {
        Self { state, event_loop }
    }
}

/// Access to Masonry's WGPU device state.
///
/// This is provided via [`AppDriver::on_wgpu_ready`] so applications can create GPU resources
/// (textures, pipelines, etc.) using the same `Device`/`Queue` as Masonry.
#[derive(Debug)]
pub struct WgpuContext<'a> {
    /// The WGPU instance used by Masonry.
    pub instance: &'a wgpu::Instance,
    /// The WGPU adapter used to create the device.
    pub adapter: &'a wgpu::Adapter,
    /// The shared WGPU device.
    pub device: &'a wgpu::Device,
    /// The shared WGPU queue.
    pub queue: &'a wgpu::Queue,
}

/// Strategy for selecting `wgpu::Limits` when requesting the WGPU device.
#[derive(Clone, Debug, Default)]
pub enum WgpuLimits {
    /// Use `wgpu::Limits::default()`.
    #[default]
    Default,
    /// Use `adapter.limits()` (maximum supported by the selected adapter).
    Adapter,
    /// Use the provided limits.
    Custom(Box<wgpu::Limits>),
}

/// A trait for defining how your app interacts with the Masonry widget tree.
///
/// When launching your app with [`crate::app::run`], you need to provide
/// a type that implements this trait.
#[expect(unused_variables, reason = "Default impls doesn't use arguments")]
pub trait AppDriver {
    /// A hook which will be executed when a widget emits an `action`.
    ///
    /// This action is type-erased, and the type of action emitted will depend on.
    /// Each widget should document which types of action it might emit.
    fn on_action(
        &mut self,
        window_id: WindowId,
        ctx: &mut DriverCtx<'_>,
        widget_id: WidgetId,
        action: ErasedAction,
    );

    /// A hook which will be executed for async actions sent outside the widget tree.
    ///
    /// This is called when the winit event loop gets a [`MasonryUserEvent::AsyncAction`] event.
    ///
    /// [`MasonryUserEvent::AsyncAction`]: crate::app::MasonryUserEvent::AsyncAction
    fn on_async_action(
        &mut self,
        window_id: WindowId,
        ctx: &mut DriverCtx<'_>,
        action: ErasedAction,
    ) {
    }

    /// A hook which will be executed when the application starts, to allow initial configuration of the `MasonryState`.
    ///
    /// Use cases include loading fonts.
    ///
    /// There are circumstances under which this will be called multiple times during the lifecycle of your app.
    /// This is not intended to be the behaviour of Masonry Winit long-term, but this method should currently
    /// not assume it will only be called once (but should feel free to waste work if it is called multiple times,
    /// for example, as the mentioned circumstances are very rare).
    // TODO: Turn into something like on window created, or split into two.
    fn on_start(&mut self, state: &mut MasonryState) {}

    /// A hook called when a user has requested to close a window.
    fn on_close_requested(&mut self, window_id: WindowId, ctx: &mut DriverCtx<'_>) {
        ctx.exit();
    }

    /// Called when Masonry has created its WGPU device.
    fn on_wgpu_ready(&mut self, _wgpu: &WgpuContext<'_>) {}
}

impl DriverCtx<'_> {
    // TODO - Add method to create timer

    /// Access the render root of a live window, if present.
    pub fn render_root(&mut self, window_id: WindowId) -> Option<&mut RenderRoot> {
        Some(&mut self.window(window_id)?.render_root)
    }

    /// Access the state of a live window, if present.
    pub fn window(&mut self, window_id: WindowId) -> Option<&mut Window> {
        self.state.window_mut(window_id)
    }

    /// Creates a new window, or queues it while suspended.
    ///
    /// # Errors
    /// Returns an error for a duplicate identifier or platform window creation failure.
    pub fn create_window(&mut self, new_window: NewWindow) -> Result<(), WindowCreationError> {
        self.state.create_window(self.event_loop, new_window)
    }

    /// Closes a live or queued window. Returns whether the window existed.
    pub fn close_window(&mut self, window_id: WindowId) -> bool {
        self.state.close_window(window_id)
    }

    /// Exits the application (stops the event loop).
    pub const fn exit(&mut self) {
        self.state.exit = true;
    }
}
