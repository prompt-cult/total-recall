//! #686: `todo_history` — a rollout's todowrite flushes replayed as a JSONL
//! stream of edit events, one compact JSON object per line
//! (`{"ts":…,"action":…,"todo":…[,"status":…]}`), in ts order. The pure diff
//! core, the opencode adapter (fixture store, never the live DB), the
//! default-harness refusal, the real MCP stdio server, and the CLI subcommand
//! are all covered here. Written RED, before the implementation existed.

mod common;

use common::scratch::{child_cwd, scratch};
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use std::io::{BufRead, Write};
use std::path::Path;
use total_recall::RolloutAdapter;
use total_recall::rollout::opencode::OpenCodeAdapter;
use total_recall::todo_history::{TodoEditEvent, TodoItemState, TodoWrite, diff_todo_states};

// --- Pure diff core ---------------------------------------------------------

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

fn event(ts: &str, action: &str, todo: &str, status: Option<&str>) -> TodoEditEvent {
    TodoEditEvent {
        ts: ts.to_string(),
        action: action.to_string(),
        todo: todo.to_string(),
        status: status.map(str::to_string),
    }
}

#[test]
fn first_flush_adds_every_item_in_order_with_its_status() {
    let events = diff_todo_states(&[write(
        "2026-01-01T00:00:01Z",
        vec![
            item("item00: a", "pending"),
            item("item01: b", "in_progress"),
        ],
    )]);
    assert_eq!(
        events,
        vec![
            event(
                "2026-01-01T00:00:01Z",
                "added",
                "item00: a",
                Some("pending")
            ),
            event(
                "2026-01-01T00:00:01Z",
                "added",
                "item01: b",
                Some("in_progress")
            ),
        ]
    );
}

#[test]
fn status_transition_is_actioned_as_the_new_status() {
    let events = diff_todo_states(&[
        write("2026-01-01T00:00:01Z", vec![item("item00: a", "pending")]),
        write(
            "2026-01-01T00:00:02Z",
            vec![item("item00: a", "in_progress")],
        ),
    ]);
    assert_eq!(
        events[1],
        event(
            "2026-01-01T00:00:02Z",
            "in_progress",
            "item00: a",
            Some("in_progress")
        )
    );
    assert_eq!(events.len(), 2, "one add, one transition: {events:?}");
}

#[test]
fn content_change_with_unchanged_status_is_updated() {
    let events = diff_todo_states(&[
        write("2026-01-01T00:00:01Z", vec![item("item00: a", "pending")]),
        write("2026-01-01T00:00:02Z", vec![item("item00: b", "pending")]),
    ]);
    assert_eq!(
        events[1],
        event(
            "2026-01-01T00:00:02Z",
            "updated",
            "item00: b",
            Some("pending")
        )
    );
}

#[test]
fn an_identical_reflush_emits_nothing() {
    let flush = write("2026-01-01T00:00:01Z", vec![item("item00: a", "pending")]);
    let events = diff_todo_states(&[flush.clone(), flush]);
    assert_eq!(events.len(), 1, "only the first flush's add: {events:?}");
    assert_eq!(events[0].action, "added");
}

#[test]
fn a_removed_item_is_reported_with_status_omitted() {
    let events = diff_todo_states(&[
        write("2026-01-01T00:00:01Z", vec![item("item00: a", "pending")]),
        write("2026-01-01T00:00:02Z", vec![]),
    ]);
    assert_eq!(
        events[1],
        event("2026-01-01T00:00:02Z", "removed", "item00: a", None)
    );
    let line = serde_json::to_string(&events[1]).unwrap();
    assert_eq!(
        line, r#"{"ts":"2026-01-01T00:00:02Z","action":"removed","todo":"item00: a"}"#,
        "a removed event carries no status key: {line}"
    );
}

#[test]
fn a_slugged_item_keeps_its_identity_across_a_rewording() {
    let events = diff_todo_states(&[
        write(
            "2026-01-01T00:00:01Z",
            vec![item("item00: alpha", "pending")],
        ),
        write(
            "2026-01-01T00:00:02Z",
            vec![item("item00: beta", "pending")],
        ),
    ]);
    assert_eq!(
        events.len(),
        2,
        "one add, one update — no spurious remove+add: {events:?}"
    );
    assert_eq!(events[1].action, "updated");
    assert_eq!(events[1].todo, "item00: beta");
}

