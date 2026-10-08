use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::functions::FunctionFlags;
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::{
    EventType, InterestingEvent, LISTING_ROW_CAP, OPENCODE_ROOT_ENV_VAR, ReadResult,
    RolloutAdapter, RolloutMessage, SessionListing, SessionProfile, SessionSummary,
    no_session_error, resolve_root, slice_from_compaction, summarize_tool_call, truncate_chars,
};
use crate::todo_history::{TodoItemState, TodoWrite};

/// OpenCode adapter. Reads session history from the local SQLite database at
/// ~/.local/share/opencode/opencode.db (read-only), mirroring the schema used
/// by opencode-chat-history: session(id, parent_id, title, time_created,
/// time_updated), message(id, session_id, data JSON with role and time.created),
/// part(id, message_id, data JSON with type text|tool|compaction|...).
pub struct OpenCodeAdapter {
    db_path: PathBuf,
    from_env: bool,
}

impl OpenCodeAdapter {
    pub fn new() -> Self {
        let (db_path, from_env) = resolve_root(
            OPENCODE_ROOT_ENV_VAR,
            &[".local/share/opencode", "opencode.db"],
        );
        let db_path = normalize_db_path(db_path);
        Self { db_path, from_env }
    }

    /// Path points at the opencode SQLite database file. If a directory is
    /// given, `opencode.db` inside it is used.
    pub fn with_root<P: Into<PathBuf>>(root: P) -> Self {
        let db_path = normalize_db_path(root.into());
        Self {
            db_path,
            from_env: true,
        }
    }

    fn connect(&self) -> ReadResult<Connection> {
        Connection::open_with_flags(&self.db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| {
            format!(
                "cannot open opencode database at {}: {}",
                self.db_path.display(),
                e
            )
        })
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

    fn load_messages(conn: &Connection, full_id: &str) -> ReadResult<Vec<RolloutMessage>> {
        let mut stmt = conn
            .prepare(
                "SELECT m.data, p.data
             FROM message m
             JOIN part p ON m.id = p.message_id
             WHERE m.session_id = ?1
             ORDER BY m.time_created ASC, p.time_created ASC",
            )
            .map_err(|e| format!("cannot query opencode messages for {full_id}: {e}"))?;

        let rows = stmt
            .query_map([&full_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("cannot read opencode messages for {full_id}: {e}"))?;

        let mut messages = Vec::new();
        for row in rows.flatten() {
            let (msg_json, part_json) = row;
            let msg: Value = match serde_json::from_str(&msg_json) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("opencode session {full_id}: skipping unparseable message: {e}");
                    continue;
                }
            };
            let part: Value = match serde_json::from_str(&part_json) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("opencode session {full_id}: skipping unparseable part: {e}");
                    continue;
                }
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
        Ok(messages)
    }
}

fn append_part(messages: &mut Vec<RolloutMessage>, role: &str, timestamp: &str, part: &Value) {
    let ptype = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ptype {
        "text" => {
            if part
                .get("synthetic")
                .and_then(|s| s.as_bool())
                .unwrap_or(false)
            {
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
                thinking: None,
                tool_calls_summary: Vec::new(),
                timestamp: Some(timestamp.to_string()),
                injected: false,
            });
        }
        "reasoning" => {
            if part
                .get("synthetic")
                .and_then(|s| s.as_bool())
                .unwrap_or(false)
            {
                return;
            }
            let thinking = part
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            if thinking.is_empty() {
                return;
            }
            messages.push(RolloutMessage {
                role: role.to_string(),
                content: String::new(),
                thinking: Some(thinking),
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
                thinking: None,
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
                thinking: None,
                tool_calls_summary: Vec::new(),
                timestamp: Some(timestamp.to_string()),
                injected: false,
            });
        }
        "compaction" => {
            messages.push(RolloutMessage {
                role: "user".to_string(),
                content: "context compaction".to_string(),
                thinking: None,
                tool_calls_summary: Vec::new(),
                timestamp: Some(timestamp.to_string()),
                injected: true,
            });
        }
        _ => {}
    }
}

