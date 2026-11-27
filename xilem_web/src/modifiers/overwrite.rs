// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use super::Modifier;

/// A last-writer-wins boolean modifier, with constant storage regardless of nesting.
///
/// Only the outermost modifier affects the DOM property. Reconciliation visits
/// inner modifiers first and then the outer ones. There is no need to retain
/// shadowed values; each view supplies its current value on rebuild.
#[derive(Default)]
pub struct OverwriteBool {
    // Rebuild walks down by decrementing the cursor; build/rebuild then advances
    // it on the way out. A memoized child may retain its existing cursor prefix.
    idx: usize,
    len: usize,
    effective: Option<bool>,
    needs_update: bool,
}

impl OverwriteBool {
    pub const fn rebuild(&mut self, prev_len: usize) {
        self.idx -= prev_len;
    }

    fn set_effective(this: &mut Modifier<'_, Self>, value: bool) {
        if this.modifier.effective != Some(value) {
            this.modifier.effective = Some(value);
            this.modifier.needs_update = true;
            this.flags.set_needs_update();
        }
    }

    /// Append a modifier while creating a fresh element.
    pub fn push(this: &mut Modifier<'_, Self>, value: bool) {
        debug_assert!(
            this.flags.was_created(),
            "Use update when rebuilding an existing element."
        );
        this.modifier.idx += 1;
        this.modifier.len += 1;
        Self::set_effective(this, value);
    }

    /// Reconcile the next modifier. Only the outermost value is observable.
    pub fn update(this: &mut Modifier<'_, Self>, value: bool) {
        if this.flags.was_created() {
            Self::push(this, value);
        } else {
            this.modifier.idx += 1;
            if this.modifier.idx == this.modifier.len {
                Self::set_effective(this, value);
            }
        }
    }

    /// Apply a changed effective value, at most once per reconciliation.
    pub fn apply_changes(&mut self, f: impl FnOnce(Option<bool>)) {
        if core::mem::take(&mut self.needs_update) {
            f(self.effective);
        }
    }
}

#[macro_export]
/// A macro to create a boolean overwrite modifier.
macro_rules! overwrite_bool_modifier {
    ($modifier: ident) => {
        #[derive(Default)]
        pub struct $modifier($crate::modifiers::OverwriteBool);

        impl $modifier {
            const fn as_overwrite_bool_modifier(
                this: $crate::modifiers::Modifier<'_, Self>,
            ) -> $crate::modifiers::Modifier<'_, $crate::modifiers::OverwriteBool> {
                $crate::modifiers::Modifier::new(&mut this.modifier.0, this.flags)
            }

            pub fn apply_changes(&mut self, f: impl FnOnce(Option<bool>)) {
                self.0.apply_changes(f);
            }
        }
    };
}

#[macro_export]
/// A macro to create a boolean overwrite modifier view for a modifier that's in the parent module with the same name.
macro_rules! overwrite_bool_modifier_view {
    ($modifier: ident) => {
        pub struct $modifier<V, State, Action> {
            value: bool,
            inner: V,
            phantom: std::marker::PhantomData<fn() -> (State, Action)>,
        }

        impl<V, State, Action> $modifier<V, State, Action> {
            pub fn new(inner: V, value: bool) -> Self {
                $modifier {
                    inner,
                    value,
                    phantom: std::marker::PhantomData,
                }
            }
        }

        impl<V, State, Action> $crate::core::ViewMarker for $modifier<V, State, Action> {}
        impl<V, State, Action> $crate::core::View<State, Action, $crate::ViewCtx>
            for $modifier<V, State, Action>
        where
            State: 'static,
            Action: 'static,
            V: $crate::DomView<
                    State,
                    Action,
                    Element: $crate::modifiers::WithModifier<super::$modifier>,
                >,
            for<'a> <V::Element as $crate::core::ViewElement>::Mut<'a>:
                $crate::modifiers::WithModifier<super::$modifier>,
        {
            type Element = V::Element;

            type ViewState = V::ViewState;

            fn build(
                &self,
                ctx: &mut $crate::ViewCtx,
                app_state: &mut State,
            ) -> (Self::Element, Self::ViewState) {
                use $crate::modifiers::WithModifier;
                let (mut el, state) = self.inner.build(ctx, app_state);
                let modifier = &mut super::$modifier::as_overwrite_bool_modifier(el.modifier());
                $crate::modifiers::OverwriteBool::push(modifier, self.value);
                (el, state)
            }

            fn rebuild(
                &self,
                prev: &Self,
                view_state: &mut Self::ViewState,
                ctx: &mut $crate::ViewCtx,
                mut element: $crate::core::Mut<'_, Self::Element>,
                app_state: &mut State,
            ) {
                use $crate::modifiers::WithModifier;
                element.modifier().modifier.0.rebuild(1);
                self.inner.rebuild(
                    &prev.inner,
                    view_state,
                    ctx,
                    element.reborrow_mut(),
                    app_state,
                );
                let mut modifier = super::$modifier::as_overwrite_bool_modifier(element.modifier());
                $crate::modifiers::OverwriteBool::update(&mut modifier, self.value);
            }

            fn teardown(
                &self,
                view_state: &mut Self::ViewState,
                ctx: &mut $crate::ViewCtx,
                element: $crate::core::Mut<'_, Self::Element>,
            ) {
                self.inner.teardown(view_state, ctx, element);
            }

            fn message(
                &self,
                view_state: &mut Self::ViewState,
                message: &mut $crate::core::MessageCtx,
                element: $crate::core::Mut<'_, Self::Element>,
                app_state: &mut State,
            ) -> $crate::core::MessageResult<Action> {
                self.inner.message(view_state, message, element, app_state)
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PodFlags;

    #[test]
    fn outermost_value_wins_for_arbitrary_nesting() {
        let mut modifier = OverwriteBool::default();
        let mut flags = PodFlags::new(false);
        let m = &mut Modifier::new(&mut modifier, &mut flags);
        assert!(!m.flags.needs_update());
        for i in 0..257 {
            OverwriteBool::push(m, i % 2 == 0);
        }
        let mut applied = Vec::new();
        m.modifier.apply_changes(|value| applied.push(value));
        assert_eq!(applied, [Some(true)]);
        m.flags.clear();

        m.modifier.rebuild(257);
        for i in 0..256 {
            OverwriteBool::update(m, i % 2 != 0);
        }
        assert!(!m.flags.needs_update());
        OverwriteBool::update(m, true);
        m.modifier.apply_changes(|value| applied.push(value));
        assert_eq!(applied, [Some(true)]);

        // A memoized inner subtree may retain a prefix of modifiers.
        m.modifier.rebuild(1);
        OverwriteBool::update(m, false);
        assert!(m.flags.needs_update());
        m.modifier.apply_changes(|value| applied.push(value));
        assert_eq!(applied, [Some(true), Some(false)]);
    }

    #[test]
    fn recreated_element_applies_even_false_initial_value() {
        let mut modifier = OverwriteBool::default();
        let mut flags = PodFlags::new(false);
        let m = &mut Modifier::new(&mut modifier, &mut flags);
        OverwriteBool::update(m, false);
        let mut applied = Vec::new();
        m.modifier.apply_changes(|value| applied.push(value));
        m.modifier.apply_changes(|value| applied.push(value));
        assert_eq!(applied, [Some(false)]);
    }
}
