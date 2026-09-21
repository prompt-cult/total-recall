use std::io::Write;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use total_recall::{
    MercuryProvider, RolloutAdapter, RolloutMessage, SYSTEM_PROMPT, build_structured_prompt,
    harness::{make_adapter, resolve_harness, resolve_session},
    index,
    recall::{
        GOALS_SYSTEM_PROMPT, STATE_SYSTEM_PROMPT, build_goals_prompt, build_plan_files_section,
        build_recall_output, build_recent_rollouts_table, build_state_prompt,
        filter_recent_sessions,
    },
};

#[derive(Parser)]
#[command(name = "total-recall")]
#[command(version)]
#[command(about = "Total-recall MCP tool for fast compaction and log mining of session rollouts")]
pub struct Cli {
    /// Which harness to use (vibe | codex | claude | opencode). Required for list/profile/extract/user-messages/compact; optional for mcp (falls back to HARNESS env var).
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

    /// LLM provider: mercury (default) or mistral
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

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// List all rollouts for a harness
    List,
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
    /// Compact a rollout using Mercury 2.5
    Compact,
    /// Total recall: state summary + user goals + rollouts table + plan files
    Recall,
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
fn read_messages(
    adapter: &dyn RolloutAdapter,
    session_id: &str,
    full: bool,
) -> Vec<RolloutMessage> {
    if full {
        adapter.read_session_mmap(session_id)
    } else {
        adapter.read_session_from_compaction(session_id)
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
        let keep = total_recall::bound::fit_count(&sized, max_bytes);
        windowed.truncate(keep);
    }
    let returned = windowed.len();
    if returned < total || clamped > 0 {
        eprintln!(
            "TRUNCATED: returned records {}..{} of {}{}{}. Use --offset/--limit/--max-bytes to page.",
            start,
            start + returned,
            total,
            if clamped > 0 { format!(", {} records clamped", clamped) } else { String::new() },
            if start + returned < total { format!(", next_offset={}", start + returned) } else { String::new() }
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
        resolve_session(adapter.as_ref(), cli.session.as_deref().unwrap_or(""))
            .unwrap_or_else(|| {
                eprintln!("No sessions found");
                std::process::exit(1);
            })
    };

    match cli.command {
        Command::List => {
            let mut sessions = adapter.list_sessions();
            index::annotate_sessions(&mut sessions, adapter.as_ref());
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
                let json = serde_json::to_string_pretty(&sessions)?;
                println!("{}", json);
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
            let mut profile = adapter.profile_session(&session_id);
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
            let messages = adapter.extract_user_messages(&session_id);
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
            let entries = adapter.read_session_entries(&session_id, cli.full);
            let filtered: Vec<&total_recall::rollout::RolloutEntry> = entries
                .iter()
                .filter(|e| want_all || types.contains(&e.entry_type))
                .collect();
            // CLI: unbounded unless --limit/--max-bytes given.
            let limit = if cli.limit == 0 { filtered.len().max(1) } else { cli.limit };
            let total = filtered.len();
            let (start, end) = total_recall::bound::window(total, cli.offset, limit);
            let max_record_bytes = total_recall::bound::normalize_max_record_bytes(cli.max_record_bytes);
            let mut out = String::new();
            let mut used = 0usize;
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
                if cli.max_bytes > 0 && used + line.len() + 1 > cli.max_bytes {
                    break;
                }
                used += line.len() + 1;
                out.push_str(&line);
                out.push('\n');
            }
            let returned = out.lines().count();
            if returned < total {
                eprintln!(
                    "TRUNCATED: returned records {}..{} of {}. Use --offset/--limit/--max-bytes to page.",
                    start,
                    start + returned,
                    total
                );
            }
            print!("{}", out);
            let _ = std::io::stdout().flush();
        }

        Command::Compact => {
            let messages = read_messages(adapter.as_ref(), &session_id, cli.full);
            tracing::info!("Read {} messages from {}", messages.len(), session_id);

            let t0 = std::time::Instant::now();
            let prompt = build_structured_prompt(&messages);
            let format_time = t0.elapsed();
            tracing::info!("Prompt built in {:?}", format_time);

            let provider = MercuryProvider::new()?;
            let t1 = std::time::Instant::now();
            let summary = provider.compact(SYSTEM_PROMPT, &prompt).await?;
            let mercury_time = t1.elapsed();
            tracing::info!("Mercury responded in {:?}", mercury_time);

            print!("{}", summary);
            let _ = std::io::stdout().flush();
        }

        Command::Recall => {
            let messages = read_messages(adapter.as_ref(), &session_id, cli.full);
            tracing::info!("Read {} messages from {}", messages.len(), session_id);

            if messages.is_empty() {
                eprintln!("No messages found in session {}", session_id);
                std::process::exit(1);
            }

            let user_messages = adapter.extract_user_messages(&session_id);
            tracing::info!("Extracted {} user messages", user_messages.len());

            let state_prompt = build_state_prompt(&messages);
            let goals_prompt = build_goals_prompt(&user_messages);

            let provider = match cli.provider.as_deref() {
                Some("mistral") => MercuryProvider::new_mistral()?,
                _ => MercuryProvider::new()?,
            };
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

            let all_sessions = adapter.list_sessions();
            let hours_back: u64 = 24;
            let recent_sessions = filter_recent_sessions(&all_sessions, hours_back);
            let rollouts_table =
                build_recent_rollouts_table(&recent_sessions, Some(&session_id), hours_back);
            let plan_files = build_plan_files_section();
            let output =
                build_recall_output(&state_summary, &goals_summary, &rollouts_table, &plan_files);

            eprintln!(
                "total_recall: {} messages, {} user msgs, {} recent sessions, {:.1}s",
                messages.len(),
                user_messages.len(),
                recent_sessions.len(),
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
        } => match index::search(adapter.as_ref(), &sessions, &query, hours, directory.as_deref())
        {
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
