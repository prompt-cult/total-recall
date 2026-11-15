mod common;

use std::path::PathBuf;
use total_recall::{EventType, RolloutAdapter, VibeAdapter};

use common::scratch::scratch;

fn test_data_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rollouts/vibe_sessions")
}

#[test]
fn test_read_vibe_session() {
    let adapter = VibeAdapter::with_root(test_data_root());
    let messages = adapter
        .read_session("4a0051b6")
        .expect("healthy fixture read");

    assert!(
        !messages.is_empty(),
        "Should read messages from vibe session"
    );
    // The session has 6 lines in messages.jsonl
    assert_eq!(messages.len(), 6, "Should read exactly 6 messages");
}

#[test]
fn test_read_vibe_session_mmap() {
    let adapter = VibeAdapter::with_root(test_data_root());
    let messages = adapter
        .read_session_mmap("4a0051b6")
        .expect("healthy fixture mmap read");

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
    assert!(
        !small_session.title.is_empty(),
        "Should have a title from meta.json"
    );
    assert!(small_session.title.contains("README"));
}

#[test]
fn test_vibe_profile_session() {
    let adapter = VibeAdapter::with_root(test_data_root());
    let profile = adapter
        .profile_session("4a0051b6")
        .expect("healthy fixture profile");

    assert_eq!(profile.line_count, 6);
    assert!(profile.file_size > 0);
    assert!(profile.role_counts.contains_key("user"));
    assert!(profile.role_counts.contains_key("assistant"));
}

#[test]
fn test_vibe_profile_compaction_session() {
    let adapter = VibeAdapter::with_root(test_data_root());
    let profile = adapter
        .profile_session("2a421f21")
        .expect("healthy fixture profile");

    assert!(
        profile.line_count > 100,
        "Compaction session should have many lines"
    );

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
    let user_messages = adapter
        .extract_user_messages("4a0051b6")
        .expect("healthy fixture user messages");

    assert!(!user_messages.is_empty(), "Should extract user messages");
    // The first user message should mention README
    assert!(user_messages[0].contains("README"));
}

#[test]
fn test_vibe_extract_user_messages_no_injected() {
    let adapter = VibeAdapter::with_root(test_data_root());
    let user_messages = adapter
        .extract_user_messages("2a421f21")
        .expect("healthy fixture user messages");

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
    let messages = adapter
        .read_session_mmap("2a421f21")
        .expect("healthy fixture mmap read");
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
    use total_recall::build_structured_prompt;

    let adapter = VibeAdapter::with_root(test_data_root());
    let messages = adapter
        .read_session_mmap("2a421f21")
        .expect("healthy fixture mmap read");

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
    let full = adapter
        .read_session_mmap("2a421f21")
        .expect("healthy fixture mmap read");
    let from_compaction = adapter
        .read_session_from_compaction("2a421f21")
        .expect("healthy fixture compaction read");

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
    let full = adapter
        .read_session_mmap("4a0051b6")
        .expect("healthy fixture mmap read");
    let from_compaction = adapter
        .read_session_from_compaction("4a0051b6")
        .expect("healthy fixture compaction read");

    // No compaction marker -> return all messages
    assert_eq!(
        full.len(),
        from_compaction.len(),
        "Without compaction marker, should return all messages"
    );
}

// --- #9: list_sessions dedupes the same rollout under two session ids ---

fn make_session_dir(root: &std::path::Path, name: &str, start: &str, end: &str, messages: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("messages.jsonl"), messages).unwrap();
    std::fs::write(
        dir.join("meta.json"),
        format!(
            "{{\"session_id\":\"uuid-{name}\",\"start_time\":\"{start}\",\"end_time\":\"{end}\",\"title\":\"t\",\"total_messages\":1}}"
        ),
    )
    .unwrap();
}

const DUP_MSGS: &str = "{\"role\":\"user\",\"content\":\"same body\",\"timestamp\":\"2026-09-15T09:59:55Z\",\"injected\":false}\n";

