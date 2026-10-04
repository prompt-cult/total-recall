//! The two LLM prompts of the recall path are byte-bounded, tail-kept and
//! marked. Written RED against unbounded prompt builders.
//!
//! What is under test is the *input* leg of `total_recall`, before any vendor
//! call: the prompts are built from a session that has already been read into
//! memory, and nothing here touches a network, a provider or the live store.

mod common;

use std::time::Instant;

use common::scratch::scratch;
use rusqlite::Connection;
use total_recall::{
    LISTING_ROW_CAP, MAX_GOALS_BYTES, MAX_STATE_BYTES, OpenCodeAdapter, RolloutAdapter,
    RolloutMessage, SessionListing, SessionSummary, build_goals_prompt, build_goals_prompt_bounded,
    build_recent_rollouts_table, build_state_prompt, build_state_prompt_bounded,
    build_structured_prompt, message_to_text, messages_to_text,
};

/// Byte budget the recall prompts are held to. Mirrors `MAX_STATE_BYTES` /
/// `MAX_GOALS_BYTES`; the tie between the two is asserted in
/// `the_published_budgets_are_the_ones_under_test`.
const BUDGET: usize = 200 * 1024;
/// Slack for the fixed prompt wrapper (instructions plus the marker line).
const WRAPPER_SLACK: usize = 4 * 1024;

fn message(role: &str, content: String) -> RolloutMessage {
    RolloutMessage {
        role: role.to_string(),
        content,
        thinking: None,
        tool_calls_summary: Vec::new(),
        timestamp: None,
        injected: false,
    }
}

/// One synthetic message body: several lines, so a tail cut that lands on a
/// line boundary is distinguishable from one that lands mid-character.
fn body(tag: &str, filler: usize) -> String {
    let mut s = String::with_capacity(filler + tag.len() + 64);
    for line in 0..4 {
        s.push_str(tag);
        s.push_str("-line-");
        s.push_str(&line.to_string());
        s.push(' ');
        s.push_str(&"lorem ipsum dolor sit amet ".repeat(filler / 27));
        s.push('\n');
    }
    s
}

/// A synthetic session of `count` messages, several MB in total.
fn synthetic_session(count: usize, filler: usize) -> Vec<RolloutMessage> {
    (0..count)
        .map(|i| {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            message(role, body(&format!("msg{i:06}"), filler))
        })
        .collect()
}

#[test]
fn state_prompt_is_bounded_keeps_the_newest_and_names_what_it_dropped() {
    let messages = synthetic_session(4000, 400);
    let prompt = build_state_prompt(&messages);

    assert!(
        prompt.len() <= BUDGET + WRAPPER_SLACK,
        "state prompt is {} bytes, over the {} byte budget plus wrapper slack: the \
         prompt input is unbounded",
        prompt.len(),
        BUDGET
    );
    assert!(
        prompt.contains("msg003999-line-0"),
        "the newest message must survive: a state summary is about now"
    );
    assert!(
        !prompt.contains("msg000000-line-0"),
        "the oldest message must be the content that goes"
    );
    assert!(
        prompt.contains("bytes dropped"),
        "a bounded prompt must say so in-band: {}",
        &prompt[..prompt.len().min(400)]
    );
}

#[test]
fn goals_prompt_is_bounded_keeps_the_newest_and_names_what_it_dropped() {
    let users: Vec<String> = (0..4000)
        .map(|i| body(&format!("goal{i:06}"), 400))
        .collect();
    let prompt = build_goals_prompt(&users);

    assert!(
        prompt.len() <= BUDGET + WRAPPER_SLACK,
        "goals prompt is {} bytes, over the {} byte budget plus wrapper slack",
        prompt.len(),
        BUDGET
    );
    assert!(
        prompt.contains("goal003999-line-0"),
        "the newest user message must survive"
    );
    assert!(
        !prompt.contains("goal000000-line-0"),
        "the oldest user message must be the content that goes"
    );
    assert!(
        prompt.contains("bytes dropped"),
        "a bounded prompt must say so in-band"
    );
}

