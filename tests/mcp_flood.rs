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
        // a title so large that list_sessions (hours_back 0) overflows the
        // default 16 KiB window: the JSON-tear contract, proven for real
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
    // old: one part so it has content when listed at hours_back 0
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

fn spawn() -> (Mcp, common::scratch::ScratchRoot) {
    let dir = scratch("mcp_flood");
    build_fixture(&dir);
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
        serde_json::json!({"words": ["release"], "session_id": "ses_alpha", "hours_back": 0}),
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
        serde_json::json!({"words": ["release"], "session_id": "ses_alpha", "hours_back": 0, "max_bytes": 200}),
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
    // The fixture's huge-title session overflows the default window, so the
    // return is the marker; the cutoff contract lives in the overflow file.
    let (err, text) = mcp.call("list_sessions", serde_json::json!({}));
    assert!(!err, "list_sessions succeeds: {text}");
    let path = marker_path(&text);
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        !written.contains("ses_old"),
        "the epoch-old session is cut off by the 240-hour default"
    );
    assert!(
        written.contains("ses_alpha"),
        "the recent session is listed"
    );
    let (err, text) = mcp.call("list_sessions", serde_json::json!({"hours_back": 0}));
    assert!(!err);
    let path = marker_path(&text);
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("ses_old"), "hours_back 0 lifts the cutoff");
    reap(&mut mcp);
}

#[test]
fn an_overflowing_json_response_is_never_torn() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = mcp.call("list_sessions", serde_json::json!({"hours_back": 0}));
    assert!(!err, "the overflowing list still succeeds: {text}");
    assert!(
        text.contains("--- [EOF-TRUNCATED] ---"),
        "the huge-title list overflows and returns the marker"
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
