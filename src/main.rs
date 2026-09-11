use std::io::Write;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use inception_mercury_compaction::{
    MercuryProvider, RolloutAdapter, RolloutMessage, SYSTEM_PROMPT, build_structured_prompt,
    harness::{make_adapter, resolve_harness},
};

#[derive(Parser)]
#[command(name = "inception-mercury-compaction")]
#[command(about = "Compact agent session rollouts using Inception Mercury 2.5")]
pub struct Cli {
    /// Which harness to use (vibe | codex | claude | opencode | cursor). Required for list/profile/extract/user-messages/compact; optional for mcp (falls back to HARNESS env var).
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
    /// Compact a rollout using Mercury 2.5
    Compact,
    /// Start as an MCP server on stdio
    Mcp,
}

fn resolve_session(adapter: &dyn RolloutAdapter, session: &Option<String>) -> String {
    match session {
        Some(s) => s.clone(),
        None => {
            let sessions = adapter.list_sessions();
            if sessions.is_empty() {
                eprintln!("No sessions found");
                std::process::exit(1);
            }
            sessions[0].session_id.clone()
        }
    }
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
        let server = inception_mercury_compaction::mcp::CompactionServer::with_harness(harness);
        let service = server.serve(stdio()).await?;
        service.waiting().await?;
        return Ok(());
    }

    let harness_name = match cli.harness.as_deref() {
        Some(name) => name,
        None => {
            eprintln!("error: --harness is required (vibe | codex | claude | opencode | cursor)");
            std::process::exit(2);
        }
    };
    let adapter = make_adapter(harness_name).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(2);
    });
    let session_id = resolve_session(adapter.as_ref(), &cli.session);

    match cli.command {
        Command::List => {
            let sessions = adapter.list_sessions();
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
            let profile = adapter.profile_session(&session_id);
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

        Command::Mcp => {}
    }

    Ok(())
}
