mod common;

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use total_recall::{CodexAdapter, EventType, RolloutAdapter};

use common::lock_env;

fn codex_test_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rollouts/codex_sessions")
}

fn adapter() -> CodexAdapter {
    CodexAdapter::with_root(codex_test_root())
}

const SESSION_A: &str = "01aa1111-2222-3333-4444-555566667777";
const SESSION_B: &str = "02bb2222-3333-4444-5555-666677778888";

/// Pin the two fixture files' mtimes so "most recent" and window tests are
/// deterministic: session A is newer than session B.
fn pin_mtimes() {
    let set = |rel: &str, secs_since_epoch: u64| {
        let path = codex_test_root().join(rel);
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs_since_epoch))
            .unwrap();
    };
    set(
        "2026/11/12/rollout-2026-11-12T08-00-00-01aa1111-2222-3333-4444-555566667777.jsonl",
        1_800_000_000,
    );
    set(
        "2026/11/11/rollout-2026-11-11T09-00-00-02bb2222-3333-4444-5555-666677778888.jsonl",
        1_700_000_000,
    );
}

#[test]
fn test_codex_adapter_name() {
    assert_eq!(adapter().name(), "codex");
}

#[test]
fn test_read_codex_session() {
    let messages = adapter()
        .read_session(SESSION_A)
        .expect("healthy fixture read");
    // user, assistant, assistant (tool-call summary), tool (custom output),
    // tool (function output), injected compaction marker, user, assistant
    assert_eq!(messages.len(), 8, "should read 8 normalized messages");

    let roles: Vec<&str> = messages.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(
        roles,
        vec![
            "user",
            "assistant",
            "assistant",
            "tool",
            "tool",
            "system",
            "user",
            "assistant",
        ]
    );

    assert_eq!(messages[0].content, "Summarise this repo.");
    assert_eq!(messages[1].content, "I'll look at the tree first.");
    assert_eq!(messages[6].content, "Now write the summary.");
    assert_eq!(messages[7].content, "Done.");
}

#[test]
fn test_codex_tool_call_summary_attached_to_assistant() {
    let messages = adapter().read_session(SESSION_A).unwrap();
    let call = &messages[2];
    assert_eq!(call.role, "assistant");
    assert_eq!(call.tool_calls_summary.len(), 1);
    assert!(call.tool_calls_summary[0].contains("exec("));
    let result = &messages[3];
    assert_eq!(result.role, "tool");
    assert_eq!(result.content, "On branch main nothing to commit");
    let function_result = &messages[4];
    assert_eq!(function_result.role, "tool");
    assert_eq!(function_result.content, "{\"page_id\":null}");
}

#[test]
fn test_codex_encrypted_reasoning_is_not_a_message() {
    let messages = adapter().read_session(SESSION_A).unwrap();
    let has_reasoning = messages
        .iter()
        .any(|m| m.thinking.is_some() || m.content.contains("encrypted"));
    assert!(
        !has_reasoning,
        "encrypted reasoning records must not surface as messages"
    );
}

#[test]
fn test_codex_developer_message_is_skipped() {
    let messages = adapter().read_session(SESSION_A).unwrap();
    assert!(
        messages.iter().all(|m| m.role != "developer"),
        "developer scaffolding records must be skipped"
    );
}

#[test]
fn test_read_codex_session_mmap() {
    let messages = adapter().read_session_mmap(SESSION_A).expect("mmap read");
    assert_eq!(messages.len(), 8);
}

#[test]
fn test_codex_list_sessions() {
    pin_mtimes();
    let sessions = adapter().list_sessions();
    assert_eq!(sessions.len(), 2, "should find both fixture sessions");

    // Sorted by start_time descending: session A (Nov 12) then B (Nov 11).
    assert_eq!(sessions[0].session_id, SESSION_A);
    assert_eq!(sessions[1].session_id, SESSION_B);

    let a = &sessions[0];
    assert_eq!(a.user_count, 2);
    assert_eq!(a.assistant_count, 3);
    assert_eq!(a.tool_count, 2);
    assert!(
        a.has_compaction,
        "session A carries a typed compacted record"
    );
    assert!(a.line_count >= 15);
    assert!(a.file_size > 0);
    assert!(a.title.contains("codex-fixture-project"), "title from cwd");
    assert_eq!(
        a.directory.as_deref(),
        Some("/Users/sample/codex-fixture-project")
    );

    let b = &sessions[1];
    assert_eq!(b.user_count, 1);
    assert_eq!(b.assistant_count, 1);
    assert_eq!(b.tool_count, 0);
    assert!(!b.has_compaction);
}

#[test]
fn test_codex_list_sessions_scoped_directory_filter() {
    let listing = adapter().list_sessions_scoped(0, Some("codex-fixture-project"));
    assert_eq!(listing.sessions.len(), 1);
    assert_eq!(listing.sessions[0].session_id, SESSION_A);
    assert_eq!(listing.window_count, 1);

    let listing = adapter().list_sessions_scoped(0, Some("nothing-matches"));
    assert!(listing.sessions.is_empty());
    assert_eq!(listing.window_count, 0);
}

