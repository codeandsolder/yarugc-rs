// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::sync::Arc;

use masonry::core::{ErasedAction, WidgetId};
use masonry::peniko::Blob;
use masonry_winit::app::{
    AppDriver, DriverCtx, MasonryState, MasonryUserEvent, NewWindow, WindowId,
};

use crate::app::StartCallback;
use crate::core::{
    DynMessage, MessageCtx, MessageResult, ProxyError, RawProxy, SendMessage, View, ViewId,
    ViewPathTracker,
};
use crate::window_view::{WindowView, WindowViewState};
use crate::{AppState, Color, ViewCtx};

/// The composition root of Xilem's Masonry backend.
///
/// Owns the root state, the root view (kind of), the window state, the event loop proxy,
/// the tokio runtime, etc.
pub struct MasonryDriver<State: 'static, Logic> {
    state: State,
    logic: Logic,
    windows: HashMap<WindowId, Window<State>>,
    proxy: Arc<MasonryProxy>,
    runtime: Arc<tokio::runtime::Runtime>,
    default_base_color: Color,
    // Fonts which will be registered on startup.
    fonts: Vec<Blob<u8>>,
    // Optional callback invoked once on startup, after windows creation.
    start_callback: Option<StartCallback>,
}

struct Window<State: 'static> {
    view: WindowView<State>,
    view_ctx: ViewCtx,
    view_state: WindowViewState,
}

impl<State, Logic, WindowIter> MasonryDriver<State, Logic>
where
    State: 'static,
    Logic: FnMut(&mut State) -> WindowIter,
    WindowIter: Iterator<Item = WindowView<State>>,
{
    pub(crate) fn new(
        state: State,
        logic: Logic,
        // TODO: narrow down MasonryUserEvent in event_sink once masonry_winit supports custom event types
        // (we only ever use it to send MasonryUserEvent::Action with ASYNC_MARKER_WIDGET)
        event_sink: impl Fn(MasonryUserEvent) -> Result<(), MasonryUserEvent> + Send + Sync + 'static,
        runtime: Arc<tokio::runtime::Runtime>,
        default_base_color: Color,
        fonts: Vec<Blob<u8>>,
        start_callback: Option<StartCallback>,
    ) -> (Self, Vec<NewWindow>) {
        let mut driver = Self {
            state,
            logic,
            windows: HashMap::new(),
            proxy: Arc::new(MasonryProxy(Box::new(event_sink))),
            runtime,
            default_base_color,
            fonts,
            start_callback,
        };
        let windows: Vec<_> = (driver.logic)(&mut driver.state)
            .filter_map(|view| {
                if driver.windows.contains_key(&view.id) {
                    tracing::error!(
                        window_id = view.id.trace(),
                        "Duplicate initial window, ignoring it"
                    );
                    None
                } else {
                    Some(driver.build_window(view))
                }
            })
            .collect();
        (driver, windows)
    }
}

/// The action which should be used for async events.
#[must_use = "The action must be dispatched to the event loop."]
pub fn async_action(path: Arc<[ViewId]>, message: SendMessage) -> ErasedAction {
    Box::<MessagePackage>::new((path, message))
}

/// The type used to send a message for async events.
type MessagePackage = (Arc<[ViewId]>, SendMessage);

impl MasonryProxy {
    fn send_message(
        &self,
        window_id: WindowId,
        path: Arc<[ViewId]>,
        message: SendMessage,
    ) -> Result<(), ProxyError> {
        let user_event = MasonryUserEvent::AsyncAction(window_id, async_action(path, message));
        match (self.0)(user_event) {
            Ok(()) => Ok(()),
            Err(err) => {
                if let MasonryUserEvent::AsyncAction(_, res) = err
                    && let Ok(package) = res.downcast::<MessagePackage>()
                {
                    Err(ProxyError::DriverFinished(package.1))
                } else {
                    Err(ProxyError::Other(Box::new(std::io::Error::other(
                        "Event sink returned an unrelated event",
                    ))))
                }
            }
        }
    }
}

struct MasonryProxy(
    pub(crate) Box<dyn Fn(MasonryUserEvent) -> Result<(), MasonryUserEvent> + Send + Sync>,
);

impl Debug for MasonryProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("MasonryProxy").finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct WindowProxy(WindowId, Arc<MasonryProxy>);

impl RawProxy for WindowProxy {
    fn send_message(&self, path: Arc<[ViewId]>, message: SendMessage) -> Result<(), ProxyError> {
        self.1.send_message(self.0, path, message)
    }

    fn dyn_debug(&self) -> &dyn Debug {
        self
    }
}

