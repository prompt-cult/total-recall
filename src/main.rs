use std::io::Write;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use total_recall::{
    LISTING_ROW_CAP, ListingRow, RolloutAdapter, RolloutMessage,
    harness::{make_adapter, resolve_harness, resolve_session},
    index,
    mercury::provider_for,
    recall::{
        CHURN_STEER_COUNT, GOALS_SYSTEM_PROMPT, STATE_SYSTEM_PROMPT, build_commits_section,
        build_compaction_footer, build_goals_prompt, build_plan_files_section,
        build_prior_period_output, build_recent_rollouts_table, build_state_prompt,
        build_todo_log_section, build_todo_log_unavailable, recent_steers,
    },
    todo_history::{diff_todo_states, opened_closed_counts},
};

#[derive(Parser)]
#[command(name = "total-recall")]
#[command(version)]
#[command(about = "Total-recall MCP tool for log mining of session rollouts")]
pub struct Cli {
    /// Which harness to use (vibe | codex | claude | opencode). Required for list/profile/extract/user-messages; optional for mcp (falls back to HARNESS env var).
    #[arg(long, global = true)]
    pub harness: Option<String>,

    /// Session ID (partial match). Defaults to most recent.
    #[arg(long, global = true)]
    pub session: Option<String>,

    /// Output as JSON (default)
    #[arg(long, global = true)]
    pub json: bool,

    /// Output as markdown
    #[arg(long, global = true)]
    pub markdown: bool,

    /// Process the entire rollout
    #[arg(long, global = true)]
    pub full: bool,

    /// Trace-level logging
    #[arg(long, global = true)]
    pub verbose: bool,

    /// LLM provider: mercury (default) or mistral. A provider not compiled
    /// into this build fails with an explicit error — see README "Build features"
    #[arg(long, global = true)]
    pub provider: Option<String>,

    /// Bound extract/user-messages to at most N records. 0 = unbounded (default;
    /// stdout is a stream). Truncation notice goes to stderr.
    #[arg(long, global = true, default_value_t = 0)]
    pub limit: usize,

    /// Start record index for extract/user-messages (omit = most recent `limit`).
    #[arg(long, global = true)]
    pub offset: Option<usize>,

    /// Byte cap on extract/user-messages output. 0 = unbounded (default).
    #[arg(long, global = true, default_value_t = 0)]
    pub max_bytes: usize,

    /// Per-record clamp for extract/user-messages, bytes. 0 = default 256 KiB.
    #[arg(long, global = true, default_value_t = 0)]
    pub max_record_bytes: usize,

    /// Include injected (synthetic) user records in extract-by-type. Default false.
    #[arg(long, global = true, default_value_t = false)]
    pub include_injected: bool,