#[test]
fn an_unslugged_rewording_is_a_remove_plus_an_add() {
    let events = diff_todo_states(&[
        write("2026-01-01T00:00:01Z", vec![item("alpha", "pending")]),
        write("2026-01-01T00:00:02Z", vec![item("beta", "pending")]),
    ]);
    assert_eq!(
        events.len(),
        3,
        "add, then the new item added and the old removed: {events:?}"
    );
    assert_eq!(events[1].action, "added");
    assert_eq!(events[1].todo, "beta");
    assert_eq!(events[2].action, "removed");
    assert_eq!(events[2].todo, "alpha");
}

#[test]
fn events_follow_the_input_flush_order() {
    let events = diff_todo_states(&[
        write("2026-01-01T00:00:03Z", vec![item("item00: a", "pending")]),
        write("2026-01-01T00:00:01Z", vec![item("item00: a", "completed")]),
        write(
            "2026-01-01T00:00:02Z",
            vec![item("item00: a", "completed"), item("item01: b", "pending")],
        ),
    ]);
    let ts: Vec<&str> = events.iter().map(|e| e.ts.as_str()).collect();
    assert_eq!(
        ts,
        vec![
            "2026-01-01T00:00:03Z",
            "2026-01-01T00:00:01Z",
            "2026-01-01T00:00:02Z"
        ],
        "events carry their flush's ts, in input order: {events:?}"
    );
}

#[test]
fn the_event_line_is_the_documented_jsonl_shape() {
    let events = diff_todo_states(&[write(
        "2026-01-01T00:00:01Z",
        vec![item("item00: ship it", "pending")],
    )]);
    let line = serde_json::to_string(&events[0]).unwrap();
    assert_eq!(
        line,
        r#"{"ts":"2026-01-01T00:00:01Z","action":"added","todo":"item00: ship it","status":"pending"}"#
    );
}

