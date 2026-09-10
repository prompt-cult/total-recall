use criterion::{black_box, criterion_group, criterion_main, Criterion};
use inception_mercury_compaction::{build_structured_prompt, RolloutAdapter, VibeAdapter};

fn bench_read_rollout(c: &mut Criterion) {
    let adapter = VibeAdapter::new();
    let sessions = adapter.list_sessions();
    if sessions.is_empty() {
        return;
    }
    let session_id = sessions[0].session_id.clone();

    c.bench_function("read_session_mmap", |b| {
        b.iter(|| {
            let messages = adapter.read_session_mmap(black_box(&session_id));
            black_box(messages);
        })
    });

    c.bench_function("read_session", |b| {
        b.iter(|| {
            let messages = adapter.read_session(black_box(&session_id));
            black_box(messages);
        })
    });

    let messages = adapter.read_session_mmap(&session_id);
    c.bench_function("build_structured_prompt", |b| {
        b.iter(|| {
            let prompt = build_structured_prompt(black_box(&messages));
            black_box(prompt);
        })
    });
}

criterion_group!(benches, bench_read_rollout);
criterion_main!(benches);
