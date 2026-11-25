// Copyright 2023 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};

const KEY: &str = "todomvc_persist";

#[derive(Default, Debug, Serialize, Deserialize)]
pub(super) struct AppState {
    #[serde(skip)]
    pub new_todo: String,
    pub todos: Vec<Todo>,
    #[serde(skip)]
    pub filter: Filter,
    #[serde(skip)]
    pub editing_id: Option<u64>,
    #[serde(skip)]
    pub focus_new_todo: bool,
    next_id: u64,
}

impl AppState {
    pub(super) fn create_todo(&mut self) {
        if self.new_todo.trim().is_empty() {
            return;
        }
        let title = self.new_todo.trim().to_string();
        let Some(id) = self.next_id() else {
            return;
        };
        self.new_todo.clear();
        self.todos.push(Todo::new(title, id));
        self.focus_new_todo = true;
        self.save();
    }

    fn next_id(&mut self) -> Option<u64> {
        self.next_id = self.next_id.checked_add(1)?;
        Some(self.next_id)
    }

    /// Are all the todos complete?
    pub(super) fn are_all_complete(&self) -> bool {
        self.todos.iter().all(|todo| todo.completed)
    }

    /// If all TODOs are complete, then mark them all not complete,
    /// else mark them all complete.
    pub(super) fn toggle_all_complete(&mut self) {
        let complete = !self.are_all_complete();
        for todo in &mut self.todos {
            todo.completed = complete;
        }
        self.save();
    }

    pub(super) fn visible_todos(&mut self) -> impl Iterator<Item = (usize, &mut Todo)> {
        self.todos
            .iter_mut()
            .enumerate()
            .filter(|(_, todo)| match self.filter {
                Filter::All => true,
                Filter::Active => !todo.completed,
                Filter::Completed => todo.completed,
            })
    }

    pub(super) fn update_new_todo(&mut self, new_text: &str) {
        self.new_todo.clear();
        self.new_todo.push_str(new_text);
    }

    pub(super) fn start_editing(&mut self, id: u64) {
        if let Some(ref mut todo) = self.todos.iter_mut().find(|todo| todo.id == id) {
            todo.title_editing.clear();
            todo.title_editing.push_str(&todo.title);
            self.editing_id = Some(id);
        }
    }

    /// Load the current state from local storage, or use the default.
    pub(super) fn load() -> Self {
        let Some(raw) = storage().and_then(|storage| storage.get_item(KEY).ok().flatten()) else {
            return Self::default();
        };
        match serde_json::from_str(&raw) {
            Ok(todos) => todos,
            Err(e) => {
                tracing::error!("couldn't load existing todos: {e}");
                Self::default()
            }
        }
    }

    /// Save the current state to local storage
    pub(super) fn save(&self) {
        let Some(storage) = storage() else {
            return;
        };
        match serde_json::to_string(self) {
            Ok(raw) => {
                if let Err(err) = storage.set_item(KEY, &raw) {
                    tracing::error!(?err, "Could not save todos");
                }
            }
            Err(err) => tracing::error!(%err, "Could not serialize todos"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Todo {
    pub id: u64,
    pub title: String,
    #[serde(skip)]
    pub title_editing: String,
    pub completed: bool,
}

impl Todo {
    pub(super) fn new(title: String, id: u64) -> Self {
        let title_editing = title.clone();
        Self {
            id,
            title,
            title_editing,
            completed: false,
        }
    }

    pub(super) fn save_editing(&mut self) {
        self.title.clear();
        self.title.push_str(&self.title_editing);
    }
}

#[derive(Debug, Default, PartialEq, Copy, Clone)]
pub(super) enum Filter {
    #[default]
    All,
    Active,
    Completed,
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}
