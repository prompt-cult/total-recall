//! The end state, proven through the real MCP stdio server against an
//! opencode fixture store: session-scoped tools, the 240-hour list cutoff,
//! flood control that never tears JSON, the marker with its histogram, and
//! the line_histogram companion tool. Written RED.

mod common;

use common::scratch::{child_cwd, scratch};
use rusqlite::Connection;
use serde_json::Value;
use std::io::{BufRead, Write};
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Build an opencode fixture with three sessions: alpha and beta updated
/// now, old updated at the unix epoch. Alpha carries many long matching
/// parts so a she_said report overflows a small cap.
fn build_fixture(root: &std::path::Path) {
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
            id text PRIMARY KEY, message_id text NOT NULL, session_id text NOT NULL,
            time_created integer NOT NULL, time_updated integer NOT NULL,
            data text NOT NULL);",
    )
    .unwrap();
    let now = now_ms();
    let sessions: Vec<(&str, &str, String, i64)> = vec![
        (
            "ses_alpha0000000000000000000000aa",
            "/Users/dev/alpha",
            "alpha session".into(),
            now,
        ),
        (
            "ses_beta00000000000000000000000bb",
            "/Users/dev/beta",
            "beta session".into(),
            now,
        ),
        (
            "ses_old0000000000000000000000000c",
            "/Users/dev/old",
            "old session".into(),
            4000,
        ),
        // a title so large that list_sessions overflows the default 16 KiB
        // window: the JSON-tear contract, proven for real
        (
            "ses_huge000000000000000000000000d",
            "/Users/dev/huge",
            "H".repeat(20_000),
            now,
        ),
    ];
    for (id, dir, title, updated) in &sessions {
        conn.execute(
            "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
             VALUES (?1, NULL, ?2, ?3, ?4, ?4)",
            rusqlite::params![id, dir, title, updated],
        )
        .unwrap();
    }
    // alpha: one message with many long user parts, each matching "release"
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data)
         VALUES ('amsg1', 'ses_alpha0000000000000000000000aa', ?1, ?1,
                 '{\"role\":\"user\",\"time\":{\"created\":1}}')",
        rusqlite::params![now],
    )
    .unwrap();
    for i in 0..20 {
        let text = format!("release alpha finding {i} {}", "x".repeat(120));
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES (?1, 'amsg1', 'ses_alpha0000000000000000000000aa', ?2, ?2, ?3)",
            rusqlite::params![
                format!("apart{i}"),
                now,
                serde_json::json!({"type":"text","text":text}).to_string()
            ],
        )
        .unwrap();
    }
    // beta: one short matching user part
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data)
         VALUES ('bmsg1', 'ses_beta00000000000000000000000bb', ?1, ?1,
                 '{\"role\":\"user\",\"time\":{\"created\":1}}')",
        rusqlite::params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
         VALUES ('bpart1', 'bmsg1', 'ses_beta00000000000000000000000bb', ?1, ?1, ?2)",
        rusqlite::params![
            now,
            serde_json::json!({"type":"text","text":"release beta finding"}).to_string()
        ],
    )
    .unwrap();
    // old: one part so it has content when listed with all: true
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data)
         VALUES ('omsg1', 'ses_old0000000000000000000000000c', 4000, 4000,
                 '{\"role\":\"user\",\"time\":{\"created\":1}}')",
        [],
    )
    .unwrap();
}

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
        self.send(&serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": args}
        }));
        let resp = self.read_until_id(id);
        let result = resp["result"].clone();
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
        self.send(&serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/list"
        }));
        self.read_until_id(id)["result"]["tools"].clone()
    }
}

/// A fixture of `count` sessions all updated inside the default 240-hour
/// window, with strictly descending `time_updated` (session 0 is the newest)
/// so the listing's most-recent-first ordering is deterministic, and one
/// tiny message + part each so the aggregates are non-zero.
fn build_listing_fixture(root: &std::path::Path, count: usize) {
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
            id text PRIMARY KEY, message_id text NOT NULL, session_id text NOT NULL,
            time_created integer NOT NULL, time_updated integer NOT NULL,
            data text NOT NULL);",
    )
    .unwrap();
    let now = now_ms();
    for i in 0..count {
        let sid = format!("ses_listing{i:03}0000000000000000000000aa");
        let updated = now - (i as i64) * 1_000;
        conn.execute(
            "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
             VALUES (?1, NULL, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                &sid,
                format!("/Users/dev/listing{i:03}"),
                format!("listing session {i}"),
                updated,
                updated
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?3, '{\"role\":\"user\",\"time\":{\"created\":1}}')",
            rusqlite::params![format!("lmsg{i:03}"), &sid, updated],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?2, ?3, ?3, ?4)",
            rusqlite::params![
                format!("lpart{i:03}"),
                &sid,
                updated,
                serde_json::json!({"type":"text","text":format!("listing body {i}")}).to_string()
            ],
        )
        .unwrap();
    }
}

fn spawn() -> (Mcp, common::scratch::ScratchRoot) {
    spawn_fixture(build_fixture)
}

