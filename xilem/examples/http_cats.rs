// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! An example demonstrating the use of Async web requests in Xilem to access the <https://http.cat/> API.
//! This also demonstrates image loading.

use std::sync::Arc;

use masonry::layout::{AsUnit, Length, UnitPoint};
use masonry::properties::{LineBreaking, Padding};
use tokio::sync::mpsc::Sender;
use winit::dpi::LogicalSize;
use xilem::RunError;
use xilem::core::fork;
use xilem::core::one_of::OneOf3;
use xilem::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use xilem::style::Style as _;
use xilem::view::{
    FlexExt, FlexSpacer, ZStackExt, flex_col, flex_row, image, inline_prose, portal, prose,
    sized_box, spinner, split, text_button, worker, zstack,
};
use xilem::{EventLoop, EventLoopBuilder, TextAlign, WidgetView, WindowOptions, Xilem, palette};

/// The main state of the application.
struct HttpCats {
    statuses: Vec<Status>,
    // The currently active (http status) code.
    selected_code: Option<u32>,
    /// Send a status code to download the image for it.
    download_sender: Option<Sender<u32>>,
}

#[derive(Debug)]
struct Status {
    code: u32,
    message: &'static str,
    image: ImageState,
}

#[derive(Debug)]
/// The state of the download of an image from a URL
enum ImageState {
    NotRequested,
    Pending,
    Error(String),
    Available(ImageData),
}

impl HttpCats {
    fn view(&mut self) -> impl WidgetView<Self> + use<> {
        let left_column = flex_col((
            prose("Status"),
            self.statuses
                .iter()
                .map(Status::list_view)
                .collect::<Vec<_>>(),
        ))
        .padding(Padding::left(5.px()));

        let info_area = self.selected_code.map_or_else(
            || {
                OneOf3::C(
                    prose("No selection yet made. Select an item from the sidebar to continue.")
                        .text_alignment(TextAlign::Center),
                )
            },
            |selected_code| {
                self.statuses
                    .iter()
                    .find(|it| it.code == selected_code)
                    .map_or_else(
                        || {
                            OneOf3::B(
                                prose(format!(
                                    "Status code {selected_code} selected, but this was not found."
                                ))
                                .text_alignment(TextAlign::Center)
                                .text_color(palette::css::YELLOW),
                            )
                        },
                        |status| OneOf3::A(status.details_view()),
                    )
            },
        );

        // TODO: Should `web_image` be a built-in component?

        fork(
            flex_col((
                // Add padding to the top for Android. Still a horrible hack
                FlexSpacer::Fixed(40.px()),
                split(portal(left_column), info_area)
                    .split_point(0.4)
                    .flex(1.),
            )),
            worker(
                std::num::NonZeroUsize::MIN.saturating_add(31),
                |proxy, rx| {
                    xilem::view::process_requests(
                        proxy,
                        rx,
                        std::num::NonZeroUsize::MIN.saturating_add(7),
                        |code| async move {
                            let url = format!("https://http.cat/{code}");
                            let result = image_from_url(&url).await.map_err(|err| err.to_string());
                            (code, result)
                        },
                    )
                },
                |state: &mut Self, sender| {
                    state.download_sender = Some(sender);
                },
                |state: &mut Self, (code, result): (u32, Result<ImageData, String>)| {
                    if let Some(status) = state.statuses.iter_mut().find(|it| it.code == code) {
                        status.image = match result {
                            Ok(image) => ImageState::Available(image),
                            Err(err) => ImageState::Error(err),
                        };
                    } else {
                        // TODO: Error handling?
                    }
                },
            ),
        )
    }
}

