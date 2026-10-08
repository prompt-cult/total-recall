//! Todo-list history: replay a rollout's `todowrite` flushes as a JSONL
//! stream of edit events — what was done to the todo list, and when.
//!
//! The opencode store keeps every `todowrite` call as a tool part carrying
//! the whole todo list as the tool wrote it. The raw flushes are snapshots;
//! the events are the edits between consecutive snapshots, which is what
//! "what was done, and the date/time of what was done" means. Folding the
//! events reproduces the snapshots; the events are the smaller, honest
//! record.

/// One todo item as a `todowrite` flush carries it. `status` and `priority`
/// are optional in the source: a flush that omits them still diffs (an
/// absent status reads as the empty string, an absent priority as `None`).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct TodoItemState {
    pub content: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub priority: Option<String>,
}

/// One `todowrite` flush: the whole todo list as the tool wrote it, with the
/// ISO8601 timestamp of the message that carried it (`None` when the source
/// message carries no creation time).
#[derive(Debug, Clone)]
pub struct TodoWrite {
    pub timestamp: Option<String>,
    pub todos: Vec<TodoItemState>,
}

/// One edit between two consecutive todo-list states, serialized as the
/// JSONL line `{"ts":…,"action":…,"todo":…[,"status":…]}`. `action` is
/// `added`, `updated`, the item's new status on a status change, or
/// `removed`. `status` is the item's status after the edit and is omitted
/// for `removed`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TodoEditEvent {
    pub ts: String,
    pub action: String,
    pub todo: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// The identity key of a todo item: the `itemNN:` / `itemNN.NN:` slug prefix
/// this repo's sessions carry, else the full content string. A slugged item
/// that is reworded keeps its identity (one `updated` event); an unslugged
/// item that is reworded is a different item (a `removed` plus an `added`).
fn item_key(content: &str) -> &str {
    let Some(rest) = content.strip_prefix("item") else {
        return content;
    };
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return content;
    }
    let rest = &rest[digits..];
    let rest = match rest.strip_prefix('.') {
        Some(after_dot) => {
            let dot_digits = after_dot.chars().take_while(|c| c.is_ascii_digit()).count();
            if dot_digits == 0 {
                return content;
            }
            &after_dot[dot_digits..]
        }
        None => rest,
    };
    let Some(after_colon) = rest.strip_prefix(':') else {
        return content;
    };
    if after_colon.starts_with(char::is_whitespace) {
        &content[..content.len() - after_colon.len()]
    } else {
        content
    }
}

/// The edits between consecutive todo-list states, in input order. The first
/// flush adds every item; a flush identical to the previous state emits
/// nothing; a status change is actioned as the new status; a content change
/// with an unchanged status is `updated`; an item the flush dropped is
/// `removed` (the todowrite protocol replaces the whole list, so a vanished
/// key was removed — reported, never hidden).
pub fn diff_todo_states(states: &[TodoWrite]) -> Vec<TodoEditEvent> {
    let mut events = Vec::new();
    // The previous flush's items as (key, item) pairs, in list order.
    let mut prev: Vec<(String, TodoItemState)> = Vec::new();

    for write in states {
        let ts = write.timestamp.clone().unwrap_or_else(|| "0".to_string());
        let mut current: Vec<(String, TodoItemState)> = Vec::with_capacity(write.todos.len());
        for item in &write.todos {
            let key = item_key(&item.content).to_string();
            match prev.iter().find(|(k, _)| *k == key) {
                None => events.push(TodoEditEvent {
                    ts: ts.clone(),
                    action: "added".to_string(),
                    todo: item.content.clone(),
                    status: Some(item.status.clone()),
                }),
                Some((_, old)) => {
                    if old.status != item.status {
                        events.push(TodoEditEvent {
                            ts: ts.clone(),
                            action: item.status.clone(),
                            todo: item.content.clone(),
                            status: Some(item.status.clone()),
                        });
                    } else if old.content != item.content {
                        events.push(TodoEditEvent {
                            ts: ts.clone(),
                            action: "updated".to_string(),
                            todo: item.content.clone(),
                            status: Some(item.status.clone()),
                        });
                    }
                }
            }
            current.push((key, item.clone()));
        }
        for (key, old) in &prev {
            if !current.iter().any(|(k, _)| k == key) {
                events.push(TodoEditEvent {
                    ts: ts.clone(),
                    action: "removed".to_string(),
                    todo: old.content.clone(),
                    status: None,
                });
            }
        }
        prev = current;
    }
    events
}
