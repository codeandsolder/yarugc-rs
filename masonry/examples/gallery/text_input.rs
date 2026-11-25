// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use masonry::core::{NewWidget, StyleProperty, Widget};
use masonry::properties::types::CrossAxisAlignment;
use masonry::widgets::{Flex, Label, SizedBox, TextInput};

use crate::demo::{DemoPage, ShellTags, wrap_in_shell};

pub(super) struct TextInputDemo {
    shell: ShellTags,
}

impl TextInputDemo {
    pub(super) const fn new(shell: ShellTags) -> Self {
        Self { shell }
    }
}

impl DemoPage for TextInputDemo {
    fn name(&self) -> &'static str {
        "Text input"
    }

    fn shell_tags(&self) -> ShellTags {
        self.shell
    }

    fn build(&self) -> NewWidget<dyn Widget> {
        let body = Flex::column()
            .cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_fixed(
                Label::new("Type in the text box:")
                    .with_style(StyleProperty::FontSize(14.0))
                    .prepare(),
            )
            .with_fixed_spacer(masonry::layout::length!(8.0))
            .with_fixed(
                SizedBox::new(
                    TextInput::new("")
                        .with_placeholder("Hello from Masonry…")
                        .prepare(),
                )
                .height(masonry::layout::length!(40.0))
                .prepare(),
            );

        wrap_in_shell(self.shell, NewWidget::new(body).erased())
    }
}