impl<State, Logic, WindowIter> MasonryDriver<State, Logic>
where
    State: 'static,
    Logic: FnMut(&mut State) -> WindowIter,
    WindowIter: Iterator<Item = WindowView<State>>,
{
    fn build_window(&mut self, mut window_view: WindowView<State>) -> NewWindow {
        window_view
            .base_color
            .get_or_insert(self.default_base_color);

        let mut view_ctx = ViewCtx::new(
            Arc::new(WindowProxy(window_view.id, self.proxy.clone())),
            self.runtime.clone(),
        );
        let (new_window, view_state) = window_view.build(&mut view_ctx, &mut self.state);
        self.windows.insert(
            window_view.id,
            Window {
                view: window_view,
                view_ctx,
                view_state,
            },
        );
        new_window.0
    }

    pub(crate) fn create_window(
        &mut self,
        driver_ctx: &mut DriverCtx<'_>,
        view: WindowView<State>,
    ) {
        let id = view.id;
        if let Err(err) = driver_ctx.create_window(self.build_window(view)) {
            tracing::error!(%err, "Could not create Xilem window");
            self.windows.remove(&id);
        }
    }

    fn close_window(&mut self, window_id: WindowId, ctx: &mut DriverCtx<'_>) {
        if let Some(mut window) = self.windows.remove(&window_id)
            && let Some(element) = ctx.window(window_id)
        {
            window
                .view
                .teardown(&mut window.view_state, &mut window.view_ctx, element);
        }
        ctx.close_window(window_id);
    }

    fn run_logic(&mut self, driver_ctx: &mut DriverCtx<'_>) {
        let mut returned_ids = HashSet::new();
        for mut next_view in (self.logic)(&mut self.state) {
            next_view.base_color.get_or_insert(self.default_base_color);
            if !returned_ids.insert(next_view.id) {
                tracing::error!(
                    window_id = next_view.id.trace(),
                    "logic function returned two windows with the same id, ignoring the duplicate"
                );
                continue;
            }

            match self.windows.get_mut(&next_view.id) {
                Some(Window {
                    view,
                    view_ctx,
                    view_state,
                }) => {
                    let Some(element) = driver_ctx.window(next_view.id) else {
                        continue;
                    };
                    next_view.rebuild(view, view_state, view_ctx, element, &mut self.state);
                    *view = next_view;
                }
                None => self.create_window(driver_ctx, next_view),
            }
        }

        let to_be_closed: Vec<_> = self
            .windows
            .keys()
            .copied()
            .filter(|id| !returned_ids.contains(id))
            .collect();
        for window_id in to_be_closed {
            self.close_window(window_id, driver_ctx);
        }
    }
}

impl<State, Logic, WindowIter> MasonryDriver<State, Logic>
where
    State: AppState + 'static,
    Logic: FnMut(&mut State) -> WindowIter,
    WindowIter: Iterator<Item = WindowView<State>>,
{
    fn dispatch_message(
        &mut self,
        window_id: WindowId,
        masonry_ctx: &mut DriverCtx<'_>,
        id_path: Vec<ViewId>,
        message: DynMessage,
    ) -> MessageResult<()> {
        let Some(window) = self.windows.get_mut(&window_id) else {
            return MessageResult::Stale;
        };

        let Some(element) = masonry_ctx.window(window_id) else {
            return MessageResult::Stale;
        };
        let mut message_context = MessageCtx::new(
            std::mem::take(window.view_ctx.environment()),
            id_path,
            message,
        );
        let res = window.view.message(
            &mut window.view_state,
            &mut message_context,
            element,
            &mut self.state,
        );
        // TODO: Handle `_message` somehow?
        let (env, _id_path, _message) = message_context.finish();
        *window.view_ctx.environment() = env;

        res
    }

    fn handle_message_result(
        &mut self,
        window_id: WindowId,
        masonry_ctx: &mut DriverCtx<'_>,
        message_result: &MessageResult<()>,
    ) {
        let Some(window) = self.windows.get_mut(&window_id) else {
            return;
        };

        match message_result {
            // The semantics here haven't exactly been worked out.
            // This version of the implementation is based on the assumptions that:
            // 1) `MessageResult::Action` means that the app's state has changed (and so the logic needs to be reran)
            // 2) `MessageResult::RequestRebuild` requires that the app state is *not* rebuilt; this allows
            //     avoiding infinite loops.
            MessageResult::Action(()) => {
                self.run_logic(masonry_ctx);
                // Check if the app wants to exit after this state mutation.
                if !self.state.keep_running() {
                    masonry_ctx.exit();
                }
            }
            MessageResult::RequestRebuild => {
                let Some(root) = masonry_ctx.render_root(window_id) else {
                    return;
                };
                window.view.masonry_root.rebuild(
                    &window.view.masonry_root,
                    &mut window.view_state,
                    &mut window.view_ctx,
                    root,
                    &mut self.state,
                );
            }
            MessageResult::Nop => {}
            MessageResult::Stale => {
                tracing::info!("Discarding message");
            }
        }
    }
}

