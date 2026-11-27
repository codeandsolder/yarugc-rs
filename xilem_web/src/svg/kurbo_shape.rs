// Copyright 2023 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Implementation of the View trait for various kurbo shapes.

use peniko::kurbo::{BezPath, Circle, Line, Rect};

use crate::core::{MessageCtx, MessageResult, Mut, OrphanView};
use crate::modifiers::{Attributes, WithModifier};
use crate::{FromWithContext, Pod, SVG_NS, ViewCtx};

fn create_element<R>(
    name: &str,
    ctx: &mut ViewCtx,
    attr_size_hint: usize,
    f: impl FnOnce(Pod<web_sys::Element>, &mut ViewCtx) -> R,
) -> R {
    ctx.with_size_hint::<Attributes, _>(attr_size_hint, |ctx| {
        let element = if ctx.is_hydrating() {
            Pod::hydrate_element_with_ctx(Vec::new(), ctx)
        } else {
            Pod::new_element_with_ctx(Vec::new(), SVG_NS, name, ctx)
        };
        f(element, ctx)
    })
}

// Declare the SVG attribute names and their values once, so initial creation,
// hydration and subsequent rebuilds use exactly the same geometry schema.
macro_rules! numeric_shape {
    ($shape:ty, $element:ty, $tag:literal, |$view:ident| [$($key:literal => $value:expr),+ $(,)?]) => {
        impl<State: 'static, Action: 'static> OrphanView<$shape, State, Action> for ViewCtx {
            type OrphanViewState = ();
            type OrphanElement = Pod<$element>;

            fn orphan_build(
                $view: &$shape,
                ctx: &mut Self,
                _: &mut State,
            ) -> (Self::OrphanElement, Self::OrphanViewState) {
                let values = [$(($key, $value)),+];
                create_element($tag, ctx, values.len(), |element, ctx| {
                    let mut element = Self::OrphanElement::from_with_ctx(element, ctx);
                    let attrs = &mut element.modifier();
                    for value in values {
                        Attributes::push(attrs, value);
                    }
                    (element, ())
                })
            }

            fn orphan_rebuild(
                new: &$shape,
                prev: &$shape,
                (): &mut Self::OrphanViewState,
                _ctx: &mut Self,
                element: Mut<'_, Self::OrphanElement>,
                _: &mut State,
            ) {
                let values = |$view: &$shape| [$($value),+];
                let keys = [$($key),+];
                Attributes::rebuild(element, keys.len(), |mut element| {
                    let attrs = &mut element.modifier();
                    for ((key, previous), next) in keys.into_iter().zip(values(prev)).zip(values(new)) {
                        Attributes::update_with_same_key(attrs, key, &previous, &next);
                    }
                });
            }

            fn orphan_teardown(
                _view: &$shape,
                (): &mut Self::OrphanViewState,
                _ctx: &mut Self,
                _element: Mut<'_, Self::OrphanElement>,
            ) {}

            fn orphan_message(
                _view: &$shape,
                (): &mut Self::OrphanViewState,
                _message: &mut MessageCtx,
                _element: Mut<'_, Self::OrphanElement>,
                _app_state: &mut State,
            ) -> MessageResult<Action> {
                MessageResult::Stale
            }
        }
    };
}

numeric_shape!(Line, web_sys::SvgLineElement, "line", |view| [
    "x1" => view.p0.x,
    "y1" => view.p0.y,
    "x2" => view.p1.x,
    "y2" => view.p1.y,
]);

numeric_shape!(Rect, web_sys::SvgRectElement, "rect", |view| [
    "x" => view.x0,
    "y" => view.y0,
    "width" => view.width(),
    "height" => view.height(),
]);

numeric_shape!(Circle, web_sys::SvgCircleElement, "circle", |view| [
    "cx" => view.center.x,
    "cy" => view.center.y,
    "r" => view.radius,
]);

impl<State: 'static, Action: 'static> OrphanView<BezPath, State, Action> for ViewCtx {
    type OrphanViewState = ();
    type OrphanElement = Pod<web_sys::SvgPathElement>;

    fn orphan_build(
        view: &BezPath,
        ctx: &mut Self,
        _: &mut State,
    ) -> (Self::OrphanElement, Self::OrphanViewState) {
        create_element("path", ctx, 1, |element, ctx| {
            let mut element = Self::OrphanElement::from_with_ctx(element, ctx);
            let attrs = &mut element.modifier();
            Attributes::push(attrs, ("d", view.to_svg()));
            (element, ())
        })
    }

    fn orphan_rebuild(
        new: &BezPath,
        prev: &BezPath,
        (): &mut Self::OrphanViewState,
        _ctx: &mut Self,
        element: Mut<'_, Self::OrphanElement>,
        _: &mut State,
    ) {
        Attributes::rebuild(element, 1, |mut element| {
            let attrs = &mut element.modifier();
            if attrs.flags.was_created() {
                Attributes::push(attrs, ("d", new.to_svg()));
            } else if new != prev {
                Attributes::mutate(attrs, |m| *m = ("d", new.to_svg()).into());
            } else {
                Attributes::skip(attrs, 1);
            }
        });
    }

    fn orphan_teardown(
        _view: &BezPath,
        _view_state: &mut Self::OrphanViewState,
        _ctx: &mut Self,
        _element: Mut<'_, Self::OrphanElement>,
    ) {
    }

    fn orphan_message(
        _view: &BezPath,
        _view_state: &mut Self::OrphanViewState,
        _message: &mut MessageCtx,
        _element: Mut<'_, Self::OrphanElement>,
        _app_state: &mut State,
    ) -> MessageResult<Action> {
        MessageResult::Stale
    }
}

// TODO: RoundedRect