pub(crate) fn ms_to_iso8601(ms: i64) -> String {
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

fn normalize_db_path(path: PathBuf) -> PathBuf {
    if path.is_dir() {
        path.join("opencode.db")
    } else {
        path
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

    fn root_is_from_env(&self) -> bool {
        self.from_env
    }

    fn most_recent_session_id(&self) -> Option<String> {
        let conn = match self.connect() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("{e}");
                return None;
            }
        };
        self.resolve_session_id(&conn, "")
    }

    fn shadow_index_root(&self) -> PathBuf {
        self.db_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(".tantivy")
    }

    fn list_sessions(&self) -> Vec<SessionSummary> {
        let conn = match self.connect() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("{e}");
                return Vec::new();
            }
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

        // Single GROUP BY query: message and part are each aggregated once in
        // derived tables, joined to session — no correlated subqueries per row.
        let sql = "
            SELECT s.id, s.title, s.parent_id, s.directory,
                s.time_created, s.time_updated,
                COALESCE(m.user_count, 0),
                COALESCE(m.assistant_count, 0),
                COALESCE(m.bytes, 0),
                COALESCE(p.tool_count, 0),
                COALESCE(p.compaction_count, 0),
                COALESCE(p.part_count, 0),
                COALESCE(p.bytes, 0)
            FROM session s
            LEFT JOIN (
                SELECT session_id,
                    SUM(CASE WHEN json_extract(data, '$.role') = 'user' THEN 1 ELSE 0 END) AS user_count,
                    SUM(CASE WHEN json_extract(data, '$.role') = 'assistant' THEN 1 ELSE 0 END) AS assistant_count,
                    SUM(length(data)) AS bytes
                FROM message GROUP BY session_id
            ) m ON m.session_id = s.id
            LEFT JOIN (
                SELECT session_id,
                    SUM(CASE WHEN json_extract(data, '$.type') = 'tool' THEN 1 ELSE 0 END) AS tool_count,
                    SUM(CASE WHEN json_extract(data, '$.type') = 'compaction' THEN 1 ELSE 0 END) AS compaction_count,
                    COUNT(*) AS part_count,
                    SUM(length(data)) AS bytes
                FROM part GROUP BY session_id
            ) p ON p.session_id = s.id
            ORDER BY s.time_updated DESC";

        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("opencode session listing query failed: {e}");
                return Vec::new();
            }
        };
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, i64>(11)?,
                row.get::<_, i64>(12)?,
            ))
        });
        let rows = match rows {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("opencode session listing failed: {e}");
                return Vec::new();
            }
        };

        let mut summaries = Vec::new();
        for row in rows.flatten() {
            let (
                id,
                title,
                parent_id,
                directory,
                created,
                updated,
                user_count,
                assistant_count,
                message_bytes,
                tool_count,
                compaction_count,
                part_count,
                part_bytes,
            ) = row;
            summaries.push(SessionSummary {
                session_id: id.clone(),
                title,
                start_time: ms_to_iso8601(created),
                end_time: ms_to_iso8601(updated),
                file_size: (message_bytes + part_bytes).max(0) as u64,
                line_count: part_count.max(0) as u64,
                user_count: user_count.max(0) as u64,
                assistant_count: assistant_count.max(0) as u64,
                tool_count: tool_count.max(0) as u64,
                has_compaction: compaction_count > 0,
                directory,
                parent_session_id: parent_id,
                child_sessions: children.remove(&id).unwrap_or_default(),
                has_tantivy_index: false,
                aliases: Vec::new(),
                read_error: None,
            });
        }
        summaries
    }

    fn list_sessions_scoped(&self, hours_back: u64, directory: Option<&str>) -> SessionListing {
        let conn = match self.connect() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("{e}");
                return SessionListing {
                    sessions: Vec::new(),
                    window_count: 0,
                };
            }
        };
        scoped_listing_sqlite(&conn, hours_back, directory)
    }

    fn read_session(&self, session_id: &str) -> ReadResult<Vec<RolloutMessage>> {
        let conn = self.connect()?;
        let full_id = self
            .resolve_session_id(&conn, session_id)
            .ok_or_else(|| no_session_error(&self.db_path, session_id))?;
        Self::load_messages(&conn, &full_id)
    }

    /// mmap applies to file-based rollout formats; SQLite is read through the
    /// message_session_time_created_id_idx index instead (no N+1, no full scan).
    fn read_session_mmap(&self, session_id: &str) -> ReadResult<Vec<RolloutMessage>> {
        self.read_session(session_id)
    }

    fn read_session_from_compaction(&self, session_id: &str) -> ReadResult<Vec<RolloutMessage>> {
        Ok(slice_from_compaction(self.read_session_mmap(session_id)?))
    }

    fn profile_session_opts(&self, session_id: &str, cache: bool) -> ReadResult<SessionProfile> {
        let conn = self.connect()?;
        let full_id = self
            .resolve_session_id(&conn, session_id)
            .ok_or_else(|| no_session_error(&self.db_path, session_id))?;

        let cache_path = crate::profile_cache::cache_path(&self.shadow_index_root(), &full_id);
        if cache
            && let Some(time_updated) = session_time_updated(&conn, &full_id)
            && let Some(cached) = crate::profile_cache::read_fresh(&cache_path, time_updated)
        {
            return Ok(cached);
        }

        let profile = profile_session_sqlite(&conn, &full_id)?;
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

    fn read_todo_writes(&self, session_id: &str, full: bool) -> ReadResult<Vec<TodoWrite>> {
        let conn = self.connect()?;
        let full_id = self
            .resolve_session_id(&conn, session_id)
            .ok_or_else(|| no_session_error(&self.db_path, session_id))?;
        load_todo_writes(&conn, &full_id, full)
    }

    fn she_said_he_said_action(
        &self,
        sessions: &[String],
        words: &[String],
        hours_back: u64,
        directory: Option<&str>,
    ) -> Result<String, String> {
        she_said_he_said(&self.db_path, sessions, words, hours_back, directory)
    }
}

