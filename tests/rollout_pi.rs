mod common;

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use total_recall::{EventType, PiAdapter, RolloutAdapter};

use common::lock_env;

fn pi_test_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rollouts/pi_sessions")
}

fn adapter() -> PiAdapter {
    PiAdapter::with_root(pi_test_root())
}

const SESSION_A: &str = "01aa1111-2222-3333-4444-555566667777";
const SESSION_B: &str = "02bb2222-3333-4444-5555-666677778888";

fn pin_mtimes() {
    let set = |rel: &str, secs_since_epoch: u64| {
        let path = pi_test_root().join(rel);
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs_since_epoch))
            .unwrap();
    };
    set(
        "--Users-sample-pi-fixture-project--/2026-11-12T08-35-30-123Z_01aa1111-2222-3333-4444-555566667777.jsonl",
        1_800_000_000,
    );
    set(
        "--Users-sample-other-project--/2026-11-11T09-00-00-000Z_02bb2222-3333-4444-5555-666677778888.jsonl",
        1_700_000_000,
    );
}

#[test]
fn test_pi_adapter_name() {
    assert_eq!(adapter().name(), "pi");
}

#[test]
fn test_read_pi_session() {
    let messages = adapter()
        .read_session(SESSION_A)
        .expect("healthy fixture read");
    assert_eq!(messages.len(), 6, "should read 6 normalized messages");

    let roles: Vec<&str> = messages.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(
        roles,
        vec![
            "user",
            "assistant",
            "tool",
            "assistant",
            "user",
            "assistant",
        ]
    );

    assert_eq!(messages[0].content, "Set up the demo project.");
    assert_eq!(
        messages[1].content,
        "I'll take a look at the current directory."
    );
    assert_eq!(
        messages[3].content,
        "Found a demo folder — setting it up now."
    );
    assert_eq!(messages[4].content, "Now add a README.");
    assert_eq!(messages[5].content, "README added.");
}

#[test]
fn test_pi_assistant_thinking_and_tool_call() {
    let messages = adapter().read_session(SESSION_A).unwrap();
    let assistant = &messages[1];
    let thinking = assistant
        .thinking
        .as_deref()
        .expect("assistant carries thinking");
    assert!(thinking.contains("demo project"), "{thinking}");
    assert_eq!(assistant.tool_calls_summary.len(), 1);
    assert!(assistant.tool_calls_summary[0].contains("bash:"));

    let tool = &messages[2];
    assert_eq!(tool.role, "tool");
    assert!(tool.content.contains("total 8"));
    assert!(tool.thinking.is_none());
}

#[test]
fn test_pi_system_message_is_skipped() {
    let messages = adapter().read_session(SESSION_A).unwrap();
    assert!(
        messages.iter().all(|m| m.role != "system"),
        "system scaffolding records must be skipped"
    );
}

#[test]
fn test_read_pi_session_mmap() {
    let messages = adapter().read_session_mmap(SESSION_A).expect("mmap read");
    assert_eq!(messages.len(), 6);
}

#[test]
fn test_pi_list_sessions() {
    pin_mtimes();
    let sessions = adapter().list_sessions();
    assert_eq!(sessions.len(), 2, "should find both fixture sessions");

    assert_eq!(sessions[0].session_id, SESSION_A);
    assert_eq!(sessions[1].session_id, SESSION_B);

    let a = &sessions[0];
    assert_eq!(a.user_count, 2);
    assert_eq!(a.assistant_count, 3);
    assert_eq!(a.tool_count, 1);
    assert!(a.line_count >= 10);
    assert!(a.file_size > 0);
    assert!(
        a.title.contains("pi-fixture-project"),
        "title from cwd, got: {}",
        a.title
    );
    assert_eq!(
        a.directory.as_deref(),
        Some("/Users/sample/pi-fixture-project")
    );

    let b = &sessions[1];
    assert_eq!(b.user_count, 1);
    assert_eq!(b.assistant_count, 1);
    assert_eq!(b.tool_count, 1);
}

