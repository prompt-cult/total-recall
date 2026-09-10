use inception_mercury_compaction::{MockAdapter, RolloutAdapter};
use std::io::Write;

fn test_data_path() -> String {
    env!("CARGO_MANIFEST_DIR").to_string() + "/.tmp/test-data/mock_sample.jsonl"
}

#[test]
fn test_mock_read_from_compaction() {
    let adapter = MockAdapter::new(test_data_path());
    let all_messages = adapter.read_session_mmap("test");
    let from_compaction = adapter.read_session_from_compaction("test");

    // The mock data has a compaction marker on line 5 (0-indexed: 4)
    // "You are continuing a trajectory after a context compaction"
    assert!(
        from_compaction.len() <= all_messages.len(),
        "From-compaction should return fewer or equal messages"
    );
    // The compaction marker is at index 4, so from_compaction should have 4 messages (indices 4-7)
    assert_eq!(
        from_compaction.len(),
        4,
        "Should return 4 messages from compaction point onward"
    );
    // First message should contain the compaction marker
    assert!(from_compaction[0].content.contains("context compaction"));
}

#[test]
fn test_mock_read_from_compaction_no_marker() {
    // Create a file with no compaction marker
    let temp = std::env::temp_dir().join("test_no_compaction.jsonl");
    let mut f = std::fs::File::create(&temp).unwrap();
    let _ = f.write_all(
        br#"{"role":"user","content":"hello","tool_calls_summary":[],"timestamp":null,"injected":false}
{"role":"assistant","content":"hi","tool_calls_summary":[],"timestamp":null,"injected":false}"#,
    );
    let adapter = MockAdapter::new(&temp);
    let messages = adapter.read_session_from_compaction("test");
    // No compaction marker -> return all messages
    assert_eq!(messages.len(), 2);
    let _ = std::fs::remove_file(&temp);
}

#[test]
fn test_mock_compaction_vs_full() {
    let adapter = MockAdapter::new(test_data_path());
    let full = adapter.read_session_mmap("test");
    let from_compaction = adapter.read_session_from_compaction("test");

    assert!(
        full.len() > from_compaction.len(),
        "Full read should have more messages than from-compaction"
    );
    assert_eq!(full.len(), 8);
    assert_eq!(from_compaction.len(), 4);
}
