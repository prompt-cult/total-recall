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

/// The fixed orientation warning appended to every prior-period summary.
///
/// Summarising in-flight work in more detail is a fool's errand — it cannot
/// be perfect — and the feature is a moral hazard: a model that trusts the
/// summary stops double-checking. The warning makes re-orientation the
/// model's job instead of the summary's.
pub const ORIENTATION_WARNING: &str = "## Orientation Warning\n\nThis summary does not attempt to show what was in-flight or the next steps. Those are known from the todo list, the user's most recent inputs, and the commit history. Be sure to take stock and get your bearings before picking up the work.";

/// The cap on each todo-log list (opened, closed, transitions): the most
/// recent entries are kept and the held-back count is stated, so a long
/// period's todo history cannot spend the whole response budget on its own.
pub const TODO_LOG_LIST_CAP: usize = 200;

/// How many compaction points the FYI footer names: the last few, with the
/// counts since each. Nothing more elaborate — the model can mine the logs
/// itself for anything deeper.
pub const COMPACTION_FOOTER_CAP: usize = 3;

/// How many recent user messages the churn note reads in as the human's
/// recent steers.
pub const CHURN_STEER_COUNT: usize = 3;

/// The byte cap on one steer or commit line.
const LINE_TRUNCATE: usize = 300;

/// `1 message` / `2 messages` — the footer's counts read as English.
fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Keep the most recent `cap` lines of a rendered list, returning them and
/// the held-back count.
fn capped_recent(lines: Vec<String>, cap: usize) -> (Vec<String>, usize) {
    let total = lines.len();
    if total <= cap {
        return (lines, 0);
    }
    (lines[total - cap..].to_vec(), total - cap)
}

/// The human's recent steers: the newest `count` user messages, redacted and
/// truncated — the raw steering signal, unfiltered by any LLM extraction.
/// The churn note reads these in, since the distractions are usually steered.
pub fn recent_steers(user_messages: &[String], count: usize) -> Vec<String> {
    user_messages
        .iter()
        .rev()
        .take(count)
        .rev()
        .map(|m| crate::rollout::truncate_chars(&redact_secrets(m), LINE_TRUNCATE).to_string())
        .collect()
}

/// Build the todo log section of a prior-period summary: items opened and
/// closed in the period with timings, the state-change history, and the
/// churn note. `opened`/`closed` come from
/// [`crate::todo_history::opened_closed_counts`]; `baseline` is the size of
/// a restated first flush the counts excluded — the Opened list excludes it
/// too, so the headwords and the churn note never contradict the list.
/// `recent_steers` feed the churn note.
pub fn build_todo_log_section(
    events: &[crate::todo_history::TodoEditEvent],
    opened: usize,
    closed: usize,
    baseline: usize,
    steers: &[String],
) -> String {
    let mut lines = vec!["## Todo Log (this period)".to_string(), String::new()];

    if events.is_empty() {
        lines.push("No todo activity in this period.".to_string());
        return lines.join("\n");
    }

    if opened > closed {
        if steers.is_empty() {
            lines.push(format!(
                "**Churn note:** {} opened vs {} closed in this period — starting new \
                 things without finishing old ones may be causing churn. Recent \
                 steers: none on record in this session.",
                plural(opened, "item"),
                plural(closed, "item")
            ));
        } else {
            lines.push(format!(
                "**Churn note:** {} opened vs {} closed in this period — starting new \
                 things without finishing old ones may be causing churn. Recent steers:",
                plural(opened, "item"),
                plural(closed, "item")
            ));
            for steer in steers {
                lines.push(format!("- \"{steer}\""));
            }
        }
    }

    let render = |e: &crate::todo_history::TodoEditEvent| {
        let status = e.status.clone().unwrap_or_else(|| e.action.clone());
        format!("{} — {} ({})", e.ts, e.todo, status)
    };

    let opened_events: Vec<String> = events
        .iter()
        .filter(|e| e.action == "added")
        .skip(baseline)
        .map(&render)
        .collect();
    let closed_events: Vec<String> = events
        .iter()
        .filter(|e| crate::todo_history::is_terminal_action(&e.action))
        .map(&render)
        .collect();

    let (opened_lines, opened_held) = capped_recent(opened_events, TODO_LOG_LIST_CAP);
    lines.push(format!("Opened ({}):", opened_lines.len()));
    for line in &opened_lines {
        lines.push(format!("- {line}"));
    }
    if opened_held > 0 {
        lines.push(format!("- ({opened_held} older opened items not shown)"));
    }

    lines.push(String::new());
    let (closed_lines, closed_held) = capped_recent(closed_events, TODO_LOG_LIST_CAP);
    lines.push(format!("Closed ({}):", closed_lines.len()));
    for line in &closed_lines {
        lines.push(format!("- {line}"));
    }
    if closed_held > 0 {
        lines.push(format!("- ({closed_held} older closed items not shown)"));
    }

    lines.push(String::new());
    let transitions: Vec<String> = events
        .iter()
        .map(|e| format!("{} — {}: {}", e.ts, e.action, e.todo))
        .collect();
    let (transition_lines, transitions_held) = capped_recent(transitions, TODO_LOG_LIST_CAP);
    let held = if transitions_held > 0 {
        format!(", {} held back", transitions_held)
    } else {
        String::new()
    };
    lines.push(format!(
        "State transitions ({} events{held}):",
        transition_lines.len()
    ));
    for line in &transition_lines {
        lines.push(format!("- {line}"));
    }

    lines.join("\n")
}