#[test]
fn a_session_inside_the_budget_is_never_marked() {
    let messages = synthetic_session(20, 100);
    let users: Vec<String> = (0..20).map(|i| body(&format!("goal{i:06}"), 100)).collect();

    let state = build_state_prompt(&messages);
    let goals = build_goals_prompt(&users);

    assert!(!state.contains("bytes dropped"), "{state}");
    assert!(!goals.contains("bytes dropped"), "{goals}");
    assert!(
        state.contains("msg000019-line-0"),
        "oldest must survive too"
    );
    assert!(
        goals.contains("goal000000-line-0"),
        "oldest must survive too"
    );
    assert!(
        state.contains("## Accomplished") && goals.contains("### Goals"),
        "the instruction body must survive"
    );
}

#[test]
fn a_prompt_over_the_budget_still_carries_the_instruction_body() {
    let messages = synthetic_session(4000, 400);
    let prompt = build_state_prompt(&messages);
    assert!(prompt.contains("## Accomplished"), "{prompt}");
    assert!(prompt.contains("## Key Decisions/Constraints"), "{prompt}");
}

/// The pre-LLM leg must not grow with the session. A generous bound: the point
/// is that building both prompts over several MB is milliseconds of formatting,
/// not a scan that scales with the store.
#[test]
fn prompt_build_over_a_multi_megabyte_session_is_fast() {
    let messages = synthetic_session(20_000, 400);
    let users: Vec<String> = (0..20_000)
        .map(|i| body(&format!("goal{i:06}"), 400))
        .collect();

    let t0 = Instant::now();
    let state = build_state_prompt(&messages);
    let goals = build_goals_prompt(&users);
    let elapsed = t0.elapsed();

    assert!(state.len() <= BUDGET + WRAPPER_SLACK);
    assert!(goals.len() <= BUDGET + WRAPPER_SLACK);
    assert!(
        elapsed.as_secs_f64() < 2.0,
        "building both prompts over {} messages took {:?}",
        messages.len(),
        elapsed
    );
}