fn spawn_fixture(build: impl FnOnce(&std::path::Path)) -> (Mcp, common::scratch::ScratchRoot) {
    let dir = scratch("mcp_flood");
    build(&dir);
    let cwd = child_cwd("mcp_flood");
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
    mcp.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                   "clientInfo": {"name": "flood-test", "version": "0"}}
    }));
    let _ = mcp.read_until_id(0);
    mcp.send(&serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    (mcp, dir)
}

fn reap(mcp: &mut Mcp) {
    mcp.child.kill().ok();
    mcp.child.wait().ok();
    let _ = std::fs::remove_dir_all(&mcp.cwd);
}

fn marker_path(text: &str) -> std::path::PathBuf {
    let line = text
        .lines()
        .find(|l| l.starts_with("full_report: "))
        .expect("marker names the full report path");
    std::path::PathBuf::from(line.trim_start_matches("full_report: ").trim())
}

#[test]
fn every_session_scoped_tool_takes_session_id_and_the_list_is_cut_off() {
    let (mut mcp, _dir) = spawn();
    let tools = mcp.tools_list();
    let names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for expected in [
        "she_said_he_said_action",
        "index_sessions",
        "do_android_dream_of_electric_sheep",
    ] {
        let tool = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"].as_str() == Some(expected))
            .unwrap_or_else(|| panic!("{expected} registered"));
        let props = tool["inputSchema"]["properties"].as_object().unwrap();
        assert!(
            props.contains_key("session_id"),
            "{expected} takes session_id"
        );
        assert!(
            !props.contains_key("sessions"),
            "{expected} no sessions array"
        );
    }
    assert!(
        names.contains(&"line_histogram"),
        "line_histogram is registered"
    );
    reap(&mut mcp);
}

#[test]
fn she_said_with_session_id_scans_only_that_session() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = mcp.call(
        "she_said_he_said_action",
        serde_json::json!({"words": ["release"], "session_id": "ses_alpha"}),
    );
    assert!(!err, "she_said must succeed: {text}");
    assert!(
        text.contains("ses_alpha"),
        "the alpha session is in the report"
    );
    assert!(
        !text.contains("ses_beta"),
        "the beta session must NOT be scanned"
    );
    reap(&mut mcp);
}

#[test]
fn she_said_overflow_returns_the_marker_and_keeps_the_whole_report() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = mcp.call(
        "she_said_he_said_action",
        serde_json::json!({"words": ["release"], "session_id": "ses_alpha", "max_bytes": 200}),
    );
    assert!(!err, "the capped call succeeds: {text}");
    assert!(
        text.contains("--- [EOF-TRUNCATED] ---"),
        "the marker is present"
    );
    assert!(text.contains("full_report: "), "the marker names the file");
    assert!(
        text.contains("Bucket Distribution"),
        "the marker carries the histogram"
    );
    let head = text.split("--- [EOF-TRUNCATED] ---").next().unwrap();
    assert!(
        head.len() <= 200,
        "the returned head respects max_bytes: {}",
        head.len()
    );
    let path = marker_path(&text);
    let full = std::fs::read_to_string(&path).unwrap();
    assert!(
        full.contains("release alpha finding 19"),
        "the overflow file carries the whole report, past the cut"
    );
    assert!(full.len() > head.len(), "the file is bigger than the head");
    reap(&mut mcp);
}

#[test]
fn list_sessions_default_cutoff_is_240_hours() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = mcp.call("list_sessions", serde_json::json!({}));
    assert!(!err, "list_sessions succeeds: {text}");
    let v: Value =
        serde_json::from_str(&text).expect("the default listing is returned inline, untorn");
    assert_eq!(
        v["window_count"].as_u64(),
        Some(3),
        "the 240-hour default holds the three recent sessions: {text}"
    );
    assert!(
        !whole_listing(&text).contains("ses_old"),
        "the epoch-old session is cut off by the 240-hour default"
    );
    assert!(text.contains("ses_alpha"), "the recent session is listed");
    let (err, text) = mcp.call("list_sessions", serde_json::json!({"all": true}));
    assert!(!err, "all: true succeeds: {text}");
    let v: Value =
        serde_json::from_str(&text).expect("the whole-store listing is returned inline, untorn");
    assert_eq!(
        v["window_count"].as_u64(),
        Some(4),
        "all: true lifts the window to the whole store: {text}"
    );
    assert_eq!(
        v["held_back"].as_u64(),
        Some(2),
        "the 20,000-character-title row does not fit the default budget and the rows behind it are held back with it, and the listing says so: {text}"
    );
    reap(&mut mcp);
}

