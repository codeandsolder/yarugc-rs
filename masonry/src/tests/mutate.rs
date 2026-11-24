// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::sync::mpsc;

use masonry_testing::{ModularWidget, TestHarness};

use crate::core::{NewWidget, Widget, WidgetTag};
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
        parent.ctx.mutate_self_later::<dyn Widget>(move |_| {
            sender2.send(2).unwrap();
        });

        parent
            .ctx
            .mutate_child_later(&parent.widget.state, move |_| {
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
            let mut child = SizedBox::child_mut::<dyn Widget>(&mut parent).unwrap();

            child.ctx.mutate_self_later::<dyn Widget>(move |_| {
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

    let missing = NewWidget::new(SizedBox::empty()).id();
    let (send, recv) = mpsc::channel();
    harness.edit_root_widget(|mut root| {
        root.ctx
            .mutate_later::<Label>(id, |_| panic!("wrong deferred type"));
        root.ctx
            .mutate_later::<SizedBox>(missing, |_| panic!("absent deferred target"));
        root.ctx.mutate_later::<SizedBox>(id, move |widget| {
            send.send(widget.id()).unwrap();
        });
    });
    assert_eq!(recv.try_recv().unwrap(), id);

    harness.edit_root_widget(|mut root| SizedBox::remove_child(&mut root));
    assert!(
        harness
            .render_root()
            .edit_widget(id, |_: WidgetMut<'_, SizedBox>| panic!("removed widget"))
            .is_none()
    );
}

#[test]
fn typed_child_lookup_respects_registration_and_collection_slots() {
    use crate::core::CollectionWidget;
    use crate::layout::AsUnit;
    use crate::widgets::{Flex, Label};

    let label_tag = WidgetTag::named("old-label");
    let new_tag = WidgetTag::named("new-label");
    let parent = Flex::row()
        .with_fixed(NewWidget::new(Label::new("old")).with_tag(label_tag))
        .with_fixed_spacer(10.px())
        .prepare();
    let mut harness = TestHarness::create(test_property_set(), parent);
    let old_id = harness.get_widget(label_tag).id();

    harness.edit_root_widget(|mut parent| {
        let root_id = parent.id();
        assert_eq!(Flex::get_mut::<Label>(&mut parent, 0).unwrap().id(), old_id);
        assert!(Flex::get_mut::<SizedBox>(&mut parent, 0).is_none());
        assert!(Flex::get_mut::<dyn Widget>(&mut parent, 1).is_none());
        assert!(Flex::get_mut::<dyn Widget>(&mut parent, usize::MAX).is_none());
        assert!(parent.ctx.get_mut::<Flex>(root_id).is_none());

        let replacement = NewWidget::new(Label::new("new")).with_tag(new_tag);
        let new_id = replacement.id();
        Flex::set(&mut parent, 0, replacement, 0.0).unwrap();
        assert!(parent.ctx.get_mut::<Label>(old_id).is_none());
        assert!(parent.ctx.get_mut::<Label>(new_id).is_none());
        assert!(Flex::get_mut::<Label>(&mut parent, 0).is_none());
    });

    let new_id = harness.get_widget(new_tag).id();
    harness.edit_root_widget(|mut parent| {
        assert_eq!(Flex::get_mut::<Label>(&mut parent, 0).unwrap().id(), new_id);
        assert_eq!(
            parent.ctx.get_mut::<dyn Widget>(new_id).unwrap().id(),
            new_id
        );
        assert!(parent.ctx.get_mut::<Label>(old_id).is_none());
    });
}

#[test]
fn rejected_collection_mutations_leave_slots_and_widgets_intact() {
    use crate::core::{CollectionError, CollectionWidget};
    use crate::layout::AsUnit;
    use crate::widgets::{Flex, Label};

    let tag = WidgetTag::named("retained-child");
    let parent = Flex::row()
        .with_fixed(NewWidget::new(Label::new("original")).with_tag(tag))
        .with_fixed_spacer(17.px())
        .prepare();
    let mut harness = TestHarness::create(test_property_set(), parent);
    let original = harness.get_widget(tag).id();
    let original_size = harness.root_widget().ctx().border_box();
    harness.edit_root_widget(|mut parent| {
        let error = CollectionError::OutOfBounds { index: 3, len: 2 };
        assert_eq!(
            Flex::insert(&mut parent, 3, Label::new("invalid").prepare(), 0.),
            Err(error)
        );
        assert_eq!(
            Flex::set(&mut parent, 3, Label::new("invalid").prepare(), 0.),
            Err(error)
        );
        assert_eq!(Flex::remove(&mut parent, 3), Err(error));
        assert_eq!(Flex::swap(&mut parent, 0, 3), Err(error));
        assert_eq!(
            Flex::set_params(&mut parent, 1, 2.),
            Err(CollectionError::Spacer { index: 1 })
        );
        assert_eq!(Flex::set_fixed_spacer(&mut parent, 3, 1.px()), Err(error));
        assert_eq!(
            Flex::get_mut::<Label>(&mut parent, 0).unwrap().id(),
            original
        );
        assert_eq!(parent.widget.len(), 2);
    });
    assert_eq!(harness.get_widget(tag).id(), original);
    assert_eq!(harness.root_widget().ctx().border_box(), original_size);
}

#[test]
fn indexed_stack_insert_into_empty_and_swap_keep_a_valid_active_child() {
    use crate::core::{CollectionError, CollectionWidget};
    use crate::widgets::{IndexedStack, Label};

    let mut harness = TestHarness::create(test_property_set(), IndexedStack::new().prepare());
    harness.edit_root_widget(|mut stack| {
        IndexedStack::insert(&mut stack, 0, Label::new("first").prepare(), ()).unwrap();
        assert_eq!(stack.widget.active_child(), 0);
        IndexedStack::insert(&mut stack, 0, Label::new("second").prepare(), ()).unwrap();
        assert_eq!(stack.widget.active_child(), 1);
        assert_eq!(
            IndexedStack::set_active_child(&mut stack, 2),
            Err(CollectionError::OutOfBounds { index: 2, len: 2 })
        );
        assert_eq!(stack.widget.active_child(), 1);
        IndexedStack::swap(&mut stack, 0, 1).unwrap();
        assert_eq!(stack.widget.active_child(), 0);
        IndexedStack::remove(&mut stack, 0).unwrap();
        assert_eq!(stack.widget.active_child(), 0);
    });
}
