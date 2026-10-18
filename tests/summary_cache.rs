//! #39: the summary cache as an explicit, inspectable data structure.
//!
//! The ledger is 11-column v2: `from_id,from_ts,to_id,to_ts,user,assistant,
//! tool,thinking,chars,tokens_est,part_file`, ids are 1-based message
//! sequence numbers, and a malformed or short (v0.15.0-era) row invalidates
//! the ledger — treated as empty, the file dropped.
//!
//! Every test here is vendor-free-safe: the cache-hit path, the guard
//! errors, the rollback and the introspection never construct an LLM
//! provider — a default build loads the developer's `.env` via dotenvy the
//! moment a provider is built, so no test may reach that path. The real
//! summarise path (provider constructed) has NO test for the same reason the
//! #36 suite stated: it would call the LIVE Mercury API.

mod common;

use total_recall::RolloutAdapter;
use total_recall::rollout::mock::MockAdapter;
use total_recall::summary_cache;

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

/// The four-message fixture: ids 1..=4, timestamps in order.
fn four_messages() -> Vec<String> {
    vec![
        msg("user", "start the work", "2026-10-18T10:00:00Z"),
        msg("assistant", "working", "2026-10-18T11:00:00Z"),
        msg("user", "more work", "2026-10-18T12:00:00Z"),
        msg("assistant", "done", "2026-10-18T13:00:00Z"),
    ]
}

#[tokio::test]
async fn a_session_whose_messages_are_covered_by_the_last_block_serves_the_cache_without_an_llm_call()
 {
    let (_dir, adapter) = fixture(
        "hit",
        &[
            msg("user", "early work", "2026-10-18T10:00:00Z"),
            msg("assistant", "done", "2026-10-18T12:00:00Z"),
        ],
    );
    // Seed the ledger as if this session had already been summarised through
    // id 2 (all of it). The seeded marker text in the served output proves
    // the text came from the cache, not from any provider.
    let messages = adapter.read_session_mmap("mock").unwrap();
    summary_cache::append_block(
        &adapter.shadow_index_root(),
        "mock",
        1,
        &messages,
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
        out.contains("(cached; nothing new since the last summarise"),
        "the served summary states the cache hit: {out}"
    );
}

#[test]
fn the_ledger_lives_in_the_shadow_root_next_to_the_profile_cache() {
    let (_dir, adapter) = fixture("layout", &[msg("user", "hello", "2026-10-18T10:00:00Z")]);
    let root = adapter.shadow_index_root();
    assert_eq!(
        summary_cache::ledger_path(&root, "mock"),
        root.join("tr_mock_summary.csv"),
        "one ledger per session, in the shadow root"
    );
}

#[test]
fn append_block_writes_the_eleven_column_row_with_ids_and_counts() {
    let (_dir, adapter) = fixture("append", &four_messages());
    let root = adapter.shadow_index_root();
    let messages = adapter.read_session_mmap("mock").unwrap();

    let first =
        summary_cache::append_block(&root, "mock", 1, &messages[..2], "first summary").unwrap();
    assert_eq!(first.from_id, 1, "the block starts at the id it was given");
    assert_eq!(first.to_id, 2, "the block ends at its last message's id");
    assert_eq!(first.from_ts, "2026-10-18T10:00:00Z");
    assert_eq!(first.to_ts, "2026-10-18T11:00:00Z");
    assert_eq!(first.user, 1, "the type counts describe the slice");
    assert_eq!(first.assistant, 1);
    assert_eq!(
        first.chars,
        "start the work".len() as u64 + "working".len() as u64
    );
    assert!(
        first.tokens_est > 0 && first.tokens_est <= first.chars,
        "the token estimate is the ~4 chars/token guard, not a guess of zero: {first:?}"
    );

    let second =
        summary_cache::append_block(&root, "mock", 3, &messages[2..], "delta summary").unwrap();
    assert_eq!(second.from_id, 3);
    assert_eq!(second.to_id, 4);

    let blocks = summary_cache::read_ledger(&summary_cache::ledger_path(&root, "mock"));
    assert_eq!(blocks.len(), 2, "two blocks in the ledger");
    assert_eq!(blocks[0].part_file, first.part_file);
    assert_eq!(blocks[1].part_file.file_name().unwrap(), "part_003_004.md");
    assert!(first.part_file.is_file(), "the part file holds the text");
    let composed = summary_cache::compose(&blocks);
    assert!(
        composed.contains("### part 1: 2026-10-18T10:00:00Z → 2026-10-18T11:00:00Z (ids 1..2)"),
        "each part names its id range: {composed}"
    );
    assert!(composed.contains("first summary") && composed.contains("delta summary"));
}

