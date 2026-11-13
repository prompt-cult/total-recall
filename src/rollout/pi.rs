//! Pi adapter.
//!
//! Reads the store the Pi harness (pi, the coding-agent harness) writes:
//!
//! ```text
//! ~/.pi/agent/sessions/<project-slug>/<ISOts>_<uuid>.jsonl
//! ```
//!
//! One JSONL file per session, one typed JSON record per line. The record
//! types observed in a live store: `session` (one, first — carries the id,
//! the working directory and the store version), `model_change`,
//! `thinking_level_change`, `context_edit`, and `message` — the chat stream.
//! A `message` record wraps its payload as `message`:
//!
//! - `role` `system` — harness scaffolding (preamble/tools) — skipped,
//! - `role` `user` — the user turn (content parts of type `text`),
//! - `role` `assistant` — assistant turns: `text` parts become the message,
//!   `thinking` parts become the reasoning payload, `toolCall` parts become
//!   tool-call summaries,
//! - `role` `toolResult` — a tool execution result (content `text` parts),
//!   carrying the tool name and id in the record metadata.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{
    EventType, InterestingEvent, PI_ROOT_ENV_VAR, ReadResult, RolloutAdapter, RolloutMessage,
    SessionListing, SessionProfile, SessionSummary, listing_by_mtime, mmap_error, mtime_in_window,
    no_session_error, read_error, resolve_root, slice_from_compaction, summarize_tool_call,
};

/// The UUID in a session filename: `<ISOts>_<uuid>.jsonl`.
fn uuid_from_filename(name: &str) -> Option<String> {
    let stem = name.strip_suffix(".jsonl")?;
    let uuid = stem.rsplit('_').next()?;
    let tokens: Vec<&str> = uuid.split('-').collect();
    if tokens.len() != 5 {
        return None;
    }
    let lens = [8usize, 4, 4, 4, 12];
    for (tok, want) in tokens.iter().zip(lens) {
        if tok.len() != want || !tok.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
    }
    Some(tokens.join("-"))
}

/// The `text` of each content part that carries one.
fn parts_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The joined `thinking` payloads of an assistant message.
fn parts_thinking(content: &serde_json::Value) -> Option<String> {
    let parts = content.as_array()?;
    let joined = parts
        .iter()
        .filter_map(|p| p.get("thinking").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    (!joined.is_empty()).then_some(joined)
}

/// The tool-call summaries of an assistant message (`toolCall` parts).
fn parts_tool_calls(content: &serde_json::Value) -> Vec<String> {
    let Some(parts) = content.as_array() else {
        return Vec::new();
    };
    parts
        .iter()
        .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("toolCall"))
        .filter_map(|p| {
            let name = p.get("name").and_then(|n| n.as_str())?;
            let args = p.get("arguments");
            let args_json = args
                .map(|a| serde_json::to_string(a).unwrap_or_default())
                .unwrap_or_else(|| "{}".to_string());
            Some(summarize_tool_call(name, &args_json))
        })
        .collect()
}

/// Normalize one raw rollout record into a [`RolloutMessage`]. Records that
/// are not part of the chat stream (`session`, model/thinking-level changes,
/// context edits, system scaffolding) map to `None`.
fn record_to_message(record: &serde_json::Value) -> Option<RolloutMessage> {
    if record.get("type").and_then(|t| t.as_str()) != Some("message") {
        return None;
    }
    let inner = record.get("message")?;
    let role = inner.get("role").and_then(|r| r.as_str())?;
    if role == "system" {
        return None;
    }
    let timestamp = record
        .get("timestamp")
        .and_then(|t| t.as_str())
        .map(String::from);
    let content = inner
        .get("content")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let (normalized_role, content, thinking, tool_calls_summary) = match role {
        "user" => ("user", parts_text(&content), None, Vec::new()),
        "assistant" => (
            "assistant",
            parts_text(&content),
            parts_thinking(&content),
            parts_tool_calls(&content),
        ),
        "toolResult" => ("tool", parts_text(&content), None, Vec::new()),
        _ => return None,
    };

    Some(RolloutMessage {
        role: normalized_role.to_string(),
        content,
        thinking,
        tool_calls_summary,
        timestamp,
        injected: false,
    })
}

/// Tally one line for listing/profile counts. Mirrors the message-stream
/// role rules exactly so list counts and read/profile counts agree.
#[derive(Default)]
struct LineStats {
    user: u64,
    assistant: u64,
    tool: u64,
}

impl LineStats {
    fn tally(&mut self, record: &serde_json::Value) {
        let Some(inner) = record.get("message") else {
            return;
        };
        match inner.get("role").and_then(|r| r.as_str()) {
            Some("user") => self.user += 1,
            Some("assistant") => self.assistant += 1,
            Some("toolResult") => self.tool += 1,
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
            Err(e) => tracing::warn!("pi rollout: skipping unparseable line {}: {e}", i + 1),
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

/// The working directory of a rollout, from its `session` record (first
/// line). Used as the listing `directory` for scoped listings.
fn session_meta_directory(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&data);
    let first = text.lines().find(|l| !l.trim().is_empty())?;
    let v: serde_json::Value = serde_json::from_str(first.trim()).ok()?;
    v.get("cwd").and_then(|c| c.as_str()).map(String::from)
}

pub struct PiAdapter {
    root: PathBuf,
    from_env: bool,
}

impl PiAdapter {
    pub fn new() -> Self {
        let (root, from_env) = match crate::rollout::env_root(PI_ROOT_ENV_VAR) {
            Some(path) => (path, true),
            None => resolve_root(PI_ROOT_ENV_VAR, &[".pi", "agent", "sessions"]),
        };
        Self { root, from_env }
    }

    pub fn with_root<P: Into<PathBuf>>(root: P) -> Self {
        Self {
            root: root.into(),
            from_env: true,
        }
    }

    /// Walk the project-slug store and collect every session file with its
    /// parsed UUID and mtime, newest mtime first.
    fn session_files(&self) -> Vec<(PathBuf, String)> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Ok(files) = std::fs::read_dir(&path) else {
                continue;
            };
            for f in files.flatten() {
                let fpath = f.path();
                if !fpath.is_file() {
                    continue;
                }
                let Some(name) = fpath.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if let Some(uuid) = uuid_from_filename(name) {
                    out.push((fpath, uuid));
                }
            }
        }
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

    /// Resolve a (possibly partial) session id to a session file. Empty id =
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
            .or_else(|| {
                path.parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .map(String::from)
            })
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
            has_compaction: false,
            directory,
            parent_session_id: None,
            child_sessions: Vec::new(),
            has_tantivy_index: false,
            aliases: Vec::new(),
            read_error: None,
        }
    }
}

impl Default for PiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl RolloutAdapter for PiAdapter {
    fn name(&self) -> &'static str {
        "pi"
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
            let mut stats = LineStats::default();
            stats.tally(&v);

            if stats.user > 0 {
                *role_counts.entry("user".to_string()).or_insert(0) += stats.user;
                let summary = v
                    .get("message")
                    .and_then(|m| m.get("content"))
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
