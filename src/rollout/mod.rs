pub mod claude;
pub mod codex;
pub mod cursor;
pub mod mock;
pub mod opencode;
pub mod vibe;

use std::collections::HashMap;
use std::path::PathBuf;

/// A single message from a rollout, normalized across harness formats.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RolloutMessage {
    pub role: String,
    pub content: String,
    pub tool_calls_summary: Vec<String>,
    pub timestamp: Option<String>,
    pub injected: bool,
}

/// Trait abstracting over different CLI tool session formats.
pub trait RolloutAdapter: Send + Sync {
    /// Name of the harness ("vibe", "codex", "claude", "opencode")
    fn name(&self) -> &'static str;

    /// Find all rollout sessions, return summary info
    fn list_sessions(&self) -> Vec<SessionSummary>;

    /// Stream messages from a specific session
    fn read_session(&self, session_id: &str) -> Vec<RolloutMessage>;

    /// Read session using mmap for maximum throughput
    fn read_session_mmap(&self, session_id: &str) -> Vec<RolloutMessage>;

    /// Read messages from the last compaction point onward.
    /// If no compaction marker exists, returns all messages.
    fn read_session_from_compaction(&self, session_id: &str) -> Vec<RolloutMessage>;

    /// Profile a session: file size, line count, role counts, interesting events
    fn profile_session(&self, session_id: &str) -> SessionProfile;

    /// Extract only user messages (verbatim)
    fn extract_user_messages(&self, session_id: &str) -> Vec<String>;
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub title: String,
    pub start_time: String,
    pub end_time: String,
    pub file_size: u64,
    pub line_count: u64,
    pub user_count: u64,
    pub assistant_count: u64,
    pub tool_count: u64,
    pub has_compaction: bool,
    pub parent_session_id: Option<String>,
    pub child_sessions: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionProfile {
    pub session_id: String,
    pub file_size: u64,
    pub line_count: u64,
    pub first_ts: Option<String>,
    pub last_ts: Option<String>,
    pub role_counts: HashMap<String, u64>,
    pub interesting_events: Vec<InterestingEvent>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InterestingEvent {
    pub line_number: u64,
    pub event_type: EventType,
    pub summary: String,
    pub gap_lines: u64,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub enum EventType {
    Compaction,
    TodoCreate,
    TodoUpdate,
    TodoDelete,
    GitCommit,
    GitPush,
    GitTag,
    SessionFork,
    SessionRename,
    UserMessage,
}

/// Summarize a tool call into a compact one-line description.
/// Mirrors the logic from compact.py `_summarize_tool_call`.
pub fn summarize_tool_call(name: &str, args_str: &str) -> String {
    let args: serde_json::Value = match serde_json::from_str(args_str) {
        Ok(v) => v,
        Err(_) => return format!("{}({})", name, truncate_chars(args_str, 200)),
    };

    match name {
        "bash" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            format!("bash: {}", truncate_chars(cmd, 300))
        }
        "write_file" => {
            let fp = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let content_len = args
                .get("content")
                .and_then(|v| v.as_str())
                .map(|s| s.len())
                .unwrap_or(0);
            format!("write_file({}, {} chars)", fp, content_len)
        }
        "edit" => {
            let fp = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let old_len = args
                .get("old_string")
                .and_then(|v| v.as_str())
                .map(|s| s.len())
                .unwrap_or(0);
            format!("edit({}, {} chars replaced)", fp, old_len)
        }
        "read_file" => {
            let fp = args
                .get("file_path")
                .or_else(|| args.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("read_file({})", fp)
        }
        "grep" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("?");
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            format!("grep({}, {})", pattern, path)
        }
        "task" => {
            let agent = args.get("agent").and_then(|v| v.as_str()).unwrap_or("?");
            format!("task({})", agent)
        }
        "todo" => {
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("?");
            format!("todo({})", action)
        }
        "skill" => {
            let skill_name = args.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            format!("skill({})", skill_name)
        }
        _ => {
            let s = serde_json::to_string(&args).unwrap_or_default();
            format!("{}({})", name, truncate_chars(&s, 300))
        }
    }
}

/// Convert messages to a compact text representation for the LLM.
/// Tool calls and results are summarized, not dumped verbatim.
pub fn messages_to_text(messages: &[RolloutMessage]) -> String {
    let mut lines = Vec::new();
    for msg in messages {
        let role = msg.role.to_uppercase();

        for tc_summary in &msg.tool_calls_summary {
            lines.push(format!("  {} -> {}", role, tc_summary));
        }

        if !msg.content.is_empty() {
            if role == "TOOL" {
                let truncated = truncate_chars(&msg.content, 500);
                lines.push(format!("  TOOL RESULT: {}", truncated));
                if msg.content.len() > 500 {
                    lines.push("  ... (truncated)".to_string());
                }
            } else {
                let truncated = truncate_chars(&msg.content, 1500);
                lines.push(format!("[{}]", role));
                lines.push(truncated.to_string());
                if msg.content.len() > 1500 {
                    lines.push("... (truncated)".to_string());
                }
            }
            lines.push(String::new());
        }
    }
    lines.join("\n")
}

/// Truncate a string to at most `max_bytes` without splitting a UTF-8 character.
fn truncate_chars(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Resolve a partial session ID to a full session directory path.
/// Returns the most recent matching session if multiple match.
pub fn resolve_session_dir(root: &PathBuf, partial_id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut matches: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.contains(partial_id)
            && let Ok(meta) = entry.metadata()
        {
            let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            matches.push((entry.path(), mtime));
        }
    }

    matches.sort_by_key(|b| std::cmp::Reverse(b.1));
    matches.into_iter().next().map(|(p, _)| p)
}

/// Slice messages from the last compaction marker onward.
/// If no compaction marker is found, returns all messages.
pub fn slice_from_compaction(messages: Vec<RolloutMessage>) -> Vec<RolloutMessage> {
    let last_compaction = messages
        .iter()
        .rposition(|m| m.content.contains("context compaction"));

    match last_compaction {
        Some(idx) => messages[idx..].to_vec(),
        None => messages,
    }
}
