use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::{
    slice_from_compaction, summarize_tool_call, EventType, InterestingEvent, RolloutAdapter,
    RolloutMessage, SessionProfile, SessionSummary,
};

/// OpenCode adapter. Reads session history from the local SQLite database at
/// ~/.local/share/opencode/opencode.db (read-only), mirroring the schema used
/// by opencode-chat-history: session(id, parent_id, title, time_created,
/// time_updated), message(id, session_id, data JSON with role and time.created),
/// part(id, message_id, data JSON with type text|tool|compaction|...).
pub struct OpenCodeAdapter {
    db_path: PathBuf,
}

impl OpenCodeAdapter {
    pub fn new() -> Self {
        Self {
            db_path: opencode_db_path(),
        }
    }

    /// Path points at the opencode SQLite database file. If a directory is
    /// given, `opencode.db` inside it is used.
    pub fn with_root<P: Into<PathBuf>>(root: P) -> Self {
        let path = root.into();
        let db_path = if path.is_dir() {
            path.join("opencode.db")
        } else {
            path
        };
        Self { db_path }
    }

    fn connect(&self) -> Option<Connection> {
        Connection::open_with_flags(&self.db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
    }

    /// Resolve a (possibly partial) session id to the full id, most recent match wins.
    fn resolve_session_id(&self, conn: &Connection, session_id: &str) -> Option<String> {
        conn.query_row(
            "SELECT id FROM session WHERE id LIKE '%' || ?1 || '%'
             ORDER BY time_updated DESC LIMIT 1",
            [session_id],
            |row| row.get(0),
        )
        .ok()
    }

    fn load_messages(conn: &Connection, full_id: &str) -> Vec<RolloutMessage> {
        let mut stmt = match conn.prepare(
            "SELECT m.data, p.data
             FROM message m
             JOIN part p ON m.id = p.message_id
             WHERE m.session_id = ?1
             ORDER BY m.time_created ASC, p.time_created ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = stmt.query_map([&full_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
            ))
        });
        let rows = match rows {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        let mut messages = Vec::new();
        for row in rows.flatten() {
            let (msg_json, part_json) = row;
            let msg: Value = match serde_json::from_str(&msg_json) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let part: Value = match serde_json::from_str(&part_json) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let role = msg
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown")
                .to_string();
            let ts_ms = msg
                .get("time")
                .and_then(|t| t.get("created"))
                .and_then(|t| t.as_i64())
                .unwrap_or(0);
            let timestamp = ms_to_iso8601(ts_ms);
            append_part(&mut messages, &role, &timestamp, &part);
        }
        messages
    }
}

fn append_part(messages: &mut Vec<RolloutMessage>, role: &str, timestamp: &str, part: &Value) {
    let ptype = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ptype {
        "text" => {
            if part.get("synthetic").and_then(|s| s.as_bool()).unwrap_or(false) {
                return;
            }
            let content = part
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            messages.push(RolloutMessage {
                role: role.to_string(),
                content,
                tool_calls_summary: Vec::new(),
                timestamp: Some(timestamp.to_string()),
                injected: false,
            });
        }
        "tool" => {
            let tool = part
                .get("tool")
                .and_then(|t| t.as_str())
                .unwrap_or("?")
                .to_string();
            let state = part.get("state").cloned().unwrap_or(Value::Null);
            let input = state.get("input").cloned().unwrap_or(Value::Null);
            let args = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
            let summary = summarize_tool_call(&tool, &args);
            messages.push(RolloutMessage {
                role: "assistant".to_string(),
                content: String::new(),
                tool_calls_summary: vec![summary],
                timestamp: Some(timestamp.to_string()),
                injected: false,
            });
            let output = state.get("output").cloned().unwrap_or(Value::Null);
            let out = match output {
                Value::String(s) => s,
                Value::Null => String::new(),
                other => serde_json::to_string(&other).unwrap_or_default(),
            };
            messages.push(RolloutMessage {
                role: "tool".to_string(),
                content: out,
                tool_calls_summary: Vec::new(),
                timestamp: Some(timestamp.to_string()),
                injected: false,
            });
        }
        "compaction" => {
            messages.push(RolloutMessage {
                role: "user".to_string(),
                content: "context compaction".to_string(),
                tool_calls_summary: Vec::new(),
                timestamp: Some(timestamp.to_string()),
                injected: true,
            });
        }
        _ => {}
    }
}

fn ms_to_iso8601(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    let (year, month, day) = civil_from_days(days);
    if millis == 0 {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            year, month, day, hour, min, sec
        )
    } else {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            year, month, day, hour, min, sec, millis
        )
    }
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { y + 1 } else { y }, month, day)
}

fn opencode_db_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".local/share/opencode")
            .join("opencode.db")
    } else {
        PathBuf::from(".local/share/opencode").join("opencode.db")
    }
}

