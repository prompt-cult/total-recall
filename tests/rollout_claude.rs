use std::path::PathBuf;
use total_recall::RolloutAdapter;
use total_recall::rollout::claude::ClaudeAdapter;

fn test_data_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rollouts")
}

#[test]
fn test_claude_adapter_name() {
    let adapter = ClaudeAdapter::with_root(test_data_root());
    assert_eq!(adapter.name(), "claude");
}

#[test]
fn test_claude_read_session() {
    let adapter = ClaudeAdapter::with_root(test_data_root());
    let messages = adapter.read_session("claude_small");

    assert!(
        !messages.is_empty(),
        "Should read messages from claude sample"
    );
    for msg in &messages {
        assert!(!msg.role.is_empty(), "Role should not be empty");
    }
}

#[test]
fn test_claude_read_session_mmap() {
    let adapter = ClaudeAdapter::with_root(test_data_root());
    let messages = adapter.read_session_mmap("claude_small");

    assert!(!messages.is_empty());
}

#[test]
fn test_claude_list_sessions() {
    let adapter = ClaudeAdapter::with_root(test_data_root());
    let sessions = adapter.list_sessions();

    assert!(
        !sessions.is_empty(),
        "Should find at least one claude session"
    );
    let claude_session = sessions
        .iter()
        .find(|s| s.session_id.contains("claude"))
        .expect("Should find claude_small.jsonl");
    assert!(claude_session.line_count > 0);
}

#[test]
fn test_claude_profile_session() {
    let adapter = ClaudeAdapter::with_root(test_data_root());
    let profile = adapter.profile_session("claude_small");

    assert!(profile.line_count > 0);
    assert!(profile.file_size > 0);
    assert!(
        profile.role_counts.contains_key("user") || profile.role_counts.contains_key("assistant"),
        "Should have user or assistant roles"
    );
}

#[test]
fn test_claude_extract_user_messages() {
    let adapter = ClaudeAdapter::with_root(test_data_root());
    let messages = adapter.extract_user_messages("claude_small");

    assert!(!messages.is_empty(), "Should extract user messages");
}