#[test]
fn a_malformed_or_short_row_invalidates_the_ledger_and_drops_the_file() {
    let (_dir, adapter) = fixture("invalid", &four_messages());
    let root = adapter.shadow_index_root();
    let path = summary_cache::ledger_path(&root, "mock");

    // The v0.15.0-era 3-column row: no backward compatibility, the ledger it
    // appears in is treated as empty and dropped.
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        &path,
        "2026-10-18T10:00:00Z,2026-10-18T11:00:00Z,parts/old.md\n",
    )
    .unwrap();
    assert!(
        summary_cache::read_ledger(&path).is_empty(),
        "a short row invalidates the ledger"
    );
    assert!(!path.exists(), "the invalid ledger file is dropped");

    // Garbage in any column is the same verdict.
    for body in [
        "not,a,ledger,row,at,all,not,even,close,no,really\n",
        "1,2026-10-18T10:00:00Z,2,2026-10-18T11:00:00Z,x,y,z,w,v,u,p\n",
        "1,2026-10-18T10:00:00Z,2,2026-10-18T11:00:00Z,1,1,0,0,10,2\n", // short
        "1,ts,2,ts,1,1,0,0,10,2,parts/x.md,extra\n",                    // long
        "two,ts,4,ts,1,1,0,0,10,2,parts/x.md\n",                        // unparseable id
    ] {
        std::fs::write(&path, body).unwrap();
        assert!(
            summary_cache::read_ledger(&path).is_empty(),
            "{body:?} must invalidate the ledger"
        );
        assert!(!path.exists(), "{body:?} must drop the ledger file");
    }

    // A valid 11-column row still parses.
    std::fs::write(
        &path,
        "1,2026-10-18T10:00:00Z,2,2026-10-18T11:00:00Z,1,1,0,0,22,5,parts/p.md\n",
    )
    .unwrap();
    let blocks = summary_cache::read_ledger(&path);
    assert_eq!(blocks.len(), 1, "a valid row survives the parser");
    assert_eq!(blocks[0].from_id, 1);
    assert_eq!(blocks[0].to_id, 2);
}

#[test]
fn summary_cache_status_shows_the_blocks_and_the_unsummarised_tail() {
    let (_dir, adapter) = fixture("status", &four_messages());
    let root = adapter.shadow_index_root();
    let messages = adapter.read_session_mmap("mock").unwrap();

    // Nothing cached: the tail starts at id 1 and covers everything.
    let status = summary_cache::status(&root, "mock", &messages);
    assert_eq!(status.session_id, "mock");
    assert!(status.blocks.is_empty(), "nothing cached yet");
    let tail = status.tail.expect("an uncovered session has a tail");
    assert_eq!(tail.from_id, 1);
    assert_eq!(tail.count, 4);
    assert_eq!(tail.user, 2);
    assert_eq!(tail.assistant, 2);
    assert_eq!(tail.from_ts.as_deref(), Some("2026-10-18T10:00:00Z"));

    // Cover ids 1..=2: the tail moves to id 3 and shrinks.
    summary_cache::append_block(&root, "mock", 1, &messages[..2], "first summary").unwrap();
    let status = summary_cache::status(&root, "mock", &messages);
    assert_eq!(status.blocks.len(), 1);
    assert_eq!(status.blocks[0].to_id, 2);
    let tail = status.tail.expect("ids 3..=4 are uncovered");
    assert_eq!(tail.from_id, 3);
    assert_eq!(tail.count, 2);
    assert_eq!(tail.from_ts.as_deref(), Some("2026-10-18T12:00:00Z"));

    // Everything covered: the tail is null.
    summary_cache::append_block(&root, "mock", 3, &messages[2..], "delta summary").unwrap();
    let status = summary_cache::status(&root, "mock", &messages);
    assert_eq!(status.blocks.len(), 2);
    assert!(status.tail.is_none(), "a fully covered session has no tail");

    // The status JSON shape: blocks carry all eleven fields.
    let json = serde_json::to_value(&status).unwrap();
    let block = &json["blocks"][0];
    for field in [
        "from_id",
        "from_ts",
        "to_id",
        "to_ts",
        "user",
        "assistant",
        "tool",
        "thinking",
        "chars",
        "tokens_est",
        "part_file",
    ] {
        assert!(
            block.get(field).is_some(),
            "the block JSON carries `{field}`: {json}"
        );
    }
    assert_eq!(json["tail"], serde_json::Value::Null);
}

