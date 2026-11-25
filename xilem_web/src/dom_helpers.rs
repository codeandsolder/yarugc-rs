// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use web_sys::wasm_bindgen::JsCast;

/// Failure to obtain the document or a requested HTML node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomError {
    /// No browser document is available.
    DocumentUnavailable,
    /// The document has no HTML body.
    BodyUnavailable,
    /// No HTML element has the requested identifier.
    ElementNotFound,
}
impl std::fmt::Display for DomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DocumentUnavailable => "No browser document is available",
            Self::BodyUnavailable => "Document has no HTML body",
            Self::ElementNotFound => "No HTML element has the requested identifier",
        })
    }
}
impl std::error::Error for DomError {}

/// Get the HTML document body.
///
/// # Errors
/// Returns an error if the document or its body is unavailable.
pub fn document_body() -> Result<web_sys::HtmlElement, DomError> {
    document()?.body().ok_or(DomError::BodyUnavailable)
}

/// Get the current browser document.
///
/// # Errors
/// Returns an error when there is no browser document.
pub fn document() -> Result<web_sys::Document, DomError> {
    web_sys::window()
        .and_then(|window| window.document())
        .ok_or(DomError::DocumentUnavailable)
}

/// Get an HTML element by its identifier.
///
/// # Errors
/// Returns an error if the document or requested HTML element is unavailable.
pub fn get_element_by_id(id: &str) -> Result<web_sys::HtmlElement, DomError> {
    document()?
        .get_element_by_id(id)
        .and_then(|element| element.dyn_into().ok())
        .ok_or(DomError::ElementNotFound)
}

/// Helper to get the value from an HTML input element from a given event.
///
/// Returns `None` if the event isn't a valid input event or conversions fail.
#[must_use = "Use the value of the event target, if available."]
pub fn input_event_target_value(event: &web_sys::Event) -> Option<String> {
    event
        .target()?
        .dyn_into::<web_sys::HtmlInputElement>()
        .ok()?
        .value()
        .into()
}

/// Helper to get the value from an HTML textarea element from a given event.
///
/// Returns `None` if the event isn't a valid input event or conversions fail.
#[must_use = "Use the value of the event target, if available."]
pub fn textarea_event_target_value(event: &web_sys::Event) -> Option<String> {
    event
        .target()?
        .dyn_into::<web_sys::HtmlTextAreaElement>()
        .ok()?
        .value()
        .into()
}
