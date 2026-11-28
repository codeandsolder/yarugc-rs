// Copyright 2026 the Xilem Authors and the Parley Authors
// SPDX-License-Identifier: Apache-2.0

//! An [`accesskit`] integration for Parley text layouts and editors.
//!
//! Adapted from the `editor` example of Parley 0.12, which replaced Parley's
//! former built-in `accesskit` feature. Uses explicit UTF-8 boundaries and
//! logical-to-physical span mapping for bidirectional selection.

use std::collections::HashMap;

use accesskit::{
    Node, NodeId, Rect, Role, TextAlign, TextDirection, TextPosition, TextSelection, TreeUpdate,
};
use kurbo::Point;
use parley::editing::{Cursor, PlainEditor, PlainEditorDriver, Selection};
use parley::layout::{Affinity, Alignment, Cluster, Layout, LineMetrics, Run, Style};
use parley::style::{Brush, FontStyle};
use skrifa::FontRef;
use skrifa::raw::TableProvider;
use skrifa::raw::types::NameId;

/// The maximum number of characters in a single `TextRun` node.
///
/// AccessKit's `word_starts` are `u8`s, so a span can't describe more characters than that.
const MAX_CHARACTERS_PER_SPAN: usize = u8::MAX as usize + 1;

/// Destination and coordinates of an accessible text subtree.
#[derive(Debug)]
pub struct TextAccessibilityTarget<'a> {
    /// Update receiving the child text-run nodes.
    pub update: &'a mut TreeUpdate,
    /// Parent widget node.
    pub parent: &'a mut Node,
    /// Text layout origin in the parent widget's coordinates.
    pub origin: Point,
}

/// Maps a Parley layout onto AccessKit text runs and bidirectional text selections.
/// Reuse the instance for a widget to preserve node identities across updates.
#[derive(Clone, Default, Debug)]
pub struct LayoutAccessibility {
    // Spans are kept in logical order. Their boundary arrays include the final
    // byte boundary even when the span ends before the end of a paragraph.
    spans: Vec<PublishedSpan>,
    span_by_id: HashMap<NodeId, usize>,
}

#[derive(Clone, Debug)]
struct PublishedSpan {
    id: NodeId,
    boundaries: Vec<usize>,
    hard_break_end: bool,
}

impl LayoutAccessibility {
    /// Publish the layout's text runs. `text` must be the source of `layout`.
    pub fn build_nodes<B: Brush>(
        &mut self,
        text: &str,
        layout: &Layout<B>,
        mut target: TextAccessibilityTarget<'_>,
        mut next_node_id: impl FnMut() -> NodeId,
        set_brush_properties: impl Fn(&mut Node, &Style<B>),
    ) {
        self.span_by_id.clear();
        let mut spans_started = 0;
        let mut runs = Vec::new();
        let alignment = layout.alignment();
        let paragraph_rtl = layout.is_rtl();
        for line in layout.lines() {
            let metrics = line.metrics();
            // Compute physical run locations before sorting into logical text order.
            let mut run_offset = metrics.offset;
            runs.clear();
            for run in line.runs() {
                runs.push((run, run_offset));
                run_offset += run.advance();
            }
            runs.sort_by_key(|(run, _)| run.text_range().start);
            let mut previous = None;
            for (run, run_offset) in runs.iter().cloned() {
                let mut span = SpanData::new(run.text_range().start);
                let mut node = text_run_node(&run, alignment, paragraph_rtl);
                let mut span_offset = 0.0;
                let mut style_index = None;
                for cluster in run.clusters() {
                    if style_index.is_some_and(|index| index != cluster.style_index())
                        || span.character_lengths.len() >= MAX_CHARACTERS_PER_SPAN
                    {
                        let advance = span.advance;
                        let next = SpanData::new(cluster.text_range().start);
                        self.publish_span(
                            std::mem::replace(&mut span, next),
                            std::mem::replace(
                                &mut node,
                                text_run_node(&run, alignment, paragraph_rtl),
                            ),
                            SpanGeometry::new(&run, metrics, run_offset, span_offset, advance),
                            &mut target,
                            &mut spans_started,
                            &mut next_node_id,
                            &mut previous,
                        );
                        span_offset += advance;
                        style_index = None;
                    }
                    if style_index.is_none() {
                        style_index = Some(cluster.style_index());
                        let style = cluster.style();
                        set_brush_properties(&mut node, style);
                        if let Some(locale) = &style.locale {
                            node.set_language(locale.as_str());
                        }
                    }
                    span.push(text, cluster);
                }
                let geometry =
                    SpanGeometry::new(&run, metrics, run_offset, span_offset, span.advance);
                self.publish_span(
                    span,
                    node,
                    geometry,
                    &mut target,
                    &mut spans_started,
                    &mut next_node_id,
                    &mut previous,
                );
            }
            if let Some((id, node)) = previous.take() {
                target.update.nodes.push((id, node));
                target.parent.push_child(id);
            }
        }
        self.spans.truncate(spans_started);
    }

