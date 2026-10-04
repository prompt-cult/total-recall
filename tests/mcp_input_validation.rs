//! #25: the input-validation contract, proven through the real MCP stdio
//! server against an opencode fixture store and read from the SERVED
//! `tools/list` schemas, so the schema and the handler cannot drift apart
//! again:
//!
//! * `hours_back: 0` is the tool's default window on every window tool —
//!   never an error, never the whole store — while `all: true` with a real
//!   window is still the contradiction it is.
//! * The served schema states the bounds the handlers enforce: `minimum: 1`
//!   on every window tool's `hours_back`, `maximum` on `limit` and
//!   `max_bytes`, `minimum: 0` on the 0-based inputs, `minimum: 1` on the
//!   1-based line selectors.
//! * An unknown parameter is rejected by name, naming the fields the tool
//!   does accept — a typo is a silently unscoped call otherwise.
//! * Every numeric input out of range returns the house form
//!   (`<tool>: `<field>` is out of range — <cheap form>`), not a raw serde
//!   type error, and `line_histogram`'s mode/line selector sets are
//!   validated as a set.
//!
//! Written RED.

mod common;

use common::scratch::{child_cwd, scratch};
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use std::io::{BufRead, Write};
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// An opencode fixture with two sessions updated now and one updated at the
/// unix epoch, each with a message and a matching text part. The epoch-old
/// session is how "the default window" and "the whole store" are told apart.
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
    let sessions: Vec<(&str, &str, i64, &str)> = vec![
        (
            "ses_alpha0000000000000000000000aa",
            "/Users/dev/alpha",
            now,
            "release alpha finding",
        ),
        (
            "ses_beta00000000000000000000000bb",
            "/Users/dev/beta",
            now,
            "release beta finding",
        ),
        (
            "ses_old0000000000000000000000000c",
            "/Users/dev/old",
            4000,
            "release old finding from the store",
        ),
    ];
    for (id, dir, updated, text) in &sessions {
        conn.execute(
            "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
             VALUES (?1, NULL, ?2, ?3, ?4, ?4)",
            rusqlite::params![id, dir, format!("{} session", &id[4..9]), updated],
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
                json!({"type":"text","text":text}).to_string()
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

fn spawn() -> (Mcp, common::scratch::ScratchRoot) {
    let dir = scratch("mcp_input_validation");
    build_fixture(&dir);
    std::fs::write(dir.join("lines.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    let cwd = child_cwd("mcp_input_validation");
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
                   "clientInfo": {"name": "input-validation-test", "version": "0"}}
    }));
    let _ = mcp.read_until_id(0);
    mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    (mcp, dir)
}

/// The fixture's five-line file, substituted into the `{FILE}` placeholder the
/// case tables below carry.
fn file_args(v: &Value, dir: &std::path::Path) -> Value {
    let file = dir.join("lines.txt").to_string_lossy().to_string();
    serde_json::from_str(&v.to_string().replace("{FILE}", &file)).expect("args are JSON")
}

fn reap(mcp: &mut Mcp) {
    mcp.child.kill().ok();
    mcp.child.wait().ok();
    let _ = std::fs::remove_dir_all(&mcp.cwd);
}

