// Copyright 2018 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

//! Support for text display and rendering
//!
//! There are three kinds of text commonly needed:
//!  1) Non interactive text (e.g. a button's label)
//!  2) Selectable text (e.g. a paragraph of content)
//!  3) Editable text (e.g. a search bar)
//!
//! All of these have the same set of global styling options, and can contain rich text

/// A reference counted string slice.
///
/// This is a data-friendly way to represent strings in Masonry. Unlike `String`
/// it cannot be mutated, but unlike `String` it can be cheaply cloned.
pub type ArcStr = std::sync::Arc<str>;

/// The Parley [`Brush`] used within Masonry.
///
/// This enables updating of brush details without performing relayouts;
/// the inner values are indexes into the `brushes` argument to [`render_text()`].
///
/// [`Brush`]: parley::Brush
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct BrushIndex(pub usize);

/// A style property specialised for use within Masonry.
pub type StyleProperty = parley::StyleProperty<'static, BrushIndex>;

/// A set of styles specialised for use within Masonry.
pub type StyleSet = parley::StyleSet<BrushIndex>;

use accesskit::{TextDecoration, TextDecorationStyle};
use kurbo::{Affine, Line, Stroke};
use parley::{Decoration, GlyphRun, Layout, PositionedLayoutItem, Style};
use peniko::{Brush, Fill};
use smallvec::SmallVec;

use crate::imaging::{PaintSink, Painter, record::Glyph};

/// A function that renders laid out glyphs through imaging's [`Painter`].
///
/// The `BrushIndex` values of the runs are indices into `brushes`.
/// Runs or decorations whose brush is missing are skipped.
pub fn render_text(
    painter: &mut Painter<'_, impl PaintSink + ?Sized>,
    transform: Affine,
    layout: &Layout<BrushIndex>,
    brushes: &[Brush],
    // TODO: Should this be part of `BrushIndex` (i.e. `brushes`)?
    hint: bool,
) {
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            let style = glyph_run.style();
            // Underline is painted behind glyphs, strikethrough in front.
            if let Some(underline) = &style.underline {
                paint_decoration(
                    painter,
                    transform,
                    &glyph_run,
                    underline,
                    brushes,
                    DecorationKind::Underline,
                );
            }
            let run = glyph_run.run();
            let font = run.font();
            let font_size = run.font_size();
            let synthesis = run.synthesis();
            let glyph_xform = synthesis
                .skew()
                .map(|angle| Affine::skew(f64::from(angle.to_radians().tan()), 0.0));
            if let Some(brush) = brushes.get(style.brush.0) {
                let coords: SmallVec<[i16; 8]> = run
                    .normalized_coords()
                    .iter()
                    .map(|coord| coord.to_bits())
                    .collect();
                let glyphs: SmallVec<[Glyph; 16]> = glyph_run
                    .positioned_glyphs()
                    .map(|glyph| Glyph {
                        id: glyph.id,
                        x: glyph.x,
                        y: glyph.y,
                    })
                    .collect();
                painter
                    .glyphs(font, brush)
                    .hint(hint)
                    .transform(transform)
                    .glyph_transform(glyph_xform)
                    .font_size(font_size)
                    .normalized_coords(&coords)
                    .draw(&peniko::Style::Fill(Fill::NonZero), &glyphs);
            }
            if let Some(strikethrough) = &style.strikethrough {
                paint_decoration(
                    painter,
                    transform,
                    &glyph_run,
                    strikethrough,
                    brushes,
                    DecorationKind::Strikethrough,
                );
            }
        }
    }
}

#[derive(Clone, Copy)]
enum DecorationKind {
    Underline,
    Strikethrough,
}

