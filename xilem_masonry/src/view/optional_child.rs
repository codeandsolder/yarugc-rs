// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use masonry::core::{FromDynWidget, NewWidget, Widget, WidgetMut};
use masonry::widgets;

use crate::core::{MessageCtx, MessageResult, ViewId, ViewPathTracker};
use crate::{ViewCtx, WidgetView};

/// State for a dynamically present child, including its incarnation in the routing path.
#[doc(hidden)]
#[expect(
    unnameable_types,
    reason = "Associated view state is an implementation detail."
)]
pub struct OptionalChildState<T> {
    state: Option<T>,
    generation: u64,
}

pub(super) trait OptionalChildParent: Widget + FromDynWidget {
    fn get<'a, W: Widget + FromDynWidget + ?Sized>(
        element: &'a mut WidgetMut<'_, Self>,
    ) -> Option<WidgetMut<'a, W>>;
    fn set(element: &mut WidgetMut<'_, Self>, child: NewWidget<dyn Widget>);
    fn clear(element: &mut WidgetMut<'_, Self>);
}

impl OptionalChildParent for widgets::Badged {
    fn get<'a, W: Widget + FromDynWidget + ?Sized>(
        element: &'a mut WidgetMut<'_, Self>,
    ) -> Option<WidgetMut<'a, W>> {
        Self::badge_mut(element)
    }

    fn set(element: &mut WidgetMut<'_, Self>, child: NewWidget<dyn Widget>) {
        Self::set_badge(element, child);
    }

    fn clear(element: &mut WidgetMut<'_, Self>) {
        Self::clear_badge(element);
    }
}

impl OptionalChildParent for widgets::Divider {
    fn get<'a, W: Widget + FromDynWidget + ?Sized>(
        element: &'a mut WidgetMut<'_, Self>,
    ) -> Option<WidgetMut<'a, W>> {
        Self::content_mut(element)
    }

    fn set(element: &mut WidgetMut<'_, Self>, child: NewWidget<dyn Widget>) {
        Self::set_content(element, child);
    }

    fn clear(element: &mut WidgetMut<'_, Self>) {
        Self::clear_content(element);
    }
}

