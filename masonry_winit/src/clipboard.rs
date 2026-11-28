// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Clipboard access bound to the window system selected by Winit.

use std::io;

use winit::event_loop::OwnedDisplayHandle;

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "windows",
    target_os = "macos",
))]
use copypasta::{ClipboardContext, ClipboardProvider};

pub(super) enum NativeClipboard {
    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
    ))]
    Wayland(smithay_clipboard::Clipboard),
    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "windows",
        target_os = "macos",
    ))]
    System(ClipboardContext),
    Unavailable(String),
}

impl NativeClipboard {
    pub(super) fn new(display: &OwnedDisplayHandle) -> Self {
        #[cfg(any(
            target_os = "linux",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly",
        ))]
        if let Some(connection) = display.wayland_connection() {
            return match smithay_clipboard::Clipboard::from_connection(connection) {
                Ok(clipboard) => Self::Wayland(clipboard),
                Err(err) => Self::unavailable(err.to_string()),
            };
        }
        let _ = display;
        #[cfg(any(
            target_os = "linux",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly",
            target_os = "windows",
            target_os = "macos",
        ))]
        {
            match ClipboardContext::new() {
                Ok(clipboard) => Self::System(clipboard),
                Err(err) => Self::unavailable(err.to_string()),
            }
        }
        #[cfg(not(any(
            target_os = "linux",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly",
            target_os = "windows",
            target_os = "macos",
        )))]
        Self::unavailable("clipboard unsupported on this platform".into())
    }

    fn unavailable(reason: String) -> Self {
        tracing::warn!(%reason, "Clipboard unavailable");
        Self::Unavailable(reason)
    }

    pub(super) fn get_contents(&mut self) -> io::Result<String> {
        match self {
            #[cfg(any(
                target_os = "linux",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            Self::Wayland(clipboard) => clipboard.load(),
            #[cfg(any(
                target_os = "linux",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly",
                target_os = "windows",
                target_os = "macos"
            ))]
            Self::System(clipboard) => clipboard.get_contents().map_err(io::Error::other),
            Self::Unavailable(reason) => Err(io::Error::other(reason.clone())),
        }
    }

    pub(super) fn set_contents(&mut self, text: String) -> io::Result<()> {
        match self {
            #[cfg(any(
                target_os = "linux",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            Self::Wayland(clipboard) => clipboard.store(text),
            #[cfg(any(
                target_os = "linux",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly",
                target_os = "windows",
                target_os = "macos"
            ))]
            Self::System(clipboard) => clipboard.set_contents(text).map_err(io::Error::other),
            Self::Unavailable(reason) => Err(io::Error::other(reason.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_clipboard_reports_both_operations() {
        let mut clipboard = NativeClipboard::Unavailable("display unavailable".into());
        assert_eq!(
            clipboard.get_contents().unwrap_err().to_string(),
            "display unavailable"
        );
        assert_eq!(
            clipboard
                .set_contents("text".into())
                .unwrap_err()
                .to_string(),
            "display unavailable"
        );
    }
}
