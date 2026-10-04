mod common;

use rusqlite::Connection;
use std::path::Path;
use total_recall::LISTING_ROW_CAP;
use total_recall::RolloutAdapter;
use total_recall::rollout::opencode::OpenCodeAdapter;

use common::scratch::{ScratchRoot, scratch};

fn create_fixture(path: &Path) {
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

fn fixture_adapter(name: &str) -> (ScratchRoot, OpenCodeAdapter) {
    let dir = scratch(&format!("opencode_{name}"));
    let path = dir.join("fixture.db");
    create_fixture(&path);
    let adapter = OpenCodeAdapter::with_root(&path);
    (dir, adapter)
}

#[test]
fn test_opencode_adapter_name() {
    let (_fixture, adapter) = fixture_adapter("name");
    assert_eq!(adapter.name(), "opencode");
}

#[test]
fn test_opencode_read_session() {
    let (_fixture, adapter) = fixture_adapter("read");
    let messages = adapter
        .read_session("ses_fixture")
        .expect("healthy fixture read");

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
    let (_fixture, adapter) = fixture_adapter("mmap");
    let mmap = adapter
        .read_session_mmap("ses_fixture")
        .expect("healthy fixture mmap read");
    let plain = adapter
        .read_session("ses_fixture")
        .expect("healthy fixture read");
    assert!(!mmap.is_empty());
    assert_eq!(mmap.len(), plain.len());
    for (a, b) in mmap.iter().zip(plain.iter()) {
        assert_eq!(a.role, b.role);
        assert_eq!(a.content, b.content);
    }
}

#[test]
fn test_opencode_list_sessions() {
    let (_fixture, adapter) = fixture_adapter("list");
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
    let (_fixture, adapter) = fixture_adapter("profile");
    let profile = adapter
        .profile_session("ses_fixture")
        .expect("healthy fixture profile");

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
    let (_fixture, adapter) = fixture_adapter("extract");
    let messages = adapter
        .extract_user_messages("ses_fixture")
        .expect("healthy fixture user messages");

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
    let (_fixture, adapter) = fixture_adapter("slice");
    let messages = adapter
        .read_session_from_compaction("ses_fixture")
        .expect("healthy fixture compaction read");

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
fn test_opencode_unknown_session_is_an_error_not_an_empty_session() {
    let (_fixture, adapter) = fixture_adapter("unknown");
    let err = adapter
        .read_session("ses_nope")
        .expect_err("an unresolvable session id must not read as an empty session");
    assert!(
        err.contains("ses_nope"),
        "error must name the session: {err}"
    );
}

#[test]
fn test_opencode_reasoning_part_becomes_thinking_message() {
    let dir = scratch("opencode_reasoning");
    let path = dir.join("fixture.db");
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
    let messages = adapter
        .read_session("ses_fixture")
        .expect("healthy fixture read");
    let reasoning = messages
        .iter()
        .find(|m| {
            m.thinking
                .as_deref()
                .is_some_and(|t| t.contains("pondering"))
        })
        .expect("reasoning part must become a thinking-carrying message");
    assert_eq!(reasoning.role, "assistant");
    assert_eq!(reasoning.content, "");
    assert!(
        !messages.iter().any(|m| m
            .thinking
            .as_deref()
            .is_some_and(|t| t.contains("synthetic"))),
        "synthetic reasoning must be skipped: {:?}",
        messages
    );
}

#[test]
fn test_opencode_reasoning_e2e_indexed_and_searchable_as_thinking() {
    use total_recall::index;

    let dir = scratch("opencode_reasoning_search");
    let path = dir.join("fixture.db");
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
        rusqlite::params![
            r#"{"type":"reasoning","text":"the electriczebra appears only in reasoning"}"#
        ],
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

/// An opencode fixture with explicit session times and directories: one
/// message and two parts per session so the aggregates are non-zero and
/// per-session distinguishable.
fn create_timed_fixture(path: &Path, sessions: &[(&str, &str, i64)]) {
    let conn = Connection::open(path).unwrap();
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
    for (sid, directory, updated) in sessions {
        conn.execute(
            "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
             VALUES (?1, NULL, ?2, ?3, ?4, ?4)",
            rusqlite::params![*sid, *directory, format!("timed session {sid}"), *updated],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?3, ?4)",
            rusqlite::params![
                format!("msg-{sid}"),
                *sid,
                *updated,
                format!("{{\"role\":\"user\",\"time\":{{\"created\":{updated}}}}}")
            ],
        )
        .unwrap();
        for (p, text) in [
            (0, format!("timed body of {sid} one")),
            (1, format!("timed body of {sid} two")),
        ] {
            conn.execute(
                "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
                rusqlite::params![
                    format!("part-{sid}-{p}"),
                    format!("msg-{sid}"),
                    *sid,
                    *updated,
                    serde_json::json!({"type":"text","text":text}).to_string()
                ],
            )
            .unwrap();
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[test]
fn test_opencode_list_sessions_scoped_window_directory_ordering_and_counts() {
    let now = now_ms();
    let dir = scratch("opencode_scoped_small");
    let path = dir.join("fixture.db");
    create_timed_fixture(
        &path,
        &[
            (
                "ses_new_alpha00000000000000000000aa",
                "/Users/dev/alpha",
                now - 1_000,
            ),
            (
                "ses_mid_beta000000000000000000000bb",
                "/Users/dev/beta",
                now - 2_000,
            ),
            (
                "ses_old_gamma00000000000000000000cc",
                "/Users/dev/gamma",
                1_700_000_000_000,
            ),
        ],
    );
    let adapter = OpenCodeAdapter::with_root(&path);

    // The 24-hour window holds the alpha and beta sessions; the epoch-old
    // gamma session is invisible to the scoped listing — not merely unprinted.
    let listing = adapter.list_sessions_scoped(24, None);
    assert_eq!(listing.window_count, 2);
    let ids: Vec<&str> = listing
        .sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            "ses_new_alpha00000000000000000000aa",
            "ses_mid_beta000000000000000000000bb",
        ],
        "most recent first, out-of-window sessions never returned"
    );

    // Aggregates are computed for the returned rows: one user message and
    // two parts per session.
    for s in &listing.sessions {
        assert_eq!(s.user_count, 1, "{}", s.session_id);
        assert_eq!(s.assistant_count, 0, "{}", s.session_id);
        assert_eq!(s.tool_count, 0, "{}", s.session_id);
        assert_eq!(s.line_count, 2, "{}", s.session_id);
        assert!(s.file_size > 0, "{}", s.session_id);
    }

    // The directory substring is pushed into the store query too.
    let listing = adapter.list_sessions_scoped(24, Some("beta"));
    assert_eq!(listing.window_count, 1);
    assert_eq!(listing.sessions.len(), 1);
    assert_eq!(
        listing.sessions[0].session_id,
        "ses_mid_beta000000000000000000000bb"
    );
    assert_eq!(
        listing.sessions[0].directory.as_deref(),
        Some("/Users/dev/beta")
    );

    // hours_back 0 is no bound at the mechanism layer: everything listed.
    let listing = adapter.list_sessions_scoped(0, None);
    assert_eq!(listing.window_count, 3);
    assert_eq!(listing.sessions.len(), 3);
}

#[test]
fn test_opencode_list_sessions_scoped_caps_at_200_and_reports_the_window_count() {
    let now = now_ms();
    let dir = scratch("opencode_scoped_cap");
    let path = dir.join("fixture.db");
    let sessions: Vec<(String, &str, i64)> = (0..LISTING_ROW_CAP + 50)
        .map(|i| {
            (
                format!("ses_cap{i:03}0000000000000000000000aa"),
                "/Users/dev/cap",
                now - (i as i64) * 1_000,
            )
        })
        .collect();
    let refs: Vec<(&str, &str, i64)> = sessions
        .iter()
        .map(|(sid, d, t)| (sid.as_str(), *d, *t))
        .collect();
    create_timed_fixture(&path, &refs);
    let adapter = OpenCodeAdapter::with_root(&path);

    let listing = adapter.list_sessions_scoped(24, None);
    assert_eq!(
        listing.window_count,
        LISTING_ROW_CAP + 50,
        "the window count is the whole window, not just the returned rows"
    );
    assert_eq!(
        listing.sessions.len(),
        LISTING_ROW_CAP,
        "at most {LISTING_ROW_CAP} rows are returned, most recent first"
    );
    assert_eq!(
        listing.sessions[0].session_id, "ses_cap0000000000000000000000000aa",
        "the first row is the most recent session of the window"
    );
    assert_eq!(
        listing.sessions[LISTING_ROW_CAP - 1].session_id,
        format!("ses_cap{:03}0000000000000000000000aa", LISTING_ROW_CAP - 1),
        "the 200th row is the 200th most recent session"
    );
    let ranks: Vec<i32> = listing
        .sessions
        .iter()
        .map(|s| s.session_id[7..10].parse::<i32>().unwrap())
        .collect();
    assert!(
        ranks
            .iter()
            .all(|r| (0..LISTING_ROW_CAP as i32).contains(r)),
        "the 50 oldest rows of the window are held back, not returned: first {} last {}",
        ranks.first().unwrap(),
        ranks.last().unwrap()
    );
}

/// The #21 regression: a store of 3,000 sessions outside the window, each
/// carrying payload aggregates, and ~50 inside. The scoped listing returns
/// the 50 with their aggregates and reports window_count 50 — the 3,000 are
/// invisible, not just unprinted.
#[test]
fn test_opencode_scoped_listing_ignores_three_thousand_out_of_window_sessions() {
    const OUT_OF_WINDOW: usize = 3_000;
    const IN_WINDOW: usize = 50;
    let dir = scratch("opencode_scoped_bulk");
    let path = dir.join("fixture.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=OFF;
         PRAGMA synchronous=OFF;
         CREATE TABLE session (
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
    let tx = conn.unchecked_transaction().unwrap();
    {
        let mut ins_session = tx
            .prepare(
                "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
                      VALUES (?1, NULL, ?2, ?3, ?4, ?4)",
            )
            .unwrap();
        let mut ins_message = tx
            .prepare(
                "INSERT INTO message (id, session_id, time_created, time_updated, data)
                      VALUES (?1, ?2, ?3, ?3, ?4)",
            )
            .unwrap();
        let mut ins_part = tx
            .prepare(
                "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
                      VALUES (?1, ?2, ?2, ?3, ?3, ?4)",
            )
            .unwrap();
        for i in 0..OUT_OF_WINDOW + IN_WINDOW {
            let in_window = i >= OUT_OF_WINDOW;
            let sid = format!("ses_bulk{i:04}00000000000000000000aa");
            let updated = if in_window {
                now - 1_000
            } else {
                1_700_000_000_000
            };
            ins_session
                .execute(rusqlite::params![
                    &sid,
                    "/Users/dev/bulk",
                    &format!("bulk session {i}"),
                    updated
                ])
                .unwrap();
            ins_message
                .execute(rusqlite::params![
                    format!("bmsg{i:04}"),
                    &sid,
                    updated,
                    "{\"role\":\"user\",\"time\":{\"created\":1}}"
                ])
                .unwrap();
            ins_part
                .execute(rusqlite::params![
                    format!("bpart{i:04}"),
                    &sid,
                    updated,
                    serde_json::json!({"type":"text","text":format!("bulk body {i} {}", "x".repeat(512))})
                        .to_string()
                ])
                .unwrap();
        }
    }
    tx.commit().unwrap();
    drop(conn);

    let adapter = OpenCodeAdapter::with_root(&path);
    let listing = adapter.list_sessions_scoped(24, None);
    assert_eq!(listing.window_count, IN_WINDOW);
    assert_eq!(listing.sessions.len(), IN_WINDOW);
    for s in &listing.sessions {
        assert!(s.session_id.starts_with("ses_bulk30"), "{}", s.session_id);
        assert_eq!(s.user_count, 1, "{}", s.session_id);
        assert_eq!(s.line_count, 1, "{}", s.session_id);
        assert!(s.file_size > 0, "{}", s.session_id);
    }
    assert!(
        !listing
            .sessions
            .iter()
            .any(|s| s.session_id[8..12].parse::<i32>().unwrap() < OUT_OF_WINDOW as i32),
        "none of the 3,000 out-of-window sessions may appear"
    );
}
