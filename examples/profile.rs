use inception_mercury_compaction::{RolloutAdapter, VibeAdapter};

fn main() {
    let adapter = VibeAdapter::new();
    let sessions = adapter.list_sessions();

    println!("Found {} sessions\n", sessions.len());
    println!(
        "{:<50} {:<40} {:>6} {:>5} {:>5} {:>5} {}",
        "Session ID", "Title", "Lines", "User", "Asst", "Tool", "Compaction"
    );
    println!("{}", "-".repeat(130));

    for s in sessions.iter().take(20) {
        println!(
            "{:<50} {:<40} {:>6} {:>5} {:>5} {:>5} {}",
            &s.session_id[..s.session_id.len().min(50)],
            &s.title[..s.title.len().min(40)],
            s.line_count,
            s.user_count,
            s.assistant_count,
            s.tool_count,
            s.has_compaction
        );
    }

    if !sessions.is_empty() {
        let session_id = &sessions[0].session_id;
        println!("\nProfiling most recent session: {}", session_id);
        let profile = adapter.profile_session(session_id);

        println!("  File size: {} bytes", profile.file_size);
        println!("  Line count: {}", profile.line_count);
        if let Some(ts) = &profile.first_ts {
            println!("  First TS: {}", ts);
        }
        if let Some(ts) = &profile.last_ts {
            println!("  Last TS: {}", ts);
        }
        println!("\n  Role counts:");
        for (role, count) in &profile.role_counts {
            println!("    {}: {}", role, count);
        }
        println!("\n  Interesting events ({}):", profile.interesting_events.len());
        for event in profile.interesting_events.iter().take(20) {
            println!(
                "    line {} (gap {}): {:?} — {}",
                event.line_number, event.gap_lines, event.event_type, event.summary
            );
        }
    }
}