    /// Convert a published AccessKit position to a Parley cursor. Unknown or invalid positions
    /// are rejected rather than silently moving to the end of the document.
    #[must_use = "invalid or stale accessible positions must be handled"]
    pub fn cursor_from_access_position<B: Brush>(
        &self,
        pos: &TextPosition,
        layout: &Layout<B>,
    ) -> Option<Cursor> {
        let span = self.spans.get(*self.span_by_id.get(&pos.node)?)?;
        let index = *span.boundaries.get(pos.character_index)?;
        if index > layout.text_len() {
            return None;
        }
        let affinity = if pos.character_index != 0
            && pos.character_index + 1 == span.boundaries.len()
            && !span.hard_break_end
        {
            Affinity::Upstream
        } else {
            Affinity::Downstream
        };
        Some(Cursor::from_byte_index(layout, index, affinity))
    }

    /// Convert a Parley cursor to its published AccessKit position.
    /// A shared boundary uses the cursor's affinity, except after a hard line
    /// break, where the following line (including a phantom line) owns the caret.
    #[must_use = "the accessible position may not have been published yet"]
    pub fn cursor_to_access_position(&self, cursor: Cursor) -> Option<TextPosition> {
        let index = cursor.index();
        let mut span_index = self
            .spans
            .partition_point(|span| span.boundaries[0] <= index)
            .checked_sub(1)?;
        if cursor.affinity() == Affinity::Upstream && span_index > 0 {
            let current = &self.spans[span_index];
            let previous = &self.spans[span_index - 1];
            if current.boundaries[0] == index
                && previous.boundaries.last() == Some(&index)
                && !previous.hard_break_end
            {
                span_index -= 1;
            }
        }
        let span = &self.spans[span_index];
        Some(TextPosition {
            node: span.id,
            character_index: span.boundaries.binary_search(&index).ok()?,
        })
    }

    /// Convert both endpoints of an accessible selection, preserving anchor and focus order.
    #[must_use = "invalid or stale accessible selections must be handled"]
    pub fn selection_from_access_selection<B: Brush>(
        &self,
        selection: &TextSelection,
        layout: &Layout<B>,
    ) -> Option<Selection> {
        Some(Selection::new(
            self.cursor_from_access_position(&selection.anchor, layout)?,
            self.cursor_from_access_position(&selection.focus, layout)?,
        ))
    }

