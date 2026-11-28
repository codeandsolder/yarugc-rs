// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::assert_matches;

use masonry_testing::{ModularWidget, Record, TestHarness, TestWidgetExt, assert_any, assert_none};

use crate::core::{NewWidget, Widget, WidgetTag};
use crate::theme::test_property_set;
use crate::widgets::SizedBox;

#[test]
fn request_accessibility() {
    let target_tag = WidgetTag::named("target");
    let parent_tag = WidgetTag::named("parent");
    let child = NewWidget::new(SizedBox::empty().record()).with_tag(target_tag);
    let parent = NewWidget::new(ModularWidget::new_parent(child).record()).with_tag(parent_tag);
    let grandparent = NewWidget::new(ModularWidget::new_parent(parent));

    let mut harness = TestHarness::create(test_property_set(), grandparent);
    harness.flush_records_of(target_tag).unwrap();
    harness.flush_records_of(parent_tag).unwrap();

    harness
        .edit_widget(target_tag, |mut widget| {
            widget.ctx.request_accessibility_update();
        })
        .unwrap();
    let _ = harness.render().unwrap();

    // Check that `Widget::accessibility()` is called for the child (which did request it)
    // but not the parent (which did not).
    let records = harness.take_records_of(target_tag).unwrap();
    assert_any(records, |r| matches!(r, Record::Accessibility));

    let records = harness.take_records_of(parent_tag).unwrap();
    assert_none(records, |r| matches!(r, Record::Accessibility));

    // Check that `Widget::accessibility()` is not called: neither node has requested
    // an accessibility update.
    let _ = harness.render().unwrap();
    assert_matches!(harness.take_records_of(target_tag).unwrap()[..], []);
    assert_matches!(harness.take_records_of(parent_tag).unwrap()[..], []);
}

#[test]
fn access_node_children() {
    let parent_tag = WidgetTag::named("parent");

    let child_1 = NewWidget::new(SizedBox::empty());
    let child_2 = NewWidget::new(SizedBox::empty());
    let child_3 = NewWidget::new(SizedBox::empty());

    let parent = NewWidget::new(ModularWidget::new_multi_parent(vec![
        child_1, child_2, child_3,
    ]))
    .with_tag(parent_tag);
    let grandparent = NewWidget::new(ModularWidget::new_parent(parent));

    let mut harness = TestHarness::create(test_property_set(), grandparent);
    let _ = harness.render().unwrap();

    let parent_ref = harness.get_widget(parent_tag).unwrap();
    let parent_node_id = parent_ref.id();
    let [id_1, id_2, id_3] = parent_ref.inner().children_ids()[..] else {
        unreachable!()
    };

    let parent_node = harness.access_node(parent_node_id).unwrap();
    assert_eq!(
        Vec::<u64>::from_iter(parent_node.children().map(|node| node.locate().0.0)),
        vec![id_1.to_raw(), id_2.to_raw(), id_3.to_raw()]
    );

    // We stash a child
    harness
        .edit_widget(parent_tag, |mut parent| {
            parent.ctx.set_stashed(&parent.widget.state[1], true);
        })
        .unwrap();
    let _ = harness.render().unwrap();

    // Stash child is not included
    let parent_node = harness.access_node(parent_node_id).unwrap();
    assert_eq!(
        Vec::<u64>::from_iter(parent_node.children().map(|node| node.locate().0.0)),
        vec![id_1.to_raw(), id_3.to_raw()]
    );

    harness
        .edit_widget(parent_tag, |mut parent| {
            parent.ctx.set_stashed(&parent.widget.state[1], false);
        })
        .unwrap();
    let _ = harness.render().unwrap();
    let parent_node = harness.access_node(parent_node_id).unwrap();
    assert_eq!(
        Vec::<u64>::from_iter(parent_node.children().map(|node| node.locate().0.0)),
        vec![id_1.to_raw(), id_2.to_raw(), id_3.to_raw()]
    );
}
