use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{
    CLAUDE_ROOT_ENV_VAR, EventType, InterestingEvent, RolloutAdapter, RolloutMessage,
    SessionProfile, SessionSummary, resolve_root, slice_from_compaction, summarize_tool_call,
};

/// Claude adapter. Reads JSONL files from ~/.claude/projects/
///
/// Format: one JSON object per line with `type` ("user"/"assistant") and
/// nested `message.content` field.
pub struct ClaudeAdapter {
    root: PathBuf,
    from_env: bool,
}

impl ClaudeAdapter {
    pub fn new() -> Self {
        let (root, from_env) = resolve_root(CLAUDE_ROOT_ENV_VAR, &[".claude", "projects"]);
        Self { root, from_env }
    }

    pub fn with_root<P: Into<PathBuf>>(root: P) -> Self {
        Self {
            root: root.into(),
            from_env: true,
        }
    }

    fn session_path(&self, session_id: &str) -> Option<PathBuf> {
        let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
        for (_, path) in self.session_files() {
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
            let matches = stem.as_deref().is_some_and(|s| s.contains(session_id));
            if !matches {
                continue;
            }
            let mtime = path
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            if best.as_ref().is_none_or(|(_, t)| mtime > *t) {
                best = Some((path, mtime));
            }
        }
        best.map(|(p, _)| p)
    }

    /// Discover session JSONL files. The real Claude Code layout nests them
    /// one project-directory level down (`<root>/<project>/<uuid>.jsonl`);
    /// flat files under root (and a root that is itself a file) are kept for
    /// fixture compatibility. `subagents/` and other nested dirs are not
    /// sessions. Returns (project dir name, path).
    fn session_files(&self) -> Vec<(Option<String>, PathBuf)> {
        let mut out = Vec::new();
        if self.root.is_file() {
            out.push((None, self.root.clone()));
            return out;
        }
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && is_jsonl(&path) {
                out.push((None, path));
            } else if path.is_dir() {
                let dir_name = entry.file_name().to_string_lossy().into_owned();
                let Ok(subs) = std::fs::read_dir(&path) else {
                    continue;
                };
                for sub in subs.flatten() {
                    let sp = sub.path();
                    if sp.is_file() && is_jsonl(&sp) {
                        out.push((Some(dir_name.clone()), sp));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.1.cmp(&b.1));
        out
    }

    fn parse_jsonl(data: &[u8]) -> Vec<RolloutMessage> {
        let text = String::from_utf8_lossy(data);
        let mut messages = Vec::new();

        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
                && let Some(msg) = json_to_rollout_message(&v)
            {
                messages.push(msg);
            }
        }
        messages
    }
}

fn is_jsonl(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "jsonl")
}

/// Classify a JSONL line. Only user/assistant/tool lines are messages;
/// ai-title/mode/summary/step markers and unknown types are not. Returns
/// `None` for non-message lines (the title line is reported separately).
fn classify_line(v: &serde_json::Value) -> Option<(String, serde_json::Value)> {
    let kind = v
        .get("type")
        .or_else(|| v.get("role"))
        .and_then(|r| r.as_str())?;
    match kind {
        "user" | "assistant" | "tool" => Some((kind.to_string(), v.clone())),
        _ => None,
    }
}

fn json_to_rollout_message(v: &serde_json::Value) -> Option<RolloutMessage> {
    let (role, v) = classify_line(v)?;

    // Content can be in message.content (string or array of content blocks)
    let content = extract_content(&v);

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

    // Also check message.content array for tool_use blocks
    if let Some(content_arr) = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        for block in content_arr {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                let input = block.get("input").unwrap_or(&serde_json::Value::Null);
                let args_str = serde_json::to_string(input).unwrap_or_default();
                tool_calls_summary.push(summarize_tool_call(name, &args_str));
            }
        }
    }

    let thinking = extract_thinking(&v);

    Some(RolloutMessage {
        role,
        content,
        thinking,
        tool_calls_summary,
        timestamp,
        injected,
    })
}

/// Collect `thinking` blocks from message.content into the thinking field.
fn extract_thinking(v: &serde_json::Value) -> Option<String> {
    let content_arr = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())?;
    let thinking: Vec<String> = content_arr
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("thinking"))
        .filter_map(|b| b.get("thinking").and_then(|t| t.as_str()))
        .map(str::to_string)
        .collect();
    if thinking.is_empty() {
        None
    } else {
        Some(thinking.join("\n"))
    }
}

