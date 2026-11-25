// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::core::{MessageCtx, MessageResult, Mut, View, ViewMarker};
use crate::{Pod, ViewCtx};

use masonry::core::ArcStr;
use masonry::widgets::{self, RadioButtonSelected};

/// An element which can be in checked and unchecked state.
///
/// # Example
/// ```ignore
/// use xilem::view::{flex_row, radio_button};
///
/// #[derive(Debug, PartialEq, Eq)]
/// enum Fruit {
///     Banana,
///     Apple,
///     Lime,
/// }
///
/// struct State {
///     fruit: Fruit,
/// }
///
/// // ...
///
/// flex_row((
///     radio_button("Banana", app_state.fruit == Fruit::Banana, |app_state: &mut State| {
///         app_state.fruit = Fruit::Banana;
///     }),
///     radio_button("Apple", app_state.fruit == Fruit::Apple, |app_state: &mut State| {
///         app_state.fruit = Fruit::Apple;
///     }),
///     radio_button("Lime", app_state.fruit == Fruit::Lime, |app_state: &mut State| {
///         app_state.fruit = Fruit::Lime;
///     }),
/// ))
/// ```
pub fn radio_button<F, State, Action>(
    label: impl Into<ArcStr>,
    checked: bool,
    callback: F,
) -> RadioButton<F>
where
    F: Fn(&mut State) -> Action + Send + 'static,
{
    RadioButton {
        label: label.into(),
        callback,
        checked,
        disabled: false,
    }
}

/// The [`View`] created by [`radio_button`] from a `label`, a bool value and a callback.
///
/// See `radio_button` documentation for more context.
#[must_use = "View values do nothing unless provided to Xilem."]
pub struct RadioButton<F> {
    label: ArcStr,
    checked: bool,
    callback: F,
    disabled: bool,
}

impl<F> RadioButton<F> {
    /// Set the disabled state of the widget.
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl<F> ViewMarker for RadioButton<F> {}
impl<F, State, Action> View<State, Action, ViewCtx> for RadioButton<F>
where
    State: 'static,
    Action: 'static,
    F: Fn(&mut State) -> Action + Send + Sync + 'static,
{
    type Element = Pod<widgets::RadioButton>;
    type ViewState = ();

    fn build(&self, ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        let element = ctx.with_action_widget({
            let mut pod = Pod::new(widgets::RadioButton::new(self.checked, self.label.clone()));
            pod.new_widget.options.disabled = self.disabled;
            pod
        });
        (element, ())
    }

    fn rebuild(
        &self,
        prev: &Self,
        (): &mut Self::ViewState,
        _ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        _: &mut State,
    ) {
        if prev.disabled != self.disabled {
            element.ctx.set_disabled(self.disabled);
        }
        if prev.label != self.label {
            widgets::RadioButton::set_text(&mut element, self.label.clone());
        }
        if element.widget.is_checked() != self.checked {
            widgets::RadioButton::set_checked(&mut element, self.checked);
        }
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
        debug_assert!(
            message.remaining_path().is_empty(),
            "id path should be empty in RadioButton::message"
        );
        if element.ctx.is_disabled() || element.ctx.is_stashed() {
            return MessageResult::Stale;
        }
        if message.take_message::<RadioButtonSelected>().is_some() {
            MessageResult::Action((self.callback)(app_state))
        } else {
            tracing::error!("Wrong message type in RadioButton::message, got {message:?}.");
            MessageResult::Stale
        }
    }
}