#[tokio::test]
async fn summarise_from_with_the_wrong_id_is_an_error_naming_the_expected_id() {
    let (_dir, adapter) = fixture("guard", &four_messages());
    let messages = adapter.read_session_mmap("mock").unwrap();
    let root = adapter.shadow_index_root();

    // No blocks: the only legal from_id is 1.
    let err = summary_cache::summarise_from_id(&adapter, "mock", 3, None)
        .await
        .expect_err("from_id 3 with no blocks must be refused");
    assert!(
        err.contains("1") && err.contains("from_id"),
        "the error must name the expected id in the house form: {err}"
    );

    // After a block covering ids 1..=2: the only legal from_id is 3.
    summary_cache::append_block(&root, "mock", 1, &messages[..2], "first summary").unwrap();
    for wrong in [1, 2, 4, 9] {
        let err = summary_cache::summarise_from_id(&adapter, "mock", wrong, None)
            .await
            .expect_err(&format!(
                "from_id {wrong} is behind or ahead of the block edge"
            ));
        assert!(
            err.contains("3"),
            "the error must name the expected id 3: {err}"
        );
    }
    // The failed calls stored nothing.
    let blocks = summary_cache::read_ledger(&summary_cache::ledger_path(&root, "mock"));
    assert_eq!(blocks.len(), 1, "a refused call writes no block");
}

#[tokio::test]
async fn summarise_from_on_an_already_covered_tail_serves_the_cache_without_an_llm_call() {
    let (_dir, adapter) = fixture(
        "covered",
        &[
            msg("user", "early work", "2026-10-18T10:00:00Z"),
            msg("assistant", "done", "2026-10-18T12:00:00Z"),
        ],
    );
    let messages = adapter.read_session_mmap("mock").unwrap();
    let root = adapter.shadow_index_root();
    summary_cache::append_block(
        &root,
        "mock",
        1,
        &messages,
        "CACHED-TEXT everything, summarised",
    )
    .unwrap();

    // expected = 3, the session ends at id 2: nothing to summarise, the
    // composition is served, no provider is ever constructed.
    let out = summary_cache::summarise_from_id(&adapter, "mock", 3, None)
        .await
        .expect("a covered tail is served from cache");
    assert!(
        out.contains("CACHED-TEXT everything, summarised"),
        "the cached composition is served: {out}"
    );
}

#[test]
fn cache_delete_latest_rolls_the_newest_block_back() {
    let (_dir, adapter) = fixture("rollback", &four_messages());
    let root = adapter.shadow_index_root();
    let messages = adapter.read_session_mmap("mock").unwrap();
    let first =
        summary_cache::append_block(&root, "mock", 1, &messages[..2], "first summary").unwrap();
    let second =
        summary_cache::append_block(&root, "mock", 3, &messages[2..], "delta summary").unwrap();
    assert!(second.part_file.is_file());

    // Roll back one block: the newest ledger row and its part file are gone,
    // the older block survives.
    summary_cache::delete_latest(&root, "mock").unwrap();
    let blocks = summary_cache::read_ledger(&summary_cache::ledger_path(&root, "mock"));
    assert_eq!(blocks.len(), 1, "the newest row is removed from the ledger");
    assert_eq!(blocks[0].to_id, 2, "the older row survives");
    assert!(
        !second.part_file.exists(),
        "the newest part file is deleted"
    );
    assert!(first.part_file.is_file(), "the older part file survives");

    // Roll back the last block: the ledger file goes with it.
    summary_cache::delete_latest(&root, "mock").unwrap();
    let path = summary_cache::ledger_path(&root, "mock");
    assert!(
        summary_cache::read_ledger(&path).is_empty(),
        "no blocks remain"
    );
    assert!(
        !path.exists(),
        "an emptied ledger is dropped, not left empty"
    );

    // And the rolled-back period is summarisable again from id 1.
    let status = summary_cache::status(&root, "mock", &messages);
    let tail = status
        .tail
        .expect("everything is uncovered after both rollbacks");
    assert_eq!(tail.from_id, 1);
    assert_eq!(tail.count, 4);
}

#[test]
fn the_list_marker_reports_what_the_cache_covers_through() {
    let (_dir, adapter) = fixture("marker", &four_messages());
    let root = adapter.shadow_index_root();

    assert_eq!(
        summary_cache::through_marker(&root, "mock"),
        None,
        "no blocks, no marker"
    );

    let messages = adapter.read_session_mmap("mock").unwrap();
    summary_cache::append_block(&root, "mock", 1, &messages[..2], "first summary").unwrap();
    let marker =
        summary_cache::through_marker(&root, "mock").expect("a cached session has a marker");
    assert_eq!(marker.through_id, 2);
    assert_eq!(marker.through_ts, "2026-10-18T11:00:00Z");

    summary_cache::append_block(&root, "mock", 3, &messages[2..], "delta summary").unwrap();
    let marker = summary_cache::through_marker(&root, "mock").unwrap();
    assert_eq!(marker.through_id, 4);
    assert_eq!(marker.through_ts, "2026-10-18T13:00:00Z");
    let marker_json = serde_json::to_value(&marker).unwrap();
    assert_eq!(marker_json["through_id"], 4);
    assert_eq!(marker_json["through_ts"], "2026-10-18T13:00:00Z");
}
