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
