// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use masonry_testing::{TestHarness, assert_debug_panics};

use crate::core::{NewWidget, WidgetTag};
use crate::theme::test_property_set;
use crate::widgets::{Flex, SizedBox};

#[test]
fn duplicate_widget_tag() {
    let tag = WidgetTag::named("hello");

    let target = NewWidget::new(SizedBox::empty()).with_tag(tag);
    let parent = NewWidget::new(Flex::row().with_fixed(target));

    let mut harness = TestHarness::create(test_property_set(), parent);

    assert_debug_panics!(
        harness
            .edit_root_widget(|mut flex| {
                let new_child = NewWidget::new(SizedBox::empty()).with_tag(tag);
                Flex::add_fixed(&mut flex, new_child);
            })
            .unwrap(),
        "already exists in the widget tree"
    );
}

#[test]
fn removed_subtree_releases_all_tags() {
    let parent_tag = WidgetTag::named("parent");
    let child_tag = WidgetTag::named("child");
    let subtree = || {
        NewWidget::new(SizedBox::new(
            NewWidget::new(SizedBox::empty()).with_tag(child_tag),
        ))
        .with_tag(parent_tag)
    };
    let mut harness = TestHarness::create(
        test_property_set(),
        NewWidget::new(SizedBox::new(subtree())),
    );
    let old_parent = harness.get_widget(parent_tag).unwrap().id();
    let old_child = harness.get_widget(child_tag).unwrap().id();

    harness
        .edit_root_widget(|mut root| SizedBox::remove_child(&mut root))
        .unwrap();
    assert!(harness.get_widget_with_id(old_parent).is_none());
    assert!(harness.get_widget_with_id(old_child).is_none());
    assert!(
        harness
            .render_root()
            .get_widget_with_tag(parent_tag)
            .is_none()
    );
    assert!(
        harness
            .render_root()
            .get_widget_with_tag(child_tag)
            .is_none()
    );

    harness
        .edit_root_widget(|mut root| SizedBox::set_child(&mut root, subtree()))
        .unwrap();
    assert_ne!(harness.get_widget(parent_tag).unwrap().id(), old_parent);
    assert_ne!(harness.get_widget(child_tag).unwrap().id(), old_child);
}

#[test]
fn replacing_unregistered_child_in_same_edit_is_valid() {
    let tag = WidgetTag::named("final-child");
    let mut harness = TestHarness::create(test_property_set(), NewWidget::new(SizedBox::empty()));
    harness
        .edit_root_widget(|mut root| {
            SizedBox::set_child(&mut root, NewWidget::new(SizedBox::empty()).with_tag(tag));
            SizedBox::set_child(&mut root, NewWidget::new(SizedBox::empty()).with_tag(tag));
        })
        .unwrap();
    assert_eq!(harness.get_widget(tag).unwrap().children().len(), 0);
}
