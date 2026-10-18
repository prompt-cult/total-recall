//! #31: `prior_period_summary` — an orientation summary of a prior period
//! of work. The pure builders (todo log with the churn note, the
//! period's commits, the compaction-points FYI footer, the orientation
//! warning, the assembly) and the boundary behaviour of the period
//! selection. Written RED, before the implementation existed.

mod common;

use common::scratch::scratch;
use total_recall::rollout::slice_from_compaction;
use total_recall::{
    MockAdapter, RolloutAdapter, RolloutMessage,
    recall::{
        ORIENTATION_WARNING, build_commits_section, build_compaction_footer,
        build_prior_period_output, build_todo_log_section, build_todo_log_unavailable,
        recent_steers,
    },
    todo_history::{TodoItemState, TodoWrite, diff_todo_states, opened_closed_counts},
};

fn item(content: &str, status: &str) -> TodoItemState {
    TodoItemState {
        content: content.to_string(),
        status: status.to_string(),
        priority: None,
    }
}

fn write(ts: &str, todos: Vec<TodoItemState>) -> TodoWrite {
    TodoWrite {
        timestamp: Some(ts.to_string()),
        todos,
    }
}

fn message(role: &str, content: &str, ts: Option<&str>, calls: &[&str]) -> RolloutMessage {
    RolloutMessage {
        role: role.to_string(),
        content: content.to_string(),
        thinking: None,
        tool_calls_summary: calls.iter().map(|c| c.to_string()).collect(),
        timestamp: ts.map(str::to_string),
        injected: false,
    }
}

// --- Orientation warning -----------------------------------------------------

#[test]
fn the_orientation_warning_is_appended_last_to_every_output() {
    let output = build_prior_period_output(
        "STATE", "GOALS", "ROLLOUTS", "PLANS", "TODOLOG", "COMMITS", "FOOTER",
    );
    assert!(
        output.contains(ORIENTATION_WARNING),
        "the warning must appear verbatim: {output}"
    );
    let warning_pos = output.find(ORIENTATION_WARNING).unwrap();
    let instructions_pos = output.find("## Instructions").unwrap();
    assert!(
        instructions_pos < warning_pos,
        "the warning comes after the instructions"
    );
    let tail = &output[warning_pos + ORIENTATION_WARNING.len()..];
    assert!(
        tail.trim().is_empty(),
        "nothing follows the warning: {tail:?}"
    );
}

#[test]
fn the_orientation_warning_is_there_even_when_every_section_is_empty() {
    let output = build_prior_period_output("", "", "", "", "", "", "");
    assert!(output.contains(ORIENTATION_WARNING), "{output}");
    assert!(output.contains("## Instructions"), "{output}");
}

#[test]
fn the_warning_text_is_the_fixed_sentence_from_the_issue() {
    assert!(ORIENTATION_WARNING.contains("does not attempt to show what was in-flight"));
    assert!(
        ORIENTATION_WARNING
            .contains("the todo list, the user's most recent inputs, and the commit history")
    );
    assert!(
        ORIENTATION_WARNING.contains("take stock and get your bearings before picking up the work")
    );
}

// --- Todo log ----------------------------------------------------------------

