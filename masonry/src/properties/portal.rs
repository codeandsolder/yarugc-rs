// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::core::Property;

/// Hides the portal scrollbars after pointer inactivity.
/// Independent of the thumb-width styling controlled by Collapsible.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoHideScrollBar(pub bool);

impl Property for AutoHideScrollBar {
    fn static_default() -> &'static Self {
        static DEFAULT: AutoHideScrollBar = AutoHideScrollBar(false);
        &DEFAULT
    }
}
