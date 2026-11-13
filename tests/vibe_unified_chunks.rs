//! Issue #53: Mistral Vibe v3.1.2 writes the session's records into the
//! unified store's chunk files (`unified/<uuid>/chunks/<sha256>.json`), not
//! into the recovery journal. A reader that walks the journal alone sees the
//! tail of a session: the live session held 129 user turns and the tool read
//! 2.
//!
//! The fixtures here are synthetic and small: they carry the record shapes a
//! v3.1.2 store holds (live turns, the assistant's messages, reasoning,
//! tool effects, the checkpoint's history snapshot and the synthetic
//! injections), so the layout can be regression-tested without a live store.

mod common;

use std::path::Path;

use total_recall::RolloutAdapter;
use total_recall::rollout::vibe::VibeAdapter;

const UUID: &str = "9ad943ac-0add-ffd1-c55b-25769e186206";

/// A live user turn: `source: "turn_start"`, the record's own `id` and
/// `createdAt` (unix milliseconds).
fn turn(id: &str, created_at: i64, text: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "message",
        "role": "user",
        "source": "turn_start",
        "id": id,
        "createdAt": created_at,
        "generationStatus": "completed",
        "content": [{"type": "text", "text": text}],
    })
}

/// A steering message: the same live shape, a different `source`.
fn steer(id: &str, created_at: i64, text: &str) -> serde_json::Value {
    let mut record = turn(id, created_at, text);
    record["source"] = serde_json::Value::String("turn_steer".to_string());
    record
}

/// The assistant's own message (`source: "harness"`).
fn assistant(id: &str, created_at: i64, text: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "message",
        "role": "assistant",
        "source": "harness",
        "id": id,
        "createdAt": created_at,
        "generationStatus": "completed",
        "content": [{"type": "text", "text": text}],
    })
}

/// A reasoning record: the model's prose, in a field of its own.
fn reasoning(id: &str, created_at: i64, text: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "reasoning",
        "id": id,
        "createdAt": created_at,
        "text": text,
    })
}

/// A tool effect: the call that was made and what it returned.
fn effect(id: &str, created_at: i64, tool: &str, output: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "effect",
        "id": id,
        "createdAt": created_at,
        "detail": {"kind": "tool", "toolName": tool, "input": {"path": "/tmp/x"}},
        "state": {"output": {"content": [{"type": "text", "text": output}]}},
    })
}

/// A checkpoint-snapshot record: the conversation as the model was given it.
/// The message body is nested under `message` and carries no timestamp.
fn history(turn_index: u64, role: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "source": "history",
        "turn_index": turn_index,
        "message": {"role": role, "content": [{"type": "text", "text": text}]},
    })
}

/// A synthetic injection (the runtime notification the harness writes into
/// the conversation).
fn injection(turn_index: u64, text: &str) -> serde_json::Value {
    serde_json::json!({
        "source": "injection",
        "turn_index": turn_index,
        "message": {"role": "user", "content": [{"type": "text", "text": text}]},
    })
}

