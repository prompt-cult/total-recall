//! The `he_said_she_said_they_did` MCP tool, proven through the real stdio
//! server against an opencode fixture store: HE SAID is verbatim user text
//! (synthetic parts skipped, compaction markers excluded), SHE SAID is
//! assistant text only (no reasoning, no tool results), THEY DID is every
//! action without its result (built-in, MCP-ish and todo tool calls alike)
//! with compaction boundaries inline, and the context section carries the
//! period, the compaction points and the summary-cache blocks. Written RED.

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

/// One session (newest) carrying every part kind the report distinguishes:
/// user text, synthetic text, reasoning, assistant text, a bash tool call
/// with its result, an MCP-ish tool call, a todo tool call, and a compaction
/// part. A second, older session makes the most-recent resolution real.
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

    let sessions: Vec<(&str, &str, &str, i64)> = vec![
        (
            "ses_alpha0000000000000000000000aa",
            "/Users/dev/alpha",
            "alpha session",
            now,
        ),
        (
            "ses_beta000000000000000000000000bb",
            "/Users/dev/beta",
            "beta session",
            now - 86_400_000,
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

    // alpha: one message per part kind, in time order; the role lives in the
    // message data, the timestamp in `time.created`.
    let msgs: Vec<(&str, &str, &str, i64)> = vec![
        (
            "amsg1",
            "ses_alpha0000000000000000000000aa",
            "user",
            now - 600_000,
        ),
        (
            "amsg2",
            "ses_alpha0000000000000000000000aa",
            "assistant",
            now - 500_000,
        ),
        (
            "amsg3",
            "ses_beta000000000000000000000000bb",
            "user",
            now - 86_400_000,
        ),
    ];
    for (id, sid, role, t) in &msgs {
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?3, ?4)",
            rusqlite::params![
                id,
                sid,
                t,
                serde_json::json!({"role": role, "time": {"created": t}}).to_string()
            ],
        )
        .unwrap();
    }

    let sid = "ses_alpha0000000000000000000000aa";
    let parts: Vec<(&str, &str, &str, i64, String)> = vec![
        // user text, verbatim
        ("apart1", "amsg1", sid, now - 600_000,
         serde_json::json!({"type":"text","text":"Please run git status for me"}).to_string()),
        // synthetic text part: HE SAID skips it
        ("apart2", "amsg1", sid, now - 590_000,
         serde_json::json!({"type":"text","synthetic":true,"text":"environment hint injected"}).to_string()),
        // assistant text: SHE SAID shows it
        ("apart3", "amsg2", sid, now - 500_000,
         serde_json::json!({"type":"text","text":"Here is the branch state for you"}).to_string()),
        // reasoning: SHE SAID never shows it
        ("apart4", "amsg2", sid, now - 490_000,
         serde_json::json!({"type":"reasoning","text":"reasoning about the branch"}).to_string()),
        // built-in tool call + its result: THEY DID shows the call, not "status output"
        ("apart5", "amsg2", sid, now - 480_000,
         serde_json::json!({"type":"tool","tool":"bash","state":{"status":"completed",
             "input":{"command":"git status"},"output":"status output the build was clean"}}).to_string()),
        // an MCP-ish tool call: anything tool-like is an action
        ("apart6", "amsg2", sid, now - 470_000,
         serde_json::json!({"type":"tool","tool":"do_android_dream_of_electric_sheep",
             "state":{"status":"completed","input":{"query":"branch"},"output":"hits"}}).to_string()),
        // a todo action: also just a tool-like part
        ("apart7", "amsg2", sid, now - 460_000,
         serde_json::json!({"type":"tool","tool":"todowrite",
             "state":{"status":"completed","input":{"todos":["item00: ship"]},"output":"todo list written"}}).to_string()),
        // compaction part: the boundary, inline in THEY DID
        ("apart8", "amsg2", sid, now - 450_000,
         serde_json::json!({"type":"compaction"}).to_string()),
    ];
    for (id, mid, psid, t, data) in parts {
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            rusqlite::params![id, mid, psid, t, data],
        )
        .unwrap();
    }

    // beta: one user text so the most-recent resolution is observable
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
         VALUES ('bpart1', 'amsg3', 'ses_beta000000000000000000000000bb', ?1, ?1,
                 '{\"type\":\"text\",\"text\":\"git in beta\"}')",
        [now - 86_400_000],
    )
    .unwrap();
}

/// A pre-seeded summary-cache ledger for alpha: blocks 1..=2 summarised, so
/// the context section reports the ranges and the high watermark (id 3).
fn seed_ledger(root: &std::path::Path) {
    let shadow = root.join(".tantivy");
    std::fs::create_dir_all(&shadow).unwrap();
    let ledger = shadow.join("tr_ses_alpha0000000000000000000000aa_summary.csv");
    std::fs::write(
        ledger,
        format!(
            "1,{ts},2,{ts},1,0,0,0,40,10,{part}\n",
            ts = "2020-01-01T00:00:00Z",
            part = root.join(".tantivy").join("parts_p0.bin").display()
        ),
    )
    .unwrap();
}

struct Mcp {
    child: std::process::Child,
    reader: std::io::BufReader<std::process::ChildStdout>,
    stdin: std::process::ChildStdin,
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
    let dir = scratch("he_said_she_said");
    build_fixture(&dir);
    seed_ledger(&dir);
    let cwd = child_cwd("he_said_she_said");
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
        next: 1,
    };
    mcp.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                   "clientInfo": {"name": "he-said-test", "version": "0"}}
    }));
    let _ = mcp.read_until_id(0);
    mcp.send(&serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    (mcp, dir)
}

