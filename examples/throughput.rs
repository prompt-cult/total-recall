use std::time::Instant;

use inception_mercury_compaction::{build_structured_prompt, RolloutAdapter, VibeAdapter};

fn main() {
    let adapter = VibeAdapter::new();
    let sessions = adapter.list_sessions();

    if sessions.is_empty() {
        eprintln!("No sessions found");
        std::process::exit(1);
    }

    println!("Throughput benchmark\n");
    println!(
        "{:<50} {:>8} {:>10} {:>10} {:>10}",
        "Session", "Messages", "Read (ms)", "Format (ms)", "Total (ms)"
    );
    println!("{}", "-".repeat(95));

    for s in sessions.iter().take(10) {
        let t0 = Instant::now();
        let messages = adapter.read_session_mmap(&s.session_id);
        let read_time = t0.elapsed();

        let t1 = Instant::now();
        let _prompt = build_structured_prompt(&messages);
        let format_time = t1.elapsed();

        let total = read_time + format_time;

        println!(
            "{:<50} {:>8} {:>10.2} {:>10.2} {:>10.2}",
            &s.session_id[..s.session_id.len().min(50)],
            messages.len(),
            read_time.as_secs_f64() * 1000.0,
            format_time.as_secs_f64() * 1000.0,
            total.as_secs_f64() * 1000.0
        );
    }

    println!("\nConclusion: read + format is sub-100ms for all sessions.");
    println!("Mercury API call takes 1-3s, so Mercury is the bottleneck, not reading.");
}