#[test]
fn the_todo_log_lists_opened_and_closed_items_with_timings() {
    let writes = vec![
        write(
            "2026-10-16T09:00:00Z",
            vec![
                item("item00: first", "pending"),
                item("item01: second", "in_progress"),
            ],
        ),
        write(
            "2026-10-16T09:05:00Z",
            vec![
                item("item00: first", "in_progress"),
                item("item01: second", "in_progress"),
            ],
        ),
        write(
            "2026-10-16T09:10:00Z",
            vec![
                item("item00: first", "completed"),
                item("item01: second", "in_progress"),
            ],
        ),
    ];
    let events = diff_todo_states(&writes);
    let (opened, closed) = opened_closed_counts(&writes, false);
    assert_eq!((opened, closed), (2, 1), "two adds, one completion");

    let section = build_todo_log_section(&events, opened, closed, 0, &[]);
    assert!(section.contains("## Todo Log"), "{section}");
    // No opened/closed summary line: the lists carry the counts, and a
    // contradicting headline above them is exactly the repetition that
    // misled (the count can exclude a carried-in baseline the list shows).
    assert!(
        !section.contains("opened, "),
        "the summary headline is gone; the lists speak: {section}"
    );
    assert!(section.contains("Opened (2):"), "{section}");
    assert!(
        section.contains("2026-10-16T09:00:00Z — item00: first (pending)"),
        "{section}"
    );
    assert!(
        section.contains("2026-10-16T09:10:00Z — item00: first (completed)"),
        "{section}"
    );
    assert!(section.contains("Closed (1):"), "{section}");
    assert!(section.contains("State transitions"), "{section}");
    assert!(
        section.contains("2026-10-16T09:05:00Z — in_progress: item00: first"),
        "{section}"
    );
}

#[test]
fn the_opened_list_excludes_the_restated_baseline_so_the_counts_agree() {
    // On a sliced period the first flush restates the carried-in list; the
    // churn counts already exclude it, so the Opened list must exclude it
    // too — a headline count over a contradicting list was the defect.
    let writes = vec![
        write(
            "2026-10-16T09:00:00Z",
            vec![
                item("item00: carried a", "pending"),
                item("item01: carried b", "pending"),
            ],
        ),
        write(
            "2026-10-16T09:05:00Z",
            vec![
                item("item00: carried a", "pending"),
                item("item01: carried b", "pending"),
                item("item02: new c", "pending"),
                item("item03: new d", "pending"),
            ],
        ),
        write(
            "2026-10-16T09:10:00Z",
            vec![
                item("item00: carried a", "completed"),
                item("item01: carried b", "pending"),
                item("item02: new c", "pending"),
                item("item03: new d", "pending"),
            ],
        ),
    ];
    let events = diff_todo_states(&writes);
    let (opened, closed) = opened_closed_counts(&writes, true);
    assert_eq!((opened, closed), (2, 1), "two genuine opens, one close");

    let section = build_todo_log_section(&events, opened, closed, 2, &[]);
    assert!(section.contains("Opened (2):"), "{section}");
    assert!(
        !section.contains("item00: carried a (pending)"),
        "the carried-in items are not counted as opened: {section}"
    );
    assert!(
        section.contains("item02: new c (pending)"),
        "the genuine opens are listed: {section}"
    );
    assert!(section.contains("Closed (1):"), "{section}");
    // The transitions remain the complete record, baseline included.
    assert!(section.contains("added: item00: carried a"), "{section}");
}

#[test]
fn the_churn_note_fires_when_items_open_faster_than_they_close() {
    let writes = vec![write(
        "2026-10-16T09:00:00Z",
        vec![
            item("item00: a", "pending"),
            item("item01: b", "pending"),
            item("item02: c", "pending"),
        ],
    )];
    let events = diff_todo_states(&writes);
    let (opened, closed) = opened_closed_counts(&writes, false);
    assert_eq!((opened, closed), (3, 0));

    let steers = vec!["stop starting new things".to_string()];
    let section = build_todo_log_section(&events, opened, closed, 0, &steers);
    assert!(section.contains("Churn note"), "{section}");
    assert!(
        section.contains("starting new things without finishing old ones may be causing churn"),
        "{section}"
    );
    assert!(section.contains("stop starting new things"), "{section}");
}

#[test]
fn the_churn_note_stays_silent_when_closes_keep_up() {
    let writes = vec![
        write(
            "2026-10-16T09:00:00Z",
            vec![item("item00: a", "pending"), item("item01: b", "pending")],
        ),
        write(
            "2026-10-16T09:05:00Z",
            vec![
                item("item00: a", "completed"),
                item("item01: b", "completed"),
            ],
        ),
    ];
    let events = diff_todo_states(&writes);
    let (opened, closed) = opened_closed_counts(&writes, false);
    assert_eq!((opened, closed), (2, 2));

    let section = build_todo_log_section(&events, opened, closed, 0, &[]);
    assert!(!section.contains("Churn note"), "{section}");
}

