mod common;

use std::path::PathBuf;
use total_recall::RolloutAdapter;
use total_recall::rollout::codex::CodexAdapter;

use common::scratch::scratch;

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
    let messages = adapter
        .read_session("codex_small")
        .expect("healthy fixture read");

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
    let messages = adapter
        .read_session_mmap("codex_small")
        .expect("healthy fixture mmap read");

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
    let profile = adapter
        .profile_session("codex_small")
        .expect("healthy fixture profile");

    assert!(profile.line_count > 0);
    assert!(profile.file_size > 0);
    assert!(
        profile.role_counts.contains_key("user") || profile.role_counts.contains_key("assistant")
    );
}

#[test]
fn test_codex_extract_user_messages() {
    let adapter = CodexAdapter::with_root(test_data_root());
    let messages = adapter
        .extract_user_messages("codex_small")
        .expect("healthy fixture user messages");

    assert!(!messages.is_empty(), "Should extract user messages");
}

#[test]
fn test_codex_list_sessions_scoped_windows_by_payload_mtime() {
    let tmp = scratch("codex_scoped");
    let a = tmp.join("session_a.jsonl");
    let b = tmp.join("session_b.jsonl");
    std::fs::write(&a, "{\"role\":\"user\",\"content\":\"fresh codex line\"}\n").unwrap();
    std::fs::write(&b, "{\"role\":\"user\",\"content\":\"stale codex line\"}\n").unwrap();

    let now = std::time::SystemTime::now();
    let three_days_ago = now - std::time::Duration::from_secs(3 * 24 * 3600);
    for (path, t) in [(&a, now), (&b, three_days_ago)] {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    let adapter = CodexAdapter::with_root(&*tmp);

    // The 24-hour window holds only the fresh session — the stale one is
    // invisible, not merely unprinted — with its aggregates computed.
    let listing = adapter.list_sessions_scoped(24, None);
    assert_eq!(listing.window_count, 1);
    assert_eq!(listing.sessions.len(), 1);
    assert_eq!(listing.sessions[0].session_id, "session_a.jsonl");
    assert_eq!(listing.sessions[0].user_count, 1);
    assert_eq!(listing.sessions[0].line_count, 1);

    // hours_back 0 is no bound at the mechanism layer: both, most-recent-first.
    let listing = adapter.list_sessions_scoped(0, None);
    assert_eq!(listing.window_count, 2);
    assert_eq!(listing.sessions[0].session_id, "session_a.jsonl");
    assert_eq!(listing.sessions[1].session_id, "session_b.jsonl");
}