    /// On-disk profile cache for `profile`, on by default: serve from
    /// <shadow_root>/tr_<session-id>_meta.json when fresh (15 s staleness
    /// tolerance); recompute and rewrite otherwise. --no-cache forces a
    /// recompute without touching the cache.
    #[arg(long = "no-cache", global = true)]
    pub no_cache: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// List rollouts for a harness: `--hours N >= 1` is the scoped listing
    /// (the window and the 200-row cap applied where the data lives, held-back
    /// count on stderr); `--hours 0` (the default) is the unbounded stream.
    List {
        /// Hours back (mtime window) for the scoped listing. 0 = unbounded
        /// (the default; stdout is a stream). N >= 1: at most the 200 most
        /// recent rows of the window.
        #[arg(long, default_value_t = 0)]
        hours: u64,
    },
    /// Profile a specific rollout (file size, counts, events)
    Profile,
    /// Extract all data that would be summarised from a rollout
    Extract,
    /// Extract what the user said verbatim
    UserMessages,
    /// Raw extract of entries by type (user|assistant|tool|thinking), one
    /// `type,timestamp,json` record per line, for recovering large/damaged sessions
    #[command(name = "extract-by-type")]
    ExtractByType {
        /// Entry type to include (repeatable): user|assistant|tool|thinking, or "all"
        #[arg(long = "type", value_name = "kind")]
        types: Vec<String>,
    },
    /// Recover a rollout's todo-list edits as a JSONL event stream: one
    /// {ts,action,todo} object per line, in ts order
    #[command(name = "todo-history")]
    TodoHistory {
        /// Read from the last compaction point instead of the whole rollout
        #[arg(long = "no-full")]
        no_full: bool,
    },
    /// Structured summary of a rollout up to its current point
    /// (Accomplished, Current Work, Files, Next Steps, Key Decisions)
    #[command(name = "summarise-current-session")]
    SummariseCurrentSession,
    /// Introspect a rollout's summary cache: the cached blocks (id ranges,
    /// type counts, chars, token estimates) plus the unsummarised tail.
    /// Read-only — never calls the LLM
    #[command(name = "summary-cache")]
    SummaryCache,
    /// Summarise a rollout from an explicit 1-based message id. The guard:
    /// the id must be the first unsummarised id (one past the last cached
    /// block's to_id, or 1 with no blocks); anything else is an error naming
    /// the expected id
    #[command(name = "summarise-from")]
    SummariseFrom {
        /// 1-based message id to summarise from (summary-cache reports it
        /// as tail.from_id)
        #[arg(long)]
        from_id: u64,
    },
    /// Roll the summary cache back one block: delete the newest ledger row
    /// and its part file, then print the new cache status
    #[command(name = "cache-delete-latest")]
    CacheDeleteLatest,
    /// An orientation summary of a prior period of work: state
    /// summary, user goals, the period's todo log, commits, the rollouts
    /// table and plan files, plus a compaction-points FYI and an orientation
    /// warning. The period is the work since the last compaction boundary —
    /// the work after that boundary
    #[command(name = "prior-period-summary")]
    PriorPeriodSummary,
    /// She-said/he-said/they-did: matched dialogue and tool actions
    HeSaidSheSaid {
        /// Comma-separated case-insensitive search terms (required)
        #[arg(long, required = true)]
        words: String,
        /// Session ID (partial match, repeatable). Empty = all rollouts within --hours.
        #[arg(long, value_name = "id")]
        sessions: Vec<String>,
        /// Hours back when no sessions are given (default 48; 0 = no bound)
        #[arg(long, default_value_t = 48)]
        hours: u64,
        /// Directory substring filter (empty-session-list mode)
        #[arg(long)]
        directory: Option<String>,
    },
    /// Build/refresh per-session tantivy shadow indexes
    Index {
        /// Session ID (partial match, repeatable). Empty = all rollouts within --hours.
        #[arg(long, value_name = "id")]
        sessions: Vec<String>,
        /// Hours back when no sessions are given (default 0 = no bound)
        #[arg(long, default_value_t = 0)]
        hours: u64,
        /// Directory substring filter (empty-session-list mode)
        #[arg(long)]
        directory: Option<String>,
    },
    /// Full-text search across per-session tantivy shadow indexes
    #[command(name = "do-android-dream-of-electric-sheep")]
    Sheep {
        /// Tantivy query syntax (required)
        #[arg(long, required = true)]
        query: String,
        /// Session ID (partial match, repeatable). Empty = all rollouts within --hours.
        #[arg(long, value_name = "id")]
        sessions: Vec<String>,
        /// Hours back when no sessions are given (default 48; 0 = no bound)
        #[arg(long, default_value_t = 48)]
        hours: u64,
        /// Directory substring filter (empty-session-list mode)
        #[arg(long)]
        directory: Option<String>,
    },
    /// Start as an MCP server on stdio
    Mcp,
}

/// Read messages respecting --full vs --from-compaction flags.
/// Default (neither flag): from compaction point.
/// --full: entire session.
/// A payload that cannot be read exits 2 with the path named on stderr: an
/// unreadable rollout must never print as an empty session.
fn read_messages(
    adapter: &dyn RolloutAdapter,
    session_id: &str,
    full: bool,
) -> Vec<RolloutMessage> {
    let result = if full {
        adapter.read_session_mmap(session_id)
    } else {
        adapter.read_session_from_compaction(session_id)
    };
    unwrap_read(result)
}

