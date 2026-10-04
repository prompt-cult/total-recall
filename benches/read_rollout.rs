// The suite's one scratch-directory helper (tests/common/scratch.rs), so
// the bench fixtures follow the same per-call unique, self-cleaning
// discipline as the tests. The module carries its own hygiene #[test];
// under the custom bench harness it is inert code.
#[path = "../tests/common/scratch.rs"]
#[allow(dead_code)]
mod scratch;

use std::path::{Path, PathBuf};

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use rusqlite::Connection;
use total_recall::{
    MAX_GOALS_BYTES, MAX_STATE_BYTES, MockAdapter, OpenCodeAdapter, RolloutAdapter, RolloutMessage,
    VibeAdapter, build_goals_prompt_bounded, build_state_prompt_bounded, build_structured_prompt,
};

use scratch::scratch;

fn bench_read_rollout(c: &mut Criterion) {
    let adapter = VibeAdapter::new();
    let sessions = adapter.list_sessions();
    if sessions.is_empty() {
        return;
    }
    let session_id = sessions[0].session_id.clone();

    c.bench_function("read_session_mmap", |b| {
        b.iter(|| {
            let messages = adapter
                .read_session_mmap(black_box(&session_id))
                .expect("healthy fixture mmap read");
            black_box(messages);
        })
    });

    c.bench_function("read_session", |b| {
        b.iter(|| {
            let messages = adapter
                .read_session(black_box(&session_id))
                .expect("healthy fixture read");
            black_box(messages);
        })
    });

    let messages = adapter
        .read_session_mmap(&session_id)
        .expect("healthy fixture mmap read");
    c.bench_function("build_structured_prompt", |b| {
        b.iter(|| {
            let prompt = build_structured_prompt(black_box(&messages));
            black_box(prompt);
        })
    });
}

/// Budgets for the committed fixtures: the read and format timings that
/// tests/throughput.rs used to assert as wall-clock bounds. Criterion's
/// statistics record them without flaking on a loaded machine.
fn bench_committed_fixtures(c: &mut Criterion) {
    let manifest = env!("CARGO_MANIFEST_DIR");

    let mock = MockAdapter::new(format!("{manifest}/rollouts/mock_sample.jsonl"));
    let mock_messages = mock
        .read_session_mmap("test")
        .expect("healthy fixture mmap read");
    assert!(!mock_messages.is_empty());
    c.bench_function("fixture_mock_read_mmap", |b| {
        b.iter(|| {
            let messages = mock
                .read_session_mmap(black_box("test"))
                .expect("healthy fixture mmap read");
            black_box(messages);
        })
    });
    c.bench_function("fixture_mock_build_structured_prompt", |b| {
        b.iter(|| {
            let prompt = build_structured_prompt(black_box(&mock_messages));
            black_box(prompt);
        })
    });

    let vibe = VibeAdapter::with_root(format!("{manifest}/rollouts/vibe_sessions"));
    let vibe_messages = vibe
        .read_session_mmap("2a421f21")
        .expect("healthy fixture mmap read");
    assert!(!vibe_messages.is_empty());
    c.bench_function("fixture_vibe_read_mmap", |b| {
        b.iter(|| {
            let messages = vibe
                .read_session_mmap(black_box("2a421f21"))
                .expect("healthy fixture mmap read");
            black_box(messages);
        })
    });
    c.bench_function("fixture_vibe_build_structured_prompt", |b| {
        b.iter(|| {
            let prompt = build_structured_prompt(black_box(&vibe_messages));
            black_box(prompt);
        })
    });
}

/// The recall prompts, over a synthetic session several megabytes of text
/// long. Recorded rather than asserted: what matters is the shape of the curve
/// (flat as the session grows, because the payload is byte-bounded and only the
/// newest messages are ever rendered), and criterion's statistics show that
/// without a wall-clock assertion flaking on a loaded machine.
fn bench_bounded_recall_prompts(c: &mut Criterion) {
    let messages: Vec<RolloutMessage> = (0..20_000)
        .map(|i| RolloutMessage {
            role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
            content: format!("msg{i:06} {}", "lorem ipsum dolor sit amet ".repeat(60)),
            thinking: None,
            tool_calls_summary: Vec::new(),
            timestamp: None,
            injected: false,
        })
        .collect();
    let users: Vec<String> = (0..20_000)
        .map(|i| format!("goal{i:06} {}", "lorem ipsum dolor sit amet ".repeat(60)))
        .collect();
    for count in [1_000usize, 20_000] {
        c.bench_function(&format!("build_recall_prompts_{count}_messages"), |b| {
            b.iter(|| {
                let state = build_state_prompt_bounded(&messages[..count], MAX_STATE_BYTES);
                let goals = build_goals_prompt_bounded(&users[..count], MAX_GOALS_BYTES);
                black_box((state.bytes, goals.bytes))
            })
        });
    }
}

// --- The #21 acceptance: the scoped listing is flat in store size ---------

/// Fixed in-window population for the scoped-listing bench: 50 sessions,
/// 8 parts of 4 KB each — constant across the fixture sizes, so the only
/// thing that varies is how much of the store sits OUTSIDE the window.
const BENCH_IN_WINDOW: usize = 50;
const BENCH_PARTS_PER_SESSION: usize = 8;
const BENCH_PART_BYTES: usize = 4096;

