//! Tests for the reusable clipping library (`crate::clip`).

use total_recall::clip::{
    ByteBudget, ClipSide, ClipStrategy, clip_by_strategy, clip_bytes, dynamic_clip_row, marker,
    sizes,
};

#[test]
fn end_clip_never_splits_multibyte() {
    // 🎉 is 4 bytes (U+1F389); three of them are 12 bytes.
    let s = "🎉🎉🎉";
    let (kept, clipped, dropped) = clip_bytes(s, 6, ClipSide::End);
    assert!(clipped);
    // The 6-byte window backs off to the char boundary at 4, not into a char.
    assert_eq!(kept, "🎉");
    assert_eq!(dropped, 8);
    assert_eq!(kept.len() + dropped, s.len());
}

#[test]
fn end_clip_backs_off_to_char_boundary_in_mixed_text() {
    // "abcé": é is 2 bytes, so boundaries are 0,1,2,3,5 — 4 is not one.
    let s = "abcé";
    let (kept, clipped, dropped) = clip_bytes(s, 4, ClipSide::End);
    assert!(clipped);
    assert_eq!(kept, "abc");
    assert_eq!(dropped, 2);
}

#[test]
fn front_clip_never_splits_multibyte() {
    let s = "🎉🎉🎉"; // 12 bytes
    let (kept, clipped, dropped) = clip_bytes(s, 6, ClipSide::Front);
    assert!(clipped);
    // The cut at 6 advances to the char boundary at 8; the kept suffix is
    // shorter than the budget but whole.
    assert_eq!(kept, "🎉");
    assert_eq!(dropped, 8);
    assert_eq!(kept.len() + dropped, s.len());
}

#[test]
fn short_strings_are_not_clipped() {
    let s = "hello";
    assert_eq!(clip_bytes(s, 100, ClipSide::End), (s, false, 0));
    assert_eq!(clip_bytes(s, 100, ClipSide::Front), (s, false, 0));
    // Exactly at the bound: nothing to drop.
    assert_eq!(clip_bytes(s, 5, ClipSide::End), (s, false, 0));
    assert_eq!(clip_bytes(s, 5, ClipSide::Front), (s, false, 0));
}

#[test]
fn front_and_end_keep_complementary_extremes() {
    let s = "abcdefghij";
    let (tail, tail_clipped, tail_dropped) = clip_bytes(s, 4, ClipSide::Front);
    let (head, head_clipped, head_dropped) = clip_bytes(s, 4, ClipSide::End);
    assert_eq!(tail, "ghij");
    assert!(tail_clipped);
    assert_eq!(tail_dropped, 6);
    assert_eq!(head, "abcd");
    assert!(head_clipped);
    assert_eq!(head_dropped, 6);
}

#[test]
fn zero_budget_clips_everything() {
    let (kept, clipped, dropped) = clip_bytes("abc", 0, ClipSide::End);
    assert_eq!(kept, "");
    assert!(clipped);
    assert_eq!(dropped, 3);
    let (kept, clipped, dropped) = clip_bytes("abc", 0, ClipSide::Front);
    assert_eq!(kept, "");
    assert!(clipped);
    assert_eq!(dropped, 3);
}

#[test]
fn budget_exhaustion_stops_rows() {
    let mut budget = ByteBudget::new(10);
    let mut out: Vec<String> = Vec::new();
    for row in ["aaaa", "bbbb", "cccc", "dddd"] {
        let got = dynamic_clip_row(&mut budget, row.to_string(), |s| s.len(), |s| s);
        if got.is_empty() {
            break;
        }
        out.extend(got);
    }
    // 10 / 4 admits two rows (8 bytes); the third (4 bytes) no longer fits —
    // the driver stops.
    assert_eq!(out, vec!["aaaa".to_string(), "bbbb".to_string()]);
    assert_eq!(budget.used(), 8);
    assert_eq!(budget.remaining(), 2);
    assert!(!budget.is_exhausted());
}

#[test]
fn rows_are_clipped_into_the_remaining_budget() {
    let mut budget = ByteBudget::new(10);
    let room = budget.remaining();
    let row = "x".repeat(20);
    let got = dynamic_clip_row(
        &mut budget,
        row,
        |s| s.len(),
        |s| clip_bytes(&s, room, ClipSide::End).0.to_string(),
    );
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].len(), 10);
    assert_eq!(budget.used(), 10);
    assert!(budget.is_exhausted());
    // A further row cannot fit even clipped to nothing.
    let room = budget.remaining();
    let got = dynamic_clip_row(
        &mut budget,
        "z".to_string(),
        |s| s.len(),
        |s| clip_bytes(&s, room, ClipSide::End).0.to_string(),
    );
    assert!(got.is_empty());
}