fn unwrap_read<T>(result: Result<T, String>) -> T {
    match result {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(2);
        }
    }
}

/// CLI-side bounding: unbounded by default. When `limit` or `max_bytes` is
/// given, apply the same window/fit/clamp primitives as the MCP envelope and
/// emit a truncation notice to stderr (stdout stays machine-parseable).
fn bound_cli_messages(
    mut messages: Vec<RolloutMessage>,
    limit: usize,
    offset: Option<usize>,
    max_bytes: usize,
    max_record_bytes: usize,
) -> Vec<RolloutMessage> {
    if limit == 0 && max_bytes == 0 {
        return messages;
    }
    let total = messages.len();
    let limit = if limit == 0 { total.max(1) } else { limit };
    let (start, end) = total_recall::bound::window(total, offset, limit);
    let mut windowed: Vec<RolloutMessage> = messages.drain(start..end).collect();
    let max_record_bytes = total_recall::bound::normalize_max_record_bytes(max_record_bytes);
    let mut clamped = 0usize;
    for m in windowed.iter_mut() {
        if total_recall::bound::clamp_message(m, max_record_bytes) {
            clamped += 1;
        }
    }
    if max_bytes > 0 {
        let sized: Vec<total_recall::bound::SizedRecord> = windowed
            .iter()
            .map(|m| {
                let json = serde_json::to_string(m).unwrap_or_default();
                let bytes = json.len();
                total_recall::bound::SizedRecord { json, bytes }
            })
            .collect();
        // Even one record must fit; otherwise stdout would break the cap.
        if sized
            .first()
            .is_some_and(|r| r.bytes.saturating_add(1) > max_bytes)
        {
            eprintln!(
                "error: even one record ({}) exceeds --max-bytes ({}); raise --max-bytes or set --max-record-bytes to enable the clamp",
                sized[0].bytes, max_bytes
            );
            std::process::exit(2);
        }
        let keep = total_recall::bound::fit_count(&sized, max_bytes);
        windowed.truncate(keep);
    }
    let returned = windowed.len();
    // An empty result window (paging past the end) cut nothing; never signal
    // truncation for it.
    if returned == 0 {
        return windowed;
    }
    if returned < total || clamped > 0 {
        eprintln!(
            "TRUNCATED: returned records {}..{} of {}{}{}. Use --offset/--limit/--max-bytes to page.",
            start,
            start + returned,
            total,
            if clamped > 0 {
                format!(", {} records clamped", clamped)
            } else {
                String::new()
            },
            if start + returned < total {
                format!(", next_offset={}", start + returned)
            } else {
                String::new()
            }
        );
    }
    windowed
}

