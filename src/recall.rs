use crate::redact::redact_secrets;
use crate::rollout::{LISTING_ROW_CAP, RolloutMessage, SessionListing};

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

/// Byte budget for the payload of the current-state prompt.
///
/// 200 KB is ~50K tokens, which at the documented free-tier ceiling of
/// 1,000,000 input tokens per minute is ~3 seconds of ingest — inside any
/// client deadline, with the rest of the 260K-token context left for the
/// answer. Unbounded, a single large session reached millions of tokens: past
/// the provider's input ceiling it is a hard error, and under it the ingest
/// alone outlived the deadline.
pub const MAX_STATE_BYTES: usize = 200 * 1024;

/// Byte budget for the payload of the user-goals prompt. Same sizing and the
/// same reasoning as [`MAX_STATE_BYTES`]; the two calls run in parallel, so
/// they are held to the same ceiling rather than one being allowed to starve
/// the other.
pub const MAX_GOALS_BYTES: usize = 200 * 1024;

/// Bytes held back from a prompt's budget for its truncation marker, so a
/// marked prompt is still inside its budget. Sized above the longest marker
/// [`truncation_marker`] can write (`the_boundary_marker_fits_its_reserve`
/// holds that).
const MARKER_RESERVE: usize = 256;

/// The built prompt and what the budget cost to hold it to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedPrompt {
    /// The prompt, ready to send. Carries the truncation marker when anything
    /// was dropped.
    pub text: String,
    /// Bytes of payload in `text`, marker included.
    pub bytes: usize,
    /// Bytes of raw source text that are not in the prompt.
    pub dropped_bytes: usize,
    /// How many source items (messages, user strings) are not shown in full.
    pub dropped_items: usize,
}

/// The one line that says a prompt was cut, naming what was dropped. A budget
/// without this line is a silent lie about what the model was shown.
fn truncation_marker(
    item: &str,
    dropped_items: usize,
    dropped_bytes: usize,
    budget: usize,
) -> String {
    format!(
        "[total-recall: prompt truncated — {dropped_items} oldest {item} not shown in full \
         ({dropped_bytes} bytes dropped of raw source text); the newest context is kept and \
         this prompt is held to {budget} bytes]"
    )
}

/// The newest `budget` bytes of `block`, cut at a character boundary and then
/// at a line boundary so the retained text never starts mid-word or mid-line.
/// Returns `None` when nothing fits.
fn tail_slice(block: &str, budget: usize) -> Option<&str> {
    if budget == 0 || block.is_empty() {
        return None;
    }
    let mut start = block.len() - budget;
    while start < block.len() && !block.is_char_boundary(start) {
        start += 1;
    }
    // Prefer the line boundary at or after the cut so the payload opens on a
    // whole line; fall back to the character boundary when a single line is
    // longer than the budget.
    let line_start = block[start..]
        .find('\n')
        .map(|offset| start + offset + 1)
        .unwrap_or(block.len());
    let text = &block[line_start..];
    if text.is_empty() { None } else { Some(text) }
}

/// What the tail walk kept and what it cost to keep it.
struct TailKeep {
    /// Retained blocks, oldest first, ready to join with the separator.
    blocks: Vec<String>,
    /// Bytes the retained blocks occupy once joined.
    kept_bytes: usize,
    /// Bytes of raw source text that are not in the prompt.
    dropped_bytes: usize,
    /// How many source items are not shown in full.
    dropped_items: usize,
}

/// Walk `items` newest-first, rendering item `i` with `render(i, &items[i])`,
/// and keep the newest blocks that fit `budget`. Whole blocks only: half a
/// message is worth less to a summary than a whole earlier message, and the
/// dropped count stays exact. The single exception is a newest block larger
/// than the whole budget, where its newest slice is kept so the prompt is never
/// empty.
///
/// Once the first item is dropped, every older item is dropped too and is
/// accounted from its raw length without being rendered. That is what keeps the
/// walk proportional to what is kept rather than to the size of the session:
/// rendering is where redaction happens, and redaction of text that is about to
/// be thrown away is the cost this budget exists to avoid.
fn tail_keep<T>(
    items: &[T],
    render: impl Fn(usize, &T) -> String,
    raw_len: impl Fn(&T) -> usize,
    separator: &str,
    budget: usize,
) -> TailKeep {
    let mut blocks: Vec<String> = Vec::new();
    let mut kept_bytes = 0usize;
    let mut dropped_bytes = 0usize;
    let mut dropped_items = 0usize;

    for (i, item) in items.iter().enumerate().rev() {
        if dropped_items > 0 {
            dropped_items += 1;
            dropped_bytes += raw_len(item);
            continue;
        }
        let block = render(i, item);
        if block.is_empty() {
            continue;
        }
        let cost = block.len()
            + if blocks.is_empty() {
                0
            } else {
                separator.len()
            };
        if kept_bytes + cost <= budget {
            kept_bytes += cost;
            blocks.push(block);
            continue;
        }
        if blocks.is_empty()
            && let Some(slice) = tail_slice(&block, budget)
        {
            blocks.push(slice.to_string());
            kept_bytes += slice.len();
        }
        dropped_items = 1;
        dropped_bytes = raw_len(item);
    }

    blocks.reverse();
    TailKeep {
        blocks,
        kept_bytes,
        dropped_bytes,
        dropped_items,
    }
}

