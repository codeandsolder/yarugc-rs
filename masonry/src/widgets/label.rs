// Copyright 2019 the Xilem Authors and the Druid Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;
use std::mem::Discriminant;

use accesskit::{Node, Role};
use include_doc_path::include_doc_path;
use smallvec::SmallVec;
use tracing::{Span, trace_span};

use crate::core::{
    AccessCtx, ArcStr, BrushIndex, ChildrenIds, LayoutCtx, MeasureCtx, NoAction, PaintCtx,
    PropertiesMut, PropertiesRef, RegisterCtx, StyleProperty, StyleSet, Update, UpdateCtx,
    UsesProperty, Widget, WidgetId, WidgetMut, render_text, set_accesskit_brush_properties,
};
use crate::imaging::Painter;
use crate::kurbo::{Affine, Axis, Point, Size};
use crate::layout::{AsUnit, LenReq, Length};
use crate::parley::{FontContext, Layout, LayoutAccessibility, LayoutContext};
use crate::properties::{ContentColor, LineBreaking};
use crate::theme::default_text_styles;
use crate::util::debug_panic;
use crate::{TextAlign, TextAlignOptions, theme};

/// A widget displaying non-interactive text.
///
/// This is useful for creating interactive widgets which internally
/// need support for displaying text, such as a button.
///
/// You can customize the look of this label with the
/// [`LineBreaking`] and [`ContentColor`] properties.
///
#[doc = concat!(
    "![Styled label](",
    include_doc_path!("screenshots/label_styled_label.png"),
    ")",
)]
pub struct Label {
    /// Cached layouts.
    layouts: Vec<TextLayout>,
    /// Time tracking for cache usage.
    cache_time: u8,
    /// The currently active layout index.
    ///
    /// `usize::MAX` works well for none, as it will be overwritten before its used
    /// for layout access, but will be read as-is during cache eviction.
    /// During which any value larger than the cache capacity will be ignored.
    active_layout: usize,

    text: ArcStr,
    styles: StyleSet,
    text_alignment: TextAlign,

    /// Whether to hint whilst drawing the text.
    ///
    /// Should be disabled whilst an animation involving this label is ongoing.
    // TODO: What classes of animations?
    hint: bool,

    accessibility_hidden: bool,
    accessibility: LayoutAccessibility,
}

/// Text layout computation inputs and output.
struct TextLayout {
    /// Computed text layout.
    layout: Layout<BrushIndex>,
    /// Max advance value that was used when calculating this layout.
    max_advance: Option<f32>,
    /// Alignment parameters, if alignment has been applied.
    alignment: Option<(TextAlign, f32)>,
    /// Last use timestamp for cache eviction purposes.
    last_used: u8,
}

impl TextLayout {
    /// Creates a new [`TextLayout`] with the specified `max_advance` constraint and `timestamp`.
    fn new(max_advance: Option<f32>, timestamp: u8) -> Self {
        Self {
            layout: Layout::new(),
            max_advance,
            alignment: None,
            last_used: timestamp,
        }
    }

    /// Align the text layout.
    ///
    /// This method ensures that alignment only happens when the inputs have changed.
    fn align(&mut self, alignment: TextAlign, alignment_width: f32) {
        let parameters = Some((alignment, alignment_width));
        if self.alignment != parameters {
            // Parley now records the alignment width on individual lines.
            // Rebreak the existing shaped runs only when the actual container
            // width differs from the wrapping constraint, preserving unwrapped
            // Clip/Overflow labels and explicit paragraph boundaries.
            if self.max_advance != Some(alignment_width) {
                let mut breaker = self.layout.break_lines();
                let wrapping_width = self.max_advance.unwrap_or(f32::MAX);
                breaker.state_mut().set_layout_max_advance(wrapping_width);
                breaker.state_mut().set_line_max_advance(wrapping_width);
                while breaker.break_next().is_some() {
                    breaker.set_prior_line_width(alignment_width);
                }
                breaker.finish();
            }
            self.layout.align(alignment, TextAlignOptions::default());
            self.alignment = parameters;
        }
    }
}

