mod common;

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use total_recall::RolloutAdapter;
use total_recall::rollout::opencode::OpenCodeAdapter;
use total_recall::{MockAdapter, RolloutMessage, VibeAdapter};

use common::scratch::{ScratchRoot, scratch};

fn vibe_qa_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rollouts/qa_vibe_sessions")
}

fn create_qa_fixture(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session (
            id text PRIMARY KEY,
            parent_id text,
            directory text,
            title text,
            time_created integer NOT NULL,
            time_updated integer NOT NULL
        );
        CREATE TABLE message (
            id text PRIMARY KEY,
            session_id text NOT NULL,
            time_created integer NOT NULL,
            time_updated integer NOT NULL,
            data text NOT NULL
        );
        CREATE TABLE part (
            id text PRIMARY KEY,
            message_id text NOT NULL,
            session_id text NOT NULL,
            time_created integer NOT NULL,
            time_updated integer NOT NULL,
            data text NOT NULL
        );",
    )
    .unwrap();

    conn.execute(
        "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
         VALUES (?1, NULL, ?2, ?3, 1000, 5000)",
        rusqlite::params![
            "ses_qafixture0a",
            "/Users/dev/qa-fixture",
            "question fixture session"
        ],
    )
    .unwrap();

    let messages: Vec<(&str, i64, &str)> = vec![
        ("msg1", 1000, r#"{"role":"user","time":{"created":1000}}"#),
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
        (
            "msg4",
            4000,
            r#"{"role":"assistant","time":{"created":4000}}"#,
        ),
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
            rusqlite::params![id, "ses_qafixture0a", t, data],
        )
        .unwrap();
    }

    let parts: Vec<(&str, &str, i64, &str)> = vec![
        (
            "part1",
            "msg1",
            1000,
            r#"{"type":"text","text":"please set up the demo deployment"}"#,
        ),
        (
            "part2",
            "msg2",
            1500,
            r#"{"type":"text","text":"checking the deployment options"}"#,
        ),
        (
            "part3",
            "msg3",
            3000,
            r#"{"type":"tool","callID":"toolu_qa0001","tool":"question","state":{"status":"completed","input":{"questions":[{"question":"Which deployment target should the demo use?","header":"First Question","options":[{"label":"Docker Compose","description":"Single-host compose stack"},{"label":"Bare Metal","description":"Direct systemd deployment"}]},{"question":"Which license note should the sample carry?","header":"Second Question","options":[{"label":"MIT","description":"Permissive"}]}]},"output":"User has answered your questions: \"Which deployment target should the demo use?\"=\"Docker Compose\", \"Which license note should the sample carry?\"=\"a plain free text note\"","title":"Asked 2 questions","metadata":{"answers":[["Docker Compose"],["a plain free text note"]]}}}"#,
        ),
        (
            "part4",
            "msg4",
            4000,
            r#"{"type":"tool","callID":"toolu_qa0002","tool":"question","state":{"status":"pending","input":{"questions":[{"question":"Should the pending question appear?","header":"Pending Question","options":[{"label":"Yes","description":"Include it"}]}]}}}"#,
        ),
        (
            "part5",
            "msg5",
            5000,
            r#"{"type":"tool","callID":"toolu_qa0003","tool":"question","state":{"status":"completed","input":{"questions":[{"question":"Which sample port should the demo bind?","header":"Port Question","options":[{"label":"8080","description":"HTTP alternate"}]}]},"output":"User has answered your questions: \"Which sample port should the demo bind?\"=\"8080\"","title":"Asked 1 question"}}"#,
        ),
    ];
    for (id, mid, t, data) in parts {
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            rusqlite::params![id, mid, "ses_qafixture0a", t, data],
        )
        .unwrap();
    }
}

fn qa_fixture_adapter() -> (ScratchRoot, OpenCodeAdapter) {
    let dir = scratch("opencode_qa_pairs");
    let path = dir.join("fixture.db");
    create_qa_fixture(&path);
    let adapter = OpenCodeAdapter::with_root(&path);
    (dir, adapter)
}

