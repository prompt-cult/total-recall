//! The listing row shape: the two id arrays a store can grow without limit are
//! bounded where the row is produced, with the true count alongside, so the
//! response budget decides the row count instead of one fat row. Every
//! navigational field a caller pages the store with stays. Written RED.

use total_recall::{IDS_PER_LISTING_ROW, ListingRow, SessionSummary};

/// A row with `children` child ids and `aliases` alias ids — the two arrays a
/// subagent-heavy store and a vibe dedupe grow without bound.
fn fat_summary(children: usize, aliases: usize) -> SessionSummary {
    SessionSummary {
        session_id: "ses_parent0000000000000000000a".to_string(),
        title: "parent session".to_string(),
        start_time: "2026-10-04T09:00:00Z".to_string(),
        end_time: "2026-10-04T10:00:00Z".to_string(),
        file_size: 95_844,
        line_count: 2_400,
        user_count: 12,
        assistant_count: 180,
        tool_count: 940,
        has_compaction: true,
        directory: Some("/Users/dev/parent".to_string()),
        parent_session_id: None,
        child_sessions: (0..children)
            .map(|i| format!("ses_child{i:05}0000000000000000000aa"))
            .collect(),
        has_tantivy_index: true,
        aliases: (0..aliases)
            .map(|i| format!("session_2026100409{i:06}_abcdef"))
            .collect(),
        read_error: None,
    }
}

/// The measured defect: 300 child ids in one row is ~10 KB of the 16 KiB
/// default budget. The rendered row carries the cap and the truth.
#[test]
fn a_row_with_300_children_is_bounded_with_the_true_count_alongside() {
    let row = ListingRow::new(&fat_summary(300, 0));
    assert_eq!(
        row.child_sessions.len(),
        IDS_PER_LISTING_ROW,
        "the row carries at most the per-row id cap"
    );
    assert_eq!(
        row.child_session_count, 300,
        "the row states the true child count"
    );
    assert!(
        serde_json::to_string_pretty(&row).unwrap().len() < 2048,
        "the rendered row stays a row, not a report"
    );
    let notice = row.notice.as_deref().unwrap_or_default();
    assert!(
        notice.contains("290") && notice.contains("child_sessions"),
        "the row states how many ids it held back: {notice}"
    );
}

/// Aliases are bounded for the same reason: a vibe dedupe group records every
/// other directory name that resolves to the payload, and nothing bounded that
/// list at the source.
#[test]
fn a_row_with_50_aliases_is_bounded_with_the_true_count_alongside() {
    let row = ListingRow::new(&fat_summary(0, 50));
    assert_eq!(row.aliases.len(), IDS_PER_LISTING_ROW);
    assert_eq!(row.alias_count, 50);
    let notice = row.notice.as_deref().unwrap_or_default();
    assert!(
        notice.contains("40") && notice.contains("aliases"),
        "the row states how many aliases it held back: {notice}"
    );
    assert!(
        serde_json::to_string_pretty(&row).unwrap().len() < 2048,
        "the rendered row stays a row, not a report"
    );
}

/// A lean row carries neither a count nor a notice: nothing was held back, so
/// there is nothing to say, and the bytes go to rows instead.
#[test]
fn a_lean_row_carries_no_counts_and_no_notice() {
    let obj = serde_json::to_value(ListingRow::new(&fat_summary(0, 0)))
        .unwrap()
        .as_object()
        .unwrap()
        .clone();
    for absent in ["child_session_count", "alias_count", "notice", "read_error"] {
        assert!(
            !obj.contains_key(absent),
            "{absent} is absent when there is nothing to state"
        );
    }
    assert_eq!(
        obj["child_sessions"].as_array().map(Vec::len),
        Some(0),
        "no children, no array weight"
    );
}

/// Every id under the cap travels with the row: the bound cuts fat, it does
/// not thin out lean rows.
#[test]
fn ids_under_the_cap_all_travel_with_the_row() {
    let obj = serde_json::to_value(ListingRow::new(&fat_summary(3, 2))).unwrap();
    assert_eq!(obj["child_sessions"].as_array().map(Vec::len), Some(3));
    assert_eq!(obj["aliases"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        obj["child_session_count"].as_u64(),
        Some(3),
        "the count states what the ids are, even when nothing was held back"
    );
    assert!(
        obj["notice"].is_null(),
        "nothing was held back, so nothing is said"
    );
}

/// Every field a caller navigates with survives the bound. The row shape is
/// pinned by this test so a future field cannot be dropped from the listing
/// without a red suite.
#[test]
fn the_row_keeps_every_field_a_caller_navigates_with() {
    let mut summary = fat_summary(1, 1);
    summary.read_error = Some("permission denied".to_string());
    let obj = serde_json::to_value(ListingRow::new(&summary)).unwrap();
    let mut keys: Vec<&str> = obj
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "alias_count",
            "aliases",
            "assistant_count",
            "child_session_count",
            "child_sessions",
            "directory",
            "end_time",
            "file_size",
            "has_compaction",
            "has_tantivy_index",
            "line_count",
            "parent_session_id",
            "read_error",
            "session_id",
            "start_time",
            "title",
            "tool_count",
            "user_count",
        ],
        "the rendered row is the whole navigational surface, minus what a lean row has nothing to say about"
    );
}

/// The cap is a value, not a mood: small enough that a pathological row cannot
/// dominate a 16 KiB budget, large enough that a caller can see a subagent
/// fan-out and the alias set without paging.
#[test]
fn the_per_row_id_cap_is_ten() {
    assert_eq!(IDS_PER_LISTING_ROW, 10);
}
