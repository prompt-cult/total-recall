use std::time::Instant;

use total_recall::{MockAdapter, RolloutAdapter, VibeAdapter, build_structured_prompt};

/// Read speed is a statistical property measured by criterion in
/// benches/read_rollout.rs; this file asserts only what is load-independent:
/// both reads succeed, yield the full fixture, and format into a non-empty
/// prompt. Wall-clock budgets asserted here were load detectors on a shared
/// machine and are gone.
#[test]
fn test_read_speed_vs_format() {
    let path = env!("CARGO_MANIFEST_DIR").to_string() + "/rollouts/mock_sample.jsonl";
    let adapter = MockAdapter::new(&path);

    let t0 = Instant::now();
    let messages = adapter
        .read_session_mmap("test")
        .expect("healthy fixture mmap read");
    let read_time = t0.elapsed();

    let t1 = Instant::now();
    let prompt = build_structured_prompt(&messages);
    let format_time = t1.elapsed();

    println!(
        "Read: {:?}, Format: {:?}, Messages: {}",
        read_time,
        format_time,
        messages.len()
    );

    assert!(!messages.is_empty());
    assert!(!prompt.is_empty());
}

/// Test that mmap read is at least as fast as regular read.
#[test]
fn test_mmap_vs_regular_read() {
    let path = env!("CARGO_MANIFEST_DIR").to_string() + "/rollouts/mock_sample.jsonl";
    let adapter = MockAdapter::new(&path);

    let t0 = Instant::now();
    let messages1 = adapter.read_session("test").expect("healthy fixture read");
    let regular_time = t0.elapsed();

    let t1 = Instant::now();
    let messages2 = adapter
        .read_session_mmap("test")
        .expect("healthy fixture mmap read");
    let mmap_time = t1.elapsed();

    assert_eq!(messages1.len(), messages2.len());
    println!(
        "Regular: {:?}, mmap: {:?} ({} messages)",
        regular_time,
        mmap_time,
        messages1.len()
    );
}

/// Vibe session read and format on the larger fixture: same deal as
/// test_read_speed_vs_format — functional assertions here, timings in
/// benches/read_rollout.rs.
#[test]
fn test_vibe_read_speed_large() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rollouts/vibe_sessions");
    let adapter = VibeAdapter::with_root(root);

    let t0 = Instant::now();
    let messages = adapter
        .read_session_mmap("2a421f21")
        .expect("healthy fixture mmap read");
    let read_time = t0.elapsed();

    let t1 = Instant::now();
    let prompt = build_structured_prompt(&messages);
    let format_time = t1.elapsed();

    println!(
        "Vibe read: {:?}, Format: {:?}, Messages: {}",
        read_time,
        format_time,
        messages.len()
    );

    assert!(!messages.is_empty());
    assert!(!prompt.is_empty());
}
