// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::marker::PhantomData;

use masonry::kurbo::{Axis, Cap};
use masonry::layout::Length;
use masonry::widgets::{self, DashFit, Placement};
use smallvec::SmallVec;

use super::optional_child::OptionalChildState;
use crate::core::{MessageCtx, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker};
use crate::view::Spinner;
use crate::{Pod, ViewCtx, WidgetView};

/// Creates a new [`Divider`] parallel with the given `axis`.
///
/// Optionally the divider can also display `content`.
pub fn divider<State: 'static, Action, V: WidgetView<State, Action>>(
    axis: Axis,
    content: impl Into<Option<V>>,
) -> Divider<State, Action, V> {
    Divider {
        axis,
        thickness: None,
        dash_fit: DashFit::default(),
        dash_pattern: SmallVec::default(),
        start_cap: Cap::Butt,
        end_cap: Cap::Butt,
        placement: Placement::default(),
        pad: masonry::layout::length!(5.),
        content: content.into(),

        phantom: PhantomData,
    }
}

/// Creates a new horizontal [`Divider`] with no content.
#[must_use = "Use the returned value."]
pub fn divider_h<State: 'static, Action>() -> Divider<State, Action> {
    divider(Axis::Horizontal, None)
}

/// Creates a new vertical [`Divider`] with no content.
#[must_use = "Use the returned value."]
pub fn divider_v<State: 'static, Action>() -> Divider<State, Action> {
    divider(Axis::Vertical, None)
}

/// A line to divide your content.
///
/// By default it is a thin solid line. A dash pattern can be configured with [`dash_pattern`].
///
/// [`dash_pattern`]: Self::dash_pattern
// Using Spinner as a dummy default so Divider can be constructed without a manual content type.
pub struct Divider<State, Action, V = Spinner> {
    axis: Axis,
    /// No set thickness means hairline - 1 device pixel.
    thickness: Option<Length>,
    dash_fit: DashFit,
    dash_pattern: SmallVec<[Length; 2]>,
    start_cap: Cap,
    end_cap: Cap,
    placement: Placement,
    pad: Length,
    content: Option<V>,

    phantom: PhantomData<fn(State) -> Action>,
}

impl<State: 'static, Action, V: WidgetView<State, Action>> Divider<State, Action, V> {
    masonry::divider_style_builders!();
}

/// Use a distinctive number here, to be able to catch bugs.
/// In case the generational-id view path in `View::Message` leads to the wrong view.
/// This is a randomly generated 32 bit number - 2799496121 in decimal.
const DIVIDER_CONTENT_VIEW_ID: ViewId = ViewId::new(0xA6DC_EBB9);

impl<State, Action, V> ViewMarker for Divider<State, Action, V> {}
impl<State: 'static, Action: 'static, V: WidgetView<State, Action>> View<State, Action, ViewCtx>
    for Divider<State, Action, V>
{
    type Element = Pod<widgets::Divider>;
    type ViewState = OptionalChildState<V::ViewState>;

    fn build(&self, ctx: &mut ViewCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        let mut divider = widgets::Divider::new(self.axis)
            .dash_fit(self.dash_fit)
            .dash_pattern(&self.dash_pattern)
            .start_cap(self.start_cap)
            .end_cap(self.end_cap)
            .placement(self.placement)
            .pad(self.pad);
        if let Some(thickness) = self.thickness {
            divider = divider.thickness(thickness);
        } else {
            divider = divider.hairline();
        }
        let (child, view_state) = ctx.with_id(DIVIDER_CONTENT_VIEW_ID, |ctx| {
            OptionalChildState::build(self.content.as_ref(), ctx, app_state)
        });
        if let Some(child) = child {
            divider = divider.content(child);
        }
        (Pod::new(divider), view_state)
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        if prev.axis != self.axis {
            widgets::Divider::set_direction(&mut element, self.axis);
        }
        if prev.thickness != self.thickness {
            if let Some(thickness) = self.thickness {
                widgets::Divider::set_thickness(&mut element, thickness);
            } else {
                widgets::Divider::set_hairline(&mut element);
            }
        }
        if prev.dash_fit != self.dash_fit {
            widgets::Divider::set_dash_fit(&mut element, self.dash_fit);
        }
        if prev.dash_pattern != self.dash_pattern {
            widgets::Divider::set_dash_pattern(&mut element, &self.dash_pattern);
        }
        if prev.start_cap != self.start_cap {
            widgets::Divider::set_start_cap(&mut element, self.start_cap);
        }
        if prev.end_cap != self.end_cap {
            widgets::Divider::set_end_cap(&mut element, self.end_cap);
        }
        if prev.placement != self.placement {
            widgets::Divider::set_placement(&mut element, self.placement);
        }
        if prev.pad != self.pad {
            widgets::Divider::set_pad(&mut element, self.pad);
        }
        ctx.with_id(DIVIDER_CONTENT_VIEW_ID, |ctx| {
            view_state.rebuild(
                self.content.as_ref(),
                prev.content.as_ref(),
                ctx,
                &mut element,
                app_state,
            );
        });
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
    ) {
        ctx.with_id(DIVIDER_CONTENT_VIEW_ID, |ctx| {
            view_state.teardown(self.content.as_ref(), ctx, &mut element);
        });
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageCtx,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        match message.take_first() {
            Some(DIVIDER_CONTENT_VIEW_ID) => {
                view_state.message(self.content.as_ref(), message, &mut element, app_state)
            }
            None => {
                tracing::error!(
                    ?message,
                    "Message arrived for Divider, but Divider doesn't consume any messages, this is a bug."
                );
                MessageResult::Stale
            }
            _ => {
                tracing::warn!("Got unexpected id path in Divider::message");
                MessageResult::Stale
            }
        }
    }
}