/// Extract content from a Claude JSONL line.
/// Content can be a string at message.content, or an array of content blocks.
fn extract_content(v: &serde_json::Value) -> String {
    // Try message.content as string
    if let Some(content) = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
    {
        return content.to_string();
    }

    // Try message.content as array of content blocks
    if let Some(content_arr) = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        let mut parts = Vec::new();
        for block in content_arr {
            if let Some(block_type) = block.get("type").and_then(|t| t.as_str()) {
                match block_type {
                    "text" => {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            parts.push(text.to_string());
                        }
                    }
                    "tool_result" => match block.get("content") {
                        Some(serde_json::Value::String(content)) => {
                            parts.push(format!("TOOL RESULT: {content}"));
                        }
                        Some(serde_json::Value::Array(blocks)) => {
                            for tb in blocks {
                                if tb.get("type").and_then(|t| t.as_str()) == Some("text")
                                    && let Some(text) = tb.get("text").and_then(|t| t.as_str())
                                {
                                    parts.push(format!("TOOL RESULT: {text}"));
                                }
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
        return parts.join("\n");
    }

    // Fallback: top-level content field
    v.get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string()
}

impl Default for ClaudeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl RolloutAdapter for ClaudeAdapter {
    fn name(&self) -> &'static str {
        "claude"
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
        let mut summaries = Vec::new();

        for (directory, path) in self.session_files() {
            let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let data = std::fs::read(&path).unwrap_or_default();
            let text = String::from_utf8_lossy(&data);

            let mut line_count = 0u64;
            let mut user_count = 0u64;
            let mut assistant_count = 0u64;
            let mut tool_count = 0u64;
            let mut has_compaction = false;
            let mut title = String::new();
            let mut start_time = String::new();
            let mut end_time = String::new();

            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                line_count += 1;
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };

                if title.is_empty()
                    && let Some(t) = v.get("aiTitle").and_then(|t| t.as_str())
                {
                    title = t.to_string();
                }

                if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                    if start_time.is_empty() {
                        start_time = ts.to_string();
                    }
                    end_time = ts.to_string();
                }

                let Some((role, v)) = classify_line(&v) else {
                    continue;
                };
                match role.as_str() {
                    "user" => {
                        user_count += 1;
                        let content = extract_content(&v);
                        if content.contains("context compaction") {
                            has_compaction = true;
                        }
                    }
                    "assistant" => assistant_count += 1,
                    "tool" => tool_count += 1,
                    _ => {}
                }
            }

            let session_id = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();

            summaries.push(SessionSummary {
                session_id,
                title,
                start_time,
                end_time,
                file_size,
                line_count,
                user_count,
                assistant_count,
                tool_count,
                has_compaction,
                directory,
                parent_session_id: None,
                child_sessions: Vec::new(),
                has_tantivy_index: false,
                aliases: Vec::new(),
                read_error: None,
            });
        }

        summaries.sort_by(|a, b| b.session_id.cmp(&a.session_id));
        summaries
    }

    fn read_session(&self, session_id: &str) -> Vec<RolloutMessage> {
        let path = match self.session_path(session_id) {
            Some(p) => p,
            None => return Vec::new(),
        };
        let data = std::fs::read(&path).unwrap_or_default();
        Self::parse_jsonl(&data)
    }

    fn read_session_mmap(&self, session_id: &str) -> Vec<RolloutMessage> {
        let path = match self.session_path(session_id) {
            Some(p) => p,
            None => return Vec::new(),
        };
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };
        let mmap = match unsafe { memmap2::Mmap::map(&file) } {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };
        Self::parse_jsonl(&mmap[..])
    }

    fn read_session_from_compaction(&self, session_id: &str) -> Vec<RolloutMessage> {
        slice_from_compaction(self.read_session_mmap(session_id))
    }

    fn profile_session_opts(&self, session_id: &str, cache: bool) -> SessionProfile {
        let path = match self.session_path(session_id) {
            Some(p) => p,
            None => {
                return SessionProfile {
                    session_id: session_id.to_string(),
                    file_size: 0,
                    line_count: 0,
                    first_ts: None,
                    last_ts: None,
                    role_counts: HashMap::new(),
                    has_tantivy_index: false,
                    interesting_events: Vec::new(),
                };
            }
        };

        let cache_path = crate::profile_cache::cache_path(&self.shadow_index_root(), session_id);
        if cache
            && let Some(cached) = crate::profile_cache::read_fresh(
                &cache_path,
                crate::profile_cache::mtime_ms(&path).unwrap_or(0),
            )
        {
            return cached;
        }

        let data = std::fs::read(&path).unwrap_or_default();
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
                    .get("type")
                    .or_else(|| v.get("role"))
                    .and_then(|r| r.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                *role_counts.entry(role.clone()).or_insert(0) += 1;

                let content = extract_content(&v);

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
        profile
    }

    fn extract_user_messages(&self, session_id: &str) -> Vec<String> {
        self.read_session(session_id)
            .into_iter()
            .filter(|m| m.role == "user" && !m.injected)
            .map(|m| m.content)
            .collect()
    }
}
