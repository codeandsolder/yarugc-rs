// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Browser regression fixture, exercised by tests/browser/lifecycle.mjs.

use wasm_bindgen::UnwrapThrowExt;
use xilem_web::concurrent::memoized_await;
use xilem_web::elements::{custom_element, html, svg};
use xilem_web::interfaces::{Element, HtmlInputElement, SvgElement};
use xilem_web::modifiers::style;
use xilem_web::svg::StrokeStyle;
use xilem_web::svg::kurbo::{Cap, Circle, Join, Line, Rect};
use xilem_web::svg::peniko::color::palette;
use xilem_web::{App, DomFragment, PointerMsg, document_body, get_element_by_id};

#[derive(Default)]
struct State {
    phase: usize,
    count: usize,
    request: usize,
    output: String,
    pointer_events: String,
}

struct RequestGuard(usize);

impl Drop for RequestGuard {
    fn drop(&mut self) {
        let _ = web_sys::js_sys::Reflect::set(
            &web_sys::js_sys::global(),
            &format!("droppedRequest{}", self.0).into(),
            &true.into(),
        );
    }
}

fn view(state: &mut State) -> impl DomFragment<State> + use<> {
    let shift = if state.phase == 0 { 0.0 } else { 5.0 };
    let tag = if state.phase == 0 { "button" } else { "span" };
    let event = if state.phase == 2 {
        "mousedown"
    } else {
        "click"
    };
    let classes = if state.phase == 0 {
        vec!["shared", "initial"]
    } else {
        vec!["shared"]
    };

    xilem_web::core::fork(
        html::div((
            html::button("advance")
                .id("advance")
                .on_click(|state: &mut State, _| {
                    state.phase += 1;
                }),
            custom_element(tag, "target")
                .id("target")
                .class(classes)
                .class("outer")
                .style(style("color", "red"))
                .on(event, |state: &mut State, _: web_sys::Event| {
                    state.count += 1;
                }),
            html::button("reentrant")
                .id("reentrant")
                .on_click(|state: &mut State, _| {
                    state.count += 10;
                    let element = get_element_by_id("target").unwrap_throw();
                    let event = web_sys::Event::new("mousedown").unwrap_throw();
                    element.dispatch_event(&event).unwrap_throw();
                }),
            html::button("retire-event")
                .id("retire-event")
                .on_click(|state: &mut State, _| {
                    let element = get_element_by_id("target").unwrap_throw();
                    let event = web_sys::Event::new("mousedown").unwrap_throw();
                    element.dispatch_event(&event).unwrap_throw();
                    state.phase += 1;
                }),
            html::div(state.count).id("count"),
            html::div(())
                .id("cascade")
                .style(style("margin", if state.phase < 2 { "4px" } else { "8px" }))
                .style(style("margin-left", "21px")),
            html::button("request")
                .id("request")
                .on_click(|state: &mut State, _| {
                    state.request += 1;
                }),
            html::div(state.output.clone()).id("output"),
            html::input(())
                .type_("checkbox")
                .checked(state.phase == 1)
                .checked(state.phase < 2)
                .id("overwritten-checked"),
            svg::svg((
                Circle::new((10.0 + shift, 20.0), 3.0 + shift).id("circle"),
                Line::new((1.0, 2.0 + shift), (3.0 + shift, 4.0))
                    .stroke(
                        palette::css::BLUE,
                        if state.phase == 0 {
                            StrokeStyle::new(2.0)
                        } else {
                            StrokeStyle::new(4.0)
                                .with_cap(Cap::Square)
                                .with_join(Join::Miter)
                                .with_miter_limit(6.0)
                                .with_dashes(2.0, [3.0, 1.0])
                        },
                    )
                    .id("line"),
                Rect::new(1.0 + shift, 2.0, 2.0_f64.mul_add(shift, 6.0), 9.0).id("rect"),
            )),
            custom_element(tag, "pointer").id("pointer-target").pointer(
                |state: &mut State, event| {
                    use std::fmt::Write as _;
                    let kind = match event {
                        PointerMsg::Down(_) => "D",
                        PointerMsg::Move(_) => "M",
                        PointerMsg::Up(_) => "U",
                        PointerMsg::Cancel(_) => "C",
                    };
                    let _ = write!(
                        state.pointer_events,
                        "{kind}{}:{};",
                        event.id(),
                        event.buttons()
                    );
                },
            ),
            html::div(state.pointer_events.clone()).id("pointer-events"),
        )),
        memoized_await(
            state.request,
            |request| {
                let name = format!("resolveRequest{request}");
                let promise = web_sys::js_sys::Promise::new(&mut |resolve, _| {
                    web_sys::js_sys::Reflect::set(
                        &web_sys::js_sys::global(),
                        &name.clone().into(),
                        &resolve,
                    )
                    .unwrap_throw();
                });
                let guard = RequestGuard(*request);
                async move {
                    let _guard = guard;
                    wasm_bindgen_futures::JsFuture::from(promise)
                        .await
                        .unwrap_throw()
                        .as_string()
                        .unwrap_throw()
                }
            },
            |state: &mut State, output| state.output = output,
        )
        .debounce_ms(if state.request == 0 { 0 } else { 100 }),
    )
}

fn main() -> Result<(), xilem_web::DomError> {
    App::new(document_body()?, State::default(), view)?.run();
    Ok(())
}
