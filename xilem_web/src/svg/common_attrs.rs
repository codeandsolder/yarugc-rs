// Copyright 2023 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::borrow::Borrow;
use std::fmt::Write as _;
use std::marker::PhantomData;

use peniko::{Color, kurbo};

use crate::core::{Mut, View, ViewElement, ViewMarker};
use crate::modifiers::{AttributeModifier, Attributes, Modifier, WithModifier};
use crate::{DomView, ViewCtx};

pub struct Fill<V, State, Action> {
    child: V,
    // This could reasonably be static Cow also, but keep things simple
    color: Color,
    phantom: PhantomData<fn() -> (State, Action)>,
}

/// Stroke geometry expressible by SVG, which uses a single cap for both ends.
#[derive(Clone, Debug, PartialEq)]
#[must_use = "use the configured stroke style in a view"]
pub struct StrokeStyle(kurbo::Stroke);

impl Default for StrokeStyle {
    fn default() -> Self {
        Self::new(1.0)
    }
}

impl StrokeStyle {
    /// Create a stroke with round caps and joins, and the specified width.
    pub const fn new(width: f64) -> Self {
        Self(kurbo::Stroke::new(width))
    }

    /// Set the cap used for both ends of each subpath and dash.
    pub const fn with_cap(mut self, cap: kurbo::Cap) -> Self {
        self.0.start_cap = cap;
        self.0.end_cap = cap;
        self
    }

    /// Set the shape of joins between segments.
    pub const fn with_join(mut self, join: kurbo::Join) -> Self {
        self.0.join = join;
        self
    }

    /// Set the maximum miter length relative to stroke width.
    pub const fn with_miter_limit(mut self, limit: f64) -> Self {
        self.0.miter_limit = limit;
        self
    }

    /// Set the dash offset and alternating painted and unpainted lengths.
    pub fn with_dashes<P>(mut self, offset: f64, pattern: P) -> Self
    where
        P: IntoIterator,
        P::Item: Borrow<f64>,
    {
        self.0 = self.0.with_dashes(offset, pattern);
        self
    }
}

pub struct Stroke<V, State, Action> {
    child: V,
    // This could reasonably be static Cow also, but keep things simple
    color: Color,
    style: StrokeStyle,
    phantom: PhantomData<fn() -> (State, Action)>,
}

pub fn fill<State, Action, V>(child: V, color: impl Into<Color>) -> Fill<V, State, Action> {
    Fill {
        child,
        color: color.into(),
        phantom: PhantomData,
    }
}

pub fn stroke<State, Action, V>(
    child: V,
    color: impl Into<Color>,
    style: StrokeStyle,
) -> Stroke<V, State, Action> {
    Stroke {
        child,
        color: color.into(),
        style,
        phantom: PhantomData,
    }
}

/// Rather general join string function, might be reused somewhere else as well...
fn join(iter: &mut impl Iterator<Item: std::fmt::Display>, sep: &str) -> String {
    iter.next().map_or_else(String::new, |first| {
        let (lower, _) = iter.size_hint();
        let mut result = String::with_capacity(sep.len() * lower);
        // Formatting into a String cannot fail for the numeric Display implementations used here.
        let _ = write!(&mut result, "{first}");
        for item in iter {
            result.push_str(sep);
            let _ = write!(&mut result, "{item}");
        }
        result
    })
}

/// Convert a solid SVG color into its hex string.
///
/// This will not include any alpha, if present,
/// as it is handled separately via the opacity attribute
/// instead.
fn color_to_string(color: &Color) -> String {
    if color.components[3] == 0.0 {
        "none".into()
    } else {
        format!("{:x}", color.discard_alpha().to_rgba8())
    }
}

fn opacity_attr_modifier(attr: &'static str, color: &Color) -> AttributeModifier {
    let alpha = color.components[3];
    (attr, (alpha < 1.0).then_some(f64::from(alpha))).into()
}

impl<V, State, Action> ViewMarker for Fill<V, State, Action> {}
impl<State, Action, V> View<State, Action, ViewCtx> for Fill<V, State, Action>
where
    State: 'static,
    Action: 'static,
    V: DomView<State, Action, Element: WithModifier<Attributes>>,
    for<'a> <V::Element as ViewElement>::Mut<'a>: WithModifier<Attributes>,
{
    type ViewState = V::ViewState;
    type Element = V::Element;

    fn build(&self, ctx: &mut ViewCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        let (mut element, state) =
            ctx.with_size_hint::<Attributes, _>(2, |ctx| self.child.build(ctx, app_state));
        let mut attrs = element.modifier();
        Attributes::push(&mut attrs, ("fill", color_to_string(&self.color)));
        Attributes::push(
            &mut attrs,
            opacity_attr_modifier("fill-opacity", &self.color),
        );
        (element, state)
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        Attributes::rebuild(element, 2, |mut element| {
            self.child.rebuild(
                &prev.child,
                view_state,
                ctx,
                element.reborrow_mut(),
                app_state,
            );
            let mut attrs = element.modifier();
            if attrs.flags.was_created() {
                Attributes::push(&mut attrs, ("fill", color_to_string(&self.color)));
                Attributes::push(
                    &mut attrs,
                    opacity_attr_modifier("fill-opacity", &self.color),
                );
            } else if self.color == prev.color {
                Attributes::skip(&mut attrs, 2);
            } else {
                Attributes::mutate(&mut attrs, |m| {
                    *m = ("fill", color_to_string(&self.color)).into();
                });
                Attributes::mutate(&mut attrs, |m| {
                    *m = opacity_attr_modifier("fill-opacity", &self.color);
                });
            }
        });
    }

    forward_view_lifecycle!(child, State, Action);
}

