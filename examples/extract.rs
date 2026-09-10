use inception_mercury_compaction::{RolloutAdapter, VibeAdapter};

fn main() {
    let adapter = VibeAdapter::new();
    let sessions = adapter.list_sessions();

    if sessions.is_empty() {
        eprintln!("No sessions found");
        std::process::exit(1);
    }

    let session_id = &sessions[0].session_id;
    println!("Extracting from session: {}\n", session_id);

    let messages = adapter.read_session_mmap(session_id);
    println!("Total messages: {}\n", messages.len());

    for msg in &messages {
        println!("[{}]", msg.role.to_uppercase());
        if msg.injected {
            println!("  (injected)");
        }
        if !msg.content.is_empty() {
            let preview = &msg.content[..msg.content.len().min(200)];
            println!("  {}", preview);
            if msg.content.len() > 200 {
                println!("  ... ({} chars total)", msg.content.len());
            }
        }
        for tc in &msg.tool_calls_summary {
            println!("  -> {}", tc);
        }
        println!();
    }
}
