// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! The bitmap image widget.

use masonry::core::ArcStr;
use masonry::peniko::ImageBrush;
use masonry::widgets;

use crate::core::{MessageCtx, MessageResult, Mut, View, ViewMarker};
use crate::view::Prop;
use crate::{Pod, ViewCtx, WidgetView};

pub use masonry::properties::ObjectFit;

/// Displays the bitmap `image`.
///
/// By default, the Image will be scaled to fully fit within the container ([`ObjectFit::Contain`]).
/// To configure this, call [`fit`](Image::fit) on the returned value.
///
/// Corresponds to the [`Image`](widgets::Image) widget.
///
/// It is not currently supported to use a GPU-resident texture in this widget.
/// See [#vello > vello adding wgpu texture buffers to scene](https://xi.zulipchat.com/#narrow/channel/197075-vello/topic/vello.20adding.20wgpu.20texture.20buffers.20to.20scene/with/456486490)
/// for discussion.
pub fn image(image: impl Into<ImageBrush>) -> Image {
    Image {
        brush: image.into(),
        decorative: false,
        alt_text: None,
    }
}

/// The [`View`] created by [`image`].
///
/// See `image`'s docs for more details.
#[must_use = "View values do nothing unless provided to Xilem."]
pub struct Image {
    brush: ImageBrush,
    decorative: bool,
    alt_text: Option<ArcStr>,
}

impl Image {
    image_view_builders!(alt_text);
}
image_view_lifecycle!(
    Image,
    Image,
    brush,
    set_brush,
    |prev: &ImageBrush, next: &ImageBrush| prev != next
);
