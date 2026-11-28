// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::mastodon::{Attachment, AttachmentType};
use xilem::{
    Blob, ImageFormat, WidgetView,
    core::one_of::{OneOf, OneOf4},
    masonry::peniko::{ImageAlphaType, ImageData},
    view::{ObjectFit, flex_col, image, prose},
};

use crate::actions::Navigation;

/// Render a single media attachment for use in a status.
///
/// This currently doesn't perform any caching.
pub(crate) fn attachment<State: 'static>(
    attachment: &Attachment,
) -> impl WidgetView<State, Navigation> + use<State> {
    match attachment.r#type {
        AttachmentType::Audio => OneOf4::A(audio_attachment(attachment)),
        AttachmentType::Video | AttachmentType::Gifv => OneOf::B(video_attachment(attachment)),
        AttachmentType::Image => OneOf::C(image_attachment(attachment)),
        AttachmentType::Unknown => OneOf::D(flex_col((
            maybe_blurhash(attachment),
            prose("Unknown media"),
        ))),
    }
}

/// If the attachment has a [blurhash], create a view of it.
///
/// This view currently does not cache the blurhash, or take any other steps to
/// avoid recalculating the image.
/// We haven't ran into this being a performance issue.
fn maybe_blurhash<State: 'static>(
    attachment: &Attachment,
) -> Option<impl WidgetView<State, Navigation> + use<State>> {
    let start = Instant::now();
    let blurhash = attachment.blurhash.as_deref()?;
    // TODO: Maybe use `memoize` here? We don't at the moment because we
    // wouldn't have any way to pass the "attachment" in.
    let (width, height) = 'dimensions: {
        if let Some(meta) = &attachment.meta {
            if let Some(width) = meta.width
                && let Some(height) = meta.height
            {
                break 'dimensions (width, height);
            } else if let Some(original) = &meta.original
                && let Some(width) = original.width
                && let Some(height) = original.height
            {
                break 'dimensions (width, height);
            }
        }
        tracing::error!("Couldn't find dimensions for blurhash. Using default.");
        (960, 960)
    };
    let (blur_width, blur_height) = preview_dimensions(width, height)?;
    let result_bytes = blurhash::decode(blurhash, blur_width, blur_height, 1.0).ok()?;
    let image_data = Blob::new(Arc::new(result_bytes));
    // This image format doesn't seem to be documented by the blurhash crate, but this value seems to work.
    let image2 = ImageData {
        data: image_data,
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: blur_width,
        height: blur_height,
    };
    let took = start.elapsed();
    if took > Duration::from_millis(5) {
        tracing::info!("Calculating a blurhash (size {blur_width}x{blur_height}) took {took:?}.");
    }

    // Retain the aspect ratio, and don't go bigger than the image's actual dimensions.
    // Prefer to fill up the width rather than the height.
    // TODO: Don't go bigger than the image's actual dimensions.
    //       Can achieve it with ObjectFit::ScaleDown if we surface IMAGE_SCALE from the widget.
    Some(image(image2).fit(ObjectFit::FitWidth))
}

/// Show some useful info for audio attachments.
fn audio_attachment<State: 'static>(
    attachment: &Attachment,
) -> impl WidgetView<State, Navigation> + use<State> {
    flex_col((
        maybe_blurhash(attachment),
        prose("Audio File - Unsupported"),
        attachment.meta.as_ref().map(|meta| {
            meta.length
                .as_deref()
                .map(|it| prose(format!("Length: {it}")))
        }),
        attachment.description.as_deref().map(prose),
    ))
}

/// Show some useful info for audio attachments.
fn video_attachment<State: 'static>(
    attachment: &Attachment,
) -> impl WidgetView<State, Navigation> + use<State> {
    flex_col((
        maybe_blurhash(attachment),
        prose("Video File - Unsupported"),
        attachment.meta.as_ref().map(|meta| {
            meta.length
                .as_deref()
                .map(|it| prose(format!("Length: {it}")))
        }),
        attachment.description.as_deref().map(prose),
    ))
}

/// Show some useful info for audio attachments.
fn image_attachment<State: 'static>(
    attachment: &Attachment,
) -> impl WidgetView<State, Navigation> + use<State> {
    flex_col((
        maybe_blurhash(attachment),
        prose("Image File - Unsupported"),
        attachment.meta.as_ref().map(|meta| {
            meta.size
                .as_deref()
                .or_else(|| meta.original.as_ref().and_then(|it| it.size.as_deref()))
                .map(|it| prose(format!("Dimensions: {it}")))
        }),
        attachment.description.as_deref().map(prose),
    ))
}

// Bound remote dimensions before allocation, retaining the attachment aspect ratio.
fn preview_dimensions(width: u32, height: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let longest = width.max(height);
    let scaled = (longest / 16).clamp(1, 128);
    let dimension = |value: u32| -> Option<u32> {
        u32::try_from((u64::from(value) * u64::from(scaled) / u64::from(longest)).max(1)).ok()
    };
    Some((dimension(width)?, dimension(height)?))
}

#[cfg(test)]
mod tests {
    use super::preview_dimensions;
    #[test]
    fn remote_preview_dimensions_are_bounded() {
        assert_eq!(preview_dimensions(960, 480), Some((60, 30)));
        assert_eq!(preview_dimensions(u32::MAX, 1), Some((128, 1)));
        assert_eq!(preview_dimensions(1, 1), Some((1, 1)));
        assert_eq!(preview_dimensions(0, 100), None);
    }
}
