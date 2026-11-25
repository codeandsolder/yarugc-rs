// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::marker::PhantomData;

use masonry::widgets::{self, SliderMoved, SliderState};

use crate::core::{MessageCtx, MessageResult, Mut, View, ViewMarker};
use crate::{Pod, ViewCtx, WidgetView};

/// A view that displays a [`Slider`] widget.
pub struct Slider<State, Action, F> {
    min: f64,
    max: f64,
    value: f64,
    on_change: F,
    step: Option<f64>,
    disabled: bool,
    phantom: PhantomData<fn(State) -> Action>,
}

/// Creates a slider widget for selecting a value from a range.
pub fn slider<State: 'static, Action, F: Fn(&mut State, f64) -> Action + Send + Sync + 'static>(
    min: f64,
    max: f64,
    value: f64,
    on_change: F,
) -> Slider<State, Action, F>
where
    Slider<State, Action, F>: WidgetView<State, Action>,
{
    Slider {
        min,
        max,
        value,
        on_change,
        step: None,
        disabled: false,
        phantom: PhantomData,
    }
}

impl<State, Action, F> Slider<State, Action, F> {
    /// Sets the stepping interval of the slider.
    #[must_use = "Use the configured view."]
    pub fn step(mut self, step: f64) -> Self {
        if step.is_finite() && step > 0.0 {
            self.step = Some(step);
        }
        self
    }
    /// Sets whether the slider is disabled.
    #[must_use = "Use the configured view."]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl<State, Action, F> ViewMarker for Slider<State, Action, F> {}
impl<F, State, Action> View<State, Action, ViewCtx> for Slider<State, Action, F>
where
    State: 'static,
    Action: 'static,
    F: Fn(&mut State, f64) -> Action + Send + Sync + 'static,
{
    type Element = Pod<widgets::Slider>;
    type ViewState = ();

    fn build(&self, ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        (
            ctx.with_action_widget({
                let widget = widgets::Slider::from_state(SliderState::new(
                    self.min, self.max, self.value, self.step,
                ));
                let mut pod = Pod::new(widget);
                pod.new_widget.options.disabled = self.disabled;
                pod
            }),
            (),
        )
    }

    fn rebuild(
        &self,
        prev: &Self,
        (): &mut Self::ViewState,
        _: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        _: &mut State,
    ) {
        if prev.disabled != self.disabled {
            element.ctx.set_disabled(self.disabled);
        }
        widgets::Slider::set_state(
            &mut element,
            SliderState::new(self.min, self.max, self.value, self.step),
        );
    }

    fn teardown(
        &self,
        (): &mut Self::ViewState,
        ctx: &mut ViewCtx,
        element: Mut<'_, Self::Element>,
    ) {
        ctx.teardown_action_source(element.ctx.widget_id());
    }

    fn message(
        &self,
        (): &mut Self::ViewState,
        message: &mut MessageCtx,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        if element.ctx.is_disabled() || element.ctx.is_stashed() {
            return MessageResult::Stale;
        }
        if message.take_first().is_some() {
            tracing::warn!("Got unexpected id path in Slider::message");
            return MessageResult::Stale;
        }
        let Some(value) = message.take_message::<SliderMoved>() else {
            tracing::error!(
                "Wrong message type in Slider::message: {message:?}, expected {}",
                std::any::type_name::<SliderMoved>(),
            );
            return MessageResult::Stale;
        };
        MessageResult::Action((self.on_change)(app_state, value.value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{render_root, view_context};

    #[test]
    fn rebuild_applies_new_configuration_atomically_and_restores_controlled_value() {
        let mut ctx = view_context();
        let callback = |_: &mut (), _| ();
        let initial = slider(0.0, 10.0, 5.0, callback).step(5.0);
        let updated = slider(20.0, 40.0, 32.0, callback).step(3.0);
        let (pod, mut state) = initial.build(&mut ctx, &mut ());
        let id = pod.new_widget.id();
        let (mut root, _receive) = render_root(pod.new_widget, (200, 32));
        root.edit_base_layer(|element| {
            updated.rebuild(&initial, &mut state, &mut ctx, element, &mut ());
        })
        .unwrap();
        assert_eq!(
            root.get_widget(id)
                .unwrap()
                .downcast::<widgets::Slider>()
                .unwrap()
                .value(),
            32.0,
            "new value must use new range and step, not old configuration"
        );

        root.edit_base_layer(|mut element| {
            widgets::Slider::set_value(&mut element, 38.0);
        })
        .unwrap();
        root.edit_base_layer(|element| {
            updated.rebuild(&updated, &mut state, &mut ctx, element, &mut ());
        })
        .unwrap();
        assert_eq!(
            root.get_widget(id)
                .unwrap()
                .downcast::<widgets::Slider>()
                .unwrap()
                .value(),
            32.0,
            "unchanged declarative value must restore controlled state"
        );
    }
}
