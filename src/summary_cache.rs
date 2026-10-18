//! #36: the summary range ledger — a per-session record of what has already
//! been summarised, and the cached parts themselves.
//!
//! A summary is just a summary of the slice it names. The ledger names that
//! slice: one CSV row per summarised part, `from_ts,to_ts,part_file`. On a
//! re-run the session's current max ts decides: unchanged max → compose the
//! cached parts and make no LLM call; moved max → summarise only the delta
//! (previous max → new max), store it as a new part, append the ledger.
//!
//! Files live in the adapter's shadow root beside the store, under the same
//! rules as the profile cache and the tantivy indexes: disposable, never
//! inside the session store, MUST NOT be committed.

use std::path::{Path, PathBuf};

/// ISO-8601 timestamps sort lexicographically, so string comparison is a
/// total order over the session's messages.
pub fn bounds(messages: &[crate::RolloutMessage]) -> Option<(String, String)> {
    let ts: Vec<&str> = messages.iter().filter_map(|m| m.timestamp.as_deref()).collect();
    let first = ts.first()?.to_string();
    let last = ts.last()?.to_string();
    Some((first, last))
}

/// Messages strictly after `to_ts` — the not-yet-summarised delta.
pub fn delta(messages: &[crate::RolloutMessage], to_ts: &str) -> Vec<crate::RolloutMessage> {
    messages
        .iter()
        .filter(|m| m.timestamp.as_deref().is_some_and(|t| t > to_ts))
        .cloned()
        .collect()
}

pub fn ledger_path(shadow_root: &Path, session_id: &str) -> PathBuf {
    shadow_root.join(format!("tr_{session_id}_summary.csv"))
}

fn parts_dir(shadow_root: &Path, session_id: &str) -> PathBuf {
    shadow_root.join(format!("tr_{session_id}_summary_parts"))
}

/// One summarised part: the range it covers and the file holding its text.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub from_ts: String,
    pub to_ts: String,
    pub part_file: PathBuf,
}

/// Read the ledger; a missing or malformed file is "nothing summarised yet",
/// never an error — the cache is disposable by design.
pub fn read_ledger(path: &Path) -> Vec<Part> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let mut f = l.split(',');
            let from_ts = f.next()?.to_string();
            let to_ts = f.next()?.to_string();
            let part_file = PathBuf::from(f.next()?);
            Some(Part { from_ts, to_ts, part_file })
        })
        .collect()
}

/// Append one summarised part: write the part file, then the ledger row.
/// The ledger write happens last so a crash mid-way leaves a part file
/// without a ledger row — an orphan, never a lie.
pub fn append_part(
    shadow_root: &Path,
    session_id: &str,
    from_ts: &str,
    to_ts: &str,
    summary: &str,
) -> std::io::Result<Part> {
    let dir = parts_dir(shadow_root, session_id);
    std::fs::create_dir_all(&dir)?;
    let n = read_ledger(&ledger_path(shadow_root, session_id)).len() + 1;
    let file = dir.join(format!("part_{n:03}_{}.md", to_ts.replace(':', "")));
    std::fs::write(&file, summary)?;
    let part = Part { from_ts: from_ts.to_string(), to_ts: to_ts.to_string(), part_file: file };
    use std::io::Write;
    let mut ledger = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ledger_path(shadow_root, session_id))?;
    writeln!(ledger, "{from_ts},{to_ts},{}", part.part_file.display())?;
    Ok(part)
}