#[test]
fn todo_items_deserialize_leniently() {
    let parsed: TodoItemState = serde_json::from_str(r#"{"content":"x"}"#).unwrap();
    assert_eq!(parsed.content, "x");
    assert_eq!(parsed.status, "", "a missing status tolerated");
    assert_eq!(parsed.priority, None, "a missing priority tolerated");
}

// --- opencode adapter (fixture store) ---------------------------------------

/// The fixture's expected event lines, full=true, in ts order. The seventh
/// event is flush 4 dropping item00: the post-compaction flush carries only
/// item02, so the diff reports the removal.
const EXPECTED_EVENT_LINES: [&str; 7] = [
    r#"{"ts":"1970-01-01T00:00:01Z","action":"added","todo":"item00: first task","status":"pending"}"#,
    r#"{"ts":"1970-01-01T00:00:01Z","action":"added","todo":"item01: second task","status":"pending"}"#,
    r#"{"ts":"1970-01-01T00:00:02Z","action":"in_progress","todo":"item00: first task","status":"in_progress"}"#,
    r#"{"ts":"1970-01-01T00:00:03Z","action":"updated","todo":"item00: first task renamed","status":"in_progress"}"#,
    r#"{"ts":"1970-01-01T00:00:03Z","action":"removed","todo":"item01: second task"}"#,
    r#"{"ts":"1970-01-01T00:00:05Z","action":"added","todo":"item02: after compaction","status":"pending"}"#,
    r#"{"ts":"1970-01-01T00:00:05Z","action":"removed","todo":"item00: first task renamed"}"#,
];

fn expected_lines() -> Vec<String> {
    EXPECTED_EVENT_LINES.iter().map(|l| l.to_string()).collect()
}

fn build_fixture(root: &Path) {
    let conn = Connection::open(root.join("fixture.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE session (
            id text PRIMARY KEY, parent_id text, directory text, title text,
            time_created integer NOT NULL, time_updated integer NOT NULL);
        CREATE TABLE message (
            id text PRIMARY KEY, session_id text NOT NULL,
            time_created integer NOT NULL, time_updated integer NOT NULL,
            data text NOT NULL);
        CREATE TABLE part (
            id text PRIMARY KEY, message_id text, session_id text NOT NULL,
            time_created integer NOT NULL, time_updated integer NOT NULL,
            data text NOT NULL);",
    )
    .unwrap();

    // An older session with no todowrites: the empty session_id must resolve
    // to the main session (most recent by time_updated), not to this one.
    conn.execute(
        "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
         VALUES ('ses_todo0000000000000000000000bb', NULL, '/Users/dev/old', 'older', 100, 100)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data)
         VALUES ('msg_bb1', 'ses_todo0000000000000000000000bb', 100, 100,
                 '{\"role\":\"user\",\"time\":{\"created\":100}}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
         VALUES ('part_bb1', 'msg_bb1', 'ses_todo0000000000000000000000bb', 100, 100,
                 '{\"type\":\"text\",\"text\":\"old\"}')",
        [],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
         VALUES ('ses_todo0000000000000000000001aa', NULL, '/Users/dev/todo', 'todo fixture', 1000, 5000)",
        [],
    )
    .unwrap();

    let sid = "ses_todo0000000000000000000001aa";
    let messages: Vec<(&str, i64, &str)> = vec![
        (
            "msg1",
            1000,
            r#"{"role":"assistant","time":{"created":1000}}"#,
        ),
        (
            "msg2",
            2000,
            r#"{"role":"assistant","time":{"created":2000}}"#,
        ),
        (
            "msg3",
            3000,
            r#"{"role":"assistant","time":{"created":3000}}"#,
        ),
        ("msg4", 4000, r#"{"role":"user","time":{"created":4000}}"#),
        (
            "msg5",
            5000,
            r#"{"role":"assistant","time":{"created":5000}}"#,
        ),
    ];
    for (id, t, data) in messages {
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?3, ?4)",
            rusqlite::params![id, sid, t, data],
        )
        .unwrap();
    }

    let parts: Vec<(&str, &str, i64, &str)> = vec![
        // Flush 1: object input, two items.
        (
            "part_tw1",
            "msg1",
            1000,
            r#"{"type":"tool","tool":"todowrite","callID":"c1","state":{"status":"completed","input":{"todos":[{"content":"item00: first task","status":"pending","priority":"high"},{"content":"item01: second task","status":"pending"}]}}}"#,
        ),
        // Flush 2: the input itself is a JSON-encoded string.
        (
            "part_tw2",
            "msg2",
            2000,
            r#"{"type":"tool","tool":"todowrite","callID":"c2","state":{"status":"completed","input":"{\"todos\":[{\"content\":\"item00: first task\",\"status\":\"in_progress\"},{\"content\":\"item01: second task\",\"status\":\"pending\"}]}"}}"#,
        ),
        // A non-todowrite tool part: never a flush.
        (
            "part_bash",
            "msg2",
            2100,
            r#"{"type":"tool","tool":"bash","callID":"c3","state":{"status":"completed","input":{"command":"ls"},"output":"ok"}}"#,
        ),
        // Flush 3: item00 reworded (same status), item01 gone.
        (
            "part_tw3",
            "msg3",
            3100,
            r#"{"type":"tool","tool":"todowrite","callID":"c4","state":{"status":"completed","input":{"todos":[{"content":"item00: first task renamed","status":"in_progress"}]}}}"#,
        ),
        // The compaction marker: full=false slices here.
        (
            "part_comp",
            "msg4",
            4000,
            r#"{"type":"compaction","auto":false}"#,
        ),
        // Flush 4: post-compaction, one new item.
        (
            "part_tw4",
            "msg5",
            5000,
            r#"{"type":"tool","tool":"todowrite","callID":"c5","state":{"status":"completed","input":{"todos":[{"content":"item02: after compaction","status":"pending"}]}}}"#,
        ),
        // A todowrite whose todos is a string that is not valid JSON: skipped.
        (
            "part_tw5",
            "msg5",
            5100,
            r#"{"type":"tool","tool":"todowrite","callID":"c6","state":{"status":"completed","input":{"todos":"[{\"content\": broken \escape\"}]"}}}"#,
        ),
        // A failed todowrite (error state, no todos): not a flush, skipped.
        (
            "part_tw6",
            "msg5",
            5200,
            r#"{"type":"tool","tool":"todowrite","callID":"c7","state":{"status":"error","input":{},"error":"aborted"}}"#,
        ),
    ];
    for (id, mid, t, data) in parts {
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            rusqlite::params![id, mid, sid, t, data],
        )
        .unwrap();
    }
}

fn fixture_adapter(name: &str) -> (common::scratch::ScratchRoot, OpenCodeAdapter) {
    let dir = scratch(&format!("todo_history_{name}"));
    build_fixture(&dir);
    let adapter = OpenCodeAdapter::with_root(dir.join("fixture.db"));
    (dir, adapter)
}