/// Every tool that takes parameters, with the arguments that get it past its
/// required fields.
fn tools() -> [(&'static str, Value); 11] {
    [
        ("list_sessions", json!({})),
        ("profile_session", json!({})),
        ("extract_messages", json!({})),
        ("extract_user_messages", json!({})),
        ("extract_by_type", json!({})),
        ("compact_session", json!({})),
        ("total_recall", json!({})),
        ("she_said_he_said_action", json!({"words": ["release"]})),
        ("index_sessions", json!({})),
        (
            "do_android_dream_of_electric_sheep",
            json!({"query": "release"}),
        ),
        ("line_histogram", json!({"file_path": "{FILE}"})),
    ]
}

const WINDOW_TOOLS: [&str; 4] = [
    "list_sessions",
    "she_said_he_said_action",
    "index_sessions",
    "do_android_dream_of_electric_sheep",
];

const CONTRADICTION: &str =
    "all: true is the whole store; hours_back is a window — pass one, not both";

/// Every window tool's `hours_back: 0` call, per tool, with the field its
/// required arguments need.
fn zero_hours_call(tool: &str) -> Value {
    match tool {
        "she_said_he_said_action" => json!({"words": ["release"], "hours_back": 0}),
        "do_android_dream_of_electric_sheep" => json!({"query": "release", "hours_back": 0}),
        _ => json!({"hours_back": 0}),
    }
}

fn all_with(hours: Value) -> Value {
    json!({"all": true, "hours_back": hours})
}

#[test]
fn hours_back_zero_is_the_default_window_on_every_window_tool() {
    let (mut mcp, _dir) = spawn();
    // The sheep search needs an index; build one over the whole store so the
    // window is the only thing that can exclude the epoch-old session.
    let (err, text) = mcp.call("index_sessions", json!({"all": true}));
    assert!(!err, "whole-store indexing succeeds: {text}");

    for tool in WINDOW_TOOLS {
        let (err, text) = mcp.call(tool, zero_hours_call(tool));
        assert!(
            !err,
            "{tool} with hours_back: 0 is the default window, never an error: {text}"
        );
        assert!(
            !text.contains("ses_old"),
            "{tool} with hours_back: 0 must stay inside the default window: {text}"
        );
        assert!(
            !text.contains("[EOF-TRUNCATED]"),
            "{tool} with hours_back: 0 returns a normal response: {text}"
        );
    }
    reap(&mut mcp);
}

#[test]
fn all_true_with_zero_hours_is_the_whole_store_and_a_real_window_still_contradicts() {
    let (mut mcp, _dir) = spawn();
    let (err, text) = mcp.call("index_sessions", json!({"all": true}));
    assert!(!err, "whole-store indexing succeeds: {text}");
    for tool in WINDOW_TOOLS {
        let args = match tool {
            "she_said_he_said_action" => {
                json!({"words": ["release"], "all": true, "hours_back": 0})
            }
            "do_android_dream_of_electric_sheep" => {
                json!({"query": "release", "all": true, "hours_back": 0})
            }
            _ => all_with(json!(0)),
        };
        let (err, text) = mcp.call(tool, args);
        assert!(
            !err,
            "{tool} with all: true and hours_back: 0 is the store the caller asked for: {text}"
        );
        assert!(
            text.contains("ses_old"),
            "{tool} with all: true reaches outside every window: {text}"
        );

        let args = match tool {
            "she_said_he_said_action" => {
                json!({"words": ["release"], "all": true, "hours_back": 24})
            }
            "do_android_dream_of_electric_sheep" => {
                json!({"query": "release", "all": true, "hours_back": 24})
            }
            _ => all_with(json!(24)),
        };
        let (err, text) = mcp.call(tool, args);
        assert!(
            err,
            "{tool} must still reject all: true with a real window: {text}"
        );
        assert!(
            text.contains(CONTRADICTION),
            "{tool} must name the contradiction: {text}"
        );
    }
    reap(&mut mcp);
}

#[test]
fn served_schemas_state_the_bounds_the_handlers_enforce() {
    let (mut mcp, _dir) = spawn();
    // Every window tool states minimum 1 on hours_back, so a schema-honouring
    // client cannot send the 0 that the handler reads as "omitted".
    let mut window_tools = WINDOW_TOOLS.to_vec();
    window_tools.push("total_recall");
    for tool in window_tools {
        let props = mcp.schema(tool);
        let hours = &props["hours_back"];
        assert_eq!(
            hours["minimum"].as_f64(),
            Some(1.0),
            "{tool} hours_back must state minimum 1: {hours}"
        );
        assert!(
            hours["description"]
                .as_str()
                .unwrap_or_default()
                .contains("1 or more"),
            "{tool} hours_back description must state the bound: {hours}"
        );
    }
    // The numeric bounds the handlers enforce.
    let cases: [(&str, &str, Option<f64>, Option<f64>); 14] = [
        ("extract_messages", "limit", Some(0.0), Some(1000.0)),
        ("extract_messages", "offset", Some(0.0), None),
        (
            "extract_messages",
            "max_bytes",
            Some(0.0),
            Some(8_388_608.0),
        ),
        (
            "extract_messages",
            "max_record_bytes",
            Some(0.0),
            Some(8_388_608.0),
        ),
        ("extract_user_messages", "limit", Some(0.0), Some(1000.0)),
        ("extract_user_messages", "offset", Some(0.0), None),
        ("extract_by_type", "limit", Some(0.0), Some(1000.0)),
        ("extract_by_type", "max_bytes", Some(0.0), Some(8_388_608.0)),
        ("list_sessions", "max_bytes", Some(1.0), Some(8_388_608.0)),
        (
            "she_said_he_said_action",
            "max_bytes",
            Some(1.0),
            Some(8_388_608.0),
        ),
        (
            "do_android_dream_of_electric_sheep",
            "max_bytes",
            Some(1.0),
            Some(8_388_608.0),
        ),
        ("total_recall", "max_bytes", Some(1.0), Some(8_388_608.0)),
        ("line_histogram", "line", Some(1.0), None),
        ("line_histogram", "start", Some(1.0), None),
    ];
    for (tool, field, min, max) in cases {
        let props = mcp.schema(tool);
        let prop = props
            .get(field)
            .unwrap_or_else(|| panic!("{tool} takes {field}"));
        assert_eq!(
            prop["minimum"].as_f64(),
            min,
            "{tool}.{field} minimum must state the enforced bound: {prop}"
        );
        assert_eq!(
            prop["maximum"].as_f64(),
            max,
            "{tool}.{field} maximum must state the enforced ceiling: {prop}"
        );
    }
    // line_histogram's `end` is 1-based like `start`.
    let props = mcp.schema("line_histogram");
    assert_eq!(props["end"]["minimum"].as_f64(), Some(1.0));
    reap(&mut mcp);
}

#[test]
fn every_tool_rejects_an_unknown_field_by_name_and_lists_what_it_accepts() {
    let (mut mcp, dir) = spawn();
    for (tool, base) in tools() {
        let base = file_args(&base, &dir);
        let accepted = mcp.schema(tool);
        let mut args = base.clone();
        let args_map = args.as_object_mut().expect("object args");
        args_map.insert("hour_back".to_string(), json!(24));
        let (err, text) = mcp.call(tool, args);
        assert!(
            err,
            "{tool} must reject the typo `hour_back` rather than run on the default scope: {text}"
        );
        assert!(
            text.contains("unrecognised field `hour_back`"),
            "{tool} must name the unrecognised field: {text}"
        );
        for field in accepted.keys() {
            assert!(
                text.contains(field.as_str()),
                "{tool} must list the accepted field `{field}`: {text}"
            );
        }
        assert!(
            !text.contains("failed to deserialize parameters"),
            "{tool} must not surface a raw serde dump: {text}"
        );
    }
    reap(&mut mcp);
}

#[test]
fn every_numeric_input_out_of_range_returns_the_house_form() {
    let (mut mcp, dir) = spawn();
    // (tool, args, the field the error must name)
    let cases: [(&str, Value, &str); 22] = [
        ("list_sessions", json!({"hours_back": -1}), "hours_back"),
        ("list_sessions", json!({"max_bytes": -1}), "max_bytes"),
        (
            "list_sessions",
            json!({"max_bytes": 9_000_000}),
            "max_bytes",
        ),
        (
            "she_said_he_said_action",
            json!({"words": ["release"], "hours_back": -1}),
            "hours_back",
        ),
        (
            "she_said_he_said_action",
            json!({"words": ["release"], "max_bytes": 9_000_000}),
            "max_bytes",
        ),
        ("index_sessions", json!({"hours_back": -7}), "hours_back"),
        (
            "do_android_dream_of_electric_sheep",
            json!({"query": "release", "hours_back": -1}),
            "hours_back",
        ),
        (
            "do_android_dream_of_electric_sheep",
            json!({"query": "release", "max_bytes": -3}),
            "max_bytes",
        ),
        ("extract_messages", json!({"limit": -1}), "limit"),
        ("extract_messages", json!({"limit": 1001}), "limit"),
        ("extract_messages", json!({"offset": -1}), "offset"),
        (
            "extract_messages",
            json!({"max_bytes": 9_000_000}),
            "max_bytes",
        ),
        (
            "extract_messages",
            json!({"max_record_bytes": -1}),
            "max_record_bytes",
        ),
        (
            "extract_messages",
            json!({"max_record_bytes": 9_000_000}),
            "max_record_bytes",
        ),
        ("extract_user_messages", json!({"limit": 1001}), "limit"),
        ("extract_user_messages", json!({"offset": -5}), "offset"),
        ("extract_by_type", json!({"limit": -2}), "limit"),
        ("extract_by_type", json!({"limit": 5000}), "limit"),
        ("total_recall", json!({"hours_back": -1}), "hours_back"),
        ("total_recall", json!({"max_bytes": 9_000_000}), "max_bytes"),
        (
            "line_histogram",
            json!({"file_path": "{FILE}", "line": 0}),
            "line",
        ),
        (
            "line_histogram",
            json!({"file_path": "{FILE}", "start": 0, "end": 4}),
            "start",
        ),
    ];
    for (tool, args, field) in cases {
        let args = file_args(&args, &dir);
        let (err, text) = mcp.call(tool, args.clone());
        assert!(err, "{tool} {args} must be a tool error: {text}");
        assert!(
            text.starts_with(&format!("{tool}: ")),
            "{tool} must speak in the house form: {text}"
        );
        assert!(
            text.contains(&format!("`{field}` is out of range")),
            "{tool} {args} must name `{field}` as out of range: {text}"
        );
        assert!(
            !text.contains("invalid type") && !text.contains("failed to deserialize"),
            "{tool} must not surface a raw serde error: {text}"
        );
    }
    reap(&mut mcp);
}

#[test]
fn line_histogram_validates_mode_and_the_line_selectors_as_a_set() {
    let (mut mcp, dir) = spawn();
    let cases: [(&str, Value, &str); 6] = [
        (
            "mode is not a mode",
            json!({"file_path": "{FILE}", "mode": "extractt"}),
            "unknown mode `extractt`",
        ),
        (
            "extract with no selector",
            json!({"file_path": "{FILE}", "mode": "extract"}),
            "mode: extract needs",
        ),
        (
            "end without start",
            json!({"file_path": "{FILE}", "mode": "extract", "end": 4}),
            "`end` needs `start`",
        ),
        (
            "start past end",
            json!({"file_path": "{FILE}", "mode": "extract", "start": 9, "end": 4}),
            "past",
        ),
        (
            "a selector without mode: extract",
            json!({"file_path": "{FILE}", "start": 1, "end": 2}),
            "mode: extract",
        ),
        (
            "line and range together",
            json!({"file_path": "{FILE}", "mode": "extract", "line": 2, "start": 1, "end": 4}),
            "one",
        ),
    ];
    for (what, args, expected) in cases {
        let args = file_args(&args, &dir);
        let (err, text) = mcp.call("line_histogram", args);
        assert!(err, "line_histogram {what} must be a tool error: {text}");
        assert!(
            text.contains(expected),
            "line_histogram {what} must say `{expected}`: {text}"
        );
        assert!(
            !text.contains("Bucket Distribution"),
            "line_histogram {what} must not answer with a histogram nobody asked for: {text}"
        );
    }
    // The legal sets still work.
    let (err, text) = mcp.call(
        "line_histogram",
        file_args(
            &json!({"file_path": "{FILE}", "mode": "extract", "start": 1, "end": 2}),
            &dir,
        ),
    );
    assert!(!err, "a legal extract range still runs: {text}");
    reap(&mut mcp);
}
