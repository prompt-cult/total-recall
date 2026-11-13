//! Modern Codex adapter.
//!
//! Reads the store current Codex desktop (26.x) writes:
//!
//! ```text
//! ~/.codex/sessions/<YYYY>/<MM>/<DD>/rollout-<ts>-<uuid>.jsonl
//! ```
//!
//! Each line is one typed JSON record. The types observed in a live store:
//! `session_meta` (one, first), `response_item`, `event_msg`,
//! `token_usage_record`, `turn_context`, `world_state`, `compacted`. The
//! chat stream lives in `response_item` payloads:
//!
//! - `message` with `role` `user` / `assistant` / `developer` (content is a
//!   `content[]` array of `input_text` / `output_text` parts; developer
//!   messages are the harness's own system scaffolding and are skipped),
//! - `custom_tool_call` / `function_call` (an assistant tool call — the call
//!   becomes an assistant message carrying the normalized tool-call summary),
//! - `custom_tool_call_output` / `function_call_output` (the tool result — a
//!   `tool`-role message),
//! - `reasoning` — encrypted at rest (`encrypted_content`), so it is skipped;
//!   the plaintext is unrecoverable by design.
//!
//! A typed `compacted` record marks a context compaction: it is replayed as
//! a synthetic injected marker message, so the shared compaction slicing and
//! marker logic work unchanged and no prose can forge a boundary. The old
//! flat-store adapter (pre-0.20.0) read `~/.codex/sessions/*.jsonl`
//! directly; this adapter walks the date tree and reads the modern record
//! stream.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{
    CODEX_ROOT_ENV_VAR, EventType, InterestingEvent, ReadResult, RolloutAdapter, RolloutMessage,
    SessionListing, SessionProfile, SessionSummary, listing_by_mtime, mmap_error, mtime_in_window,
    no_session_error, read_error, resolve_root, slice_from_compaction, summarize_tool_call,
};

/// The UUID in a rollout filename: `rollout-<ts>-<uuid>.jsonl`.
fn uuid_from_filename(name: &str) -> Option<String> {
    let stem = name.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
    let tokens: Vec<&str> = stem.split('-').collect();
    // The UUID is the last five dash-separated segments (8-4-4-4-12 hex).
    if tokens.len() < 5 {
        return None;
    }
    let uuid_tokens = &tokens[tokens.len() - 5..];
    let lens = [8usize, 4, 4, 4, 12];
    for (tok, want) in uuid_tokens.iter().zip(lens) {
        if tok.len() != want || !tok.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
    }
    Some(uuid_tokens.join("-"))
}