    /// Convert both endpoints of a Parley selection, preserving anchor and focus order.
    #[must_use = "the accessible selection may not have been published yet"]
    pub fn selection_to_access_selection(&self, selection: &Selection) -> Option<TextSelection> {
        Some(TextSelection {
            anchor: self.cursor_to_access_position(selection.anchor())?,
            focus: self.cursor_to_access_position(selection.focus())?,
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "publish one span using the shared output state and node allocator"
    )]
    fn publish_span(
        &mut self,
        span: SpanData,
        mut node: Node,
        geometry: SpanGeometry,
        target: &mut TextAccessibilityTarget<'_>,
        spans_started: &mut usize,
        next_node_id: &mut impl FnMut() -> NodeId,
        previous: &mut Option<(NodeId, Node)>,
    ) {
        let id = self
            .spans
            .get(*spans_started)
            .map_or_else(next_node_id, |span| span.id);
        let mut span = span;
        let published = PublishedSpan {
            id,
            boundaries: std::mem::take(&mut span.boundaries),
            hard_break_end: span.hard_break_end,
        };
        if let Some(old) = self.spans.get_mut(*spans_started) {
            *old = published;
        } else {
            self.spans.push(published);
        }
        self.span_by_id.insert(id, *spans_started);
        *spans_started += 1;
        span.finish(&mut node, target.origin, geometry);
        if let Some((prev_id, mut prev)) = previous.take() {
            prev.set_next_on_line(id);
            node.set_previous_on_line(prev_id);
            target.update.nodes.push((prev_id, prev));
            target.parent.push_child(prev_id);
        }
        *previous = Some((id, node));
    }
}

fn text_run_node<B: Brush>(
    run: &Run<'_, B>,
    alignment: Option<Alignment>,
    paragraph_rtl: bool,
) -> Node {
    let mut node = Node::new(Role::TextRun);
    node.set_text_direction(if run.is_rtl() {
        TextDirection::RightToLeft
    } else {
        TextDirection::LeftToRight
    });

    let font = run.font();
    if let Ok(font_ref) = FontRef::from_index(font.data.as_ref(), font.index)
        && let Ok(name) = font_ref.name()
    {
        for n in name.name_record() {
            if n.name_id.get() == NameId::FAMILY_NAME {
                if let Ok(string) = n.string(name.string_data()) {
                    node.set_font_family(string.to_string());
                }
                break;
            }
        }
    }
    node.set_font_size(run.font_size());
    let attrs = run.font_attrs();
    node.set_font_weight(attrs.weight.value());
    if matches!(attrs.style, FontStyle::Italic | FontStyle::Oblique(_)) {
        node.set_italic();
    }
    if let Some(align) = alignment {
        node.set_text_align(match align {
            Alignment::Start if paragraph_rtl => TextAlign::Right,
            Alignment::Start | Alignment::Left => TextAlign::Left,
            Alignment::End if paragraph_rtl => TextAlign::Left,
            Alignment::End | Alignment::Right => TextAlign::Right,
            Alignment::Center => TextAlign::Center,
            Alignment::Justify => TextAlign::Justify,
        });
    }

    node
}

/// Exposes a [`PlainEditor`] to assistive technologies.
///
/// This wraps [`LayoutAccessibility`] with the editor-specific text selection.
#[derive(Clone, Default, Debug)]
pub struct PlainEditorAccessibility {
    layout_access: LayoutAccessibility,
    published_text: String,
}

impl PlainEditorAccessibility {
    /// Perform an accessibility update for `editor`.
    ///
    /// `node` is the node for the text input itself; the spans are pushed onto `update`
    /// as its children. See [`LayoutAccessibility::build_nodes`] for the remaining
    /// arguments.
    ///
    /// Returns `None` if the editor's layout is not up to date.
    pub fn build_nodes<B: Brush>(
        &mut self,
        editor: &PlainEditor<B>,
        target: TextAccessibilityTarget<'_>,
        next_node_id: impl FnMut() -> NodeId,
        set_brush_properties: impl Fn(&mut Node, &Style<B>),
    ) -> Option<()> {
        let layout = editor.try_layout()?;
        if self.published_text != editor.raw_text() {
            self.published_text.clear();
            self.published_text.push_str(editor.raw_text());
        }
        let TextAccessibilityTarget {
            update,
            parent: node,
            origin,
        } = target;
        self.layout_access.build_nodes(
            editor.raw_text(),
            layout,
            TextAccessibilityTarget {
                update,
                parent: node,
                origin,
            },
            next_node_id,
            set_brush_properties,
        );
        // The IME can ask for the caret to be hidden, in which case there is no
        // selection to report.
        if editor.is_cursor_visible()
            && let Some(selection) = self
                .layout_access
                .selection_to_access_selection(editor.raw_selection())
        {
            node.set_text_selection(selection);
        } else {
            node.clear_text_selection();
        }
        node.add_action(accesskit::Action::SetTextSelection);
        Some(())
    }

