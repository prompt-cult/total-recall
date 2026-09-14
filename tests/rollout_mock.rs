use total_recall::{EventType, MockAdapter, RolloutAdapter};
use std::io::Write;

fn test_data_path() -> String {
    env!("CARGO_MANIFEST_DIR").to_string() + "/rollouts/mock_sample.jsonl"
}

#[test]
fn test_read_mock_session() {
    let adapter = MockAdapter::new(test_data_path());
    let messages = adapter.read_session("test");

    assert!(!messages.is_empty(), "Should read messages from mock data");
    assert_eq!(messages.len(), 8, "Should read exactly 8 messages");
}

#[test]
fn test_read_mock_session_mmap() {
    let adapter = MockAdapter::new(test_data_path());
    let messages = adapter.read_session_mmap("test");

    assert_eq!(messages.len(), 8, "mmap read should return same count");
}

#[test]
fn test_mock_adapter_name() {
    let adapter = MockAdapter::new(test_data_path());
    assert_eq!(adapter.name(), "mock");
}

#[test]
fn test_mock_list_sessions() {
    let adapter = MockAdapter::new(test_data_path());
    let sessions = adapter.list_sessions();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].line_count, 8);
    assert_eq!(sessions[0].user_count, 3);
    assert_eq!(sessions[0].assistant_count, 4);
    assert_eq!(sessions[0].tool_count, 1);
    assert!(
        sessions[0].has_compaction,
        "Should detect compaction marker"
    );
}

#[test]
fn test_mock_profile_session() {
    let adapter = MockAdapter::new(test_data_path());
    let profile = adapter.profile_session("test");

    assert_eq!(profile.line_count, 8);
    assert!(profile.file_size > 0);
    assert!(profile.role_counts.contains_key("user"));
    assert!(profile.role_counts.contains_key("assistant"));
    assert!(profile.role_counts.contains_key("tool"));

    // Should find compaction event
    let compaction_events: Vec<_> = profile
        .interesting_events
        .iter()
        .filter(|e| e.event_type == EventType::Compaction)
        .collect();
    assert!(
        !compaction_events.is_empty(),
        "Should find compaction event"
    );

    // Should find git commit event
    let commit_events: Vec<_> = profile
        .interesting_events
        .iter()
        .filter(|e| e.event_type == EventType::GitCommit)
        .collect();
    assert!(!commit_events.is_empty(), "Should find git commit event");

    // Should find git push event
    let push_events: Vec<_> = profile
        .interesting_events
        .iter()
        .filter(|e| e.event_type == EventType::GitPush)
        .collect();
    assert!(!push_events.is_empty(), "Should find git push event");
}

#[test]
fn test_mock_extract_user_messages() {
    let adapter = MockAdapter::new(test_data_path());
    let user_messages = adapter.extract_user_messages("test");

    // 3 user messages total, but one is injected
    assert_eq!(
        user_messages.len(),
        2,
        "Should extract 2 non-injected user messages"
    );
    assert!(user_messages[0].contains("Fix the auth bug"));
    assert!(user_messages[1].contains("push to origin"));
}

#[test]
fn test_mock_first_and_last_timestamp() {
    let adapter = MockAdapter::new(test_data_path());
    let profile = adapter.profile_session("test");

    assert!(profile.first_ts.is_some());
    assert!(profile.last_ts.is_some());
    assert_eq!(profile.first_ts.as_deref(), Some("2026-09-10T10:00:00Z"));
    assert_eq!(profile.last_ts.as_deref(), Some("2026-09-10T10:02:05Z"));
}

#[test]
fn test_mock_read_speed() {
    let adapter = MockAdapter::new(test_data_path());
    let t0 = std::time::Instant::now();
    let messages = adapter.read_session_mmap("test");
    let read_time = t0.elapsed();

    assert!(!messages.is_empty());
    // Small file should read in under 100ms
    assert!(
        read_time.as_millis() < 100,
        "Read should be < 100ms, took {:?}",
        read_time
    );
}

#[test]
fn test_mock_format_speed() {
    use total_recall::build_structured_prompt;

    let adapter = MockAdapter::new(test_data_path());
    let messages = adapter.read_session_mmap("test");

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
fn test_mock_empty_file() {
    let temp = std::env::temp_dir().join("test_empty.jsonl");
    let mut f = std::fs::File::create(&temp).unwrap();
    let _ = f.write_all(b"");
    let adapter = MockAdapter::new(&temp);
    let messages = adapter.read_session("test");
    assert!(messages.is_empty());
    let _ = std::fs::remove_file(&temp);
}
