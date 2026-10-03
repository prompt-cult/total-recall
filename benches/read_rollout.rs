use criterion::{Criterion, black_box, criterion_group, criterion_main};
use total_recall::{
    MAX_GOALS_BYTES, MAX_STATE_BYTES, MockAdapter, RolloutAdapter, RolloutMessage, VibeAdapter,
    build_goals_prompt_bounded, build_state_prompt_bounded, build_structured_prompt,
};

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

criterion_group!(
    benches,
    bench_read_rollout,
    bench_committed_fixtures,
    bench_bounded_recall_prompts
);
criterion_main!(benches);
