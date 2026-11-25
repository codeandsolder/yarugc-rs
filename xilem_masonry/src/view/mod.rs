// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Views for the widgets which are built-in to Masonry. These are the primitives your Xilem app's view tree will generally be constructed from.

// Container elements all carry the same mutable parent/index pair.
// A self-downcast is just a shorter reborrow of that pair.
macro_rules! impl_indexed_element {
    ($element:ident, $handle:ident) => {
        impl ViewElement for $element {
            type Mut<'w> = $handle<'w>;
        }

        impl SuperElement<Self, ViewCtx> for $element {
            fn upcast(_ctx: &mut ViewCtx, child: Self) -> Self {
                child
            }

            fn with_downcast_val<R>(
                mut this: Mut<'_, Self>,
                f: impl FnOnce(Mut<'_, Self>) -> R,
            ) -> (Self::Mut<'_>, Option<R>) {
                let result = f($handle {
                    idx: this.idx,
                    parent: this.parent.reborrow_mut(),
                });
                (this, Some(result))
            }
        }
    };
}

// A malformed splice cannot propagate a Result through ElementSplice's trait.
// Report the rejected operation and do not advance its insertion cursor.
fn collection_succeeded(result: Result<(), masonry::core::CollectionError>) -> bool {
    match result {
        Ok(()) => true,
        Err(error) => {
            tracing::error!(%error, "Widget collection mutation rejected");
            false
        }
    }
}

// Indexed views use the same checked element lifecycle regardless of their layout.
macro_rules! indexed_item_lifecycle {
    ($widget:ident, $params:ident) => {
        fn rebuild(
            &self,
            prev: &Self,
            view_state: &mut Self::ViewState,
            ctx: &mut ViewCtx,
            mut element: Mut<'_, Self::Element>,
            app_state: &mut State,
        ) {
            if self.$params != prev.$params {
                if !super::collection_succeeded(widgets::$widget::set_params(
                    &mut element.parent,
                    element.idx,
                    self.$params,
                )) {
                    return;
                }
            }
            if let Some(child) = widgets::$widget::get_mut(&mut element.parent, element.idx) {
                self.view
                    .rebuild(&prev.view, view_state, ctx, child, app_state);
            }
        }

        fn teardown(
            &self,
            view_state: &mut Self::ViewState,
            ctx: &mut ViewCtx,
            mut element: Mut<'_, Self::Element>,
        ) {
            if let Some(child) = widgets::$widget::get_mut(&mut element.parent, element.idx) {
                self.view.teardown(view_state, ctx, child);
            }
        }

        fn message(
            &self,
            view_state: &mut Self::ViewState,
            message: &mut MessageCtx,
            mut element: Mut<'_, Self::Element>,
            app_state: &mut State,
        ) -> MessageResult<Action> {
            let Some(child) = widgets::$widget::get_mut(&mut element.parent, element.idx) else {
                return MessageResult::Stale;
            };
            self.view.message(view_state, message, child, app_state)
        }
    };
}

// Views with a single child delegate the lifecycle through the same checked lookup.
macro_rules! single_child_view_lifecycle {
    ($widget:ident, $field:ident, rebuild) => {
        fn rebuild(
            &self,
            prev: &Self,
            view_state: &mut Self::ViewState,
            ctx: &mut ViewCtx,
            mut element: Mut<'_, Self::Element>,
            app_state: &mut State,
        ) {
            if let Some(child) = widgets::$widget::child_mut(&mut element) {
                self.$field
                    .rebuild(&prev.$field, view_state, ctx, child, app_state);
            }
        }
        single_child_view_lifecycle!($widget, $field);
    };
    ($widget:ident, $field:ident) => {
        fn teardown(
            &self,
            view_state: &mut Self::ViewState,
            ctx: &mut ViewCtx,
            mut element: Mut<'_, Self::Element>,
        ) {
            if let Some(child) = widgets::$widget::child_mut(&mut element) {
                self.$field.teardown(view_state, ctx, child);
            }
        }

        fn message(
            &self,
            view_state: &mut Self::ViewState,
            message: &mut MessageCtx,
            mut element: Mut<'_, Self::Element>,
            app_state: &mut State,
        ) -> MessageResult<Action> {
            let Some(child) = widgets::$widget::child_mut(&mut element) else {
                return MessageResult::Stale;
            };
            self.$field.message(view_state, message, child, app_state)
        }
    };
}

// A leaf action source owns no child state.
macro_rules! leaf_action_teardown {
    () => {
        fn teardown(
            &self,
            (): &mut Self::ViewState,
            ctx: &mut ViewCtx,
            element: Mut<'_, Self::Element>,
        ) {
            ctx.teardown_action_source(element.ctx.widget_id());
        }
    };
}

macro_rules! toggle_action_lifecycle {
    ($action:ty) => {
        leaf_action_teardown!();

        fn message(
            &self,
            (): &mut Self::ViewState,
            message: &mut MessageCtx,
            _: Mut<'_, Self::Element>,
            app_state: &mut State,
        ) -> MessageResult<Action> {
            message
                .take_message::<$action>()
                .map_or(MessageResult::Stale, |value| {
                    MessageResult::Action((self.callback)(app_state, value.0))
                })
        }
    };
}

// Splice cursors and scratch storage are identical for all widget collections.
struct CollectionSplice<'w, 's, W: masonry::core::Widget, E> {
    idx: usize,
    element: masonry::core::WidgetMut<'w, W>,
    scratch: &'s mut crate::core::AppendVec<E>,
}

impl<'w, 's, W: masonry::core::Widget, E> CollectionSplice<'w, 's, W, E> {
    const fn new(
        element: masonry::core::WidgetMut<'w, W>,
        scratch: &'s mut crate::core::AppendVec<E>,
    ) -> Self {
        debug_assert!(scratch.is_empty());
        Self {
            idx: 0,
            element,
            scratch,
        }
    }
}

/// Shared retained state for a widget collection.
#[doc(hidden)]
pub struct CollectionState<S, E> {
    seq_state: S,
    scratch: crate::core::AppendVec<E>,
}

impl<S, E: crate::core::ViewElement> CollectionState<S, E> {
    fn rebuild<State, Action, Seq, W>(
        &mut self,
        sequence: &Seq,
        prev: &Seq,
        ctx: &mut crate::ViewCtx,
        element: masonry::core::WidgetMut<'_, W>,
        app_state: &mut State,
    ) where
        W: masonry::core::Widget,
        Seq: crate::core::ViewSequence<State, Action, crate::ViewCtx, E, SeqState = S>,
        for<'w, 's> CollectionSplice<'w, 's, W, E>: crate::core::ElementSplice<E>,
    {
        let mut splice = CollectionSplice::new(element, &mut self.scratch);
        sequence.seq_rebuild(prev, &mut self.seq_state, ctx, &mut splice, app_state);
        debug_assert!(self.scratch.is_empty());
    }

    fn teardown<State, Action, Seq, W>(
        &mut self,
        sequence: &Seq,
        ctx: &mut crate::ViewCtx,
        element: masonry::core::WidgetMut<'_, W>,
    ) where
        W: masonry::core::Widget,
        Seq: crate::core::ViewSequence<State, Action, crate::ViewCtx, E, SeqState = S>,
        for<'w, 's> CollectionSplice<'w, 's, W, E>: crate::core::ElementSplice<E>,
    {
        let mut splice = CollectionSplice::new(element, &mut self.scratch);
        sequence.seq_teardown(&mut self.seq_state, ctx, &mut splice);
        debug_assert!(self.scratch.is_empty());
    }

    fn message<State, Action, Seq, W>(
        &mut self,
        sequence: &Seq,
        message: &mut crate::core::MessageCtx,
        element: masonry::core::WidgetMut<'_, W>,
        app_state: &mut State,
    ) -> crate::core::MessageResult<Action>
    where
        W: masonry::core::Widget,
        Seq: crate::core::ViewSequence<State, Action, crate::ViewCtx, E, SeqState = S>,
        for<'w, 's> CollectionSplice<'w, 's, W, E>: crate::core::ElementSplice<E>,
    {
        let mut splice = CollectionSplice::new(element, &mut self.scratch);
        let result = sequence.seq_message(&mut self.seq_state, message, &mut splice, app_state);
        debug_assert!(self.scratch.is_empty());
        result
    }
}

// Action-producing wrappers must release their own action source even when the child is absent.
macro_rules! action_child_teardown {
    ($widget:ident, $field:ident, $id:ident) => {
        fn teardown(
            &self,
            view_state: &mut Self::ViewState,
            ctx: &mut ViewCtx,
            mut element: Mut<'_, Self::Element>,
        ) {
            ctx.with_id($id, |ctx| {
                if let Some(child) = widgets::$widget::child_mut(&mut element) {
                    View::<State, Action, _>::teardown(&self.$field, view_state, ctx, child);
                }
            });
            ctx.teardown_action_source(element.ctx.widget_id());
        }
    };
}

macro_rules! collection_splice_insert {
    ($widget:ident, $element:ident, $child:ident, $item:ident => $params:expr) => {
        fn with_scratch<R>(&mut self, f: impl FnOnce(&mut AppendVec<$element>) -> R) -> R {
            let ret = f(self.scratch);
            for $item in self.scratch.drain() {
                if super::collection_succeeded(widgets::$widget::insert(
                    &mut self.element,
                    self.idx,
                    $item.$child.new_widget,
                    $params,
                )) {
                    self.idx += 1;
                }
            }
            ret
        }

        fn insert(&mut self, $item: $element) {
            if super::collection_succeeded(widgets::$widget::insert(
                &mut self.element,
                self.idx,
                $item.$child.new_widget,
                $params,
            )) {
                self.idx += 1;
            }
        }
    };
}

// All collection splices advance and delete using the same parent/index handle.
macro_rules! collection_splice_cursor {
    ($widget:ident, $element:ident, $handle:ident) => {
        fn mutate<R>(&mut self, f: impl FnOnce(Mut<'_, $element>) -> R) -> R {
            let ret = f($handle {
                parent: self.element.reborrow_mut(),
                idx: self.idx,
            });
            self.idx += 1;
            ret
        }

        fn skip(&mut self, n: usize) {
            self.idx += n;
        }

        fn index(&self) -> usize {
            self.idx
        }

        fn delete<R>(&mut self, f: impl FnOnce(Mut<'_, $element>) -> R) -> R {
            let ret = f($handle {
                parent: self.element.reborrow_mut(),
                idx: self.idx,
            });
            super::collection_succeeded(widgets::$widget::remove(&mut self.element, self.idx));
            ret
        }
    };
}

// Raster and vector images have identical accessibility and view lifecycle rules.
macro_rules! image_view_builders {
    ($alt_text:ident) => {
        // Because this method is image-specific, we don't add it to the Style trait.
        /// Specify the object fit.
        #[must_use = "Use the returned value."]
        pub fn fit<State: 'static, Action: 'static>(
            self,
            fill: ObjectFit,
        ) -> Prop<ObjectFit, Self, State, Action> {
            self.prop(fill)
        }

        /// Specifies whether the image is decorative, meaning it doesn't have meaningful content
        /// and is only for visual presentation.
        ///
        /// If `is_decorative` is `true`, the image will be ignored by screen readers.
        pub const fn decorative(mut self, is_decorative: bool) -> Self {
            self.decorative = is_decorative;
            self
        }

        /// Set the text that will describe the image to screen readers.
        ///
        /// Users are encouraged to set alt text for the image.
        /// If possible, the alt-text should succinctly describe what the image represents.
        ///
        /// If the image is decorative users should set alt text to `""`.
        /// If it's too hard to describe through text, the alt text should be left unset.
        /// This allows accessibility clients to know that there is no accessible description of the image content.
        pub fn $alt_text(mut self, alt_text: impl Into<ArcStr>) -> Self {
            self.alt_text = Some(alt_text.into());
            self
        }
    };
}

macro_rules! image_view_lifecycle {
    ($view:ident, $widget:ident, $source:ident, $setter:ident, $changed:expr) => {
        impl ViewMarker for $view {}
        impl<State: 'static, Action> View<State, Action, ViewCtx> for $view {
            type Element = Pod<widgets::$widget>;
            type ViewState = ();

            fn build(&self, _ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
                let mut image =
                    widgets::$widget::new(self.$source.clone()).decorative(self.decorative);
                if let Some(alt_text) = &self.alt_text {
                    image = image.with_alt_text(alt_text.clone());
                }
                (Pod::new(image), ())
            }

            fn rebuild(
                &self,
                prev: &Self,
                (): &mut Self::ViewState,
                _: &mut ViewCtx,
                mut element: Mut<'_, Self::Element>,
                _: &mut State,
            ) {
                if ($changed)(&prev.$source, &self.$source) {
                    widgets::$widget::$setter(&mut element, self.$source.clone());
                }
                if self.decorative != prev.decorative {
                    widgets::$widget::set_decorative(&mut element, self.decorative);
                }
                if self.alt_text != prev.alt_text {
                    widgets::$widget::set_alt_text(&mut element, self.alt_text.clone());
                }
            }

            fn teardown(
                &self,
                (): &mut Self::ViewState,
                _: &mut ViewCtx,
                _: Mut<'_, Self::Element>,
            ) {
            }

            fn message(
                &self,
                (): &mut Self::ViewState,
                message: &mut MessageCtx,
                _: Mut<'_, Self::Element>,
                _: &mut State,
            ) -> MessageResult<Action> {
                tracing::error!(
                    ?message,
                    concat!(
                        "Message arrived in ",
                        stringify!($view),
                        "::message, but it does not consume messages."
                    )
                );
                MessageResult::Stale
            }
        }
    };
}

mod badge;
mod badged;
mod button;
mod canvas;
mod checkbox;
mod collapse_panel;
mod divider;
mod flex;
mod grid;
mod image;
mod indexed_stack;
mod label;
mod optional_child;
mod portal;
mod progress_bar;
mod prop;
mod prose;
mod radio_button;
mod radio_group;
mod resize_observer;
mod sized_box;
mod slider;
mod spinner;
mod split;
mod svg;
mod switch;
mod task;
mod text_input;
mod transform;
mod variable_label;
mod virtual_scroll;
mod worker;
mod zstack;

pub use self::badge::*;
pub use self::badged::*;
pub use self::button::*;
pub use self::canvas::*;
pub use self::checkbox::*;
pub use self::collapse_panel::*;
pub use self::divider::*;
pub use self::flex::*;
pub use self::grid::*;
pub use self::image::*;
pub use self::indexed_stack::*;
pub use self::label::*;
pub use self::portal::*;
pub use self::progress_bar::*;
pub use self::prop::*;
pub use self::prose::*;
pub use self::radio_button::*;
pub use self::radio_group::*;
pub use self::resize_observer::*;
pub use self::sized_box::*;
pub use self::slider::*;
pub use self::spinner::*;
pub use self::split::*;
pub use self::svg::*;
pub use self::switch::*;
pub use self::task::*;
pub use self::text_input::*;
pub use self::transform::*;
pub use self::variable_label::*;
pub use self::virtual_scroll::*;
pub use self::worker::*;
pub use self::zstack::*;
