//! Dedicated regression tests for #86 and #87:
//! - #86: extract-by-type `tool` entries carry the call's name and arguments
//!   (identity, not just the synthesized assistant summary).
//! - #87: a non-full extract reports the compaction window instead of
//!   silently serving only the post-compaction segment.

use total_recall::rollout::{
    CompactionWindow, RolloutEntry, RolloutMessage, attach_tool_call_identity,
    compaction_window_of, entries_from_messages, parse_tool_call_summary,
};

fn msg(role: &str, content: &str, summaries: &[&str], ts: &str) -> RolloutMessage {
    RolloutMessage {
        role: role.to_string(),
        content: content.to_string(),
        thinking: None,
        tool_calls_summary: summaries.iter().map(|s| s.to_string()).collect(),
        timestamp: Some(ts.to_string()),
        injected: false,
    }
}

// --- #86: tool identity ---

#[test]
fn tool_entry_carries_call_name_and_arguments() {
    let messages = vec![
        msg("user", "ls the dir", &[], "2026-11-01T10:00:00Z"),
        msg(
            "assistant",
            "",
            &["read_file(/etc/hosts)"],
            "2026-11-01T10:00:01Z",
        ),
        msg("tool", "127.0.0.1 localhost", &[], "2026-11-01T10:00:02Z"),
    ];
    let entries = entries_from_messages(&messages);
    let tool = entries
        .iter()
        .find(|e| e.entry_type == "tool")
        .expect("tool entry exists");
    assert_eq!(tool.record["tool_call_name"], "read_file");
    assert_eq!(tool.record["tool_call_input"], "/etc/hosts");
    assert_eq!(tool.record["tool_calls"][0], "read_file(/etc/hosts)");
}

#[test]
fn tool_entry_identity_best_effort_falls_back_to_whole_line() {
    let messages = vec![
        msg("assistant", "", &["bash: echo hi"], "2026-11-01T10:00:01Z"),
        msg("tool", "hi", &[], "2026-11-01T10:00:02Z"),
    ];
    let entries = entries_from_messages(&messages);
    let tool = entries
        .iter()
        .find(|e| e.entry_type == "tool")
        .expect("tool entry exists");
    assert_eq!(tool.record["tool_call_name"], "bash");
    assert_eq!(tool.record["tool_call_input"], "echo hi");
}

#[test]
fn tool_entry_without_pending_call_stays_unchanged() {
    let messages = vec![msg("tool", "output", &[], "2026-11-01T10:00:02Z")];
    let entries = entries_from_messages(&messages);
    let tool = entries
        .iter()
        .find(|e| e.entry_type == "tool")
        .expect("tool entry exists");
    assert!(tool.record.get("tool_call_name").is_none());
    assert!(tool.record.get("tool_call_input").is_none());
}

#[test]
fn multiple_tool_entries_consume_pending_calls_in_order() {
    let messages = vec![
        msg(
            "assistant",
            "",
            &["grep(foo, src)", "read_file(Cargo.toml)"],
            "2026-11-01T10:00:01Z",
        ),
        msg("tool", "hit", &[], "2026-11-01T10:00:02Z"),
        msg("tool", "contents", &[], "2026-11-01T10:00:03Z"),
    ];
    let entries = entries_from_messages(&messages);
    let tools: Vec<&RolloutEntry> = entries.iter().filter(|e| e.entry_type == "tool").collect();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].record["tool_call_name"], "grep");
    assert_eq!(tools[1].record["tool_call_name"], "read_file");
}

#[test]
fn parse_tool_call_summary_shapes() {
    assert_eq!(
        parse_tool_call_summary("bash: ls"),
        ("bash".to_string(), Some("ls".to_string()))
    );
    assert_eq!(
        parse_tool_call_summary("write_file(/tmp/x, 5 chars)"),
        (
            "write_file".to_string(),
            Some("/tmp/x, 5 chars".to_string())
        )
    );
    assert_eq!(
        parse_tool_call_summary("plain"),
        ("plain".to_string(), None)
    );
}

#[test]
fn attach_identity_never_rewrites_existing_fields() {
    let mut record = serde_json::json!({"role": "tool", "content": "out"});
    let pending = vec!["read_file(/etc/hosts)".to_string()];
    let mut cursor = 0;
    attach_tool_call_identity(&mut record, &pending, &mut cursor);
    assert_eq!(record["role"], "tool");
    assert_eq!(record["content"], "out");
    assert_eq!(record["tool_call_name"], "read_file");
    assert_eq!(cursor, 1);
}

// --- #87: compaction window ---

#[test]
fn compaction_window_reports_slice_after_marker() {
    let messages = vec![
        msg("user", "a", &[], "2026-11-01T10:00:00Z"),
        msg("user", "b", &[], "2026-11-01T10:00:01Z"),
        msg(
            "assistant",
            "context compaction",
            &[],
            "2026-11-01T10:00:02Z",
        ),
        msg("user", "c", &[], "2026-11-01T10:00:03Z"),
        msg("tool", "d", &[], "2026-11-01T10:00:04Z"),
    ];
    assert_eq!(
        compaction_window_of(&messages),
        CompactionWindow {
            begins_at_compaction: true,
            elided_messages: 3,
        }
    );
}

#[test]
fn compaction_window_no_marker_is_full_window() {
    let messages = vec![msg("user", "a", &[], "2026-11-01T10:00:00Z")];
    assert_eq!(
        compaction_window_of(&messages),
        CompactionWindow {
            begins_at_compaction: false,
            elided_messages: 0,
        }
    );
}