#[test]
fn adapter_returns_every_parseable_flush_in_order_with_iso_timestamps() {
    let (_fixture, adapter) = fixture_adapter("adapter_full");
    let writes = adapter
        .read_todo_writes("ses_todo", true)
        .expect("healthy fixture read");
    assert_eq!(writes.len(), 4, "four parseable flushes: {writes:?}");
    let ts: Vec<&str> = writes
        .iter()
        .map(|w| w.timestamp.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(
        ts,
        vec![
            "1970-01-01T00:00:01Z",
            "1970-01-01T00:00:02Z",
            "1970-01-01T00:00:03Z",
            "1970-01-01T00:00:05Z",
        ],
        "ts comes from the carrying message, in message/part order"
    );
    assert_eq!(writes[0].todos.len(), 2);
    assert_eq!(writes[0].todos[0].content, "item00: first task");
    assert_eq!(writes[0].todos[0].status, "pending");
    assert_eq!(writes[0].todos[0].priority.as_deref(), Some("high"));
    assert_eq!(
        writes[1].todos[0].status, "in_progress",
        "string input parsed"
    );
    assert_eq!(writes[2].todos[0].content, "item00: first task renamed");
    assert_eq!(writes[3].todos[0].content, "item02: after compaction");
}

#[test]
fn adapter_full_false_slices_at_the_last_compaction() {
    let (_fixture, adapter) = fixture_adapter("adapter_slice");
    let writes = adapter
        .read_todo_writes("ses_todo", false)
        .expect("healthy fixture read");
    assert_eq!(
        writes.len(),
        1,
        "only the post-compaction flush: {writes:?}"
    );
    assert_eq!(writes[0].todos[0].content, "item02: after compaction");
    assert_eq!(writes[0].timestamp.as_deref(), Some("1970-01-01T00:00:05Z"));
}

#[test]
fn adapter_events_over_the_full_session_match_the_documented_stream() {
    let (_fixture, adapter) = fixture_adapter("adapter_events");
    let writes = adapter.read_todo_writes("ses_todo", true).unwrap();
    let events = diff_todo_states(&writes);
    let lines: Vec<String> = events
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect();
    assert_eq!(lines, expected_lines());
}

#[test]
fn adapter_unknown_session_is_the_house_no_session_error() {
    let (_fixture, adapter) = fixture_adapter("adapter_unknown");
    let err = adapter
        .read_todo_writes("ses_nope", true)
        .expect_err("an unresolvable session id must not read as an empty history");
    assert!(err.contains("no session matching"), "{err}");
    assert!(
        err.contains("ses_nope"),
        "the error names the session: {err}"
    );
}

#[test]
fn a_non_opencode_harness_gets_the_explicit_unsupported_error() {
    let dir = scratch("todo_history_mock");
    let path = dir.join("messages.jsonl");
    std::fs::write(
        &path,
        "{\"role\":\"user\",\"content\":\"hi\",\"tool_calls_summary\":[],\"timestamp\":null,\"injected\":false}\n",
    )
    .unwrap();
    let adapter = total_recall::MockAdapter::new(&path);
    let err = adapter
        .read_todo_writes("mock", true)
        .expect_err("a non-opencode harness must refuse, never return an empty list");
    assert!(
        err.contains("todo_history is only available for the opencode harness"),
        "{err}"
    );
    assert!(err.contains("mock"), "the error names the harness: {err}");
}

// --- MCP stdio server -------------------------------------------------------

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

    /// The call as a client sees it: `(is_error, text)`.
    fn call(&mut self, name: &str, args: Value) -> (bool, String) {
        let id = self.next;
        self.next += 1;
        self.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": args}
        }));
        let result = self.read_until_id(id)["result"].clone();
        let is_error = result["isError"].as_bool().unwrap_or(false);
        (
            is_error,
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

    /// The SERVED input schema's properties for one tool.
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

fn spawn_mcp() -> (Mcp, common::scratch::ScratchRoot) {
    let dir = scratch("todo_history_mcp");
    build_fixture(&dir);
    let cwd = child_cwd("todo_history_mcp");
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
                   "clientInfo": {"name": "todo-history-test", "version": "0"}}
    }));
    let _ = mcp.read_until_id(0);
    mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    (mcp, dir)
}

fn reap(mcp: &mut Mcp) {
    mcp.child.kill().ok();
    mcp.child.wait().ok();
    let _ = std::fs::remove_dir_all(&mcp.cwd);
}