const STROKE_MODIFIER_COUNT: usize = 8;

fn stroke_geometry(stroke: &kurbo::Stroke) -> [AttributeModifier; 5] {
    let cap = match stroke.start_cap {
        kurbo::Cap::Butt => "butt",
        kurbo::Cap::Square => "square",
        kurbo::Cap::Round => "round",
    };
    let join = match stroke.join {
        kurbo::Join::Bevel => "bevel",
        kurbo::Join::Miter => "miter",
        kurbo::Join::Round => "round",
    };
    [
        ("stroke-dashoffset", stroke.dash_offset).into(),
        ("stroke-width", stroke.width).into(),
        ("stroke-linecap", cap).into(),
        ("stroke-linejoin", join).into(),
        ("stroke-miterlimit", stroke.miter_limit).into(),
    ]
}

fn push_stroke_modifiers(
    mut attrs: Modifier<'_, Attributes>,
    stroke: &kurbo::Stroke,
    color: &Color,
) {
    let dash_pattern =
        (!stroke.dash_pattern.is_empty()).then(|| join(&mut stroke.dash_pattern.iter(), " "));
    Attributes::push(&mut attrs, ("stroke", color_to_string(color)));
    Attributes::push(&mut attrs, opacity_attr_modifier("stroke-opacity", color));
    Attributes::push(&mut attrs, ("stroke-dasharray", dash_pattern));

    for modifier in stroke_geometry(stroke) {
        Attributes::push(&mut attrs, modifier);
    }
}

// This function is not inlined to avoid unnecessary monomorphization, which may result in a bigger binary.
fn update_stroke_modifiers(
    mut attrs: Modifier<'_, Attributes>,
    prev_stroke: &kurbo::Stroke,
    next_stroke: &kurbo::Stroke,
    prev_color: &Color,
    next_color: &Color,
) {
    if attrs.flags.was_created() {
        push_stroke_modifiers(attrs, next_stroke, next_color);
    } else {
        if next_color == prev_color {
            Attributes::skip(&mut attrs, 2);
        } else {
            Attributes::mutate(&mut attrs, |m| {
                *m = ("stroke", color_to_string(next_color)).into();
            });
            Attributes::mutate(&mut attrs, |m| {
                *m = opacity_attr_modifier("stroke-opacity", next_color);
            });
        }
        if next_stroke.dash_pattern == prev_stroke.dash_pattern {
            Attributes::skip(&mut attrs, 1);
        } else {
            let dash_pattern = (!next_stroke.dash_pattern.is_empty())
                .then(|| join(&mut next_stroke.dash_pattern.iter(), " "));
            Attributes::mutate(&mut attrs, |m| {
                *m = ("stroke-dasharray", dash_pattern).into();
            });
        }
        for (prev, next) in stroke_geometry(prev_stroke)
            .iter()
            .zip(stroke_geometry(next_stroke).iter())
        {
            Attributes::update(&mut attrs, prev, next);
        }
    }
}

impl<V, State, Action> ViewMarker for Stroke<V, State, Action> {}
impl<State, Action, V> View<State, Action, ViewCtx> for Stroke<V, State, Action>
where
    State: 'static,
    Action: 'static,
    V: DomView<State, Action, Element: WithModifier<Attributes>>,
    for<'a> <V::Element as ViewElement>::Mut<'a>: WithModifier<Attributes>,
{
    type ViewState = V::ViewState;
    type Element = V::Element;

    fn build(&self, ctx: &mut ViewCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        let (mut element, state) = ctx
            .with_size_hint::<Attributes, _>(STROKE_MODIFIER_COUNT, |ctx| {
                self.child.build(ctx, app_state)
            });
        push_stroke_modifiers(element.modifier(), &self.style.0, &self.color);
        (element, state)
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        Attributes::rebuild(element, STROKE_MODIFIER_COUNT, |mut element| {
            self.child.rebuild(
                &prev.child,
                view_state,
                ctx,
                element.reborrow_mut(),
                app_state,
            );
            update_stroke_modifiers(
                element.modifier(),
                &prev.style.0,
                &self.style.0,
                &prev.color,
                &self.color,
            );
        });
    }

    forward_view_lifecycle!(child, State, Action);
}

#[cfg(test)]
mod tests {
    use peniko::Color;
    use peniko::color::palette;

    use super::color_to_string;

    #[test]
    fn solid_color_to_string() {
        let transparent: Color = palette::css::TRANSPARENT.into();
        assert_eq!(color_to_string(&transparent), "none");

        let red: Color = palette::css::RED.into();
        assert_eq!(color_to_string(&red), "#ff0000");

        let lime: Color = palette::css::LIME.with_alpha(0.5).into();
        assert_eq!(color_to_string(&lime), "#00ff00");
    }
}