/// Compose the cached parts, oldest first, each named by the range it covers.
pub fn compose(parts: &[Part]) -> String {
    parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let text = std::fs::read_to_string(&p.part_file).unwrap_or_default();
            format!("### part {}: {} → {}\n\n{}", i + 1, p.from_ts, p.to_ts, text.trim())
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The summarise_current_session flow, MCP and CLI: read the session, consult
/// the ledger, and either return the cached composition (no LLM call) or
/// summarise only the delta and store it as a new part. `use_cache = false`
/// summarises the whole session and touches no files.
///
/// A cache hit needs no vendor at all — a vendor-free build can still serve
/// what was already summarised. Only a cache miss constructs the provider, so
/// a vendor-free build errors there with the usual rebuild message.
pub async fn summarise_session(
    adapter: &dyn crate::RolloutAdapter,
    session_id: &str,
    use_cache: bool,
    requested_provider: Option<&str>,
) -> Result<String, String> {
    let messages = adapter
        .read_session_mmap(session_id)
        .map_err(|e| e.to_string())?;
    if messages.is_empty() {
        return Err("No messages found in session".to_string());
    }

    let bounds = bounds(&messages);
    let (first_ts, last_ts) = match &bounds {
        Some(b) => b.clone(),
        None => {
            // Untimestamped sessions cannot key a ledger: summarise whole,
            // cache nothing.
            return summarise_whole(requested_provider, messages).await;
        }
    };

    if !use_cache {
        return summarise_whole(requested_provider, messages).await;
    }

    let shadow_root = adapter.shadow_index_root();
    let ledger = read_ledger(&ledger_path(&shadow_root, session_id));

    if let Some(last) = ledger.last()
        && last.to_ts == last_ts
    {
        let mut out = compose(&ledger);
        out.push_str(&format!(
            "\n\n---\nsummary cache: {} part(s), {first_ts} → {last_ts} (cached; nothing new since the last summarise)",
            ledger.len()
        ));
        return Ok(out);
    }

    let from_ts = ledger
        .last()
        .map(|p| p.to_ts.clone())
        .unwrap_or_else(|| first_ts.clone());
    let delta_msgs = delta(&messages, &from_ts);
    // A moved max with an empty delta (timestamped bounds, untimestamped tail)
    // falls back to the whole session rather than summarising nothing.
    let slice = if delta_msgs.is_empty() { &messages } else { &delta_msgs };

    let text = llm_summary(requested_provider, slice).await?;
    let part_from = if ledger.is_empty() { first_ts.clone() } else { from_ts.clone() };
    append_part(&shadow_root, session_id, &part_from, &last_ts, &text)
        .map_err(|e| format!("summary cache write failed: {e}"))?;

    let ledger = read_ledger(&ledger_path(&shadow_root, session_id));
    let mut out = compose(&ledger);
    out.push_str(&format!(
        "\n\n---\nsummary cache: {} part(s), {first_ts} → {last_ts}",
        ledger.len()
    ));
    Ok(out)
}

async fn llm_summary(
    requested_provider: Option<&str>,
    messages: &[crate::RolloutMessage],
) -> Result<String, String> {
    let prompt = crate::build_structured_prompt(messages);
    let provider = crate::provider_for(requested_provider)
        .map_err(|e| format!("{e:#}"))?;
    provider
        .compact(crate::SYSTEM_PROMPT, &prompt)
        .await
        .map_err(|e| format!("Mercury API error: {e}"))
}

async fn summarise_whole(
    requested_provider: Option<&str>,
    messages: Vec<crate::RolloutMessage>,
) -> Result<String, String> {
    llm_summary(requested_provider, &messages).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RolloutMessage;

    fn msg(ts: Option<&str>, content: &str) -> RolloutMessage {
        RolloutMessage {
            role: "user".into(),
            content: content.into(),
            thinking: None,
            tool_calls_summary: vec![],
            timestamp: ts.map(str::to_string),
            injected: false,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tr_summary_cache_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn bounds_take_the_first_and_last_timestamped_entries() {
        let msgs = vec![
            msg(Some("2026-10-18T10:00:00Z"), "a"),
            msg(None, "no ts is skipped for bounds"),
            msg(Some("2026-10-18T12:00:00Z"), "c"),
        ];
        let b = bounds(&msgs).unwrap();
        assert_eq!(b, ("2026-10-18T10:00:00Z".into(), "2026-10-18T12:00:00Z".into()));
    }

    #[test]
    fn a_session_without_timestamps_has_no_bounds() {
        assert_eq!(bounds(&[msg(None, "x")]), None);
    }

    #[test]
    fn delta_takes_only_entries_after_the_covered_max() {
        let msgs = vec![
            msg(Some("2026-10-18T10:00:00Z"), "covered"),
            msg(Some("2026-10-18T11:00:00Z"), "covered too"),
            msg(Some("2026-10-18T12:00:00Z"), "new"),
        ];
        let d = delta(&msgs, "2026-10-18T11:00:00Z");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].content, "new");
    }

    #[test]
    fn the_ledger_round_trips_and_names_what_is_covered() {
        let dir = scratch("ledger");
        let path = ledger_path(&dir, "ses_x");
        assert!(read_ledger(&path).is_empty(), "missing ledger is empty");
        let p1 = append_part(&dir, "ses_x", "2026-10-18T10:00:00Z", "2026-10-18T11:00:00Z", "first summary").unwrap();
        let p2 = append_part(&dir, "ses_x", "2026-10-18T11:00:00Z", "2026-10-18T12:00:00Z", "delta summary").unwrap();
        let ledger = read_ledger(&path);
        assert_eq!(ledger.len(), 2);
        assert_eq!(ledger[1].to_ts, "2026-10-18T12:00:00Z");
        assert_eq!(p1.part_file, ledger[0].part_file);
        let composed = compose(&ledger);
        assert!(composed.contains("### part 1: 2026-10-18T10:00:00Z → 2026-10-18T11:00:00Z"));
        assert!(composed.contains("first summary"));
        assert!(composed.contains("delta summary"));
        assert_eq!(p2.part_file.file_name().unwrap(), "part_002_2026-10-18T120000Z.md");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
