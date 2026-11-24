// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use masonry_core::core::WidgetId;
use masonry_core::imaging::ImageRendererError;
use std::path::PathBuf;

/// Failure to simulate an interaction, render a frame, or compare a snapshot.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    /// The requested widget is not registered.
    #[error("Widget {0} not found")]
    MissingWidget(WidgetId),
    /// The target cannot receive the requested pointer interaction.
    #[error("Widget {0} cannot be reached by the pointer")]
    UnreachableWidget(WidgetId),
    /// The requested render size overflows the image dimensions.
    #[error("Image dimensions overflow")]
    ImageDimensions,
    /// The renderer returned an invalid pixel buffer.
    #[error("Renderer returned an invalid pixel buffer")]
    InvalidPixels,
    /// Rendering failed.
    #[error(transparent)]
    Render(#[from] ImageRendererError),
    /// Image encoding or decoding failed.
    #[error(transparent)]
    Image(#[from] image::ImageError),
    /// Snapshot compression failed.
    #[error(transparent)]
    Compression(#[from] oxipng::PngError),
    /// A filesystem operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The system clock predates the Unix epoch.
    #[error(transparent)]
    Clock(#[from] std::time::SystemTimeError),
    /// The reference snapshot is absent.
    #[error("No reference snapshot at {}", .0.display())]
    MissingReference(PathBuf),
    /// Snapshot pixels or dimensions differ.
    #[error("Snapshot differs from {}", .0.display())]
    SnapshotMismatch(PathBuf),
    /// A snapshot expected to differ matches the reference.
    #[error("Snapshot unexpectedly matches {}", .0.display())]
    UnexpectedMatch(PathBuf),
    /// The compressed snapshot is larger than the configured limit.
    #[error("Snapshot {} exceeds limit of {limit} bytes ({size} bytes)", path.display())]
    SnapshotTooLarge {
        /// Snapshot filename.
        path: PathBuf,
        /// Actual number of bytes.
        size: u64,
        /// Maximum permitted bytes.
        limit: u32,
    },
}