/// The two pre-LLM costs of one `total_recall` call, measured against each
/// other: formatting the two prompts, and the `list_sessions` scan behind the
/// rollouts table. The store is sized like the operator's (3,365 sessions) but
/// small enough for a test; the scan is reported per MB so it scales to a store
/// of any size, and the prompt build is measured across four session sizes to
/// show what the byte budget did to it.
#[test]
fn pre_llm_split_prompt_build_vs_session_scan() {
    const SESSIONS: usize = 3365;
    const PARTS_PER_SESSION: usize = 8;
    const PART_BYTES: usize = 4096;

    let root = scratch("recall_scan");
    let db = root.join("fixture.db");
    let conn = Connection::open(&db).expect("fixture db");
    conn.execute_batch(
        "PRAGMA journal_mode=OFF;
         PRAGMA synchronous=OFF;
         CREATE TABLE session (
            id text PRIMARY KEY, parent_id text, directory text, title text,
            time_created integer NOT NULL, time_updated integer NOT NULL);
        CREATE TABLE message (
            id text PRIMARY KEY, session_id text NOT NULL,
            time_created integer NOT NULL, time_updated integer NOT NULL,
            data text NOT NULL);
        CREATE TABLE part (
            id text PRIMARY KEY, message_id text NOT NULL, session_id text NOT NULL,
            time_created integer NOT NULL, time_updated integer NOT NULL,
            data text NOT NULL);",
    )
    .expect("fixture schema");
    let now = 1_700_000_000_000i64;
    let tx = conn.unchecked_transaction().expect("fixture tx");
    {
        let mut ins_session = tx
            .prepare(
                "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
                      VALUES (?1, NULL, ?2, ?3, ?4, ?4)",
            )
            .expect("prepare session");
        let mut ins_message = tx
            .prepare(
                "INSERT INTO message (id, session_id, time_created, time_updated, data)
                      VALUES (?1, ?2, ?3, ?3, ?4)",
            )
            .expect("prepare message");
        let mut ins_part = tx
            .prepare(
                "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
                      VALUES (?1, ?2, ?2, ?3, ?3, ?4)",
            )
            .expect("prepare part");
        for s in 0..SESSIONS {
            let sid = format!("ses_{s:032x}");
            ins_session
                .execute(rusqlite::params![
                    &sid,
                    "/tmp/synthetic",
                    &format!("synthetic session {s}"),
                    now
                ])
                .expect("insert session");
            ins_message
                .execute(rusqlite::params![
                    format!("msg{s}"),
                    &sid,
                    now,
                    "{\"role\":\"assistant\",\"time\":{\"created\":1}}"
                ])
                .expect("insert message");
            for p in 0..PARTS_PER_SESSION {
                let text = format!(
                    "synthetic rollout body {s}/{p} {}",
                    "x".repeat(PART_BYTES - 32)
                );
                ins_part
                    .execute(rusqlite::params![
                        format!("part{s}_{p}"),
                        format!("msg{s}"),
                        now,
                        serde_json::json!({"type": "text", "text": text}).to_string()
                    ])
                    .expect("insert part");
            }
        }
    }
    tx.commit().expect("commit fixture");
    drop(conn);
    let store_bytes = std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0);

    let adapter = OpenCodeAdapter::with_root(&db);

    // Leg 1: the listing behind the rollouts table. The full scan is the
    // defect #21 measured — aggregates for every message and part row in the
    // store, set by the size of the store and not the size of the answer.
    // The scoped listing is what the call pays now: the window and the row
    // cap are pushed into SQL, so its cost is set by the answer, and the
    // table holds its bounded rows plus the held-back count.
    let t0 = Instant::now();
    let full_scan = adapter.list_sessions();
    let scan = t0.elapsed();
    let t1 = Instant::now();
    let listing = adapter.list_sessions_scoped(0, None);
    let scoped = t1.elapsed();
    let table = build_recent_rollouts_table(&listing, Some("ses_"), 0);

    // The unbounded prompt, for scale: the same renderer with no byte budget,
    // which is what the call used to send.
    let unbounded_messages = synthetic_session(4000, 400);
    let t_unbounded = Instant::now();
    let unbounded = build_structured_prompt(&unbounded_messages);
    let unbounded_time = t_unbounded.elapsed();

    // Leg 2: both prompts, at four session sizes. Before the byte budget this
    // line grew with the session; now the prompt is the same size every time
    // and so is the work.
    let mut builds = Vec::new();
    let mut prompts = (0usize, 0usize);
    for count in [1000usize, 4000, 20_000, 80_000] {
        let messages = synthetic_session(count, 400);
        let users: Vec<String> = (0..count)
            .map(|i| body(&format!("goal{i:06}"), 400))
            .collect();
        let t1 = Instant::now();
        let state = build_state_prompt_bounded(&messages, MAX_STATE_BYTES);
        let goals = build_goals_prompt_bounded(&users, MAX_GOALS_BYTES);
        let elapsed = t1.elapsed();
        println!(
            "prompt build over {count:>6} messages / {} user strings: state {} B ({} messages dropped), goals {} B ({} dropped) in {:.4}s",
            users.len(),
            state.bytes,
            state.dropped_items,
            goals.bytes,
            goals.dropped_items,
            elapsed.as_secs_f64(),
        );
        builds.push(elapsed.as_secs_f64());
        prompts = (state.bytes, goals.bytes);
    }

    let mb = store_bytes as f64 / 1_000_000.0;
    println!(
        "full scan: {SESSIONS} sessions, {mb:.1} MB store in {:.3}s = {:.0} MB/s; scoped listing (unbounded window) {:.4}s for {} of {SESSIONS} rows",
        scan.as_secs_f64(),
        mb / scan.as_secs_f64(),
        scoped.as_secs_f64(),
        listing.sessions.len(),
    );
    println!(
        "rollouts table {} rows / {} bytes (cap {LISTING_ROW_CAP})",
        table.lines().filter(|l| l.starts_with("| ses_")).count(),
        table.len(),
    );
    println!(
        "unbounded state prompt over the same 4000 messages: {} B in {:.4}s",
        unbounded.len(),
        unbounded_time.as_secs_f64(),
    );
    println!(
        "prompt bytes are size-independent at {}/{} B across a 80x range of session sizes; \
         slowest build {:.4}s vs scan {:.3}s",
        prompts.0,
        prompts.1,
        builds.iter().cloned().fold(0.0, f64::max),
        scan.as_secs_f64(),
    );

    assert_eq!(full_scan.len(), SESSIONS);
    assert_eq!(listing.window_count, SESSIONS);
    assert!(prompts.0 <= MAX_STATE_BYTES && prompts.1 <= MAX_GOALS_BYTES);
    assert!(
        builds.iter().all(|b| *b < 1.0),
        "building both prompts must not scale with session size: {builds:?}"
    );
    assert_eq!(listing.sessions.len(), LISTING_ROW_CAP.min(SESSIONS));
    assert!(
        table.lines().filter(|l| l.starts_with("| ses_")).count() <= LISTING_ROW_CAP,
        "the rollouts table must hold at most {LISTING_ROW_CAP} rows"
    );
}

