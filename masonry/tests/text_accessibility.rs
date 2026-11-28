// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use masonry::accesskit::{Node, NodeId, Role, TextPosition, TreeId, TreeUpdate};
use masonry::core::{
    BrushIndex, LayoutAccessibility, PlainEditorAccessibility, TextAccessibilityTarget,
};
use masonry::kurbo::Point;
use masonry::parley::editing::Cursor;
use masonry::parley::layout::Affinity;
use masonry::parley::{
    FontContext, FontFamily, FontFamilyName, Layout, LayoutContext, PlainEditor, StyleProperty,
};
use masonry::peniko::Blob;
use masonry::testing::ROBOTO;

fn text_contexts() -> (FontContext, LayoutContext<BrushIndex>) {
    let mut fonts = FontContext::new();
    fonts
        .collection
        .register_fonts(Blob::new(Arc::new(ROBOTO)), None);
    (fonts, LayoutContext::new())
}

fn layout(text: &str, width: Option<f32>) -> Layout<BrushIndex> {
    let (mut fonts, mut context) = text_contexts();
    let mut builder = context.ranged_builder(&mut fonts, text, 1.0, false);
    builder.push_default(StyleProperty::FontFamily(FontFamily::Single(
        FontFamilyName::named("Roboto"),
    )));
    builder.push_default(StyleProperty::FontSize(16.0));
    let mut result = builder.build(text);
    result.break_all_lines(width);
    result
}

fn update() -> TreeUpdate {
    TreeUpdate {
        nodes: Vec::new(),
        tree: None,
        tree_id: TreeId::ROOT,
        focus: NodeId(0),
    }
}

fn publish(
    bridge: &mut LayoutAccessibility,
    text: &str,
    layout: &Layout<BrushIndex>,
    next: &mut u64,
) -> (TreeUpdate, Node) {
    let mut update = update();
    let mut parent = Node::new(Role::Label);
    bridge.build_nodes(
        text,
        layout,
        TextAccessibilityTarget {
            update: &mut update,
            parent: &mut parent,
            origin: Point::new(7.0, 9.0),
        },
        || {
            *next += 1;
            NodeId(*next)
        },
        |_, _| {},
    );
    (update, parent)
}

#[test]
fn split_spans_publish_every_byte_and_roundtrip_line_boundaries() {
    let text = format!("{}\nsecond paragraph\n", "é".repeat(300));
    let layout = layout(&text, None);
    let mut bridge = LayoutAccessibility::default();
    let mut next = 0;
    let (first, parent) = publish(&mut bridge, &text, &layout, &mut next);
    assert!(first.nodes.len() >= 4);
    let recovered: String = first
        .nodes
        .iter()
        .map(|(_, node)| node.value().unwrap_or_default())
        .collect();
    assert_eq!(recovered, text);
    for (id, node) in &first.nodes {
        let value = node.value().unwrap_or_default();
        let lengths = node.character_lengths();
        assert_eq!(
            lengths.iter().map(|&len| usize::from(len)).sum::<usize>(),
            value.len()
        );
        assert_eq!(node.character_positions().len(), lengths.len());
        assert_eq!(node.character_widths().len(), lengths.len());
        let end = TextPosition {
            node: *id,
            character_index: lengths.len(),
        };
        let cursor = bridge.cursor_from_access_position(&end, &layout).unwrap();
        let back = bridge.cursor_to_access_position(cursor).unwrap();
        assert_eq!(
            bridge
                .cursor_from_access_position(&back, &layout)
                .unwrap()
                .index(),
            cursor.index()
        );
        assert!(
            bridge
                .cursor_from_access_position(
                    &TextPosition {
                        node: *id,
                        character_index: lengths.len() + 1
                    },
                    &layout
                )
                .is_none()
        );
    }
    let first_id = first.nodes[0].0;
    let first_len = first.nodes[0].1.value().unwrap().len();
    assert!(first_len < text.len());
    assert_eq!(
        bridge
            .cursor_from_access_position(
                &TextPosition {
                    node: first_id,
                    character_index: first.nodes[0].1.character_lengths().len()
                },
                &layout
            )
            .unwrap()
            .index(),
        first_len
    );
    let end = Cursor::from_byte_index(&layout, text.len(), Affinity::Upstream);
    let last = bridge.cursor_to_access_position(end).unwrap();
    assert_eq!(last.node, *parent.children().last().unwrap());
    assert_eq!(last.character_index, 0);
    let (second, _) = publish(&mut bridge, &text, &layout, &mut next);
    assert_eq!(
        first.nodes.iter().map(|(id, _)| id).collect::<Vec<_>>(),
        second.nodes.iter().map(|(id, _)| id).collect::<Vec<_>>()
    );
}

#[test]
fn wrapped_bidi_runs_have_valid_geometry_and_logical_text() {
    let text = "left שלום right\n";
    let layout = layout(text, Some(65.0));
    let mut bridge = LayoutAccessibility::default();
    let (update, _) = publish(&mut bridge, text, &layout, &mut 0);
    let recovered: String = update
        .nodes
        .iter()
        .map(|(_, node)| node.value().unwrap_or_default())
        .collect();
    assert_eq!(recovered, text);
    for (_, node) in update.nodes {
        let bounds = node.bounds().unwrap();
        assert!(bounds.x0.is_finite() && bounds.x1 >= bounds.x0);
        assert!(bounds.y0.is_finite() && bounds.y1 >= bounds.y0);
    }
}

#[test]
fn editor_ignores_accessible_selection_after_text_changes() {
    let (mut fonts, mut context) = text_contexts();
    let mut editor = PlainEditor::<BrushIndex>::new(16.0);
    editor.set_text("hello world");
    editor.refresh_layout(&mut fonts, &mut context);
    let mut bridge = PlainEditorAccessibility::default();
    let mut update = update();
    let mut parent = Node::new(Role::TextInput);
    let mut next = 0;
    bridge
        .build_nodes(
            &editor,
            TextAccessibilityTarget {
                update: &mut update,
                parent: &mut parent,
                origin: Point::ORIGIN,
            },
            || {
                next += 1;
                NodeId(next)
            },
            |_, _| {},
        )
        .unwrap();
    let position = TextPosition {
        node: update.nodes[0].0,
        character_index: 2,
    };
    let selection = masonry::accesskit::TextSelection {
        anchor: position,
        focus: position,
    };
    bridge.select_from_access_selection(&mut editor.driver(&mut fonts, &mut context), &selection);
    assert_eq!(editor.raw_selection().focus().index(), 2);
    editor.set_text("other text");
    let before = *editor.raw_selection();
    bridge.select_from_access_selection(&mut editor.driver(&mut fonts, &mut context), &selection);
    assert_eq!(editor.raw_selection(), &before);
}