#[test]
fn a_restated_first_flush_is_the_baseline_not_new_opens() {
    // A full=false slice starts at the last compaction point: the first flush
    // after the marker restates the carried-in list, so its adds are the
    // baseline, not items opened in the period.
    let writes = vec![
        write(
            "2026-10-16T09:00:00Z",
            vec![
                item("item00: carried a", "pending"),
                item("item01: carried b", "pending"),
                item("item02: carried c", "pending"),
                item("item03: carried d", "pending"),
                item("item04: carried e", "pending"),
            ],
        ),
        write(
            "2026-10-16T09:05:00Z",
            vec![
                item("item00: carried a", "pending"),
                item("item01: carried b", "pending"),
                item("item02: carried c", "pending"),
                item("item03: carried d", "pending"),
                item("item04: carried e", "pending"),
                item("item05: new f", "pending"),
            ],
        ),
        write(
            "2026-10-16T09:10:00Z",
            vec![
                item("item00: carried a", "completed"),
                item("item01: carried b", "pending"),
                item("item02: carried c", "pending"),
                item("item03: carried d", "pending"),
                item("item04: carried e", "pending"),
                item("item05: new f", "pending"),
            ],
        ),
    ];
    assert_eq!(
        opened_closed_counts(&writes, true),
        (1, 1),
        "one genuine open, one close"
    );
    assert_eq!(
        opened_closed_counts(&writes, false),
        (6, 1),
        "without the baseline the adds all count"
    );
}

#[test]
fn an_empty_period_has_no_churn_note_and_says_so() {
    let section = build_todo_log_section(&[], 0, 0, 0, &[]);
    assert!(
        section.contains("No todo activity in this period"),
        "{section}"
    );
    assert!(!section.contains("Churn note"), "{section}");
}

#[test]
fn a_harness_without_todo_flushes_says_so_instead_of_reading_as_no_todos() {
    let section =
        build_todo_log_unavailable("vibe", "todowrite flushes are only readable on opencode");
    assert!(section.contains("## Todo Log"), "{section}");
    assert!(section.contains("vibe"), "{section}");
    assert!(section.contains("only readable on opencode"), "{section}");
}

// --- Commits -----------------------------------------------------------------

#[test]
fn the_commits_section_lists_the_period_git_commits_with_timestamps() {
    let messages = vec![
        message(
            "assistant",
            "working",
            Some("2026-10-16T09:00:00Z"),
            &["bash: git commit -m \"wip: first\""],
        ),
        message("user", "carry on", Some("2026-10-16T09:01:00Z"), &[]),
        message(
            "assistant",
            "working",
            Some("2026-10-16T09:02:00Z"),
            &["bash: git commit -m \"wip: second\""],
        ),
    ];
    let section = build_commits_section(&messages);
    assert!(section.contains("## Commits"), "{section}");
    assert!(
        section.contains("2026-10-16T09:00:00Z — bash: git commit -m \"wip: first\""),
        "{section}"
    );
    assert!(
        section.contains("2026-10-16T09:02:00Z — bash: git commit -m \"wip: second\""),
        "{section}"
    );
}

#[test]
fn a_period_without_commits_says_so() {
    let messages = vec![message("user", "hello", Some("2026-10-16T09:00:00Z"), &[])];
    let section = build_commits_section(&messages);
    assert!(section.contains("No commits in this period"), "{section}");
}

// --- Compaction footer ---------------------------------------------------------

