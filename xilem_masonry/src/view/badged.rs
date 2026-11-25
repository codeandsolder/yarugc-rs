// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::marker::PhantomData;

use masonry::kurbo::Vec2;
use masonry::widgets::{self, BadgePlacement};

use super::optional_child::OptionalChildState;
use crate::core::{MessageCtx, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker};
use crate::{Pod, ViewCtx, WidgetView};

/// Decorate `content` by overlaying a badge widget on top of it.
pub fn badged<State, Action, Content, BadgeV>(
    content: Content,
    badge: BadgeV,
) -> Badged<Content, BadgeV, State, Action>
where
    State: 'static,
    Action: 'static,
    Content: WidgetView<State, Action>,
    BadgeV: WidgetView<State, Action>,
{
    Badged {
        content,
        badge: Some(badge),
        placement: BadgePlacement::TopRight,
        offset: Vec2::ZERO,
        phantom: PhantomData,
    }
}

/// Like [`badged`], but the badge may be absent (for example when `count == 0`).
pub fn badged_optional<State, Action, Content, BadgeV>(
    content: Content,
    badge: Option<BadgeV>,
) -> Badged<Content, BadgeV, State, Action>
where
    State: 'static,
    Action: 'static,
    Content: WidgetView<State, Action>,
    BadgeV: WidgetView<State, Action>,
{
    Badged {
        content,
        badge,
        placement: BadgePlacement::TopRight,
        offset: Vec2::ZERO,
        phantom: PhantomData,
    }
}

/// The [`View`] created by [`badged`] / [`badged_optional`].
#[must_use = "View values do nothing unless provided to Xilem."]
pub struct Badged<Content, BadgeV, State, Action = ()> {
    content: Content,
    badge: Option<BadgeV>,
    placement: BadgePlacement,
    offset: Vec2,
    phantom: PhantomData<fn() -> (State, Action)>,
}

impl<Content, BadgeV, State, Action> Badged<Content, BadgeV, State, Action> {
    /// Sets the badge placement relative to the content.
    pub const fn placement(mut self, placement: BadgePlacement) -> Self {
        self.placement = placement;
        self
    }

    /// Sets an additional badge offset.
    pub const fn offset(mut self, offset: Vec2) -> Self {
        self.offset = offset;
        self
    }
}

const BADGED_CONTENT_VIEW_ID: ViewId = ViewId::new(0x6f3b_2dfd);
const BADGED_BADGE_VIEW_ID: ViewId = ViewId::new(0x0c3f_7b7a);

mod hidden {
    use super::OptionalChildState;
    #[doc(hidden)]
    #[expect(
        unnameable_types,
        reason = "Implementation detail, public because of trait visibility rules"
    )]
    pub struct BadgedState<ContentState, BadgeState> {
        pub(crate) content: ContentState,
        pub(crate) badge: OptionalChildState<BadgeState>,
    }
}

use hidden::BadgedState;

impl<Content, BadgeV, State, Action> ViewMarker for Badged<Content, BadgeV, State, Action> {}
impl<Content, BadgeV, State, Action> View<State, Action, ViewCtx>
    for Badged<Content, BadgeV, State, Action>
where
    State: 'static,
    Action: 'static,
    Content: WidgetView<State, Action>,
    BadgeV: WidgetView<State, Action>,
{
    type Element = Pod<widgets::Badged>;
    type ViewState = BadgedState<Content::ViewState, BadgeV::ViewState>;

    fn build(&self, ctx: &mut ViewCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        let (content_el, content_state) = ctx.with_id(BADGED_CONTENT_VIEW_ID, |ctx| {
            View::<State, Action, _>::build(&self.content, ctx, app_state)
        });
        let (badge_new, badge_state) = ctx.with_id(BADGED_BADGE_VIEW_ID, |ctx| {
            OptionalChildState::build(self.badge.as_ref(), ctx, app_state)
        });

        let widget = widgets::Badged::new_optional(content_el.new_widget, badge_new)
            .with_badge_placement(self.placement)
            .with_badge_offset(self.offset);
        (
            Pod::new(widget),
            BadgedState {
                content: content_state,
                badge: badge_state,
            },
        )
    }

    fn rebuild(
        &self,
        prev: &Self,
        BadgedState { content, badge }: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        if prev.placement != self.placement {
            widgets::Badged::set_badge_placement(&mut element, self.placement);
        }
        if prev.offset != self.offset {
            widgets::Badged::set_badge_offset(&mut element, self.offset);
        }

        ctx.with_id(BADGED_CONTENT_VIEW_ID, |ctx| {
            let Some(child_element) = widgets::Badged::content_mut(&mut element) else {
                return;
            };
            View::<State, Action, _>::rebuild(
                &self.content,
                &prev.content,
                content,
                ctx,
                child_element,
                app_state,
            );
        });

        ctx.with_id(BADGED_BADGE_VIEW_ID, |ctx| {
            badge.rebuild(
                self.badge.as_ref(),
                prev.badge.as_ref(),
                ctx,
                &mut element,
                app_state,
            );
        });
    }

    fn teardown(
        &self,
        BadgedState { content, badge }: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
    ) {
        ctx.with_id(BADGED_CONTENT_VIEW_ID, |ctx| {
            let Some(child_element) = widgets::Badged::content_mut(&mut element) else {
                return;
            };
            View::<State, Action, _>::teardown(&self.content, content, ctx, child_element);
        });

        ctx.with_id(BADGED_BADGE_VIEW_ID, |ctx| {
            badge.teardown(self.badge.as_ref(), ctx, &mut element);
        });
    }

    fn message(
        &self,
        BadgedState { content, badge }: &mut Self::ViewState,
        message: &mut MessageCtx,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        match message.take_first() {
            Some(BADGED_CONTENT_VIEW_ID) => {
                let Some(child_element) = widgets::Badged::content_mut(&mut element) else {
                    return MessageResult::Stale;
                };
                self.content
                    .message(content, message, child_element, app_state)
            }
            Some(BADGED_BADGE_VIEW_ID) => {
                badge.message(self.badge.as_ref(), message, &mut element, app_state)
            }
            None => {
                tracing::error!(
                    ?message,
                    "Message arrived in Badged::message, but Badged doesn't consume any messages, this is a bug"
                );
                MessageResult::Stale
            }
            Some(_) => {
                tracing::error!(?message, "Unexpected view id for Badged::message");
                MessageResult::Stale
            }
        }
    }
}