impl Default for OpenCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl RolloutAdapter for OpenCodeAdapter {
    fn name(&self) -> &'static str {
        "opencode"
    }

    fn list_sessions(&self) -> Vec<SessionSummary> {
        let conn = match self.connect() {
            Some(c) => c,
            None => return Vec::new(),
        };

        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        if let Ok(mut stmt) =
            conn.prepare("SELECT parent_id, id FROM session WHERE parent_id IS NOT NULL")
            && let Ok(rows) = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
        {
            for row in rows.flatten() {
                children.entry(row.0).or_default().push(row.1);
            }
        }

        let sql = "
            SELECT s.id, s.title, s.parent_id, s.time_created, s.time_updated,
                (SELECT COUNT(*) FROM message m WHERE m.session_id = s.id
                    AND json_extract(m.data, '$.role') = 'user'),
                (SELECT COUNT(*) FROM message m WHERE m.session_id = s.id
                    AND json_extract(m.data, '$.role') = 'assistant'),
                (SELECT COUNT(*) FROM part p WHERE p.session_id = s.id
                    AND json_extract(p.data, '$.type') = 'tool'),
                (SELECT COUNT(*) FROM part p WHERE p.session_id = s.id
                    AND json_extract(p.data, '$.type') = 'compaction'),
                (SELECT COUNT(*) FROM part p WHERE p.session_id = s.id),
                (SELECT COALESCE(SUM(length(data)), 0) FROM message WHERE session_id = s.id)
                    + (SELECT COALESCE(SUM(length(data)), 0) FROM part WHERE session_id = s.id)
            FROM session s
            ORDER BY s.time_updated DESC";

        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, i64>(10)?,
            ))
        });
        let rows = match rows {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        let mut summaries = Vec::new();
        for row in rows.flatten() {
            let (id, title, parent_id, created, updated, user_count, assistant_count, tool_count, compaction_count, part_count, total_bytes) =
                row;
            summaries.push(SessionSummary {
                session_id: id.clone(),
                title,
                start_time: ms_to_iso8601(created),
                end_time: ms_to_iso8601(updated),
                file_size: total_bytes.max(0) as u64,
                line_count: part_count.max(0) as u64,
                user_count: user_count.max(0) as u64,
                assistant_count: assistant_count.max(0) as u64,
                tool_count: tool_count.max(0) as u64,
                has_compaction: compaction_count > 0,
                parent_session_id: parent_id,
                child_sessions: children.remove(&id).unwrap_or_default(),
            });
        }
        summaries
    }

    fn read_session(&self, session_id: &str) -> Vec<RolloutMessage> {
        let conn = match self.connect() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let Some(full_id) = self.resolve_session_id(&conn, session_id) else {
            return Vec::new();
        };
        Self::load_messages(&conn, &full_id)
    }

    /// mmap applies to file-based rollout formats; SQLite is read through the
    /// message_session_time_created_id_idx index instead (no N+1, no full scan).
    fn read_session_mmap(&self, session_id: &str) -> Vec<RolloutMessage> {
        self.read_session(session_id)
    }

    fn read_session_from_compaction(&self, session_id: &str) -> Vec<RolloutMessage> {
        slice_from_compaction(self.read_session_mmap(session_id))
    }

    fn profile_session(&self, session_id: &str) -> SessionProfile {
        let empty = SessionProfile {
            session_id: session_id.to_string(),
            file_size: 0,
            line_count: 0,
            first_ts: None,
            last_ts: None,
            role_counts: HashMap::new(),
            interesting_events: Vec::new(),
        };
        let conn = match self.connect() {
            Some(c) => c,
            None => return empty,
        };
        let Some(full_id) = self.resolve_session_id(&conn, session_id) else {
            return empty;
        };

        let file_size: i64 = conn
            .query_row(
                "SELECT (SELECT COALESCE(SUM(length(data)), 0) FROM message WHERE session_id = ?1)
                    + (SELECT COALESCE(SUM(length(data)), 0) FROM part WHERE session_id = ?1)",
                [&full_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let line_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM part WHERE session_id = ?1",
                [&full_id],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let messages = Self::load_messages(&conn, &full_id);
        drop(conn);

        let mut role_counts: HashMap<String, u64> = HashMap::new();
        let mut interesting_events = Vec::new();
        let mut last_event_line = 0u64;
        let mut first_ts: Option<String> = None;
        let mut last_ts: Option<String> = None;

        for (i, msg) in messages.iter().enumerate() {
            let line_num = (i + 1) as u64;
            *role_counts.entry(msg.role.clone()).or_insert(0) += 1;

            if msg.content.contains("context compaction") && msg.injected {
                interesting_events.push(InterestingEvent {
                    line_number: line_num,
                    event_type: EventType::Compaction,
                    summary: "Compaction marker".to_string(),
                    gap_lines: line_num - last_event_line,
                });
                last_event_line = line_num;
            }

            if msg.role == "user" && !msg.injected && !msg.content.is_empty() {
                interesting_events.push(InterestingEvent {
                    line_number: line_num,
                    event_type: EventType::UserMessage,
                    summary: msg.content.chars().take(80).collect(),
                    gap_lines: line_num - last_event_line,
                });
                last_event_line = line_num;
            }

            if let Some(ts) = &msg.timestamp {
                if first_ts.is_none() {
                    first_ts = Some(ts.clone());
                }
                last_ts = Some(ts.clone());
            }
        }

        SessionProfile {
            session_id: full_id,
            file_size: file_size.max(0) as u64,
            line_count: line_count.max(0) as u64,
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