fn reap(mcp: &mut Mcp) {
    mcp.child.kill().ok();
    mcp.child.wait().ok();
}

fn call_he_said(mcp: &mut Mcp, args: Value) -> (bool, String) {
    mcp.call("he_said_she_said_they_did", args)
}

#[test]
fn tool_is_registered() {
    let (mut mcp, _dir) = spawn();
    let tools = mcp.tools_list();
    let names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        names.contains(&"he_said_she_said_they_did"),
        "he_said_she_said_they_did is registered in tools/list: {names:?}"
    );
    reap(&mut mcp);
}

#[test]
fn he_said_is_verbatim_user_text_without_synthetic_parts() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = call_he_said(&mut mcp, serde_json::json!({"session_id": "ses_alpha"}));
    assert!(!err, "call succeeds: {text}");
    assert!(text.contains("## HE SAID"), "{}", text);
    assert!(
        text.contains("Please run git status for me"),
        "verbatim user text: {}",
        text
    );
    assert!(
        !text.contains("environment hint injected"),
        "synthetic part is skipped: {}",
        text
    );
    assert!(
        !text.contains("context compaction"),
        "the injected compaction marker is not HE SAID content: {}",
        text
    );
    reap(&mut mcp);
}

#[test]
fn she_said_is_assistant_text_only() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = call_he_said(&mut mcp, serde_json::json!({"session_id": "ses_alpha"}));
    assert!(!err, "call succeeds: {text}");
    assert!(text.contains("## SHE SAID"), "{}", text);
    assert!(
        text.contains("Here is the branch state for you"),
        "assistant text: {}",
        text
    );
    assert!(
        !text.contains("reasoning about the branch"),
        "thinking must never appear in SHE SAID: {}",
        text
    );
    assert!(
        !text.contains("status output the build was clean"),
        "tool results must never appear in SHE SAID: {}",
        text
    );
    reap(&mut mcp);
}

#[test]
fn they_did_is_every_action_without_results() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = call_he_said(&mut mcp, serde_json::json!({"session_id": "ses_alpha"}));
    assert!(!err, "call succeeds: {text}");
    assert!(text.contains("## THEY DID"), "{}", text);
    assert!(
        text.contains("bash: git status"),
        "built-in call by name+args: {}",
        text
    );
    assert!(
        text.contains("do_android_dream_of_electric_sheep("),
        "MCP-style call: {}",
        text
    );
    assert!(text.contains("todowrite("), "todo action: {}", text);
    assert!(
        !text.contains("status output the build was clean"),
        "tool result never appears in THEY DID: {}",
        text
    );
    reap(&mut mcp);
}

#[test]
fn compaction_boundary_is_inline() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = call_he_said(&mut mcp, serde_json::json!({"session_id": "ses_alpha"}));
    assert!(!err, "call succeeds: {text}");
    assert!(
        text.contains("COMPACTION BOUNDARY"),
        "the boundary is named inline in the report: {}",
        text
    );
    reap(&mut mcp);
}

#[test]
fn context_carries_period_compaction_points_and_cache() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = call_he_said(&mut mcp, serde_json::json!({"session_id": "ses_alpha"}));
    assert!(!err, "call succeeds: {text}");
    // The period: bounded by first and last message timestamps.
    assert!(text.contains("PERIOD:"), "the period is stated: {}", text);
    assert!(
        text.lines().any(|l| l.contains("## CONTEXT")),
        "the context section exists: {}",
        text
    );
    assert!(
        text.contains("compaction points: 1"),
        "the compaction-point count is stated: {}",
        text
    );
    // The summary-cache ledger was seeded with one block 1..=2, so the context
    // names the block and the high watermark (3): summarise_from id guarded.
    assert!(
        text.contains("1..=2"),
        "the cached block's id range is stated: {}",
        text
    );
    assert!(
        text.contains("high watermark, is 3"),
        "the first unsummarised id is stated: {}",
        text
    );
    reap(&mut mcp);
}

#[test]
fn empty_session_id_resolves_the_most_recent_session() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = call_he_said(&mut mcp, serde_json::json!({}));
    assert!(!err, "call succeeds: {text}");
    assert!(
        text.contains("ses_alpha0000000000000000000000aa"),
        "the newest session is served by default: {}",
        text
    );
    assert!(
        !text.contains("git in beta"),
        "one report is one session: {}",
        text
    );
    reap(&mut mcp);
}

#[test]
fn flood_control_eof_marker_names_the_overflow_file() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = call_he_said(
        &mut mcp,
        serde_json::json!({"session_id": "ses_alpha", "max_bytes": 100}),
    );
    assert!(!err, "overflow is not an error: {text}");
    assert!(
        text.contains("EOF-TRUNCATED"),
        "the EOF marker replaces the overflowed report: {}",
        text
    );
    assert!(
        text.contains("full_report"),
        "the overflow file is named: {}",
        text
    );
    reap(&mut mcp);
}

#[test]
fn max_bytes_out_of_range_is_a_bound_error() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = call_he_said(
        &mut mcp,
        serde_json::json!({"session_id": "ses_alpha", "max_bytes": 0}),
    );
    assert!(err, "out-of-range max_bytes is an error");
    assert!(
        text.contains("max_bytes") && text.contains("1 to 8388608"),
        "the bound is stated in the house form: {text}"
    );
    reap(&mut mcp);
}
