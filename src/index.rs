use std::path::PathBuf;

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::document::{
    CompactDocValue, ReferenceValue, ReferenceValueLeaf, Value as _,
};
use tantivy::schema::{NumericOptions, Schema, TantivyDocument, TextOptions, STORED, TEXT};
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
                && !summary
                    .directory
                    .as_deref()
                    .is_none_or(|d| d.contains(dir))
            {
                continue;
            }
            selected.push(summary);
        }
    } else {
        for partial in sessions {
            if let Some(found) = all
                .iter()
                .find(|s| s.session_id.contains(partial.as_str()))
            {
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

pub fn index_session(
    adapter: &dyn RolloutAdapter,
    session_id: &str,
) -> Result<IndexStats, String> {
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
            .find(|s| s.session_id.contains(session_id))
            .map(|s| s.session_id)
            .unwrap_or_else(|| session_id.to_string())
    };

    let messages = adapter.read_session_mmap(&resolved);
    let dir = session_index_dir(adapter, &resolved);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| {
            format!(
                "cannot clear existing index dir {}: {}",
                dir.display(),
                e
            )
        })?;
    }
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create index dir {}: {}", dir.display(), e))?;

    let schema = build_schema();
    let index = Index::create_in_dir(&dir, schema.clone())
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
    writer
        .commit()
        .map_err(|e| format!("index commit failed: {}", e))?;

    let built_at = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let meta = serde_json::json!({
        "session_id": resolved,
        "doc_count": doc_count,
        "built_at": built_at,
    });
    std::fs::write(
        meta_path(adapter, &resolved),
        serde_json::to_string_pretty(&meta).unwrap_or_default(),
    )
    .map_err(|e| format!("cannot write total-recall-meta.json: {}", e))?;

    Ok(IndexStats {
        session_id: resolved,
        doc_count,
    })
}

struct Hit {
    score: f32,
    session_id: String,
    timestamp: String,
    role: String,
    fragment: String,
    in_thinking: bool,
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
) -> Result<String, String> {
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
    let mut unindexed = 0usize;
    let mut corrupt: Vec<String> = Vec::new();
    let mut sessions_with_hits = 0usize;

    for summary in &selected {
        if !index_exists(adapter, &summary.session_id) {
            unindexed += 1;
            continue;
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
                    let content_hit = tokens
                        .iter()
                        .any(|t| content.to_lowercase().contains(t));
                    let thinking_hit = tokens
                        .iter()
                        .any(|t| thinking.to_lowercase().contains(t));
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

    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    let mut out = format!(
        "# do-android-dream-of-electric-sheep — query: {}\n",
        query
    );
    let mut header = format!(
        "{} sessions searched, {} with hits, {} not indexed (run index first)",
        selected.len(),
        sessions_with_hits,
        unindexed
    );
    if !unmatched.is_empty() {
        header += &format!(
            ", {} unmatched session ids: {}",
            unmatched.len(),
            unmatched.join(", ")
        );
    }
    for line in &corrupt {
        header += &format!("\ncorrupt index: {}", line);
    }
    out += &header;
    out.push('\n');

    for hit in &hits {
        let role = if hit.in_thinking {
            format!("{} (thinking)", hit.role.to_uppercase())
        } else {
            hit.role.to_uppercase()
        };
        out += &format!(
            "{} | {:.4} | {} | {} | {}\n",
            hit.session_id, hit.score, hit.timestamp, role, hit.fragment
        );
    }

    Ok(out)
}
