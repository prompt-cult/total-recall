use criterion::{Criterion, black_box, criterion_group, criterion_main};
use total_recall::{MockAdapter, RolloutAdapter, VibeAdapter, build_structured_prompt};

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

criterion_group!(benches, bench_read_rollout, bench_committed_fixtures);
criterion_main!(benches);
