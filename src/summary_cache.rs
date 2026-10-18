//! #36, re-carried by #39: the summary range ledger — a per-session record of
//! what has already been summarised, and the cached parts themselves.
//!
//! A summary is just a summary of the slice it names. The ledger names that
//! slice as a **block**: one CSV row per summarised part, 11 columns,
//! `from_id,from_ts,to_id,to_ts,user,assistant,tool,thinking,chars,
//! tokens_est,part_file`. The ids are 1-based message sequence numbers in
//! the session stream — computed by enumerating the messages at read time,
//! so no new field is needed on the message type and every harness works.
//!
//! There is no backward compatibility with the v0.15.0 3-column ledger
//! (alpha): a malformed or short row invalidates the ledger — it is treated
//! as empty and the file is dropped, and any orphaned part files may remain
//! and may be clobbered later.
//!
//! Files live in the adapter's shadow root beside the store, under the same
//! rules as the profile cache and the tantivy indexes: disposable, never
//! inside the session store, MUST NOT be committed.

use std::path::{Path, PathBuf};

/// One summarised block: the id range and timestamps it covers, the shape of
/// the slice it summarised, and the file holding its text.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Block {
    pub from_id: u64,
    pub from_ts: String,
    pub to_id: u64,
    pub to_ts: String,
    pub user: u64,
    pub assistant: u64,
    pub tool: u64,
    pub thinking: u64,
    pub chars: u64,
    pub tokens_est: u64,
    pub part_file: PathBuf,
}

/// The listing marker for one session's cache: the id and timestamp the
/// blocks cover through. `None` when nothing is cached.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CacheThrough {
    pub through_id: u64,
    pub through_ts: String,
}

/// The not-yet-summarised tail: the first uncovered id and the shape of the
/// slice a summarise from there would cover.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TailInfo {
    pub from_id: u64,
    pub from_ts: Option<String>,
    pub count: usize,
    pub user: u64,
    pub assistant: u64,
    pub tool: u64,
    pub thinking: u64,
    pub chars: u64,
    pub tokens_est: u64,
}

/// The `summary_cache` introspection payload: the cached blocks and the tail.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CacheStatus {
    pub session_id: String,
    pub blocks: Vec<Block>,
    pub tail: Option<TailInfo>,
}

/// ISO-8601 timestamps sort lexicographically, so string comparison is a
/// total order over the session's messages.
pub fn bounds(messages: &[crate::RolloutMessage]) -> Option<(String, String)> {
    let ts: Vec<&str> = messages
        .iter()
        .filter_map(|m| m.timestamp.as_deref())
        .collect();
    let first = ts.first()?.to_string();
    let last = ts.last()?.to_string();
    Some((first, last))
}

/// Messages strictly after `to_ts` — the not-yet-summarised delta. Kept for
/// the ts-based slice tests; the ledger itself keys on ids now.
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

/// The first id a summarise may start at: one past the last block's `to_id`,
/// or 1 with no blocks.
pub fn expected_from_id(blocks: &[Block]) -> u64 {
    blocks.last().map(|b| b.to_id + 1).unwrap_or(1)
}

/// The per-slice counts shared by `append_block` and the tail: role counts,
/// a thinking count, and the size the summarise reads (characters, and the
/// ~4 chars/token estimate the request guardrails use).
fn count_slice(slice: &[crate::RolloutMessage]) -> (u64, u64, u64, u64, u64, u64) {
    let mut user = 0u64;
    let mut assistant = 0u64;
    let mut tool = 0u64;
    let mut thinking = 0u64;
    let mut chars = 0u64;
    let mut tokens = 0u64;
    for m in slice {
        match m.role.as_str() {
            "user" => user += 1,
            "assistant" => assistant += 1,
            "tool" => tool += 1,
            _ => {}
        }
        if let Some(t) = &m.thinking {
            if !t.is_empty() {
                thinking += 1;
            }
            chars += t.chars().count() as u64;
            tokens += crate::estimate_tokens(t) as u64;
        }
        chars += m.content.chars().count() as u64;
        tokens += crate::estimate_tokens(&m.content) as u64;
    }
    (user, assistant, tool, thinking, chars, tokens)
}

