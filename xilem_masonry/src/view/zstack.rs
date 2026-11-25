// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::marker::PhantomData;

use masonry::core::{CollectionWidget, FromDynWidget, Widget, WidgetMut};
use masonry::layout::UnitPoint;
use masonry::widgets;
pub use masonry::widgets::ChildAlignment;

use crate::core::{
    AppendVec, ElementSplice, MessageCtx, MessageResult, Mut, SuperElement, View, ViewElement,
    ViewMarker, ViewSequence,
};
use crate::{Pod, ViewCtx, WidgetView};

/// A widget that lays out its children on top of each other.
/// The children are laid out back to front.
///
/// # Example
///
/// This example shows how to add two text labels on top of each other.
///
/// ```
/// # use xilem_masonry as xilem;
/// use xilem::WidgetView;
/// use xilem::view::{zstack, label, text_button};
/// fn view<State: 'static>() -> impl WidgetView<State> {
///     zstack((
///         label("Background"),
///         text_button("Click me", |_| {})
///     ))
/// }
/// ```
pub const fn zstack<State: 'static, Action, Seq: ZStackSequence<State, Action>>(
    sequence: Seq,
) -> ZStack<Seq> {
    ZStack {
        sequence,
        alignment: UnitPoint::CENTER,
    }
}

/// A view container that lays the child widgets on top of each other.
///
/// See [`zstack`] for more details.
#[must_use = "View values do nothing unless provided to Xilem."]
pub struct ZStack<Seq> {
    sequence: Seq,
    alignment: UnitPoint,
}

impl<Seq> ZStack<Seq> {
    /// Changes the alignment of the children.
    pub fn alignment(mut self, alignment: impl Into<UnitPoint>) -> Self {
        self.alignment = alignment.into();
        self
    }
}

mod hidden {
    use super::ZStackElement;
    use crate::core::AppendVec;

    #[doc(hidden)]
    #[expect(
        unnameable_types,
        reason = "Implementation detail, public because of trait visibility rules"
    )]
    pub struct ZStackState<SeqState> {
        pub(crate) seq_state: SeqState,
        pub(crate) scratch: AppendVec<ZStackElement>,
    }
}

use hidden::ZStackState;

impl<Seq> ViewMarker for ZStack<Seq> {}
impl<State, Action, Seq> View<State, Action, ViewCtx> for ZStack<Seq>
where
    State: 'static,
    Action: 'static,
    Seq: ZStackSequence<State, Action>,
{
    type Element = Pod<widgets::ZStack>;

    type ViewState = ZStackState<Seq::SeqState>;

    fn build(&self, ctx: &mut ViewCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        let mut elements = AppendVec::default();
        let mut widget = widgets::ZStack::new().with_alignment(self.alignment);
        let seq_state = self.sequence.seq_build(ctx, &mut elements, app_state);
        for child in elements.drain() {
            widget = widget.with(child.widget.new_widget, child.alignment);
        }
        let pod = Pod::new(widget);
        (
            pod,
            ZStackState {
                seq_state,
                scratch: elements,
            },
        )
    }

    fn rebuild(
        &self,
        prev: &Self,
        ZStackState { seq_state, scratch }: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        if self.alignment != prev.alignment {
            widgets::ZStack::set_alignment(&mut element, self.alignment);
        }

        let mut splice = ZStackSplice::new(element, scratch);
        self.sequence
            .seq_rebuild(&prev.sequence, seq_state, ctx, &mut splice, app_state);
        debug_assert!(scratch.is_empty());
    }

    fn teardown(
        &self,
        ZStackState { seq_state, scratch }: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        element: Mut<'_, Self::Element>,
    ) {
        let mut splice = ZStackSplice::new(element, scratch);
        self.sequence.seq_teardown(seq_state, ctx, &mut splice);
        debug_assert!(scratch.is_empty());
    }

    fn message(
        &self,
        ZStackState { seq_state, scratch }: &mut Self::ViewState,
        message: &mut MessageCtx,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        let mut splice = ZStackSplice::new(element, scratch);
        let result = self
            .sequence
            .seq_message(seq_state, message, &mut splice, app_state);
        debug_assert!(scratch.is_empty());
        result
    }
}

// --- MARK: ZStackExt

