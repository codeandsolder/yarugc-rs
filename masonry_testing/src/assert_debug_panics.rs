// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

/// Checks that the given expression panics in debug mode. No-op in release mode.
///
/// This macro is useful for tests that check the behavior of `debug_assert!` or `debug_panic!` calls.
#[macro_export]
macro_rules! assert_debug_panics {
    ($expr:expr) => {
        $crate::assert_debug_panics_inner(
            || {
                let _ = $expr;
            },
            "",
        )
    };

    ($expr:expr, $needle:expr) => {
        $crate::assert_debug_panics_inner(
            || {
                let _ = $expr;
            },
            $needle,
        )
    };
}

use std::panic::{AssertUnwindSafe, catch_unwind};

#[track_caller]
#[doc(hidden)]
pub fn assert_debug_panics_inner(callback: impl FnOnce(), needle: &str) {
    if cfg!(not(debug_assertions)) {
        return;
    }

    // This function is only meant to be used in tests, so the potential for breakage is limited.
    // Note that AssertUnwindSafe is not a safety invariant: if someone misuses this somehow,
    // they may bump into functional bugs, but never undefined behavior.
    // See https://doc.rust-lang.org/std/panic/trait.UnwindSafe.html for details.
    let callback = AssertUnwindSafe(callback);

    let result = catch_unwind(callback);
    assert!(result.is_err(), "test did not panic as expected");
    if let Err(payload) = result {
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
        assert!(message.is_some(), "panic had unexpected type");
        assert!(
            message.is_some_and(|message| message.contains(needle)),
            "panic did not contain expected substring {needle:?}; message: {message:?}"
        );
    }
}
