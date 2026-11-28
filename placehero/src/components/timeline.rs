// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::mastodon::{Account, Status};
use xilem::core::one_of::{OneOf, OneOf3};
use xilem::core::{NoElement, View};
use xilem::masonry::core::ArcStr;
use xilem::masonry::layout::AsUnit;
use xilem::palette::css;
use xilem::style::{Padding, Style};
use xilem::tokio::sync::mpsc::{Receiver, Sender};
use xilem::view::{
    MainAxisAlignment, flex_col, flex_row, prose, sized_box, spinner, virtual_scroll, worker_raw,
};
use xilem::{TextAlign, ViewCtx, WidgetView};

use super::base_status;
use crate::Mastodon;
use crate::actions::Navigation;

const BUFFER: usize = 3;

struct TimelineContinuationRequest {
    generation: u64,
    user_id: ArcStr,
    /// The "max id" to send with the request.
    max_id: Option<String>,
}

/// The state needed to load and display a single timeline (i.e. a sequence of unrelated posts)
#[derive(Default)]
pub(crate) struct Timeline {
    /// The currently loaded statuses in the timeline.
    statuses: Vec<Arc<Status>>,
    generation: u64,
    /// Whether we're currently expecting a response.
    pending_id: bool,
    /// Whether we've reached the end of this timeline
    at_end: bool,
    error: Option<String>,
    /// The sender for newly requested items.
    requests: Option<Sender<TimelineContinuationRequest>>,
    // We can't cache here due to Avatars not being cache busting ready.
    // /// Cached views for each status, as their input won't change.
    // cached_statuses: HashMap<i64, Arc<AnyWidgetView<Timeline, Navigation>>>,
    // TODO: Generalise to e.g. the explore page.
    user_id: ArcStr,
}

impl Timeline {
    pub(crate) fn set_account(&mut self, account: Account) {
        self.generation = self.generation.wrapping_add(1);
        self.statuses.clear();
        self.pending_id = false;
        self.user_id = account.id.into();
        self.at_end = false;
        self.error = None;
    }

    pub(crate) fn has_account(&self) -> bool {
        !self.user_id.is_empty()
    }

    pub(crate) fn view(&self) -> impl WidgetView<Self, Navigation> + use<> {
        virtual_scroll(
            // Show the statuses which have been loaded
            // We currently never unload previous statuses (note that the widgets but not the remainder are)
            // +1 to allow room for the spinner.
            // Note that we also launch the very first initial request from the spinner
            self.statuses.len() + 1,
            |timeline: &mut Self, idx| {
                // If we're "close" to the last downloaded item.
                if idx.saturating_add(BUFFER) >= timeline.statuses.len()
                        // And don't already have a request in flight
                        && !timeline.pending_id
                        && !timeline.at_end
                        && timeline.error.is_none()
                {
                    let request = TimelineContinuationRequest {
                        generation: timeline.generation,
                        user_id: timeline.user_id.clone(),
                        max_id: timeline.statuses.last().map(|status| status.id.clone()),
                    };
                    if let Some(sender) = timeline.requests.as_ref() {
                        match sender.try_send(request) {
                            Ok(()) => timeline.pending_id = true,
                            Err(xilem::tokio::sync::mpsc::error::TrySendError::Full(_)) => {}
                            Err(xilem::tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                timeline.error = Some("Timeline worker stopped".to_owned());
                            }
                        }
                    }
                }
                if idx >= timeline.statuses.len() {
                    if let Some(error) = timeline.error.as_ref() {
                        OneOf3::A(prose(error.clone()).text_alignment(TextAlign::Center))
                    } else if timeline.at_end {
                        OneOf3::A(
                            prose("End of timeline.".to_owned()).text_alignment(TextAlign::Center),
                        )
                    } else {
                        OneOf::B(
                            flex_row(spinner().dims(50.px()))
                                .main_axis_alignment(MainAxisAlignment::Center),
                        )
                    }
                } else {
                    // We would like to cache the status, but this is currently not supported due to `Avatars`
                    // (and everything environment-system) not being properly rebuild aware.
                    // This is planned, but the infra isn't in place yet.
                    // match timeline.cached_statuses.entry(idx) {
                    //     hash_map::Entry::Occupied(occupied_entry) => occupied_entry.get().clone(),
                    //     hash_map::Entry::Vacant(vacant_entry) => vacant_entry
                    //         .insert(Arc::new(timeline_status(&timeline.statuses[local_idx])))
                    //         .clone(),
                    // }
                    OneOf::C(timeline_status(&timeline.statuses[idx]))
                }
            },
        )
    }

    // The worker is mounted at the application root, not in the currently
    // visible timeline view. Changing accounts or hiding the view must not
    // lose its sender or capture the identity of an obsolete account.
    pub(crate) fn worker(
        mastodon: Mastodon,
    ) -> impl View<Self, (), ViewCtx, Element = NoElement> + use<> {
        worker_raw(
            std::num::NonZeroUsize::MIN,
            move |proxy, mut recv: Receiver<TimelineContinuationRequest>| {
                let mastodon = mastodon.clone();
                async move {
                    // For every request we receive (there should only ever be one
                    // at a time, but the queue is bounded to one request)
                    // we load the requested statuses
                    while let Some(next) = recv.recv().await {
                        let result = mastodon
                            .statuses(&next.user_id, next.max_id.as_deref())
                            .await;
                        drop(proxy.message((next.generation, result)));
                    }
                }
            },
            |timeline: &mut Self, sender| {
                // `worker` creates a channel pair for us; we need to keep our own track
                // of the sender
                timeline.requests = Some(sender);
            },
            |timeline: &mut Self, (generation, resp)| {
                if timeline.generation != generation {
                    return;
                }
                match resp {
                    Ok(mut statuses) => {
                        // If we get an empty response, that means we're at the end of
                        // the timeline (...probably, this is undocumented)
                        if statuses.is_empty() {
                            timeline.at_end = true;
                        }
                        timeline.pending_id = false;
                        timeline.statuses.append(&mut statuses);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "Could not load timeline");
                        timeline.pending_id = false;
                        timeline.error = Some(format!("Could not load timeline: {error}"));
                    }
                }
            },
        )
    }
}

/// The component for a single status in a [`Timeline`].
///
/// These statuses are currently not currently rendered with a reply indicator, media, etc.
/// This is planned. Reblogged statuses are also not currently handled correctly.
///
/// They are rendered with a surrounding padded box.
// TODO: Work out how much of this component can be reused in a reply timeline.
// I think you want the same thing, but without the box, and without any "this is a reply" indicator.
// It also wouldn't need to handle reblogs (the API doesn't provide any way to make a reply status which is a reblog).
// N.b. API wise, there's no reason that you can't reply to a "reblog" status. TODO: Confirm this
pub(crate) fn timeline_status(
    status: &Arc<Status>,
) -> impl WidgetView<Timeline, Navigation> + use<> {
    let (info_line, primary_status) = status.reblog.as_ref().map_or((None, status), |reblog| {
        (
            Some(prose(format!("🔄 {} boosted", status.account.display_name))),
            reblog,
        )
    });
    // Use a wrapping sized box for margin
    sized_box(
        sized_box(flex_col((info_line, base_status(primary_status))))
            .border(css::WHITE, 2.px())
            .padding(10.px())
            .corner_radius(5.px()),
    )
    .padding(Padding::from_vh(5.px(), 4.px()))
}
