// Copyright 2023 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! A recreation of todomvc in Xilem Web
// TODO: Link

#[expect(
    clippy::redundant_pub_crate,
    reason = "State is internal to the example under unreachable_pub."
)]
mod state;

use state::{AppState, Filter, Todo, TodoAction};
use xilem_web::core::reset_on_key;
use xilem_web::elements::html as el;
use xilem_web::interfaces::{Element, HtmlInputElement};
use xilem_web::modifiers::style as s;
use xilem_web::{App, DomView, get_element_by_id, input_event_target_value};

fn todo_item(todo: &Todo, editing: Option<&str>) -> impl Element<AppState> + use<> {
    let id = todo.id;
    let checkbox = el::input(())
        .class("toggle")
        .type_("checkbox")
        .checked(todo.completed)
        .on_click(move |state: &mut AppState, _| state.dispatch(TodoAction::Toggle(id)));

    el::li((
        el::div((
            checkbox,
            el::label(todo.title.clone()).on_dblclick(move |state: &mut AppState, _| {
                state.dispatch(TodoAction::BeginEdit(id));
            }),
            el::button(())
                .class("destroy")
                .on_click(move |state: &mut AppState, _| state.dispatch(TodoAction::Delete(id))),
        ))
        .class("view"),
        el::input(())
            .value(editing.unwrap_or(&todo.title).to_owned())
            .class("edit")
            .on_keydown(move |state: &mut AppState, event| {
                if !event.is_composing() {
                    match event.key().as_str() {
                        "Enter" => state.dispatch(TodoAction::CommitEdit(id)),
                        "Escape" => state.dispatch(TodoAction::CancelEdit(id)),
                        _ => {}
                    }
                }
            })
            .on_input(move |state: &mut AppState, event| {
                if let Some(title) = input_event_target_value(&event) {
                    state.dispatch(TodoAction::EditTitle(id, title));
                }
            })
            .on_blur(move |state: &mut AppState, _| state.dispatch(TodoAction::CommitEdit(id))),
    ))
    .class(todo.completed.then_some("completed"))
    .class(editing.is_some().then_some("editing"))
}

fn footer_view(state: &AppState, should_display: bool) -> impl Element<AppState> + use<> {
    let clear_button = state.todos().iter().any(|todo| todo.completed).then(|| {
        el::button("Clear completed")
            .class("clear-completed")
            .on_click(|state: &mut AppState, _| {
                state.dispatch(TodoAction::ClearCompleted);
            })
    });

    let filter_class = |filter| (state.filter == filter).then_some("selected");

    let items_left = state.todos().iter().filter(|todo| !todo.completed).count();
    let item_noun = if items_left == 1 { "item" } else { "items" };

    el::footer((
        el::span(format!("{items_left} {item_noun} left")).class("todo-count"),
        el::ul((
            el::li(Element::on_click(
                el::a("All")
                    .attr("href", "#/")
                    .class(filter_class(Filter::All)),
                |state: &mut AppState, _| {
                    state.dispatch(TodoAction::SetFilter(Filter::All));
                },
            )),
            " ",
            el::li(Element::on_click(
                el::a("Active")
                    .attr("href", "#/active")
                    .class(filter_class(Filter::Active)),
                |state: &mut AppState, _| {
                    state.dispatch(TodoAction::SetFilter(Filter::Active));
                },
            )),
            " ",
            el::li(Element::on_click(
                el::a("Completed")
                    .attr("href", "#/completed")
                    .class(filter_class(Filter::Completed)),
                |state: &mut AppState, _| {
                    state.dispatch(TodoAction::SetFilter(Filter::Completed));
                },
            )),
        ))
        .class("filters"),
        clear_button,
    ))
    .class("footer")
    .style((!should_display).then_some(s("display", "none")))
}

fn main_view(state: &AppState, should_display: bool) -> impl Element<AppState> + use<> {
    let todos: Vec<_> = state
        .visible_todos()
        .map(|todo| reset_on_key(todo.id, todo_item(todo, state.editing_title(todo.id))))
        .collect();
    let toggle_all = el::input(())
        .attr("id", "toggle-all")
        .class("toggle-all")
        .type_("checkbox")
        .checked(state.are_all_complete());

    el::section((
        toggle_all.on_click(|state: &mut AppState, _| state.dispatch(TodoAction::ToggleAll)),
        el::label(()).attr("for", "toggle-all"),
        el::ul(todos).class("todo-list"),
    ))
    .class("main")
    .style((!should_display).then_some(s("display", "none")))
}

fn app_logic(state: &mut AppState) -> impl DomView<AppState> + use<> {
    let some_todos = !state.todos().is_empty();
    let main = main_view(state, some_todos);
    let footer = footer_view(state, some_todos);
    let input = el::input(())
        .class("new-todo")
        .attr("placeholder", "What needs to be done?")
        .value(state.new_todo.clone())
        .attr("autofocus", true);
    el::div((
        el::header((
            el::h1("TODOs"),
            input
                .on_keydown(|state: &mut AppState, evt| {
                    if !evt.is_composing() && evt.key() == "Enter" {
                        state.dispatch(TodoAction::Create);
                    }
                })
                .on_input(|state: &mut AppState, event| {
                    if let Some(title) = input_event_target_value(&event) {
                        state.dispatch(TodoAction::SetNewTitle(title));
                    }
                }),
        ))
        .class("header"),
        main,
        footer,
    ))
}

fn main() -> Result<(), xilem_web::DomError> {
    console_error_panic_hook::set_once();
    tracing_wasm::set_as_global_default();
    App::new(get_element_by_id("todoapp")?, AppState::load(), app_logic)?.run();
    Ok(())
}
