// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! The boilerplate run function for desktop platforms

use xilem::EventLoop;

fn main() -> anyhow::Result<()> {
    placehero::run(EventLoop::with_user_event())
}