// --- MARK: BUILDERS
impl Label {
    /// Creates a new label with the given text.
    ///
    // This is written out fully to appease rust-analyzer; StyleProperty is imported but not recognised.
    /// To change the font size, use `with_style`, setting [`StyleProperty::FontSize`](crate::parley::StyleProperty::FontSize).
    pub fn new(text: impl Into<ArcStr>) -> Self {
        let mut styles = StyleSet::new(theme::TEXT_SIZE_NORMAL);
        default_text_styles(&mut styles);
        Self {
            layouts: Vec::new(),
            cache_time: 0,
            active_layout: usize::MAX,
            text: text.into(),
            styles,
            text_alignment: TextAlign::Start,
            hint: true,
            accessibility_hidden: false,
            accessibility: LayoutAccessibility::default(),
        }
    }

    /// Sets a style property for the new label.
    ///
    /// Setting [`StyleProperty::Brush`](crate::parley::StyleProperty::Brush) is not supported.
    /// Use the [`ContentColor`] property instead.
    ///
    /// To set a style property on an active label, use [`insert_style`](Self::insert_style).
    #[must_use = "Use the configured value."]
    pub fn with_style(mut self, property: impl Into<StyleProperty>) -> Self {
        self.insert_style_inner(property.into());
        self
    }

    /// Sets a style property for the new label, returning the old value.
    ///
    /// Most users should prefer [`with_style`](Self::with_style) instead.
    pub fn try_with_style(
        mut self,
        property: impl Into<StyleProperty>,
    ) -> (Self, Option<StyleProperty>) {
        let old = self.insert_style_inner(property.into());
        (self, old)
    }

    /// Sets the alignment of the text.
    ///
    /// Text alignment might have unexpected results when the label has no horizontal constraints.
    /// To modify this on an active label, use [`set_text_alignment`](Self::set_text_alignment).
    #[must_use = "Use the returned value."]
    pub const fn with_text_alignment(mut self, text_alignment: TextAlign) -> Self {
        self.text_alignment = text_alignment;
        self
    }

    /// Sets whether [hinting](https://en.wikipedia.org/wiki/Font_hinting) will be used for this label.
    ///
    /// Hinting is a process where text is drawn "snapped" to pixel boundaries to improve fidelity.
    /// The default is true, i.e. hinting is enabled by default.
    ///
    /// This should be set to false if the label will be animated at creation.
    /// The kinds of relevant animations include changing variable font parameters,
    /// translating or scaling.
    /// Failing to do so will likely lead to an unpleasant shimmering effect, as different parts of the
    /// text "snap" at different times.
    ///
    /// To modify this on an active label, use [`set_hint`](Self::set_hint).
    // TODO: Should we tell each widget if smooth scrolling is ongoing so they can disable their hinting?
    // Alternatively, we should automate disabling hinting at the Vello layer when composing.
    #[must_use = "Use the returned value."]
    pub const fn with_hint(mut self, hint: bool) -> Self {
        self.hint = hint;
        self
    }

    /// Sets whether this label is hidden from the accessibility tree.
    #[must_use = "Use the returned value."]
    pub const fn accessibility_hidden(mut self, hidden: bool) -> Self {
        self.accessibility_hidden = hidden;
        self
    }

    /// Shared logic between `with_style` and `insert_style`
    fn insert_style_inner(&mut self, property: StyleProperty) -> Option<StyleProperty> {
        if let StyleProperty::Brush(idx @ BrushIndex(1..))
        | StyleProperty::UnderlineBrush(Some(idx @ BrushIndex(1..)))
        | StyleProperty::StrikethroughBrush(Some(idx @ BrushIndex(1..))) = &property
        {
            debug_panic!(
                "Can't set a non-zero brush index ({idx:?}) on a `Label`, as it only supports global styling."
            );
        }
        self.styles.insert(property)
    }
}

// --- MARK: METHODS
impl Label {
    /// Returns a reference to the current text of this label.
    ///
    /// To update the text of an active label, use [`set_text`](Self::set_text).
    #[must_use = "Use the returned value."]
    pub const fn text(&self) -> &ArcStr {
        &self.text
    }

    pub(crate) fn font_weight(&self) -> crate::parley::style::FontWeight {
        use crate::parley::style::FontWeight;
        let key = std::mem::discriminant(&StyleProperty::FontWeight(FontWeight::NORMAL));
        match self.styles.inner().get(&key) {
            Some(StyleProperty::FontWeight(weight)) => *weight,
            _ => FontWeight::NORMAL,
        }
    }

    pub(crate) const fn hint(&self) -> bool {
        self.hint
    }
}