#[test]
fn test_opencode_read_qa_pairs_returns_both_questions_with_headers_options_and_answers() {
    let (_fixture, adapter) = qa_fixture_adapter();
    let pairs = adapter
        .read_qa_pairs("ses_qafixture0a")
        .expect("healthy fixture Q/A read");

    assert_eq!(
        pairs.len(),
        3,
        "two answered questions plus the output-fallback pair: {:?}",
        pairs
    );

    let first = &pairs[0];
    assert_eq!(
        first.question,
        "Which deployment target should the demo use?"
    );
    assert_eq!(first.header.as_deref(), Some("First Question"));
    assert_eq!(first.options.len(), 2);
    assert_eq!(first.options[0].label, "Docker Compose");
    assert_eq!(first.options[0].description, "Single-host compose stack");
    assert_eq!(first.options[1].label, "Bare Metal");
    assert_eq!(first.options[1].description, "Direct systemd deployment");
    assert_eq!(first.answers, vec!["Docker Compose".to_string()]);
    assert!(
        first.timestamp.is_some(),
        "the pair carries the asking message's timestamp"
    );

    let second = &pairs[1];
    assert_eq!(
        second.question,
        "Which license note should the sample carry?"
    );
    assert_eq!(second.header.as_deref(), Some("Second Question"));
    assert_eq!(second.answers, vec!["a plain free text note".to_string()]);

    assert!(
        pairs
            .iter()
            .all(|p| p.question != "Should the pending question appear?"),
        "a pending question part must yield no pair: {:?}",
        pairs
    );
}

#[test]
fn test_opencode_read_qa_pairs_falls_back_to_output_text_without_metadata() {
    let (_fixture, adapter) = qa_fixture_adapter();
    let pairs = adapter
        .read_qa_pairs("ses_qafixture0a")
        .expect("healthy fixture Q/A read");

    let fallback = pairs
        .iter()
        .find(|p| p.question == "Which sample port should the demo bind?")
        .expect("the metadata-less completed question part must still pair via its output text");
    assert_eq!(fallback.header.as_deref(), Some("Port Question"));
    assert_eq!(fallback.answers, vec!["8080".to_string()]);
}

#[test]
fn test_opencode_read_qa_pairs_unknown_session_is_an_error() {
    let (_fixture, adapter) = qa_fixture_adapter();
    let err = adapter
        .read_qa_pairs("ses_nope")
        .expect_err("an unresolvable session id must not read as an empty session");
    assert!(
        err.contains("ses_nope"),
        "error must name the session: {err}"
    );
}

#[test]
fn test_vibe_read_qa_pairs_pairs_by_tool_call_id_and_skips_cancelled() {
    let adapter = VibeAdapter::with_root(vibe_qa_root());
    let pairs = adapter
        .read_qa_pairs("qa0a0a0a")
        .expect("healthy fixture Q/A read");

    assert_eq!(
        pairs.len(),
        4,
        "three answered calls (one re-asked) plus the text-only fallback: {:?}",
        pairs
    );

    let first = &pairs[0];
    assert_eq!(
        first.question,
        "Which deployment target should the demo use?"
    );
    assert_eq!(first.header.as_deref(), Some("Deployment Target"));
    assert_eq!(first.options.len(), 2);
    assert_eq!(first.options[0].label, "Docker Compose");
    assert_eq!(first.options[0].description, "Single-host compose stack");
    assert_eq!(first.options[1].label, "Bare Metal");
    assert_eq!(first.options[1].description, "Direct systemd deployment");
    assert_eq!(first.answers, vec!["Docker Compose".to_string()]);
    assert!(first.timestamp.is_some(), "the pair carries a timestamp");

    let second = &pairs[1];
    assert_eq!(
        second.question,
        "Which license note should the sample carry?"
    );
    assert_eq!(second.answers, vec!["a plain free text note".to_string()]);

    // The same question text was asked again in a second call and answered
    // differently: pairing must follow tool_call_id, not question text.
    let re_asked = &pairs[2];
    assert_eq!(
        re_asked.question,
        "Which deployment target should the demo use?"
    );
    assert_eq!(re_asked.answers, vec!["Bare Metal".to_string()]);

    assert!(
        pairs
            .iter()
            .all(|p| p.question != "Should the demo include the cancelled question?"),
        "a cancelled question call must yield no pair: {:?}",
        pairs
    );
}