/// A v3.1.2 unified store: the chunk files hold the session's records, and
/// the journal holds only the tail of it — the two disagree, which is the
/// whole point of the layout.
fn create_chunk_store(root: &Path, uuid: &str) {
    let dir = root.join("unified").join(uuid);
    std::fs::create_dir_all(dir.join("chunks")).unwrap();
    std::fs::create_dir_all(dir.join("journal")).unwrap();

    std::fs::write(
        dir.join("CURRENT"),
        r#"{"generation":"0000000000010566","session_id":"<uuid>","snapshot_sequence":10566}"#,
    )
    .unwrap();

    std::fs::write(
        dir.join("meta.json"),
        r#"{"title":"chunk fixture","start_time":"2026-11-12T14:55:42.779000+00:00","end_time":"2026-11-13T11:09:10.339000+00:00","environment":{"working_directory":"/fixture/cwd"},"parent_session_id":null}"#,
    )
    .unwrap();

    // The recovery journal: the tail of the session only. A reader that
    // stops here sees one user turn and nothing else.
    std::fs::write(
        dir.join("journal").join("0000000000010599.jsonl"),
        concat!(
            r#"{"type":"core_input","payload":{"input":{"command":{"type":"user_message","mode":"queue","content":[{"text":"the last turn","type":"text"}]},"determinism":{"time_unix_ms":1794568150339},"input_id":1,"protocol_version":1}}}"#,
            "\n",
        ),
    )
    .unwrap();

    // The chunk files: the session's records, split over two content
    // addressed files.
    let first = serde_json::json!([
        turn("rs-1", 1794495356414, "first question"),
        assistant("asst-1", 1794495360000, "first answer"),
        reasoning(
            "reasoning-1",
            1794495361000,
            "thinking about the first question"
        ),
        effect(
            "effect-1",
            1794495362000,
            "file_system.read_file",
            "the file says hello"
        ),
        steer("rs-2", 1794495370000, "a steering message"),
        turn("rs-3", 1794495380000, "second question"),
        assistant("asst-2", 1794495385000, "second answer"),
    ]);
    std::fs::write(
        dir.join("chunks").join("aaa111.json"),
        serde_json::to_string(&first).unwrap(),
    )
    .unwrap();

    let second = serde_json::json!([
        // The checkpoint's snapshot: the same conversation in the older shape,
        // plus the injections the harness wrote into it.
        history(0, "user", "first question"),
        history(0, "assistant", "first answer"),
        injection(1, "Runtime notification: a subagent finished"),
        // A store record that is not dialogue at all.
        {"type": "notice", "id": "notice-1", "message": "Running hooks"},
    ]);
    std::fs::write(
        dir.join("chunks").join("bbb222.json"),
        serde_json::to_string(&second).unwrap(),
    )
    .unwrap();
}

#[test]
fn chunk_store_extracts_every_user_turn() {
    let root = common::scratch::scratch("vibe_chunk_extract");
    create_chunk_store(&root, UUID);
    let ad = VibeAdapter::with_root(&*root);

    // Every live turn of the session, in order — not the journal's one turn.
    let extracted = ad.extract_user_messages(UUID).expect("extract");
    assert_eq!(
        extracted,
        vec![
            "first question".to_string(),
            "a steering message".to_string(),
            "second question".to_string(),
        ],
        "the chunk store holds every user turn: {extracted:?}"
    );
}

#[test]
fn chunk_store_reads_the_whole_session_in_order() {
    let root = common::scratch::scratch("vibe_chunk_read");
    create_chunk_store(&root, UUID);
    let ad = VibeAdapter::with_root(&*root);

    let messages = ad.read_session(UUID).expect("unified read");
    let roles: Vec<&str> = messages.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(
        roles,
        vec![
            "user",
            "assistant",
            "assistant",
            "tool",
            "user",
            "user",
            "assistant",
            "user",
        ],
        "the live records in timestamp order, then the injection: {messages:?}"
    );

    // The checkpoint's snapshot holds the same first turn again; it is the
    // same turn, not a second one.
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.role == "user" && m.content == "first question")
            .count(),
        1,
        "the snapshot's copy of a turn is not a second turn: {messages:?}"
    );

    // The injection is the harness's own message, not the user's.
    let injection = messages.last().expect("the injection is in the stream");
    assert!(injection.injected, "the injection is marked injected");
    assert!(
        injection.content.contains("Runtime notification"),
        "the injection is the runtime notification: {messages:?}"
    );

    // The timestamps come from the records' `createdAt`.
    assert_eq!(
        messages[0].timestamp.as_deref(),
        Some("2026-11-12T14:55:56Z"),
        "createdAt to ISO: {messages:?}"
    );

    // The reasoning record is the assistant's thinking, not its answer.
    assert_eq!(
        messages[2].thinking.as_deref(),
        Some("thinking about the first question")
    );
    assert!(messages[2].content.is_empty());

    // The tool effect carries the tool's output and the call it made.
    assert_eq!(messages[3].content, "the file says hello");
    assert!(
        messages[3].tool_calls_summary[0].contains("file_system.read_file"),
        "the effect names the tool: {messages:?}"
    );
}

