// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::core::{ProxyError, RawProxy, SendMessage, ViewId};

#[derive(Debug)]
pub(crate) struct ClosedProxy;

impl RawProxy for ClosedProxy {
    fn send_message(&self, _: Arc<[ViewId]>, message: SendMessage) -> Result<(), ProxyError> {
        Err(ProxyError::DriverFinished(message))
    }

    fn dyn_debug(&self) -> &dyn std::fmt::Debug {
        self
    }
}

pub(crate) fn view_context() -> crate::ViewCtx {
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap(),
    );
    crate::ViewCtx::new(Arc::new(ClosedProxy), runtime)
}

pub(crate) fn render_root(
    widget: masonry::core::NewWidget<impl masonry::core::Widget + ?Sized>,
    size: (u32, u32),
) -> (
    masonry::app::RenderRoot,
    std::sync::mpsc::Receiver<masonry::app::RenderRootSignal>,
) {
    use masonry::app::{RenderRoot, RenderRootOptions, WindowSizePolicy};
    let (send, receive) = std::sync::mpsc::channel();
    let root = RenderRoot::new(
        widget,
        move |signal| {
            let _ = send.send(signal);
        },
        RenderRootOptions {
            default_properties: Arc::new(masonry::theme::default_property_set()),
            use_system_fonts: false,
            size_policy: WindowSizePolicy::User,
            size: size.into(),
            scale_factor: 1.0,
            test_font: None,
        },
    );
    (root, receive)
}