// --- MARK: WIDGETMUT
impl Label {
    // Note: These docs are lazy, but also have a decreased likelihood of going out of date.
    /// The runtime equivalent of [`with_style`](Self::with_style).
    ///
    /// Setting [`StyleProperty::Brush`](crate::parley::StyleProperty::Brush) is not supported.
    /// Use the [`ContentColor`] property instead.
    pub fn insert_style(
        this: &mut WidgetMut<'_, Self>,
        property: impl Into<StyleProperty>,
    ) -> Option<StyleProperty> {
        let old = this.widget.insert_style_inner(property.into());

        this.widget.clear_cache();
        this.ctx.request_layout();
        old
    }

    /// Keeps only the styles for which `f` returns true.
    ///
    /// Styles which are removed return to Parley's default values.
    /// In most cases, these are the defaults for this widget.
    ///
    /// Of note, behaviour is unspecified for unsetting the [`FontSize`](crate::parley::StyleProperty::FontSize).
    pub fn retain_styles(this: &mut WidgetMut<'_, Self>, f: impl FnMut(&StyleProperty) -> bool) {
        this.widget.styles.retain(f);

        this.widget.clear_cache();
        this.ctx.request_layout();
    }

    /// Removes the style with the discriminant `property`.
    ///
    /// To get the discriminant requires constructing a valid `StyleProperty` for the
    /// the desired property and passing it to [`core::mem::discriminant`].
    /// Getting this discriminant is usually possible in a `const` context.
    ///
    /// Styles which are removed return to Parley's default values.
    /// In most cases, these are the defaults for this widget.
    ///
    /// Of note, behaviour is unspecified for unsetting the [`FontSize`](crate::parley::StyleProperty::FontSize).
    pub fn remove_style(
        this: &mut WidgetMut<'_, Self>,
        property: Discriminant<StyleProperty>,
    ) -> Option<StyleProperty> {
        let old = this.widget.styles.remove(property);
        if old.is_some() {
            this.widget.clear_cache();
            this.ctx.request_layout();
        }
        old
    }

    /// Replaces the text of this widget.
    pub fn set_text(this: &mut WidgetMut<'_, Self>, new_text: impl Into<ArcStr>) {
        let new_text = new_text.into();
        if this.widget.text != new_text {
            this.widget.text = new_text;
            this.widget.clear_cache();
            this.ctx.request_layout();
        }
    }

    /// The runtime equivalent of [`with_text_alignment`](Self::with_text_alignment).
    pub fn set_text_alignment(this: &mut WidgetMut<'_, Self>, text_alignment: TextAlign) {
        if this.widget.text_alignment != text_alignment {
            this.widget.text_alignment = text_alignment;
            this.ctx.request_layout();
        }
    }

    /// The runtime equivalent of [`with_hint`](Self::with_hint).
    pub fn set_hint(this: &mut WidgetMut<'_, Self>, hint: bool) {
        if this.widget.hint != hint {
            this.widget.hint = hint;
            this.ctx.request_paint_only();
        }
    }
}

impl Label {
    /// Clears the text layout cache.
    ///
    /// Call this whenever text, styles, or fonts have changed.
    fn clear_cache(&mut self) {
        self.layouts.clear();
        self.active_layout = usize::MAX;
    }

    /// Total number of text layouts to cache.
    ///
    /// Must be at least `2`, to allow for one active layout and one speculative one.
    /// Must be less than `u8::MAX` because it's also used as the cache time reset value.
    const CACHE_CAPACITY: usize = 5;

    /// Increments and returns the cache timestamp.
    fn cache_time(&mut self) -> u8 {
        if self.cache_time == u8::MAX {
            // Compress all last_used timestamps
            let n = self.layouts.len();
            let mut idx: SmallVec<[usize; Self::CACHE_CAPACITY]> = (0..n).collect();
            idx.sort_unstable_by_key(|&i| self.layouts[i].last_used);
            for (rank, &i) in idx.iter().enumerate() {
                self.layouts[i].last_used = rank as u8;
            }
            self.cache_time = Self::CACHE_CAPACITY as u8;
        } else {
            self.cache_time += 1;
        }
        self.cache_time
    }