#[test]
fn test_vibe_read_qa_pairs_falls_back_to_content_text_without_tool_result() {
    let adapter = VibeAdapter::with_root(vibe_qa_root());
    let pairs = adapter
        .read_qa_pairs("qa0a0a0a")
        .expect("healthy fixture Q/A read");

    let fallback = pairs
        .iter()
        .find(|p| p.question == "Which sample port should the demo bind?")
        .expect("a tool message without tool_result must still pair via its content text");
    assert_eq!(fallback.header.as_deref(), Some("Port"));
    assert_eq!(fallback.answers, vec!["8080".to_string()]);
}

#[test]
fn test_vibe_read_qa_pairs_unknown_session_is_an_error() {
    let adapter = VibeAdapter::with_root(vibe_qa_root());
    let err = adapter
        .read_qa_pairs("no_such_session")
        .expect_err("an unresolvable session id must not read as an empty session");
    assert!(
        err.contains("no_such_session"),
        "error must name the session: {err}"
    );
}

/// The default (non-overridden) read path: the normalized summary line
/// `question({...})` pairs against the next tool-role message, whose
/// python-ish `answers: [...]` text is parsed. Cancelled calls yield nothing.
#[test]
fn test_default_read_qa_pairs_parses_summary_and_answers_text() {
    let dir = scratch("mock_qa_pairs");
    let path = dir.join("qa.jsonl");
    let lines = vec![
        RolloutMessage {
            role: "user".to_string(),
            content: "please pick the demo settings".to_string(),
            thinking: None,
            tool_calls_summary: Vec::new(),
            timestamp: Some("2026-01-02T00:00:00Z".to_string()),
            injected: false,
        },
        RolloutMessage {
            role: "assistant".to_string(),
            content: String::new(),
            thinking: None,
            tool_calls_summary: vec![
                "question({\"questions\":[{\"header\":\"Mode\",\"question\":\"Which demo mode should run?\",\"options\":[{\"label\":\"Fast\",\"description\":\"Skip checks\"}]}]})".to_string(),
            ],
            timestamp: Some("2026-01-02T00:00:10Z".to_string()),
            injected: false,
        },
        RolloutMessage {
            role: "tool".to_string(),
            content: "answers: [{'question': 'Which demo mode should run?', 'answer': 'Fast', 'is_other': False}]\ncancelled: False".to_string(),
            thinking: None,
            tool_calls_summary: Vec::new(),
            timestamp: Some("2026-01-02T00:00:20Z".to_string()),
            injected: false,
        },
        RolloutMessage {
            role: "assistant".to_string(),
            content: String::new(),
            thinking: None,
            tool_calls_summary: vec![
                "ask_user_question({\"questions\":[{\"header\":\"Colour\",\"question\":\"Which accent colour?\"}]})".to_string(),
            ],
            timestamp: Some("2026-01-02T00:00:30Z".to_string()),
            injected: false,
        },
        RolloutMessage {
            role: "tool".to_string(),
            content: "answers: []\ncancelled: True".to_string(),
            thinking: None,
            tool_calls_summary: Vec::new(),
            timestamp: Some("2026-01-02T00:00:40Z".to_string()),
            injected: false,
        },
    ];
    let mut payload = String::new();
    for m in &lines {
        payload.push_str(&serde_json::to_string(m).unwrap());
        payload.push('\n');
    }
    std::fs::write(&path, payload).unwrap();
    let adapter = MockAdapter::new(&path);

    let pairs = adapter
        .read_qa_pairs("mock")
        .expect("default read path over a mock payload");
    assert_eq!(
        pairs.len(),
        1,
        "the cancelled ask_user_question call yields nothing: {:?}",
        pairs
    );
    let pair = &pairs[0];
    assert_eq!(pair.question, "Which demo mode should run?");
    assert_eq!(pair.header.as_deref(), Some("Mode"));
    assert_eq!(pair.options.len(), 1);
    assert_eq!(pair.options[0].label, "Fast");
    assert_eq!(pair.options[0].description, "Skip checks");
    assert_eq!(pair.answers, vec!["Fast".to_string()]);
    assert_eq!(pair.timestamp.as_deref(), Some("2026-01-02T00:00:10Z"));
}