impl DecorationKind {
    fn resolve(
        self,
        decoration: &Decoration<BrushIndex>,
        metrics: &parley::FontMetrics,
        baseline: f32,
    ) -> (f64, f64) {
        let (default_offset, default_width) = match self {
            Self::Underline => (metrics.underline_offset, metrics.underline_size),
            Self::Strikethrough => (metrics.strikethrough_offset, metrics.strikethrough_size),
        };
        let width = decoration.size.unwrap_or(default_width);
        let offset = decoration.offset.unwrap_or(default_offset);
        // Preserve the top edge of an underline and the center of a
        // strikethrough when overriding its thickness.
        let center_width = match self {
            Self::Underline => width,
            Self::Strikethrough => default_width,
        };
        (
            f64::from(baseline - offset + center_width / 2.0),
            f64::from(width),
        )
    }
}

fn paint_decoration(
    painter: &mut Painter<'_, impl PaintSink + ?Sized>,
    transform: Affine,
    run: &GlyphRun<'_, BrushIndex>,
    decoration: &Decoration<BrushIndex>,
    brushes: &[Brush],
    kind: DecorationKind,
) {
    let Some(brush) = brushes.get(decoration.brush.0) else {
        return;
    };
    let (y, width) = kind.resolve(decoration, run.run().font_metrics(), run.baseline());
    let line = Line::new(
        (f64::from(run.offset()), y),
        (f64::from(run.offset() + run.advance()), y),
    );
    painter
        .stroke(line, &Stroke::new(width), brush)
        .transform(transform)
        .draw();
}

fn to_accesskit_color(brush: &Brush) -> Option<accesskit::Color> {
    if let Brush::Solid(color) = brush {
        let rgba = color.to_rgba8();
        Some(accesskit::Color {
            red: rgba.r,
            green: rgba.g,
            blue: rgba.b,
            alpha: rgba.a,
        })
    } else {
        None
    }
}

/// Sets AccessKit text properties from the brush(es) for the given style.
///
/// The `BrushIndex` values of the runs are indices into `brushes`.
pub fn set_accesskit_brush_properties(
    node: &mut accesskit::Node,
    style: &Style<BrushIndex>,
    brushes: &[Brush],
) {
    let color = |index: &BrushIndex| brushes.get(index.0).and_then(to_accesskit_color);
    if let Some(color) = color(&style.brush) {
        node.set_foreground_color(color);
    } else {
        node.clear_foreground_color();
    }
    let decoration = |deco: &Decoration<BrushIndex>| {
        Some(TextDecoration {
            style: TextDecorationStyle::Solid,
            color: color(&deco.brush)?,
        })
    };
    if let Some(underline) = style.underline.as_ref().and_then(decoration) {
        node.set_underline(underline);
    } else {
        node.clear_underline();
    }
    if let Some(strikethrough) = style.strikethrough.as_ref().and_then(decoration) {
        node.set_strikethrough(strikethrough);
    } else {
        node.clear_strikethrough();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoration_thickness_preserves_underline_edge_and_strike_center() {
        let metrics = parley::FontMetrics {
            underline_offset: -2.0,
            underline_size: 2.0,
            strikethrough_offset: 8.0,
            strikethrough_size: 2.0,
            ..parley::FontMetrics::fallback(20.0)
        };
        let mut deco = Decoration {
            brush: BrushIndex(0),
            offset: None,
            size: None,
        };
        assert_eq!(
            DecorationKind::Underline.resolve(&deco, &metrics, 20.0),
            (23.0, 2.0)
        );
        assert_eq!(
            DecorationKind::Strikethrough.resolve(&deco, &metrics, 20.0),
            (13.0, 2.0)
        );
        deco.size = Some(6.0);
        assert_eq!(
            DecorationKind::Underline.resolve(&deco, &metrics, 20.0),
            (25.0, 6.0)
        );
        assert_eq!(
            DecorationKind::Strikethrough.resolve(&deco, &metrics, 20.0),
            (13.0, 6.0)
        );
        deco.offset = Some(4.0);
        assert_eq!(
            DecorationKind::Underline.resolve(&deco, &metrics, 20.0),
            (19.0, 6.0)
        );
        assert_eq!(
            DecorationKind::Strikethrough.resolve(&deco, &metrics, 20.0),
            (17.0, 6.0)
        );
    }
}
