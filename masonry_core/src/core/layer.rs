// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::core::Widget;
use crate::kurbo::{Point, Rect, Size};

use crate::core::{EventCtx, PointerEvent, PropertiesMut};

/// The type of a new [`Layer`].
///
/// When adding a new [layer](crate::doc::masonry_concepts#layers) to the app,
/// this tells the app driver what type of item to add.
#[derive(Clone, Debug, Default)]
pub enum LayerType {
    /// A simple tooltip showing some text until the mouse moves.
    Tooltip(String),
    /// A menu showing the different options of a selector widget.
    Selector {
        /// The text of the options.
        options: Vec<String>,
        /// The initially selected option.
        selected_option: usize,
    },
    /// Unknown layer type. Always use the widget fallback.
    #[default]
    Other,
}

/// Placement policy for a top-level layer, in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayerPlacement {
    /// Unconstrained layer at a fixed origin.
    Absolute(Point),
    /// A popup anchored to a rectangle. Prefer below the anchor, flip above
    /// when necessary, and constrain its size to the available viewport.
    Popup(Rect),
}

impl LayerPlacement {
    pub(crate) fn resolve(self, natural: Size, viewport: Size) -> (Point, Size) {
        match self {
            Self::Absolute(origin) => (origin, natural),
            Self::Popup(anchor) => {
                let width = natural.width.max(anchor.width()).min(viewport.width);
                let x = anchor.x0.clamp(0.0, (viewport.width - width).max(0.0));
                let top = anchor.y0.clamp(0.0, viewport.height);
                let bottom = anchor.y1.clamp(0.0, viewport.height);
                let below = viewport.height - bottom;
                let above = top;
                if below == 0.0 && above == 0.0 {
                    return (
                        Point::new(x, 0.0),
                        Size::new(width, natural.height.min(viewport.height)),
                    );
                }
                if natural.height <= below || below >= above {
                    (
                        Point::new(x, bottom),
                        Size::new(width, natural.height.min(below)),
                    )
                } else {
                    let height = natural.height.min(above);
                    (Point::new(x, top - height), Size::new(width, height))
                }
            }
        }
    }
}

/// The trait implemented by widgets which are meant to be at the root of
/// a [layer](crate::doc::masonry_concepts#layers).
pub trait Layer: Widget {
    // TODO - Possible evolutions:
    // - Return flag to suppress event from reaching children.
    // - Return flag to remove layer.
    // - Pass layer id to method.

    /// An event handler called for every layer for all pointer events, even those outside the layer's root widget.
    fn capture_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_placement_flips_and_clamps_to_viewport() {
        let viewport = Size::new(100.0, 80.0);
        let popup = LayerPlacement::Popup(Rect::new(75.0, 60.0, 95.0, 75.0));
        let (origin, size) = popup.resolve(Size::new(60.0, 200.0), viewport);
        assert_eq!(origin, Point::new(40.0, 0.0));
        assert_eq!(size, Size::new(60.0, 60.0));
        let popup = LayerPlacement::Popup(Rect::new(5.0, 10.0, 80.0, 20.0));
        let (origin, size) = popup.resolve(Size::new(30.0, 40.0), viewport);
        assert_eq!(origin, Point::new(5.0, 20.0));
        assert_eq!(size, Size::new(75.0, 40.0));
    }
}