#[test]
fn duplicate_rollout_dirs_dedupe_to_one_canonical_entry() {
    let root = scratch("dup");
    // The canonical id's date prefix agrees with meta.start_time; the alias's
    // date prefix disagrees (the reported #9 shape).
    make_session_dir(
        &root,
        "session_20260915_095955_51a9645a",
        "2026-09-15T09:59:55+00:00",
        "2026-09-15T10:00:00+00:00",
        DUP_MSGS,
    );
    make_session_dir(
        &root,
        "session_20260921_135113_c20a924e",
        "2026-09-15T09:59:55+00:00",
        "2026-09-15T10:00:00+00:00",
        DUP_MSGS,
    );

    let adapter = VibeAdapter::with_root(&*root);
    let sessions = adapter.list_sessions();
    assert_eq!(
        sessions.len(),
        1,
        "duplicate dirs must collapse to one entry"
    );
    let s = &sessions[0];
    assert_eq!(
        s.session_id, "session_20260915_095955_51a9645a",
        "canonical id is the one whose date prefix agrees with start_time"
    );
    assert_eq!(
        s.aliases,
        vec!["session_20260921_135113_c20a924e".to_string()]
    );
}

#[test]
fn distinct_rollouts_are_not_deduped() {
    let root = scratch("distinct");
    make_session_dir(
        &root,
        "session_20260915_095955_51a9645a",
        "2026-09-15T09:59:55+00:00",
        "2026-09-15T10:00:00+00:00",
        DUP_MSGS,
    );
    make_session_dir(
        &root,
        "session_20260921_135113_c20a924e",
        "2026-09-21T13:51:13+00:00",
        "2026-09-21T14:00:00+00:00",
        "{\"role\":\"user\",\"content\":\"different body\"}\n",
    );

    let adapter = VibeAdapter::with_root(&*root);
    let sessions = adapter.list_sessions();
    assert_eq!(sessions.len(), 2, "distinct content must stay two entries");
    assert!(sessions.iter().all(|s| s.aliases.is_empty()));
}

#[test]
fn empty_sessions_are_never_deduped_together() {
    let root = scratch("empty");
    make_session_dir(
        &root,
        "session_20260915_095955_51a9645a",
        "2026-09-15T09:59:55+00:00",
        "2026-09-15T10:00:00+00:00",
        "",
    );
    make_session_dir(
        &root,
        "session_20260921_135113_c20a924e",
        "2026-09-21T13:51:13+00:00",
        "2026-09-21T14:00:00+00:00",
        "",
    );

    let adapter = VibeAdapter::with_root(&*root);
    let sessions = adapter.list_sessions();
    assert_eq!(sessions.len(), 2, "empty stores must not merge");
}

#[test]
fn canonical_and_alias_read_the_same_rollout() {
    let root = scratch("read_same");
    make_session_dir(
        &root,
        "session_20260915_095955_51a9645a",
        "2026-09-15T09:59:55+00:00",
        "2026-09-15T10:00:00+00:00",
        DUP_MSGS,
    );
    make_session_dir(
        &root,
        "session_20260921_135113_c20a924e",
        "2026-09-15T09:59:55+00:00",
        "2026-09-15T10:00:00+00:00",
        DUP_MSGS,
    );

    let adapter = VibeAdapter::with_root(&*root);
    let via_canonical = adapter
        .read_session("51a9645a")
        .expect("healthy fixture read");
    let via_alias = adapter
        .read_session("c20a924e")
        .expect("healthy fixture read");
    assert_eq!(via_canonical.len(), via_alias.len());
    assert_eq!(via_canonical.len(), 1);
    assert_eq!(via_canonical[0].content, via_alias[0].content);
}