impl<T> OptionalChildState<T> {
    pub(super) fn build<State: 'static, Action, V: WidgetView<State, Action, ViewState = T>>(
        view: Option<&V>,
        ctx: &mut ViewCtx,
        app_state: &mut State,
    ) -> (Option<NewWidget<dyn Widget>>, Self) {
        let mut this = Self {
            state: None,
            generation: 0,
        };
        let child = view.map(|view| this.build_child(view, ctx, app_state));
        (child, this)
    }

    fn build_child<State: 'static, Action, V: WidgetView<State, Action, ViewState = T>>(
        &mut self,
        view: &V,
        ctx: &mut ViewCtx,
        app_state: &mut State,
    ) -> NewWidget<dyn Widget> {
        let (child, state) = ctx.with_id(ViewId::new(self.generation), |ctx| {
            view.build(ctx, app_state)
        });
        self.state = Some(state);
        child.new_widget.erased()
    }

    pub(super) fn rebuild<State: 'static, Action, V, P>(
        &mut self,
        view: Option<&V>,
        prev: Option<&V>,
        ctx: &mut ViewCtx,
        element: &mut WidgetMut<'_, P>,
        app_state: &mut State,
    ) where
        V: WidgetView<State, Action, ViewState = T>,
        P: OptionalChildParent,
    {
        if let (Some(view), Some(prev), Some(state)) = (view, prev, self.state.as_mut())
            && let Some(child) = P::get(element)
        {
            ctx.with_id(ViewId::new(self.generation), |ctx| {
                view.rebuild(prev, state, ctx, child, app_state);
            });
            return;
        }

        if self.state.is_some() {
            self.teardown(prev, ctx, element);
            P::clear(element);
            self.generation = self.generation.wrapping_add(1);
        }
        if let Some(view) = view {
            let child = self.build_child(view, ctx, app_state);
            P::set(element, child);
        }
    }

    pub(super) fn teardown<State: 'static, Action, V, P>(
        &mut self,
        view: Option<&V>,
        ctx: &mut ViewCtx,
        element: &mut WidgetMut<'_, P>,
    ) where
        V: WidgetView<State, Action, ViewState = T>,
        P: OptionalChildParent,
    {
        let state = self.state.take();
        ctx.with_id(ViewId::new(self.generation), |ctx| {
            if let (Some(view), Some(mut state), Some(child)) = (view, state, P::get(element)) {
                view.teardown(&mut state, ctx, child);
            } else {
                ctx.remove_action_sources_under_current_path();
            }
        });
    }

    pub(super) fn message<State: 'static, Action, V, P>(
        &mut self,
        view: Option<&V>,
        message: &mut MessageCtx,
        element: &mut WidgetMut<'_, P>,
        app_state: &mut State,
    ) -> MessageResult<Action>
    where
        V: WidgetView<State, Action, ViewState = T>,
        P: OptionalChildParent,
    {
        if message.take_first() != Some(ViewId::new(self.generation)) {
            return MessageResult::Stale;
        }
        let (Some(view), Some(state), Some(child)) = (view, self.state.as_mut(), P::get(element))
        else {
            return MessageResult::Stale;
        };
        view.message(state, message, child, app_state)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use masonry::app::{RenderRoot, RenderRootOptions, WindowSizePolicy};
    use masonry::dpi::PhysicalSize;
    use masonry::kurbo::Axis;
    use masonry::widgets::{ButtonPress, Divider};

    use crate::core::{DynMessage, Environment, ProxyError, RawProxy, SendMessage};
    use crate::view::{button, spinner};

    use super::*;

    #[derive(Debug)]
    struct ClosedProxy;

    impl RawProxy for ClosedProxy {
        fn send_message(&self, _: Arc<[ViewId]>, message: SendMessage) -> Result<(), ProxyError> {
            Err(ProxyError::DriverFinished(message))
        }

        fn dyn_debug(&self) -> &dyn std::fmt::Debug {
            self
        }
    }

    #[test]
    fn replacing_optional_child_rejects_old_messages_and_cleans_routing() {
        let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
        let mut ctx = ViewCtx::new(Arc::new(ClosedProxy), runtime);
        let view = button(spinner(), |count: &mut usize| *count += 1);
        let mut count = 0;
        let (child, mut state) = OptionalChildState::build(Some(&view), &mut ctx, &mut count);
        let child = child.unwrap();
        let old_id = child.id();
        let old_path = ctx.get_id_path(old_id).unwrap().clone();
        let mut root = RenderRoot::new(
            NewWidget::new(Divider::new(Axis::Horizontal).content(child)),
            |_| {},
            RenderRootOptions {
                default_properties: Arc::new(masonry::theme::default_property_set()),
                use_system_fonts: false,
                size_policy: WindowSizePolicy::User,
                size: PhysicalSize::new(100, 100),
                scale_factor: 1.0,
                test_font: None,
            },
        );
        let send = |path| {
            MessageCtx::new(
                Environment::default(),
                path,
                DynMessage::new(ButtonPress { button: None }),
            )
        };

        root.edit_base_layer(|mut element: WidgetMut<'_, Divider>| {
            state.rebuild(None, Some(&view), &mut ctx, &mut element, &mut count);
        })
        .unwrap();
        assert!(
            ctx.get_id_path(old_id).is_none(),
            "removed child must not retain its action route"
        );
        root.edit_base_layer(|mut element: WidgetMut<'_, Divider>| {
            state.rebuild(Some(&view), None, &mut ctx, &mut element, &mut count);
        })
        .unwrap();

        let new_id = root
            .edit_base_layer(|mut element: WidgetMut<'_, Divider>| {
                Divider::content_mut::<dyn Widget>(&mut element)
                    .unwrap()
                    .ctx
                    .widget_id()
            })
            .unwrap();
        let new_path = ctx.get_id_path(new_id).unwrap().clone();
        assert_ne!(
            old_path, new_path,
            "recreated child must have a new routing generation"
        );

        root.edit_base_layer(|mut element: WidgetMut<'_, Divider>| {
            assert!(
                matches!(
                    state.message(Some(&view), &mut send(old_path), &mut element, &mut count),
                    MessageResult::Stale
                ),
                "an old asynchronous message must not reach the new child"
            );
            assert_eq!(count, 0);
            assert!(
                matches!(
                    state.message(Some(&view), &mut send(new_path), &mut element, &mut count),
                    MessageResult::Action(())
                ),
                "the current generation must still receive actions"
            );
            assert_eq!(count, 1);
        })
        .unwrap();
    }
}
