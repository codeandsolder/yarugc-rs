// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! A demonstration of the [`VirtualScroll`] widget, producing an infinite[^1] `FizzBuzz`.
//!
//! [^1]: Limited to `i64::MIN..i64::MAX-1`; that is, there are `2^64-1` possible items.
//! However, there is (currently...) no way to jump to a specific item, so it's impossible to reach the end.

// On Windows platform, don't show a console when opening the app.
#![windows_subsystem = "windows"]

use masonry::core::{ArcStr, ErasedAction, NewWidget, StyleProperty, WidgetId, WidgetTag};
use masonry::dpi::LogicalSize;
use masonry::theme::default_property_set;
use masonry::widgets::{Label, VirtualScroll, VirtualScrollAction};
use masonry_winit::app::{AppDriver, DriverCtx, NewWindow, WindowId};
use masonry_winit::winit::window::Window;

/// Function to create the virtual scroll area.
fn init() -> VirtualScroll {
    // We start our fizzbuzzing with the top of the screen at item 0
    VirtualScroll::new(0, usize::MAX)
}

struct Driver {
    scroll_tag: WidgetTag<VirtualScroll>,
    fizz: ArcStr,
    buzz: ArcStr,
    fizzbuzz: ArcStr,
    window_id: WindowId,
}

impl AppDriver for Driver {
    fn on_action(
        &mut self,
        window_id: WindowId,
        ctx: &mut DriverCtx<'_>,
        widget_id: WidgetId,
        action: ErasedAction,
    ) {
        debug_assert_eq!(window_id, self.window_id, "unknown window");

        let Some(render_root) = ctx.render_root(window_id) else {
            return;
        };
        let scroll_id = render_root
            .get_widget_with_tag(self.scroll_tag)
            .map(|w| w.id());

        if Some(widget_id) == scroll_id {
            // The VirtualScroll widget will send us a VirtualScrollAction every time it wants different
            // items to be loaded or unloaded.
            let Ok(action) = action.downcast::<VirtualScrollAction>() else {
                return;
            };
            let VirtualScrollAction::Fetch(action) = *action else {
                tracing::trace!("Got scroll action {action:?}");
                return;
            };
            let _ = render_root.edit_base_layer(
                |mut scroll: masonry::core::WidgetMut<'_, VirtualScroll>| {
                    // We need to tell the `VirtualScroll` which request this is associated with
                    // This is so that the controller knows which actions have been handled.
                    let Some(mut fetch) = VirtualScroll::begin_fetch(&mut scroll, &action) else {
                        return;
                    };
                    for idx in action.removed() {
                        // Save any associated application data before unloading.
                        let _ = fetch.remove_child(idx);
                    }
                    for idx in action.added() {
                        let label: ArcStr = match (idx % 3 == 0, idx % 5 == 0) {
                            (false, true) => self.buzz.clone(),
                            (true, false) => self.fizz.clone(),
                            (true, true) => self.fizzbuzz.clone(),
                            (false, false) => format!("{idx}").into(),
                        };
                        let _ = fetch.add_child(
                            idx,
                            NewWidget::new(Label::new(label).with_style(StyleProperty::FontSize(
                                if idx % 100 == 0 { 40. } else { 20. },
                            )))
                            .erased(),
                        );
                    }
                },
            );
        } else {
            tracing::warn!("Got unexpected action {action:?}");
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scroll_tag = WidgetTag::unique();
    let main_widget = NewWidget::new(init()).with_tag(scroll_tag).erased();
    let driver = Driver {
        scroll_tag,
        fizz: "Fizz".into(),
        buzz: "Buzz".into(),
        fizzbuzz: "FizzBuzz".into(),
        window_id: WindowId::next(),
    };
    let window_size = LogicalSize::new(800.0, 500.0);
    let window_attributes = Window::default_attributes()
        .with_title("Infinite FizzBuzz")
        .with_resizable(true)
        .with_min_inner_size(window_size);

    masonry_winit::app::run(
        vec![NewWindow::new_with_id(
            driver.window_id,
            window_attributes,
            main_widget,
        )],
        driver,
        default_property_set(),
    )?;
    Ok(())
}