#[test]
fn admit_never_overdraws_the_budget() {
    let mut budget = ByteBudget::new(6);
    assert!(budget.admit(4));
    assert!(budget.admit(2));
    assert!(budget.is_exhausted());
    assert!(!budget.admit(1));
    assert_eq!(budget.used(), 6);
}

#[test]
fn marker_emits_canonical_notice() {
    assert_eq!(
        marker(true, 123, Some("file:///tmp/full.bin")),
        "[clipped: 123 bytes; full: file:///tmp/full.bin]"
    );
    assert_eq!(marker(true, 5, None), "[clipped: 5 bytes]");
    // Unclipped output carries no notice — a silent clip is a lie.
    assert_eq!(marker(false, 0, None), "");
    assert_eq!(marker(false, 999, Some("sid://x")), "");
}

#[test]
fn truncate_strategy_dispatches_to_clip_bytes() {
    let s = "abcdef";
    let (kept, clipped, dropped) = clip_by_strategy(
        s,
        ClipStrategy::Truncate {
            max_bytes: 3,
            side: ClipSide::End,
        },
    );
    assert_eq!(kept, "abc");
    assert!(clipped);
    assert_eq!(dropped, 3);

    let (kept, clipped, dropped) = clip_by_strategy(
        s,
        ClipStrategy::Truncate {
            max_bytes: 3,
            side: ClipSide::Front,
        },
    );
    assert_eq!(kept, "def");
    assert!(clipped);
    assert_eq!(dropped, 3);
}

#[test]
fn take_while_keeps_leading_matching_chars() {
    let s = "   indented";
    let (kept, clipped, dropped) = clip_by_strategy(
        s,
        ClipStrategy::TakeWhile {
            predicate: |c| c == ' ',
        },
    );
    assert_eq!(kept, "   ");
    assert!(clipped);
    assert_eq!(dropped, 8);
    assert_eq!(kept.len() + dropped, s.len());
}

#[test]
fn drop_while_drops_leading_matching_chars() {
    let s = "  \t indented";
    let (kept, clipped, dropped) = clip_by_strategy(
        s,
        ClipStrategy::DropWhile {
            predicate: |c| c.is_whitespace(),
        },
    );
    assert_eq!(kept, "indented");
    assert!(clipped);
    assert_eq!(dropped, 4);
    assert_eq!(kept.len() + dropped, s.len());
}

#[test]
fn take_while_with_nothing_to_drop_is_not_clipped() {
    let s = "abc";
    let (kept, clipped, dropped) = clip_by_strategy(
        s,
        ClipStrategy::TakeWhile {
            predicate: |c| c.is_ascii_alphabetic(),
        },
    );
    assert_eq!(kept, "abc");
    assert!(!clipped);
    assert_eq!(dropped, 0);
}

#[test]
fn sizes_match_inventory_values() {
    // Copy of values from the Section A inventory — assert by literal so a
    // drift in the copy is a test failure, not a silent divergence.
    assert_eq!(sizes::MAX_STATE_BYTES, 200 * 1024);
    assert_eq!(sizes::MAX_GOALS_BYTES, 200 * 1024);
    assert_eq!(sizes::MARKER_RESERVE, 256);
    assert_eq!(sizes::TOOL_MESSAGE_TRUNCATE_CHARS, 500);
    assert_eq!(sizes::OTHER_MESSAGE_TRUNCATE_CHARS, 1500);
    assert_eq!(sizes::TOOL_CALL_FALLBACK_TRUNCATE_CHARS, 200);
    assert_eq!(sizes::TOOL_CALL_BASH_TRUNCATE_CHARS, 300);
    assert_eq!(sizes::DEFAULT_RECORD_LIMIT, 100);
    assert_eq!(sizes::MAX_RECORD_LIMIT, 1000);
    assert_eq!(sizes::DEFAULT_MAX_BYTES, 8 * 1024 * 1024);
    assert_eq!(sizes::DEFAULT_MAX_RECORD_BYTES, 256 * 1024);
    assert_eq!(sizes::REPORT_TEXT_FLOOD_CAP_BYTES, 16 * 1024);
    assert_eq!(sizes::PRIOR_PERIOD_TODO_LOG_LIST_CAP, 200);
    assert_eq!(sizes::PRIOR_PERIOD_COMPACTION_FOOTER_CAP, 3);
    assert_eq!(sizes::PRIOR_PERIOD_LINE_TRUNCATE, 300);
}
