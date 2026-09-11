use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::{
    slice_from_compaction, summarize_tool_call, EventType, InterestingEvent, RolloutAdapter,
    RolloutMessage, SessionProfile, SessionSummary,
};

/// Cursor adapter. Reads the agent chat history from Cursor's global SQLite
/// state database at
/// `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb`
/// (read-only). Schema (verified 2026-09-12 against a real install): a single
/// JSON key-value table `cursorDiskKV(key TEXT UNIQUE, value BLOB)` where
/// - `composerData:<composerId>` rows hold session metadata (composerId,
///   createdAt ms epoch, name, subComposerIds, fullConversationHeadersOnly —
///   the ordered [{bubbleId, type}] conversation index, type 1 = user,
///   type 2 = assistant), and
/// - `bubbleId:<composerId>:<bubbleId>` rows hold individual messages (type,
///   text, createdAt ISO-8601, toolFormerData {name, rawArgs, result},
///   summarizedComposers for compaction summaries).
pub struct CursorAdapter {
    db_path: PathBuf,
}

const COMPOSER_PREFIX: &str = "composerData:";
const BUBBLE_PREFIX: &str = "bubbleId:";
const COMPACTION_TEXT: &str = "context compaction";

impl CursorAdapter {
    pub fn new() -> Self {
        Self {
            db_path: cursor_db_path(),
        }
    }

    /// Path points at Cursor's `state.vscdb` SQLite file. If a directory is
    /// given, `state.vscdb` inside it is used.
    pub fn with_root<P: Into<PathBuf>>(root: P) -> Self {
        let path = root.into();
        let db_path = if path.is_dir() {
            path.join("state.vscdb")
        } else {
            path
        };
        Self { db_path }
    }

    fn connect(&self) -> Option<Connection> {
        Connection::open_with_flags(&self.db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
    }

    /// Resolve a (possibly partial) session id to the full composerId,
    /// most recent composerData row wins.
    fn resolve_session_id(&self, conn: &Connection, session_id: &str) -> Option<String> {
        conn.query_row(
            &format!(
                "SELECT substr(key, {}) FROM cursorDiskKV
                 WHERE key GLOB ?1 || '*' AND instr(key, ?2) > 0
                 ORDER BY key DESC LIMIT 1",
                COMPOSER_PREFIX.len() + 1
            ),
            rusqlite::params![COMPOSER_PREFIX, session_id],
            |row| row.get(0),
        )
        .ok()
    }

    fn load_composer(conn: &Connection, full_id: &str) -> Option<Value> {
        let value: String = conn
            .query_row(
                "SELECT value FROM cursorDiskKV WHERE key = ?1",
                [format!("{}{}", COMPOSER_PREFIX, full_id)],
                |row| row.get(0),
            )
            .ok()?;
        serde_json::from_str(&value).ok()
    }

    /// Load messages in conversation order via the composer's
    /// fullConversationHeadersOnly index, joining each bubble by primary key
    /// (single query, no N+1 scans).
    fn load_messages(conn: &Connection, full_id: &str, headers_json: &str) -> Vec<RolloutMessage> {
        let sql = "SELECT json_extract(je.value, '$.type'), b.value
             FROM json_each(?1) je
             LEFT JOIN cursorDiskKV b
                ON b.key = ?2 || json_extract(je.value, '$.bubbleId')
             ORDER BY je.key";
        let key_prefix = format!("{}{}:", BUBBLE_PREFIX, full_id);
        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map(
            rusqlite::params![headers_json, key_prefix],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        );
        let rows = match rows {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        let mut messages = Vec::new();
        for row in rows.flatten() {
            let (header_type, bubble_json) = row;
            let Some(bubble_json) = bubble_json else {
                continue;
            };
            let bubble: Value = match serde_json::from_str(&bubble_json) {
                Ok(v) => v,
                Err(_) => continue,
            };
            append_bubble(&mut messages, header_type, &bubble);
        }
        messages
    }
}

fn role_from_type(t: i64) -> Option<&'static str> {
    match t {
        1 => Some("user"),
        2 => Some("assistant"),
        _ => None,
    }
}

fn append_bubble(messages: &mut Vec<RolloutMessage>, header_type: i64, bubble: &Value) {
    let btype = bubble.get("type").and_then(|t| t.as_i64()).unwrap_or(-1);
    let Some(role) = role_from_type(btype).or_else(|| role_from_type(header_type)) else {
        return;
    };
    let timestamp = bubble
        .get("createdAt")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    let summarized = bubble
        .get("summarizedComposers")
        .and_then(|s| s.as_array())
        .is_some_and(|a| !a.is_empty());
    if summarized {
        messages.push(RolloutMessage {
            role: "user".to_string(),
            content: COMPACTION_TEXT.to_string(),
            tool_calls_summary: Vec::new(),
            timestamp: Some(timestamp.clone()),
            injected: true,
        });
    }

    if let Some(tool) = bubble.get("toolFormerData").filter(|t| !t.is_null()) {
        let name = tool
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("?")
            .to_string();
        let raw_args = tool
            .get("rawArgs")
            .and_then(|a| a.as_str())
            .unwrap_or("{}")
            .to_string();
        let summary = summarize_tool_call(&name, &raw_args);
        messages.push(RolloutMessage {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls_summary: vec![summary],
            timestamp: Some(timestamp.clone()),
            injected: false,
        });
        let out = tool
            .get("result")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();
        if !out.is_empty() {
            messages.push(RolloutMessage {
                role: "tool".to_string(),
                content: out,
                tool_calls_summary: Vec::new(),
                timestamp: Some(timestamp.clone()),
                injected: false,
            });
        }
    }

    let text = bubble
        .get("text")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    if text.is_empty() {
        return;
    }
    messages.push(RolloutMessage {
        role: role.to_string(),
        content: text,
        tool_calls_summary: Vec::new(),
        timestamp: Some(timestamp),
        injected: false,
    });
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

fn cursor_db_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join("Library/Application Support/Cursor/User/globalStorage")
            .join("state.vscdb")
    } else {
        PathBuf::from("Library/Application Support/Cursor/User/globalStorage").join("state.vscdb")
    }
}

