// Copyright 2023 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

const KEY: &str = "todomvc_persist";

#[derive(Default, Debug)]
pub(super) struct AppState {
    pub new_todo: String,
    pub filter: Filter,
    editing: Option<Editing>,
    persisted: PersistedTodos,
}

#[derive(Default, Debug, Serialize, Deserialize)]
struct PersistedTodos {
    todos: Vec<Todo>,
    // High-water mark, including IDs of deleted records.
    next_id: u64,
}

#[derive(Debug)]
struct Editing {
    id: u64,
    draft: String,
}

#[derive(Debug)]
pub(super) enum TodoAction {
    Create,
    Toggle(u64),
    ToggleAll,
    Delete(u64),
    ClearCompleted,
    BeginEdit(u64),
    EditTitle(u64, String),
    CommitEdit(u64),
    CancelEdit(u64),
    SetFilter(Filter),
    SetNewTitle(String),
}

impl AppState {
    pub(super) fn todos(&self) -> &[Todo] {
        &self.persisted.todos
    }

    pub(super) fn visible_todos(&self) -> impl Iterator<Item = &Todo> {
        self.todos()
            .iter()
            .filter(|todo| self.filter.includes(todo))
    }

    pub(super) fn editing_title(&self, id: u64) -> Option<&str> {
        self.editing
            .as_ref()
            .filter(|edit| edit.id == id)
            .map(|edit| edit.draft.as_str())
    }

    pub(super) fn are_all_complete(&self) -> bool {
        self.todos().iter().all(|todo| todo.completed)
    }

    fn create_todo(&mut self) -> bool {
        let title = self.new_todo.trim();
        if title.is_empty() {
            return false;
        }
        let Some(id) = self.persisted.next_id.checked_add(1) else {
            tracing::error!("Todo identifier space exhausted");
            return false;
        };
        self.persisted.todos.push(Todo {
            id,
            title: title.to_owned(),
            completed: false,
        });
        self.persisted.next_id = id;
        self.new_todo.clear();
        true
    }

    fn delete_todo(&mut self, id: u64) -> bool {
        let before = self.persisted.todos.len();
        self.persisted.todos.retain(|todo| todo.id != id);
        before != self.persisted.todos.len()
    }

    fn commit_edit(&mut self, id: u64) -> bool {
        let Some(edit) = self.editing.take_if(|edit| edit.id == id) else {
            return false;
        };
        let title = edit.draft.trim();
        if title.is_empty() {
            return self.delete_todo(id);
        }
        let Some(todo) = self.persisted.todos.iter_mut().find(|todo| todo.id == id) else {
            return false;
        };
        if todo.title == title {
            return false;
        }
        todo.title.clear();
        todo.title.push_str(title);
        true
    }

    fn clear_hidden_edit(&mut self) {
        if self
            .editing
            .as_ref()
            .is_some_and(|edit| !self.visible_todos().any(|todo| todo.id == edit.id))
        {
            self.editing = None;
        }
    }

    // The reducer is independent of the browser and reports whether durable
    // state changed. Transient input and filter updates do not write storage.
    fn apply(&mut self, action: TodoAction) -> bool {
        let changed = match action {
            TodoAction::Create => self.create_todo(),
            TodoAction::Toggle(id) => {
                if let Some(todo) = self.persisted.todos.iter_mut().find(|todo| todo.id == id) {
                    todo.completed = !todo.completed;
                    true
                } else {
                    false
                }
            }
            TodoAction::ToggleAll => {
                let complete = !self.are_all_complete();
                for todo in &mut self.persisted.todos {
                    todo.completed = complete;
                }
                !self.persisted.todos.is_empty()
            }
            TodoAction::Delete(id) => self.delete_todo(id),
            TodoAction::ClearCompleted => {
                let before = self.persisted.todos.len();
                self.persisted.todos.retain(|todo| !todo.completed);
                before != self.persisted.todos.len()
            }
            TodoAction::BeginEdit(id) => {
                let draft = self
                    .visible_todos()
                    .find(|todo| todo.id == id)
                    .map(|todo| todo.title.clone());
                if let Some(draft) = draft {
                    self.editing = Some(Editing { id, draft });
                }
                false
            }
            TodoAction::EditTitle(id, title) => {
                if let Some(edit) = self.editing.as_mut().filter(|edit| edit.id == id) {
                    edit.draft = title;
                }
                false
            }
            TodoAction::CommitEdit(id) => self.commit_edit(id),
            TodoAction::CancelEdit(id) => {
                if self.editing.as_ref().is_some_and(|edit| edit.id == id) {
                    self.editing = None;
                }
                false
            }
            TodoAction::SetFilter(filter) => {
                self.filter = filter;
                self.clear_hidden_edit();
                false
            }
            TodoAction::SetNewTitle(title) => {
                self.new_todo = title;
                false
            }
        };
        if changed {
            self.clear_hidden_edit();
        }
        changed
    }

    pub(super) fn dispatch(&mut self, action: TodoAction) {
        if self.apply(action) {
            self.save();
        }
    }