    /// Set the editor's selection from a selection over the nodes built by
    /// [`Self::build_nodes`].
    pub fn select_from_access_selection<B: Brush>(
        &self,
        driver: &mut PlainEditorDriver<'_, B>,
        selection: &TextSelection,
    ) {
        if self.published_text != driver.editor.raw_text() {
            return;
        }
        driver.refresh_layout();
        let selection = driver.editor.try_layout().and_then(|layout| {
            self.layout_access
                .selection_from_access_selection(selection, layout)
        });
        if let Some(selection) = selection {
            driver.set_selection(selection);
        }
    }
}

// A span's physical box runs right-to-left when the shaping run does. Character
// offsets inside it are always in the logical direction (AccessKit's convention).
#[derive(Clone, Copy)]
struct SpanGeometry {
    x: f32,
    top: f32,
    bottom: f32,
}

impl SpanGeometry {
    fn new<B: Brush>(
        run: &Run<'_, B>,
        metrics: &LineMetrics,
        run_offset: f32,
        span_offset: f32,
        advance: f32,
    ) -> Self {
        let offset = if run.is_rtl() {
            run.advance() - span_offset - advance
        } else {
            span_offset
        };
        Self {
            x: run_offset + offset,
            top: metrics.content_block_min_coord,
            bottom: metrics.content_block_max_coord,
        }
    }
}

struct SpanData {
    text: String,
    advance: f32,
    boundaries: Vec<usize>,
    hard_break_end: bool,
    character_lengths: Vec<u8>,
    character_positions: Vec<f32>,
    character_widths: Vec<f32>,
    word_starts: Vec<u8>,
}

impl SpanData {
    fn new(start: usize) -> Self {
        Self {
            text: String::new(),
            advance: 0.0,
            boundaries: vec![start],
            hard_break_end: false,
            character_lengths: Vec::new(),
            character_positions: Vec::new(),
            character_widths: Vec::new(),
            word_starts: Vec::new(),
        }
    }

    fn push<B: Brush>(&mut self, text: &str, cluster: Cluster<'_, B>) {
        self.hard_break_end = cluster.is_hard_line_break();
        let range = cluster.text_range();
        let cluster_text = &text[range.clone()];
        self.text.push_str(cluster_text);
        if cluster.is_word_boundary()
            && !cluster.is_space_or_nbsp()
            && let Ok(index) = u8::try_from(self.character_lengths.len())
        {
            self.word_starts.push(index);
        }
        // AccessKit stores each selectable unit's UTF-8 length in u8. Extremely
        // long graphemes must be represented by multiple UTF-8-safe pieces;
        // their interior positions still snap to Parley's actual caret boundary.
        let mut offset = 0;
        while offset < cluster_text.len() {
            let mut end = (offset + usize::from(u8::MAX)).min(cluster_text.len());
            while !cluster_text.is_char_boundary(end) {
                end -= 1;
            }
            let width = if offset == 0 { cluster.advance() } else { 0.0 };
            self.character_positions.push(self.advance);
            self.character_widths.push(width);
            self.advance += width;
            self.character_lengths
                .push(u8::try_from(end - offset).unwrap_or(u8::MAX));
            self.boundaries.push(range.start + end);
            offset = end;
        }
    }

    fn finish(self, node: &mut Node, origin: Point, geometry: SpanGeometry) {
        node.set_bounds(Rect {
            x0: origin.x + f64::from(geometry.x),
            y0: origin.y + f64::from(geometry.top),
            x1: origin.x + f64::from(geometry.x + self.advance),
            y1: origin.y + f64::from(geometry.bottom),
        });
        node.set_value(self.text);
        node.set_character_lengths(self.character_lengths);
        node.set_character_positions(self.character_positions);
        node.set_character_widths(self.character_widths);
        node.set_word_starts(self.word_starts);
    }
}
