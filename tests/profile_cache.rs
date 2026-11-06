mod common;

use std::path::{Path, PathBuf};

use total_recall::RolloutAdapter;
use total_recall::rollout::mock::MockAdapter;

use common::scratch::{ScratchRoot, scratch};

#[allow(dead_code, unused_imports)]
mod cli {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"));
}

use clap::Parser;
use cli::Cli;

fn msg(role: &str, content: &str, timestamp: &str) -> String {
    serde_json::json!({
        "role": role,
        "content": content,
        "thinking": Option::<&str>::None,
        "tool_calls_summary": [],
        "injected": false,
        "timestamp": timestamp,
    })
    .to_string()
}

fn fixture_lines() -> Vec<String> {
    vec![
        msg("user", "cache me if you can", "2026-09-24T10:00:00Z"),
        msg("assistant", "cached", "2026-09-24T10:00:01Z"),
        msg("tool", "result", "2026-09-24T10:00:02Z"),
    ]
}

fn write_fixture(name: &str, lines: &[String]) -> (ScratchRoot, MockAdapter, PathBuf) {
    let dir = scratch(&format!("profile_cache_{name}"));
    let data_path = dir.join("data.jsonl");
    std::fs::write(&data_path, lines.join("\n") + "\n").unwrap();
    let adapter = MockAdapter::new(&data_path);
    let cache_path = adapter.shadow_index_root().join("tr_mock_meta.json");
    (dir, adapter, cache_path)
}

fn set_mtime_epoch_secs(path: &Path, epoch_secs: i64) {
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(
        std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(epoch_secs.max(0) as u64),
    ))
    .unwrap();
}

fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[test]
fn test_first_call_writes_cache_second_call_serves_it() {
    let (_dir, adapter, cache_path) = write_fixture("hit", &fixture_lines());
    let first = adapter
        .profile_session_opts("mock", true)
        .expect("healthy fixture profile");
    assert!(cache_path.is_file(), "cache file must be written");

    // Push the source file's mtime into the past: still inside the 15 s
    // tolerance relative to the younger cache file, so the second call must
    // be served from the cache and return the identical profile.
    set_mtime_epoch_secs(&_dir.join("data.jsonl"), now_epoch_secs() - 3_600);
    let second = adapter
        .profile_session_opts("mock", true)
        .expect("healthy fixture profile");
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&second).unwrap(),
        "cache hit must return the identical profile"
    );
    assert!(cache_path.is_file());
}

#[test]
fn test_stale_cache_recomputes_and_rewrites() {
    let (dir, adapter, cache_path) = write_fixture("stale", &fixture_lines());
    let first = adapter
        .profile_session_opts("mock", true)
        .expect("healthy fixture profile");
    assert!(cache_path.is_file());

    // Change the payload AND push its mtime beyond the staleness tolerance:
    // the cache must be treated as stale, the profile recomputed from the new
    // content, and the cache rewritten.
    let mut grown = fixture_lines();
    grown.push(msg("user", "after the bump", "2026-09-24T10:00:03Z"));
    std::fs::write(dir.join("data.jsonl"), grown.join("\n") + "\n").unwrap();
    set_mtime_epoch_secs(&dir.join("data.jsonl"), now_epoch_secs() + 120);

    let second = adapter
        .profile_session_opts("mock", true)
        .expect("healthy fixture profile");
    assert_eq!(
        second.line_count,
        first.line_count + 1,
        "stale cache must recompute from the changed fixture"
    );
    assert!(
        second.file_size > first.file_size,
        "recomputed profile must reflect the grown fixture"
    );
    assert_eq!(
        serde_json::to_value(&second).unwrap(),
        serde_json::to_value(
            adapter
                .profile_session_opts("mock", false)
                .expect("healthy fixture profile")
        )
        .unwrap(),
        "rewritten cache must match the fresh compute"
    );
}

#[test]
fn test_corrupt_cache_is_repaired() {
    let (_dir, adapter, cache_path) = write_fixture("corrupt", &fixture_lines());
    adapter
        .profile_session_opts("mock", true)
        .expect("healthy fixture profile");
    assert!(cache_path.is_file());
    std::fs::write(&cache_path, b"\x00not json {{{").unwrap();

    let profile = adapter
        .profile_session_opts("mock", true)
        .expect("healthy fixture profile");
    assert_eq!(profile.session_id, "mock");
    assert_eq!(profile.line_count, 3);

    let data = std::fs::read(&cache_path).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&data)
        .expect("corrupt cache must have been rewritten as valid JSON");
    assert!(value["written_at_epoch_ms"].is_u64());
    assert!(value["profile"]["role_counts"].is_object());
}