/// One ledger row, exactly the 11 columns the parser reads back.
fn row_line(block: &Block) -> String {
    format!(
        "{},{},{},{},{},{},{},{},{},{},{}",
        block.from_id,
        block.from_ts,
        block.to_id,
        block.to_ts,
        block.user,
        block.assistant,
        block.tool,
        block.thinking,
        block.chars,
        block.tokens_est,
        block.part_file.display()
    )
}

fn parse_row(line: &str) -> Option<Block> {
    let mut f = line.split(',');
    let from_id = f.next()?.parse().ok()?;
    let from_ts = f.next()?.to_string();
    let to_id = f.next()?.parse().ok()?;
    let to_ts = f.next()?.to_string();
    let user = f.next()?.parse().ok()?;
    let assistant = f.next()?.parse().ok()?;
    let tool = f.next()?.parse().ok()?;
    let thinking = f.next()?.parse().ok()?;
    let chars = f.next()?.parse().ok()?;
    let tokens_est = f.next()?.parse().ok()?;
    let part_file = PathBuf::from(f.next()?);
    if f.next().is_some() || to_id < from_id {
        return None;
    }
    Some(Block {
        from_id,
        from_ts,
        to_id,
        to_ts,
        user,
        assistant,
        tool,
        thinking,
        chars,
        tokens_est,
        part_file,
    })
}

/// Read the ledger. A missing file is "nothing summarised yet"; a malformed
/// or short (v0.15.0-era) row invalidates the whole ledger — it is treated
/// as empty and the file is dropped, because the cache is disposable by
/// design and a half-understood ledger is a lie. Orphaned part files may
/// remain and may be clobbered later.
pub fn read_ledger(path: &Path) -> Vec<Block> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut blocks = Vec::new();
    for line in body.lines().filter(|l| !l.trim().is_empty()) {
        let Some(block) = parse_row(line) else {
            let _ = std::fs::remove_file(path);
            return Vec::new();
        };
        blocks.push(block);
    }
    blocks
}

/// Append one summarised block covering `from_id..=to_id` (the slice's last
/// message's id): write the part file, then the ledger row. The ledger write
/// happens last so a crash mid-way leaves a part file without a ledger row —
/// an orphan, never a lie.
pub fn append_block(
    shadow_root: &Path,
    session_id: &str,
    from_id: u64,
    slice: &[crate::RolloutMessage],
    summary: &str,
) -> std::io::Result<Block> {
    let dir = parts_dir(shadow_root, session_id);
    std::fs::create_dir_all(&dir)?;
    let Some((from_ts, to_ts)) = bounds(slice) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "block slice has no timestamps to key the ledger row",
        ));
    };
    if slice.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "block slice is empty",
        ));
    }
    let to_id = from_id + slice.len() as u64 - 1;
    let file = dir.join(format!("part_{from_id:03}_{to_id:03}.md"));
    std::fs::write(&file, summary)?;
    let (user, assistant, tool, thinking, chars, tokens_est) = count_slice(slice);
    let block = Block {
        from_id,
        from_ts,
        to_id,
        to_ts,
        user,
        assistant,
        tool,
        thinking,
        chars,
        tokens_est,
        part_file: file,
    };
    use std::io::Write;
    let mut ledger = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ledger_path(shadow_root, session_id))?;
    writeln!(ledger, "{}", row_line(&block))?;
    Ok(block)
}