    /// Builds the text layout and breaks the text into lines.
    ///
    /// Backed by a cache layer.
    fn build_and_break(
        &mut self,
        font_ctx: &mut FontContext,
        layout_ctx: &mut LayoutContext<BrushIndex>,
        max_advance: Option<f32>,
    ) -> usize {
        let timestamp = self.cache_time();

        // Line breaking depends on the exact constraint, including trailing whitespace
        // and clusters that overflow. The measured width alone cannot prove equivalence.
        for (idx, layout) in self.layouts.iter_mut().enumerate() {
            if layout.max_advance == max_advance {
                layout.last_used = timestamp;
                return idx;
            }
        }

        // No known compatible cache entry, so need to do text layout.
        // Select a reusable cache slot, or append when no safe eviction candidate exists.
        let reusable = (self.layouts.len() >= Self::CACHE_CAPACITY)
            .then(|| {
                self.layouts
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != self.active_layout)
                    .min_by_key(|(_, layout)| layout.last_used)
                    .map(|(index, _)| index)
            })
            .flatten();
        let idx = reusable.unwrap_or(self.layouts.len());
        let rebuild_shape = reusable.is_none() || self.layouts[idx].alignment.is_some();
        if idx == self.layouts.len() {
            self.layouts.push(TextLayout::new(max_advance, timestamp));
        }
        let layout = &mut self.layouts[idx];
        layout.max_advance = max_advance;
        layout.last_used = timestamp;
        layout.alignment = None;

        // An unaligned speculative entry can be rebroken without shaping again.
        // An aligned entry may have had its glyph advances changed by justification.
        if rebuild_shape {
            // TODO: Should we use a different scale?
            // See https://github.com/linebender/xilem/issues/1264
            let mut builder = layout_ctx.ranged_builder(font_ctx, &self.text, 1.0, true);
            for prop in self.styles.inner().values() {
                builder.push_default(prop.to_owned());
            }
            builder.build_into(&mut layout.layout, &self.text);
        }
        layout.layout.break_all_lines(max_advance);

        idx
    }
}

impl UsesProperty<ContentColor> for Label {}
impl UsesProperty<LineBreaking> for Label {}

// --- MARK: IMPL WIDGET
impl Widget for Label {
    type Action = NoAction;

    fn accepts_pointer_interaction(&self) -> bool {
        false
    }

    fn register_children(&mut self, _ctx: &mut RegisterCtx<'_>) {}

    fn property_changed(&mut self, ctx: &mut UpdateCtx<'_>, property_type: TypeId) {
        LineBreaking::prop_changed(ctx, property_type);
        ContentColor::prop_changed(ctx, property_type);
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        match event {
            Update::FontsChanged => {
                self.clear_cache();
                ctx.request_layout();
            }
            Update::DisabledChanged(_) => {
                ctx.request_render();
            }
            _ => {}
        }
    }

    fn measure(
        &mut self,
        ctx: &mut MeasureCtx<'_>,
        props: &PropertiesRef<'_>,
        axis: Axis,
        len_req: LenReq,
        cross_length: Option<Length>,
    ) -> Length {
        // Currently we only support the common horizontal-tb writing mode,
        // so we hardcode the assumption that inline axis is horizontal.
        let inline = Axis::Horizontal;

        let cache = ctx.property_cache();
        let line_break_mode = props.get::<LineBreaking>(cache);

        let max_advance = super::text_layout::measure_max_advance(
            *line_break_mode == LineBreaking::WordWrap,
            axis,
            len_req,
            cross_length,
        );

        let (font_ctx, layout_ctx) = ctx.text_contexts();
        let layout_idx = self.build_and_break(font_ctx, layout_ctx, max_advance);
        let layout = &self.layouts[layout_idx];

        let length = if axis == inline {
            layout.layout.width() // Inline length
        } else {
            layout.layout.height() // Block length
        };

        length.px()
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, props: &PropertiesRef<'_>, size: Size) {
        // Currently we only support the common horizontal-tb writing mode,
        // so we hardcode the assumption that inline axis is horizontal.
        let inline = Axis::Horizontal;

        let cache = ctx.property_cache();
        let line_break_mode = props.get::<LineBreaking>(cache);

        let inline_space = size.get_coord(inline) as f32;

        let max_advance = match line_break_mode {
            LineBreaking::WordWrap => Some(inline_space),
            LineBreaking::Clip | LineBreaking::Overflow => None,
        };

        let (font_ctx, layout_ctx) = ctx.text_contexts();
        self.active_layout = self.build_and_break(font_ctx, layout_ctx, max_advance);
        let layout = &mut self.layouts[self.active_layout];

        layout.align(self.text_alignment, inline_space);

        let line_count = layout.layout.len();
        if let (Some(line_first), Some(line_last)) = (
            layout.layout.get(0),
            layout.layout.get(line_count.saturating_sub(1)),
        ) {
            let first_baseline = f64::from(line_first.metrics().baseline);
            let last_baseline = f64::from(line_last.metrics().baseline);
            ctx.set_baselines(first_baseline, last_baseline);
        } else {
            ctx.clear_baselines();
        }

        if *line_break_mode == LineBreaking::Clip {
            let border_box = size.to_rect() + ctx.border_box_insets();
            ctx.set_clip_path(border_box);
        } else {
            ctx.clear_clip_path();
        }
    }

    fn paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        let cache = ctx.property_cache();
        let text_color = props.get::<ContentColor>(cache);

        let layout = &self.layouts[self.active_layout];

        render_text(
            painter,
            Affine::IDENTITY,
            &layout.layout,
            &[text_color.color.into()],
            self.hint,
        );
    }

