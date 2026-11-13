//! The Mistral Vibe unified-session chunk store (v3.1.2 and later).
//!
//! A unified session directory (`unified/<uuid>/`) keeps the session's records
//! in content-addressed chunk files:
//!
//! ```text
//! chunks/<sha256>.json   a JSON array of records
//! ```
//!
//! Two kinds of record live in those files:
//!
//! * **live records** — the session's own timeline: the user's turns
//!   (`source: "turn_start"` | `"turn_steer"`), the assistant's messages
//!   (`source: "harness"`), the reasoning records and the tool effects.
//!   They carry an `id` and a `createdAt` (unix milliseconds).
//! * **history records** — the checkpoint's snapshot of the conversation the
//!   model was given (`source: "history"`) plus the synthetic injections
//!   (`source: "injection"`). They carry no timestamp.
//!
//! The recovery journal (`journal/*.jsonl`) holds only the tail of a session,
//! so it cannot answer "what did the user say" — the chunk store is the
//! session's record, and a reader that walks the journal alone sees a sliver
//! of a live session.

use std::collections::HashSet;
use std::path::Path;

use super::{RolloutMessage, summarize_tool_call, unix_ms_to_iso};

/// One chunk-store record projected into the normalized message shape.
#[derive(Debug, Clone)]
pub struct ChunkMessage {
    pub message: RolloutMessage,
    /// The record's `source`: `turn_start`, `turn_steer`, `harness`,
    /// `history` or `injection`.
    pub source: String,
    /// The record's `id`, when it has one (the live records do).
    pub id: Option<String>,
    /// The sort key: `createdAt`, or the chunk file's write time for the
    /// records that carry none (the history snapshot).
    pub sort_key: i64,
}

impl ChunkMessage {
    /// A history snapshot record (the checkpoint's copy of the conversation).
    fn is_snapshot(&self) -> bool {
        self.source == "history" || self.source == "injection"
    }
}

/// The chunk store of one unified session.
#[derive(Debug, Default)]
pub struct ChunkStore {
    /// The store's records, merged into the session's message stream.
    pub messages: Vec<ChunkMessage>,
    /// How many records the chunk files hold.
    pub records: usize,
    /// The chunk files that could not be read or parsed.
    pub damaged: Vec<String>,
}

/// Read a unified session's chunk store.
///
/// The chunk directory must be readable: an unreadable store is an error, not
/// an empty session. A chunk file that is damaged is named in
/// [`ChunkStore::damaged`] and skipped, so one bad file cannot hide the rest
/// of the session.
pub fn read_chunk_store(dir: &Path) -> Result<ChunkStore, String> {
    let chunk_dir = dir.join("chunks");
    let entries = std::fs::read_dir(&chunk_dir).map_err(|e| {
        format!(
            "cannot list vibe chunk store {}: {}",
            chunk_dir.display(),
            e
        )
    })?;

    let mut paths: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    // The chunk names are content hashes, so the file order carries no
    // chronology: the records' own timestamps order the stream.
    paths.sort();

    let mut store = ChunkStore::default();
    let mut raw: Vec<ChunkMessage> = Vec::new();

    for path in paths {
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("vibe chunk {}: cannot read: {e}", path.display());
                store.damaged.push(path.display().to_string());
                continue;
            }
        };

        let mtime_ms = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let parsed: serde_json::Value = match serde_json::from_slice(&data) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("vibe chunk {}: is not valid JSON: {e}", path.display());
                store.damaged.push(path.display().to_string());
                continue;
            }
        };

        let records: Vec<&serde_json::Value> = match &parsed {
            serde_json::Value::Array(items) => items.iter().collect(),
            serde_json::Value::Object(_) => vec![&parsed],
            _ => {
                tracing::warn!(
                    "vibe chunk {}: is neither a record nor an array of records",
                    path.display()
                );
                store.damaged.push(path.display().to_string());
                continue;
            }
        };

        for record in records {
            store.records += 1;
            match project_record(record, mtime_ms) {
                Some(message) => raw.push(message),
                None => {
                    // A record shape this reader has no read for contributes
                    // nothing: notices, checkpoints and the store's own
                    // bookkeeping are not dialogue.
                }
            }
        }
    }

    store.messages = merge(raw);
    Ok(store)
}

