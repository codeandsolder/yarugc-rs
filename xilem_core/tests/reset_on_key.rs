// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Regression coverage for identity-aware sequence replacement.
mod common;

use common::*;
use xilem_core::{DynMessage, MessageResult, View, reset_on_key};

#[test]
fn reset_only_changed_identity_and_never_revive_old_message_paths() {
    let make = |key, id| {
        sequence(
            0,
            (
                reset_on_key(key, OperationView::<0>(id)),
                OperationView::<0>(99),
            ),
        )
    };
    let first = make(1, 10);
    let mut ctx = TestCtx::default();
    let (mut element, mut state) = first.build(&mut ctx, &mut ());
    let path = element.children.as_ref().unwrap().active[0]
        .view_path
        .clone();

    let update = make(1, 11);
    update.rebuild(&first, &mut state, &mut ctx, &mut element, &mut ());
    let children = element.children.as_ref().unwrap();
    assert_eq!(
        children.active[0].operations,
        [
            Operation::Build(10),
            Operation::Rebuild { from: 10, to: 11 }
        ]
    );
    assert_eq!(children.active[0].view_path, path);
    assert!(children.deleted.is_empty());

    let second = make(2, 20);
    second.rebuild(&update, &mut state, &mut ctx, &mut element, &mut ());
    let children = element.children.as_ref().unwrap();
    assert_eq!(children.active[0].operations, [Operation::Build(20)]);
    assert_ne!(children.active[0].view_path, path);
    assert_eq!(children.deleted.len(), 1);
    assert_eq!(children.active[1].operations[0], Operation::Build(99));
    assert_eq!(children.active[1].operations.len(), 3);

    let returned = make(1, 30);
    returned.rebuild(&second, &mut state, &mut ctx, &mut element, &mut ());
    let children = element.children.as_ref().unwrap();
    let new_path = children.active[0].view_path.clone();
    assert_ne!(new_path, path);
    ctx.with_message_context(path, DynMessage::new(()), |ctx| {
        assert!(matches!(
            returned.message(&mut state, ctx, &mut element, &mut ()),
            MessageResult::Stale
        ));
    });
    ctx.with_message_context(new_path, DynMessage::new(()), |ctx| {
        assert_action(returned.message(&mut state, ctx, &mut element, &mut ()), 30);
    });
    ctx.assert_empty();
}