struct BubbleStats {
    total: i64,
    user: i64,
    assistant: i64,
    tool: i64,
    summarized: i64,
    last_ts: Option<String>,
    bytes: i64,
}

fn bubble_stats(conn: &Connection) -> HashMap<String, BubbleStats> {
    let sql = format!(
        "SELECT substr(key, {}, 36),
            COUNT(*),
            SUM(CASE WHEN json_extract(value, '$.type') = 1 THEN 1 ELSE 0 END),
            SUM(CASE WHEN json_extract(value, '$.type') = 2 THEN 1 ELSE 0 END),
            SUM(CASE WHEN json_extract(value, '$.toolFormerData') IS NOT NULL THEN 1 ELSE 0 END),
            SUM(CASE WHEN COALESCE(json_extract(value, '$.summarizedComposers'), '[]')
                     NOT IN ('', '[]') THEN 1 ELSE 0 END),
            MAX(json_extract(value, '$.createdAt')),
            SUM(length(value))
         FROM cursorDiskKV WHERE key GLOB ?1 || '*'
         GROUP BY 1",
        BUBBLE_PREFIX.len() + 1
    );
    let mut stats = HashMap::new();
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return stats;
    };
    let Ok(rows) = stmt.query_map([BUBBLE_PREFIX], |row| {
        let cid: String = row.get(0)?;
        let last: Option<String> = row.get(6)?;
        Ok((cid, BubbleStats {
            total: row.get(1)?,
            user: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            assistant: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
            tool: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
            summarized: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
            last_ts: last.filter(|s| !s.is_empty()),
            bytes: row.get(7)?,
        }))
    }) else {
        return stats;
    };
    for row in rows.flatten() {
        let (cid, s) = row;
        stats.insert(cid, s);
    }
    stats
}

