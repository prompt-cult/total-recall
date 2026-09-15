use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{
    EventType, InterestingEvent, RolloutAdapter, RolloutMessage, SessionProfile, SessionSummary,
    resolve_session_dir, slice_from_compaction, summarize_tool_call,
};

/// Vibe adapter. Reads sessions from ~/.vibe/logs/session/
///
/// Session directory structure:
///   session_YYYYMMDD_HHMMSS_<short-id>/
///   ├── messages.jsonl   (one JSON object per line)
///   └── meta.json        (session metadata)
pub struct VibeAdapter {
    root: PathBuf,
}

impl VibeAdapter {
    pub fn new() -> Self {
        Self {
            root: dirs_home_vibe_session(),
        }
    }

    pub fn with_root<P: Into<PathBuf>>(root: P) -> Self {
        Self { root: root.into() }
    }

    fn session_dir(&self, session_id: &str) -> Option<PathBuf> {
        if session_id.is_empty() {
            // Most recent session
            resolve_session_dir(&self.root, "")
        } else {
            resolve_session_dir(&self.root, session_id)
        }
    }

    fn messages_path(&self, session_id: &str) -> Option<PathBuf> {
        self.session_dir(session_id)
            .map(|d| d.join("messages.jsonl"))
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

    fn parse_jsonl_mmap(path: &Path) -> Vec<RolloutMessage> {
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };
        let mmap = match unsafe { memmap2::Mmap::map(&file) } {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };
        Self::parse_jsonl(&mmap[..])
    }

    fn read_meta(&self, dir: &Path) -> Option<serde_json::Value> {
        let meta_path = dir.join("meta.json");
        let data = std::fs::read(&meta_path).ok()?;
        serde_json::from_slice(&data).ok()
    }
}

fn dirs_home_vibe_session() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".vibe")
            .join("logs")
            .join("session")
    } else {
        PathBuf::from(".vibe").join("logs").join("session")
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

    // Summarize tool calls
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

impl Default for VibeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl RolloutAdapter for VibeAdapter {
    fn name(&self) -> &'static str {
        "vibe"
    }

    fn list_sessions(&self) -> Vec<SessionSummary> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };

        let mut summaries = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with("session_") {
                continue;
            }

            let messages_path = path.join("messages.jsonl");
            let file_size = std::fs::metadata(&messages_path)
                .map(|m| m.len())
                .unwrap_or(0);

            let messages_data = std::fs::read(&messages_path).unwrap_or_default();
            let line_count = String::from_utf8_lossy(&messages_data)
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count() as u64;

            let meta = self.read_meta(&path);

            let title = meta
                .as_ref()
                .and_then(|m| m.get("title"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();

            let start_time = meta
                .as_ref()
                .and_then(|m| m.get("start_time"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();

            let end_time = meta
                .as_ref()
                .and_then(|m| m.get("end_time"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();

            let parent_session_id = meta
                .as_ref()
                .and_then(|m| m.get("parent_session_id"))
                .and_then(|p| p.as_str())
                .map(|s| s.to_string());

            let session_id = name_str.to_string();

            // Count roles and check for compaction
            let mut user_count = 0u64;
            let mut assistant_count = 0u64;
            let mut tool_count = 0u64;
            let mut has_compaction = false;

            for line in String::from_utf8_lossy(&messages_data).lines() {
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
                directory: None,
                parent_session_id,
                child_sessions: Vec::new(),
            });
        }

        // Sort by start_time descending (most recent first)
        summaries.sort_by(|a, b| b.start_time.cmp(&a.start_time));
        summaries
    }

    fn read_session(&self, session_id: &str) -> Vec<RolloutMessage> {
        let path = match self.messages_path(session_id) {
            Some(p) => p,
            None => return Vec::new(),
        };
        let data = std::fs::read(&path).unwrap_or_default();
        Self::parse_jsonl(&data)
    }

    fn read_session_mmap(&self, session_id: &str) -> Vec<RolloutMessage> {
        let path = match self.messages_path(session_id) {
            Some(p) => p,
            None => return Vec::new(),
        };
        Self::parse_jsonl_mmap(&path)
    }

    fn read_session_from_compaction(&self, session_id: &str) -> Vec<RolloutMessage> {
        slice_from_compaction(self.read_session_mmap(session_id))
    }

    fn profile_session(&self, session_id: &str) -> SessionProfile {
        let path = match self.messages_path(session_id) {
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
                };
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

                // Check for compaction markers
                if content.contains("context compaction") {
                    interesting_events.push(InterestingEvent {
                        line_number: line_num,
                        event_type: EventType::Compaction,
                        summary: "Compaction marker".to_string(),
                        gap_lines: line_num - last_event_line,
                    });
                    last_event_line = line_num;
                }

                // Check for user messages (non-injected)
                if role == "user" {
                    let injected = v.get("injected").and_then(|i| i.as_bool()).unwrap_or(false);
                    if !injected {
                        interesting_events.push(InterestingEvent {
                            line_number: line_num,
                            event_type: EventType::UserMessage,
                            summary: content.chars().take(80).collect(),
                            gap_lines: line_num - last_event_line,
                        });
                        last_event_line = line_num;
                    }
                }

                // Check tool calls for todo, git operations
                if let Some(tool_calls) = v.get("tool_calls").and_then(|tc| tc.as_array()) {
                    for tc in tool_calls {
                        let name = tc
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .or_else(|| tc.get("name"))
                            .and_then(|n| n.as_str())
                            .unwrap_or("");
                        let args = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(|a| a.as_str())
                            .unwrap_or("");

                        let summary = summarize_tool_call(name, args);

                        match name {
                            "todo" => {
                                let event_type = if summary.contains("write") {
                                    EventType::TodoCreate
                                } else if summary.contains("delete") {
                                    EventType::TodoDelete
                                } else {
                                    EventType::TodoUpdate
                                };
                                interesting_events.push(InterestingEvent {
                                    line_number: line_num,
                                    event_type,
                                    summary: summary.clone(),
                                    gap_lines: line_num - last_event_line,
                                });
                                last_event_line = line_num;
                            }
                            _ => {
                                // Check for git commands in bash tool calls
                                if summary.contains("git commit") {
                                    interesting_events.push(InterestingEvent {
                                        line_number: line_num,
                                        event_type: EventType::GitCommit,
                                        summary: summary.clone(),
                                        gap_lines: line_num - last_event_line,
                                    });
                                    last_event_line = line_num;
                                }
                                if summary.contains("git push") {
                                    interesting_events.push(InterestingEvent {
                                        line_number: line_num,
                                        event_type: EventType::GitPush,
                                        summary: summary.clone(),
                                        gap_lines: line_num - last_event_line,
                                    });
                                    last_event_line = line_num;
                                }
                                if summary.contains("git tag") {
                                    interesting_events.push(InterestingEvent {
                                        line_number: line_num,
                                        event_type: EventType::GitTag,
                                        summary: summary.clone(),
                                        gap_lines: line_num - last_event_line,
                                    });
                                    last_event_line = line_num;
                                }
                            }
                        }
                    }
                }

                // Track timestamps
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