    fn accessibility_role(&self) -> Role {
        Role::Label
    }

    fn accessibility(
        &mut self,
        ctx: &mut AccessCtx<'_>,
        props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        if self.accessibility_hidden {
            node.set_hidden();
            return;
        }

        let text_origin_in_border_box_space = Point::ORIGIN + ctx.border_box_translation();

        let cache = ctx.property_cache();
        let text_color = props.get::<ContentColor>(cache);

        let layout = &self.layouts[self.active_layout];

        self.accessibility.build_nodes(
            self.text.as_ref(),
            &layout.layout,
            ctx.tree_update(),
            node,
            AccessCtx::next_node_id,
            text_origin_in_border_box_space.x,
            text_origin_in_border_box_space.y,
            |node, style| set_accesskit_brush_properties(node, style, &[text_color.color.into()]),
        );
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::new()
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("Label", id = id.trace())
    }

    fn get_debug_text(&self) -> Option<String> {
        Some(self.text.to_string())
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{NewWidget, PropertySet};
    use crate::layout::{AsUnit, Dim};
    use crate::parley::style::GenericFamily;
    use crate::parley::{FontFamily, FontFamilyName, StyleProperty};
    use crate::properties::Dimensions;
    use crate::properties::Gap;
    use crate::properties::types::CrossAxisAlignment;
    use crate::testing::{TestHarness, assert_render_snapshot};
    use crate::theme::{ACCENT_COLOR, test_property_set};
    use crate::widgets::{Flex, SizedBox};

    #[test]
    fn cache_matches_fresh_layout_across_wrap_boundaries() {
        let mut fonts = FontContext::new();
        fonts.collection.register_fonts(
            crate::peniko::Blob::new(std::sync::Arc::new(masonry_testing::ROBOTO)),
            None,
        );
        let mut context = LayoutContext::new();
        for text in ["hello world", "hello world  ", "a long unbreakableword"] {
            let label =
                || Label::new(text).with_style(FontFamily::Single(FontFamilyName::named("Roboto")));
            let mut cached = label();
            let initial = cached.build_and_break(&mut fonts, &mut context, None);
            let width = cached.layouts[initial].layout.full_width();
            assert!(width > 0.);
            cached.active_layout = initial;
            for constraint in [
                Some(width - 0.005),
                Some(width),
                Some(width + 0.005),
                Some(0.),
                Some(width * 0.5),
                Some(width * 0.25),
                Some(width * 0.75),
                Some(width - 0.005),
                None,
            ] {
                let index = cached.build_and_break(&mut fonts, &mut context, constraint);
                let mut fresh = label();
                let reference = fresh.build_and_break(&mut fonts, &mut context, constraint);
                let actual = &cached.layouts[index].layout;
                let expected = &fresh.layouts[reference].layout;
                assert_eq!(actual.len(), expected.len(), "{text:?} at {constraint:?}");
                for (actual, expected) in actual.lines().zip(expected.lines()) {
                    assert_eq!(actual.text_range(), expected.text_range());
                    assert_eq!(actual.break_reason(), expected.break_reason());
                    assert_eq!(
                        actual.metrics().advance.to_bits(),
                        expected.metrics().advance.to_bits()
                    );
                }
                assert!(cached.layouts.len() <= Label::CACHE_CAPACITY);
            }
            assert_eq!(cached.layouts[initial].max_advance, None);
        }
    }

    #[test]
    fn unwrapped_labels_align_without_adding_soft_breaks() {
        let mut fonts = FontContext::new();
        fonts.collection.register_fonts(
            crate::peniko::Blob::new(std::sync::Arc::new(masonry_testing::ROBOTO)),
            None,
        );
        let mut context = LayoutContext::new();
        let mut label = Label::new("a long label with spaces\nshort")
            .with_style(FontFamily::Single(FontFamilyName::named("Roboto")));
        let index = label.build_and_break(&mut fonts, &mut context, None);
        let entry = &mut label.layouts[index];
        let ranges: Vec<_> = entry.layout.lines().map(|line| line.text_range()).collect();
        assert_eq!(ranges.len(), 2);
        let long_width = entry.layout.width();
        let narrow = long_width * 0.5;
        entry.align(TextAlign::Center, narrow);
        assert_eq!(
            entry
                .layout
                .lines()
                .map(|line| line.text_range())
                .collect::<Vec<_>>(),
            ranges
        );
        let short_line = entry.layout.get(1).unwrap();
        let short = short_line.metrics();
        let expected = (narrow - short.advance + short.trailing_whitespace) * 0.5;
        assert!(expected > 0.0);
        assert!((short.offset - expected).abs() < 0.001);
        assert_eq!(entry.layout.width(), long_width);
        entry.align(TextAlign::End, long_width * 2.0);
        assert_eq!(
            entry
                .layout
                .lines()
                .map(|line| line.text_range())
                .collect::<Vec<_>>(),
            ranges
        );
    }

    #[test]
    fn repeated_overflowing_constraint_hits_cache() {
        let mut fonts = FontContext::new();
        let mut context = LayoutContext::new();
        let mut label = Label::new("unbreakable");
        let index = label.build_and_break(&mut fonts, &mut context, Some(0.));
        assert_eq!(
            label.build_and_break(&mut fonts, &mut context, Some(0.)),
            index
        );
        assert_eq!(label.layouts.len(), 1);
    }

    #[test]
    fn unchanged_text_and_alignment_preserve_cached_layouts() {
        let mut harness = TestHarness::create_with_size(
            test_property_set(),
            Label::new("Stable").prepare(),
            (100, 40),
        );
        let _ = harness.redraw();
        let before = harness.root_widget().unwrap();
        let timestamp = before.cache_time;
        let active = before.active_layout;
        let count = before.layouts.len();
        let alignment = before.text_alignment;
        harness
            .edit_root_widget(|mut label| {
                Label::set_text(&mut label, String::from("Stable"));
                Label::set_text_alignment(&mut label, alignment);
            })
            .unwrap();
        let after = harness.root_widget().unwrap();
        assert_eq!(
            after.cache_time, timestamp,
            "unchanged text must not trigger shaping or measurement"
        );
        assert_eq!(after.active_layout, active);
        assert_eq!(after.layouts.len(), count);
    }

    #[test]
    fn simple_label() {
        let label = Label::new("Hello").prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), label, (100, 40));