/// Merge the store's records into the session's message stream.
///
/// The live records are the session's own timeline and keep their order
/// (`createdAt`). The history snapshot is the same conversation in an older
/// shape, so it contributes only what the live records no longer hold: the
/// user turns that were compacted away, and the synthetic injections (the
/// compaction-continuation prompt, the runtime notifications) that mark the
/// session's boundaries.
fn merge(raw: Vec<ChunkMessage>) -> Vec<ChunkMessage> {
    let mut live: Vec<ChunkMessage> = Vec::new();
    let mut snapshot: Vec<ChunkMessage> = Vec::new();

    let mut seen_ids: HashSet<String> = HashSet::new();

    for record in raw {
        if record.is_snapshot() {
            snapshot.push(record);
            continue;
        }
        // A record repeats across chunk files (the store writes the same
        // record into more than one chunk); its `id` is the identity. Two
        // turns the user sent with the same words are two turns: only the
        // identity collapses them, never the text.
        if let Some(id) = &record.id
            && !seen_ids.insert(id.clone())
        {
            continue;
        }
        live.push(record);
    }

    // The user turns the live records hold, so a snapshot copy of one of them
    // is not counted twice.
    let live_user_texts: HashSet<String> = live
        .iter()
        .filter(|r| r.message.role == "user")
        .map(|r| normalize_text(&r.message.content))
        .filter(|t| !t.is_empty())
        .collect();

    let mut seen_snapshot: HashSet<String> = HashSet::new();
    let mut kept_snapshot: Vec<ChunkMessage> = Vec::new();
    for record in snapshot {
        // The snapshot's assistant and tool records are the live timeline in
        // an older shape; the live records carry them already.
        if record.message.role != "user" {
            continue;
        }
        let key = format!(
            "{}\u{0}{}",
            record.message.role,
            content_key(&record.message)
        );
        if !seen_snapshot.insert(key) {
            continue;
        }
        let text = normalize_text(&record.message.content);
        if !text.is_empty() && live_user_texts.contains(&text) {
            continue;
        }
        // A snapshot copy of a turn is sometimes cut at the paste boundary,
        // so the same turn can differ from the live record in its tail.
        if !text.is_empty()
            && live_user_texts
                .iter()
                .any(|live_text| same_turn(live_text, &text))
        {
            continue;
        }
        kept_snapshot.push(record);
    }

    let mut out = live;
    out.extend(kept_snapshot);
    out.sort_by_key(|record| record.sort_key);
    out
}

/// The identity of a message's content, for the duplicate check.
fn content_key(message: &RolloutMessage) -> String {
    format!(
        "{}\u{0}{}\u{0}{}",
        message.role,
        normalize_text(&message.content),
        message.thinking.as_deref().unwrap_or("")
    )
}

/// The message text with the surrounding whitespace removed: a snapshot copy
/// of a turn and the live record of the same turn differ in trailing
/// newlines, not in content.
fn normalize_text(text: &str) -> String {
    text.trim().to_string()
}

/// Whether a snapshot record and a live record are the same turn. The texts
/// are equal, or one is the other's prefix (a snapshot copy of a turn is
/// truncated at the paste boundary).
fn same_turn(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    short.len() >= 32 && long.starts_with(short)
}