/// Assemble a bounded payload: the marker when anything was dropped, then the
/// retained blocks.
fn bounded_payload(tail: &TailKeep, item: &str, budget: usize, separator: &str) -> String {
    let mut payload = String::with_capacity(tail.kept_bytes + 256);
    if tail.dropped_items > 0 {
        payload.push_str(&truncation_marker(
            item,
            tail.dropped_items,
            tail.dropped_bytes,
            budget,
        ));
        payload.push('\n');
    }
    payload.push_str(&tail.blocks.join(separator));
    payload
}

/// Build the current-state prompt under `max_bytes` of payload.
///
/// The session text is redacted upstream in [`crate::rollout::message_to_text`],
/// which is the only path that reaches this prompt. The payload is filled from
/// the newest message backwards and the oldest messages are dropped; the drop is
/// reported in the returned [`BoundedPrompt`] and written into the prompt.
pub fn build_state_prompt_bounded(messages: &[RolloutMessage], max_bytes: usize) -> BoundedPrompt {
    let tail = tail_keep(
        messages,
        |_, m| crate::rollout::message_to_text(m).unwrap_or_default(),
        |m: &RolloutMessage| m.content.len(),
        "\n",
        max_bytes.saturating_sub(MARKER_RESERVE),
    );
    let payload = bounded_payload(&tail, "message", max_bytes, "\n");
    BoundedPrompt {
        bytes: payload.len(),
        text: STATE_PROMPT.replace("{conversation}", &payload),
        dropped_bytes: tail.dropped_bytes,
        dropped_items: tail.dropped_items,
    }
}

/// Build the prompt for the current-state LLM call at [`MAX_STATE_BYTES`].
pub fn build_state_prompt(messages: &[RolloutMessage]) -> String {
    build_state_prompt_bounded(messages, MAX_STATE_BYTES).text
}

/// Build the user-goals prompt under `max_bytes` of payload.
///
/// This is the second of the two prompts the recall path sends, and it does
/// *not* go through the session renderer: it takes the extracted user strings
/// directly, so it redacts them itself. Skipping this is how a key a user
/// pasted into a prompt would reach the vendor through the back door while the
/// state prompt looked clean. The tail is kept for the same reason as the state
/// prompt: the goals and corrections still live are the newest ones.
pub fn build_goals_prompt_bounded(user_messages: &[String], max_bytes: usize) -> BoundedPrompt {
    let tail = tail_keep(
        user_messages,
        // The ordinal is the message's real position in the session, so a
        // dropped head does not renumber what is left.
        |i, msg: &String| format!("{}. {}", i + 1, redact_secrets(msg)),
        |msg: &String| msg.len(),
        "\n\n",
        max_bytes.saturating_sub(MARKER_RESERVE),
    );
    let payload = bounded_payload(&tail, "user message", max_bytes, "\n\n");
    BoundedPrompt {
        bytes: payload.len(),
        text: GOALS_PROMPT.replace("{messages}", &payload),
        dropped_bytes: tail.dropped_bytes,
        dropped_items: tail.dropped_items,
    }
}

/// Build the prompt for the user-goals LLM call at [`MAX_GOALS_BYTES`].
pub fn build_goals_prompt(user_messages: &[String]) -> String {
    build_goals_prompt_bounded(user_messages, MAX_GOALS_BYTES).text
}

/// Build the recent rollouts table as markdown from a scoped listing.
/// `current_session_id` is marked as "SUMMARISED" in the table.
/// `hours_back` names the window the listing already applied.
///
/// The listing holds at most [`LISTING_ROW_CAP`] rows (the cap is the
/// listing's, applied where the data lives, not the table's); the rows the
/// window holds but the table did not print are stated in the table itself.
pub fn build_recent_rollouts_table(
    listing: &SessionListing,
    current_session_id: Option<&str>,
    hours_back: u64,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!("## Recent Rollouts ({}h)", hours_back));
    lines.push(String::new());

    if listing.window_count == 0 {
        lines.push("No recent sessions found.".to_string());
        return lines.join("\n");
    }

    lines.push(
        "| Session | Title | Size | Lines | User | Asst | Tool | Compaction | Notes |".to_string(),
    );
    lines.push(
        "|---------|-------|------|-------|------|------|------|------------|-------|".to_string(),
    );

    for s in &listing.sessions {
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
            s.session_id,
            s.title,
            size_str,
            s.line_count,
            s.user_count,
            s.assistant_count,
            s.tool_count,
            compaction,
            notes
        ));
    }

    let omitted = listing.window_count.saturating_sub(listing.sessions.len());
    if omitted > 0 {
        lines.push(String::new());
        lines.push(format!(
            "- {omitted} older session(s) in this window are not listed: the table holds the \
             {LISTING_ROW_CAP} most recent rows."
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
