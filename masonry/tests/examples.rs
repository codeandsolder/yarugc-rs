// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! This module imports widget code from most of the examples.
//!
//! When running integration tests, it will compare the rendered snapshots of the widgets
//! with the expected snapshots.

/// Example widgets and their snapshot tests.
#[cfg(test)]
#[expect(
    dead_code,
    reason = "The standalone example entry points are not called by snapshot integration tests"
)]
#[path = "../examples"]
pub mod others {
    /// Calculator example.
    pub mod calc_masonry;
    /// Custom widget example.
    pub mod custom_widget;
    /// Grid layout example.
    pub mod grid_masonry;
    /// Image widget example.
    pub mod simple_image;
    /// To-do list example.
    pub mod to_do_list;
}

#[cfg(test)]
mod tests {
    use masonry::core::Widget;
    use masonry::dpi::PhysicalSize;
    use masonry::theme::default_property_set;
    use masonry_testing::{TestHarness, TestHarnessParams};

    use super::*;

    // A series of tests to check that various widgets (the ones used in examples)
    // can handle being laid out and painted with a size of zero.

    const PARAMS_ZERO_SIZE: TestHarnessParams = {
        let mut params = TestHarnessParams::DEFAULT;
        params.window_size = PhysicalSize::new(0, 0);
        params
    };

    #[test]
    fn zero_size_calc_masonry() {
        let mut harness = TestHarness::create_with(
            default_property_set(),
            others::calc_masonry::build_calc(),
            PARAMS_ZERO_SIZE,
        );
        let _ = harness.render().unwrap();
    }

    #[test]
    fn zero_size_custom_widget() {
        let mut harness = TestHarness::create_with(
            default_property_set(),
            others::custom_widget::CustomWidget("Foobar".to_string()).prepare(),
            PARAMS_ZERO_SIZE,
        );
        let _ = harness.render().unwrap();
    }

    #[test]
    fn zero_size_grid_masonry() {
        let mut harness = TestHarness::create_with(
            default_property_set(),
            others::grid_masonry::make_grid(1.0),
            PARAMS_ZERO_SIZE,
        );
        let _ = harness.render().unwrap();
    }

    #[test]
    fn zero_size_simple_image() {
        let mut harness = TestHarness::create_with(
            default_property_set(),
            others::simple_image::make_image(),
            PARAMS_ZERO_SIZE,
        );
        let _ = harness.render().unwrap();
    }

    #[test]
    fn zero_size_to_do_list() {
        let mut harness = TestHarness::create_with(
            default_property_set(),
            others::to_do_list::make_widget_tree(),
            PARAMS_ZERO_SIZE,
        );
        let _ = harness.render().unwrap();
    }
}
