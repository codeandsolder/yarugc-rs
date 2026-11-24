// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use core::marker::PhantomData;

use crate::element::NoElement;
use crate::{
    AppendVec, Count, ElementSplice, MessageCtx, MessageResult, NoElements, ViewElement,
    ViewPathTracker, ViewSequence,
};

type SequenceSignature<State, Action, Context> = fn() -> (State, Action, Context);

/// The [`ViewSequence`] for [`without_elements`], see its documentation for more context.
#[derive(Debug)]
pub struct WithoutElements<Seq, State, Action, Context> {
    seq: Seq,
    phantom: PhantomData<SequenceSignature<State, Action, Context>>,
}

/// An adapter which turns a [`ViewSequence`] containing any number of views with side effects, into a `ViewSequence` with any element type.
///
/// The returned `ViewSequence` will not contain any elements, and will instead run the side effects
/// of the wrapped `ViewSequence` when built and/or rebuilt.
///
/// This can be used to embed side-effects naturally into the flow of your program.
/// This can be used as an alternative to [`fork`](crate::fork) , which avoids adding extra nesting at the cost of only being usable in places where there is already an existing sequence.
///
/// # Examples
///
/// ```
/// # use xilem_core::docs::{DocsViewSequence as WidgetViewSequence, some_component_generic as component};
/// use xilem_core::{without_elements, run_once};
///
/// fn isolated_child(state: &mut AppState) -> impl WidgetViewSequence<AppState> {
///     (component(state), without_elements(run_once(|| {})))
/// }
///
/// # struct AppState;
/// ```
pub fn without_elements<State, Action, Context, Seq>(
    seq: Seq,
) -> WithoutElements<Seq, State, Action, Context>
where
    State: 'static,
    Action: 'static,
    Context: ViewPathTracker + 'static,
    Seq: ViewSequence<State, Action, Context, NoElement>,
{
    WithoutElements {
        seq,
        phantom: PhantomData,
    }
}

impl<State, Action, Context, Element, Seq> ViewSequence<State, Action, Context, Element>
    for WithoutElements<Seq, State, Action, Context>
where
    State: 'static,
    Action: 'static,
    Element: ViewElement,
    Context: ViewPathTracker + 'static,
    Seq: ViewSequence<State, Action, Context, NoElement>,
{
    type SeqState = Seq::SeqState;

    const ELEMENTS_COUNT: Count = Count::Zero;

    fn seq_build(
        &self,
        ctx: &mut Context,
        _elements: &mut AppendVec<Element>,
        app_state: &mut State,
    ) -> Self::SeqState {
        self.seq
            .seq_build(ctx, &mut AppendVec::default(), app_state)
    }

    fn seq_rebuild(
        &self,
        prev: &Self,
        seq_state: &mut Self::SeqState,
        ctx: &mut Context,
        _elements: &mut impl ElementSplice<Element>,
        app_state: &mut State,
    ) {
        self.seq
            .seq_rebuild(&prev.seq, seq_state, ctx, &mut NoElements, app_state);
    }

    fn seq_teardown(
        &self,
        seq_state: &mut Self::SeqState,
        ctx: &mut Context,
        _elements: &mut impl ElementSplice<Element>,
    ) {
        self.seq.seq_teardown(seq_state, ctx, &mut NoElements);
    }

    fn seq_message(
        &self,
        seq_state: &mut Self::SeqState,
        message: &mut MessageCtx,
        _elements: &mut impl ElementSplice<Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        self.seq
            .seq_message(seq_state, message, &mut NoElements, app_state)
    }
}
