// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use masonry::widgets;

use crate::core::{MessageCtx, MessageResult, Mut, View, ViewMarker};
use crate::{Pod, ViewCtx};

/// A view which displays a progress bar.
///
/// This can be for showing progress of a task or a download.
pub const fn progress_bar(progress: Option<f64>) -> ProgressBar {
    ProgressBar { progress }
}

/// The [`View`] created by [`progress_bar`].
#[must_use = "View values do nothing unless provided to Xilem."]
pub struct ProgressBar {
    progress: Option<f64>,
}

impl ViewMarker for ProgressBar {}
impl<State: 'static, Action> View<State, Action, ViewCtx> for ProgressBar {
    type Element = Pod<widgets::ProgressBar>;
    type ViewState = ();

    fn build(&self, _ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        (Pod::new(widgets::ProgressBar::new(self.progress)), ())
    }

    fn rebuild(
        &self,
        _: &Self,
        (): &mut Self::ViewState,
        _ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        _: &mut State,
    ) {
        widgets::ProgressBar::set_progress(&mut element, self.progress);
    }

    fn teardown(&self, (): &mut Self::ViewState, _: &mut ViewCtx, _: Mut<'_, Self::Element>) {}

    fn message(
        &self,
        (): &mut Self::ViewState,
        message: &mut MessageCtx,
        _element: Mut<'_, Self::Element>,
        _app_state: &mut State,
    ) -> MessageResult<Action> {
        tracing::error!(
            ?message,
            "Message arrived in ProgressBar::message, but ProgressBar doesn't consume any messages, this is a bug"
        );
        MessageResult::Stale
    }
}
