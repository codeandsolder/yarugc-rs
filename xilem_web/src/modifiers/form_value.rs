// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::marker::PhantomData;

use wasm_bindgen::JsCast as _;

use crate::core::{Mut, View, ViewElement, ViewMarker};
use crate::modifiers::{Modifier, WithModifier};
use crate::{AttributeValue, DomView, IntoAttributeValue, ViewCtx};

/// Retained state for the live value of a form control.
#[derive(Default)]
pub struct FormValueState {
    idx: usize,
    len: usize,
    value: Option<AttributeValue>,
}

impl FormValueState {
    fn push(this: &mut Modifier<'_, Self>, value: Option<&AttributeValue>) {
        this.modifier.idx += 1;
        this.modifier.len += 1;
        this.modifier.value = value.cloned();
        this.flags.set_needs_update();
    }

    fn update(this: &mut Modifier<'_, Self>, value: Option<&AttributeValue>) {
        if this.flags.was_created() {
            Self::push(this, value);
        } else {
            this.modifier.idx += 1;
            if this.modifier.idx == this.modifier.len {
                if this.modifier.value.as_ref() != value {
                    this.modifier.value = value.cloned();
                }
                // The user may have changed the live property even when the
                // application supplied the same value again.
                this.flags.set_needs_update();
            }
        }
    }

    pub(crate) fn apply(&self, element: &web_sys::Element) {
        let value = self
            .value
            .as_ref()
            .map(AttributeValue::serialize)
            .unwrap_or_default();
        if let Some(input) = element.dyn_ref::<web_sys::HtmlInputElement>() {
            assign_if_changed(&input.value(), &value, |value| input.set_value(value));
        } else if let Some(select) = element.dyn_ref::<web_sys::HtmlSelectElement>() {
            if self.value.is_none() {
                if select.selected_index() != -1 {
                    select.set_selected_index(-1);
                }
            } else if select.selected_index() == -1 || select.value() != value {
                select.set_value(&value);
            }
        } else if let Some(textarea) = element.dyn_ref::<web_sys::HtmlTextAreaElement>() {
            assign_if_changed(&textarea.value(), &value, |value| textarea.set_value(value));
        }
    }
}

fn assign_if_changed(current: &str, desired: &str, set: impl FnOnce(&str)) {
    if current != desired {
        set(desired);
    }
}

/// A controlled live form value, separate from an HTML content attribute.
pub struct FormValue<V, State, Action> {
    inner: V,
    value: Option<AttributeValue>,
    phantom: PhantomData<fn() -> (State, Action)>,
}

impl<V, State, Action> FormValue<V, State, Action> {
    pub(crate) fn new(inner: V, value: impl IntoAttributeValue) -> Self {
        Self {
            inner,
            value: value.into_attr_value(),
            phantom: PhantomData,
        }
    }
}

impl<V, State, Action> ViewMarker for FormValue<V, State, Action> {}

impl<V, State, Action> View<State, Action, ViewCtx> for FormValue<V, State, Action>
where
    State: 'static,
    Action: 'static,
    V: DomView<State, Action, Element: WithModifier<FormValueState>>,
    for<'a> <V::Element as ViewElement>::Mut<'a>: WithModifier<FormValueState>,
{
    type Element = V::Element;
    type ViewState = V::ViewState;

    fn build(&self, ctx: &mut ViewCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        let (mut element, state) = self.inner.build(ctx, app_state);
        FormValueState::push(&mut element.modifier(), self.value.as_ref());
        (element, state)
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        element.modifier().modifier.idx -= 1;
        self.inner.rebuild(
            &prev.inner,
            view_state,
            ctx,
            element.reborrow_mut(),
            app_state,
        );
        FormValueState::update(&mut element.modifier(), self.value.as_ref());
    }

    forward_view_lifecycle!(inner, State, Action);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PodFlags;

    #[test]
    fn outer_value_wins_and_equal_model_still_reconciles() {
        let mut state = FormValueState::default();
        let mut flags = PodFlags::new(false);
        let m = &mut Modifier::new(&mut state, &mut flags);
        let inner = "inner".into_attr_value();
        let outer = "outer".into_attr_value();
        FormValueState::push(m, inner.as_ref());
        FormValueState::push(m, outer.as_ref());
        assert_eq!(m.modifier.value, outer);
        m.flags.clear();
        m.modifier.idx -= 2;
        FormValueState::update(m, None);
        assert!(!m.flags.needs_update());
        FormValueState::update(m, outer.as_ref());
        assert!(m.flags.needs_update());
        assert_eq!(m.modifier.value, outer);
        m.flags.clear();
        m.modifier.idx -= 1;
        FormValueState::update(m, None);
        assert!(m.flags.needs_update());
        assert_eq!(m.modifier.value, None);
    }

    #[test]
    fn property_setter_only_runs_when_live_value_differs() {
        assign_if_changed("same", "same", |_| panic!("unnecessary value write"));
        let mut result = String::new();
        assign_if_changed("old", "new", |v| result.push_str(v));
        assert_eq!(result, "new");
    }
}