/// The scoped listing for the opencode store. The window, the directory
/// substring and the row cap are pushed into SQL, so out-of-window and
/// out-of-cap sessions cost nothing in aggregates.
///
/// The bounded session set is materialized once as the `window` CTE (the
/// cap is inside it). The message/part aggregates then only look up
/// sessions in that set — through the store's real indexes
/// (`message(session_id, time_created, id)` and `part(session_id)`) — and
/// the window's total is a bare `COUNT(*)` over the same predicate, with no
/// aggregates anywhere near it. Child links are resolved for the returned
/// rows only.
fn scoped_listing_sqlite(
    conn: &Connection,
    hours_back: u64,
    directory: Option<&str>,
) -> SessionListing {
    let now = now_ms();
    let cutoff = now - (hours_back as i64).saturating_mul(3_600_000);

    let window_count: usize = match conn.query_row(
        "SELECT COUNT(*) FROM session
         WHERE (?1 = 0 OR time_updated >= ?2)
           AND (?3 IS NULL OR directory LIKE '%' || ?3 || '%')",
        rusqlite::params![hours_back as i64, cutoff, directory],
        |r| r.get::<_, i64>(0),
    ) {
        Ok(n) => n.max(0) as usize,
        Err(e) => {
            tracing::warn!("opencode scoped listing count failed: {e}");
            return SessionListing {
                sessions: Vec::new(),
                window_count: 0,
            };
        }
    };

    // Single GROUP BY query over the bounded set: message and part are each
    // aggregated once in derived tables that only ever look up sessions in
    // `window`, joined to it — no correlated subqueries per row.
    let sql = "
        WITH window AS (
            SELECT id, title, parent_id, directory, time_created, time_updated
            FROM session
            WHERE (?1 = 0 OR time_updated >= ?2)
              AND (?3 IS NULL OR directory LIKE '%' || ?3 || '%')
            ORDER BY time_updated DESC
            LIMIT ?4
        )
        SELECT w.id, w.title, w.parent_id, w.directory, w.time_created, w.time_updated,
            COALESCE(m.user_count, 0),
            COALESCE(m.assistant_count, 0),
            COALESCE(m.bytes, 0),
            COALESCE(p.tool_count, 0),
            COALESCE(p.compaction_count, 0),
            COALESCE(p.part_count, 0),
            COALESCE(p.bytes, 0)
        FROM window w
        LEFT JOIN (
            SELECT session_id,
                SUM(CASE WHEN json_extract(data, '$.role') = 'user' THEN 1 ELSE 0 END) AS user_count,
                SUM(CASE WHEN json_extract(data, '$.role') = 'assistant' THEN 1 ELSE 0 END) AS assistant_count,
                SUM(length(data)) AS bytes
            FROM message
            WHERE session_id IN (SELECT id FROM window)
            GROUP BY session_id
        ) m ON m.session_id = w.id
        LEFT JOIN (
            SELECT session_id,
                SUM(CASE WHEN json_extract(data, '$.type') = 'tool' THEN 1 ELSE 0 END) AS tool_count,
                SUM(CASE WHEN json_extract(data, '$.type') = 'compaction' THEN 1 ELSE 0 END) AS compaction_count,
                COUNT(*) AS part_count,
                SUM(length(data)) AS bytes
            FROM part
            WHERE session_id IN (SELECT id FROM window)
            GROUP BY session_id
        ) p ON p.session_id = w.id
        ORDER BY w.time_updated DESC";

    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("opencode scoped listing query failed: {e}");
            return SessionListing {
                sessions: Vec::new(),
                window_count,
            };
        }
    };
    let rows = stmt.query_map(
        rusqlite::params![hours_back as i64, cutoff, directory, LISTING_ROW_CAP as i64],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, i64>(11)?,
                row.get::<_, i64>(12)?,
            ))
        },
    );
    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("opencode scoped listing failed: {e}");
            return SessionListing {
                sessions: Vec::new(),
                window_count,
            };
        }
    };

    let mut summaries = Vec::new();
    for row in rows.flatten() {
        let (
            id,
            title,
            parent_id,
            directory,
            created,
            updated,
            user_count,
            assistant_count,
            message_bytes,
            tool_count,
            compaction_count,
            part_count,
            part_bytes,
        ) = row;
        summaries.push(SessionSummary {
            session_id: id.clone(),
            title,
            start_time: ms_to_iso8601(created),
            end_time: ms_to_iso8601(updated),
            file_size: (message_bytes + part_bytes).max(0) as u64,
            line_count: part_count.max(0) as u64,
            user_count: user_count.max(0) as u64,
            assistant_count: assistant_count.max(0) as u64,
            tool_count: tool_count.max(0) as u64,
            has_compaction: compaction_count > 0,
            directory,
            parent_session_id: parent_id,
            child_sessions: Vec::new(),
            has_tantivy_index: false,
            aliases: Vec::new(),
            read_error: None,
        });
    }

    // Child links for the returned rows only: parents in the bounded set.
    // A session-table query, cheap next to the aggregates above it.
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    if !summaries.is_empty() {
        let ids: Vec<&str> = summaries.iter().map(|s| s.session_id.as_str()).collect();
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        // Most-recent-first, so the ids a bounded row keeps are the ones a
        // caller would have started from: the rendering cap
        // (`IDS_PER_LISTING_ROW`) cuts the tail, never the head.
        let sql = format!(
            "SELECT parent_id, id FROM session WHERE parent_id IN ({placeholders}) \
             ORDER BY time_updated DESC"
        );
        if let Ok(mut stmt) = conn.prepare(&sql)
            && let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
        {
            for row in rows.flatten() {
                children.entry(row.0).or_default().push(row.1);
            }
        }
    }
    for summary in &mut summaries {
        summary.child_sessions = children.remove(&summary.session_id).unwrap_or_default();
    }

    SessionListing {
        sessions: summaries,
        window_count,
    }
}