/// The content parts of a `message` payload: the `text` of each part that
/// carries one (`input_text`, `output_text`, ...). Non-text parts are
/// skipped.
fn message_text(payload: &serde_json::Value) -> String {
    payload
        .get("content")
        .and_then(|c| c.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Normalize one raw rollout record into a [`RolloutMessage`]. Records that
/// are not part of the chat stream (`session_meta`, token accounting,
/// `event_msg` lifecycle records, encrypted reasoning, developer scaffolding)
/// map to `None`.
fn record_to_message(record: &serde_json::Value) -> Option<RolloutMessage> {
    let record_type = record.get("type").and_then(|t| t.as_str())?;
    let timestamp = record
        .get("timestamp")
        .and_then(|t| t.as_str())
        .map(String::from);

    // A typed compaction record replays as a synthetic injected marker so the
    // shared compaction machinery (slicing, markers, listing flag) sees it.
    if record_type == "compacted" {
        return Some(RolloutMessage {
            role: "system".to_string(),
            content: "context compaction".to_string(),
            thinking: None,
            tool_calls_summary: Vec::new(),
            timestamp,
            injected: true,
        });
    }

    if record_type != "response_item" {
        return None;
    }
    let payload = record.get("payload")?;
    let payload_type = payload.get("type").and_then(|t| t.as_str())?;

    match payload_type {
        "message" => {
            let role = payload.get("role").and_then(|r| r.as_str())?;
            // Developer messages are the harness's system scaffolding (base
            // instructions, tool definitions) — not a chat turn.
            if role == "developer" {
                return None;
            }
            Some(RolloutMessage {
                role: role.to_string(),
                content: message_text(payload),
                thinking: None,
                tool_calls_summary: Vec::new(),
                timestamp,
                injected: false,
            })
        }
        "custom_tool_call" | "function_call" => {
            let name = payload.get("name").and_then(|n| n.as_str()).unwrap_or("?");
            let input = payload
                .get("input")
                .or_else(|| payload.get("arguments"))
                .and_then(|a| a.as_str())
                .unwrap_or("{}");
            Some(RolloutMessage {
                role: "assistant".to_string(),
                content: String::new(),
                thinking: None,
                tool_calls_summary: vec![summarize_tool_call(name, input)],
                timestamp,
                injected: false,
            })
        }
        "custom_tool_call_output" | "function_call_output" => {
            // The output is an array of text parts (custom) or a JSON string
            // (function). Join the parts; keep the raw string otherwise.
            let content = match payload.get("output") {
                Some(serde_json::Value::Array(parts)) => parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n"),
                Some(serde_json::Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            Some(RolloutMessage {
                role: "tool".to_string(),
                content,
                thinking: None,
                tool_calls_summary: Vec::new(),
                timestamp,
                injected: false,
            })
        }
        // Reasoning is encrypted at rest; nothing readable to carry.
        "reasoning" => None,
        _ => None,
    }
}

/// Tally one line for listing/profile counts. Mirrors the message-stream
/// role rules exactly so list counts and read/profile counts agree.
#[derive(Default)]
struct LineStats {
    user: u64,
    assistant: u64,
    tool: u64,
    compacted: bool,
}

impl LineStats {
    fn tally(&mut self, record: &serde_json::Value) {
        let Some(record_type) = record.get("type").and_then(|t| t.as_str()) else {
            return;
        };
        if record_type == "compacted" {
            self.compacted = true;
            return;
        }
        if record_type != "response_item" {
            return;
        }
        let Some(payload) = record.get("payload") else {
            return;
        };
        let Some(payload_type) = payload.get("type").and_then(|t| t.as_str()) else {
            return;
        };
        match payload_type {
            "message" => {
                if payload.get("role").and_then(|r| r.as_str()) == Some("user") {
                    self.user += 1;
                } else if payload.get("role").and_then(|r| r.as_str()) == Some("assistant") {
                    self.assistant += 1;
                }
            }
            "custom_tool_call" | "function_call" => self.assistant += 1,
            "custom_tool_call_output" | "function_call_output" => self.tool += 1,
            _ => {}
        }
    }
}

/// Parse a full rollout payload into the normalized message stream.
fn parse_jsonl(data: &[u8]) -> Vec<RolloutMessage> {
    let text = String::from_utf8_lossy(data);
    let mut messages = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => {
                if let Some(m) = record_to_message(&v) {
                    messages.push(m);
                }
            }
            Err(e) => tracing::warn!("codex rollout: skipping unparseable line {}: {e}", i + 1),
        }
    }
    messages
}

fn parse_jsonl_mmap(path: &Path) -> ReadResult<Vec<RolloutMessage>> {
    let file = std::fs::File::open(path).map_err(|e| read_error(path, &e))?;
    let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| mmap_error(path, &e))?;
    Ok(parse_jsonl(&mmap[..]))
}

/// Full-file stats for one rollout (exact counts, one read).
fn file_stats(path: &Path) -> ReadResult<(LineStats, u64)> {
    let data = std::fs::read(path).map_err(|e| read_error(path, &e))?;
    let text = String::from_utf8_lossy(&data);
    let mut stats = LineStats::default();
    let mut lines = 0u64;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        lines += 1;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            stats.tally(&v);
        }
    }
    Ok((stats, lines))
}

/// The first and last record timestamps of a rollout: reads the file head and
/// tail only, never the body.
fn first_last_timestamps(path: &Path) -> (Option<String>, Option<String>) {
    let read_ts = |data: &[u8], want_last: bool| -> Option<String> {
        let text = String::from_utf8_lossy(data);
        let line = if want_last {
            text.lines().rev().find(|l| !l.trim().is_empty())
        } else {
            text.lines().find(|l| !l.trim().is_empty())
        }?;
        let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        v.get("timestamp")
            .and_then(|t| t.as_str())
            .map(String::from)
    };

    let first = std::fs::read(path).ok().and_then(|d| read_ts(&d, false));
    // Tail read: the last 128 KiB usually cover the final record.
    let last = std::fs::File::open(path).ok().and_then(|f| {
        let len = f.metadata().ok()?.len();
        use std::io::{Read, Seek, SeekFrom};
        let start = len.saturating_sub(128 * 1024);
        let mut buf = vec![0u8; (len - start) as usize];
        let mut f = f;
        f.seek(SeekFrom::Start(start)).ok()?;
        f.read_exact(&mut buf).ok()?;
        read_ts(&buf, true)
    });
    (first, last)
}

/// The working directory of a rollout, from its `session_meta` record (first
/// line). Used as the listing `directory` for scoped listings.
fn session_meta_directory(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&data);
    let first = text.lines().find(|l| !l.trim().is_empty())?;
    let v: serde_json::Value = serde_json::from_str(first.trim()).ok()?;
    let payload = v.get("payload")?;
    payload
        .get("cwd")
        .and_then(|c| c.as_str())
        .map(String::from)
}

pub struct CodexAdapter {
    root: PathBuf,
    from_env: bool,
}

