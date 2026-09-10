use std::collections::HashMap;
use std::path::PathBuf;

use super::{
    resolve_session_dir, slice_from_compaction, summarize_tool_call, EventType, InterestingEvent,
    RolloutAdapter, RolloutMessage, SessionProfile, SessionSummary,
};

/// Codex adapter. Reads flat JSONL files from ~/.codex/sessions/
///
/// Format: one JSON object per line with `role` and `content` fields.
/// Simpler than vibe: no `injected`, `tool_calls`, or `message_id` fields.
pub struct CodexAdapter {
    root: PathBuf,
}

impl CodexAdapter {
    pub fn new() -> Self {
        Self {
            root: codex_sessions_root(),
        }
    }

    pub fn with_root<P: Into<PathBuf>>(root: P) -> Self {
        Self {
            root: root.into(),
        }
    }

    fn session_path(&self, session_id: &str) -> Option<PathBuf> {
        if session_id.is_empty() {
            resolve_session_dir(&self.root, "")
        } else {
            resolve_session_dir(&self.root, session_id)
        }
    }

    fn parse_jsonl(data: &[u8]) -> Vec<RolloutMessage> {
        let text = String::from_utf8_lossy(data);
        let mut messages = Vec::new();

        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                messages.push(json_to_rollout_message(&v));
            }
        }
        messages
    }
}

fn codex_sessions_root() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".codex").join("sessions")
    } else {
        PathBuf::from(".codex").join("sessions")
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

    let injected = v
        .get("injected")
        .and_then(|i| i.as_bool())
        .unwrap_or(false);

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

    fn list_sessions(&self) -> Vec<SessionSummary> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };

        let mut summaries = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().is_none() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy().to_string();

            let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let data = std::fs::read(&path).unwrap_or_default();
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

            summaries.push(SessionSummary {
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
                parent_session_id: None,
                child_sessions: Vec::new(),
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

    fn profile_session(&self, session_id: &str) -> SessionProfile {
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
                    interesting_events: Vec::new(),
                }
            }
        };

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

        SessionProfile {
            session_id: session_id.to_string(),
            file_size,
            line_count,
            first_ts,
            last_ts,
            role_counts,
            interesting_events,
        }
    }

    fn extract_user_messages(&self, session_id: &str) -> Vec<String> {
        self.read_session(session_id)
            .into_iter()
            .filter(|m| m.role == "user" && !m.injected)
            .map(|m| m.content)
            .collect()
    }
}
