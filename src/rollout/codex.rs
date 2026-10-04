use std::collections::HashMap;
use std::path::PathBuf;

use super::{
    CODEX_ROOT_ENV_VAR, EventType, InterestingEvent, ReadResult, RolloutAdapter, RolloutMessage,
    SessionListing, SessionProfile, SessionSummary, listing_by_mtime, mmap_error, mtime_in_window,
    no_session_error, read_error, resolve_root, resolve_session_dir, slice_from_compaction,
    summarize_tool_call,
};

/// Codex adapter. Reads flat JSONL files from ~/.codex/sessions/
///
/// Format: one JSON object per line with `role` and `content` fields.
pub struct CodexAdapter {
    root: PathBuf,
    from_env: bool,
}

impl CodexAdapter {
    pub fn new() -> Self {
        let (root, from_env) = resolve_root(CODEX_ROOT_ENV_VAR, &[".codex", "sessions"]);
        Self { root, from_env }
    }

    pub fn with_root<P: Into<PathBuf>>(root: P) -> Self {
        Self {
            root: root.into(),
            from_env: true,
        }
    }

    fn session_path(&self, session_id: &str) -> Option<PathBuf> {
        if session_id.is_empty() {
            resolve_session_dir(&self.root, "")
        } else {
            resolve_session_dir(&self.root, session_id)
        }
    }

    /// A line that is not valid JSON is skipped with a warning carrying its
    /// line number, so a damaged payload is diagnosable rather than silently
    /// short.
    fn parse_jsonl(data: &[u8]) -> Vec<RolloutMessage> {
        let text = String::from_utf8_lossy(data);
        let mut messages = Vec::new();

        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<serde_json::Value>(line) {
                Ok(v) => messages.push(json_to_rollout_message(&v)),
                Err(e) => tracing::warn!("codex rollout: skipping unparseable line {}: {e}", i + 1),
            }
        }
        messages
    }

    /// Read the raw payload bytes for a session, naming the path on failure.
    fn read_payload(&self, session_id: &str) -> ReadResult<Vec<u8>> {
        let path = self
            .session_path(session_id)
            .ok_or_else(|| no_session_error(&self.root, session_id))?;
        std::fs::read(&path).map_err(|e| read_error(&path, &e))
    }

    /// The store's candidate payload paths, enumerated exactly as the full
    /// listing enumerates them.
    fn payload_paths(&self) -> Vec<PathBuf> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("codex store {}: cannot be listed: {e}", self.root.display());
                return Vec::new();
            }
        };
        entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| !(path.is_file() && path.extension().is_none()))
            .collect()
    }
}

/// One codex payload summarized into a [`SessionSummary`]. Damage is
/// surfaced: an unreadable payload still lists, with counts from whatever
/// could be read and a `read_error` message. Callers apply the mtime window
/// before calling, so this payload read only ever happens for in-window
/// sessions.
fn summarize_payload(path: &std::path::Path) -> SessionSummary {
    let name_str = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // Distinguish read damage (permissions, I/O error) from a payload
    // that does not exist yet: damage is surfaced on the entry.
    let (data, read_error) = match std::fs::read(path) {
        Ok(data) => (data, None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), None),
        Err(e) => (Vec::new(), Some(read_error(path, &e))),
    };
    let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let line_count = String::from_utf8_lossy(&data)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count() as u64;

    let mut user_count = 0u64;
    let mut assistant_count = 0u64;
    let mut tool_count = 0u64;
    let mut has_compaction = false;

    for line in String::from_utf8_lossy(&data).lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
            match role {
                "user" => {
                    user_count += 1;
                    let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
                    if content.contains("context compaction") {
                        has_compaction = true;
                    }
                }
                "assistant" => assistant_count += 1,
                "tool" => tool_count += 1,
                _ => {}
            }
        }
    }

    SessionSummary {
        session_id: name_str,
        title: String::new(),
        start_time: String::new(),
        end_time: String::new(),
        file_size,
        line_count,
        user_count,
        assistant_count,
        tool_count,
        has_compaction,
        directory: None,
        parent_session_id: None,
        child_sessions: Vec::new(),
        has_tantivy_index: false,
        aliases: Vec::new(),
        read_error,
    }
}