/// A trait that extends a [`WidgetView`] with methods to provide parameters for a parent [`ZStack`].
pub trait ZStackExt<State: 'static, Action>: WidgetView<State, Action> {
    /// Applies [`ChildAlignment`] to this view.
    /// This allows the view to override the default alignment of the parent [`ZStack`].
    /// This can only be used on views that are direct children of a [`ZStack`].
    fn alignment(self, alignment: impl Into<ChildAlignment>) -> ZStackItem<Self, State, Action>
    where
        State: 'static,
        Action: 'static,
        Self: Sized,
    {
        zstack_item(self, alignment)
    }
}

impl<State: 'static, Action, V: WidgetView<State, Action>> ZStackExt<State, Action> for V {}

/// A wrapper around a [`WidgetView`], with a specified [`ChildAlignment`].
/// This struct is most often constructed indirectly using [`ZStackExt::alignment`].
pub struct ZStackItem<V, State, Action> {
    view: V,
    alignment: ChildAlignment,
    phantom: PhantomData<fn() -> (State, Action)>,
}

/// Constructs a new `ZStackItem`.
/// See also [`ZStackExt::alignment`], for constructing a `ZStackItem` from an existing view.
pub fn zstack_item<V, State, Action>(
    view: V,
    alignment: impl Into<ChildAlignment>,
) -> ZStackItem<V, State, Action>
where
    State: 'static,
    Action: 'static,
    V: WidgetView<State, Action>,
{
    ZStackItem {
        view,
        alignment: alignment.into(),
        phantom: PhantomData,
    }
}

impl<V, State, Action> ViewMarker for ZStackItem<V, State, Action> {}

impl<State, Action, V> View<State, Action, ViewCtx> for ZStackItem<V, State, Action>
where
    State: 'static,
    Action: 'static,
    V: WidgetView<State, Action>,
{
    type Element = ZStackElement;

    type ViewState = V::ViewState;

    fn build(&self, ctx: &mut ViewCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        let (pod, state) = self.view.build(ctx, app_state);
        (ZStackElement::new(pod.erased(), self.alignment), state)
    }

    indexed_item_lifecycle!(ZStack, alignment);
}

// --- MARK: ZStackElement

/// A struct implementing [`ViewElement`] for a `ZStack`.
pub struct ZStackElement {
    widget: Pod<dyn Widget>,
    alignment: ChildAlignment,
}

/// A mutable version of `ZStackElement`.
pub struct ZStackElementMut<'w> {
    parent: WidgetMut<'w, widgets::ZStack>,
    idx: usize,
}

impl ZStackElement {
    fn new(widget: Pod<dyn Widget>, alignment: ChildAlignment) -> Self {
        Self { widget, alignment }
    }
}

impl_indexed_element!(ZStackElement, ZStackElementMut);

impl<W: Widget + FromDynWidget + ?Sized> SuperElement<Pod<W>, ViewCtx> for ZStackElement {
    fn upcast(_: &mut ViewCtx, child: Pod<W>) -> Self {
        Self::new(child.erased(), ChildAlignment::ParentAligned)
    }

    fn with_downcast_val<R>(
        mut this: Mut<'_, Self>,
        f: impl FnOnce(Mut<'_, Pod<W>>) -> R,
    ) -> (Self::Mut<'_>, Option<R>) {
        let ret = { widgets::ZStack::get_mut(&mut this.parent, this.idx).map(f) };

        (this, ret)
    }
}

// MARK: Sequence

/// A trait implementing `ViewSequence` for `ZStackElement`.
pub trait ZStackSequence<State: 'static, Action = ()>:
    ViewSequence<State, Action, ViewCtx, ZStackElement>
{
}

impl<Seq, State, Action> ZStackSequence<State, Action> for Seq
where
    Seq: ViewSequence<State, Action, ViewCtx, ZStackElement>,
    State: 'static,
{
}

// MARK: Splice

type ZStackSplice<'w, 's> = super::CollectionSplice<'w, 's, widgets::ZStack, ZStackElement>;
impl ElementSplice<ZStackElement> for ZStackSplice<'_, '_> {
    collection_splice_insert!(ZStack, ZStackElement, widget, element => element.alignment);

    collection_splice_cursor!(ZStack, ZStackElement, ZStackElementMut);
}