/// The todo log section on a harness whose todowrite flushes cannot be read.
/// Never a silent empty list that reads as "no todos": the section names the
/// harness, states why the log is unavailable, and names the record that
/// remains.
pub fn build_todo_log_unavailable(harness: &str, reason: &str) -> String {
    format!(
        "## Todo Log (this period)\n\nUnavailable on the {harness} harness: \
         {reason} The plan/todo files above and the commit history are the \
         record."
    )
}

/// Build the commits section: the git commit tool calls of the period, with
/// their timestamps. The detection matches the profile's `GitCommit` events:
/// a tool-call summary line containing `git commit`.
pub fn build_commits_section(messages: &[RolloutMessage]) -> String {
    let mut lines = vec!["## Commits in this Period".to_string(), String::new()];
    let mut commits: Vec<String> = Vec::new();
    for msg in messages {
        for tc in &msg.tool_calls_summary {
            if tc.contains("git commit") {
                let ts = msg.timestamp.as_deref().unwrap_or("unknown time");
                commits.push(format!(
                    "{ts} — {}",
                    crate::rollout::truncate_chars(tc, LINE_TRUNCATE)
                ));
            }
        }
    }
    if commits.is_empty() {
        lines.push("No commits in this period.".to_string());
    } else {
        for c in &commits {
            lines.push(format!("- {c}"));
        }
    }
    lines.join("\n")
}

/// Build the compaction-points FYI footer: the timestamps of the last few
/// compaction points with the message and line counts since each. The
/// summary's period is the work since the last compaction boundary — the
/// work after that boundary; a session with no compaction points has no
/// earlier period, so the period is the whole session. `markers` are the
/// boundary indices of the full session, from
/// [`RolloutAdapter::compaction_markers`] — the adapter decides what a real
/// boundary is, and prose cannot forge one.
pub fn build_compaction_footer(full_messages: &[RolloutMessage], markers: &[usize]) -> String {
    let mut lines = vec!["## Compaction Points (FYI)".to_string(), String::new()];
    lines.push(
        "Period: the work since the last compaction boundary — the work after that boundary."
            .to_string(),
    );

    if markers.is_empty() {
        lines.push(String::new());
        lines.push(
            "No compaction points in this session — the period is the whole session.".to_string(),
        );
        lines.push(String::new());
        lines.push(
            "FYI only — nothing here is summarised; mine the logs for anything deeper.".to_string(),
        );
        return lines.join("\n");
    }

    let shown = &markers[markers.len().saturating_sub(COMPACTION_FOOTER_CAP)..];
    let held_back = markers.len() - shown.len();
    lines.push(String::new());
    for (j, &idx) in shown.iter().enumerate() {
        let msg = &full_messages[idx];
        let ts = msg.timestamp.as_deref().unwrap_or("unknown time");
        let since = &full_messages[idx + 1..];
        let messages_since = since.len();
        let lines_since: usize = since.iter().map(|m| m.content.lines().count()).sum();
        let annotation = if j + 1 == shown.len() {
            " (the current period starts here)"
        } else {
            ""
        };
        lines.push(format!(
            "- {ts} — {}, {} since{annotation}",
            plural(messages_since, "message"),
            plural(lines_since, "line")
        ));
    }
    if held_back > 0 {
        lines.push(format!("- ({held_back} older compaction points not shown)"));
    }
    lines.push(String::new());
    lines.push(
        "FYI only — nothing here is summarised; mine the logs for anything deeper.".to_string(),
    );
    lines.join("\n")
}

/// Assemble the full prior-period summary from its parts.
///
/// Ordering is deliberate for autoregressive LLMs:
/// 1. Recent Rollouts table (metadata -- sets the scene, reference data)
/// 2. Plan Files (metadata -- anchors the current plan)
/// 3. Todo Log (ground truth -- what was in flight, with timings)
/// 4. Commits (ground truth -- what actually landed)
/// 5. Current State (LLM summary -- untrusted; see the warning)
/// 6. User Goals/Tasks/Steers (LLM summary -- the steering signal)
/// 7. Compaction Points FYI footer (metadata -- the period's boundaries)
/// 8. Instructions (call to action)
/// 9. Orientation Warning (fixed, last -- re-orientation is the model's job)
///
/// The ground truth comes before the LLM summaries so the model reads the
/// authoritative record first; the warning is appended last because the
/// final tokens have the strongest influence on what the model does next.
#[allow(clippy::too_many_arguments)]
pub fn build_prior_period_output(
    state_summary: &str,
    goals_summary: &str,
    rollouts_table: &str,
    plan_files: &str,
    todo_log: &str,
    commits: &str,
    compaction_footer: &str,
) -> String {
    let sections = vec![
        // 1. Metadata: recent rollouts (sets the scene)
        rollouts_table.to_string(),
        String::new(),
        // 2. Metadata: plan/todo files (anchors current plan)
        plan_files.to_string(),
        String::new(),
        // 3. Ground truth: the period's todo log
        todo_log.to_string(),
        String::new(),
        // 4. Ground truth: the period's commits
        commits.to_string(),
        String::new(),
        // 5. Substance: current state (what just happened)
        "## Current State".to_string(),
        String::new(),
        state_summary.to_string(),
        String::new(),
        // 6. Substance: user goals/tasks/steers (steering signal)
        "## User Goals, Tasks, and Steers".to_string(),
        String::new(),
        goals_summary.to_string(),
        String::new(),
        // 7. Metadata: compaction points (the period's boundaries)
        compaction_footer.to_string(),
        String::new(),
        // 8. Call to action (last, strongest influence on next token)
        INSTRUCTIONS.to_string(),
        String::new(),
        // 9. The fixed orientation warning, appended to every output
        ORIENTATION_WARNING.to_string(),
    ];

    sections.join("\n")
}