/// One cheap single-row query for the staleness check: the session's
/// `time_updated` only — never the full `list_sessions` aggregates.
fn session_time_updated(conn: &Connection, full_id: &str) -> Option<i64> {
    conn.query_row(
        "SELECT time_updated FROM session WHERE id = ?1",
        [full_id],
        |r| r.get(0),
    )
    .ok()
}

/// Load every `todowrite` flush of a session, in message/part order. The
/// part stream is filtered in Rust, exactly like `load_messages`: a part
/// whose JSON is unparseable is skipped with a warning, never fatal and
/// never silently empty (a malformed row cannot take the query down with
/// it). `full = false` slices at the session's last compaction part — rows
/// before the marker are dropped, mirroring how `read_session_from_compaction`
/// finds the marker.
fn load_todo_writes(conn: &Connection, full_id: &str, full: bool) -> ReadResult<Vec<TodoWrite>> {
    let mut stmt = conn
        .prepare(
            "SELECT m.data, p.data, p.time_created
              FROM message m
              JOIN part p ON m.id = p.message_id
              WHERE m.session_id = ?1
              ORDER BY m.time_created ASC, p.time_created ASC",
        )
        .map_err(|e| format!("cannot query opencode todo writes for {full_id}: {e}"))?;

    let rows = stmt
        .query_map([full_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| format!("cannot read opencode todo writes for {full_id}: {e}"))?;

    // One ordered pass: parse every row, remember the last compaction part,
    // keep what the window allows.
    let mut last_compaction: Option<i64> = None;
    let mut parsed: Vec<(Value, Value, i64)> = Vec::new();
    for row in rows.flatten() {
        let (msg_json, part_json, part_time) = row;
        let msg: Value = match serde_json::from_str(&msg_json) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("opencode session {full_id}: skipping unparseable message: {e}");
                continue;
            }
        };
        let part: Value = match serde_json::from_str(&part_json) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("opencode session {full_id}: skipping unparseable part: {e}");
                continue;
            }
        };
        if part.get("type").and_then(|t| t.as_str()) == Some("compaction") {
            last_compaction = Some(part_time);
        }
        parsed.push((msg, part, part_time));
    }

    let mut writes = Vec::new();
    for (msg, part, part_time) in parsed {
        if !full && last_compaction.is_some_and(|marker| part_time < marker) {
            continue;
        }
        if part.get("type").and_then(|t| t.as_str()) != Some("tool") {
            continue;
        }
        if part.get("tool").and_then(|t| t.as_str()) != Some("todowrite") {
            continue;
        }
        let timestamp = msg
            .get("time")
            .and_then(|t| t.get("created"))
            .and_then(|t| t.as_i64())
            .map(ms_to_iso8601);
        let Some(todos) = todo_items_from_part(&part, full_id) else {
            continue;
        };
        writes.push(TodoWrite { timestamp, todos });
    }
    Ok(writes)
}

