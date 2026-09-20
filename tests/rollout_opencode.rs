use rusqlite::Connection;
use std::path::PathBuf;
use total_recall::RolloutAdapter;
use total_recall::rollout::opencode::OpenCodeAdapter;

fn tmp_db(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(format!("opencode_{}.db", name));
    let _ = std::fs::remove_file(&path);
    path
}

fn create_fixture(path: &PathBuf) {
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
         VALUES (?1, ?2, ?3, ?4, 1000, 4000)",
        rusqlite::params![
            "ses_fixture0001aaaa",
            "ses_parent0000bbbb",
            "/Users/dev/fixture",
            "fixture session"
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
        ("msg3", 3000, r#"{"role":"user","time":{"created":3000}}"#),
        ("msg4", 4000, r#"{"role":"user","time":{"created":4000}}"#),
    ];
    for (id, t, data) in messages {
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?3, ?4)",
            rusqlite::params![id, "ses_fixture0001aaaa", t, data],
        )
        .unwrap();
    }

    let parts: Vec<(&str, &str, i64, &str)> = vec![
        (
            "part1",
            "msg1",
            1000,
            r#"{"type":"text","text":"hello fixture world"}"#,
        ),
        (
            "part2",
            "msg2",
            1500,
            r#"{"type":"tool","tool":"bash","callID":"c1","state":{"status":"completed","input":{"command":"git commit -m x"},"output":"ok"}}"#,
        ),
        (
            "part3",
            "msg2",
            1600,
            r#"{"type":"text","text":"did the thing"}"#,
        ),
        (
            "part4",
            "msg3",
            3000,
            r#"{"type":"compaction","auto":false}"#,
        ),
        (
            "part5",
            "msg4",
            4000,
            r#"{"type":"text","text":"after compaction"}"#,
        ),
    ];
    for (id, mid, t, data) in parts {
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            rusqlite::params![id, mid, "ses_fixture0001aaaa", t, data],
        )
        .unwrap();
    }
}

fn fixture_adapter(name: &str) -> OpenCodeAdapter {
    let path = tmp_db(name);
    create_fixture(&path);
    OpenCodeAdapter::with_root(&path)
}

#[test]
fn test_opencode_adapter_name() {
    let adapter = fixture_adapter("name");
    assert_eq!(adapter.name(), "opencode");
}

#[test]
fn test_opencode_read_session() {
    let adapter = fixture_adapter("read");
    let messages = adapter.read_session("ses_fixture");

    assert!(!messages.is_empty(), "Should read messages from fixture");
    let user = messages
        .iter()
        .find(|m| m.role == "user" && m.content == "hello fixture world")
        .expect("user text part should be present");
    assert_eq!(user.timestamp.as_deref(), Some("1970-01-01T00:00:01Z"));
    assert!(!user.injected);

    let tool_msg = messages
        .iter()
        .find(|m| m.role == "tool")
        .expect("tool output should be a tool message");
    assert_eq!(tool_msg.content, "ok");

    let assistant = messages
        .iter()
        .find(|m| m.role == "assistant" && !m.tool_calls_summary.is_empty())
        .expect("assistant tool call should be summarised");
    assert!(
        assistant.tool_calls_summary[0].contains("git commit"),
        "tool summary should mention the command: {}",
        assistant.tool_calls_summary[0]
    );
}

#[test]
fn test_opencode_read_session_mmap() {
    let adapter = fixture_adapter("mmap");
    let mmap = adapter.read_session_mmap("ses_fixture");
    let plain = adapter.read_session("ses_fixture");
    assert!(!mmap.is_empty());
    assert_eq!(mmap.len(), plain.len());
    for (a, b) in mmap.iter().zip(plain.iter()) {
        assert_eq!(a.role, b.role);
        assert_eq!(a.content, b.content);
    }
}

#[test]
fn test_opencode_list_sessions() {
    let adapter = fixture_adapter("list");
    let sessions = adapter.list_sessions();

    assert!(!sessions.is_empty(), "Should find at least one session");
    let s = sessions
        .iter()
        .find(|s| s.session_id.contains("fixture"))
        .expect("Should find fixture session by partial id");
    assert_eq!(s.title, "fixture session");
    assert_eq!(
        s.directory.as_deref(),
        Some("/Users/dev/fixture"),
        "directory must be populated from session.directory"
    );
    assert_eq!(s.parent_session_id.as_deref(), Some("ses_parent0000bbbb"));
    assert!(s.user_count >= 2, "user messages counted");
    assert!(s.assistant_count >= 1, "assistant messages counted");
    assert_eq!(s.tool_count, 1, "tool parts counted");
    assert!(s.has_compaction, "compaction part detected");
    assert!(s.file_size > 0);
    assert!(s.line_count > 0);
}

