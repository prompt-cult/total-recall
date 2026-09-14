use crate::rollout::{RolloutMessage, SessionSummary, messages_to_text};

/// System prompt for the current-state summary (same as compaction).
pub const STATE_SYSTEM_PROMPT: &str =
    "You are a helpful coding assistant that summarizes conversations.";

const STATE_PROMPT: &str = r#"Create a structured summary of this coding conversation. Use these exact sections:

## Accomplished
List what was completed.

## Current Work
What is being worked on now.

## Files Involved
List all files mentioned or modified.

## Next Steps
Clear actions to take.

## Key Decisions/Constraints
Important user preferences, project requirements, or decisions made.

Be precise. Include file paths, function names, and specific details.

--- Conversation ---
{conversation}"#;

/// System prompt for the user-goals summary.
pub const GOALS_SYSTEM_PROMPT: &str = "You are a helpful assistant that extracts the user's goals, tasks, steers, and corrections from their messages in a coding session.";

const GOALS_PROMPT: &str = r#"Below are all the user's messages from a coding session, in order.
Extract a structured list of:

1. **Goals/Objectives**: What the user wants to achieve
2. **Tasks**: Specific things the user asked to be done
3. **Steers**: Preferences, directions, or constraints the user gave (e.g. "use mistral not openrouter", "don't spend money", "use rust not python")
4. **Corrections**: When the user corrected the agent's approach or behaviour

List them as an append-only list in the order they appeared. Use this format:

### Goals
- Goal 1
- Goal 2

### Tasks
- Task 1
- Task 2

### Steers
- Steer 1
- Steer 2

### Corrections
- Correction 1

Be concise but preserve the material facts of what the user asked for.

--- User Messages ---
{messages}"#;

/// Build the prompt for the current-state LLM call.
pub fn build_state_prompt(messages: &[RolloutMessage]) -> String {
    let conversation = messages_to_text(messages);
    STATE_PROMPT.replace("{conversation}", &conversation)
}

/// Build the prompt for the user-goals LLM call.
pub fn build_goals_prompt(user_messages: &[String]) -> String {
    let messages_text = user_messages
        .iter()
        .enumerate()
        .map(|(i, msg)| format!("{}. {}", i + 1, msg))
        .collect::<Vec<_>>()
        .join("\n\n");
    GOALS_PROMPT.replace("{messages}", &messages_text)
}

/// Build the recent rollouts table as markdown.
/// `current_session_id` is marked as "SUMMARISED" in the table.
/// `hours_back` limits to sessions with mtime within that many hours.
pub fn build_recent_rollouts_table(
    sessions: &[SessionSummary],
    current_session_id: Option<&str>,
    hours_back: u64,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "## Recent Rollouts ({}h)",
        hours_back
    ));
    lines.push(String::new());

    if sessions.is_empty() {
        lines.push("No recent sessions found.".to_string());
        return lines.join("\n");
    }

    lines.push("| Session | Title | Size | Lines | User | Asst | Tool | Compaction | Notes |".to_string());
    lines.push("|---------|-------|------|-------|------|------|------|------------|-------|".to_string());

    for s in sessions {
        let size_str = if s.file_size >= 1_000_000 {
            format!("{:.1}MB", s.file_size as f64 / 1_000_000.0)
        } else {
            format!("{}KB", s.file_size / 1000)
        };

        let notes = if let Some(cid) = current_session_id {
            if s.session_id.contains(cid) {
                "**SUMMARISED**".to_string()
            } else {
                "-".to_string()
            }
        } else {
            "-".to_string()
        };

        let compaction = if s.has_compaction { "yes" } else { "no" };

        lines.push(format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            s.session_id, s.title, size_str, s.line_count, s.user_count, s.assistant_count, s.tool_count, compaction, notes
        ));
    }

    lines.join("\n")
}

/// Build the plan/todo files section.
/// Looks for `.tmp/delegation/itemNN.md` and `~/.vibe/plans/*.md` files.
pub fn build_plan_files_section() -> String {
    let mut lines = Vec::new();
    lines.push("## Plan and Todo Files".to_string());
    lines.push(String::new());

    let mut found_any = false;

    // Check .tmp/delegation/ in current directory
    let delegation_dir = std::path::Path::new(".tmp/delegation");
    if let Ok(entries) = std::fs::read_dir(delegation_dir) {
        let mut files: Vec<_> = entries
            .flatten()
            .filter(|e| {
                e.file_name().to_string_lossy().ends_with(".md")
                    && e.file_name().to_string_lossy().starts_with("item")
            })
            .collect();
        files.sort_by_key(|f| f.file_name());

        for entry in &files {
            let path = entry.path();
            if let Ok(meta) = entry.metadata() {
                let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                let mtime_str = format!("{:?}", mtime);
                let size = meta.len();
                lines.push(format!(
                    "- {} ({} bytes, mtime: {})",
                    path.display(),
                    size,
                    mtime_str
                ));
                found_any = true;
            }
        }
    }

    // Check ~/.vibe/plans/
    if let Ok(home) = std::env::var("HOME") {
        let plans_dir = std::path::Path::new(&home).join(".vibe/plans");
        if let Ok(entries) = std::fs::read_dir(&plans_dir) {
            let mut files: Vec<_> = entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".md"))
                .collect();
            files.sort_by_key(|f| f.file_name());

            for entry in &files {
                let path = entry.path();
                if let Ok(meta) = entry.metadata() {
                    let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    let mtime_str = format!("{:?}", mtime);
                    let size = meta.len();
                    lines.push(format!(
                        "- {} ({} bytes, mtime: {})",
                        path.display(),
                        size,
                        mtime_str
                    ));
                    found_any = true;
                }
            }
        }
    }

    if !found_any {
        lines.push("No plan or todo files found.".to_string());
    }

    lines.join("\n")
}