#[test]
fn test_structurally_wrong_cache_fails_jtd_gate_and_recomputes() {
    let (_dir, adapter, cache_path) = write_fixture("jtd_gate", &fixture_lines());
    adapter
        .profile_session_opts("mock", true)
        .expect("healthy fixture profile");

    let data = std::fs::read(&cache_path).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&data).unwrap();
    value["profile"]["role_counts"] = serde_json::json!([]);
    std::fs::write(&cache_path, serde_json::to_vec(&value).unwrap()).unwrap();

    let profile = adapter
        .profile_session_opts("mock", true)
        .expect("healthy fixture profile");
    assert_eq!(profile.line_count, 3, "wrong-shape cache must recompute");

    let repaired: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cache_path).unwrap()).unwrap();
    assert!(
        repaired["profile"]["role_counts"].is_object(),
        "cache must be repaired with the correct shape"
    );
}

#[test]
fn test_flag_off_writes_no_cache() {
    let (_dir, adapter, cache_path) = write_fixture("flag_off", &fixture_lines());
    let _ = adapter
        .profile_session_opts("mock", false)
        .expect("healthy fixture profile");
    assert!(
        !cache_path.exists(),
        "cache=false must not create a cache file"
    );
    let _ = adapter
        .profile_session("mock")
        .expect("healthy fixture profile");
    assert!(!cache_path.exists(), "trait default must not cache either");
}

#[test]
fn test_cli_no_cache_flag_parses_on_profile() {
    let cli = Cli::try_parse_from(["bin", "profile", "--no-cache"])
        .unwrap_or_else(|e| panic!("clap rejected --no-cache on profile: {e}"));
    assert!(cli.no_cache);
    assert!(matches!(cli.command, cli::Command::Profile));

    let cli =
        Cli::try_parse_from(["bin", "profile"]).expect("profile must parse without --no-cache");
    assert!(
        !cli.no_cache,
        "the cache must default to on: --no-cache is the opt-out"
    );
}

#[test]
fn test_freshness_rule_is_enforced_in_profile_cache_module() {
    use total_recall::profile_cache;

    let dir = scratch("profile_cache_module_freshness");
    let cache_path = dir.join("tr_mock_meta.json");

    // Missing cache file → miss.
    assert!(profile_cache::read_fresh(&cache_path, 0).is_none());

    // Write a cache whose mtime is now; a source time far in the past is
    // fresh, a source time beyond the tolerance is stale.
    let profile = total_recall::SessionProfile {
        session_id: "mock".to_string(),
        file_size: 1,
        line_count: 1,
        first_ts: None,
        last_ts: None,
        role_counts: Default::default(),
        has_tantivy_index: false,
        interesting_events: Vec::new(),
    };
    profile_cache::write(&cache_path, &profile);
    let cache_mtime = profile_cache::mtime_ms(&cache_path).unwrap();
    assert!(profile_cache::read_fresh(&cache_path, cache_mtime).is_some());
    assert!(profile_cache::read_fresh(&cache_path, cache_mtime + 15_000).is_some());
    assert!(profile_cache::read_fresh(&cache_path, cache_mtime + 15_001).is_none());

    // Corrupt bytes → miss. Valid JSON of the wrong variant name → JTD
    // passes (event_type is a plain string) but the unknown variant must fail
    // the conversion into SessionProfile, i.e. treated as corrupt.
    std::fs::write(&cache_path, b"{{{garbage").unwrap();
    assert!(profile_cache::read_fresh(&cache_path, cache_mtime).is_none());

    profile_cache::write(&cache_path, &profile);
    let mut valid: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cache_path).unwrap()).unwrap();
    valid["profile"]["interesting_events"] = serde_json::json!([{
        "line_number": 1,
        "event_type": "NotARealEvent",
        "summary": "s",
        "gap_lines": 1
    }]);
    std::fs::write(&cache_path, serde_json::to_vec(&valid).unwrap()).unwrap();
    assert!(profile_cache::read_fresh(&cache_path, cache_mtime).is_none());
}

// --- MCP stdio server: the record-stream contract ---------------------------
//
// The response shape is pinned at the MCP layer over a fixture SQLite store
// (the same pattern tests/summary_cache.rs pins summary_cache's): the
// cache-recompute path (first call) and the cache-hit path (second call) must
// BOTH return the bare record array.

use rusqlite::Connection;

const SID: &str = "ses_prof0000000000000000000001aa";

