mod common;

use total_recall::RolloutAdapter;
use total_recall::index;
use total_recall::rollout::mock::MockAdapter;

use common::scratch::{ScratchRoot, scratch};

fn write_fixture(name: &str, lines: &[String]) -> (ScratchRoot, MockAdapter) {
    let dir = scratch(&format!("sheep_{name}"));
    let data_path = dir.join("data.jsonl");
    std::fs::write(&data_path, lines.join("\n") + "\n").unwrap();
    let adapter = MockAdapter::new(&data_path);
    (dir, adapter)
}

fn msg(role: &str, content: &str, timestamp: &str, thinking: Option<&str>) -> String {
    serde_json::json!({
        "role": role,
        "content": content,
        "thinking": thinking,
        "tool_calls_summary": [],
        "injected": false,
        "timestamp": timestamp,
    })
    .to_string()
}

fn w(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn fixture_lines() -> Vec<String> {
    vec![
        msg(
            "user",
            "please review the alpha query engine",
            "2026-09-19T10:00:00Z",
            None,
        ),
        msg(
            "assistant",
            "the alpha engine handles queries well",
            "2026-09-19T10:01:00Z",
            None,
        ),
        msg(
            "assistant",
            "",
            "2026-09-19T10:02:00Z",
            Some("contemplating the electric sheep dream"),
        ),
    ]
}

#[test]
fn test_index_session_writes_meta_and_doc_count() {
    let (_dir, adapter) = write_fixture("meta", &fixture_lines());
    let stats = index::index_session(&adapter, "").unwrap();
    assert_eq!(stats.session_id, "mock");
    assert!(stats.doc_count > 0);
    let meta = adapter
        .shadow_index_root()
        .join("mock")
        .join("total-recall-meta.json");
    assert!(meta.is_file(), "total-recall-meta.json must be written");
    assert!(index::index_exists(&adapter, "mock"));
}

#[test]
fn test_reindex_replaces_without_duplicate_growth() {
    let (_dir, adapter) = write_fixture("reindex", &fixture_lines());
    let first = index::index_session(&adapter, "").unwrap();
    let second = index::index_session(&adapter, "").unwrap();
    assert_eq!(
        first.doc_count, second.doc_count,
        "re-index must not grow docs"
    );
}

#[test]
fn test_search_finds_content_term() {
    let (_dir, adapter) = write_fixture("content", &fixture_lines());
    index::index_session(&adapter, "").unwrap();
    let outcome = index::search(&adapter, &[], "alpha", 0, None).unwrap();
    assert_eq!(outcome.hits.len(), 2, "{:?}", outcome.hits);
    let hit = outcome
        .hits
        .iter()
        .find(|h| h.role == "user")
        .expect("the user message is a hit");
    assert_eq!(hit.session_id, "mock");
    assert_eq!(hit.role, "user");
    assert_eq!(hit.timestamp, "2026-09-19T10:00:00Z");
    assert!(
        hit.fragment
            .contains("please review the alpha query engine"),
        "{}",
        hit.fragment
    );
    assert!(!hit.in_thinking);
}

#[test]
fn test_search_finds_thinking_only_term_and_marks_it() {
    let (_dir, adapter) = write_fixture("thinking", &fixture_lines());
    index::index_session(&adapter, "").unwrap();
    let outcome = index::search(&adapter, &[], "contemplating", 0, None).unwrap();
    let hit = outcome
        .hits
        .iter()
        .find(|h| h.in_thinking)
        .expect("the thinking-only hit must carry the thinking flag");
    assert!(
        hit.fragment
            .contains("contemplating the electric sheep dream"),
        "{}",
        hit.fragment
    );
}

#[test]
fn test_search_reports_unindexed_sessions() {
    let (_dir, adapter) = write_fixture("unindexed", &fixture_lines());
    let outcome = index::search(&adapter, &[], "alpha", 0, None).unwrap();
    assert_eq!(
        outcome.not_indexed,
        w(&["mock"]),
        "the unindexed session is named in the outcome, not counted in prose"
    );
    assert!(outcome.hits.is_empty());
}

#[test]
fn test_search_empty_query_is_error() {
    let (_dir, adapter) = write_fixture("empty_query", &fixture_lines());
    assert!(index::search(&adapter, &[], "", 0, None).is_err());
    assert!(index::search(&adapter, &[], "  ", 0, None).is_err());
}

#[test]
fn test_has_tantivy_index_flips_after_indexing() {
    let (_dir, adapter) = write_fixture("flag_flip", &fixture_lines());
    let mut sessions = adapter.list_sessions();
    assert!(!sessions[0].has_tantivy_index);
    let mut profile = adapter
        .profile_session("mock")
        .expect("healthy fixture profile");
    assert!(!profile.has_tantivy_index);

    index::index_session(&adapter, "").unwrap();
    index::annotate_sessions(&mut sessions, &adapter);
    assert!(sessions[0].has_tantivy_index);
    assert!(index::index_exists(&adapter, &sessions[0].session_id));
    profile.has_tantivy_index = index::index_exists(&adapter, &profile.session_id);
    assert!(profile.has_tantivy_index);
}

#[test]
fn test_explicit_partial_id_resolves_and_unmatched_reported() {
    let (_dir, adapter) = write_fixture("partial_id", &fixture_lines());
    index::index_session(&adapter, "mock").unwrap();
    let outcome = index::search(&adapter, &w(&["mock", "nope"]), "alpha", 0, None).unwrap();
    assert_eq!(
        outcome.unmatched,
        w(&["nope"]),
        "the unmatched id is named in the outcome, not counted in prose"
    );
    assert!(
        !outcome.hits.is_empty(),
        "the indexed mock session carries hits: {:?}",
        outcome.hits
    );
}

#[test]
fn test_search_corrupt_index_reported_in_header() {
    let (dir, adapter) = write_fixture("corrupt", &fixture_lines());
    index::index_session(&adapter, "").unwrap();

    let index_dir = dir.join(".tantivy").join("mock");
    let meta = index_dir.join("meta.json");
    std::fs::write(&meta, "{invalid").unwrap();

    let outcome = index::search(&adapter, &[], "alpha", 0, None).unwrap();
    assert!(
        outcome
            .corrupt
            .iter()
            .any(|line| line.contains("cannot open index")),
        "corrupt index must be reported in the outcome:\n{:?}",
        outcome.corrupt
    );
}

#[test]
fn test_search_invalid_query_reported_as_corrupt() {
    let (_dir, adapter) = write_fixture("bad_query", &fixture_lines());
    index::index_session(&adapter, "").unwrap();
    let outcome = index::search(&adapter, &[], "AND (", 0, None).unwrap();
    assert!(
        outcome
            .corrupt
            .iter()
            .any(|line| line.contains("invalid query")),
        "invalid query must surface in the outcome:\n{:?}",
        outcome.corrupt
    );
}

#[test]
fn test_select_sessions_empty_end_time_included_for_any_hours_back() {
    let (_dir, adapter) = write_fixture("hours_back", &fixture_lines());
    let (selected_zero, _) = index::select_sessions(&adapter, &[], 0, None);
    assert!(
        selected_zero.iter().any(|s| s.session_id == "mock"),
        "hours_back=0 must include mock session"
    );
    let (selected_one, _) = index::select_sessions(&adapter, &[], 1, None);
    assert!(
        selected_one.iter().any(|s| s.session_id == "mock"),
        "hours_back=1 must include mock session with empty end_time"
    );
}

#[test]
fn test_search_directory_filter_keeps_session_with_none_directory() {
    let (_dir, adapter) = write_fixture("directory_none", &fixture_lines());
    index::index_session(&adapter, "").unwrap();
    let outcome = index::search(&adapter, &[], "alpha", 0, Some("anything")).unwrap();
    assert!(
        outcome.hits.iter().all(|h| h.session_id == "mock") && !outcome.hits.is_empty(),
        "directory filter must not drop session with directory=None:\n{:?}",
        outcome.hits
    );
}

#[test]
fn test_search_fragment_multibyte_boundary_no_panic() {
    let prefix = "x".to_string() + &"🦀".repeat(50);
    let content = prefix.clone() + " needle term " + &"🦀".repeat(60);
    let lines = vec![msg("user", &content, "2026-09-19T10:00:00Z", None)];
    let (_dir, adapter) = write_fixture("utf8_emoji", &lines);
    index::index_session(&adapter, "").unwrap();
    let outcome = index::search(&adapter, &[], "needle", 0, None).unwrap();
    assert!(
        outcome.hits[0].fragment.contains("needle term"),
        "fragment must contain the matched term:\n{:?}",
        outcome.hits
    );
}

#[test]
fn test_search_fragment_cjk_no_panic() {
    let content = "漢字".repeat(100) + " needle term " + &"漢字".repeat(100);
    let lines = vec![msg("user", &content, "2026-09-19T10:00:00Z", None)];
    let (_dir, adapter) = write_fixture("utf8_cjk", &lines);
    index::index_session(&adapter, "").unwrap();
    let outcome = index::search(&adapter, &[], "needle", 0, None).unwrap();
    assert!(
        outcome.hits[0].fragment.contains("needle term"),
        "fragment must contain the matched term:\n{:?}",
        outcome.hits
    );
}

#[test]
fn test_search_score_ordering_higher_first() {
    let lines = vec![
        msg("user", "zebra zebra", "2026-09-19T10:00:00Z", None),
        msg("user", "only one zebra here", "2026-09-19T10:01:00Z", None),
    ];
    let (_dir, adapter) = write_fixture("score_order", &lines);
    index::index_session(&adapter, "").unwrap();
    let outcome = index::search(&adapter, &[], "zebra", 0, None).unwrap();

    assert_eq!(outcome.hits.len(), 2, "expected two hits");
    assert!(
        outcome.hits[0].score >= outcome.hits[1].score,
        "higher-scoring hit must come first: {} >= {}",
        outcome.hits[0].score,
        outcome.hits[1].score
    );
}

// --- MCP stdio server: the record-stream contract ---------------------------

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

fn spawn_sheep_mcp() -> (Mcp, ScratchRoot) {
    use rusqlite::Connection;
    let dir = scratch("sheep_mcp");
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
         VALUES ('ses_sheep00000000000000000000001aa', NULL, '/Users/dev/sheep', 'sheep fixture', 1000, 2000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data)
         VALUES ('msg1', 'ses_sheep00000000000000000000001aa', 1000, 1000,
                 '{\"role\":\"user\",\"time\":{\"created\":1000}}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
         VALUES ('part1', 'msg1', 'ses_sheep00000000000000000000001aa', 1000, 1000,
                 '{\"type\":\"text\",\"text\":\"release alpha finding\"}')",
        [],
    )
    .unwrap();
    drop(conn);

    let cwd = common::scratch::child_cwd("sheep_mcp");
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
                   "clientInfo": {"name": "sheep-test", "version": "0"}}
    }));
    let _ = mcp.read_until_id(0);
    mcp.send(&serde_json::json!({
        "jsonrpc": "2.0", "method": "notifications/initialized"
    }));
    (mcp, dir)
}