/// The instructions section appended to every recall output.
pub const INSTRUCTIONS: &str = r#"## Instructions

You must use the total-recall tool to read into the session logs to find out
the material facts of what has been done. Ensure there is a clean todo list
where nothing the user has asked for that is material is dropped. If a todo
is not in the current context, use the tool to mine the rollouts to
efficiently continue where you left off."#;

/// Assemble the full recall output from its parts.
///
/// Ordering is deliberate for autoregressive LLMs:
/// 1. Recent Rollouts table (metadata -- sets the scene, reference data)
/// 2. Plan Files (metadata -- anchors the current plan)
/// 3. Current State (LLM summary -- what just happened)
/// 4. User Goals/Tasks/Steers (LLM summary -- the steering signal, fresh before instructions)
/// 5. Instructions (call to action -- last, strongest influence on next token)
///
/// Metadata goes first because it is reference data the model will look back at,
/// not generate from. The LLM summaries are the substance. Instructions go last
/// because the final tokens have the strongest influence on what the model does next.
pub fn build_recall_output(
    state_summary: &str,
    goals_summary: &str,
    rollouts_table: &str,
    plan_files: &str,
) -> String {
    let sections = vec![
        // 1. Metadata: recent rollouts (sets the scene)
        rollouts_table.to_string(),
        String::new(),
        // 2. Metadata: plan/todo files (anchors current plan)
        plan_files.to_string(),
        String::new(),
        // 3. Substance: current state (what just happened)
        "## Current State".to_string(),
        String::new(),
        state_summary.to_string(),
        String::new(),
        // 4. Substance: user goals/tasks/steers (steering signal, fresh before instructions)
        "## User Goals, Tasks, and Steers".to_string(),
        String::new(),
        goals_summary.to_string(),
        String::new(),
        // 5. Call to action (last, strongest influence on next token)
        INSTRUCTIONS.to_string(),
    ];

    sections.join("\n")
}

/// The result of a total-recall operation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecallOutput {
    /// The full assembled markdown output.
    pub output: String,
    /// Time taken for the state summary LLM call (seconds).
    pub state_time_s: f64,
    /// Time taken for the goals summary LLM call (seconds).
    pub goals_time_s: f64,
    /// Total wall-clock time (seconds).
    pub total_time_s: f64,
    /// Number of sessions in the rollouts table.
    pub sessions_count: usize,
    /// Number of user messages extracted.
    pub user_messages_count: usize,
    /// Number of messages in the session (from compaction point).
    pub session_messages_count: usize,
}

/// Filter sessions to those modified within `hours_back` hours.
pub fn filter_recent_sessions(
    sessions: &[SessionSummary],
    hours_back: u64,
) -> Vec<SessionSummary> {
    let now = std::time::SystemTime::now();
    let cutoff = now
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_sub(hours_back * 3600);

    sessions
        .iter()
        .filter(|s| {
            // Parse start_time as a rough proxy for mtime.
            // Vibe sessions are named session_YYYYMMDD_HHMMSS_<id>
            // We can parse the session_id for a rough timestamp.
            let parts: Vec<&str> = s.session_id.split('_').collect();
            if parts.len() >= 3 {
                let date_part = parts[1]; // YYYYMMDD
                let time_part = parts[2]; // HHMMSS
                if date_part.len() == 8 && time_part.len() == 6 {
                    let year: u32 = date_part[..4].parse().unwrap_or(0);
                    let month: u32 = date_part[4..6].parse().unwrap_or(0);
                    let day: u32 = date_part[6..8].parse().unwrap_or(0);
                    let hour: u32 = time_part[..2].parse().unwrap_or(0);
                    let min: u32 = time_part[2..4].parse().unwrap_or(0);
                    let sec: u32 = time_part[4..6].parse().unwrap_or(0);

                    // Rough epoch conversion (not perfect, but good enough for filtering)
                    // Days since epoch = (year-1970)*365 + leap days + day of year
                    let days_since_epoch = ((year as u64 - 1970) * 365)
                        + ((year as u64 - 1969) / 4) // leap years
                        - ((year as u64 - 1901) / 100) // century non-leap
                        + ((year as u64 - 1601) / 400) // 400-year leap
                        + day_of_year(month, day, year);
                    let epoch_secs = days_since_epoch * 86400
                        + (hour as u64) * 3600
                        + (min as u64) * 60
                        + sec as u64;
                    return epoch_secs >= cutoff;
                }
            }
            // If we can't parse, include it (safe default)
            true
        })
        .cloned()
        .collect()
}

fn day_of_year(month: u32, day: u32, year: u32) -> u64 {
    let days_in_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut doy = 0u64;
    for m in 1..month {
        doy += days_in_month[(m - 1) as usize];
    }
    // Leap year adjustment
    if month > 2 && (year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))) {
        doy += 1;
    }
    doy + (day as u64) - 1
}
