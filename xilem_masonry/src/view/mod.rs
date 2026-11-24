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
            ctx.teardown_action_source(element);
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