/// Delete the newest block: rewrite the ledger without its last row (dropping
/// the file when no rows remain) and remove that row's part file. A part file
/// that is already gone is gone as far as rollback cares.
pub fn delete_latest(shadow_root: &Path, session_id: &str) -> std::io::Result<()> {
    let path = ledger_path(shadow_root, session_id);
    let blocks = read_ledger(&path);
    let Some(last) = blocks.last() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no cached blocks",
        ));
    };
    let kept = &blocks[..blocks.len() - 1];
    if kept.is_empty() {
        std::fs::remove_file(&path)?;
    } else {
        let mut body = String::new();
        for b in kept {
            body.push_str(&row_line(b));
            body.push('\n');
        }
        std::fs::write(&path, body)?;
    }
    match std::fs::remove_file(&last.part_file) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    Ok(())
}

/// Compose the cached blocks, oldest first, each named by its ordinal and
/// the id range and timestamps it covers.
pub fn compose(blocks: &[Block]) -> String {
    blocks
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let text = std::fs::read_to_string(&b.part_file).unwrap_or_default();
            format!(
                "### part {}: {} → {} (ids {}..{})\n\n{}",
                i + 1,
                b.from_ts,
                b.to_ts,
                b.from_id,
                b.to_id,
                text.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The `summary_cache` introspection: the ledger's blocks and the
/// not-yet-summarised tail, or `tail: None` when every message is covered.
/// Read-only — no LLM provider, no writes.
pub fn status(
    shadow_root: &Path,
    session_id: &str,
    messages: &[crate::RolloutMessage],
) -> CacheStatus {
    let blocks = read_ledger(&ledger_path(shadow_root, session_id));
    let expected = expected_from_id(&blocks);
    let tail = if expected <= messages.len() as u64 {
        let slice = &messages[(expected - 1) as usize..];
        let (user, assistant, tool, thinking, chars, tokens_est) = count_slice(slice);
        Some(TailInfo {
            from_id: expected,
            from_ts: bounds(slice).map(|(first, _)| first),
            count: slice.len(),
            user,
            assistant,
            tool,
            thinking,
            chars,
            tokens_est,
        })
    } else {
        None
    };
    CacheStatus {
        session_id: session_id.to_string(),
        blocks,
        tail,
    }
}

/// The listing marker: what the session's cache covers through, or none.
pub fn through_marker(shadow_root: &Path, session_id: &str) -> Option<CacheThrough> {
    read_ledger(&ledger_path(shadow_root, session_id))
        .last()
        .map(|b| CacheThrough {
            through_id: b.to_id,
            through_ts: b.to_ts.clone(),
        })
}

/// The explicit `summarise_from` flow, MCP and CLI: the caller names the id
/// to start from and the guard refuses anything that
/// is not exactly the first unsummarised id — one past the last block's
/// `to_id`, or 1 with no blocks — in the house form naming the expected id.
pub async fn summarise_from_id(
    adapter: &dyn crate::RolloutAdapter,
    session_id: &str,
    from_id: u64,
    requested_provider: Option<&str>,
) -> Result<String, String> {
    let messages = adapter
        .read_session_mmap(session_id)
        .map_err(|e| e.to_string())?;
    if messages.is_empty() {
        return Err("No messages found in session".to_string());
    }

    let shadow_root = adapter.shadow_index_root();
    let ledger = read_ledger(&ledger_path(&shadow_root, session_id));
    let expected = expected_from_id(&ledger);
    if from_id != expected {
        return Err(format!(
            "summarise_from: `from_id` {from_id} is not the first unsummarised id — the cache covers through id {}, so the expected `from_id` is {expected} (summary_cache shows the blocks)",
            ledger.last().map(|b| b.to_id).unwrap_or(0)
        ));
    }
    summarise_delta(
        &shadow_root,
        session_id,
        &messages,
        expected,
        requested_provider,
    )
    .await
}

/// The shared delta path behind `summarise_from`:
/// summarise messages `from_id..=max id`, store the block, return the
/// composed parts plus the footer. A tail with no timestamps cannot key a
/// block, so it falls back to a whole-session summarise that touches no
/// files — the same rule an untimestamped session has always had.
async fn summarise_delta(
    shadow_root: &Path,
    session_id: &str,
    messages: &[crate::RolloutMessage],
    from_id: u64,
    requested_provider: Option<&str>,
) -> Result<String, String> {
    let Some((first_ts, last_ts)) = bounds(messages) else {
        return summarise_whole(requested_provider, messages.to_vec()).await;
    };
    let slice = &messages[(from_id - 1) as usize..];
    if slice.is_empty() {
        // The caller asked to start past the session's end: everything is
        // covered, so the composition is the answer and no provider runs.
        let ledger = read_ledger(&ledger_path(shadow_root, session_id));
        let mut out = compose(&ledger);
        out.push_str(&format!(
            "\n\n---\nsummary cache: {} part(s), {first_ts} → {last_ts}",
            ledger.len()
        ));
        return Ok(out);
    }
    if bounds(slice).is_none() {
        // The uncovered tail carries no timestamps: it cannot key a block.
        return summarise_whole(requested_provider, messages.to_vec()).await;
    }

    let text = llm_summary(requested_provider, slice).await?;
    append_block(shadow_root, session_id, from_id, slice, &text)
        .map_err(|e| format!("summary cache write failed: {e}"))?;

    let ledger = read_ledger(&ledger_path(shadow_root, session_id));
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
    let provider = crate::provider_for(requested_provider).map_err(|e| format!("{e:#}"))?;
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
        let dir =
            std::env::temp_dir().join(format!("tr_summary_cache_{name}_{}", std::process::id()));
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
        assert_eq!(
            b,
            ("2026-10-18T10:00:00Z".into(), "2026-10-18T12:00:00Z".into())
        );
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
    fn expected_from_id_is_one_past_the_last_block_or_one() {
        assert_eq!(expected_from_id(&[]), 1, "no blocks start at 1");
        let blocks = vec![Block {
            from_id: 1,
            from_ts: "a".into(),
            to_id: 4,
            to_ts: "b".into(),
            user: 0,
            assistant: 0,
            tool: 0,
            thinking: 0,
            chars: 0,
            tokens_est: 0,
            part_file: PathBuf::from("p"),
        }];
        assert_eq!(expected_from_id(&blocks), 5);
    }

    #[test]
    fn the_ledger_round_trips_ids_counts_and_drops_any_invalid_row() {
        let dir = scratch("ledger");
        let path = ledger_path(&dir, "ses_x");
        assert!(read_ledger(&path).is_empty(), "missing ledger is empty");

        let msgs = vec![
            msg(Some("2026-10-18T10:00:00Z"), "aaaa"),
            msg(Some("2026-10-18T11:00:00Z"), "bb"),
        ];
        let b1 = append_block(&dir, "ses_x", 1, &msgs, "first summary").unwrap();
        assert_eq!(b1.from_id, 1);
        assert_eq!(b1.to_id, 2, "ids come from the slice's position, 1-based");
        assert_eq!(b1.chars, 6, "chars count the slice's content");

        let msgs2 = vec![msg(Some("2026-10-18T12:00:00Z"), "cccc")];
        let b2 = append_block(&dir, "ses_x", 3, &msgs2, "delta summary").unwrap();
        assert_eq!(b2.to_id, 3);

        let ledger = read_ledger(&path);
        assert_eq!(ledger.len(), 2);
        assert_eq!(ledger[1].to_id, 3);
        assert_eq!(ledger[1].to_ts, "2026-10-18T12:00:00Z");
        assert_eq!(b1.part_file, ledger[0].part_file);
        let composed = compose(&ledger);
        assert!(
            composed.contains("### part 1: 2026-10-18T10:00:00Z → 2026-10-18T11:00:00Z (ids 1..2)"),
            "each part names its id range: {composed}"
        );
        assert!(composed.contains("first summary"));
        assert!(composed.contains("delta summary"));
        assert_eq!(b2.part_file.file_name().unwrap(), "part_003_003.md");

        // One malformed row poisons the file it sits in.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "old,format,row").unwrap();
        drop(f);
        assert!(read_ledger(&path).is_empty(), "a malformed row empties it");
        assert!(!path.exists(), "the invalid ledger file is dropped");
    }
}