/// The search's new-world record-stream contract, through the real server:
/// index first, then search — the response is a bare compact JSON array whose
/// element 0 is the meta record (query, scope, the searched counts, the
/// not_indexed/unmatched/corrupt lists) and every later element a `type=hit`
/// record with score/ts/role/thinking/fragment. The pipe-table prose is dead.
#[test]
fn sheep_streams_the_bare_hit_record_array() {
    let (mut mcp, _dir) = spawn_sheep_mcp();
    let (err, text) = mcp.call("index_sessions", serde_json::json!({"all": true}));
    assert!(!err, "indexing the fixture succeeds: {text}");

    let (err, text) = mcp.call(
        "do_android_dream_of_electric_sheep",
        serde_json::json!({"query": "alpha", "all": true}),
    );
    assert!(!err, "the search succeeds: {text}");
    assert!(
        !text.contains(" | "),
        "the pipe-table prose is gone: {text}"
    );
    let value: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("the response parses as BARE JSON ({e}): {text:?}"));
    let array = value
        .as_array()
        .expect("the response is a bare record array");
    let meta = &array[0];
    assert_eq!(meta["type"], "meta", "{meta}");
    assert_eq!(meta["tool"], "do_android_dream_of_electric_sheep", "{meta}");
    assert_eq!(meta["query"], "alpha", "{meta}");
    assert_eq!(meta["scope"], "window", "{meta}");
    assert_eq!(meta["sessions_searched"], 1, "{meta}");
    assert_eq!(meta["not_indexed"], serde_json::json!([]), "{meta}");
    assert_eq!(meta["unmatched"], serde_json::json!([]), "{meta}");
    assert_eq!(meta["corrupt"], serde_json::json!([]), "{meta}");
    let hits = &array[1..];
    assert!(!hits.is_empty(), "the fixture carries a hit: {array:?}");
    for hit in hits {
        assert_eq!(hit["type"], "hit", "{hit}");
        assert!(hit.get("score").is_some(), "{hit}");
        assert!(hit.get("ts").is_some(), "{hit}");
        assert!(hit.get("role").is_some(), "{hit}");
        assert!(hit.get("thinking").is_some(), "{hit}");
        assert!(hit.get("fragment").is_some(), "{hit}");
        assert!(
            hit["fragment"].as_str().unwrap_or("").contains("alpha"),
            "{hit}"
        );
    }
    reap(&mut mcp);
}