/// The report's header and raw event lines: everything after the `#` header.
fn report_events(text: &str) -> (Value, Vec<String>) {
    let mut lines = text.lines();
    let header_line = lines.next().expect("the report has a header line");
    assert!(
        header_line.starts_with('#'),
        "the header is #-prefixed: {header_line}"
    );
    let header: Value =
        serde_json::from_str(header_line.trim_start_matches('#')).expect("header is JSON");
    let events: Vec<String> = lines.map(str::to_string).collect();
    for line in &events {
        let _: Value = serde_json::from_str(line).expect("each event line is JSON");
    }
    (header, events)
}

#[test]
fn todo_history_is_served_with_its_schema() {
    let (mut mcp, _dir) = spawn_mcp();
    let tools = mcp.tools_list();
    let names: Vec<&str> = tools
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        names.contains(&"todo_history"),
        "todo_history is served (mounted as total-recall_todo_history): {names:?}"
    );
    let props = mcp.schema("todo_history");
    let mut keys: Vec<&String> = props.keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["full", "max_bytes", "session_id"],
        "no parameters beyond session_id/full/max_bytes: {keys:?}"
    );
    assert_eq!(props["max_bytes"]["minimum"].as_f64(), Some(1.0));
    assert_eq!(props["max_bytes"]["maximum"].as_f64(), Some(8_388_608.0));
    reap(&mut mcp);
}

#[test]
fn todo_history_emits_the_event_stream_with_a_header() {
    let (mut mcp, _dir) = spawn_mcp();
    let (err, text) = mcp.call("todo_history", json!({"session_id": "ses_todo"}));
    assert!(!err, "the call succeeds: {text}");
    let (header, events) = report_events(&text);
    // The header names the session as the caller asked for it; the adapter
    // resolves the partial id internally (the house session-tool form).
    assert_eq!(header["session_id"].as_str(), Some("ses_todo"));
    assert_eq!(header["harness"].as_str(), Some("opencode"));
    assert_eq!(
        header["full"].as_bool(),
        Some(true),
        "full defaults to true"
    );
    assert_eq!(header["events"].as_u64(), Some(7));
    assert_eq!(events, expected_lines());
    reap(&mut mcp);
}

#[test]
fn todo_history_full_false_slices_at_the_compaction() {
    let (mut mcp, _dir) = spawn_mcp();
    let (err, text) = mcp.call(
        "todo_history",
        json!({"session_id": "ses_todo", "full": false}),
    );
    assert!(!err, "the call succeeds: {text}");
    let (header, events) = report_events(&text);
    assert_eq!(header["full"].as_bool(), Some(false));
    assert_eq!(
        header["events"].as_u64(),
        Some(1),
        "only post-compaction edits"
    );
    assert_eq!(events, vec![EXPECTED_EVENT_LINES[5].to_string()]);
    reap(&mut mcp);
}

#[test]
fn todo_history_empty_session_id_resolves_to_the_most_recent_session() {
    let (mut mcp, _dir) = spawn_mcp();
    let (err, text) = mcp.call("todo_history", json!({}));
    assert!(!err, "the call succeeds: {text}");
    let (header, events) = report_events(&text);
    assert_eq!(
        header["session_id"].as_str(),
        Some("ses_todo0000000000000000000001aa"),
        "the most recent session by time_updated: {header}"
    );
    assert_eq!(events.len(), 7);
    reap(&mut mcp);
}

#[test]
fn todo_history_rejects_an_unknown_field_by_name() {
    let (mut mcp, _dir) = spawn_mcp();
    let (err, text) = mcp.call("todo_history", json!({"hour_back": 24}));
    assert!(err, "a typo is rejected: {text}");
    assert!(
        text.contains("unrecognised field `hour_back`"),
        "the error names the field: {text}"
    );
    assert!(
        text.contains("todo_history accepts:"),
        "the error lists the accepted fields: {text}"
    );
    reap(&mut mcp);
}

#[test]
fn todo_history_rejects_out_of_range_max_bytes_in_the_house_form() {
    let (mut mcp, _dir) = spawn_mcp();
    for bad in [0, -1, 9_000_000] {
        let (err, text) = mcp.call("todo_history", json!({"max_bytes": bad}));
        assert!(err, "max_bytes {bad} is a tool error: {text}");
        assert!(
            text.starts_with("todo_history: "),
            "the house form speaks first: {text}"
        );
        assert!(
            text.contains("`max_bytes` is out of range"),
            "the error names the field: {text}"
        );
        assert!(
            !text.contains("failed to deserialize"),
            "no raw serde dump: {text}"
        );
    }
    reap(&mut mcp);
}

