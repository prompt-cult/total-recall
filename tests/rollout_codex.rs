use std::path::PathBuf;
use total_recall::RolloutAdapter;
use total_recall::rollout::codex::CodexAdapter;

fn test_data_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rollouts")
}

#[test]
fn test_codex_adapter_name() {
    let adapter = CodexAdapter::with_root(test_data_root());
    assert_eq!(adapter.name(), "codex");
}

#[test]
fn test_codex_read_session() {
    let adapter = CodexAdapter::with_root(test_data_root());
    let messages = adapter.read_session("codex_small");

    assert!(
        !messages.is_empty(),
        "Should read messages from codex sample"
    );
    // All messages should have role and content
    for msg in &messages {
        assert!(!msg.role.is_empty(), "Role should not be empty");
    }
}

#[test]
fn test_codex_read_session_mmap() {
    let adapter = CodexAdapter::with_root(test_data_root());
    let messages = adapter.read_session_mmap("codex_small");

    assert!(!messages.is_empty());
}

#[test]
fn test_codex_list_sessions() {
    let adapter = CodexAdapter::with_root(test_data_root());
    let sessions = adapter.list_sessions();

    assert!(
        !sessions.is_empty(),
        "Should find at least one codex session"
    );
    let codex_session = sessions
        .iter()
        .find(|s| s.session_id.contains("codex"))
        .expect("Should find codex_small.jsonl");
    assert!(codex_session.line_count > 0);
}

#[test]
fn test_codex_profile_session() {
    let adapter = CodexAdapter::with_root(test_data_root());
    let profile = adapter.profile_session("codex_small");

    assert!(profile.line_count > 0);
    assert!(profile.file_size > 0);
    assert!(
        profile.role_counts.contains_key("user") || profile.role_counts.contains_key("assistant")
    );
}

#[test]
fn test_codex_extract_user_messages() {
    let adapter = CodexAdapter::with_root(test_data_root());
    let messages = adapter.extract_user_messages("codex_small");

    assert!(!messages.is_empty(), "Should extract user messages");
}