/// A harness with no question tools answers with an empty list, not an error.
#[test]
fn test_default_read_qa_pairs_returns_empty_for_sessions_without_question_calls() {
    let dir = scratch("mock_no_questions");
    let path = dir.join("plain.jsonl");
    let lines = vec![
        RolloutMessage {
            role: "user".to_string(),
            content: "plain question to the assistant".to_string(),
            thinking: None,
            tool_calls_summary: Vec::new(),
            timestamp: Some("2026-01-02T00:00:00Z".to_string()),
            injected: false,
        },
        RolloutMessage {
            role: "assistant".to_string(),
            content: "a plain answer".to_string(),
            thinking: None,
            tool_calls_summary: vec!["bash: git commit -m x".to_string()],
            timestamp: Some("2026-01-02T00:00:10Z".to_string()),
            injected: false,
        },
    ];
    let mut payload = String::new();
    for m in &lines {
        payload.push_str(&serde_json::to_string(m).unwrap());
        payload.push('\n');
    }
    std::fs::write(&path, payload).unwrap();
    let adapter = MockAdapter::new(&path);

    let pairs = adapter
        .read_qa_pairs("mock")
        .expect("sessions without question calls read as empty, never an error");
    assert!(pairs.is_empty());
}

// --- MCP stdio server: the record-stream contract ---------------------------
//
// The response shape is pinned at the MCP layer over the same opencode
// question fixture the extraction tests use: three answered questions (the
// pending one yields no pair).

struct Mcp {
    child: std::process::Child,
    reader: std::io::BufReader<std::process::ChildStdout>,
    stdin: std::process::ChildStdin,
    cwd: std::path::PathBuf,
    next: u64,
}

