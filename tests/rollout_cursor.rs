use inception_mercury_compaction::rollout::cursor::CursorAdapter;
use inception_mercury_compaction::RolloutAdapter;
use rusqlite::Connection;
use std::path::PathBuf;

const CID: &str = "11112222-aaaa-bbbb-cccc-ddddeeeeffff";

fn tmp_db(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(format!("cursor_{}.db", name));
    let _ = std::fs::remove_file(&path);
    path
}

fn create_fixture(path: &PathBuf) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE cursorDiskKV (
            key TEXT UNIQUE ON CONFLICT REPLACE,
            value BLOB
        );",
    )
    .unwrap();

    let headers = format!(
        r#"[{{"bubbleId":"b1","type":1}},{{"bubbleId":"b2","type":2}},{{"bubbleId":"b3","type":2}},{{"bubbleId":"b4","type":1}},{{"bubbleId":"b5","type":2}},{{"bubbleId":"b6","type":1}}]"#
    );
    conn.execute(
        "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
        rusqlite::params![
            format!("composerData:{}", CID),
            format!(
                r#"{{"composerId":"{}","createdAt":1789725600000,"name":"fixture session","isArchived":0,"subComposerIds":["22223333-aaaa-bbbb-cccc-ddddeeeeffff"],"fullConversationHeadersOnly":{}}}"#,
                CID, headers
            )
        ],
    )
    .unwrap();

    let bubbles: Vec<(&str, i64, &str, &str)> = vec![
        ("b1", 1, "2026-09-18T10:00:01.000Z", r#"{"type":1,"text":"hello cursor fixture","createdAt":"2026-09-18T10:00:01.000Z","toolResults":"[]"}"#),
        ("b2", 2, "2026-09-18T10:00:03.000Z", r#"{"type":2,"text":"doing the thing","createdAt":"2026-09-18T10:00:03.000Z","toolFormerData":{"name":"read_file_v2","rawArgs":"{\"path\":\"/w/Makefile\",\"limit\":10}","result":"{\"contents\":\"ok\"}"},"toolResults":"[]"}"#),
        ("b3", 2, "2026-09-18T10:00:04.000Z", r#"{"type":2,"text":"","createdAt":"2026-09-18T10:00:04.000Z","toolResults":"[]"}"#),
        ("b4", 1, "2026-09-18T10:01:00.000Z", r#"{"type":1,"text":"continue after summary","createdAt":"2026-09-18T10:01:00.000Z","summarizedComposers":["sum-1"],"toolResults":"[]"}"#),
        ("b5", 2, "2026-09-18T10:01:02.000Z", r#"{"type":2,"text":"post summary work","createdAt":"2026-09-18T10:01:02.000Z","toolResults":"[]"}"#),
        ("b6", 1, "2026-09-18T10:02:00.000Z", r#"{"type":1,"text":"final user turn","createdAt":"2026-09-18T10:02:00.000Z","toolResults":"[]"}"#),
    ];
    for (bid, _t, _iso, value) in bubbles {
        conn.execute(
            "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
            rusqlite::params![format!("bubbleId:{}:{}", CID, bid), value],
        )
        .unwrap();
    }
}

fn fixture_adapter(name: &str) -> CursorAdapter {
    let path = tmp_db(name);
    create_fixture(&path);
    CursorAdapter::with_root(&path)
}

#[test]
fn test_cursor_adapter_name() {
    let adapter = fixture_adapter("name");
    assert_eq!(adapter.name(), "cursor");
}

#[test]
fn test_cursor_read_session() {
    let adapter = fixture_adapter("read");
    let messages = adapter.read_session("11112222");

    assert!(!messages.is_empty(), "Should read messages from fixture");
    let user = messages
        .iter()
        .find(|m| m.role == "user" && m.content == "hello cursor fixture")
        .expect("user text should be present");
    assert_eq!(user.timestamp.as_deref(), Some("2026-09-18T10:00:01.000Z"));
    assert!(!user.injected);

    let tool_msg = messages
        .iter()
        .find(|m| m.role == "tool")
        .expect("tool result should be a tool message");
    assert_eq!(tool_msg.content, r#"{"contents":"ok"}"#);

    let assistant = messages
        .iter()
        .find(|m| m.role == "assistant" && !m.tool_calls_summary.is_empty())
        .expect("assistant toolFormerData should be summarised");
    assert!(
        assistant.tool_calls_summary[0].contains("/w/Makefile"),
        "tool summary should mention the path: {}",
        assistant.tool_calls_summary[0]
    );

    let text_assistant = messages
        .iter()
        .find(|m| m.role == "assistant" && m.content == "post summary work")
        .expect("assistant text bubble b5 should be present");
    assert!(
        text_assistant.tool_calls_summary.is_empty(),
        "text-only assistant bubble must have no tool summary"
    );
}

#[test]
fn test_cursor_read_session_mmap() {
    let adapter = fixture_adapter("mmap");
    let mmap = adapter.read_session_mmap("11112222");
    let plain = adapter.read_session("11112222");
    assert!(!mmap.is_empty());
    assert_eq!(mmap.len(), plain.len());
    for (a, b) in mmap.iter().zip(plain.iter()) {
        assert_eq!(a.role, b.role);
        assert_eq!(a.content, b.content);
    }
}

#[test]
fn test_cursor_list_sessions() {
    let adapter = fixture_adapter("list");
    let sessions = adapter.list_sessions();

    assert!(!sessions.is_empty(), "Should find at least one session");
    let s = sessions
        .iter()
        .find(|s| s.session_id.contains("11112222"))
        .expect("Should find fixture session by partial id");
    assert_eq!(s.title, "fixture session");
    assert_eq!(s.start_time, "2026-09-18T10:00:00Z");
    assert!(s.user_count >= 3, "user bubbles counted: {}", s.user_count);
    assert!(s.assistant_count >= 2, "assistant bubbles counted");
    assert_eq!(s.tool_count, 1, "toolFormerData bubbles counted");
    assert!(s.has_compaction, "summarizedComposers detected");
    assert!(s.file_size > 0);
    assert!(s.line_count >= 6, "bubble count as line count");
    assert!(
        s.child_sessions
            .iter()
            .any(|c| c.contains("22223333")),
        "subComposerIds mapped to child sessions: {:?}",
        s.child_sessions
    );
}

#[test]
fn test_cursor_profile_session() {
    let adapter = fixture_adapter("profile");
    let profile = adapter.profile_session("11112222");

    assert!(profile.line_count >= 6);
    assert!(profile.file_size > 0);
    assert!(profile.role_counts.contains_key("user"));
    assert!(profile.role_counts.contains_key("assistant"));
    assert_eq!(
        profile.first_ts.as_deref(),
        Some("2026-09-18T10:00:01.000Z")
    );
    assert_eq!(profile.last_ts.as_deref(), Some("2026-09-18T10:02:00.000Z"));
    let compaction = profile
        .interesting_events
        .iter()
        .find(|e| e.summary.contains("Compaction"))
        .expect("summarizedComposers bubble should be an interesting event");
    assert!(compaction.line_number > 0);
}

#[test]
fn test_cursor_extract_user_messages() {
    let adapter = fixture_adapter("extract");
    let messages = adapter.extract_user_messages("11112222");

    assert!(
        messages.iter().any(|m| m == "hello cursor fixture"),
        "real user messages extracted: {:?}",
        messages
    );
    assert!(
        messages.iter().any(|m| m == "final user turn"),
        "last user message extracted: {:?}",
        messages
    );
    assert!(
        !messages.iter().any(|m| m.contains("context compaction")),
        "compaction marker must not appear as a user message: {:?}",
        messages
    );
}

#[test]
fn test_cursor_read_session_from_compaction() {
    let adapter = fixture_adapter("slice");
    let messages = adapter.read_session_from_compaction("11112222");

    assert!(
        !messages.iter().any(|m| m.content == "hello cursor fixture"),
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
        messages.iter().any(|m| m.content == "post summary work"),
        "post-compaction content kept: {:?}",
        messages
    );
}

#[test]
fn test_cursor_unknown_session_returns_empty() {
    let adapter = fixture_adapter("unknown");
    assert!(adapter.read_session("99990000").is_empty());
}

#[test]
fn test_cursor_harness_wiring() {
    let adapter = inception_mercury_compaction::harness::make_adapter("cursor")
        .expect("cursor must be a valid harness");
    assert_eq!(adapter.name(), "cursor");
}
