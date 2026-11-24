// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::sync::mpsc;

use masonry_testing::{ModularWidget, TestHarness};

use crate::core::{NewWidget, WidgetTag};
use crate::theme::test_property_set;
use crate::widgets::SizedBox;

#[test]
fn mutate_order() {
    let parent_tag = WidgetTag::named("parent");
    let child = NewWidget::new(SizedBox::empty());
    let parent = NewWidget::new(ModularWidget::new_parent(child)).with_tag(parent_tag);
    let grandparent = NewWidget::new(ModularWidget::new_parent(parent));

    let (sender, receiver) = mpsc::channel::<u32>();
    let sender1 = sender.clone();
    let sender2 = sender.clone();
    let sender3 = sender;

    let mut harness = TestHarness::create(test_property_set(), grandparent);
    harness.edit_widget(parent_tag, move |mut parent| {
        parent.ctx.mutate_self_later(move |_| {
            sender2.send(2).unwrap();
        });

        parent
            .ctx
            .mutate_child_later(&mut parent.widget.state, move |_| {
                sender3.send(3).unwrap();
            });

        sender1.send(1).unwrap();
    });

    let values: Vec<_> = receiver.iter().collect();
    assert_eq!(values, [1, 2, 3]);
}

#[test]
fn cancel_mutate() {
    let parent_tag = WidgetTag::named("parent");
    let child = NewWidget::new(SizedBox::empty());
    let parent = NewWidget::new(SizedBox::new(child)).with_tag(parent_tag);

    let mut harness = TestHarness::create(test_property_set(), parent);
    harness.edit_widget(parent_tag, move |mut parent| {
        {
            let mut child = SizedBox::child_mut(&mut parent).unwrap();

            child.ctx.mutate_self_later(move |_| {
                panic!("This function should never get called");
            });
        }

        SizedBox::remove_child(&mut parent);
    });
}

#[test]
fn absent_or_mismatched_mutation_targets_do_not_run_callbacks() {
    use crate::core::WidgetMut;
    use crate::widgets::Label;

    let tag = WidgetTag::named("mutable-target");
    let missing_tag = WidgetTag::<SizedBox>::named("missing-target");
    let wrong_type = WidgetTag::<Label>::named("mutable-target");
    let child = NewWidget::new(SizedBox::empty()).with_tag(tag);
    let mut harness =
        TestHarness::create(test_property_set(), NewWidget::new(SizedBox::new(child)));
    let id = harness.get_widget(tag).id();
    let root = harness.render_root();
    assert_eq!(
        root.edit_widget_with_tag(tag, |widget| widget.id()),
        Some(id)
    );
    assert!(
        root.edit_widget_with_tag(missing_tag, |_| panic!("missing tag"))
            .is_none()
    );
    assert!(
        root.edit_widget_with_tag(wrong_type, |_| panic!("wrong type"))
            .is_none()
    );
    assert!(
        root.edit_widget(id, |_: WidgetMut<'_, Label>| panic!("wrong widget type"))
            .is_none()
    );
    assert!(root.get_layer_root(usize::MAX).is_none());
    assert!(
        root.edit_layer(usize::MAX, |_: WidgetMut<'_, SizedBox>| panic!(
            "missing layer"
        ))
        .is_none()
    );

    harness.edit_root_widget(|mut root| SizedBox::remove_child(&mut root));
    assert!(
        harness
            .render_root()
            .edit_widget(id, |_: WidgetMut<'_, SizedBox>| panic!("removed widget"))
            .is_none()
    );
}
