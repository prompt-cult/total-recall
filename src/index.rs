use std::path::PathBuf;

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::document::{CompactDocValue, ReferenceValue, ReferenceValueLeaf, Value as _};
use tantivy::schema::{NumericOptions, STORED, Schema, TEXT, TantivyDocument, TextOptions};
use tantivy::{Index, IndexWriter};

use crate::rollout::{RolloutAdapter, SessionSummary};

pub struct IndexStats {
    pub session_id: String,
    pub doc_count: u64,
}

pub(crate) fn session_index_dir(adapter: &dyn RolloutAdapter, session_id: &str) -> PathBuf {
    adapter.shadow_index_root().join(session_id)
}

fn meta_path(adapter: &dyn RolloutAdapter, session_id: &str) -> PathBuf {
    session_index_dir(adapter, session_id).join("total-recall-meta.json")
}

pub fn index_exists(adapter: &dyn RolloutAdapter, session_id: &str) -> bool {
    meta_path(adapter, session_id).is_file()
}

/// Whether an index needs rebuilding for its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReindexDecision {
    /// The index exists and was built from the payload's current mtime.
    Fresh,
    /// No index, unreadable meta, or the payload changed since the build.
    Stale,
}

/// Mtime gate for file-backed harnesses: the meta records the
/// payload mtime it indexed (`source_mtime`); a build is fresh only while
/// that matches the payload's mtime now. A harness that cannot report a
/// payload mtime (database-backed) is always stale — the gate refuses to
/// claim freshness it cannot verify. No index at all is stale.
pub fn index_session_fresh(
    adapter: &dyn RolloutAdapter,
    session_id: &str,
) -> Result<ReindexDecision, String> {
    if !index_exists(adapter, session_id) {
        return Ok(ReindexDecision::Stale);
    }
    let meta = std::fs::read_to_string(meta_path(adapter, session_id))
        .map_err(|e| format!("cannot read index meta: {e}"))?;
    let meta: serde_json::Value =
        serde_json::from_str(&meta).map_err(|e| format!("cannot parse index meta: {e}"))?;
    let source_mtime = meta.get("source_mtime").and_then(|m| m.as_u64());
    let payload_mtime = adapter
        .list_sessions()
        .into_iter()
        .find(|s| s.session_id == *session_id)
        .and_then(|s| s.mtime)
        .and_then(|m| m.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    match (source_mtime, payload_mtime) {
        (Some(built), Some(now)) if built == now => Ok(ReindexDecision::Fresh),
        _ => Ok(ReindexDecision::Stale),
    }
}

pub fn annotate_sessions(sessions: &mut [SessionSummary], adapter: &dyn RolloutAdapter) {
    for summary in sessions.iter_mut() {
        summary.has_tantivy_index = index_exists(adapter, &summary.session_id);
    }
}

/// Resolve a (possibly partial) session id against the adapter's session list,
/// most recent match wins. Empty id = most recent session. Returns the id and
/// the unmatched partials.
pub fn select_sessions(
    adapter: &dyn RolloutAdapter,
    sessions: &[String],
    hours_back: u64,
    directory: Option<&str>,
) -> (Vec<SessionSummary>, Vec<String>) {
    let all = adapter.list_sessions();
    let mut selected: Vec<SessionSummary> = Vec::new();
    let mut unmatched: Vec<String> = Vec::new();

    if sessions.is_empty() {
        let cutoff = if hours_back == 0 {
            None
        } else {
            Some(crate::rollout::opencode::iso_cutoff(hours_back))
        };
        for summary in all {
            if let Some(cut) = &cutoff
                && crate::rollout::is_iso8601(&summary.end_time)
                && summary.end_time.as_str() < cut.as_str()
            {
                continue;
            }
            if let Some(dir) = directory.filter(|d| !d.is_empty())
                && !summary.directory.as_deref().is_none_or(|d| d.contains(dir))
            {
                continue;
            }
            selected.push(summary);
        }
    } else {
        for partial in sessions {
            if let Some(found) = all.iter().find(|s| s.session_id.contains(partial.as_str())) {
                if !selected.iter().any(|s| s.session_id == found.session_id) {
                    selected.push(found.clone());
                }
            } else {
                unmatched.push(partial.clone());
            }
        }
    }

    (selected, unmatched)
}

fn build_schema() -> Schema {
    let mut builder = Schema::builder();
    let stored_text = TextOptions::default().set_stored();
    builder.add_text_field("session_id", stored_text.clone());
    builder.add_text_field("role", stored_text.clone());
    builder.add_text_field("timestamp", stored_text.clone());
    builder.add_text_field("content", TEXT | STORED);
    builder.add_text_field("thinking", TEXT | STORED);
    builder.add_u64_field("seq", NumericOptions::default().set_stored());
    builder.build()
}

pub fn index_session(adapter: &dyn RolloutAdapter, session_id: &str) -> Result<IndexStats, String> {
    let resolved = if session_id.is_empty() {
        adapter
            .list_sessions()
            .first()
            .map(|s| s.session_id.clone())
            .ok_or_else(|| "no sessions found".to_string())?
    } else {
        adapter
            .list_sessions()
            .into_iter()
            .find(|s| s.session_id == session_id)
            .or_else(|| {
                adapter
                    .list_sessions()
                    .into_iter()
                    .find(|s| s.session_id.contains(session_id))
            })
            .map(|s| s.session_id)
            .unwrap_or_else(|| session_id.to_string())
    };

    let messages = adapter
        .read_session_mmap(&resolved)
        .map_err(|e| format!("cannot index session: {e}"))?;
    let payload_mtime = adapter
        .list_sessions()
        .into_iter()
        .find(|s| s.session_id == resolved)
        .and_then(|s| s.mtime)
        .and_then(|m| m.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    let dir = session_index_dir(adapter, &resolved);
    // Build to a side path; the previous index survives any failure and is
    // swapped away only after the new one committed.
    let building = dir.with_extension("building");
    if building.exists() {
        std::fs::remove_dir_all(&building).map_err(|e| {
            format!(
                "cannot clear stale building dir {}: {}",
                building.display(),
                e
            )
        })?;
    }
    std::fs::create_dir_all(&building)
        .map_err(|e| format!("cannot create index dir {}: {}", building.display(), e))?;

    let doc_count = {
        let schema = build_schema();
        let index = Index::create_in_dir(&building, schema.clone())
            .map_err(|e| format!("cannot create index in {}: {}", dir.display(), e))?;
        let mut writer: IndexWriter = index
            .writer(50_000_000)
            .map_err(|e| format!("cannot create index writer: {}", e))?;

        let session_id_field = schema.get_field("session_id").unwrap();
        let role_field = schema.get_field("role").unwrap();
        let timestamp_field = schema.get_field("timestamp").unwrap();
        let content_field = schema.get_field("content").unwrap();
        let thinking_field = schema.get_field("thinking").unwrap();
        let seq_field = schema.get_field("seq").unwrap();

        let mut doc_count = 0u64;
        for (seq, msg) in messages.iter().enumerate() {
            if msg.content.is_empty() && msg.thinking.as_deref().is_none_or(|t| t.is_empty()) {
                continue;
            }
            let mut doc = TantivyDocument::default();
            doc.add_text(session_id_field, &resolved);
            doc.add_text(role_field, &msg.role);
            if let Some(ts) = &msg.timestamp {
                doc.add_text(timestamp_field, ts);
            }
            if !msg.content.is_empty() {
                doc.add_text(content_field, &msg.content);
            }
            if let Some(thinking) = &msg.thinking
                && !thinking.is_empty()
            {
                doc.add_text(thinking_field, thinking);
            }
            doc.add_u64(seq_field, seq as u64);
            writer
                .add_document(doc)
                .map_err(|e| format!("failed to add document: {}", e))?;
            doc_count += 1;
        }
        let mut commit_error = None;
        for attempt in 0..5 {
            match writer.commit() {
                Ok(_) => {
                    commit_error = None;
                    break;
                }
                Err(e)
                    if cfg!(windows)
                        && e.to_string().contains("Access is denied")
                        && attempt < 4 =>
                {
                    commit_error = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => {
                    commit_error = Some(e);
                    break;
                }
            }
        }
        if let Some(e) = commit_error {
            return Err(format!("index commit failed: {e}"));
        }
        // Every tantivy handle to `building` must be released before the
        // swap: Windows refuses to rename a directory anyone still holds.
        // Every tantivy handle to `building` must be released before the
        // swap: Windows refuses to rename a directory anyone still holds.
        drop(writer);
        drop(index);
        doc_count
    };

    let built_at = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let meta = serde_json::json!({
        "session_id": resolved,
        "doc_count": doc_count,
        "built_at": built_at,
        "source_mtime": payload_mtime,
    });
    std::fs::write(
        building.join("total-recall-meta.json"),
        serde_json::to_string_pretty(&meta).unwrap_or_default(),
    )
    .map_err(|e| format!("cannot write total-recall-meta.json: {}", e))?;
    // The swap: discard the previous index only now that the replacement is
    // on disk with its meta. Windows can hold a directory rename briefly
    // after its last handle closed (antivirus, indexer), so the rename
    // retries a few times before failing.
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| format!("cannot clear index dir {}: {}", dir.display(), e))?;
    }
    let mut swap_err = None;
    for attempt in 0..5 {
        match std::fs::rename(&building, &dir) {
            Ok(()) => {
                swap_err = None;
                break;
            }
            Err(e) if attempt < 4 => {
                swap_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                swap_err = Some(e);
            }
        }
    }
    if let Some(e) = swap_err {
        return Err(format!("cannot swap index dir {}: {}", dir.display(), e));
    }

    Ok(IndexStats {
        session_id: resolved,
        doc_count,
    })
}

/// One search hit: the document's score, its session and message context,
/// and the matched-text fragment. `in_thinking` is true when the term
/// matched only a message's thinking content.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Hit {
    pub score: f32,
    pub session_id: String,
    pub timestamp: String,
    pub role: String,
    pub fragment: String,
    pub in_thinking: bool,
}