/// Project one chunk-store record into a message. Returns `None` for the
/// record shapes that are not dialogue (notices, checkpoints, the store's
/// bookkeeping).
fn project_record(record: &serde_json::Value, mtime_ms: i64) -> Option<ChunkMessage> {
    let source = record
        .get("source")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();

    // The history shape: the message body is nested under `message`.
    if let Some(body) = record.get("message").filter(|m| m.is_object()) {
        let role = body.get("role").and_then(|r| r.as_str()).unwrap_or("");
        if role.is_empty() {
            return None;
        }
        let parts = read_parts(body.get("content"));
        let created_at = body.get("createdAt").and_then(|v| v.as_i64());
        return Some(ChunkMessage {
            message: RolloutMessage {
                role: role.to_string(),
                content: parts.text,
                thinking: (!parts.thinking.is_empty()).then_some(parts.thinking),
                tool_calls_summary: parts.tool_calls,
                timestamp: created_at.and_then(unix_ms_to_iso),
                injected: source == "injection",
            },
            source,
            id: record.get("id").and_then(|v| v.as_str()).map(String::from),
            sort_key: created_at.unwrap_or(mtime_ms),
        });
    }

    // The live shape: the message body is the record itself.
    if let Some(role) = record.get("role").and_then(|r| r.as_str()) {
        let parts = read_parts(record.get("content"));
        let created_at = record.get("createdAt").and_then(|v| v.as_i64());
        return Some(ChunkMessage {
            message: RolloutMessage {
                role: role.to_string(),
                content: parts.text,
                thinking: (!parts.thinking.is_empty()).then_some(parts.thinking),
                tool_calls_summary: parts.tool_calls,
                timestamp: created_at.and_then(unix_ms_to_iso),
                injected: source == "injection",
            },
            source,
            id: record.get("id").and_then(|v| v.as_str()).map(String::from),
            sort_key: created_at.unwrap_or(mtime_ms),
        });
    }

    let kind = record.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match kind {
        // The model's reasoning: a text payload of its own.
        "reasoning" => {
            let text = record
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                return None;
            }
            let created_at = record.get("createdAt").and_then(|v| v.as_i64());
            Some(ChunkMessage {
                message: RolloutMessage {
                    role: "assistant".to_string(),
                    content: String::new(),
                    thinking: Some(text),
                    tool_calls_summary: Vec::new(),
                    timestamp: created_at.and_then(unix_ms_to_iso),
                    injected: false,
                },
                source: String::new(),
                id: record.get("id").and_then(|v| v.as_str()).map(String::from),
                sort_key: created_at.unwrap_or(mtime_ms),
            })
        }
        // A tool execution: the call that was made and what it returned.
        "effect" => {
            let detail = record.get("detail");
            let tool_name = detail
                .and_then(|d| d.get("toolName"))
                .and_then(|n| n.as_str())
                .unwrap_or("tool");
            let input = detail
                .and_then(|d| d.get("input"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let args = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
            let created_at = record.get("createdAt").and_then(|v| v.as_i64());
            let output = effect_output_text(record);
            Some(ChunkMessage {
                message: RolloutMessage {
                    role: "tool".to_string(),
                    content: output,
                    thinking: None,
                    tool_calls_summary: vec![summarize_tool_call(tool_name, &args)],
                    timestamp: created_at.and_then(unix_ms_to_iso),
                    injected: false,
                },
                source: String::new(),
                id: record.get("id").and_then(|v| v.as_str()).map(String::from),
                sort_key: created_at.unwrap_or(mtime_ms),
            })
        }
        _ => None,
    }
}

/// The text of a content-part array: the plain text, the reasoning prose and
/// the tool calls a message carries.
struct Parts {
    text: String,
    thinking: String,
    tool_calls: Vec<String>,
}

fn read_parts(content: Option<&serde_json::Value>) -> Parts {
    let mut parts = Parts {
        text: String::new(),
        thinking: String::new(),
        tool_calls: Vec::new(),
    };
    let Some(content) = content else {
        return parts;
    };
    match content {
        serde_json::Value::String(text) => {
            parts.text = text.clone();
        }
        serde_json::Value::Array(items) => {
            for item in items {
                let Some(part) = item.as_object() else {
                    continue;
                };
                match part.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            append_text(&mut parts.text, text);
                        }
                    }
                    Some("reasoning") => {
                        // The reasoning prose is nested in a content array of
                        // text parts; a bare `text` field is equally valid.
                        if let Some(inner) = part.get("content").and_then(|c| c.as_array()) {
                            for nested in inner {
                                if nested.get("type").and_then(|t| t.as_str()) == Some("text")
                                    && let Some(text) = nested.get("text").and_then(|t| t.as_str())
                                {
                                    append_text(&mut parts.thinking, text);
                                }
                            }
                        }
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            append_text(&mut parts.thinking, text);
                        }
                    }
                    Some("tool_call") => {
                        let name = part.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                        let args = tool_call_args(part.get("arguments"));
                        parts.tool_calls.push(summarize_tool_call(name, &args));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    parts
}

/// The arguments of a tool-call part: a JSON string, or the wrapper object
/// (`{"raw": "<json>", "value": {...}}`) the store writes around them.
fn tool_call_args(arguments: Option<&serde_json::Value>) -> String {
    let Some(arguments) = arguments else {
        return "{}".to_string();
    };
    match arguments {
        serde_json::Value::String(raw) => raw.clone(),
        serde_json::Value::Object(map) => {
            if let Some(raw) = map.get("raw").and_then(|r| r.as_str()) {
                return raw.to_string();
            }
            serde_json::to_string(arguments).unwrap_or_else(|_| "{}".to_string())
        }
        _ => serde_json::to_string(arguments).unwrap_or_else(|_| "{}".to_string()),
    }
}

/// The output text of a tool effect: the output content parts, or the
/// structured stdout/stderr of a command.
fn effect_output_text(record: &serde_json::Value) -> String {
    let Some(output) = record.pointer("/state/output") else {
        return String::new();
    };
    if let Some(content) = output.get("content").and_then(|c| c.as_array()) {
        let mut text = String::new();
        for part in content {
            if part.get("type").and_then(|t| t.as_str()) == Some("text")
                && let Some(part_text) = part.get("text").and_then(|t| t.as_str())
            {
                append_text(&mut text, part_text);
            }
        }
        if !text.is_empty() {
            return text;
        }
    }
    if let Some(structured) = output.get("structured_content") {
        let mut text = String::new();
        for key in ["stdout", "stderr"] {
            if let Some(value) = structured.get(key).and_then(|v| v.as_str())
                && !value.is_empty()
            {
                append_text(&mut text, value);
            }
        }
        if !text.is_empty() {
            return text;
        }
    }
    output
        .get("text")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string()
}

/// Append one text fragment, newline-separating fragments.
fn append_text(into: &mut String, text: &str) {
    if text.is_empty() {
        return;
    }
    if !into.is_empty() {
        into.push('\n');
    }
    into.push_str(text);
}
