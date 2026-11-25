// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use masonry::core::ArcStr;
use masonry::properties::ObjectFit;
use masonry::widgets;
use usvg::Tree;

use crate::core::{MessageCtx, MessageResult, Mut, View, ViewMarker};
use crate::view::Prop;
use crate::{Pod, ViewCtx, WidgetView};

/// Displays the SVG.
///
/// By default, the SVG will be scaled to fully fit within the container ([`ObjectFit::Contain`]).
/// To configure this, call [`fit`](Svg::fit) on the returned value.
pub const fn svg(tree: Arc<Tree>) -> Svg {
    Svg {
        tree,
        decorative: false,
        alt_text: None,
    }
}

/// A view of an SVG image.
#[must_use = "View values do nothing unless provided to Xilem."]
pub struct Svg {
    tree: Arc<Tree>,
    decorative: bool,
    alt_text: Option<ArcStr>,
}

impl Svg {
    image_view_builders!(with_alt_text);
}
image_view_lifecycle!(
    Svg,
    Svg,
    tree,
    set_tree,
    |prev: &Arc<Tree>, next: &Arc<Tree>| !Arc::ptr_eq(prev, next)
);
