use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{
    EventType, InterestingEvent, QaAnswerRecord, QaPair, ReadResult, RolloutAdapter,
    RolloutMessage, SessionListing, SessionProfile, SessionSummary, VIBE_ROOT_ENV_VAR,
    listing_by_mtime, mmap_error, mtime_in_window, no_session_error, pair_questions_with_answers,
    parse_answers_text, parse_questions_payload, read_error, resolve_root, resolve_session_dir,
    slice_from_compaction, summarize_tool_call,
};

/// Vibe adapter. Reads sessions from ~/.vibe/logs/session/
///
/// Legacy session directory structure:
///   session_YYYYMMDD_HHMMSS_<short-id>/
///   ├── messages.jsonl   (one JSON object per line)
///   └── meta.json        (session metadata)
///
/// Vibe 3.0.3 added a second, side-by-side layout: the unified-session store
/// `unified/<uuid>/` (CURRENT pointer, meta.json, hash-chained
/// journal/*.jsonl segments, content-addressed chunks/<sha256>.json). Both
/// are read: unified sessions by journal replay, legacy sessions by
/// messages.jsonl, each id namespace its own.
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
                Err(e) => tracing::warn!("vibe rollout: skipping unparseable line {}: {e}", i + 1),
            }
        }
        messages
    }

    fn parse_jsonl_mmap(path: &Path) -> ReadResult<Vec<RolloutMessage>> {
        let file = std::fs::File::open(path).map_err(|e| read_error(path, &e))?;
        let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| mmap_error(path, &e))?;
        Ok(Self::parse_jsonl(&mmap[..]))
    }

    /// Read the raw payload bytes for a session, naming the path on failure.
    fn read_payload(&self, session_id: &str) -> ReadResult<Vec<u8>> {
        let path = self
            .messages_path(session_id)
            .ok_or_else(|| no_session_error(&self.root, session_id))?;
        std::fs::read(&path).map_err(|e| read_error(&path, &e))
    }

    /// Session metadata is decorative (title, times): an absent or corrupt
    /// `meta.json` leaves the session listed with blank metadata, and the
    /// reason is logged rather than swallowed.
    fn read_meta(&self, dir: &Path) -> Option<serde_json::Value> {
        let meta_path = dir.join("meta.json");
        let data = match std::fs::read(&meta_path) {
            Ok(d) => d,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("vibe session {}: cannot read meta.json: {e}", dir.display());
                }
                return None;
            }
        };
        match serde_json::from_slice(&data) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(
                    "vibe session {}: meta.json is not valid JSON: {e}",
                    dir.display()
                );
                None
            }
        }
    }

    /// The store's session directories: the legacy `session_*` layout plus
    /// the vibe 3.0.3 unified-store sessions (`unified/<uuid>/`), enumerated
    /// exactly as the full listing enumerates them.
    fn session_dirs(&self) -> Vec<(String, PathBuf)> {
        let mut out: Vec<(String, PathBuf)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("session_") {
                    out.push((name, entry.path()));
                }
            }
        }
        for (name, path) in self.unified_session_dirs() {
            out.push((name, path));
        }
        out
    }

    /// The vibe 3.0.3 unified-session store: `unified/<uuid>/` directories,
    /// one per session.
    fn unified_session_dirs(&self) -> Vec<(String, PathBuf)> {
        let unified = self.root.join("unified");
        let entries = match std::fs::read_dir(&unified) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                (name, entry.path())
            })
            .collect()
    }

    /// Resolve a unified session id — full uuid, or a prefix long enough to
    /// name exactly one directory — to its store path. An empty id means the
    /// most recently written unified session.
    fn unified_session_dir(&self, session_id: &str) -> Option<PathBuf> {
        let dirs = self.unified_session_dirs();
        if session_id.is_empty() {
            return dirs
                .into_iter()
                .map(|(_, path)| {
                    let stamp =
                        unified_payload_mtime(&path).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    (stamp, path)
                })
                .max_by_key(|(stamp, _)| *stamp)
                .map(|(_, path)| path);
        }
        if let Some((_, path)) = dirs.iter().find(|(name, _)| name.as_str() == session_id) {
            return Some(path.clone());
        }
        dirs.iter()
            .filter(|(name, _)| name.contains(session_id))
            .min_by_key(|(name, _)| name.len())
            .map(|(_, path)| path.clone())
    }

    /// Journal segment paths of a unified store, in replay (seq) order —
    /// the segments are named `NNNNNNNNNNNNNN.jsonl` by sequence.
    fn journal_segment_paths(dir: &Path) -> Vec<PathBuf> {
        let mut segs: Vec<PathBuf> = match std::fs::read_dir(dir.join("journal")) {
            Ok(entries) => entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
                .collect(),
            Err(_) => Vec::new(),
        };
        segs.sort();
        segs
    }

    /// Replay one unified session from its journal. `core_input` records
    /// carry the user's turns, `action_result` records the assistant's
    /// outputs (text, reasoning, tool calls); the other validated record
    /// types (turn bookkeeping, receipts, projection deltas, model-input
    /// echoes) contribute nothing, and an unparseable journal line is
    /// skipped with a warning — never fatal, never fatally short.
    fn read_unified_session(&self, session_id: &str) -> Option<Vec<RolloutMessage>> {
        let dir = self.unified_session_dir(session_id)?;
        let mut out = Vec::new();
        for seg in Self::journal_segment_paths(&dir) {
            let data = match std::fs::read(&seg) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("vibe journal {}: cannot read: {e}", seg.display());
                    continue;
                }
            };
            for line in String::from_utf8_lossy(&data).lines() {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<serde_json::Value>(line) {
                    Ok(record) => out.extend(unified_record_messages(&record)),
                    Err(e) => tracing::warn!(
                        "vibe journal {}: skipping unparseable line: {e}",
                        seg.display()
                    ),
                }
            }
        }
        Some(out)
    }

    /// One unified session summarized: size/lines/roles from the journal
    /// replay, title and times from `meta.json` (the same decorative
    /// contract as the legacy summary).
    fn summarize_unified(&self, path: &Path, name: &str) -> (SessionSummary, u64) {
        let messages = self.read_unified_session(name).unwrap_or_default();

        let file_size: u64 = Self::journal_segment_paths(path)
            .iter()
            .filter_map(|seg| std::fs::metadata(seg).ok())
            .map(|m| m.len())
            .sum();
        let line_count = messages.len() as u64;

        let meta = self.read_meta(path);
        let get_str = |key: &str| -> String {
            meta.as_ref()
                .and_then(|m| m.get(key))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string()
        };

        let mut user_count = 0u64;
        let mut assistant_count = 0u64;
        let mut tool_count = 0u64;
        let mut has_compaction = false;
        for m in &messages {
            match m.role.as_str() {
                "user" => {
                    user_count += 1;
                    has_compaction |= m.content.contains("context compaction");
                }
                "assistant" => assistant_count += 1,
                "tool" => tool_count += 1,
                _ => {}
            }
        }

        // Content hash over the replayed payload; 0 marks an empty replay,
        // and empty stores are never deduped together.
        let content_hash = if file_size == 0 {
            0
        } else {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for m in &messages {
                m.content.hash(&mut h);
            }
            h.finish()
        };

        (
            SessionSummary {
                session_id: name.to_string(),
                title: get_str("title"),
                start_time: get_str("start_time"),
                end_time: get_str("end_time"),
                file_size,
                line_count,
                user_count,
                assistant_count,
                tool_count,
                has_compaction,
                directory: meta
                    .as_ref()
                    .and_then(|m| m.pointer("/environment/working_directory"))
                    .and_then(|d| d.as_str())
                    .map(str::to_string),
                parent_session_id: meta
                    .as_ref()
                    .and_then(|m| m.get("parent_session_id"))
                    .and_then(|p| p.as_str())
                    .map(str::to_string),
                child_sessions: Vec::new(),
                has_tantivy_index: false,
                aliases: Vec::new(),
                read_error: None,
            },
            content_hash,
        )
    }

    /// Profile of a unified session: real counters from the journal replay.
    /// The interesting-events classification mirrors the legacy loop's
    /// tool-call families (todo, git operations) over the summarized calls.
    fn profile_unified(&self, session_id: &str, cache: bool) -> ReadResult<SessionProfile> {
        let dir = self
            .unified_session_dir(session_id)
            .ok_or_else(|| no_session_error(&self.root, session_id))?;

        let cache_path = crate::profile_cache::cache_path(&self.shadow_index_root(), session_id);
        let stamp = unified_payload_mtime(&dir)
            .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        if cache && let Some(cached) = crate::profile_cache::read_fresh(&cache_path, stamp) {
            return Ok(cached);
        }

        let messages = self
            .read_unified_session(session_id)
            .ok_or_else(|| no_session_error(&self.root, session_id))?;

        let file_size: u64 = Self::journal_segment_paths(&dir)
            .iter()
            .filter_map(|seg| std::fs::metadata(seg).ok())
            .map(|m| m.len())
            .sum();
        let line_count = messages.len() as u64;

        let mut role_counts: HashMap<String, u64> = HashMap::new();
        let mut interesting_events = Vec::new();
        let mut last_event_line = 0u64;
        let mut first_ts: Option<String> = None;
        let mut last_ts: Option<String> = None;

        for (i, m) in messages.iter().enumerate() {
            let line_num = (i + 1) as u64;
            *role_counts.entry(m.role.clone()).or_insert(0) += 1;

            if m.content.contains("context compaction") {
                interesting_events.push(InterestingEvent {
                    line_number: line_num,
                    event_type: EventType::Compaction,
                    summary: "Compaction marker".to_string(),
                    gap_lines: line_num - last_event_line,
                });
                last_event_line = line_num;
            }

            if m.role == "user" && !m.injected {
                interesting_events.push(InterestingEvent {
                    line_number: line_num,
                    event_type: EventType::UserMessage,
                    summary: m.content.chars().take(80).collect(),
                    gap_lines: line_num - last_event_line,
                });
                last_event_line = line_num;
            }

            for summary in &m.tool_calls_summary {
                let event_type = if summary.contains("git commit") {
                    Some(EventType::GitCommit)
                } else if summary.contains("git push") {
                    Some(EventType::GitPush)
                } else if summary.contains("git tag") {
                    Some(EventType::GitTag)
                } else {
                    None
                };
                if let Some(event_type) = event_type {
                    interesting_events.push(InterestingEvent {
                        line_number: line_num,
                        event_type,
                        summary: summary.clone(),
                        gap_lines: line_num - last_event_line,
                    });
                    last_event_line = line_num;
                }
            }

            if let Some(ts) = &m.timestamp {
                if first_ts.is_none() {
                    first_ts = Some(ts.clone());
                }
                last_ts = Some(ts.clone());
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

    /// One session directory summarized into ([`SessionSummary`], content
    /// hash). Damage is surfaced on the entry; the content hash drives the
    /// dedupe. Callers apply the mtime window before calling, so this
    /// payload read only ever happens for in-window sessions.
    fn summarize_dir(&self, path: &Path, name: &str) -> (SessionSummary, u64) {
        // A 3.0.3 unified store is hashed journal instead of messages.jsonl.
        if path.join("journal").is_dir() && !path.join("messages.jsonl").exists() {
            return self.summarize_unified(path, name);
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

        let meta = self.read_meta(path);

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

        let session_id = name.to_string();

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

        (
            SessionSummary {
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
            },
            content_hash,
        )
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
        let mut summaries = Vec::new();
        // content hash captured alongside each summary for dedupe.
        let mut identity: Vec<u64> = Vec::new();

        for (name, path) in self.session_dirs() {
            let (summary, content_hash) = self.summarize_dir(&path, &name);
            summaries.push(summary);
            identity.push(content_hash);
        }

        // Dedupe: one canonical entry per rollout payload. Two dirs describing
        // the same rollout share (file_size, line_count, content hash); the
        // canonical id is the dir-name whose date prefix agrees with
        // meta.start_time, and the rest become aliases.
        let mut summaries = dedupe_vibe_sessions(summaries, identity);
        // Sort by start_time descending (most recent first)
        summaries.sort_by(|a, b| b.start_time.cmp(&a.start_time));
        summaries
    }

    fn list_sessions_scoped(&self, hours_back: u64, _directory: Option<&str>) -> SessionListing {
        // Vibe summaries carry no directory (None passes every filter, as in
        // the full listing), so the payload's mtime is the only cheap bound —
        // decided before the payload is ever read.
        let mut mtimes: HashMap<String, Option<std::time::SystemTime>> = HashMap::new();
        let mut summaries = Vec::new();
        let mut identity: Vec<u64> = Vec::new();

        for (name, path) in self.session_dirs() {
            // Legacy payload mtime, or — for unified stores — the newest
            // journal segment's mtime.
            let mtime = std::fs::metadata(path.join("messages.jsonl"))
                .and_then(|m| m.modified())
                .ok()
                .or_else(|| unified_payload_mtime(&path));
            if !mtime_in_window(mtime, hours_back) {
                continue;
            }
            let (summary, content_hash) = self.summarize_dir(&path, &name);
            mtimes.insert(summary.session_id.clone(), mtime);
            summaries.push(summary);
            identity.push(content_hash);
        }

        let summaries = dedupe_vibe_sessions(summaries, identity);
        // The canonical entry of each dedupe group is one of the walked
        // directories, so its mtime is in the map; aliases are dropped.
        let rows = summaries
            .into_iter()
            .map(|s| (mtimes.remove(&s.session_id).flatten(), s))
            .collect();
        listing_by_mtime(rows)
    }

    fn read_session(&self, session_id: &str) -> ReadResult<Vec<RolloutMessage>> {
        // A unified-store id never collides with a legacy session_* name, so
        // trying unified first only ever changes unified sessions.
        if let Some(messages) = self.read_unified_session(session_id) {
            return Ok(messages);
        }
        Ok(Self::parse_jsonl(&self.read_payload(session_id)?))
    }

    fn read_session_mmap(&self, session_id: &str) -> ReadResult<Vec<RolloutMessage>> {
        // Unified stores are replayed, not mmapped: the journal is segmented
        // and content-addressed, so there is no single payload to map.
        if let Some(messages) = self.read_unified_session(session_id) {
            return Ok(messages);
        }
        let path = self
            .messages_path(session_id)
            .ok_or_else(|| no_session_error(&self.root, session_id))?;
        Self::parse_jsonl_mmap(&path)
    }

    fn read_session_from_compaction(&self, session_id: &str) -> ReadResult<Vec<RolloutMessage>> {
        Ok(slice_from_compaction(self.read_session_mmap(session_id)?))
    }

    fn profile_session_opts(&self, session_id: &str, cache: bool) -> ReadResult<SessionProfile> {
        if self.unified_session_dir(session_id).is_some() {
            return self.profile_unified(session_id, cache);
        }
        let path = self
            .messages_path(session_id)
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

    /// Question/answer pairs read from the raw `messages.jsonl`: the full
    /// question payload (header, options) is parsed from the assistant
    /// `ask_user_question` call's arguments, and the answer is looked up by
    /// `tool_call_id` — the structured `tool_result.output.answers[]` when
    /// the tool message carries it, the python-ish `answers: [...]` content
    /// text otherwise. A cancelled call or an unanswered question yields no
    /// pair. Pairing follows `tool_call_id`, so a question asked twice in
    /// one session keeps each call's own answer.
    fn read_qa_pairs(&self, session_id: &str) -> ReadResult<Vec<QaPair>> {
        // No question-tool record shapes are validated in the unified replay
        // yet, so unified sessions report no pairs — never an error. Legacy
        // sessions keep the full pairing.
        if self.unified_session_dir(session_id).is_some() {
            return Ok(Vec::new());
        }
        let data = self.read_payload(session_id)?;
        let text = String::from_utf8_lossy(&data);
        Ok(qa_pairs_from_vibe_lines(&text))
    }

    /// Byte-faithful raw entries: re-parse the native `messages.jsonl` so each
    /// emitted record is the source line's JSON, not a normalized projection.
    fn read_session_entries(
        &self,
        session_id: &str,
        full: bool,
        include_injected: bool,
    ) -> ReadResult<Vec<super::RolloutEntry>> {
        if let Some(messages) = self.read_unified_session(session_id) {
            // Raw-record projection from the replay; entry slicing mirrors
            // the legacy compaction window.
            return Ok(unified_entries(messages, full, include_injected));
        }
        let data = self.read_payload(session_id)?;
        let text = String::from_utf8_lossy(&data);

        // Collect raw (line_value) pairs, then honour the compaction window.
        let mut raw: Vec<serde_json::Value> = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<serde_json::Value>(line) {
                Ok(v) => raw.push(v),
                Err(e) => {
                    tracing::warn!(
                        "vibe rollout {session_id}: skipping unparseable line {}: {e}",
                        i + 1
                    )
                }
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
            let timestamp = v
                .get("timestamp")
                .and_then(|t| t.as_str())
                .map(String::from);
            out.push(super::RolloutEntry {
                index: out.len(),
                entry_type,
                timestamp,
                record: v.clone(),
            });
        }
        Ok(out)
    }
}

/// The structured answers of a question tool's result message: the records
/// from `tool_result.output.answers[]` when present, else the python-ish
/// `answers: [...]` text of the message content. Returns the records, the
/// cancelled flag and the message's timestamp.
fn vibe_answer_entry(v: &serde_json::Value) -> (Vec<QaAnswerRecord>, bool, Option<String>) {
    let timestamp = v
        .get("timestamp")
        .and_then(|t| t.as_str())
        .map(String::from);
    let structured = v
        .get("tool_result")
        .and_then(|tr| tr.get("output"))
        .and_then(|o| o.get("answers"))
        .and_then(|a| a.as_array())
        .map(|answers| {
            answers
                .iter()
                .filter_map(|a| {
                    Some(QaAnswerRecord {
                        question: a.get("question").and_then(|q| q.as_str()).map(String::from),
                        answer: a.get("answer")?.as_str()?.to_string(),
                    })
                })
                .collect()
        });
    match structured {
        Some(records) => (records, false, timestamp),
        None => {
            let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
            let (records, cancelled) = parse_answers_text(content);
            (records, cancelled, timestamp)
        }
    }
}

/// The Q/A pairs of one vibe session payload, in line order: assistant
/// `ask_user_question` calls paired with their `tool_call_id`-matched tool
/// messages.
fn qa_pairs_from_vibe_lines(text: &str) -> Vec<QaPair> {
    let mut lines = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => lines.push(v),
            Err(e) => tracing::warn!("vibe rollout: skipping unparseable line {}: {e}", i + 1),
        }
    }

    // tool_call_id -> answer payload of the tool message that carries it.
    let mut answers: HashMap<String, (Vec<QaAnswerRecord>, bool, Option<String>)> = HashMap::new();
    for v in &lines {
        if v.get("role").and_then(|r| r.as_str()) != Some("tool") {
            continue;
        }
        let Some(id) = v.get("tool_call_id").and_then(|i| i.as_str()) else {
            continue;
        };
        answers.insert(id.to_string(), vibe_answer_entry(v));
    }

    let mut pairs = Vec::new();
    for v in &lines {
        if v.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }
        let assistant_ts = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .map(String::from);
        let Some(tool_calls) = v.get("tool_calls").and_then(|tc| tc.as_array()) else {
            continue;
        };
        for tc in tool_calls {
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("");
            if name != "ask_user_question" {
                continue;
            }
            let Some(id) = tc.get("id").and_then(|i| i.as_str()) else {
                continue;
            };
            let Some((records, cancelled, answer_ts)) = answers.get(id) else {
                continue;
            };
            if *cancelled || records.is_empty() {
                continue;
            }
            let args = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
                .unwrap_or("");
            let questions = parse_questions_payload(args);
            pairs.extend(pair_questions_with_answers(
                &questions,
                records,
                None,
                answer_ts.clone().or_else(|| assistant_ts.clone()),
            ));
        }
    }
    pairs
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
    if date.len() == 8
        && time.len() == 6
        && date.bytes().all(|b| b.is_ascii_digit())
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
    iso.chars()
        .filter(|c| c.is_ascii_digit())
        .take(14)
        .collect()
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
fn dedupe_vibe_sessions(summaries: Vec<SessionSummary>, identity: Vec<u64>) -> Vec<SessionSummary> {
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

/// One unified-store journal record projected into dialogue messages.
/// `core_input` records carry the user's turn (`payload.input.command`
/// content parts, timestamped by `input.determinism.time_unix_ms`);
/// `action_result` records carry the assistant's output for the action
/// (`payload.result.result.parts`: text, reasoning and tool_call parts) as
/// one assistant message. Record types the dialogue stream has no read for
/// (turn bookkeeping, receipts, projection deltas, model-input echoes like
/// `action_intent`) contribute nothing; only unparseable lines warn.
fn unified_record_messages(record: &serde_json::Value) -> Vec<RolloutMessage> {
    let rtype = record.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let payload = match record.get("payload") {
        Some(p) => p,
        None => return Vec::new(),
    };
    match rtype {
        "core_input" => {
            let content = payload
                .pointer("/input/command/content")
                .and_then(|c| c.as_array())
                .map(|parts| unified_text_parts(parts))
                .unwrap_or_default();
            if content.is_empty() {
                return Vec::new();
            }
            let timestamp = payload
                .pointer("/input/determinism/time_unix_ms")
                .and_then(|v| v.as_i64())
                .and_then(unix_ms_to_iso);
            vec![RolloutMessage {
                role: "user".to_string(),
                content,
                thinking: None,
                tool_calls_summary: Vec::new(),
                timestamp,
                injected: false,
            }]
        }
        "action_result" => {
            let parts = match payload
                .pointer("/result/result/parts")
                .and_then(|p| p.as_array())
            {
                Some(p) => p,
                None => return Vec::new(),
            };
            let mut content = String::new();
            let mut thinking = String::new();
            let mut tool_calls_summary = Vec::new();
            for part in parts {
                match part.get("type").and_then(|t| t.as_str()) {
                    Some("text") => append_text(
                        &mut content,
                        part.get("text").and_then(|t| t.as_str()).unwrap_or(""),
                    ),
                    // This environment nests the reasoning prose in a
                    // `content` array of text parts; a bare `text` field is
                    // equally valid.
                    Some("reasoning") => {
                        if let Some(inner) = part.get("content").and_then(|c| c.as_array()) {
                            append_text(&mut thinking, &unified_text_parts(inner));
                        }
                        append_text(
                            &mut thinking,
                            part.get("text").and_then(|t| t.as_str()).unwrap_or(""),
                        );
                    }
                    Some("tool_call") => {
                        let name = part.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                        let args = part
                            .get("arguments_json")
                            .and_then(|a| a.as_str())
                            .unwrap_or("{}");
                        tool_calls_summary.push(summarize_tool_call(name, args));
                    }
                    _ => {}
                }
            }
            if content.is_empty() && thinking.is_empty() && tool_calls_summary.is_empty() {
                return Vec::new();
            }
            vec![RolloutMessage {
                role: "assistant".to_string(),
                content,
                thinking: (!thinking.is_empty()).then_some(thinking),
                tool_calls_summary,
                timestamp: None,
                injected: false,
            }]
        }
        _ => Vec::new(),
    }
}

/// The `text` fields of a content-part array joined with newlines.
fn unified_text_parts(parts: &[serde_json::Value]) -> String {
    let mut out = String::new();
    for part in parts {
        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
            append_text(&mut out, text);
        }
    }
    out
}

/// Append one text fragment, newline-separating fragments.
fn append_text(into: &mut String, text: &str) {
    if text.is_empty() {
        return;
    }
    if !into.is_empty() {
        into.push('\n');
    }
    into.push_str(text);
}

/// The write stamp of a unified store: the newest modification among its
/// journal segments.
fn unified_payload_mtime(dir: &Path) -> Option<std::time::SystemTime> {
    let mut latest: Option<std::time::SystemTime> = None;
    if let Ok(entries) = std::fs::read_dir(dir.join("journal")) {
        for entry in entries.flatten() {
            if let Ok(m) = entry.metadata().and_then(|m| m.modified()) {
                latest = Some(latest.map_or(m, |l| l.max(m)));
            }
        }
    }
    latest
}

/// Unix milliseconds → ISO8601 UTC (`YYYY-MM-DDTHH:MM:SSZ`), matching the
/// rollout convention the legacy layout carries. Inverse-free civil-date
/// math (Hinnant); no calendar dependency.
fn unix_ms_to_iso(ms: i64) -> Option<String> {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let (h, mi, s) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    Some(format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z"))
}

/// Unified-store raw entries (for `extract_by_type`): the replayed messages
/// re-serialized, with the legacy compaction window and injected filter.
fn unified_entries(
    messages: Vec<RolloutMessage>,
    full: bool,
    include_injected: bool,
) -> Vec<super::RolloutEntry> {
    let start = if full {
        0
    } else {
        messages
            .iter()
            .rposition(|m| m.content.contains("context compaction"))
            .unwrap_or(0)
    };
    let mut out = Vec::new();
    for m in messages.into_iter().skip(start) {
        if !include_injected && m.injected {
            continue;
        }
        if !matches!(m.role.as_str(), "user" | "assistant" | "tool") {
            continue;
        }
        let record = match serde_json::to_value(&m) {
            Ok(v) => v,
            Err(_) => continue,
        };
        out.push(super::RolloutEntry {
            index: out.len(),
            entry_type: m.role.clone(),
            timestamp: m.timestamp,
            record,
        });
    }
    out
}
