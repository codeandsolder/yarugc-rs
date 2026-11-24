// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use alloc::vec::Vec;

use crate::{
    AppendVec, Count, ElementSplice, MessageCtx, MessageResult, ViewElement, ViewId,
    ViewPathTracker, ViewSequence,
};

/// The state used to implement `ViewSequence` for `Vec<impl ViewSequence>`
///
/// Each vector child has two routing segments: its index and a full-width generation.
#[doc(hidden)]
#[expect(
    unnameable_types,
    reason = "Implementation detail, public because of trait visibility rules"
)]
#[derive(Debug)]
pub struct VecViewState<InnerState> {
    // We use two vectors here because the `inner_states` is the
    // same length as the actual vector, whereas the generations
    // is the same length as the longest version of this we have seen.
    /// The fields of the tuple are (number of widgets to skip, `InnerState`).
    inner_states: Vec<(usize, InnerState)>,

    generations: Vec<u64>,
}

// Use independent path segments to avoid limiting the index or generation to 32 bits.
fn with_child_id<Context: ViewPathTracker, R>(
    ctx: &mut Context,
    index: usize,
    generation: u64,
    f: impl FnOnce(&mut Context) -> R,
) -> R {
    ctx.with_id(ViewId::new(index as u64), |ctx| {
        ctx.with_id(ViewId::new(generation), f)
    })
}

/// The implementation for an `Vec` of a `ViewSequence`.
///
/// Will mark messages which were sent to any index as stale if
/// that index has been unused in the meantime.
impl<State, Action, Context, Element, Seq> ViewSequence<State, Action, Context, Element>
    for Vec<Seq>
where
    State: 'static,
    Seq: ViewSequence<State, Action, Context, Element>,
    Context: ViewPathTracker,
    Element: ViewElement,
{
    // We hide all the items in these implementation so that the top-level
    // comment is always shown. This lets us explain the caveats.
    #[doc(hidden)]
    type SeqState = VecViewState<Seq::SeqState>;

    #[doc(hidden)]
    const ELEMENTS_COUNT: Count = Seq::ELEMENTS_COUNT.multiple();

    #[doc(hidden)]
    fn seq_build(
        &self,
        ctx: &mut Context,
        elements: &mut AppendVec<Element>,
        app_state: &mut State,
    ) -> Self::SeqState {
        let start_idx = elements.index();
        let generations = alloc::vec![0; self.len()];
        let inner_states = self
            .iter()
            .enumerate()
            .zip(&generations)
            .map(|((index, seq), generation)| {
                let this_skip = elements.index() - start_idx;
                let inner_state = with_child_id(ctx, index, *generation, |ctx| {
                    seq.seq_build(ctx, elements, app_state)
                });
                (this_skip, inner_state)
            })
            .collect();
        VecViewState {
            inner_states,
            generations,
        }
    }

    #[doc(hidden)]
    fn seq_rebuild(
        &self,
        prev: &Self,
        seq_state: &mut Self::SeqState,
        ctx: &mut Context,
        elements: &mut impl ElementSplice<Element>,
        app_state: &mut State,
    ) {
        let start_idx = elements.index();
        for (i, (((child, child_prev), (child_skip, child_state)), child_generation)) in self
            .iter()
            .zip(prev)
            .zip(&mut seq_state.inner_states)
            .zip(&seq_state.generations)
            .enumerate()
        {
            *child_skip = elements.index() - start_idx;
            // Rebuild the items which are common to both vectors
            with_child_id(ctx, i, *child_generation, |ctx| {
                child.seq_rebuild(child_prev, child_state, ctx, elements, app_state);
            });
        }
        let n = self.len();
        let prev_n = prev.len();
        if n < prev_n {
            let to_teardown = prev[n..].iter();
            // Keep the generations
            let generations = seq_state.generations[n..].iter_mut();
            // But remove the old states
            let states = seq_state.inner_states.drain(n..);
            for (index, ((old_seq, generation), (_, mut inner_state))) in
                to_teardown.zip(generations).zip(states).enumerate()
            {
                with_child_id(ctx, index + n, *generation, |ctx| {
                    old_seq.seq_teardown(&mut inner_state, ctx, elements);
                });
                // We increment the generation on the "falling edge" by convention
                *generation = generation.wrapping_add(1);
            }
        } else if n > prev_n {
            // If needed, create new generations
            if n > seq_state.generations.len() {
                seq_state.generations.resize(n, 0);
            }
            let outer_idx = elements.index();
            elements.with_scratch(|elements| {
                seq_state.inner_states.extend(
                    self[prev_n..]
                        .iter()
                        .zip(&seq_state.generations[prev_n..])
                        .enumerate()
                        .map(|(index, (seq, generation))| {
                            let this_skip = elements.index() + outer_idx - start_idx;
                            let inner_state =
                                with_child_id(ctx, index + prev_n, *generation, |ctx| {
                                    seq.seq_build(ctx, elements, app_state)
                                });
                            (this_skip, inner_state)
                        }),
                );
            });
        }
    }

    #[doc(hidden)]
    fn seq_teardown(
        &self,
        seq_state: &mut Self::SeqState,
        ctx: &mut Context,
        elements: &mut impl ElementSplice<Element>,
    ) {
        for (index, ((seq, (_, state)), generation)) in self
            .iter()
            .zip(&mut seq_state.inner_states)
            .zip(&seq_state.generations)
            .enumerate()
        {
            with_child_id(ctx, index, *generation, |ctx| {
                seq.seq_teardown(state, ctx, elements);
            });
        }
    }

    #[doc(hidden)]
    fn seq_message(
        &self,
        seq_state: &mut Self::SeqState,
        message: &mut MessageCtx,
        elements: &mut impl ElementSplice<Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        let Some(index) = message
            .take_first()
            .and_then(|id| usize::try_from(id.routing_id()).ok())
        else {
            return MessageResult::Stale;
        };
        let Some(generation) = message.take_first() else {
            return MessageResult::Stale;
        };
        if seq_state.generations.get(index).copied() != Some(generation.routing_id()) {
            return MessageResult::Stale;
        }
        let Some((seq, (skip, inner_state))) =
            self.get(index).zip(seq_state.inner_states.get_mut(index))
        else {
            return MessageResult::Stale;
        };
        elements.skip(*skip);
        seq.seq_message(inner_state, message, elements, app_state)
    }
}