#[test]
fn test_opencode_profile_session() {
    let adapter = fixture_adapter("profile");
    let profile = adapter.profile_session("ses_fixture");

    assert!(profile.line_count > 0);
    assert!(profile.file_size > 0);
    assert!(profile.role_counts.contains_key("user"));
    assert!(profile.role_counts.contains_key("assistant"));
    assert_eq!(profile.first_ts.as_deref(), Some("1970-01-01T00:00:01Z"));
    assert_eq!(profile.last_ts.as_deref(), Some("1970-01-01T00:00:04Z"));
    let compaction = profile
        .interesting_events
        .iter()
        .find(|e| e.summary.contains("Compaction"))
        .expect("compaction should be an interesting event");
    assert!(compaction.line_number > 0);
}

#[test]
fn test_opencode_extract_user_messages() {
    let adapter = fixture_adapter("extract");
    let messages = adapter.extract_user_messages("ses_fixture");

    assert!(
        messages.iter().any(|m| m == "hello fixture world"),
        "real user messages extracted: {:?}",
        messages
    );
    assert!(
        messages.iter().any(|m| m == "after compaction"),
        "post-compaction user message extracted: {:?}",
        messages
    );
    assert!(
        !messages.iter().any(|m| m.contains("context compaction")),
        "compaction marker must not appear as a user message: {:?}",
        messages
    );
}

#[test]
fn test_opencode_read_session_from_compaction() {
    let adapter = fixture_adapter("slice");
    let messages = adapter.read_session_from_compaction("ses_fixture");

    assert!(
        !messages.iter().any(|m| m.content == "hello fixture world"),
        "pre-compaction user message must be sliced off: {:?}",
        messages
    );
    assert!(
        messages
            .iter()
            .any(|m| m.role == "user" && m.content.contains("context compaction")),
        "compaction marker itself is the slice start: {:?}",
        messages
    );
    assert!(
        messages.iter().any(|m| m.content == "after compaction"),
        "post-compaction content kept: {:?}",
        messages
    );
}

#[test]
fn test_opencode_unknown_session_returns_empty() {
    let adapter = fixture_adapter("unknown");
    assert!(adapter.read_session("ses_nope").is_empty());
}

#[test]
fn test_opencode_reasoning_part_becomes_thinking_message() {
    let path = tmp_db("reasoning");
    create_fixture(&path);
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data)
         VALUES ('msg5', 'ses_fixture0001aaaa', 5000, 5000, ?1)",
        rusqlite::params![r#"{"role":"assistant","time":{"created":5000}}"#],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
         VALUES ('part6', 'msg5', 'ses_fixture0001aaaa', 5000, 5000, ?1)",
        rusqlite::params![r#"{"type":"reasoning","text":"pondering the query plan"}"#],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
         VALUES ('part7', 'msg5', 'ses_fixture0001aaaa', 5100, 5100, ?1)",
        rusqlite::params![r#"{"type":"reasoning","synthetic":true,"text":"synthetic reasoning"}"#],
    )
    .unwrap();

    let adapter = OpenCodeAdapter::with_root(&path);
    let messages = adapter.read_session("ses_fixture");
    let reasoning = messages
        .iter()
        .find(|m| m.thinking.as_deref().is_some_and(|t| t.contains("pondering")))
        .expect("reasoning part must become a thinking-carrying message");
    assert_eq!(reasoning.role, "assistant");
    assert_eq!(reasoning.content, "");
    assert!(
        !messages
            .iter()
            .any(|m| m.thinking.as_deref().is_some_and(|t| t.contains("synthetic"))),
        "synthetic reasoning must be skipped: {:?}",
        messages
    );
}

#[test]
fn test_opencode_reasoning_e2e_indexed_and_searchable_as_thinking() {
    use total_recall::index;

    let path = tmp_db("reasoning_search");
    create_fixture(&path);
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data)
         VALUES ('msg5', 'ses_fixture0001aaaa', 5000, 5000, ?1)",
        rusqlite::params![r#"{"role":"assistant","time":{"created":5000}}"#],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
         VALUES ('part6', 'msg5', 'ses_fixture0001aaaa', 5000, 5000, ?1)",
        rusqlite::params![r#"{"type":"reasoning","text":"the electriczebra appears only in reasoning"}"#],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
         VALUES ('part7', 'msg5', 'ses_fixture0001aaaa', 5100, 5100, ?1)",
        rusqlite::params![r#"{"type":"text","text":"normal assistant text without the term"}"#],
    )
    .unwrap();

    let adapter = OpenCodeAdapter::with_root(&path);
    let stats = index::index_session(&adapter, "ses_fixture").unwrap();
    assert_eq!(stats.session_id, "ses_fixture0001aaaa");
    assert!(stats.doc_count > 0);

    let report = index::search(&adapter, &[], "electriczebra", 0, None).unwrap();
    assert!(
        report.contains("electriczebra"),
        "search must find the reasoning term:\n{}",
        report
    );
    assert!(
        report.contains("ASSISTANT (thinking)"),
        "reasoning hit must be marked as thinking:\n{}",
        report
    );
}