/// The budgets the tests hold the prompts to are the published ones.
#[test]
fn the_published_budgets_are_the_ones_under_test() {
    assert_eq!(MAX_STATE_BYTES, BUDGET);
    assert_eq!(MAX_GOALS_BYTES, BUDGET);
}

/// The counts the tool reports are the counts the builder made: the dropped
/// bytes are the raw source text that is absent from the prompt, and the
/// dropped items are exactly the oldest ones.
#[test]
fn the_reported_drops_match_the_text_that_is_missing() {
    let messages = synthetic_session(4000, 400);
    let state = build_state_prompt_bounded(&messages, MAX_STATE_BYTES);
    assert!(state.dropped_items > 0);
    assert!(
        state
            .text
            .contains(&format!("{} bytes dropped", state.dropped_bytes)),
        "the marker must name the dropped bytes: {}",
        state.text.lines().nth(1).unwrap_or("")
    );
    assert!(
        state
            .text
            .contains(&format!("{} oldest message", state.dropped_items)),
        "the marker must name the dropped messages"
    );
    // The dropped items are a prefix of the session, and the reported byte
    // count is their raw content length.
    let dropped_raw: usize = messages[..state.dropped_items]
        .iter()
        .map(|m| m.content.len())
        .sum();
    assert_eq!(state.dropped_bytes, dropped_raw);
    assert!(state.bytes <= MAX_STATE_BYTES);

    let users: Vec<String> = (0..4000)
        .map(|i| body(&format!("goal{i:06}"), 400))
        .collect();
    let goals = build_goals_prompt_bounded(&users, MAX_GOALS_BYTES);
    assert!(goals.dropped_items > 0);
    assert!(
        goals
            .text
            .contains(&format!("{} bytes dropped", goals.dropped_bytes))
    );
    assert_eq!(
        goals.dropped_bytes,
        users[..goals.dropped_items]
            .iter()
            .map(|u| u.len())
            .sum::<usize>()
    );
    assert!(goals.bytes <= MAX_GOALS_BYTES);
    // Ordinals stay the messages' real positions, so a dropped head does not
    // renumber what is left.
    assert!(
        goals.text.contains(&format!("{}. goal003999", 4000)),
        "{}",
        &goals.text[goals.text.len().saturating_sub(200)..]
    );
}