fn json_to_rollout_message(v: &serde_json::Value) -> RolloutMessage {
    let role = v
        .get("role")
        .and_then(|r| r.as_str())
        .unwrap_or("unknown")
        .to_string();

    let content = v
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    let injected = v.get("injected").and_then(|i| i.as_bool()).unwrap_or(false);

    let timestamp = v
        .get("timestamp")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string());

    let mut tool_calls_summary = Vec::new();
    if let Some(tool_calls) = v.get("tool_calls").and_then(|tc| tc.as_array()) {
        for tc in tool_calls {
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .or_else(|| tc.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("?");
            let args = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
                .unwrap_or("{}");
            tool_calls_summary.push(summarize_tool_call(name, args));
        }
    }

    RolloutMessage {
        role,
        content,
        thinking: None,
        tool_calls_summary,
        timestamp,
        injected,
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
            .unwrap_or(std::path::Path::new("."))
            .join(".tantivy")
    }

    fn list_sessions(&self) -> Vec<SessionSummary> {
        let mut summaries: Vec<SessionSummary> = self
            .payload_paths()
            .into_iter()
            .map(|path| summarize_payload(&path))
            .collect();
        summaries.sort_by(|a, b| b.session_id.cmp(&a.session_id));
        summaries
    }

    fn list_sessions_scoped(&self, hours_back: u64, _directory: Option<&str>) -> SessionListing {
        // Codex payloads carry no directory (None passes every filter, as in
        // the full listing), so the mtime window is the only cheap bound.
        let mut rows = Vec::new();
        for path in self.payload_paths() {
            let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            if !mtime_in_window(mtime, hours_back) {
                continue;
            }
            rows.push((mtime, summarize_payload(&path)));
        }
        listing_by_mtime(rows)
    }

    fn read_session(&self, session_id: &str) -> ReadResult<Vec<RolloutMessage>> {
        Ok(Self::parse_jsonl(&self.read_payload(session_id)?))
    }

    fn read_session_mmap(&self, session_id: &str) -> ReadResult<Vec<RolloutMessage>> {
        let path = self
            .session_path(session_id)
            .ok_or_else(|| no_session_error(&self.root, session_id))?;
        let file = std::fs::File::open(&path).map_err(|e| read_error(&path, &e))?;
        let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| mmap_error(&path, &e))?;
        Ok(Self::parse_jsonl(&mmap[..]))
    }

    fn read_session_from_compaction(&self, session_id: &str) -> ReadResult<Vec<RolloutMessage>> {
        Ok(slice_from_compaction(self.read_session_mmap(session_id)?))
    }

    fn profile_session_opts(&self, session_id: &str, cache: bool) -> ReadResult<SessionProfile> {
        let path = self
            .session_path(session_id)
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
        let file_size = data.len() as u64;
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

            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                let role = v
                    .get("role")
                    .and_then(|r| r.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                *role_counts.entry(role.clone()).or_insert(0) += 1;

                let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");

                if content.contains("context compaction") {
                    interesting_events.push(InterestingEvent {
                        line_number: line_num,
                        event_type: EventType::Compaction,
                        summary: "Compaction marker".to_string(),
                        gap_lines: line_num - last_event_line,
                    });
                    last_event_line = line_num;
                }

                if role == "user" {
                    interesting_events.push(InterestingEvent {
                        line_number: line_num,
                        event_type: EventType::UserMessage,
                        summary: content.chars().take(80).collect(),
                        gap_lines: line_num - last_event_line,
                    });
                    last_event_line = line_num;
                }

                if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                    if first_ts.is_none() {
                        first_ts = Some(ts.to_string());
                    }
                    last_ts = Some(ts.to_string());
                }
            }
        }

        let profile = SessionProfile {
            session_id: session_id.to_string(),
            file_size,
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
}