impl Mcp {
    fn send(&mut self, v: &serde_json::Value) {
        use std::io::Write;
        writeln!(self.stdin, "{}", serde_json::to_string(v).unwrap()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn read_until_id(&mut self, id: u64) -> serde_json::Value {
        use std::io::BufRead;
        loop {
            let mut line = String::new();
            self.reader
                .read_line(&mut line)
                .expect("server stdout open");
            if line.trim().is_empty() {
                panic!("server closed stdout waiting for response {id}");
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
                && v.get("id").and_then(|i| i.as_u64()) == Some(id)
            {
                return v;
            }
        }
    }

    fn call(&mut self, name: &str, args: serde_json::Value) -> (bool, String) {
        let id = self.next;
        self.next += 1;
        self.send(&serde_json::json!({
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
}

fn reap(mcp: &mut Mcp) {
    mcp.child.kill().ok();
    mcp.child.wait().ok();
    let _ = std::fs::remove_dir_all(&mcp.cwd);
}

fn spawn_qa_mcp() -> (Mcp, ScratchRoot) {
    let dir = scratch("extract_qa_pairs_mcp");
    create_qa_fixture(&dir.join("fixture.db"));
    let cwd = common::scratch::child_cwd("extract_qa_pairs_mcp");
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
                   "clientInfo": {"name": "qa-pairs-test", "version": "0"}}
    }));
    let _ = mcp.read_until_id(0);
    mcp.send(&serde_json::json!({
        "jsonrpc": "2.0", "method": "notifications/initialized"
    }));
    (mcp, dir)
}

fn parsed_qa_stream(text: &str) -> (serde_json::Value, Vec<serde_json::Value>) {
    let value: serde_json::Value = serde_json::from_str(text).unwrap_or_else(|e| {
        panic!("the response must parse as BARE JSON, no prefix ({e}): {text:?}")
    });
    let array = value.as_array().unwrap_or_else(|| {
        panic!("the response must be a bare JSON array — the === prose sections are dead: {text:?}")
    });
    let meta = array.first().expect("record 0 is the meta record").clone();
    assert_eq!(
        meta.get("type").and_then(|t| t.as_str()),
        Some("meta"),
        "record 0 is the meta record: {meta}"
    );
    (meta, array[1..].to_vec())
}

#[test]
fn extract_qa_pairs_streams_the_bare_qa_record_array() {
    let (mut mcp, _dir) = spawn_qa_mcp();

    // Single-session mode: the meta names the resolved session as the scope,
    // every pair is a qa record with the extraction's fields.
    let (is_error, text) = mcp.call(
        "extract_qa_pairs",
        serde_json::json!({"session_id": "ses_qafixture"}),
    );
    assert!(!is_error, "the extraction must not error: {text}");
    assert!(
        !text.contains("=== ") && !text.contains("QUESTION:"),
        "the ===/--- QA/QUESTION: prose shape is gone: {text}"
    );
    let (meta, records) = parsed_qa_stream(&text);
    assert_eq!(meta["tool"].as_str(), Some("extract_qa_pairs"), "{meta}");
    assert_eq!(
        meta["scope"].as_str(),
        Some("ses_qafixture"),
        "the scope is the session id as the caller gave it (the pre-existing resolution contract): {meta}"
    );
    assert_eq!(meta["sessions_scanned"], 1, "{meta}");
    assert_eq!(meta["sessions_with_pairs"], 1, "{meta}");
    assert_eq!(meta["per_session_cap"], 100, "{meta}");
    assert_eq!(records.len(), 3, "three answered pairs: {records:?}");
    for (i, record) in records.iter().enumerate() {
        assert_eq!(record["type"].as_str(), Some("qa"), "{record}");
        assert_eq!(record["session_id"].as_str(), Some("ses_qafixture"));
        assert_eq!(record["qa"], i + 1, "the qa index is 1-based: {record}");
        assert!(record.get("question").is_some(), "{record}");
        assert!(record.get("answers").is_some(), "{record}");
    }
    assert_eq!(
        records[0]["header"].as_str(),
        Some("First Question"),
        "{records:?}"
    );
    assert_eq!(
        records[0]["answers"][0].as_str(),
        Some("Docker Compose"),
        "{records:?}"
    );

    // Window mode: the same record stream, the scope names the window.
    let (is_error, text) = mcp.call("extract_qa_pairs", serde_json::json!({"all": true}));
    assert!(!is_error, "{text}");
    let (meta, records) = parsed_qa_stream(&text);
    assert_eq!(meta["scope"].as_str(), Some("window"), "{meta}");
    assert_eq!(meta["sessions_scanned"], 1, "{meta}");
    assert_eq!(records.len(), 3, "{records:?}");
    reap(&mut mcp);
}

#[test]
fn extract_qa_pairs_over_the_cap_is_prefix_array_marker_and_pure_jsonl_file() {
    let (mut mcp, _dir) = spawn_qa_mcp();
    let (is_error, text) = mcp.call(
        "extract_qa_pairs",
        serde_json::json!({"session_id": "ses_qafixture", "max_bytes": 200}),
    );
    assert!(
        !is_error,
        "an overflowing extraction still succeeds: {text}"
    );
    assert!(
        text.contains("--- [EOF-TRUNCATED] ---"),
        "the marker follows the fitting-prefix array: {text}"
    );
    assert!(
        text.trim_start().starts_with('['),
        "the array prefix always returns — never the marker alone: {text}"
    );
    let head = text
        .split("--- [EOF-TRUNCATED] ---")
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let v: serde_json::Value =
        serde_json::from_str(&head).expect("the array prefix parses as JSON — never torn");
    let array = v.as_array().expect("the prefix is an array");
    assert_eq!(
        array.len(),
        1,
        "a 200-byte budget fits the meta record alone: {array:?}"
    );
    let marker_path_line = text
        .lines()
        .find(|l| l.starts_with("full_report: "))
        .expect("the marker names the overflow file");
    let path = marker_path_line.trim_start_matches("full_report: ").trim();
    let written = std::fs::read_to_string(path).unwrap();
    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(
        lines.len(),
        4,
        "the file carries the meta plus every pair the response could not: {written}"
    );
    for line in &lines {
        serde_json::from_str::<serde_json::Value>(line).unwrap_or_else(|e| {
            panic!("every overflow-file line is one JSON object ({e}): {line}")
        });
    }
    assert!(
        written.contains("Which deployment target"),
        "the file carries the pairs: {written}"
    );
    reap(&mut mcp);
}
