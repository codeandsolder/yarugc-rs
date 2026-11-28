// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::sync::Arc;

use xilem::core::one_of::Either;
use xilem::core::{
    MessageResult, NoElement, Resource, View, fork, map_message_result, on_action_with_context,
    provides, with_context,
};
use xilem::masonry::layout::AsUnit;
use xilem::masonry::peniko::{ImageAlphaType, ImageData};
use xilem::palette::css;
use xilem::style::{Gradient, Style as _};
use xilem::tokio::sync::mpsc::{Receiver, Sender};
use xilem::view::{env_worker, flex_col, image, process_requests, sized_box, spinner};
use xilem::{Blob, ImageFormat, ViewCtx, WidgetView};

#[derive(Debug)]
struct AvatarRequest {
    client: reqwest::Client,
    avatar_url: String,
}

#[derive(Debug)]
struct AvatarResponse {
    url: String,
    result: Result<ImageData, String>,
}

#[derive(Debug)]
enum AvatarState {
    Pending,
    Ready(ImageData),
    Failed,
}

#[derive(Debug)]
pub(crate) struct Avatars {
    client: reqwest::Client,
    icons: HashMap<String, AvatarState>,
    requester: Option<Sender<AvatarRequest>>,
    // TODO: lru: ...
}

impl Resource for Avatars {}

impl Avatars {
    /// Get the avatar view/component for the given url, as a 50x50 pixel box.
    ///
    /// This will fetch the image data from the URL, and cache it.
    /// If the image hasn't yet loaded, will show a placeholder,
    ///
    /// Requires that this View is within a [`Self::provide`] call.
    // TODO: ArcStr for URL?
    pub(crate) fn avatar<State: 'static, Action: 'static>(
        url: String,
    ) -> impl WidgetView<State, Action> + use<State, Action> {
        with_context(move |resource: Option<&mut Self>, _| {
            let width = 50.px();
            let height = 50.px();
            let pending = if let Some(this) = resource {
                if let Some(AvatarState::Ready(image_)) = this.icons.get(&url) {
                    return Either::A(image(image_.clone()).dims((width, height)));
                }
                if !this.icons.contains_key(&url)
                    && let Some(requester) = this.requester.as_ref()
                    && requester
                        .try_send(AvatarRequest {
                            client: this.client.clone(),
                            avatar_url: url.clone(),
                        })
                        .is_ok()
                {
                    this.icons.insert(url.clone(), AvatarState::Pending);
                }
                !matches!(this.icons.get(&url), Some(AvatarState::Failed))
            } else {
                true
            };
            Either::B(
                sized_box(flex_col(pending.then(spinner)))
                    .dims((width, height))
                    .background_gradient(
                        Gradient::new_linear(
                            // down-right
                            const { -45_f64.to_radians() },
                        )
                        .with_stops([css::YELLOW, css::LIME]),
                    )
                    .padding(4.px()),
            )
        })
    }

    /// Provide support for Mastodon Avatar display to the `child` view.
    pub(crate) fn provide<State, Action, Child>(
        client: reqwest::Client,
        child: Child,
    ) -> impl WidgetView<State, Action, Element = Child::Element>
    where
        Child: WidgetView<State, Action>,
        State: 'static,
        Action: 'static,
    {
        provides(
            move |_| Self {
                client: client.clone(),
                icons: HashMap::default(),
                requester: None,
            },
            fork(child, Self::worker()),
        )
    }

    fn worker<State, Action>()
    -> impl View<State, Action, ViewCtx, Element = NoElement> + use<State, Action>
    where
        State: 'static,
        Action: 'static,
    {
        map_message_result(
            on_action_with_context(
                |_, this: &mut Self, response| {
                    let state = match response.result {
                        Ok(image) => AvatarState::Ready(image),
                        Err(error) => {
                            tracing::warn!(%error, "Avatar download failed");
                            AvatarState::Failed
                        }
                    };
                    let ret = this.icons.insert(response.url, state);
                    if !matches!(ret, Some(AvatarState::Pending)) {
                        tracing::warn!("Potentially loaded or tried to load same avatar twice.");
                    }
                },
                env_worker(
                    std::num::NonZeroUsize::MIN.saturating_add(63),
                    |proxy, rx: Receiver<AvatarRequest>| {
                        process_requests(
                            proxy,
                            rx,
                            std::num::NonZeroUsize::MIN.saturating_add(7),
                            |request| async move {
                                let url = request.avatar_url;
                                let result = image_from_url(&request.client, &url)
                                    .await
                                    .map_err(|err| err.to_string());
                                AvatarResponse { url, result }
                            },
                        )
                    },
                    |_, this: &mut Self, tx| {
                        if this.requester.is_some() {
                            tracing::warn!(
                                "Unexpectedly got a second worker for requesting avatars."
                            );
                        }
                        this.requester = Some(tx);
                    },
                    |_, response| response,
                ),
            ),
            // Convert to the user-defined action type by ignoring any action we actually submit.
            |_, action| match action {
                MessageResult::Action(()) | MessageResult::RequestRebuild => {
                    MessageResult::RequestRebuild
                }
                MessageResult::Nop => MessageResult::Nop,
                MessageResult::Stale => MessageResult::Stale,
            },
        )
    }
}

/// Load the [`ImageData`] from the given url.
///
/// N.B. This is functionality shared with `http_cats`
pub(crate) async fn image_from_url(
    client: &reqwest::Client,
    url: &str,
) -> anyhow::Result<ImageData> {
    let response = client.get(url).send().await?.error_for_status()?;
    let bytes = crate::mastodon::read_body(response, 8 * 1024 * 1024).await?;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode()?.thumbnail(256, 256).into_rgba8();
    let width = image.width();
    let height = image.height();
    let data = image.into_vec();
    Ok(ImageData {
        data: Blob::new(Arc::new(data)),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    })
}