fn build_profile_db_fixture(root: &Path) {
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
    conn.execute(
        "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
         VALUES (?1, NULL, '/Users/dev/prof', 'profile fixture', 1000, 4000)",
        [SID],
    )
    .unwrap();
    let messages: Vec<(&str, i64, &str)> = vec![
        ("msg1", 1000, r#"{"role":"user","time":{"created":1000}}"#),
        (
            "msg2",
            2000,
            r#"{"role":"assistant","time":{"created":2000}}"#,
        ),
        ("msg3", 3000, r#"{"role":"user","time":{"created":3000}}"#),
        (
            "msg4",
            4000,
            r#"{"role":"assistant","time":{"created":4000}}"#,
        ),
    ];
    for (id, t, data) in &messages {
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?3, ?4)",
            rusqlite::params![id, SID, t, data],
        )
        .unwrap();
    }
    let parts: Vec<(&str, &str, i64, &str)> = vec![
        (
            "part1",
            "msg1",
            1000,
            r#"{"type":"text","text":"start the work"}"#,
        ),
        ("part2", "msg2", 2000, r#"{"type":"text","text":"working"}"#),
        (
            "part3",
            "msg3",
            3000,
            r#"{"type":"compaction","auto":false}"#,
        ),
        (
            "part4",
            "msg3",
            3100,
            r#"{"type":"text","text":"more work"}"#,
        ),
        ("part5", "msg4", 4000, r#"{"type":"text","text":"done"}"#),
    ];
    for (id, mid, t, data) in &parts {
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            rusqlite::params![id, mid, SID, t, data],
        )
        .unwrap();
    }
}

fn spawn_profile_mcp() -> (Mcp, common::scratch::ScratchRoot) {
    let dir = scratch("profile_cache_mcp");
    build_profile_db_fixture(&dir);
    let cwd = common::scratch::child_cwd("profile_cache_mcp");
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
                   "clientInfo": {"name": "profile-cache-test", "version": "0"}}
    }));
    let _ = mcp.read_until_id(0);
    mcp.send(&serde_json::json!({
        "jsonrpc": "2.0", "method": "notifications/initialized"
    }));
    (mcp, dir)
}

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

fn reap_profile_mcp(mcp: &mut Mcp) {
    mcp.child.kill().ok();
    mcp.child.wait().ok();
    let _ = std::fs::remove_dir_all(&mcp.cwd);
}

fn parsed_profile_stream(text: &str) -> (serde_json::Value, Vec<serde_json::Value>) {
    let value: serde_json::Value = serde_json::from_str(text).unwrap_or_else(|e| {
        panic!("the response must parse as BARE JSON, no prefix ({e}): {text:?}")
    });
    let array = value.as_array().unwrap_or_else(|| {
        panic!(
            "the response must be a bare JSON array — the old pretty-object shape is dead: {text:?}"
        )
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
fn profile_session_streams_the_bare_array_on_recompute_and_cache_hit() {
    let (mut mcp, _dir) = spawn_profile_mcp();

    // First call: the cache-recompute path.
    let (is_error, text) = mcp.call(
        "profile_session",
        serde_json::json!({"session_id": "ses_prof"}),
    );
    assert!(!is_error, "profiling must not error: {text}");
    let (meta, records) = parsed_profile_stream(&text);
    assert_eq!(meta["tool"].as_str(), Some("profile_session"), "{meta}");
    assert_eq!(meta["session_id"].as_str(), Some(SID), "{meta}");
    assert!(meta["file_size"].is_u64(), "{meta}");
    assert!(meta["line_count"].is_u64(), "{meta}");
    assert!(meta["role_counts"].is_object(), "{meta}");
    assert!(meta.get("has_tantivy_index").is_some(), "{meta}");
    assert!(
        !records.is_empty(),
        "the fixture carries user messages and a compaction: {records:?}"
    );
    for record in &records {
        assert_eq!(record["type"].as_str(), Some("event"), "{record}");
        assert!(record.get("line_number").is_some(), "{record}");
        assert!(record.get("event_type").is_some(), "{record}");
        assert!(record.get("summary").is_some(), "{record}");
    }
    assert!(
        records
            .iter()
            .any(|r| r["event_type"].as_str() == Some("UserMessage")),
        "user messages are event records: {records:?}"
    );
    reap_profile_mcp(&mut mcp);

    // Second call, fresh server: the cache-hit path (the first call wrote
    // the cache file the second server reads) — the same array shape.
    let (mut mcp, _dir) = spawn_profile_mcp();
    let (is_error, text) = mcp.call(
        "profile_session",
        serde_json::json!({"session_id": "ses_prof"}),
    );
    assert!(!is_error, "{text}");
    let (meta, records) = parsed_profile_stream(&text);
    assert_eq!(meta["tool"].as_str(), Some("profile_session"), "{meta}");
    assert!(
        records.iter().all(|r| r["type"].as_str() == Some("event")),
        "every data record is an event: {records:?}"
    );
    reap_profile_mcp(&mut mcp);
}
