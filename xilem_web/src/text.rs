// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use wasm_bindgen::{JsCast, UnwrapThrowExt};

use crate::core::{MessageCtx, MessageResult, Mut, OrphanView};
use crate::{Pod, PodFlags, ViewCtx};

// strings -> text nodes
macro_rules! impl_string_view {
    ($ty:ty) => {
        impl<State: 'static, Action> OrphanView<$ty, State, Action> for ViewCtx {
            type OrphanElement = Pod<web_sys::Text>;

            type OrphanViewState = ();

            fn orphan_build(
                view: &$ty,
                ctx: &mut ViewCtx,
                _: &mut State,
            ) -> (Self::OrphanElement, Self::OrphanViewState) {
                let node = if ctx.is_hydrating() {
                    ctx.hydrate_node().unwrap_throw().unchecked_into()
                } else {
                    ctx.document().create_text_node(view)
                };
                (Pod::new(node, (), PodFlags::new(ctx.is_hydrating())), ())
            }

            fn orphan_rebuild(
                new: &$ty,
                prev: &$ty,
                (): &mut Self::OrphanViewState,
                _ctx: &mut ViewCtx,
                element: Mut<'_, Self::OrphanElement>,
                _: &mut State,
            ) {
                if prev != new {
                    element.node.set_data(new);
                }
            }

            fn orphan_teardown(
                _view: &$ty,
                _view_state: &mut Self::OrphanViewState,
                _ctx: &mut ViewCtx,
                _element: Mut<'_, Self::OrphanElement>,
            ) {
            }

            fn orphan_message(
                _view: &$ty,
                _view_state: &mut Self::OrphanViewState,
                _message: &mut MessageCtx,
                _element: Mut<'_, Self::OrphanElement>,
                _app_state: &mut State,
            ) -> MessageResult<Action> {
                // TODO: Panic?
                MessageResult::Stale
            }
        }
    };
}

impl_string_view!(&'static str);
impl_string_view!(String);
impl_string_view!(std::borrow::Cow<'static, str>);

macro_rules! impl_to_string_view {
    ($ty:ty) => {
        impl_to_string_view!($ty, |prev: &$ty, new: &$ty| prev != new);
    };
    ($ty:ty, $changed:expr) => {
        impl<State: 'static, Action> OrphanView<$ty, State, Action> for ViewCtx {
            type OrphanElement = Pod<web_sys::Text>;

            type OrphanViewState = ();

            fn orphan_build(
                view: &$ty,
                ctx: &mut ViewCtx,
                _: &mut State,
            ) -> (Self::OrphanElement, Self::OrphanViewState) {
                let node = if ctx.is_hydrating() {
                    ctx.hydrate_node().unwrap().unchecked_into()
                } else {
                    ctx.document().create_text_node(&view.to_string())
                };
                (Pod::new(node, (), PodFlags::new(ctx.is_hydrating())), ())
            }

            fn orphan_rebuild(
                new: &$ty,
                prev: &$ty,
                (): &mut Self::OrphanViewState,
                _ctx: &mut ViewCtx,
                element: Mut<'_, Self::OrphanElement>,
                _: &mut State,
            ) {
                if ($changed)(prev, new) {
                    element.node.set_data(&new.to_string());
                }
            }

            fn orphan_teardown(
                _view: &$ty,
                _view_state: &mut Self::OrphanViewState,
                _ctx: &mut ViewCtx,
                _element: Mut<'_, Pod<web_sys::Text>>,
            ) {
            }

            fn orphan_message(
                _view: &$ty,
                _view_state: &mut Self::OrphanViewState,
                _message: &mut MessageCtx,
                _element: Mut<'_, Self::OrphanElement>,
                _app_state: &mut State,
            ) -> MessageResult<Action> {
                MessageResult::Stale
            }
        }
    };
}

// Allow numbers to be used directly as a view
impl_to_string_view!(f32, |prev: &f32, new: &f32| prev.to_bits() != new.to_bits());
impl_to_string_view!(f64, |prev: &f64, new: &f64| prev.to_bits() != new.to_bits());
impl_to_string_view!(i8);
impl_to_string_view!(u8);
impl_to_string_view!(i16);
impl_to_string_view!(u16);
impl_to_string_view!(i32);
impl_to_string_view!(u32);
impl_to_string_view!(i64);
impl_to_string_view!(u64);
impl_to_string_view!(u128);
impl_to_string_view!(isize);
impl_to_string_view!(usize);