impl CodexAdapter {
    pub fn new() -> Self {
        let (root, from_env) = match crate::rollout::env_root(CODEX_ROOT_ENV_VAR) {
            Some(path) => (path, true),
            None => resolve_root(CODEX_ROOT_ENV_VAR, &[".codex", "sessions"]),
        };
        Self { root, from_env }
    }

    pub fn with_root<P: Into<PathBuf>>(root: P) -> Self {
        Self {
            root: root.into(),
            from_env: true,
        }
    }

    /// Walk the date-hierarchical store and collect every rollout file with
    /// its parsed UUID and mtime, newest mtime first.
    fn session_files(&self) -> Vec<(PathBuf, String)> {
        let mut out = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Some(name) = path.file_name().and_then(|n| n.to_str())
                    && let Some(uuid) = uuid_from_filename(name)
                {
                    out.push((path, uuid));
                }
            }
        }
        out.sort_by_key(|(_, uuid)| uuid.clone());
        out.sort_by_key(|(path, _)| {
            std::cmp::Reverse(
                std::fs::metadata(path)
                    .and_then(|m| m.modified())
                    .ok()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            )
        });
        out
    }

    /// Resolve a (possibly partial) session id to a rollout file. Empty id =
    /// most recent session. A partial id matches UUID prefixes; most recent
    /// wins. An id nothing matches yields `None` — the caller produces the
    /// precise "no session matching … under …" error.
    fn resolve_path(&self, session_id: &str) -> Option<PathBuf> {
        let files = self.session_files();
        if session_id.is_empty() {
            return files.into_iter().next().map(|(path, _)| path);
        }
        files
            .into_iter()
            .find(|(_, uuid)| uuid.starts_with(session_id))
            .map(|(path, _)| path)
    }

    fn summarize_file(&self, path: &Path, uuid: &str) -> SessionSummary {
        let (stats, line_count) = match file_stats(path) {
            Ok(v) => v,
            Err(e) => {
                // The listing row should still appear, with the failure
                // visible rather than silently dropped.
                return SessionSummary {
                    session_id: uuid.to_string(),
                    title: String::new(),
                    start_time: String::new(),
                    end_time: String::new(),
                    file_size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
                    line_count: 0,
                    user_count: 0,
                    assistant_count: 0,
                    tool_count: 0,
                    has_compaction: false,
                    directory: None,
                    parent_session_id: None,
                    child_sessions: Vec::new(),
                    has_tantivy_index: false,
                    aliases: Vec::new(),
                    read_error: Some(e),
                };
            }
        };
        let (first_ts, last_ts) = first_last_timestamps(path);
        let directory = session_meta_directory(path);
        let title = directory
            .as_deref()
            .and_then(|d| Path::new(d).file_name())
            .and_then(|n| n.to_str())
            .map(String::from)
            .unwrap_or_default();
        SessionSummary {
            session_id: uuid.to_string(),
            title,
            start_time: first_ts.unwrap_or_default(),
            end_time: last_ts.unwrap_or_default(),
            file_size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            line_count,
            user_count: stats.user,
            assistant_count: stats.assistant,
            tool_count: stats.tool,
            has_compaction: stats.compacted,
            directory,
            parent_session_id: None,
            child_sessions: Vec::new(),
            has_tantivy_index: false,
            aliases: Vec::new(),
            read_error: None,
        }
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl RolloutAdapter for CodexAdapter {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn root_is_from_env(&self) -> bool {
        self.from_env
    }

    fn shadow_index_root(&self) -> PathBuf {
        self.root
            .parent()
            .unwrap_or(Path::new("."))
            .join(".tantivy")
    }

    fn list_sessions(&self) -> Vec<SessionSummary> {
        let mut summaries: Vec<SessionSummary> = self
            .session_files()
            .iter()
            .map(|(path, uuid)| self.summarize_file(path, uuid))
            .collect();
        summaries.sort_by(|a, b| b.start_time.cmp(&a.start_time));
        summaries
    }

    fn list_sessions_scoped(&self, hours_back: u64, directory: Option<&str>) -> SessionListing {
        let mut rows = Vec::new();
        for (path, uuid) in self.session_files() {
            let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            if !mtime_in_window(mtime, hours_back) {
                continue;
            }
            let summary = self.summarize_file(&path, &uuid);
            let passes = match (summary.directory.as_deref(), directory) {
                (Some(dir), Some(filter)) if !filter.is_empty() => {
                    dir.to_lowercase().contains(&filter.to_lowercase())
                }
                _ => true,
            };
            if !passes {
                continue;
            }
            rows.push((mtime, summary));
        }
        listing_by_mtime(rows)
    }

    fn read_session(&self, session_id: &str) -> ReadResult<Vec<RolloutMessage>> {
        let path = self
            .resolve_path(session_id)
            .ok_or_else(|| no_session_error(&self.root, session_id))?;
        let data = std::fs::read(&path).map_err(|e| read_error(&path, &e))?;
        Ok(parse_jsonl(&data))
    }

    fn read_session_mmap(&self, session_id: &str) -> ReadResult<Vec<RolloutMessage>> {
        let path = self
            .resolve_path(session_id)
            .ok_or_else(|| no_session_error(&self.root, session_id))?;
        parse_jsonl_mmap(&path)
    }

    fn read_session_from_compaction(&self, session_id: &str) -> ReadResult<Vec<RolloutMessage>> {
        let messages = self.read_session_mmap(session_id)?;
        Ok(slice_from_compaction(messages))
    }

    /// The typed `compacted` record replays as an injected marker message;
    /// only those count as boundaries — prose never can.
    fn compaction_markers(&self, messages: &[RolloutMessage]) -> Vec<usize> {
        messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.injected && m.content.contains("context compaction"))
            .map(|(i, _)| i)
            .collect()
    }

    fn profile_session_opts(&self, session_id: &str, cache: bool) -> ReadResult<SessionProfile> {
        let path = self
            .resolve_path(session_id)
            .ok_or_else(|| no_session_error(&self.root, session_id))?;

        let cache_path = crate::profile_cache::cache_path(&self.shadow_index_root(), session_id);
        if cache
            && let Some(cached) = crate::profile_cache::read_fresh(
                &cache_path,
                crate::profile_cache::mtime_ms(&path).unwrap_or(0),
            )
        {
            return Ok(cached);
        }

        let data = std::fs::read(&path).map_err(|e| read_error(&path, &e))?;
        let text = String::from_utf8_lossy(&data);

        let mut role_counts: HashMap<String, u64> = HashMap::new();
        let mut interesting_events = Vec::new();
        let mut last_event_line = 0u64;
        let mut first_ts: Option<String> = None;
        let mut last_ts: Option<String> = None;
        let mut line_count = 0u64;

        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            line_count += 1;
            let line_num = (i + 1) as u64;

            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let record_type = v.get("type").and_then(|t| t.as_str());
            let mut stats = LineStats::default();
            stats.tally(&v);

            if record_type == Some("compacted") {
                interesting_events.push(InterestingEvent {
                    line_number: line_num,
                    event_type: EventType::Compaction,
                    summary: "Compaction marker".to_string(),
                    gap_lines: line_num - last_event_line,
                });
                last_event_line = line_num;
                continue;
            }

            if stats.user > 0 {
                *role_counts.entry("user".to_string()).or_insert(0) += stats.user;
                // The first user message is the session's ask; surface it as
                // the interesting event summary.
                let summary = v
                    .get("payload")
                    .and_then(|p| p.get("content"))
                    .and_then(|c| c.as_array())
                    .and_then(|parts| {
                        parts
                            .iter()
                            .find_map(|p| p.get("text").and_then(|t| t.as_str()))
                    })
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect::<String>();
                interesting_events.push(InterestingEvent {
                    line_number: line_num,
                    event_type: EventType::UserMessage,
                    summary,
                    gap_lines: line_num - last_event_line,
                });
                last_event_line = line_num;
            }
            if stats.assistant > 0 {
                *role_counts.entry("assistant".to_string()).or_insert(0) += stats.assistant;
            }
            if stats.tool > 0 {
                *role_counts.entry("tool".to_string()).or_insert(0) += stats.tool;
            }

            if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                if first_ts.is_none() {
                    first_ts = Some(ts.to_string());
                }
                last_ts = Some(ts.to_string());
            }
        }

        let profile = SessionProfile {
            session_id: session_id.to_string(),
            file_size: data.len() as u64,
            line_count,
            first_ts,
            last_ts,
            role_counts,
            has_tantivy_index: false,
            interesting_events,
        };
        if cache {
            crate::profile_cache::write(&cache_path, &profile);
        }
        Ok(profile)
    }

    fn extract_user_messages(&self, session_id: &str) -> ReadResult<Vec<String>> {
        Ok(self
            .read_session(session_id)?
            .into_iter()
            .filter(|m| m.role == "user" && !m.injected)
            .map(|m| m.content)
            .collect())
    }

    fn most_recent_session_id(&self) -> Option<String> {
        self.session_files()
            .into_iter()
            .next()
            .map(|(_, uuid)| uuid)
    }

    fn resolve_canonical_id(&self, session_id: &str) -> Option<String> {
        if session_id.is_empty() {
            return self.most_recent_session_id();
        }
        self.session_files()
            .into_iter()
            .find(|(_, uuid)| uuid.starts_with(session_id))
            .map(|(_, uuid)| uuid)
    }
}