#[test]
fn the_footer_names_the_last_few_compaction_points_with_counts_since() {
    let messages = vec![
        message("user", "one", Some("2026-10-14T09:00:00Z"), &[]),
        message("assistant", "two", Some("2026-10-14T10:00:00Z"), &[]),
        message("user", "three", Some("2026-10-14T11:00:00Z"), &[]),
        message(
            "user",
            "context compaction happened here",
            Some("2026-10-15T09:00:00Z"),
            &[],
        ),
        message("assistant", "five", Some("2026-10-15T10:00:00Z"), &[]),
        message("user", "six", Some("2026-10-15T11:00:00Z"), &[]),
        message("assistant", "seven", Some("2026-10-15T12:00:00Z"), &[]),
        message(
            "user",
            "context compaction again",
            Some("2026-10-16T09:00:00Z"),
            &[],
        ),
        message("assistant", "nine", Some("2026-10-16T10:00:00Z"), &[]),
        message("user", "ten", Some("2026-10-16T11:00:00Z"), &[]),
    ];
    let footer = build_compaction_footer(&messages, false, &[3, 7]);
    assert!(footer.contains("## Compaction Points"), "{footer}");
    assert!(
        footer.contains("2026-10-15T09:00:00Z — 6 messages"),
        "{footer}"
    );
    assert!(
        footer.contains("2026-10-16T09:00:00Z — 2 messages"),
        "{footer}"
    );
    assert!(
        footer.contains(
            "2026-10-16T09:00:00Z — 2 messages, 2 lines since (the current period starts here)"
        ),
        "{footer}"
    );
    assert!(footer.contains("FYI only"), "{footer}");
    assert!(footer.contains("the `/compact` flow"), "{footer}");
}

#[test]
fn the_footer_holds_back_nothing_when_full_and_says_the_period_is_the_whole_session() {
    let messages = vec![
        message("user", "one", Some("2026-10-14T09:00:00Z"), &[]),
        message(
            "user",
            "context compaction here",
            Some("2026-10-15T09:00:00Z"),
            &[],
        ),
        message("assistant", "three", Some("2026-10-16T09:00:00Z"), &[]),
    ];
    let footer = build_compaction_footer(&messages, true, &[1]);
    assert!(footer.contains("the `/new`-drop flow"), "{footer}");
    assert!(
        !footer.contains("the current period starts here"),
        "with full=true the period is not bounded by a compaction point: {footer}"
    );
    assert!(footer.contains("1 message, 1 line since"), "{footer}");
}

#[test]
fn a_session_without_compaction_points_says_the_period_is_the_whole_session() {
    let messages = vec![message("user", "only", Some("2026-10-16T09:00:00Z"), &[])];
    let footer = build_compaction_footer(&messages, false, &[]);
    assert!(
        footer.contains("No compaction points in this session"),
        "{footer}"
    );
}

// --- Steers ------------------------------------------------------------------

#[test]
fn recent_steers_takes_the_newest_user_messages_redacted_and_truncated() {
    let users: Vec<String> = (0..5).map(|i| format!("steer number {i}")).collect();
    let steers = recent_steers(&users, 3);
    assert_eq!(steers.len(), 3);
    assert_eq!(steers[0], "steer number 2");
    assert_eq!(steers[2], "steer number 4");

    let secret = "sk_abcdefghijklmnopqrstuvwxyz012345".to_string();
    let steers = recent_steers(&[secret], 3);
    assert!(
        !steers[0].contains("sk_abcdefghijklmnopqrstuvwxyz"),
        "a steer is redacted like every other string that reaches the output: {steers:?}"
    );

    let long = "x".repeat(1000);
    let steers = recent_steers(&[long], 3);
    assert!(
        steers[0].len() <= 320,
        "a steer is truncated: {}",
        steers[0].len()
    );
}

// --- Boundary ------------------------------------------------------------------

