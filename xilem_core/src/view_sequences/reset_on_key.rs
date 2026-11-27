// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::{
    AppendVec, Count, ElementSplice, MessageCtx, MessageResult, ViewElement, ViewId,
    ViewPathTracker, ViewSequence,
};

/// A sequence whose identity is reset whenever its key changes.
#[derive(Debug)]
#[must_use = "Use the sequence in the view tree."]
pub struct ResetOnKey<Key, Seq> {
    key: Key,
    seq: Seq,
}

/// Reconcile a sequence normally while its key remains equal. On a key change,
/// tear down the old sequence, build a fresh one, and invalidate its old message
/// paths, even if a previously used key returns later.
///
/// This is useful for positional lists whose entries represent independent
/// identities. It intentionally does not implement keyed moves between positions.
pub const fn reset_on_key<Key, Seq>(key: Key, seq: Seq) -> ResetOnKey<Key, Seq> {
    ResetOnKey { key, seq }
}

#[expect(
    unnameable_types,
    reason = "Implementation detail, public because of trait visibility rules"
)]
#[derive(Debug)]
pub struct ResetOnKeyState<S> {
    inner: S,
    generation: u64,
}

impl<State, Action, Context, Element, Key, Seq> ViewSequence<State, Action, Context, Element>
    for ResetOnKey<Key, Seq>
where
    State: 'static,
    Key: PartialEq + 'static,
    Context: ViewPathTracker,
    Element: ViewElement,
    Seq: ViewSequence<State, Action, Context, Element>,
{
    type SeqState = ResetOnKeyState<Seq::SeqState>;

    const ELEMENTS_COUNT: Count = Seq::ELEMENTS_COUNT;

    fn seq_build(
        &self,
        ctx: &mut Context,
        elements: &mut AppendVec<Element>,
        app_state: &mut State,
    ) -> Self::SeqState {
        let generation = 0;
        let inner = ctx.with_id(ViewId::new(generation), |ctx| {
            self.seq.seq_build(ctx, elements, app_state)
        });
        ResetOnKeyState { inner, generation }
    }

    fn seq_rebuild(
        &self,
        prev: &Self,
        state: &mut Self::SeqState,
        ctx: &mut Context,
        elements: &mut impl ElementSplice<Element>,
        app_state: &mut State,
    ) {
        if self.key == prev.key {
            ctx.with_id(ViewId::new(state.generation), |ctx| {
                self.seq
                    .seq_rebuild(&prev.seq, &mut state.inner, ctx, elements, app_state);
            });
        } else {
            ctx.with_id(ViewId::new(state.generation), |ctx| {
                prev.seq.seq_teardown(&mut state.inner, ctx, elements);
            });
            state.generation = state.generation.wrapping_add(1);
            state.inner = ctx.with_id(ViewId::new(state.generation), |ctx| {
                elements.with_scratch(|elements| self.seq.seq_build(ctx, elements, app_state))
            });
        }
    }

    fn seq_teardown(
        &self,
        state: &mut Self::SeqState,
        ctx: &mut Context,
        elements: &mut impl ElementSplice<Element>,
    ) {
        ctx.with_id(ViewId::new(state.generation), |ctx| {
            self.seq.seq_teardown(&mut state.inner, ctx, elements);
        });
    }

    fn seq_message(
        &self,
        state: &mut Self::SeqState,
        message: &mut MessageCtx,
        elements: &mut impl ElementSplice<Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        if message.take_first() != Some(ViewId::new(state.generation)) {
            return MessageResult::Stale;
        }
        self.seq
            .seq_message(&mut state.inner, message, elements, app_state)
    }
}
