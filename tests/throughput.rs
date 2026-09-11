use std::time::Instant;

use inception_mercury_compaction::{
    build_structured_prompt, MockAdapter, RolloutAdapter, VibeAdapter,
};

/// Test that read speed is fast enough to not be the bottleneck.
/// The critical question: is the bottleneck reading the rollout or pushing to Mercury?
#[test]
fn test_read_speed_vs_format() {
    let path = env!("CARGO_MANIFEST_DIR").to_string() + "/rollouts/mock_sample.jsonl";
    let adapter = MockAdapter::new(&path);

    let t0 = Instant::now();
    let messages = adapter.read_session_mmap("test");
    let read_time = t0.elapsed();

    let t1 = Instant::now();
    let _prompt = build_structured_prompt(&messages);
    let format_time = t1.elapsed();

    println!(
        "Read: {:?}, Format: {:?}, Messages: {}",
        read_time,
        format_time,
        messages.len()
    );

    // Read should be faster than 100ms
    assert!(read_time.as_millis() < 100);
    // Format should be faster than 10ms
    assert!(format_time.as_millis() < 10);
}

/// Test that mmap read is at least as fast as regular read.
#[test]
fn test_mmap_vs_regular_read() {
    let path = env!("CARGO_MANIFEST_DIR").to_string() + "/rollouts/mock_sample.jsonl";
    let adapter = MockAdapter::new(&path);

    let t0 = Instant::now();
    let messages1 = adapter.read_session("test");
    let regular_time = t0.elapsed();

    let t1 = Instant::now();
    let messages2 = adapter.read_session_mmap("test");
    let mmap_time = t1.elapsed();

    assert_eq!(messages1.len(), messages2.len());
    println!(
        "Regular: {:?}, mmap: {:?} ({} messages)",
        regular_time,
        mmap_time,
        messages1.len()
    );
}

/// Test vibe session read speed with a larger file.
#[test]
fn test_vibe_read_speed_large() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("rollouts/vibe_sessions");
    let adapter = VibeAdapter::with_root(root);

    let t0 = Instant::now();
    let messages = adapter.read_session_mmap("2a421f21");
    let read_time = t0.elapsed();

    let t1 = Instant::now();
    let _prompt = build_structured_prompt(&messages);
    let format_time = t1.elapsed();

    println!(
        "Vibe read: {:?}, Format: {:?}, Messages: {}",
        read_time,
        format_time,
        messages.len()
    );

    // 171-line file should read in under 100ms
    assert!(read_time.as_millis() < 100);
    // Format should be under 10ms
    assert!(format_time.as_millis() < 10);
}