impl<State, Logic, WindowIter> AppDriver for MasonryDriver<State, Logic>
where
    State: AppState + 'static,
    Logic: FnMut(&mut State) -> WindowIter,
    WindowIter: Iterator<Item = WindowView<State>>,
{
    fn on_action(
        &mut self,
        window_id: WindowId,
        masonry_ctx: &mut DriverCtx<'_>,
        widget_id: WidgetId,
        action: ErasedAction,
    ) {
        let Some(window) = self.windows.get_mut(&window_id) else {
            tracing::warn!(
                window_id = window_id.trace(),
                "called on_action for unknown window"
            );
            return;
        };

        let Some(widget_path) = window.view_ctx.get_id_path(widget_id) else {
            tracing::error!(
                "Got action {action:?} for unknown widget. Did you forget to use `with_action_widget`?"
            );
            return;
        };

        let id_path = widget_path.clone();
        let message = DynMessage(action);

        let message_result = self.dispatch_message(window_id, masonry_ctx, id_path, message);
        self.handle_message_result(window_id, masonry_ctx, &message_result);
    }

    fn on_async_action(
        &mut self,
        window_id: WindowId,
        masonry_ctx: &mut DriverCtx<'_>,
        action: ErasedAction,
    ) {
        if self.windows.get_mut(&window_id).is_none() {
            tracing::warn!(
                window_id = window_id.trace(),
                "called on_async_action for unknown window"
            );
            return;
        }

        // Dispatch this message using the path it contains.
        let Ok(package) = action.downcast::<MessagePackage>() else {
            tracing::warn!("Discarding malformed async action");
            return;
        };
        let (id_path, message) = *package;
        let id_path = Vec::from(&*id_path);
        let message = message.into();

        let message_result = self.dispatch_message(window_id, masonry_ctx, id_path, message);
        self.handle_message_result(window_id, masonry_ctx, &message_result);
    }

    fn on_window_created(&mut self, _id: WindowId, window: &mut masonry_winit::app::Window) {
        let root = window.render_root();
        if let Some(root_widget) = root
            .get_content_root()
            .and_then(|root| root.downcast::<masonry::widgets::Passthrough>())
        {
            let fallback = root_widget.inner().inner_id();
            root.set_focus_fallback(Some(fallback));
        }

        // Each RenderRoot owns its own font collection, including windows
        // created after the initial application startup.
        for font in &self.fonts {
            drop(root.register_fonts(font.clone()));
        }
    }

    fn on_window_creation_failed(
        &mut self,
        id: WindowId,
        _error: &masonry_winit::app::WindowCreationError,
    ) {
        self.windows.remove(&id);
    }

    fn on_start(&mut self, state: &mut MasonryState) {
        if let Some(cb) = self.start_callback.take() {
            cb(state);
        }
    }

    fn on_close_requested(&mut self, window_id: WindowId, ctx: &mut DriverCtx<'_>) {
        let Some(window) = self.windows.get(&window_id) else {
            return;
        };
        window.view.on_close(&mut self.state);
        self.run_logic(ctx);

        if !self.state.keep_running() {
            // TODO: we should probably call teardown for all windows before exiting => introduce AppDriver::on_exit
            ctx.exit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_event_sink_returns_original_message() {
        let proxy = MasonryProxy(Box::new(Err));
        let result = proxy.send_message(WindowId::next(), Arc::from([]), SendMessage::new(42_u32));
        let Err(ProxyError::DriverFinished(message)) = result else {
            panic!("expected the original undelivered message");
        };
        assert_eq!(*message.downcast::<u32>().unwrap(), 42);
    }

    #[test]
    fn duplicate_initial_windows_are_not_built_twice() {
        let id = WindowId::next();
        let runtime = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap(),
        );
        let (driver, windows) = MasonryDriver::new(
            (),
            move |_: &mut ()| {
                [
                    crate::window(id, "First", crate::view::label("first")),
                    crate::window(id, "Second", crate::view::label("second")),
                ]
                .into_iter()
            },
            |_| Ok(()),
            runtime,
            Color::BLACK,
            Vec::new(),
            None,
        );
        assert_eq!(windows.len(), 1);
        assert_eq!(driver.windows.len(), 1);
        assert_eq!(windows[0].attributes.title, "First");
    }
}