#[test]
fn an_overflowing_json_response_is_never_torn() {
    let (mut mcp, _dir) = spawn();
    // A budget too small for even the first row is the one listing the byte
    // budget cannot satisfy: the whole listing goes to flood control, whose
    // contract is that JSON is never torn.
    let (err, text) = mcp.call("list_sessions", serde_json::json!({"max_bytes": 100}));
    assert!(!err, "the overflowing list still succeeds: {text}");
    assert!(
        text.contains("--- [EOF-TRUNCATED] ---"),
        "a listing that cannot fit its budget overflows and returns the marker: {text}"
    );
    assert!(
        !text.trim_start().starts_with('['),
        "an overflowing JSON response must NOT return torn JSON"
    );
    let path = marker_path(&text);
    let written = std::fs::read_to_string(&path).unwrap();
    serde_json::from_str::<serde_json::Value>(&written)
        .expect("the overflow file carries the whole, untorn JSON");
    assert!(
        written.contains("ses_huge"),
        "the file has the huge session"
    );
    // the line_histogram companion pages the overflow file:
    let (err, hist) = mcp.call(
        "line_histogram",
        serde_json::json!({"file_path": path.to_string_lossy()}),
    );
    assert!(
        !err,
        "line_histogram succeeds over the overflow file: {hist}"
    );
    assert!(
        hist.contains("Bucket Distribution"),
        "histogram mode output"
    );
    let (err, lines) = mcp.call(
        "line_histogram",
        serde_json::json!({"file_path": path.to_string_lossy(), "mode": "extract", "start": 1, "end": 3}),
    );
    assert!(!err, "extract mode succeeds: {lines}");
    assert!(!lines.trim().is_empty(), "extract returns the lines");
    reap(&mut mcp);
}

/// The whole listing text, whether it fit under the flood cap or overflowed
/// into the marker's file.
fn whole_listing(text: &str) -> String {
    if text.contains("full_report: ") {
        let path = marker_path(text);
        std::fs::read_to_string(&path).expect("readable overflow file")
    } else {
        text.to_string()
    }
}

/// The listing cap and its held-back statement (the scope contract): the
/// listing renders as many of the most recent rows of the window as its
/// `max_bytes` response budget holds — never more than the 200 rows the store
/// query bounds — and states how many rows the window holds but did not
/// print. A default call returns rows: the budget, not the 16 KiB flood cap,
/// decides the row count.
#[test]
fn list_sessions_renders_what_the_budget_holds_and_names_the_held_back_count() {
    let (mut mcp, _dir) = spawn_fixture(|root| build_listing_fixture(root, 250));
    let (err, text) = mcp.call("list_sessions", serde_json::json!({}));
    assert!(!err, "list_sessions succeeds: {text}");
    assert!(
        !text.contains("--- [EOF-TRUNCATED] ---"),
        "the default listing fits its own budget: {text}"
    );
    let v: Value = serde_json::from_str(&text).expect("the listing response is whole, untorn JSON");
    let sessions = v["sessions"].as_array().unwrap_or_else(|| {
        panic!(
            "the listing must render its rows under `sessions`: {}",
            &text[..text.len().min(400)]
        )
    });
    assert!(
        !sessions.is_empty(),
        "a default call returns rows, not an empty table: {text}"
    );
    assert!(
        sessions.len() <= 200,
        "the listing must render at most the 200 rows the query bounds, got {}",
        sessions.len()
    );
    assert!(
        text.len() <= 16_384,
        "the rendered listing must fit the default budget, got {} bytes",
        text.len()
    );
    assert_eq!(
        v["window_count"].as_u64(),
        Some(250),
        "the listing must state how many rows the window holds, not just print rows"
    );
    assert_eq!(
        v["held_back"].as_u64(),
        Some(250 - sessions.len() as u64),
        "the listing must state the rows the window holds but did not print"
    );
    assert_eq!(
        sessions[0]["session_id"].as_str(),
        Some("ses_listing0000000000000000000000000aa"),
        "the first row must be the most recent session of the window"
    );

    // A bigger budget buys a bigger slice of the same window: the row count
    // is derived from the budget, not from a fixed cap.
    let (err, wider) = mcp.call("list_sessions", serde_json::json!({"max_bytes": 262_144}));
    assert!(!err, "a wider budget succeeds: {wider}");
    let wide: Value = serde_json::from_str(&wider).expect("whole JSON");
    let wide_rows = wide["sessions"].as_array().unwrap().len();
    assert!(
        wide_rows > sessions.len(),
        "a wider budget must return more rows: {} vs {}",
        wide_rows,
        sessions.len()
    );
    assert_eq!(
        wide["window_count"].as_u64(),
        Some(250),
        "the window is the same window"
    );
    reap(&mut mcp);
}

/// A budget too small for the first row is still handed to flood control
/// whole: the listing is never answered with an empty table, and the overflow
/// file carries the untorn JSON the marker points at.
#[test]
fn a_budget_too_small_for_one_row_still_floods_whole() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = mcp.call("list_sessions", serde_json::json!({"max_bytes": 100}));
    assert!(!err, "the oversized listing still succeeds: {text}");
    assert!(
        text.contains("--- [EOF-TRUNCATED] ---"),
        "a row larger than the budget overflows and returns the marker: {text}"
    );
    let written = whole_listing(&text);
    let v: Value =
        serde_json::from_str(&written).expect("the overflow file carries the whole, untorn JSON");
    assert!(
        v["sessions"].as_array().is_some_and(|rows| rows
            .iter()
            .any(|r| r["session_id"] == "ses_huge000000000000000000000000d")),
        "the overflow file carries the rows a 16 KiB budget could not: {written}"
    );
    reap(&mut mcp);
}