    fn decode(raw: &str) -> Result<Self, serde_json::Error> {
        let mut persisted: PersistedTodos = serde_json::from_str(raw)?;
        let mut ids = HashSet::with_capacity(persisted.todos.len());
        for todo in &persisted.todos {
            if todo.id == 0 || !ids.insert(todo.id) {
                return Err(serde::de::Error::custom("invalid or duplicate todo ID"));
            }
            persisted.next_id = persisted.next_id.max(todo.id);
        }
        Ok(Self {
            persisted,
            ..Self::default()
        })
    }

    /// Load the current state from local storage, or use the default.
    pub(super) fn load() -> Self {
        let Some(storage) = storage() else {
            return Self::default();
        };
        match storage.get_item(KEY) {
            Ok(Some(raw)) => match Self::decode(&raw) {
                Ok(state) => state,
                Err(error) => {
                    tracing::error!(%error, "Could not decode stored todos");
                    Self::default()
                }
            },
            Ok(None) => Self::default(),
            Err(error) => {
                tracing::error!(?error, "Could not read stored todos");
                Self::default()
            }
        }
    }

    fn save(&self) {
        let Some(storage) = storage() else {
            return;
        };
        match serde_json::to_string(&self.persisted) {
            Ok(raw) => {
                if let Err(error) = storage.set_item(KEY, &raw) {
                    tracing::error!(?error, "Could not save todos");
                }
            }
            Err(error) => tracing::error!(%error, "Could not serialize todos"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Todo {
    pub id: u64,
    pub title: String,
    pub completed: bool,
}

#[derive(Debug, Default, PartialEq, Eq, Copy, Clone)]
pub(super) enum Filter {
    #[default]
    All,
    Active,
    Completed,
}

impl Filter {
    const fn includes(self, todo: &Todo) -> bool {
        match self {
            Self::All => true,
            Self::Active => !todo.completed,
            Self::Completed => todo.completed,
        }
    }
}

fn storage() -> Option<web_sys::Storage> {
    match web_sys::window()?.local_storage() {
        Ok(storage) => storage,
        Err(error) => {
            tracing::error!(?error, "Could not access local storage");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(state: &mut AppState, title: &str) -> u64 {
        assert!(!state.apply(TodoAction::SetNewTitle(title.to_owned())));
        assert!(state.apply(TodoAction::Create));
        state.todos().last().unwrap().id
    }

    #[test]
    fn all_durable_mutations_request_persistence_and_reject_missing_ids() {
        let mut state = AppState::default();
        assert!(!state.apply(TodoAction::Create));
        let first = create(&mut state, "  first  ");
        let second = create(&mut state, "second");
        assert_eq!(state.todos()[0].title, "first");
        assert!(!state.apply(TodoAction::Toggle(999)));
        assert!(state.apply(TodoAction::Toggle(first)));
        assert!(state.apply(TodoAction::ClearCompleted));
        assert_eq!(state.todos().len(), 1);
        assert!(state.apply(TodoAction::ToggleAll));
        assert!(state.todos()[0].completed);
        assert!(state.apply(TodoAction::Delete(second)));
        assert!(!state.apply(TodoAction::Delete(second)));
        assert!(!state.apply(TodoAction::ToggleAll));
        assert!(create(&mut state, "third") > second);
    }

    #[test]
    fn stale_edit_actions_cannot_modify_another_record() {
        let mut state = AppState::default();
        let first = create(&mut state, "first");
        let second = create(&mut state, "second");
        assert!(!state.apply(TodoAction::BeginEdit(first)));
        assert!(!state.apply(TodoAction::BeginEdit(second)));
        assert!(!state.apply(TodoAction::EditTitle(first, "bad".into())));
        assert!(!state.apply(TodoAction::CancelEdit(first)));
        assert!(!state.apply(TodoAction::CommitEdit(first)));
        assert!(!state.apply(TodoAction::EditTitle(second, " new ".into())));
        assert!(state.apply(TodoAction::CommitEdit(second)));
        assert_eq!(state.todos()[1].title, "new");
        assert!(!state.apply(TodoAction::BeginEdit(first)));
        assert!(!state.apply(TodoAction::EditTitle(first, " ".into())));
        assert!(state.apply(TodoAction::CommitEdit(first)));
        assert_eq!(state.todos().len(), 1);
        assert!(!state.apply(TodoAction::BeginEdit(second)));
        assert!(!state.apply(TodoAction::SetFilter(Filter::Completed)));
        assert!(state.editing_title(second).is_none());
    }

    #[test]
    fn decode_validates_ids_and_preserves_high_water_mark() {
        let mut state = AppState::decode(
            r#"{"todos":[{"id":42,"title":"task","completed":false}],"next_id":1}"#,
        )
        .unwrap();
        assert_eq!(create(&mut state, "next"), 43);
        assert!(AppState::decode(r#"{"todos":[{"id":1,"title":"a","completed":false},{"id":1,"title":"b","completed":true}],"next_id":1}"#).is_err());
        let serialized = serde_json::to_string(&state.persisted).unwrap();
        assert!(state.apply(TodoAction::Delete(43)));
        let mut restored = AppState::decode(&serialized).unwrap();
        assert_eq!(create(&mut restored, "later"), 44);
        let mut exhausted =
            AppState::decode(r#"{"todos":[],"next_id":18446744073709551615}"#).unwrap();
        assert!(!exhausted.apply(TodoAction::SetNewTitle("keep".into())));
        assert!(!exhausted.apply(TodoAction::Create));
        assert_eq!(exhausted.new_todo, "keep");
    }
}