#[test]
fn test_codex_most_recent_session_id() {
    pin_mtimes();
    assert_eq!(
        adapter().most_recent_session_id().as_deref(),
        Some(SESSION_A)
    );
}

#[test]
fn test_codex_resolve_canonical_id_partial() {
    let adapter = adapter();
    assert_eq!(
        adapter.resolve_canonical_id("01aa1111").as_deref(),
        Some(SESSION_A)
    );
    assert_eq!(
        adapter.resolve_canonical_id("02bb2222").as_deref(),
        Some(SESSION_B)
    );
    assert_eq!(adapter.resolve_canonical_id("zzzz"), None);
    assert_eq!(adapter.resolve_canonical_id("").as_deref(), Some(SESSION_A));
}

#[test]
fn test_codex_profile_session() {
    let profile = adapter()
        .profile_session(SESSION_A)
        .expect("healthy fixture profile");

    assert_eq!(profile.session_id, SESSION_A);
    assert!(profile.file_size > 0);
    assert_eq!(profile.line_count, 15);
    assert_eq!(
        profile.first_ts.as_deref(),
        Some("2026-11-12T08:00:00.000Z")
    );
    assert_eq!(profile.last_ts.as_deref(), Some("2026-11-12T08:03:00.000Z"));
    assert_eq!(profile.role_counts.get("user"), Some(&2));
    assert_eq!(profile.role_counts.get("assistant"), Some(&3));
    assert_eq!(profile.role_counts.get("tool"), Some(&2));

    let compaction_events: Vec<_> = profile
        .interesting_events
        .iter()
        .filter(|e| e.event_type == EventType::Compaction)
        .collect();
    assert_eq!(compaction_events.len(), 1);
    assert_eq!(compaction_events[0].line_number, 12);

    let user_events: Vec<_> = profile
        .interesting_events
        .iter()
        .filter(|e| e.event_type == EventType::UserMessage)
        .collect();
    assert_eq!(user_events.len(), 2);
    assert_eq!(user_events[0].line_number, 5);
    assert_eq!(user_events[1].line_number, 13);
    assert!(user_events[0].summary.starts_with("Summarise this repo"));
}

#[test]
fn test_codex_read_from_compaction_slices_after_marker() {
    let messages = adapter()
        .read_session_from_compaction(SESSION_A)
        .expect("healthy fixture compaction read");
    // The slice keeps the injected marker and everything after it.
    assert_eq!(messages.len(), 3);
    assert!(messages[0].injected && messages[0].content.contains("context compaction"));
    assert_eq!(messages[1].role, "user");
    assert_eq!(messages[2].role, "assistant");
}

#[test]
fn test_codex_compaction_markers_only_injected() {
    let messages = adapter().read_session(SESSION_A).unwrap();
    let markers = adapter().compaction_markers(&messages);
    assert_eq!(markers, vec![5]);
}

#[test]
fn test_codex_extract_user_messages() {
    let users = adapter()
        .extract_user_messages(SESSION_A)
        .expect("healthy fixture user messages");
    assert_eq!(
        users,
        vec![
            "Summarise this repo.".to_string(),
            "Now write the summary.".to_string()
        ]
    );
}

#[test]
fn test_codex_read_session_entries_default() {
    let entries = adapter()
        .read_session_entries(SESSION_A, true, false)
        .expect("healthy fixture entries");
    // user x2, assistant x3 (one is the tool-call summary), tool x2; the
    // injected compaction marker is filtered by the default.
    assert_eq!(entries.len(), 7);
    let types: Vec<&str> = entries.iter().map(|e| e.entry_type.as_str()).collect();
    assert_eq!(
        types,
        vec![
            "user",
            "assistant",
            "assistant",
            "tool",
            "tool",
            "user",
            "assistant",
        ]
    );

    // From the last compaction point: only the post-compaction turns.
    let tail = adapter()
        .read_session_entries(SESSION_A, false, false)
        .expect("healthy fixture tail entries");
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0].entry_type, "user");
    assert_eq!(tail[1].entry_type, "assistant");
}

#[test]
fn test_codex_no_session_error() {
    let err = adapter().read_session("nope").unwrap_err();
    assert!(err.contains("no session matching 'nope'"), "{err}");
    assert!(err.contains("codex_sessions"), "{err}");
}

#[test]
fn test_codex_root_env_override() {
    let _guard = lock_env();
    let root = codex_test_root();
    let _env = common::EnvGuard::set("TOTAL_RECALL_CODEX_ROOT", root.to_str().unwrap());
    let adapter = CodexAdapter::new();
    assert!(
        adapter.root_is_from_env(),
        "env override must mark from_env"
    );
    assert_eq!(adapter.read_session(SESSION_A).unwrap().len(), 8);
}