/// String variant for user-messages.
fn bound_cli_strings(
    messages: Vec<String>,
    limit: usize,
    offset: Option<usize>,
    max_bytes: usize,
    max_record_bytes: usize,
) -> Vec<String> {
    let wrapped: Vec<RolloutMessage> = messages
        .into_iter()
        .map(|content| RolloutMessage {
            role: "user".to_string(),
            content,
            thinking: None,
            tool_calls_summary: Vec::new(),
            timestamp: None,
            injected: false,
        })
        .collect();
    bound_cli_messages(wrapped, limit, offset, max_bytes, max_record_bytes)
        .into_iter()
        .map(|m| m.content)
        .collect()
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let level = if cli.verbose { "trace" } else { "warn" };
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(level))
        .with_writer(std::io::stderr)
        .init();

    if let Command::Mcp = cli.command {
        use rmcp::{ServiceExt, transport::stdio};

        let harness = match resolve_harness(cli.harness.as_deref()) {
            Ok(h) => h,
            Err(msg) => {
                println!("{}", msg);
                eprintln!("{}", msg);
                std::process::exit(2);
            }
        };
        let server = total_recall::mcp::TotalRecallServer::with_harness(harness);
        let service = server.serve(stdio()).await?;
        service.waiting().await?;
        return Ok(());
    }

    let harness_name = match cli.harness.as_deref() {
        Some(name) => name,
        None => {
            eprintln!("error: --harness is required (vibe | codex | claude | opencode)");
            std::process::exit(2);
        }
    };
    let adapter = make_adapter(harness_name).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(2);
    });
    let session_id = if matches!(
        cli.command,
        Command::HeSaidSheSaid { .. } | Command::Index { .. } | Command::Sheep { .. }
    ) {
        String::new()
    } else {
        resolve_session(adapter.as_ref(), cli.session.as_deref().unwrap_or("")).unwrap_or_else(
            || {
                eprintln!("No sessions found");
                std::process::exit(1);
            },
        )
    };

    match cli.command {
        Command::List { hours } => {
            // The CLI doctrine: stdout is a stream, `0` means unbounded, and
            // the command typed is the scope asked for. `--hours N >= 1` is
            // the scoped listing: the window and the row cap are applied
            // where the data lives, and the held-back count goes to stderr
            // so stdout stays machine-parseable.
            let (mut sessions, window_count) = match hours {
                0 => (adapter.list_sessions(), None),
                n => {
                    let listing = adapter.list_sessions_scoped(n, None);
                    (listing.sessions, Some(listing.window_count))
                }
            };
            index::annotate_sessions(&mut sessions, adapter.as_ref());
            if let Some(window) = window_count
                && window > sessions.len()
            {
                eprintln!(
                    "TRUNCATED: {} sessions in the {}h window, {} printed (cap {}). Use --hours 0 for the unbounded stream.",
                    window,
                    hours,
                    sessions.len(),
                    LISTING_ROW_CAP
                );
            }
            if cli.markdown {
                println!("| Session | Title | Lines | User | Assistant | Tool | Compaction |");
                println!("|---------|-------|-------|------|-----------|------|------------|");
                for s in &sessions {
                    println!(
                        "| {} | {} | {} | {} | {} | {} | {} |",
                        s.session_id,
                        s.title,
                        s.line_count,
                        s.user_count,
                        s.assistant_count,
                        s.tool_count,
                        s.has_compaction
                    );
                }
            } else if cli.json {
                // The JSON rows are rendered ListingRows with the
                // summary_cache marker set by this render pass (reading each
                // session's ledger's last row — the adapters stay untouched).
                let root = adapter.shadow_index_root();
                let rows: Vec<ListingRow> = sessions
                    .iter()
                    .map(|s| {
                        let mut row = ListingRow::new(s);
                        row.summary_cache =
                            total_recall::summary_cache::through_marker(&root, &s.session_id);
                        row
                    })
                    .collect();
                let json = match window_count {
                    Some(window) => serde_json::to_string_pretty(&serde_json::json!({
                        "sessions": rows,
                        "window_count": window,
                    }))?,
                    None => serde_json::to_string_pretty(&rows)?,
                };
                println!("{json}");
            } else {
                for s in &sessions {
                    println!(
                        "{}  {}  lines={}  user={}  asst={}  tool={}  compaction={}",
                        s.session_id,
                        s.title,
                        s.line_count,
                        s.user_count,
                        s.assistant_count,
                        s.tool_count,
                        s.has_compaction
                    );
                }
            }
        }

        Command::Profile => {
            let mut profile = match adapter.profile_session_opts(&session_id, !cli.no_cache) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("error: {}", e);
                    std::process::exit(2);
                }
            };
            profile.has_tantivy_index = index::index_exists(adapter.as_ref(), &session_id);
            if cli.json {
                let json = serde_json::to_string_pretty(&profile)?;
                println!("{}", json);
            } else {
                println!("Session: {}", profile.session_id);
                println!("File size: {} bytes", profile.file_size);
                println!("Line count: {}", profile.line_count);
                if let Some(ts) = &profile.first_ts {
                    println!("First TS: {}", ts);
                }
                if let Some(ts) = &profile.last_ts {
                    println!("Last TS: {}", ts);
                }
                println!("\nRole counts:");
                for (role, count) in &profile.role_counts {
                    println!("  {}: {}", role, count);
                }
                println!("\nInteresting events:");
                for event in &profile.interesting_events {
                    println!(
                        "  line {} (gap {}): {:?} — {}",
                        event.line_number, event.gap_lines, event.event_type, event.summary
                    );
                }
            }
        }

        Command::Extract => {
            let messages = read_messages(adapter.as_ref(), &session_id, cli.full);
            let messages = bound_cli_messages(
                messages,
                cli.limit,
                cli.offset,
                cli.max_bytes,
                cli.max_record_bytes,
            );
            if cli.json || !cli.markdown {
                for msg in &messages {
                    let json = serde_json::to_string(msg)?;
                    println!("{}", json);
                }
            } else {
                for msg in &messages {
                    println!("[{}]", msg.role.to_uppercase());
                    if !msg.content.is_empty() {
                        println!("{}", msg.content);
                    }
                    for tc in &msg.tool_calls_summary {
                        println!("  -> {}", tc);
                    }
                    println!();
                }
            }
        }

        Command::UserMessages => {
            let messages = unwrap_read(adapter.extract_user_messages(&session_id));
            let messages = bound_cli_strings(
                messages,
                cli.limit,
                cli.offset,
                cli.max_bytes,
                cli.max_record_bytes,
            );
            if cli.json || !cli.markdown {
                let json = serde_json::to_string_pretty(&messages)?;
                println!("{}", json);
            } else {
                for (i, msg) in messages.iter().enumerate() {
                    println!("{}. {}", i + 1, msg);
                    println!();
                }
            }
        }

        Command::ExtractByType { types } => {
            const VALID: [&str; 4] = ["user", "assistant", "tool", "thinking"];
            let want_all = types.is_empty() || types.iter().any(|t| t == "all");
            if !want_all {
                for t in &types {
                    if !VALID.contains(&t.as_str()) {
                        eprintln!(
                            "error: unknown type '{}'; valid: {} or \"all\"",
                            t,
                            VALID.join("|")
                        );
                        std::process::exit(2);
                    }
                }
            }
            let entries =
                match adapter.read_session_entries(&session_id, cli.full, cli.include_injected) {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("error: {}", e);
                        std::process::exit(2);
                    }
                };
            let filtered: Vec<&total_recall::rollout::RolloutEntry> = entries
                .iter()
                .filter(|e| want_all || types.contains(&e.entry_type))
                .collect();
            // CLI: unbounded unless --limit/--max-bytes given.
            let limit = if cli.limit == 0 {
                filtered.len().max(1)
            } else {
                cli.limit
            };
            let total = filtered.len();
            let (start, end) = total_recall::bound::window(total, cli.offset, limit);
            let max_record_bytes =
                total_recall::bound::normalize_max_record_bytes(cli.max_record_bytes);
            let mut sized: Vec<total_recall::bound::SizedRecord> = Vec::with_capacity(end - start);
            for e in &filtered[start..end] {
                let mut rec = serde_json::to_string(&e.record).unwrap_or_else(|_| "{}".to_string());
                if rec.len() > max_record_bytes {
                    rec = total_recall::rollout::truncate_chars(&rec, max_record_bytes).to_string();
                }
                let line = format!(
                    "{},{},{}",
                    e.entry_type,
                    total_recall::rollout::entry_timestamp(e.timestamp.as_deref()),
                    rec
                );
                let bytes = line.len();
                sized.push(total_recall::bound::SizedRecord { json: line, bytes });
            }
            // Even one record must fit; otherwise stdout would break the cap.
            if cli.max_bytes > 0
                && sized
                    .first()
                    .is_some_and(|r| r.bytes.saturating_add(1) > cli.max_bytes)
            {
                eprintln!(
                    "error: even one record ({}) exceeds --max-bytes ({}); raise --max-bytes or set --max-record-bytes to enable the clamp",
                    sized[0].bytes, cli.max_bytes
                );
                std::process::exit(2);
            }
            let kept = if cli.max_bytes > 0 {
                &sized[..total_recall::bound::fit_count(&sized, cli.max_bytes)]
            } else {
                &sized[..]
            };
            let returned = kept.len();
            // An empty result window (paging past the end) cut nothing; never
            // signal truncation for it.
            if returned > 0 && (start > 0 || start + returned < total) {
                eprintln!(
                    "TRUNCATED: returned records {}..{} of {}. Use --offset/--limit/--max-bytes to page.",
                    start,
                    start + returned,
                    total
                );
            }
            for r in kept {
                println!("{}", r.json);
            }
            let _ = std::io::stdout().flush();
        }

        Command::TodoHistory { no_full } => {
            // The whole rollout is this command's natural scope (matching the
            // MCP tool's full=true default); --no-full slices at the last
            // compaction point.
            let full = cli.full || !no_full;
            let writes = unwrap_read(adapter.read_todo_writes(&session_id, full));
            let events = total_recall::todo_history::diff_todo_states(&writes);
            // CLI: unbounded — stdout is a stream, one JSONL event per line.
            for e in &events {
                let line = serde_json::to_string(e).unwrap_or_else(|_| "{}".to_string());
                println!("{}", line);
            }
            let _ = std::io::stdout().flush();
        }

        Command::SummariseCurrentSession => {
            // The ledger is keyed by the canonical id the listing rows carry,
            // so the CLI and the `summary_cache` marker describe the same
            // cache however the caller spells the id.
            let session_id =
                total_recall::harness::canonical_session_id(adapter.as_ref(), &session_id);
            let text = total_recall::summary_cache::summarise_session(
                adapter.as_ref(),
                &session_id,
                !cli.no_cache,
                cli.provider.as_deref(),
            )
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
            print!("{text}");
            let _ = std::io::stdout().flush();
        }

        Command::SummaryCache => {
            let session_id =
                total_recall::harness::canonical_session_id(adapter.as_ref(), &session_id);
            let messages = read_messages(adapter.as_ref(), &session_id, true);
            let status = total_recall::summary_cache::status(
                &adapter.shadow_index_root(),
                &session_id,
                &messages,
            );
            println!("{}", serde_json::to_string_pretty(&status)?);
        }

        Command::SummariseFrom { from_id } => {
            let session_id =
                total_recall::harness::canonical_session_id(adapter.as_ref(), &session_id);
            let text = total_recall::summary_cache::summarise_from_id(
                adapter.as_ref(),
                &session_id,
                from_id,
                cli.provider.as_deref(),
            )
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
            print!("{text}");
            let _ = std::io::stdout().flush();
        }

        Command::CacheDeleteLatest => {
            let session_id =
                total_recall::harness::canonical_session_id(adapter.as_ref(), &session_id);
            let root = adapter.shadow_index_root();
            let blocks = total_recall::summary_cache::read_ledger(
                &total_recall::summary_cache::ledger_path(&root, &session_id),
            );
            if blocks.is_empty() {
                eprintln!(
                    "cache-delete-latest: no cached blocks to delete — nothing has been summarised for this session (summary-cache shows what is cached)"
                );
                std::process::exit(1);
            }
            if let Err(e) = total_recall::summary_cache::delete_latest(&root, &session_id) {
                eprintln!("error: cache-delete-latest: {e}");
                std::process::exit(2);
            }
            let messages = read_messages(adapter.as_ref(), &session_id, true);
            let status = total_recall::summary_cache::status(&root, &session_id, &messages);
            println!("{}", serde_json::to_string_pretty(&status)?);
        }

        Command::PriorPeriodSummary => {
            // The full session feeds the compaction footer; the slice since
            // the session's last compaction boundary — the context a
            // compaction consumed — feeds the state summary, the commits and
            // the todo log. The boundary is the adapter's own: on opencode
            // that is the typed compaction part, which prose quoting the
            // marker text cannot forge.
            let full_messages = unwrap_read(adapter.read_session_mmap(&session_id));
            if full_messages.is_empty() {
                eprintln!("No messages found in session {}", session_id);
                std::process::exit(1);
            }
            let markers = adapter.compaction_markers(&full_messages);
            let compaction_footer = build_compaction_footer(&full_messages, &markers);
            let has_compaction = !markers.is_empty();
            let messages = unwrap_read(adapter.read_session_from_compaction(&session_id));
            tracing::info!("Read {} period messages", messages.len());

            let user_messages = unwrap_read(adapter.extract_user_messages(&session_id));
            tracing::info!("Extracted {} user messages", user_messages.len());

            let state_prompt = build_state_prompt(&messages);
            let goals_prompt = build_goals_prompt(&user_messages);

            let provider = provider_for(cli.provider.as_deref())?;
            let t0 = std::time::Instant::now();
            let batch_results = provider
                .compact_batch_pairs(vec![
                    (STATE_SYSTEM_PROMPT.to_string(), state_prompt),
                    (GOALS_SYSTEM_PROMPT.to_string(), goals_prompt),
                ])
                .await?;
            let total_time = t0.elapsed();

            let [state_summary, goals_summary] = batch_results
                .try_into()
                .expect("compact_batch_pairs returns one result per call");

            let (todo_log, todo_opens, todo_closes) =
                match adapter.read_todo_writes(&session_id, false) {
                    Ok(writes) => {
                        let restatement = has_compaction;
                        let (opened, closed) = opened_closed_counts(&writes, restatement);
                        let events = diff_todo_states(&writes);
                        let steers = recent_steers(&user_messages, CHURN_STEER_COUNT);
                        let baseline = if restatement && !writes.is_empty() {
                            writes[0].todos.len()
                        } else {
                            0
                        };
                        (
                            build_todo_log_section(&events, opened, closed, baseline, &steers),
                            opened,
                            closed,
                        )
                    }
                    Err(e) => (build_todo_log_unavailable(adapter.name(), &e), 0, 0),
                };

            let hours_back: u64 = 24;
            let listing = adapter.list_sessions_scoped(hours_back, None);
            let rollouts_table =
                build_recent_rollouts_table(&listing, Some(&session_id), hours_back);
            let plan_files = build_plan_files_section();
            let commits = build_commits_section(&messages);
            let output = build_prior_period_output(
                &state_summary,
                &goals_summary,
                &rollouts_table,
                &plan_files,
                &todo_log,
                &commits,
                &compaction_footer,
            );

            eprintln!(
                "prior_period_summary: {} period messages, {} user msgs, {} todo opens, {} todo closes, {} recent sessions, {:.1}s",
                messages.len(),
                user_messages.len(),
                todo_opens,
                todo_closes,
                listing.window_count,
                total_time.as_secs_f64()
            );
            print!("{}", output);
            let _ = std::io::stdout().flush();
        }

        Command::HeSaidSheSaid {
            words,
            sessions,
            hours,
            directory,
        } => {
            let terms: Vec<String> = words
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if terms.is_empty() {
                eprintln!("error: --words requires at least one term");
                std::process::exit(1);
            }
            match adapter.she_said_he_said_action(&sessions, &terms, hours, directory.as_deref()) {
                Ok(report) => {
                    print!("{}", report);
                    let _ = std::io::stdout().flush();
                }
                Err(e) => {
                    eprintln!("error: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Command::Index {
            sessions,
            hours,
            directory,
        } => {
            let (selected, unmatched) =
                index::select_sessions(adapter.as_ref(), &sessions, hours, directory.as_deref());
            if !unmatched.is_empty() {
                eprintln!("error: unmatched session ids: {}", unmatched.join(", "));
                std::process::exit(1);
            }
            for summary in &selected {
                match index::index_session(adapter.as_ref(), &summary.session_id) {
                    Ok(stats) => {
                        println!("indexed {} docs={}", stats.session_id, stats.doc_count)
                    }
                    Err(e) => {
                        eprintln!("error: {}", e);
                        std::process::exit(1);
                    }
                }
            }
        }

        Command::Sheep {
            query,
            sessions,
            hours,
            directory,
        } => match index::search(
            adapter.as_ref(),
            &sessions,
            &query,
            hours,
            directory.as_deref(),
        ) {
            Ok(report) => {
                print!("{}", report);
                let _ = std::io::stdout().flush();
            }
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        },

        Command::Mcp => {}
    }

    Ok(())
}
