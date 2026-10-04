//! The scope contract's policy half, proven through the real MCP stdio
//! server against an opencode fixture store: `all: true` is the explicit
//! whole-store opt-in and contradicts a real window, an unscoped request
//! takes the tool's default window (the rule for an explicit `hours_back: 0`
//! — read as "omitted", never as "no bound" — is proven in
//! `mcp_input_validation.rs`), `index_sessions` defaults to the last day,
//! and the schema descriptions speak in one voice. Written RED.

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

/// An opencode fixture with two sessions updated now and one updated at the
/// unix epoch, each with a message and a matching text part.
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
    let sessions: Vec<(&str, &str, String, i64, &str)> = vec![
        (
            "ses_alpha0000000000000000000000aa",
            "/Users/dev/alpha",
            "alpha session".into(),
            now,
            "release alpha finding",
        ),
        (
            "ses_beta00000000000000000000000bb",
            "/Users/dev/beta",
            "beta session".into(),
            now,
            "release beta finding",
        ),
        (
            "ses_old0000000000000000000000000c",
            "/Users/dev/old",
            "old session".into(),
            4000,
            "an old memory from the store",
        ),
    ];
    for (id, dir, title, updated, text) in &sessions {
        conn.execute(
            "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
             VALUES (?1, NULL, ?2, ?3, ?4, ?4)",
            rusqlite::params![id, dir, title, updated],
        )
        .unwrap();
        let msg = format!("m{}", &id[..9]);
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?3, '{\"role\":\"user\",\"time\":{\"created\":1}}')",
            rusqlite::params![msg, id, updated],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            rusqlite::params![
                format!("p{}", &id[..9]),
                msg,
                id,
                updated,
                serde_json::json!({"type":"text","text":text}).to_string()
            ],
        )
        .unwrap();
    }
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
    let dir = scratch("mcp_scope");
    build_fixture(&dir);
    let cwd = child_cwd("mcp_scope");
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
                   "clientInfo": {"name": "scope-test", "version": "0"}}
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

/// The whole listing text, whether it fit under the flood cap or overflowed
/// into the marker's file.
fn whole_listing(text: &str) -> String {
    if let Some(line) = text.lines().find(|l| l.starts_with("full_report: ")) {
        let path = line.trim_start_matches("full_report: ").trim();
        std::fs::read_to_string(path).expect("readable overflow file")
    } else {
        text.to_string()
    }
}

const WINDOW_TOOLS: [&str; 4] = [
    "list_sessions",
    "she_said_he_said_action",
    "index_sessions",
    "do_android_dream_of_electric_sheep",
];

const CONTRADICTION: &str =
    "all: true is the whole store; hours_back is a window — pass one, not both";

#[test]
fn all_true_with_hours_back_is_rejected_on_every_window_tool() {
    let (mut mcp, _dir) = spawn();
    for tool in WINDOW_TOOLS {
        let (err, text) = mcp.call(
            tool,
            match tool {
                "list_sessions" => serde_json::json!({"all": true, "hours_back": 48}),
                "she_said_he_said_action" => {
                    serde_json::json!({"words": ["release"], "all": true, "hours_back": 48})
                }
                "do_android_dream_of_electric_sheep" => {
                    serde_json::json!({"query": "release", "all": true, "hours_back": 48})
                }
                _ => serde_json::json!({"all": true, "hours_back": 48}),
            },
        );
        assert!(err, "{tool} must reject all: true with hours_back: {text}");
        assert!(
            text.contains(CONTRADICTION),
            "{tool} must name the contradiction: {text}"
        );
    }
    reap(&mut mcp);
}

#[test]
fn all_true_reaches_the_whole_store_and_the_default_window_does_not() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = mcp.call("list_sessions", serde_json::json!({}));
    assert!(!err, "list_sessions succeeds: {text}");
    let written = whole_listing(&text);
    assert!(
        !written.contains("ses_old"),
        "the epoch-old session is outside the default window"
    );
    let (err, text) = mcp.call("list_sessions", serde_json::json!({"all": true}));
    assert!(!err, "all: true succeeds: {text}");
    let written = whole_listing(&text);
    assert!(
        written.contains("ses_old"),
        "all: true is the whole store, old sessions included"
    );
    let (err, text) = mcp.call(
        "she_said_he_said_action",
        serde_json::json!({"words": ["old memory"], "all": true}),
    );
    assert!(!err, "all: true she_said succeeds: {text}");
    assert!(
        text.contains("ses_old"),
        "all: true scans the sessions outside every window"
    );
    reap(&mut mcp);
}

#[test]
fn index_default_is_the_last_day_not_the_whole_store() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = mcp.call("index_sessions", serde_json::json!({}));
    assert!(!err, "the natural index call succeeds: {text}");
    assert!(
        !text.contains("indexed ses_old"),
        "the default window is 24 hours, not the whole store: {text}"
    );
    let (err, text) = mcp.call("index_sessions", serde_json::json!({"all": true}));
    assert!(!err, "all: true indexing succeeds: {text}");
    assert!(
        text.contains("indexed ses_old"),
        "whole-store indexing is the explicit opt-in: {text}"
    );
    reap(&mut mcp);
}

#[test]
fn schema_descriptions_speak_in_one_voice() {
    let (mut mcp, _dir) = spawn();
    let tools = mcp.tools_list();
    let tools = tools.as_array().unwrap();
    for tool in tools {
        let name = tool["name"].as_str().unwrap_or_default();
        let schema = &tool["inputSchema"];
        let raw = serde_json::to_string(schema).unwrap();
        assert!(
            !raw.contains("0 = no bound"),
            "{name} must not advertise '0 = no bound': {raw}"
        );
        if let Some(session_id) = schema["properties"]["session_id"].as_object() {
            let desc = session_id["description"].as_str().unwrap_or_default();
            let expected = if WINDOW_TOOLS.contains(&name) {
                "Session ID (partial match). Empty = the window (hours_back / directory / all)."
            } else {
                "Session ID (partial match). Empty = most recent."
            };
            assert_eq!(
                desc, expected,
                "{name} session_id description must speak in one voice"
            );
        }
        if WINDOW_TOOLS.contains(&name) {
            let props = schema["properties"].as_object().expect("properties");
            assert!(props.contains_key("all"), "{name} takes all: true");
            let hours_desc = props["hours_back"]["description"]
                .as_str()
                .unwrap_or_default();
            assert!(
                hours_desc.contains("1 or more") && hours_desc.contains("the default window"),
                "{name} hours_back description must state the window rule: {hours_desc}"
            );
        }
    }
    let index = tools
        .iter()
        .find(|t| t["name"] == "index_sessions")
        .expect("index_sessions registered");
    let hours_desc = index["inputSchema"]["properties"]["hours_back"]["description"]
        .as_str()
        .unwrap_or_default();
    assert!(
        hours_desc.contains("Default: 24"),
        "index_sessions defaults to the last day: {hours_desc}"
    );
    reap(&mut mcp);
}