#[test]
fn the_period_is_the_whole_session_or_the_post_compaction_slice() {
    // rollouts/mock_sample.jsonl: 8 messages, compaction marker at index 4.
    let adapter =
        MockAdapter::new(env!("CARGO_MANIFEST_DIR").to_string() + "/rollouts/mock_sample.jsonl");
    let full_messages = adapter
        .read_session_mmap("test")
        .expect("healthy fixture mmap read");
    assert_eq!(full_messages.len(), 8);

    let period_full = full_messages.clone();
    let period_from_compaction = slice_from_compaction(full_messages);
    assert_eq!(period_from_compaction.len(), 4);
    assert!(
        period_from_compaction[0]
            .content
            .contains("context compaction")
    );
    // The two views the tool serves: full=true covers everything up to the
    // drop; full=false starts at the last compaction point.
    assert_ne!(period_full.len(), period_from_compaction.len());
}

#[test]
fn the_scratch_fixture_supports_the_boundary_test_isolation() {
    // A session with no marker reads whole under both flags.
    let dir = scratch("prior_period_no_marker");
    let path = dir.join("session.jsonl");
    std::fs::write(
        &path,
        "{\"role\":\"user\",\"content\":\"hello\",\"tool_calls_summary\":[],\"timestamp\":null,\"injected\":false}\n",
    )
    .unwrap();
    let adapter = MockAdapter::new(&path);
    let full = adapter.read_session_mmap("x").unwrap();
    assert_eq!(slice_from_compaction(full).len(), 1);
}

// --- Served boundary ----------------------------------------------------------
//
// What a client sees in tools/list is the deprecation boundary: the new tool
// registered beside the old one, the old one's description carrying the
// DEPRECATED pointer, and the new one's schema stating every bound its
// handler enforces. No LLM call happens in these tests: a fixture store, the
// API keys removed, and every call either a listing or caught at validation.

mod served {
    use rusqlite::Connection;
    use serde_json::{Map, Value, json};
    use std::io::{BufRead, Write};

    struct Mcp {
        child: std::process::Child,
        reader: std::io::BufReader<std::process::ChildStdout>,
        stdin: std::process::ChildStdin,
        cwd: std::path::PathBuf,
        next: u64,
    }

    impl Mcp {
        fn send(&mut self, v: &Value) {
            writeln!(self.stdin, "{}", serde_json::to_string(v).unwrap()).unwrap();
            self.stdin.flush().unwrap();
        }

        fn read_until_id(&mut self, id: u64) -> Value {
            loop {
                let mut line = String::new();
                self.reader
                    .read_line(&mut line)
                    .expect("server stdout open");
                if line.trim().is_empty() {
                    panic!("server closed stdout waiting for response {id}");
                }
                if let Ok(v) = serde_json::from_str::<Value>(&line)
                    && v.get("id").and_then(|i| i.as_u64()) == Some(id)
                {
                    return v;
                }
            }
        }

        fn call(&mut self, name: &str, args: Value) -> (bool, String) {
            let id = self.next;
            self.next += 1;
            self.send(&json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": {"name": name, "arguments": args}
            }));
            let result = self.read_until_id(id)["result"].clone();
            (
                result["isError"].as_bool().unwrap_or(false),
                result["content"][0]["text"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            )
        }