/// Load the [`ImageData`] from the given url.
///
/// N.B. This is functionality shared with Placehero and `virtual_cats`.
async fn image_from_url(url: &str) -> anyhow::Result<ImageData> {
    let response = reqwest::get(url).await?.error_for_status()?;
    let bytes = response.bytes().await?;
    let image = image::load_from_memory(&bytes)?.into_rgba8();
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

impl Status {
    fn list_view(&self) -> impl WidgetView<HttpCats> + use<> {
        let code = self.code;
        flex_row((
            // TODO: Reduce allocations here?
            inline_prose(self.code.to_string()),
            inline_prose(self.message),
            FlexSpacer::Flex(1.),
            // TODO: Spinner if image pending?
            // TODO: Tick if image loaded?
            text_button("Select", move |state: &mut HttpCats| {
                let Some(status) = state.statuses.iter_mut().find(|it| it.code == code) else {
                    return;
                };
                if matches!(
                    status.image,
                    ImageState::NotRequested | ImageState::Error(_)
                ) {
                    let requested = state
                        .download_sender
                        .as_ref()
                        .is_some_and(|sender| sender.try_send(code).is_ok());
                    status.image = if requested {
                        ImageState::Pending
                    } else {
                        ImageState::Error("Download worker unavailable".to_owned())
                    };
                }

                state.selected_code = Some(code);
            }),
            FlexSpacer::Fixed(Length::px(masonry::theme::SCROLLBAR_WIDTH)),
        ))
    }

    fn details_view(&self) -> impl WidgetView<HttpCats> + use<> {
        let image = match &self.image {
            ImageState::NotRequested => OneOf3::A(
                prose("Failed to start fetching image. This is a bug!")
                    .text_alignment(TextAlign::Center),
            ),
            ImageState::Error(error) => OneOf3::A(
                prose(format!("Could not load image: {error}")).text_alignment(TextAlign::Center),
            ),
            ImageState::Pending => OneOf3::B(spinner().dims(80.px())),
            // TODO: Alt text?
            ImageState::Available(image_data) => {
                let attribution = sized_box(
                    sized_box(
                        prose("Copyright ©️ https://http.cat")
                            .line_break_mode(LineBreaking::Clip)
                            .text_alignment(TextAlign::End),
                    )
                    .padding(4.px())
                    .corner_radius(4.px())
                    .background_color(palette::css::BLACK.multiply_alpha(0.5)),
                )
                .padding(Padding {
                    left: 0.px(),
                    right: 42.px(),
                    top: 30.px(),
                    bottom: 0.px(),
                });
                OneOf3::C(zstack((
                    image(image_data.clone()),
                    attribution.alignment(UnitPoint::TOP_RIGHT),
                )))
            }
        };
        flex_col((
            prose(format!("HTTP Status Code: {}", self.code)).text_alignment(TextAlign::Center),
            prose(self.message)
                .text_size(20.)
                .text_alignment(TextAlign::Center),
            FlexSpacer::Fixed(10.px()),
            image.flex(1.),
        ))
        .main_axis_alignment(xilem::view::MainAxisAlignment::Start)
    }
}

pub(crate) fn run(event_loop: EventLoopBuilder) -> Result<(), RunError> {
    let data = HttpCats {
        statuses: Status::parse_file(),
        selected_code: None,
        download_sender: None,
    };

    let app = Xilem::new_simple(
        data,
        HttpCats::view,
        WindowOptions::new("HTTP cats").with_min_inner_size(LogicalSize::new(200., 200.)),
    );

    app.run_in(event_loop)
}

impl Status {
    /// Parse the supported HTTP cats.
    fn parse_file() -> Vec<Self> {
        let mut lines = STATUS_CODES_CSV.lines();
        let first_line = lines.next();
        if first_line != Some("code,message") {
            tracing::error!("Invalid HTTP status CSV header");
            return Vec::new();
        }
        lines.filter_map(Self::parse_single).collect()
    }

    fn parse_single(line: &'static str) -> Option<Self> {
        let (code, message) = line.split_once(',')?;
        Some(Self {
            code: code.parse().ok()?,
            message: message.trim(),
            image: ImageState::NotRequested,
        })
    }
}

/// The status codes supported by <https://http.cat>, used under the MIT license.
/// Full details can be found in `xilem/resources/data/http_cats_status/README.md` from
/// the workspace root.
const STATUS_CODES_CSV: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/data/http_cats_status/status.csv",
));

// Boilerplate code: Identical across all applications which support Android

fn main() -> Result<(), RunError> {
    run(EventLoop::with_user_event())
}
