//! Unit tests for the bounded-extraction primitives.

use total_recall::bound::*;
use total_recall::rollout::RolloutMessage;

fn msg(content: &str) -> RolloutMessage {
    RolloutMessage {
        role: "user".to_string(),
        content: content.to_string(),
        thinking: None,
        tool_calls_summary: Vec::new(),
        timestamp: Some("2026-09-15T09:59:55Z".to_string()),
        injected: false,
    }
}

#[test]
fn normalize_limit_defaults_and_caps() {
    assert_eq!(normalize_limit(0).unwrap(), DEFAULT_RECORD_LIMIT);
    assert_eq!(normalize_limit(50).unwrap(), 50);
    assert!(normalize_limit(MAX_RECORD_LIMIT + 1).is_err());
}

#[test]
fn normalize_max_bytes_clamps_to_ceiling() {
    assert_eq!(normalize_max_bytes(0), MAX_BYTES_CEILING);
    assert_eq!(normalize_max_bytes(1024), 1024);
    assert_eq!(normalize_max_bytes(usize::MAX), MAX_BYTES_CEILING);
}

#[test]
fn window_tail_selects_most_recent() {
    // offset None -> tail of the list
    assert_eq!(window(1000, None, 100), (900, 1000));
    // fewer records than the limit -> whole list
    assert_eq!(window(50, None, 100), (0, 50));
}

#[test]
fn window_offset_walks_from_index() {
    assert_eq!(window(1000, Some(0), 100), (0, 100));
    assert_eq!(window(1000, Some(950), 100), (950, 1000));
    // offset past the end clamps to empty
    assert_eq!(window(1000, Some(5000), 100), (1000, 1000));
}

#[test]
fn clamp_message_respects_char_boundaries() {
    let mut m = msg(&"é".repeat(1000)); // 2-byte chars, 2000 bytes
    let clamped = clamp_message(&mut m, 100);
    assert!(clamped);
    assert!(m.content.len() <= 100);
    // did not split a UTF-8 char
    assert!(m.content.chars().all(|c| c == 'é'));
}

#[test]
fn clamp_message_unlimited_is_noop() {
    let mut m = msg(&"x".repeat(1_000_000));
    assert!(!clamp_message(&mut m, usize::MAX));
    assert_eq!(m.content.len(), 1_000_000);
}

#[test]
fn fit_count_stops_at_budget() {
    let records: Vec<SizedRecord> = (0..10)
        .map(|_| SizedRecord {
            json: "x".repeat(100),
            bytes: 100,
        })
        .collect();
    // budget 350 -> 3 records (3*101 = 303), 4th would be 404 > 350
    assert_eq!(fit_count(&records, 350), 3);
    // always emits at least one record even if it alone exceeds the budget
    assert_eq!(fit_count(&records, 1), 1);
}

#[test]
fn finalize_bounds_reports_truncation_and_next_offset() {
    let b = finalize_bounds(7525, 0, 100, 100, true, false, vec![], 8_388_608, 50_000);
    assert!(b.truncated);
    assert_eq!(b.truncation_reason, "record_limit");
    assert_eq!(b.next_offset, Some(100));
    assert!(b.notice.contains("next_offset=100"));

    let done = finalize_bounds(50, 0, 100, 50, false, false, vec![], 8_388_608, 5_000);
    assert!(!done.truncated);
    assert_eq!(done.next_offset, None);
    assert!(done.notice.is_empty());
}