fn set_mtime(path: &std::path::Path, t: std::time::SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

#[test]
fn scoped_listing_windows_by_payload_mtime_and_dedupes_inside_the_window() {
    let root = scratch("vibe_scoped");
    let now = std::time::SystemTime::now();
    let hour = std::time::Duration::from_secs(3600);

    // Three distinct fresh sessions plus one duplicate pair (both members in
    // the window), and one distinct stale session three days old.
    make_session_dir(
        &root,
        "session_20260915_095955_51a9645a",
        "2026-09-15T09:59:55+00:00",
        "2026-09-15T10:00:00+00:00",
        "{\"role\":\"user\",\"content\":\"fresh a\"}\n",
    );
    make_session_dir(
        &root,
        "session_20260922_101500_51a9645b",
        "2026-09-22T10:15:00+00:00",
        "2026-09-22T10:20:00+00:00",
        "{\"role\":\"user\",\"content\":\"fresh b\"}\n",
    );
    make_session_dir(
        &root,
        "session_20260921_135113_c20a924e",
        "2026-09-15T09:59:55+00:00",
        "2026-09-15T10:00:00+00:00",
        "{\"role\":\"user\",\"content\":\"dup one\"}\n",
    );
    make_session_dir(
        &root,
        "session_20260921_135114_c20a924f",
        "2026-09-15T09:59:55+00:00",
        "2026-09-15T10:00:00+00:00",
        "{\"role\":\"user\",\"content\":\"dup one\"}\n",
    );
    make_session_dir(
        &root,
        "session_20260923_101500_51a9645c",
        "2026-09-23T10:15:00+00:00",
        "2026-09-23T10:20:00+00:00",
        "{\"role\":\"user\",\"content\":\"stale c\"}\n",
    );

    // mtimes: a = now, dup pair = now-1h, b = now-2h, c = 3 days ago.
    for (name, t) in [
        ("session_20260915_095955_51a9645a", now),
        ("session_20260921_135113_c20a924e", now - hour),
        ("session_20260921_135114_c20a924f", now - hour),
        ("session_20260922_101500_51a9645b", now - 2 * hour),
        (
            "session_20260923_101500_51a9645c",
            now - std::time::Duration::from_secs(3 * 24 * 3600),
        ),
    ] {
        set_mtime(&root.join(name).join("messages.jsonl"), t);
    }

    let adapter = VibeAdapter::with_root(&*root);

    // The 24-hour window: the stale session is invisible (not merely
    // unprinted), the duplicate pair collapses to its canonical entry, and
    // the rows are most-recent-first by payload mtime.
    let listing = adapter.list_sessions_scoped(24, None);
    assert_eq!(listing.window_count, 3);
    let ids: Vec<&str> = listing
        .sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            "session_20260915_095955_51a9645a",
            "session_20260921_135113_c20a924e",
            "session_20260922_101500_51a9645b",
        ],
        "most recent first; the canonical entry carries the pair's alias"
    );
    assert_eq!(
        listing.sessions[1].aliases,
        vec!["session_20260921_135114_c20a924f".to_string()],
        "the deduped alias survives the scoped listing"
    );

    // hours_back 0 is no bound at the mechanism layer: the stale session is
    // listed too, ordered last by its mtime.
    let listing = adapter.list_sessions_scoped(0, None);
    assert_eq!(listing.window_count, 4);
    assert_eq!(
        listing.sessions.last().map(|s| s.session_id.as_str()),
        Some("session_20260923_101500_51a9645c")
    );
}

/// A dedupe group bigger than the per-row id cap is a real store shape, not a
/// thought experiment: fifteen directories for one payload is fifteen names,
/// and the canonical entry's `aliases` array carries all fourteen. The rendered
/// row carries the cap, the true count and the notice; the summary the adapter
/// produced keeps the whole set for the readers that need it.
#[test]
fn a_dedupe_group_larger_than_the_row_cap_renders_bounded() {
    use total_recall::{IDS_PER_LISTING_ROW, ListingRow};

    let root = scratch("vibe_alias_cap");
    let now = std::time::SystemTime::now();
    let group = 15;
    for i in 0..group {
        let name = format!("session_20260915_095955_{i:06x}a");
        make_session_dir(
            &root,
            &name,
            "2026-09-15T09:59:55+00:00",
            "2026-09-15T10:00:00+00:00",
            DUP_MSGS,
        );
        set_mtime(&root.join(&name).join("messages.jsonl"), now);
    }

    let listing = VibeAdapter::with_root(&*root).list_sessions_scoped(24, None);
    assert_eq!(listing.window_count, 1, "one payload, one canonical entry");
    let canonical = &listing.sessions[0];
    assert_eq!(
        canonical.aliases.len(),
        group - 1,
        "the adapter's summary carries every alias name"
    );

    let row = ListingRow::new(canonical);
    assert_eq!(row.aliases.len(), IDS_PER_LISTING_ROW);
    assert_eq!(row.alias_count, group - 1);
    let notice = row.notice.as_deref().unwrap_or_default();
    assert!(
        notice.contains("4 of 14 aliases not shown"),
        "the row names what it held back: {notice}"
    );
}