/// The structured search outcome behind `do_android_dream_of_electric_sheep`:
/// relevance-ordered hits plus the counts the record stream's meta record
/// carries. The prose rendering died with the JSONL cutover — the MCP layer
/// streams these as records; the CLI renders its own human-facing table.
pub struct SearchOutcome {
    pub query: String,
    pub sessions_searched: usize,
    pub sessions_with_hits: usize,
    pub not_indexed: Vec<String>,
    pub unmatched: Vec<String>,
    pub corrupt: Vec<String>,
    pub hits: Vec<Hit>,
}

fn query_tokens(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

fn text_of(value: Option<CompactDocValue>) -> String {
    let value = match value {
        Some(v) => v,
        None => return String::new(),
    };
    match value.as_value() {
        ReferenceValue::Leaf(ReferenceValueLeaf::Str(s)) => s.to_string(),
        _ => String::new(),
    }
}

fn fragment_for(text: &str, tokens: &[String], window: usize) -> String {
    let lower = text.to_lowercase();
    let mut pos = 0usize;
    for token in tokens {
        if let Some(p) = lower.find(token) {
            pos = p;
            break;
        }
    }
    let mut start = pos.saturating_sub(window / 4);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    let mut end = (pos + window).min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    if start > end {
        start = end;
    }
    text[start..end].to_string()
}

pub fn search(
    adapter: &dyn RolloutAdapter,
    sessions: &[String],
    query: &str,
    hours_back: u64,
    directory: Option<&str>,
) -> Result<SearchOutcome, String> {
    if query.trim().is_empty() {
        return Err(
            "do-android-dream-of-electric-sheep requires a tantivy query in `query`".to_string(),
        );
    }

    let (selected, unmatched) = select_sessions(adapter, sessions, hours_back, directory);

    let schema = build_schema();
    let content_field = schema.get_field("content").unwrap();
    let thinking_field = schema.get_field("thinking").unwrap();
    let role_field = schema.get_field("role").unwrap();
    let timestamp_field = schema.get_field("timestamp").unwrap();

    let tokens = query_tokens(query);
    let mut hits: Vec<Hit> = Vec::new();
    let mut not_indexed: Vec<String> = Vec::new();
    let mut corrupt: Vec<String> = Vec::new();
    let mut sessions_with_hits = 0usize;

    for summary in &selected {
        // File-backed auto-reindex: a stale index is rebuilt in
        // the search pass so a hit is never silently served from old
        // content; database-backed payloads cannot verify staleness and
        // keep the explicit pass (reported not_indexed).
        if index_session_fresh(adapter, &summary.session_id)? == ReindexDecision::Stale {
            if summary.backend_kind == Some(crate::rollout::BackendKind::File) {
                index_session(adapter, &summary.session_id)?;
            } else if !index_exists(adapter, &summary.session_id) {
                not_indexed.push(summary.session_id.clone());
                continue;
            }
        }
        let dir = session_index_dir(adapter, &summary.session_id);
        let result = (|| -> Result<Vec<Hit>, String> {
            let index = Index::open_in_dir(&dir)
                .map_err(|e| format!("cannot open index at {}: {}", dir.display(), e))?;
            let parser = QueryParser::for_index(&index, vec![content_field, thinking_field]);
            let parsed = parser
                .parse_query(query)
                .map_err(|e| format!("invalid query '{}': {}", query, e))?;
            let reader = index
                .reader()
                .map_err(|e| format!("cannot open index reader: {}", e))?;
            let searcher = reader.searcher();
            let top: Vec<(f32, tantivy::DocAddress)> = searcher
                .search(&parsed, &TopDocs::with_limit(10).order_by_score())
                .map_err(|e| format!("search failed: {}", e))?;

            let mut session_hits = Vec::new();
            for (score, addr) in top {
                let doc: TantivyDocument = searcher
                    .doc::<TantivyDocument>(addr)
                    .map_err(|e| format!("cannot retrieve document: {}", e))?;
                let content = text_of(doc.get_first(content_field));
                let thinking = text_of(doc.get_first(thinking_field));
                let in_thinking = {
                    let content_hit = tokens.iter().any(|t| content.to_lowercase().contains(t));
                    let thinking_hit = tokens.iter().any(|t| thinking.to_lowercase().contains(t));
                    !content_hit && thinking_hit
                };
                let (text, marked) = if in_thinking {
                    (thinking.clone(), true)
                } else {
                    (content.clone(), false)
                };
                let role = text_of(doc.get_first(role_field));
                let timestamp = text_of(doc.get_first(timestamp_field));
                session_hits.push(Hit {
                    score,
                    session_id: summary.session_id.clone(),
                    timestamp,
                    role,
                    fragment: fragment_for(&text, &tokens, 200),
                    in_thinking: marked,
                });
            }
            Ok(session_hits)
        })();
        match result {
            Ok(session_hits) => {
                if !session_hits.is_empty() {
                    sessions_with_hits += 1;
                }
                hits.extend(session_hits);
            }
            Err(e) => corrupt.push(e),
        }
    }

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(SearchOutcome {
        query: query.to_string(),
        sessions_searched: selected.len(),
        sessions_with_hits,
        not_indexed,
        unmatched,
        corrupt,
        hits,
    })
}