        fn tools_list(&mut self) -> Value {
            let id = self.next;
            self.next += 1;
            self.send(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"}));
            self.read_until_id(id)["result"]["tools"].clone()
        }

        fn schema(&mut self, name: &str) -> Map<String, Value> {
            let tools = self.tools_list();
            let tool = tools
                .as_array()
                .expect("tools array")
                .iter()
                .find(|t| t["name"].as_str() == Some(name))
                .unwrap_or_else(|| panic!("{name} is registered"));
            tool["inputSchema"]["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("{name} serves a properties object"))
                .clone()
        }
    }

    fn spawn() -> (Mcp, std::path::PathBuf) {
        let dir = crate::common::scratch::scratch("prior_period_served");
        let conn = Connection::open(dir.join("fixture.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
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
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
             VALUES ('ses_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', NULL, '/tmp/x', 'fixture', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('m', 'ses_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 1, 1,
                     '{\"role\":\"user\",\"time\":{\"created\":1}}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES ('p', 'm', 'ses_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 1, 1, ?1)",
            [json!({"type": "text", "text": "fixture body"}).to_string()],
        )
        .unwrap();

        let cwd = crate::common::scratch::child_cwd("prior_period_served");
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_total-recall"))
            .args(["--harness", "opencode", "mcp"])
            .current_dir(&cwd)
            .env("TOTAL_RECALL_OPENCODE_ROOT", dir.join("fixture.db"))
            .env_remove("INCEPTION_API_KEY")
            .env_remove("MISTRAL_API_KEY")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn mcp");
        let reader = std::io::BufReader::new(child.stdout.take().unwrap());
        let stdin = child.stdin.take().unwrap();
        let mut mcp = Mcp {
            child,
            reader,
            stdin,
            cwd,
            next: 1,
        };
        mcp.send(&json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                       "clientInfo": {"name": "prior-period-test", "version": "0"}}
        }));
        let _ = mcp.read_until_id(0);
        mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        (mcp, dir.join("fixture.db"))
    }

    fn reap(mcp: &mut Mcp) {
        mcp.child.kill().ok();
        mcp.child.wait().ok();
        let _ = std::fs::remove_dir_all(&mcp.cwd);
    }

    #[test]
    fn the_deleted_tool_is_gone_and_the_new_tool_registered() {
        let (mut mcp, _db) = spawn();
        let tools = mcp.tools_list();
        let array = tools.as_array().expect("tools array");

        assert!(
            array
                .iter()
                .all(|t| t["name"].as_str() != Some("total_recall")),
            "the deprecated tool is deleted, not listed: {tools}"
        );

        let new = array
            .iter()
            .find(|t| t["name"].as_str() == Some("prior_period_summary"))
            .expect("prior_period_summary is registered");
        let description = new["description"].as_str().unwrap_or("");
        assert!(
            description.contains("orientation summary of a prior period"),
            "the tool presents itself as an orientation summary of a prior period: {description}"
        );
        assert!(
            !description.contains("not a compaction")
                && !description.contains("voluntary"),
            "the description does not define the tool against the deleted compaction method: {description}"
        );
        reap(&mut mcp);
    }

    #[test]
    fn the_served_schema_states_exactly_the_fields_the_handler_accepts() {
        let (mut mcp, _db) = spawn();
        let schema = mcp.schema("prior_period_summary");
        let mut fields: Vec<&str> = schema.keys().map(String::as_str).collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            vec!["full", "hours_back", "max_bytes", "session_id"]
        );
        assert!(schema["full"]["description"].is_string());
        assert!(
            schema["full"]["description"]
                .as_str()
                .unwrap_or("")
                .contains("/new"),
            "the schema states both flows"
        );
        reap(&mut mcp);
    }

    #[test]
    fn an_unknown_field_and_an_out_of_bound_are_answered_in_the_house_form() {
        let (mut mcp, _db) = spawn();
        let (err, text) = mcp.call("prior_period_summary", json!({"fulls": true}));
        assert!(err && text.contains("unrecognised field `fulls`"), "{text}",);
        assert!(
            text.contains("session_id|full|hours_back|max_bytes")
                || text.contains("full|hours_back|max_bytes|session_id"),
            "{text}"
        );

        let (err, text) = mcp.call("prior_period_summary", json!({"max_bytes": 0}));
        assert!(err, "{text}");
        assert!(
            text.contains("pass max_bytes from 1 to 8388608"),
            "an out-of-bound max_bytes is the house form, before the session is resolved: {text}"
        );

        let (err, text) = mcp.call("prior_period_summary", json!({"hours_back": 0}));
        // hours_back 0 takes the default window; with no API key the call then
        // fails at the provider, never at the window.
        assert!(
            !err || !text.contains("hours_back"),
            "hours_back 0 is the default window, never an error about the window: {text}"
        );
        reap(&mut mcp);
    }
}