#[test]
fn test_pi_list_sessions_scoped_directory_filter() {
    let listing = adapter().list_sessions_scoped(0, Some("pi-fixture-project"));
    assert_eq!(listing.sessions.len(), 1);
    assert_eq!(listing.sessions[0].session_id, SESSION_A);
    assert_eq!(listing.window_count, 1);

    let listing = adapter().list_sessions_scoped(0, Some("nothing-matches"));
    assert!(listing.sessions.is_empty());
    assert_eq!(listing.window_count, 0);
}

#[test]
fn test_pi_most_recent_session_id() {
    pin_mtimes();
    assert_eq!(
        adapter().most_recent_session_id().as_deref(),
        Some(SESSION_A)
    );
}

#[test]
fn test_pi_resolve_canonical_id_partial() {
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
fn test_pi_profile_session() {
    let profile = adapter()
        .profile_session(SESSION_A)
        .expect("healthy fixture profile");

    assert_eq!(profile.session_id, SESSION_A);
    assert!(profile.file_size > 0);
    assert_eq!(profile.line_count, 10);
    assert_eq!(
        profile.first_ts.as_deref(),
        Some("2026-11-12T08:35:30.123Z")
    );
    assert_eq!(profile.last_ts.as_deref(), Some("2026-11-12T08:35:52.000Z"));
    assert_eq!(profile.role_counts.get("user"), Some(&2));
    assert_eq!(profile.role_counts.get("assistant"), Some(&3));
    assert_eq!(profile.role_counts.get("tool"), Some(&1));

    let user_events: Vec<_> = profile
        .interesting_events
        .iter()
        .filter(|e| e.event_type == EventType::UserMessage)
        .collect();
    assert_eq!(user_events.len(), 2);
    assert_eq!(user_events[0].line_number, 5);
    assert_eq!(user_events[1].line_number, 9);
    assert!(
        user_events[0]
            .summary
            .starts_with("Set up the demo project")
    );
}

#[test]
fn test_pi_read_from_compaction_no_marker_returns_all() {
    let messages = adapter().read_session_from_compaction(SESSION_A).unwrap();
    assert_eq!(messages.len(), 6, "no compaction marker → all messages");
}

#[test]
fn test_pi_extract_user_messages() {
    let users = adapter()
        .extract_user_messages(SESSION_A)
        .expect("healthy fixture user messages");
    assert_eq!(
        users,
        vec![
            "Set up the demo project.".to_string(),
            "Now add a README.".to_string()
        ]
    );
}

#[test]
fn test_pi_read_session_entries_default() {
    let entries = adapter()
        .read_session_entries(SESSION_A, true, false)
        .expect("healthy fixture entries");
    // 6 messages + 1 thinking entry (the assistant's reasoning part).
    assert_eq!(entries.len(), 7);
    let types: Vec<&str> = entries.iter().map(|e| e.entry_type.as_str()).collect();
    assert_eq!(
        types,
        vec![
            "user",
            "assistant",
            "thinking",
            "tool",
            "assistant",
            "user",
            "assistant",
        ]
    );
}

#[test]
fn test_pi_no_session_error() {
    let err = adapter().read_session("nope").unwrap_err();
    assert!(err.contains("no session matching 'nope'"), "{err}");
    assert!(err.contains("pi_sessions"), "{err}");
}

#[test]
fn test_pi_root_env_override() {
    let _guard = lock_env();
    let root = pi_test_root();
    let _env = common::EnvGuard::set("TOTAL_RECALL_PI_ROOT", root.to_str().unwrap());
    let adapter = PiAdapter::new();
    assert!(
        adapter.root_is_from_env(),
        "env override must mark from_env"
    );
    assert_eq!(adapter.read_session(SESSION_A).unwrap().len(), 6);
}