        assert_render_snapshot!(harness, "label_hello");
    }

    #[test]
    fn styled_label() {
        let label = Label::new("The quick brown fox jumps over the lazy dog")
            .with_style(FontFamily::Single(FontFamilyName::Generic(
                GenericFamily::Monospace,
            )))
            .with_style(StyleProperty::FontSize(20.0))
            .with_text_alignment(TextAlign::Center)
            .prepare()
            .with_props(
                PropertySet::new()
                    .with(ContentColor::new(ACCENT_COLOR))
                    .with(LineBreaking::WordWrap),
            );

        let mut harness = TestHarness::create_with_size(test_property_set(), label, (200, 200));

        assert_render_snapshot!(harness, "label_styled_label");
    }

    #[test]
    fn underline_label() {
        let label = Label::new("Emphasis")
            .with_style(StyleProperty::Underline(true))
            .prepare()
            .with_props(PropertySet::new().with(LineBreaking::WordWrap));

        let mut harness = TestHarness::create_with_size(test_property_set(), label, (100, 40));

        assert_render_snapshot!(harness, "label_underline_label");
    }
    #[test]
    fn strikethrough_label() {
        let label = Label::new("Tpyo")
            .with_style(StyleProperty::Strikethrough(true))
            .with_style(StyleProperty::StrikethroughSize(Some(4.)))
            .prepare()
            .with_props(PropertySet::new().with(LineBreaking::WordWrap));

        let mut harness = TestHarness::create_with_size(test_property_set(), label, (100, 40));

        assert_render_snapshot!(harness, "label_strikethrough_label");
    }

    #[test]
    /// A label's text alignment should be respected, regardless of
    /// its parent's alignment plans for it, if the label has stretched width.
    fn label_text_alignment_flex() {
        fn base_label(text_alignment: TextAlign) -> NewWidget<Label> {
            Label::new("Hello")
                .with_style(StyleProperty::FontSize(20.0))
                .with_text_alignment(text_alignment)
                .prepare()
                .with_props(Dimensions::width(Dim::Stretch))
        }
        let label1 = base_label(TextAlign::Start);
        let label2 = base_label(TextAlign::Center);
        let label3 = base_label(TextAlign::End);
        let label4 = base_label(TextAlign::Start);
        let label5 = base_label(TextAlign::Center);
        let label6 = base_label(TextAlign::End);
        let flex = Flex::column()
            .with(label1, CrossAxisAlignment::Start)
            .with(label2, CrossAxisAlignment::Start)
            .with(label3, CrossAxisAlignment::Start)
            .with(label4, CrossAxisAlignment::Center)
            .with(label5, CrossAxisAlignment::Center)
            .with(label6, CrossAxisAlignment::Center);
        let flex = NewWidget::new(flex).with_props(Gap::ZERO);

        let mut harness = TestHarness::create_with_size(test_property_set(), flex, (200, 200));

        assert_render_snapshot!(harness, "label_label_alignment_flex");
    }

    #[test]
    fn line_break_modes() {
        let widget = Flex::column()
            .with_spacer(1.0)
            .with_fixed(
                SizedBox::new(
                    Label::new("The quick brown fox jumps over the lazy dog")
                        .prepare()
                        .with_props(PropertySet::new().with(LineBreaking::WordWrap)),
                )
                .width(180.px())
                .prepare(),
            )
            .with_fixed_spacer(20.px())
            .with_fixed(
                SizedBox::new(
                    Label::new("The quick brown fox jumps over the lazy dog")
                        .prepare()
                        .with_props(PropertySet::new().with(LineBreaking::Clip)),
                )
                .width(180.px())
                .prepare(),
            )
            .with_fixed_spacer(20.px())
            .with_fixed(
                SizedBox::new(
                    Label::new("The quick brown fox jumps over the lazy dog")
                        .prepare()
                        .with_props(PropertySet::new().with(LineBreaking::Overflow)),
                )
                .width(180.px())
                .prepare(),
            )
            .with_spacer(1.0)
            .prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (200, 200));

        assert_render_snapshot!(harness, "label_line_break_modes");
    }

    #[test]
    fn edit_label() {
        let image_1 = {
            let label = Label::new("The quick brown fox jumps over the lazy dog")
                .with_style(FontFamily::Single(FontFamilyName::Generic(
                    GenericFamily::Monospace,
                )))
                .with_style(StyleProperty::FontSize(20.0))
                .with_text_alignment(TextAlign::Center)
                .prepare()
                .with_props(
                    PropertySet::new()
                        .with(ContentColor::new(ACCENT_COLOR))
                        .with(LineBreaking::WordWrap),
                );

            let mut harness = TestHarness::create_with_size(test_property_set(), label, (50, 50));

            harness.render().unwrap()
        };

        let image_2 = {
            let label = Label::new("Hello world")
                .with_style(StyleProperty::FontSize(40.0))
                .prepare();

            let mut harness = TestHarness::create_with_size(test_property_set(), label, (50, 50));

            harness
                .edit_root_widget(|mut label| {
                    label.insert_prop(ContentColor::new(ACCENT_COLOR));
                    label.insert_prop(LineBreaking::WordWrap);
                    Label::set_text(&mut label, "The quick brown fox jumps over the lazy dog");
                    Label::insert_style(
                        &mut label,
                        FontFamily::Single(FontFamilyName::Generic(GenericFamily::Monospace)),
                    );
                    Label::insert_style(&mut label, StyleProperty::FontSize(20.0));
                    Label::set_text_alignment(&mut label, TextAlign::Center);
                })
                .unwrap();

            harness.render().unwrap()
        };

        // We don't use assert_eq because we don't want rich assert
        assert!(image_1 == image_2);
    }
}
