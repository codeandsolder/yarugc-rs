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
