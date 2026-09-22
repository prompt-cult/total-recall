use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{
    EventType, InterestingEvent, RolloutAdapter, RolloutMessage, SessionProfile, SessionSummary,
    VIBE_ROOT_ENV_VAR, resolve_root, resolve_session_dir, slice_from_compaction,
    summarize_tool_call,
};

/// Vibe adapter. Reads sessions from ~/.vibe/logs/session/
///
/// Session directory structure:
///   session_YYYYMMDD_HHMMSS_<short-id>/
///   ├── messages.jsonl   (one JSON object per line)
///   └── meta.json        (session metadata)
pub struct VibeAdapter {
    root: PathBuf,
    from_env: bool,
}

impl VibeAdapter {
    pub fn new() -> Self {
        let (root, from_env) = resolve_root(VIBE_ROOT_ENV_VAR, &[".vibe", "logs", "session"]);
        Self { root, from_env }
    }

    pub fn with_root<P: Into<PathBuf>>(root: P) -> Self {
        Self {
            root: root.into(),
            from_env: true,
        }
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
        thinking: None,
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
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };

        let mut summaries = Vec::new();
        // content hash captured alongside each summary for dedupe.
        let mut identity: Vec<u64> = Vec::new();

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

            // Distinguish read damage (permissions, I/O error) from a payload
            // that does not exist yet: damage is surfaced on the entry.
            let (messages_data, read_error) = match std::fs::read(&messages_path) {
                Ok(data) => (data, None),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), None),
                Err(e) => (Vec::new(), Some(e.to_string())),
            };
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

            // Content hash over the rollout payload. 0 marks an empty store;
            // empty sessions are never deduped together.
            let content_hash = if file_size == 0 {
                0
            } else {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                messages_data.hash(&mut h);
                h.finish()
            };

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
                has_tantivy_index: false,
                aliases: Vec::new(),
                read_error,
            });
            identity.push(content_hash);
        }

        // Dedupe: one canonical entry per rollout payload. Two dirs describing
        // the same rollout share (file_size, line_count, content hash); the
        // canonical id is the dir-name whose date prefix agrees with
        // meta.start_time, and the rest become aliases.
        let summaries = dedupe_vibe_sessions(summaries, identity);
        // Sort by start_time descending (most recent first)
        let mut summaries = summaries;
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
                    has_tantivy_index: false,
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
            has_tantivy_index: false,
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

    /// Byte-faithful raw entries: re-parse the native `messages.jsonl` so each
    /// emitted record is the source line's JSON, not a normalized projection.
    fn read_session_entries(
        &self,
        session_id: &str,
        full: bool,
        include_injected: bool,
    ) -> Vec<super::RolloutEntry> {
        let path = match self.messages_path(session_id) {
            Some(p) => p,
            None => return Vec::new(),
        };
        let data = std::fs::read(&path).unwrap_or_default();
        let text = String::from_utf8_lossy(&data);

        // Collect raw (line_value) pairs, then honour the compaction window.
        let mut raw: Vec<serde_json::Value> = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                raw.push(v);
            }
        }
        let start = if full {
            0
        } else {
            raw.iter()
                .rposition(|v| {
                    v.get("content")
                        .and_then(|c| c.as_str())
                        .map(|c| c.contains("context compaction"))
                        .unwrap_or(false)
                })
                .unwrap_or(0)
        };

        let mut out = Vec::new();
        for v in &raw[start..] {
            let injected = v.get("injected").and_then(|i| i.as_bool()).unwrap_or(false);
            if injected && !include_injected {
                continue;
            }
            let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
            let entry_type = match role {
                "user" | "assistant" | "tool" => role.to_string(),
                _ => continue,
            };
            let timestamp = v.get("timestamp").and_then(|t| t.as_str()).map(String::from);
            out.push(super::RolloutEntry {
                index: out.len(),
                entry_type,
                timestamp,
                record: v.clone(),
            });
        }
        out
    }
}

