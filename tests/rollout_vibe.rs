use inception_mercury_compaction::{EventType, RolloutAdapter, VibeAdapter};
use std::path::PathBuf;

fn test_data_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rollouts/vibe_sessions")
}

#[test]
fn test_read_vibe_session() {
    let adapter = VibeAdapter::with_root(test_data_root());
    let messages = adapter.read_session("4a0051b6");

    assert!(!messages.is_empty(), "Should read messages from vibe session");
    // The session has 6 lines in messages.jsonl
    assert_eq!(messages.len(), 6, "Should read exactly 6 messages");
}

#[test]
fn test_read_vibe_session_mmap() {
    let adapter = VibeAdapter::with_root(test_data_root());
    let messages = adapter.read_session_mmap("4a0051b6");

    assert_eq!(messages.len(), 6, "mmap read should return same count");
}

#[test]
fn test_vibe_adapter_name() {
    let adapter = VibeAdapter::with_root(test_data_root());
    assert_eq!(adapter.name(), "vibe");
}

#[test]
fn test_vibe_list_sessions() {
    let adapter = VibeAdapter::with_root(test_data_root());
    let sessions = adapter.list_sessions();

    assert_eq!(sessions.len(), 2, "Should find 2 test sessions");

    // Should be sorted by start_time descending
    // session_20260623 > session_20260523
    assert!(sessions[0].session_id.contains("20260623"));
    assert!(sessions[1].session_id.contains("20260523"));
}

#[test]
fn test_vibe_list_sessions_has_title() {
    let adapter = VibeAdapter::with_root(test_data_root());
    let sessions = adapter.list_sessions();

    let small_session = sessions
        .iter()
        .find(|s| s.session_id.contains("4a0051b6"))
        .expect("Should find the small session");
    assert!(!small_session.title.is_empty(), "Should have a title from meta.json");
    assert!(small_session.title.contains("README"));
}

#[test]
fn test_vibe_profile_session() {
    let adapter = VibeAdapter::with_root(test_data_root());
    let profile = adapter.profile_session("4a0051b6");

    assert_eq!(profile.line_count, 6);
    assert!(profile.file_size > 0);
    assert!(profile.role_counts.contains_key("user"));
    assert!(profile.role_counts.contains_key("assistant"));
}

#[test]
fn test_vibe_profile_compaction_session() {
    let adapter = VibeAdapter::with_root(test_data_root());
    let profile = adapter.profile_session("2a421f21");

    assert!(profile.line_count > 100, "Compaction session should have many lines");

    // Should find compaction events
    let compaction_events: Vec<_> = profile
        .interesting_events
        .iter()
        .filter(|e| e.event_type == EventType::Compaction)
        .collect();
    assert!(
        !compaction_events.is_empty(),
        "Should find compaction markers in this session"
    );
}

#[test]
fn test_vibe_extract_user_messages() {
    let adapter = VibeAdapter::with_root(test_data_root());
    let user_messages = adapter.extract_user_messages("4a0051b6");

    assert!(!user_messages.is_empty(), "Should extract user messages");
    // The first user message should mention README
    assert!(user_messages[0].contains("README"));
}

#[test]
fn test_vibe_extract_user_messages_no_injected() {
    let adapter = VibeAdapter::with_root(test_data_root());
    let user_messages = adapter.extract_user_messages("2a421f21");

    // None of the extracted messages should be injected
    for msg in &user_messages {
        assert!(
            !msg.contains("context compaction"),
            "Injected compaction messages should not appear in user messages"
        );
    }
}

#[test]
fn test_vibe_read_speed() {
    let adapter = VibeAdapter::with_root(test_data_root());
    let t0 = std::time::Instant::now();
    let messages = adapter.read_session_mmap("2a421f21");
    let read_time = t0.elapsed();

    assert!(!messages.is_empty());
    // 171-line file should read in under 100ms
    assert!(
        read_time.as_millis() < 100,
        "Read should be < 100ms for 171-line file, took {:?}",
        read_time
    );
}

#[test]
fn test_vibe_format_speed() {
    use inception_mercury_compaction::build_structured_prompt;

    let adapter = VibeAdapter::with_root(test_data_root());
    let messages = adapter.read_session_mmap("2a421f21");

    let t0 = std::time::Instant::now();
    let _prompt = build_structured_prompt(&messages);
    let format_time = t0.elapsed();

    assert!(
        format_time.as_millis() < 10,
        "Format should be < 10ms, took {:?}",
        format_time
    );
}

#[test]
fn test_vibe_read_from_compaction() {
    let adapter = VibeAdapter::with_root(test_data_root());
    let full = adapter.read_session_mmap("2a421f21");
    let from_compaction = adapter.read_session_from_compaction("2a421f21");

    // Session 2a421f21 has compaction markers, so from_compaction should return fewer messages
    assert!(
        from_compaction.len() <= full.len(),
        "From-compaction should return fewer or equal messages"
    );
    // The first message from compaction should contain the compaction marker
    assert!(!from_compaction.is_empty());
    assert!(
        from_compaction[0].content.contains("context compaction"),
        "First message from compaction should contain the compaction marker"
    );
}

#[test]
fn test_vibe_read_from_compaction_no_marker() {
    let adapter = VibeAdapter::with_root(test_data_root());
    // Session 4a0051b6 has no compaction markers
    let full = adapter.read_session_mmap("4a0051b6");
    let from_compaction = adapter.read_session_from_compaction("4a0051b6");

    // No compaction marker -> return all messages
    assert_eq!(
        full.len(),
        from_compaction.len(),
        "Without compaction marker, should return all messages"
    );
}