#[test]
fn todo_history_report_is_flood_capped_with_marker_and_overflow_file() {
    let (mut mcp, _dir) = spawn_mcp();
    // The full report is ~700 bytes; 512 caps it mid-stream.
    let (err, text) = mcp.call(
        "todo_history",
        json!({"session_id": "ses_todo", "max_bytes": 512}),
    );
    assert!(!err, "a capped report is not an error: {text}");
    assert!(
        text.contains("--- [EOF-TRUNCATED] ---"),
        "the marker is present: {text}"
    );
    assert!(
        text.contains("tool: todo_history"),
        "the marker names the tool"
    );
    let head = text
        .split("--- [EOF-TRUNCATED] ---")
        .next()
        .expect("head before the marker");
    assert!(
        head.len() <= 512,
        "the returned head respects the cap: {} bytes",
        head.len()
    );
    assert!(head.starts_with('#'), "the head keeps the header line");

    let path_line = text
        .lines()
        .find(|l| l.starts_with("full_report: "))
        .expect("the marker names the full report path");
    let path = std::path::PathBuf::from(path_line.trim_start_matches("full_report: ").trim());
    let whole = std::fs::read_to_string(&path).expect("the overflow file exists");
    for expected in EXPECTED_EVENT_LINES {
        assert!(
            whole.contains(expected),
            "the file holds every event: {whole}"
        );
    }
    assert!(whole.starts_with('#'), "the file holds the header too");
    let _ = std::fs::remove_file(&path);
    reap(&mut mcp);
}

// --- CLI --------------------------------------------------------------------

fn run_cli(root: &Path, args: &[&str]) -> std::process::Output {
    let cwd = child_cwd("todo_history_cli");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_total-recall"))
        .args(args)
        .current_dir(&cwd)
        .env("TOTAL_RECALL_OPENCODE_ROOT", root.join("fixture.db"))
        .env("TOTAL_RECALL_VIBE_ROOT", root)
        .env_remove("INCEPTION_API_KEY")
        .env_remove("MISTRAL_API_KEY")
        .output()
        .expect("spawn cli");
    let _ = std::fs::remove_dir_all(&cwd);
    out
}

#[test]
fn cli_streams_the_jsonl_event_stream_unbounded() {
    let dir = scratch("todo_history_cli_full");
    build_fixture(&dir);
    let out = run_cli(
        &dir,
        &[
            "--harness",
            "opencode",
            "--session",
            "ses_todo",
            "todo-history",
        ],
    );
    assert!(
        out.status.success(),
        "exit 0: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let lines: Vec<String> = stdout.lines().map(str::to_string).collect();
    assert_eq!(lines, expected_lines(), "bare JSONL, no header, unbounded");
    for line in &lines {
        let v: Value = serde_json::from_str(line).expect("each line is JSON");
        assert!(v.get("ts").is_some() && v.get("action").is_some() && v.get("todo").is_some());
    }
}

#[test]
fn cli_no_full_slices_at_the_compaction() {
    let dir = scratch("todo_history_cli_slice");
    build_fixture(&dir);
    let out = run_cli(
        &dir,
        &[
            "--harness",
            "opencode",
            "--session",
            "ses_todo",
            "todo-history",
            "--no-full",
        ],
    );
    assert!(
        out.status.success(),
        "exit 0: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        vec![EXPECTED_EVENT_LINES[5]]
    );
}

#[test]
fn cli_unknown_session_exits_2_with_the_error_named() {
    let dir = scratch("todo_history_cli_unknown");
    build_fixture(&dir);
    let out = run_cli(
        &dir,
        &[
            "--harness",
            "opencode",
            "--session",
            "ses_nope",
            "todo-history",
        ],
    );
    assert_eq!(out.status.code(), Some(2), "an unreadable session exits 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no session matching") && stderr.contains("ses_nope"),
        "stderr names the session: {stderr}"
    );
}

#[test]
fn cli_on_a_non_opencode_harness_exits_2_with_the_unsupported_error() {
    let dir = scratch("todo_history_cli_vibe");
    build_fixture(&dir);
    let out = run_cli(
        &dir,
        &["--harness", "vibe", "--session", "ses_todo", "todo-history"],
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "the unsupported harness exits 2"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("todo_history is only available for the opencode harness"),
        "stderr names the refusal: {stderr}"
    );
    assert!(
        stderr.contains("vibe"),
        "the error names the harness: {stderr}"
    );
}