/// The per-message renderer the tail walk uses reproduces the whole-session
/// renderer byte for byte, including messages that contribute nothing.
#[test]
fn the_per_message_renderer_reproduces_the_whole_session_renderer() {
    let messages = vec![
        message("user", "first".to_string()),
        message("assistant", String::new()),
        message("tool", "tool output line".to_string()),
        message("user", "second\nwith a newline".to_string()),
        message("assistant", String::new()),
    ];
    let whole = messages_to_text(&messages);
    let per_message: String = messages
        .iter()
        .filter_map(message_to_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(whole, per_message);
}

/// A session that fits is passed through whole, and a session that does not
/// loses whole messages from its oldest end: the boundary lands between two
/// messages, never inside one.
#[test]
fn the_boundary_is_whole_messages() {
    let template = message(
        "user",
        format!("{}\n{}", "a".repeat(1400), "b".repeat(1400)),
    );
    let block = message_to_text(&template)
        .expect("a message with content renders")
        .len();
    // Every message costs the same, so the number that fits is exact.
    let fits = MAX_STATE_BYTES / (block + 1);
    let messages: Vec<RolloutMessage> = (0..fits + 3)
        .map(|i| message("user", format!("{i:04} {}", template.content)))
        .collect();

    let state = build_state_prompt_bounded(&messages, MAX_STATE_BYTES);
    assert_eq!(state.dropped_items, 3, "the three oldest go, whole");
    let dropped_raw: usize = messages[..3].iter().map(|m| m.content.len()).sum();
    assert_eq!(state.dropped_bytes, dropped_raw);
    assert!(
        !state.text.contains("0000 aaaa"),
        "the oldest message must be gone"
    );
    assert!(
        state.text.contains(&format!("{:04} aaaa", fits + 2)),
        "the newest message must be present"
    );
    assert!(state.bytes <= MAX_STATE_BYTES, "{} bytes", state.bytes);
}

/// The marker is charged to the budget it is written into: the longest one the
/// builder can write still fits the reserve, so a marked prompt is bounded.
#[test]
fn the_boundary_marker_fits_its_reserve() {
    let messages: Vec<RolloutMessage> = (0..10)
        .map(|i| message("user", format!("{i}{}", "a".repeat(1400))))
        .collect();
    let tiny = build_state_prompt_bounded(&messages, 1024);
    let marker = tiny
        .text
        .lines()
        .find(|l| l.starts_with("[total-recall:"))
        .unwrap_or_else(|| panic!("no marker in {}", tiny.text));
    assert!(marker.contains("prompt truncated"), "{marker}");
    // The reserve is the crate's, pinned here through the smallest budget a
    // caller can pass and the marker the builder then writes.
    assert!(
        marker.len() <= 256,
        "the marker is {} bytes, over the 256-byte reserve: {marker}",
        marker.len()
    );
}

/// The rollouts table names the rows the window holds but it did not print:
/// the cap is the listing's (already applied where the data lives), and the
/// table states the held-back count.
#[test]
fn the_rollouts_table_is_capped_and_says_so() {
    let sessions: Vec<SessionSummary> = (0..LISTING_ROW_CAP + 5)
        .map(|i| summary(&format!("session_{i:04}")))
        .collect();
    // The shape a scoped listing produces: the 200 most recent rows carried,
    // the whole window counted.
    let listing = SessionListing {
        sessions: sessions[..LISTING_ROW_CAP].to_vec(),
        window_count: sessions.len(),
    };
    let table = build_recent_rollouts_table(&listing, None, 24);
    assert_eq!(
        table
            .lines()
            .filter(|l| l.starts_with("| session_"))
            .count(),
        LISTING_ROW_CAP
    );
    assert!(table.contains("5 older session(s)"), "{table}");

    let under = build_recent_rollouts_table(
        &SessionListing {
            sessions: sessions[..3].to_vec(),
            window_count: 3,
        },
        None,
        24,
    );
    assert_eq!(
        under
            .lines()
            .filter(|l| l.starts_with("| session_"))
            .count(),
        3
    );
    assert!(!under.contains("not listed"), "{under}");
}

fn summary(session_id: &str) -> SessionSummary {
    SessionSummary {
        session_id: session_id.to_string(),
        title: "t".to_string(),
        start_time: String::new(),
        end_time: String::new(),
        file_size: 1024,
        line_count: 10,
        user_count: 1,
        assistant_count: 1,
        tool_count: 0,
        has_compaction: false,
        directory: None,
        parent_session_id: None,
        child_sessions: Vec::new(),
        has_tantivy_index: false,
        aliases: Vec::new(),
        read_error: None,
    }
}