/// An opencode fixture store with the indexes a real `opencode.db` carries
/// (`message(session_id, time_created, id)` and `part(session_id)`), holding
/// `out_of_window` sessions updated a month ago and `in_window` updated now,
/// each with one message and `parts_per_session` parts of `part_bytes`
/// payload, so the aggregates are real work.
fn build_scoped_store(root: &Path, out_of_window: usize, in_window: usize) -> PathBuf {
    let db = root.join("fixture.db");
    let conn = Connection::open(&db).expect("fixture db");
    conn.execute_batch(
        "PRAGMA journal_mode=OFF;
         PRAGMA synchronous=OFF;
         CREATE TABLE session (
            id text PRIMARY KEY, parent_id text, directory text, title text,
            time_created integer NOT NULL, time_updated integer NOT NULL);
         CREATE TABLE message (
            id text PRIMARY KEY, session_id text NOT NULL,
            time_created integer NOT NULL, time_updated integer NOT NULL,
            data text NOT NULL);
         CREATE TABLE part (
            id text PRIMARY KEY, message_id text NOT NULL, session_id text NOT NULL,
            time_created integer NOT NULL, time_updated integer NOT NULL,
            data text NOT NULL);",
    )
    .expect("fixture schema");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let month_ago = now - 30 * 24 * 3_600_000;
    let tx = conn.unchecked_transaction().expect("fixture tx");
    {
        let mut ins_session = tx
            .prepare(
                "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
                      VALUES (?1, NULL, ?2, ?3, ?4, ?4)",
            )
            .expect("prepare session");
        let mut ins_message = tx
            .prepare(
                "INSERT INTO message (id, session_id, time_created, time_updated, data)
                      VALUES (?1, ?2, ?3, ?3, ?4)",
            )
            .expect("prepare message");
        let mut ins_part = tx
            .prepare(
                "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
                      VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            )
            .expect("prepare part");
        for i in 0..out_of_window + in_window {
            let in_win = i >= out_of_window;
            let sid = format!("ses_bench{i:05}00000000000000000000aa");
            let updated = if in_win { now - 1_000 } else { month_ago };
            ins_session
                .execute(rusqlite::params![
                    &sid,
                    "/Users/dev/bench",
                    &format!("bench session {i}"),
                    updated
                ])
                .expect("insert session");
            ins_message
                .execute(rusqlite::params![
                    format!("bmsg{i:05}"),
                    &sid,
                    updated,
                    "{\"role\":\"user\",\"time\":{\"created\":1}}"
                ])
                .expect("insert message");
            for p in 0..BENCH_PARTS_PER_SESSION {
                let text = format!(
                    "bench rollout body {i}/{p} {}",
                    "x".repeat(BENCH_PART_BYTES - 32)
                );
                ins_part
                    .execute(rusqlite::params![
                        format!("bpart{i:05}_{p}"),
                        format!("bmsg{i:05}"),
                        &sid,
                        updated,
                        serde_json::json!({"type": "text", "text": text}).to_string()
                    ])
                    .expect("insert part");
            }
        }
    }
    tx.commit().expect("commit fixture");
    conn.execute_batch(
        "CREATE INDEX message_session_time_created_id_idx
           ON message (session_id, time_created, id);
         CREATE INDEX part_session_idx ON part (session_id);",
    )
    .expect("fixture indexes");
    drop(conn);
    db
}

/// One fixture store alive for the duration of its benchmark: the scratch
/// root is cleared when the holder drops, after the adapter is gone.
struct ScopedStore {
    adapter: OpenCodeAdapter,
    _root: scratch::ScratchRoot,
}

/// The #21 acceptance measurement: the scoped listing over fixture stores of
/// 1,000 / 3,000 / 10,000 out-of-window sessions plus a fixed 50 in-window.
/// The scoped listing time must be FLAT in out-of-window store size: the
/// window, the directory substring and the row cap are pushed into SQL, so
/// out-of-window and out-of-cap rows cost nothing in aggregates — the full
/// `list_sessions` scan behind the same stores grows linearly with the store.
fn bench_scoped_listing_flat_in_store_size(c: &mut Criterion) {
    for &out_of_window in &[1_000usize, 3_000, 10_000] {
        let root = scratch("bench_scoped_listing");
        let db = build_scoped_store(&root, out_of_window, BENCH_IN_WINDOW);
        let db_mb = std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0) as f64 / 1_000_000.0;
        let store = ScopedStore {
            adapter: OpenCodeAdapter::with_root(&db),
            _root: root,
        };
        // The measurement's honesty guard: every timed call does the same
        // real work — the whole in-window population listed with its
        // aggregates — so flat means flat work, not an empty answer.
        let listing = store.adapter.list_sessions_scoped(24, None);
        assert_eq!(listing.window_count, BENCH_IN_WINDOW);
        assert_eq!(listing.sessions.len(), BENCH_IN_WINDOW);
        for s in &listing.sessions {
            assert_eq!(s.user_count, 1, "{}", s.session_id);
            assert_eq!(
                s.line_count, BENCH_PARTS_PER_SESSION as u64,
                "{}",
                s.session_id
            );
        }
        println!(
            "scoped-listing fixture: {out_of_window} out-of-window + {BENCH_IN_WINDOW} in-window \
             sessions, {db_mb:.1} MB store",
        );
        c.bench_function(
            &format!("scoped_listing_24h_{out_of_window}_out_of_window_sessions"),
            |b| {
                b.iter(|| {
                    let listing = store
                        .adapter
                        .list_sessions_scoped(black_box(24), black_box(None));
                    black_box(&listing.sessions);
                    black_box(listing.window_count);
                })
            },
        );
    }
}

criterion_group!(
    benches,
    bench_read_rollout,
    bench_committed_fixtures,
    bench_bounded_recall_prompts,
    bench_scoped_listing_flat_in_store_size
);
criterion_main!(benches);