/// The todo list a todowrite part carried, or `None` when the part is not a
/// flush: a failed call (`input` without a `todos` list) is skipped quietly,
/// while a todo list that is present but unparseable — a JSON-encoded
/// `input` or `todos` string that does not parse, a non-array `todos`, an
/// item without a string `content` — is skipped with a warning, so the
/// damage is diagnosable. Both `input` and `todos` are accepted as JSON
/// objects or as JSON-encoded strings (both shapes occur in the live store).
fn todo_items_from_part(part: &Value, full_id: &str) -> Option<Vec<TodoItemState>> {
    let input = part.get("state")?.get("input")?;
    let input = match input {
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "opencode session {full_id}: skipping todowrite part with an unparseable input: {e}"
                );
                return None;
            }
        },
        other => other.clone(),
    };
    let todos = input.get("todos")?;
    let todos = match todos {
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "opencode session {full_id}: skipping todowrite part with an unparseable todos list: {e}"
                );
                return None;
            }
        },
        other => other.clone(),
    };
    let arr = match todos.as_array() {
        Some(a) => a,
        None => {
            tracing::warn!(
                "opencode session {full_id}: skipping todowrite part whose todos is not an array"
            );
            return None;
        }
    };
    let mut items = Vec::with_capacity(arr.len());
    for item in arr {
        match serde_json::from_value::<TodoItemState>(item.clone()) {
            Ok(i) => items.push(i),
            Err(e) => {
                tracing::warn!(
                    "opencode session {full_id}: skipping todowrite part with an unparseable todo item: {e}"
                );
                return None;
            }
        }
    }
    Some(items)
}

