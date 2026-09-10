use std::collections::HashMap;
use std::path::PathBuf;

use super::{
    EventType, InterestingEvent, RolloutAdapter, RolloutMessage, SessionProfile, SessionSummary,
    slice_from_compaction,
};

/// Mocked flat-file adapter for tests. Reads a pre-extracted JSONL of RolloutMessage objects.
pub struct MockAdapter {
    data_path: PathBuf,
}

impl MockAdapter {
    pub fn new<P: Into<PathBuf>>(data_path: P) -> Self {
        Self {
            data_path: data_path.into(),
        }
    }

    fn read_jsonl(&self) -> Vec<RolloutMessage> {
        let data = std::fs::read(&self.data_path).unwrap_or_default();
        let text = String::from_utf8_lossy(&data);
        let mut messages = Vec::new();

        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(msg) = serde_json::from_str::<RolloutMessage>(line) {
                messages.push(msg);
            }
        }
        messages
    }

    fn read_jsonl_mmap(&self) -> Vec<RolloutMessage> {
        let file = std::fs::File::open(&self.data_path).unwrap_or_else(|_| {
            return std::fs::File::create("/dev/null").unwrap();
        });
        let mmap = unsafe { memmap2::Mmap::map(&file).ok() };
        let data = match &mmap {
            Some(m) => &m[..],
            None => &[],
        };
        let text = String::from_utf8_lossy(data);
        let mut messages = Vec::new();

        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(msg) = serde_json::from_str::<RolloutMessage>(line) {
                messages.push(msg);
            }
        }
        messages
    }
}

impl RolloutAdapter for MockAdapter {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn list_sessions(&self) -> Vec<SessionSummary> {
        let messages = self.read_jsonl();
        let file_size = std::fs::metadata(&self.data_path)
            .map(|m| m.len())
            .unwrap_or(0);
        let line_count = messages.len() as u64;

        let mut user_count = 0u64;
        let mut assistant_count = 0u64;
        let mut tool_count = 0u64;
        let mut has_compaction = false;

        for msg in &messages {
            match msg.role.as_str() {
                "user" => {
                    user_count += 1;
                    if msg.content.contains("context compaction") {
                        has_compaction = true;
                    }
                }
                "assistant" => assistant_count += 1,
                "tool" => tool_count += 1,
                _ => {}
            }
        }

        vec![SessionSummary {
            session_id: "mock".to_string(),
            title: "Mock session".to_string(),
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
        }]
    }

    fn read_session(&self, _session_id: &str) -> Vec<RolloutMessage> {
        self.read_jsonl()
    }

    fn read_session_mmap(&self, _session_id: &str) -> Vec<RolloutMessage> {
        self.read_jsonl_mmap()
    }

    fn read_session_from_compaction(&self, _session_id: &str) -> Vec<RolloutMessage> {
        slice_from_compaction(self.read_jsonl_mmap())
    }

    fn profile_session(&self, _session_id: &str) -> SessionProfile {
        let messages = self.read_jsonl();
        let file_size = std::fs::metadata(&self.data_path)
            .map(|m| m.len())
            .unwrap_or(0);
        let line_count = messages.len() as u64;

        let mut role_counts: HashMap<String, u64> = HashMap::new();
        let mut interesting_events = Vec::new();
        let mut last_event_line = 0u64;

        for (i, msg) in messages.iter().enumerate() {
            let line = i as u64 + 1;
            *role_counts.entry(msg.role.clone()).or_insert(0) += 1;

            if msg.content.contains("context compaction") {
                interesting_events.push(InterestingEvent {
                    line_number: line,
                    event_type: EventType::Compaction,
                    summary: "Compaction marker found".to_string(),
                    gap_lines: line - last_event_line,
                });
                last_event_line = line;
            }

            for tc in &msg.tool_calls_summary {
                if tc.starts_with("todo(") {
                    let event_type = if tc.contains("write") {
                        EventType::TodoCreate
                    } else if tc.contains("delete") {
                        EventType::TodoDelete
                    } else {
                        EventType::TodoUpdate
                    };
                    interesting_events.push(InterestingEvent {
                        line_number: line,
                        event_type,
                        summary: tc.clone(),
                        gap_lines: line - last_event_line,
                    });
                    last_event_line = line;
                }
                if tc.contains("git commit") {
                    interesting_events.push(InterestingEvent {
                        line_number: line,
                        event_type: EventType::GitCommit,
                        summary: tc.clone(),
                        gap_lines: line - last_event_line,
                    });
                    last_event_line = line;
                }
                if tc.contains("git push") {
                    interesting_events.push(InterestingEvent {
                        line_number: line,
                        event_type: EventType::GitPush,
                        summary: tc.clone(),
                        gap_lines: line - last_event_line,
                    });
                    last_event_line = line;
                }
                if tc.contains("git tag") {
                    interesting_events.push(InterestingEvent {
                        line_number: line,
                        event_type: EventType::GitTag,
                        summary: tc.clone(),
                        gap_lines: line - last_event_line,
                    });
                    last_event_line = line;
                }
            }
        }

        let first_ts = messages.first().and_then(|m| m.timestamp.clone());
        let last_ts = messages.last().and_then(|m| m.timestamp.clone());

        SessionProfile {
            session_id: "mock".to_string(),
            file_size,
            line_count,
            first_ts,
            last_ts,
            role_counts,
            interesting_events,
        }
    }

    fn extract_user_messages(&self, _session_id: &str) -> Vec<String> {
        self.read_jsonl()
            .into_iter()
            .filter(|m| m.role == "user" && !m.injected)
            .map(|m| m.content)
            .collect()
    }
}