/// Parse the `session_YYYYMMDD_HHMMSS_<id>` directory-name prefix into a
/// compact `YYYYMMDDHHMMSS` digit string for comparison against meta times.
/// Returns empty when the name does not match the expected shape.
fn dir_name_datetime(name: &str) -> String {
    let rest = match name.strip_prefix("session_") {
        Some(r) => r,
        None => return String::new(),
    };
    let mut parts = rest.splitn(3, '_');
    let date = parts.next().unwrap_or("");
    let time = parts.next().unwrap_or("");
    if date.len() == 8 && time.len() == 6 && date.bytes().all(|b| b.is_ascii_digit())
        && time.bytes().all(|b| b.is_ascii_digit())
    {
        format!("{}{}", date, time)
    } else {
        String::new()
    }
}

/// Compact an ISO8601 meta timestamp to `YYYYMMDDHHMMSS` digits (dropping
/// separators, fractional seconds, and the zone) for comparison with the
/// directory-name prefix. Empty when unparseable.
fn iso_datetime_digits(iso: &str) -> String {
    iso.chars().filter(|c| c.is_ascii_digit()).take(14).collect()
}

/// Group session summaries that describe the same rollout payload and keep a
/// single canonical entry per group, recording the others as aliases.
///
/// Identity = (file_size, line_count, content_hash). Sessions with an empty
/// store (hash 0) are never grouped. Canonical = the member whose dir-name
/// date prefix best agrees with meta.start_time (smallest absolute difference
/// in the digit-compacted datetimes); tie-break lexicographically smallest id.
/// Payloads that share (file_size, line_count, content_hash) are byte-identical,
/// so the canonical entry's own counters are already correct — only the
/// directory names and meta timestamps differ between members.
fn dedupe_vibe_sessions(
    summaries: Vec<SessionSummary>,
    identity: Vec<u64>,
) -> Vec<SessionSummary> {
    use std::collections::HashMap;

    // group key -> indices into summaries
    let mut groups: HashMap<(u64, u64, u64), Vec<usize>> = HashMap::new();
    for (i, s) in summaries.iter().enumerate() {
        let hash = identity[i];
        if hash == 0 {
            continue; // empty stores stay unique
        }
        groups
            .entry((s.file_size, s.line_count, hash))
            .or_default()
            .push(i);
    }

    // alias_of[i] = canonical id it merged into; canonical_aliases[i] = alias list
    let mut alias_of: Vec<Option<String>> = vec![None; summaries.len()];
    let mut canonical_aliases: HashMap<usize, Vec<String>> = HashMap::new();

    for idxs in groups.values() {
        if idxs.len() < 2 {
            continue;
        }
        // canonical: dir-name prefix closest to meta.start_time
        let canonical = *idxs
            .iter()
            .min_by(|&&a, &&b| {
                let sa = &summaries[a];
                let sb = &summaries[b];
                let da = dir_name_datetime(&sa.session_id);
                let db = dir_name_datetime(&sb.session_id);
                let ta = iso_datetime_digits(&sa.start_time);
                let tb = iso_datetime_digits(&sb.start_time);
                let diff_a = datetime_diff(&da, &ta);
                let diff_b = datetime_diff(&db, &tb);
                diff_a
                    .cmp(&diff_b)
                    .then_with(|| sa.session_id.cmp(&sb.session_id))
            })
            .unwrap();

        let mut aliases: Vec<String> = idxs
            .iter()
            .filter(|&&i| i != canonical)
            .map(|&i| summaries[i].session_id.clone())
            .collect();
        aliases.sort();
        for &i in idxs {
            if i != canonical {
                alias_of[i] = Some(summaries[canonical].session_id.clone());
            }
        }
        canonical_aliases.insert(canonical, aliases);
    }

    let mut out = Vec::new();
    for (i, mut s) in summaries.into_iter().enumerate() {
        if alias_of[i].is_some() {
            continue; // merged into the canonical entry
        }
        if let Some(aliases) = canonical_aliases.remove(&i) {
            s.aliases = aliases;
        }
        out.push(s);
    }
    out
}

/// Absolute difference between two `YYYYMMDDHHMMSS` digit strings, as an
/// integer ordering key. Missing/unparseable sides sort as "worst" (u64::MAX)
/// so entries whose dir-name agrees with meta start_time win.
fn datetime_diff(dir: &str, iso: &str) -> u64 {
    if dir.len() != 14 || iso.len() != 14 {
        return u64::MAX;
    }
    let (Ok(a), Ok(b)) = (dir.parse::<u64>(), iso.parse::<u64>()) else {
        return u64::MAX;
    };
    a.abs_diff(b)
}
