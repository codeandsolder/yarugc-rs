// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Native demo HTTP transport and bounded image loading. The GUI libraries do
//! not depend on networking or image decoding through this package.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, ensure};
use peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
pub use reqwest::Client;
use reqwest::Response;
use tokio::sync::Semaphore;

// Bound in-flight encoded and decoded image memory, including decode tasks
// that outlive a cancelled virtual-list request.
static IMAGE_LOADS: Semaphore = Semaphore::const_new(4);

/// Construct a pooled client with explicit connection and request deadlines.
///
/// # Errors
/// Returns an error if the HTTP client cannot be constructed.
pub fn client(user_agent: &str) -> Result<Client> {
    Client::builder()
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(Into::into)
}

/// Bound the actual streamed bytes even when Content-Length is absent or misleading.
///
/// # Errors
/// Returns an error on transport failure or if the body exceeds `max_bytes`.
pub async fn read_body(mut response: Response, max_bytes: usize) -> Result<Vec<u8>> {
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= max_bytes as u64),
        "HTTP response exceeds {max_bytes} bytes"
    );
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            chunk.len() <= max_bytes - body.len(),
            "HTTP response exceeds {max_bytes} bytes"
        );
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Image output policy for a bounded remote image download.
#[derive(Clone, Copy, Debug)]
pub enum ImagePolicy {
    /// A small thumbnail, avoiding retained full-size profile photos.
    Avatar,
    /// Preserve the image dimensions for illustrative examples.
    Illustration,
}

/// Fetch a JPEG or PNG and decode it away from the async executor's worker.
/// Both encoded size and decoded dimensions/allocation are bounded.
///
/// # Errors
/// Returns an error for HTTP, resource limit, unsupported image, decode, or task failure.
pub async fn fetch_image(client: &Client, url: &str, policy: ImagePolicy) -> Result<ImageData> {
    let permit = IMAGE_LOADS.acquire().await?;
    let response = client.get(url).send().await?.error_for_status()?;
    let bytes = read_body(response, 8 * 1024 * 1024).await?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        decode_image(bytes, policy)
    })
    .await?
}

fn decode_image(bytes: Vec<u8>, policy: ImagePolicy) -> Result<ImageData> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode()?;
    let rgba = match policy {
        ImagePolicy::Avatar => decoded.thumbnail(256, 256).into_rgba8(),
        ImagePolicy::Illustration => decoded.into_rgba8(),
    };
    let (width, height) = rgba.dimensions();
    Ok(ImageData {
        data: Blob::new(Arc::new(rgba.into_vec())),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Result<Vec<u8>> {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(width, height)
            .write_to(&mut bytes, image::ImageFormat::Png)?;
        Ok(bytes.into_inner())
    }

    #[test]
    fn avatar_and_illustration_have_distinct_output_sizes() -> Result<()> {
        let bytes = png(512, 128)?;
        let avatar = decode_image(bytes.clone(), ImagePolicy::Avatar)?;
        assert_eq!((avatar.width, avatar.height), (256, 64));
        let full = decode_image(bytes, ImagePolicy::Illustration)?;
        assert_eq!((full.width, full.height), (512, 128));
        Ok(())
    }

    #[test]
    fn rejects_excessive_dimensions_before_thumbnailing() -> Result<()> {
        let bytes = png(4097, 1)?;
        assert!(
            decode_image(bytes, ImagePolicy::Avatar).is_err(),
            "oversized source must not be accepted as a thumbnail"
        );
        Ok(())
    }
}
