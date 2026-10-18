//! #36: the summary range ledger, served end to end over a fixture store.
//!
//! The cache-hit path must compose the stored parts and never construct an
//! LLM provider — a vendor-free build can still serve what was summarised.
//!
//! The cache-miss path (delta summarise) has NO integration test here: the
//! provider is constructed inside the flow (not injectable), and a default
//! build loads the developer's `.env` via dotenvy — so such a test would call
//! the LIVE Mercury API, breaking the suite's own rule that no test ever
//! does. The miss path's mechanics (bounds, delta slicing, ledger append) are
//! covered by the unit tests in `src/summary_cache.rs`.

mod common;

use total_recall::RolloutAdapter;
use total_recall::summary_cache;
use total_recall::rollout::mock::MockAdapter;

use common::scratch::{ScratchRoot, scratch};

fn msg(role: &str, content: &str, timestamp: &str) -> String {
    serde_json::json!({
        "role": role,
        "content": content,
        "thinking": Option::<&str>::None,
        "tool_calls_summary": [],
        "injected": false,
        "timestamp": timestamp,
    })
    .to_string()
}

fn fixture(name: &str, lines: &[String]) -> (ScratchRoot, MockAdapter) {
    let dir = scratch(&format!("summary_cache_{name}"));
    let data_path = dir.join("data.jsonl");
    std::fs::write(&data_path, lines.join("\n") + "\n").unwrap();
    (dir, MockAdapter::new(&data_path))
}

#[tokio::test]
async fn a_session_whose_max_ts_is_covered_by_the_ledger_serves_the_cache_without_an_llm_call() {
    let (_dir, adapter) = fixture(
        "hit",
        &[
            msg("user", "early work", "2026-10-18T10:00:00Z"),
            msg("assistant", "done", "2026-10-18T12:00:00Z"),
        ],
    );
    // Seed the ledger as if this session had already been summarised to its
    // current max ts. The seeded marker text in the served output proves the
    // text came from the cache, not from any provider.
    summary_cache::append_part(
        &adapter.shadow_index_root(),
        "mock",
        "2026-10-18T10:00:00Z",
        "2026-10-18T12:00:00Z",
        "CACHED-TEXT the earlier work, summarised",
    )
    .unwrap();

    let out = summary_cache::summarise_session(&adapter, "mock", true, None)
        .await
        .expect("a covered session is served from cache");
    assert!(
        out.contains("CACHED-TEXT the earlier work, summarised"),
        "the cached part text is served: {out}"
    );
    assert!(
        out.contains("2026-10-18T10:00:00Z → 2026-10-18T12:00:00Z (cached"),
        "the served summary names its range and the cache hit: {out}"
    );
}

#[test]
fn the_ledger_lives_in_the_shadow_root_next_to_the_profile_cache() {
    let (_dir, adapter) = fixture(
        "layout",
        &[msg("user", "hello", "2026-10-18T10:00:00Z")],
    );
    let root = adapter.shadow_index_root();
    assert_eq!(
        summary_cache::ledger_path(&root, "mock"),
        root.join("tr_mock_summary.csv"),
        "one ledger per session, in the shadow root"
    );
}