/// Compute the profile from SQLite for an already-resolved session id.
fn profile_session_sqlite(conn: &Connection, full_id: &str) -> ReadResult<SessionProfile> {
    let file_size: i64 = conn
        .query_row(
            "SELECT (SELECT COALESCE(SUM(length(data)), 0) FROM message WHERE session_id = ?1)
                + (SELECT COALESCE(SUM(length(data)), 0) FROM part WHERE session_id = ?1)",
            [full_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let line_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM part WHERE session_id = ?1",
            [full_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let messages = OpenCodeAdapter::load_messages(conn, full_id)?;

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

    Ok(SessionProfile {
        session_id: full_id.to_string(),
        file_size: file_size.max(0) as u64,
        line_count: line_count.max(0) as u64,
        first_ts,
        last_ts,
        role_counts,
        has_tantivy_index: false,
        interesting_events,
    })
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// ISO8601 cutoff string for `hours_back` hours ago (used for string
/// comparisons against SessionSummary::end_time).
pub(crate) fn iso_cutoff(hours_back: u64) -> String {
    ms_to_iso8601(now_ms() - (hours_back as i64) * 3_600_000)
}

/// Render an optional SQLite value as text for term matching. NULL and JSON
/// null yield None; scalars render as their text form.
fn sql_value_to_text(v: Option<SqlValue>) -> Option<String> {
    match v? {
        SqlValue::Text(s) => Some(s),
        SqlValue::Integer(n) => Some(n.to_string()),
        SqlValue::Real(f) => Some(f.to_string()),
        SqlValue::Blob(b) => Some(String::from_utf8_lossy(&b).into_owned()),
        _ => None,
    }
}

/// Term-matched dialogue and tool actions from the opencode SQLite rollout
/// store. Matching is pushed down into the query engine as a connection-local
/// `matches_any` scalar function on a read-only connection — the database is
/// never written.
fn she_said_he_said(
    db_path: &PathBuf,
    sessions: &[String],
    words: &[String],
    hours_back: u64,
    directory: Option<&str>,
) -> Result<String, String> {
    if words.is_empty() {
        return Err(
            "she_said_he_said_action requires at least one word to match; pass terms in `words`"
                .to_string(),
        );
    }
    let conn =
        Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| {
            format!(
                "cannot open opencode database at {}: {}",
                db_path.display(),
                e
            )
        })?;

    let terms: Arc<Vec<String>> = Arc::new(words.iter().map(|w| w.to_lowercase()).collect());
    {
        let terms = Arc::clone(&terms);
        conn.create_scalar_function(
            "matches_any",
            -1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            move |ctx| {
                for i in 0..ctx.len() {
                    let arg = ctx.get::<Option<SqlValue>>(i).ok().flatten();
                    if let Some(text) = sql_value_to_text(arg) {
                        let lower = text.to_lowercase();
                        if terms
                            .iter()
                            .any(|t| !t.is_empty() && lower.contains(t.as_str()))
                        {
                            return Ok(1);
                        }
                    }
                }
                Ok(0)
            },
        )
        .map_err(|e| format!("failed to register matches_any: {}", e))?;
    }

    // Resolve the ordered session list first: (id, title, time_updated),
    // most recent first. Explicit partial ids resolve to the most recent
    // match; unresolvable ones are reported in the header, not fatal.
    let now = now_ms();
    let mut ordered: Vec<(String, String, i64)> = Vec::new();
    let mut unmatched: Vec<String> = Vec::new();

    if !sessions.is_empty() {
        for partial in sessions {
            let resolved = conn
                .query_row(
                    "SELECT id, title, time_updated FROM session
                     WHERE id LIKE '%' || ?1 || '%'
                     ORDER BY time_updated DESC LIMIT 1",
                    [partial],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, i64>(2)?,
                        ))
                    },
                )
                .ok();
            match resolved {
                Some(row) if !ordered.iter().any(|(id, _, _)| *id == row.0) => ordered.push(row),
                Some(_) => {}
                None => unmatched.push(partial.clone()),
            }
        }
        ordered.sort_by_key(|(_, _, updated)| std::cmp::Reverse(*updated));
    } else {
        let sql = "SELECT id, title, time_updated FROM session
                   WHERE (?1 = 0 OR time_updated >= ?2)
                     AND (?3 IS NULL OR directory LIKE '%' || ?3 || '%')
                   ORDER BY time_updated DESC";
        let rows = {
            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| format!("session selection failed: {}", e))?;
            let rows = stmt.query_map(
                rusqlite::params![
                    hours_back as i64,
                    now - (hours_back as i64) * 3_600_000,
                    directory
                ],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            );
            let rows = rows.map_err(|e| format!("session selection failed: {}", e))?;
            rows.flatten().collect::<Vec<_>>()
        };
        for row in rows {
            ordered.push(row);
        }
    }

    // One streamed query per <=500-session chunk: matching is pushed down into
    // SQLite, rows stream out classified in Rust (text+user → he,
    // text+assistant → she, tool → they) and accumulate per session, to be
    // assembled in session order.
    let ids: Vec<String> = ordered.iter().map(|(id, _, _)| id.clone()).collect();
    let mut he: HashMap<String, Vec<String>> = HashMap::new();
    let mut she: HashMap<String, Vec<String>> = HashMap::new();
    let mut they: HashMap<String, Vec<String>> = HashMap::new();

    for chunk in ids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");

        // No string interpolation into SQL beyond the placeholder list; every
        // value flows through a bound parameter or the matches_any function.
        let sql = format!(
            "SELECT p.session_id,
                    json_extract(m.data, '$.role') AS role,
                    json_extract(p.data, '$.type') AS ptype,
                    json_extract(p.data, '$.synthetic') AS synthetic,
                    json_extract(p.data, '$.text') AS text,
                    json_extract(p.data, '$.tool') AS tool,
                    json_extract(p.data, '$.state.input') AS input
             FROM part p JOIN message m ON m.id = p.message_id
             WHERE p.session_id IN ({placeholders})
               AND (
                 (json_extract(p.data, '$.type') = 'text'
                   AND json_extract(m.data, '$.role') IN ('user', 'assistant')
                   AND COALESCE(json_extract(p.data, '$.synthetic'), 0) = 0
                   AND matches_any(json_extract(p.data, '$.text')))
                 OR (json_extract(p.data, '$.type') = 'tool'
                   AND (matches_any(json_extract(p.data, '$.tool'))
                        OR matches_any(json_extract(p.data, '$.state.input'))))
               )
             ORDER BY p.time_created ASC",
            placeholders = placeholders
        );

        let hits = {
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| format!("scan failed: {}", e))?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<SqlValue>>(1)?,
                        r.get::<_, Option<SqlValue>>(2)?,
                        r.get::<_, Option<SqlValue>>(4)?,
                        r.get::<_, Option<SqlValue>>(5)?,
                        r.get::<_, Option<SqlValue>>(6)?,
                    ))
                })
                .map_err(|e| format!("scan failed: {}", e))?;
            rows.flatten()
                .map(|(sid, role, ptype, text, tool, input)| {
                    let role = sql_value_to_text(role).unwrap_or_default();
                    let ptype = sql_value_to_text(ptype).unwrap_or_default();
                    let line = if ptype == "tool" {
                        let tool = sql_value_to_text(tool).unwrap_or_else(|| "?".to_string());
                        let args = sql_value_to_text(input).unwrap_or_else(|| "null".to_string());
                        summarize_tool_call(&tool, &args)
                    } else {
                        sql_value_to_text(text).unwrap_or_default()
                    };
                    (sid, role, ptype, line)
                })
                .collect::<Vec<_>>()
        };
        for (sid, role, ptype, line) in hits {
            if ptype == "tool" {
                they.entry(sid).or_default().push(line);
            } else if role == "user" {
                he.entry(sid).or_default().push(line);
            } else {
                she.entry(sid).or_default().push(line);
            }
        }
    }

    // Assemble the markdown report, sessions most-recent first, hits in
    // time_created ASC order (as streamed above).
    let with_hits = ordered
        .iter()
        .filter(|(id, _, _)| {
            he.get(id).is_some_and(|v| !v.is_empty())
                || she.get(id).is_some_and(|v| !v.is_empty())
                || they.get(id).is_some_and(|v| !v.is_empty())
        })
        .count();

    let mut out = format!("# she-said-he-said-action — terms: {}\n", words.join(", "));
    let mut header = format!(
        "{} sessions scanned, {} with hits",
        ordered.len(),
        with_hits
    );
    if !unmatched.is_empty() {
        header += &format!(
            ", {} unmatched session ids: {}",
            unmatched.len(),
            unmatched.join(", ")
        );
    }
    out += &header;
    out.push('\n');

    for (id, title, _) in &ordered {
        let he_hits = he.get(id).map(Vec::as_slice).unwrap_or(&[]);
        let she_hits = she.get(id).map(Vec::as_slice).unwrap_or(&[]);
        let they_hits = they.get(id).map(Vec::as_slice).unwrap_or(&[]);
        if he_hits.is_empty() && she_hits.is_empty() && they_hits.is_empty() {
            continue;
        }
        out += &format!("\n=== {} ({})\n", title, id);
        if !he_hits.is_empty() {
            out += "--- HE SAID:\n";
            for text in he_hits {
                out += truncate_chars(text, 1500);
                out.push('\n');
            }
        }
        if !she_hits.is_empty() {
            out += "--- SHE SAID:\n";
            for text in she_hits {
                out += truncate_chars(text, 1500);
                out.push('\n');
            }
        }
        if !they_hits.is_empty() {
            out += "--- THEY DID (tool calls):\n";
            let mut prev: Option<&str> = None;
            for line in they_hits {
                if prev == Some(line.as_str()) {
                    continue;
                }
                out += line;
                out.push('\n');
                prev = Some(line);
            }
        }
    }

    Ok(out)
}