impl Default for CursorAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl RolloutAdapter for CursorAdapter {
    fn name(&self) -> &'static str {
        "cursor"
    }

    fn list_sessions(&self) -> Vec<SessionSummary> {
        let conn = match self.connect() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let stats = bubble_stats(&conn);

        let mut stmt = match conn.prepare(&format!(
            "SELECT substr(key, {}), value FROM cursorDiskKV WHERE key GLOB ?1 || '*'
             ORDER BY key DESC",
            COMPOSER_PREFIX.len() + 1
        )) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([COMPOSER_PREFIX], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        });
        let rows = match rows {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        let mut summaries = Vec::new();
        for row in rows.flatten() {
            let (composer_id, value) = row;
            let Ok(composer) = serde_json::from_str::<Value>(&value) else {
                continue;
            };
            let created = composer.get("createdAt").and_then(|c| c.as_i64()).unwrap_or(0);
            let title = composer
                .get("name")
                .and_then(|n| n.as_str())
                .filter(|n| !n.is_empty())
                .or_else(|| composer.get("text").and_then(|t| t.as_str()))
                .unwrap_or("untitled")
                .to_string();
            let children = composer
                .get("subComposerIds")
                .and_then(|s| s.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let default = BubbleStats {
                total: 0,
                user: 0,
                assistant: 0,
                tool: 0,
                summarized: 0,
                last_ts: None,
                bytes: 0,
            };
            let s = stats.get(&composer_id).unwrap_or(&default);
            let end_time = match &s.last_ts {
                Some(ts) => ts.clone(),
                None => ms_to_iso8601(created),
            };
            summaries.push(SessionSummary {
                file_size: (value.len() as i64 + s.bytes).max(0) as u64,
                session_id: composer_id,
                title,
                start_time: ms_to_iso8601(created),
                end_time,
                line_count: s.total.max(0) as u64,
                user_count: s.user.max(0) as u64,
                assistant_count: s.assistant.max(0) as u64,
                tool_count: s.tool.max(0) as u64,
                has_compaction: s.summarized > 0,
                parent_session_id: None,
                child_sessions: children,
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
        let Some(composer) = Self::load_composer(&conn, &full_id) else {
            return Vec::new();
        };
        let headers_json = composer
            .get("fullConversationHeadersOnly")
            .map(|h| h.to_string())
            .unwrap_or_else(|| "[]".to_string());
        Self::load_messages(&conn, &full_id, &headers_json)
    }

    /// mmap applies to file-based rollout formats; SQLite is read through the
    /// primary key index on cursorDiskKV.key instead (no N+1, no full scan).
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
        let composer_bytes: i64 = conn
            .query_row(
                "SELECT COALESCE(length(value), 0) FROM cursorDiskKV WHERE key = ?1",
                [format!("{}{}", COMPOSER_PREFIX, full_id)],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let line_count: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM cursorDiskKV WHERE key GLOB ?1 || ':*'",
                ),
                rusqlite::params![format!("{}{}", BUBBLE_PREFIX, full_id)],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let Some(composer) = Self::load_composer(&conn, &full_id) else {
            return empty;
        };
        let headers_json = composer
            .get("fullConversationHeadersOnly")
            .map(|h| h.to_string())
            .unwrap_or_else(|| "[]".to_string());
        let messages = Self::load_messages(&conn, &full_id, &headers_json);
        drop(conn);

        let mut role_counts: HashMap<String, u64> = HashMap::new();
        let mut interesting_events = Vec::new();
        let mut last_event_line = 0u64;
        let mut first_ts: Option<String> = None;
        let mut last_ts: Option<String> = None;

        for (i, msg) in messages.iter().enumerate() {
            let line_num = (i + 1) as u64;
            *role_counts.entry(msg.role.clone()).or_insert(0) += 1;

            if msg.content.contains(COMPACTION_TEXT) && msg.injected {
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
            file_size: (composer_bytes.max(0) as u64),
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