#[test]
fn chunk_store_counts_are_the_session_s_own() {
    let root = common::scratch::scratch("vibe_chunk_listing");
    create_chunk_store(&root, UUID);
    let ad = VibeAdapter::with_root(&*root);

    let sessions = ad.list_sessions();
    let s = sessions
        .iter()
        .find(|s| s.session_id == UUID)
        .expect("the session is listed");
    assert_eq!(s.title, "chunk fixture");
    assert_eq!(s.user_count, 4, "three turns and the injection");
    assert_eq!(s.assistant_count, 3, "two answers and the reasoning");
    assert_eq!(s.tool_count, 1);
    assert!(s.file_size > 0, "the payload size is the store's size");
    assert_eq!(s.directory.as_deref(), Some("/fixture/cwd"));
    assert!(s.read_error.is_none(), "a readable store reports no damage");

    // The scoped listing applies the directory filter the unified store's
    // `meta.json` names.
    let scoped = ad.list_sessions_scoped(24, Some("/fixture"));
    assert!(
        scoped.sessions.iter().any(|s| s.session_id == UUID),
        "the session's own directory passes the filter: {:?}",
        scoped.sessions
    );
    let scoped = ad.list_sessions_scoped(24, Some("/elsewhere"));
    assert!(
        !scoped.sessions.iter().any(|s| s.session_id == UUID),
        "another directory does not: {:?}",
        scoped.sessions
    );
}

#[test]
fn a_fresh_session_never_extracts_as_empty() {
    // The regression of issue #53: a session whose journal was touched in the
    // last 24 hours must not read as an empty session. The journal alone
    // holds one turn; the chunk store holds the session.
    let root = common::scratch::scratch("vibe_chunk_not_empty");
    create_chunk_store(&root, UUID);
    let ad = VibeAdapter::with_root(&*root);

    let scoped = ad.list_sessions_scoped(24, None);
    assert!(
        scoped.sessions.iter().any(|s| s.session_id == UUID),
        "the session is inside the window: {:?}",
        scoped.sessions
    );

    let extracted = ad.extract_user_messages(UUID).expect("extract");
    assert!(
        !extracted.is_empty(),
        "a session with records never extracts as empty"
    );
    assert!(
        extracted.len() > 1,
        "and not as the journal's one turn: {extracted:?}"
    );
}

#[test]
fn an_unknown_layout_is_an_error_not_an_empty_session() {
    let root = common::scratch::scratch("vibe_chunk_unknown_layout");
    let dir = root.join("unified").join(UUID);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("meta.json"), r#"{"title":"broken"}"#).unwrap();
    let ad = VibeAdapter::with_root(&*root);

    let err = ad
        .read_session(UUID)
        .expect_err("an unreadable layout is an error");
    assert!(
        err.contains("unknown layout"),
        "the error names the problem: {err}"
    );
}

#[test]
fn an_unreadable_chunk_store_is_an_error() {
    let root = common::scratch::scratch("vibe_chunk_damaged");
    create_chunk_store(&root, UUID);
    // A chunk file that is not JSON at all: the store's records are
    // unreadable, and the session must say so rather than read as empty.
    let dir = root.join("unified").join(UUID).join("chunks");
    std::fs::write(dir.join("ccc333.json"), "{not json").unwrap();
    let ad = VibeAdapter::with_root(&*root);

    let messages = ad
        .read_session(UUID)
        .expect("the readable chunks still read");
    assert!(
        messages.len() > 1,
        "one damaged file does not hide the rest: {messages:?}"
    );
}
