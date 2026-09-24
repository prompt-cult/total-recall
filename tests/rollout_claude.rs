use std::path::PathBuf;
use total_recall::RolloutAdapter;
use total_recall::rollout::claude::ClaudeAdapter;

fn test_data_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rollouts")
}

#[test]
fn test_claude_adapter_name() {
    let adapter = ClaudeAdapter::with_root(test_data_root());
    assert_eq!(adapter.name(), "claude");
}

#[test]
fn test_claude_read_session() {
    let adapter = ClaudeAdapter::with_root(test_data_root());
    let messages = adapter.read_session("claude_small");

    assert!(
        !messages.is_empty(),
        "Should read messages from claude sample"
    );
    for msg in &messages {
        assert!(!msg.role.is_empty(), "Role should not be empty");
    }
}

#[test]
fn test_claude_read_session_mmap() {
    let adapter = ClaudeAdapter::with_root(test_data_root());
    let messages = adapter.read_session_mmap("claude_small");

    assert!(!messages.is_empty());
}

#[test]
fn test_claude_list_sessions() {
    let adapter = ClaudeAdapter::with_root(test_data_root());
    let sessions = adapter.list_sessions();

    assert!(
        !sessions.is_empty(),
        "Should find at least one claude session"
    );
    let claude_session = sessions
        .iter()
        .find(|s| s.session_id.contains("claude"))
        .expect("Should find claude_small.jsonl");
    assert!(claude_session.line_count > 0);
}

#[test]
fn test_claude_profile_session() {
    let adapter = ClaudeAdapter::with_root(test_data_root());
    let profile = adapter.profile_session("claude_small");

    assert!(profile.line_count > 0);
    assert!(profile.file_size > 0);
    assert!(
        profile.role_counts.contains_key("user") || profile.role_counts.contains_key("assistant"),
        "Should have user or assistant roles"
    );
}

#[test]
fn test_claude_extract_user_messages() {
    let adapter = ClaudeAdapter::with_root(test_data_root());
    let messages = adapter.extract_user_messages("claude_small");

    assert!(!messages.is_empty(), "Should extract user messages");
}

/// Real Claude Code layout: <root>/<project-dir>/<session-uuid>.jsonl, with
/// ai-title lines, ISO timestamps, thinking blocks, and subagents/ dirs that
/// are NOT sessions.
#[test]
fn test_claude_real_project_layout() {
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("claude_real_layout");
    let _ = std::fs::remove_dir_all(&tmp);
    let proj = tmp.join("-Users-someone-code-myrepo");
    std::fs::create_dir_all(proj.join("subagents")).unwrap();

    let sid = "12345678-aaaa-bbbb-cccc-dddddddddddd";
    let lines = [
        r#"{"type":"ai-title","aiTitle":"Fix the flux capacitor","sessionId":"12345678-aaaa-bbbb-cccc-dddddddddddd"}"#,
        r#"{"type":"user","sessionId":"12345678-aaaa-bbbb-cccc-dddddddddddd","timestamp":"2026-09-20T10:00:00.000Z","message":{"role":"user","content":"what breaks without the capacitor"}}"#,
        r#"{"type":"assistant","sessionId":"12345678-aaaa-bbbb-cccc-dddddddddddd","timestamp":"2026-09-20T10:01:00.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"the capacitor stores flux; without it time reverses"},{"type":"text","text":"Short answer: time."},{"type":"tool_use","name":"read_file","input":{"path":"cap.py"}}]}}"#,
        r#"{"type":"user","sessionId":"12345678-aaaa-bbbb-cccc-dddddddddddd","timestamp":"2026-09-20T10:02:00.000Z","message":{"role":"user","content":[{"type":"tool_result","content":[{"type":"text","text":"capacitor missing"}]}]}}"#,
        "\n",
        r#"{"type":"other","sessionId":"12345678-aaaa-bbbb-cccc-dddddddddddd"}"#,
    ];
    std::fs::write(proj.join(format!("{sid}.jsonl")), lines.join("\n") + "\n").unwrap();
    std::fs::write(
        proj.join("subagents").join("agent-99.jsonl"),
        r#"{"type":"user","message":{"role":"user","content":"subagent noise"}}"#,
    )
    .unwrap();

    let adapter = ClaudeAdapter::with_root(&tmp);

    let sessions = adapter.list_sessions();
    assert_eq!(
        sessions.len(),
        1,
        "subagents/ must not count as a session: {:?}",
        sessions
    );
    let s = &sessions[0];
    assert_eq!(
        s.session_id, sid,
        "session id must be the uuid stem, no .jsonl"
    );
    assert_eq!(s.title, "Fix the flux capacitor");
    assert_eq!(s.directory.as_deref(), Some("-Users-someone-code-myrepo"));
    assert_eq!(s.start_time, "2026-09-20T10:00:00.000Z");
    assert_eq!(s.end_time, "2026-09-20T10:02:00.000Z");
    assert_eq!(s.user_count, 2, "user message + tool_result line");
    assert_eq!(s.assistant_count, 1);

    let messages = adapter.read_session(sid);
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].content, "what breaks without the capacitor");
    let asst = &messages[1];
    assert_eq!(asst.content, "Short answer: time.");
    assert_eq!(
        asst.thinking.as_deref(),
        Some("the capacitor stores flux; without it time reverses"),
        "thinking blocks must surface in the message stream"
    );
    assert!(
        asst.tool_calls_summary
            .iter()
            .any(|t| t.contains("read_file")),
        "tool_use blocks must summarize: {:?}",
        asst.tool_calls_summary
    );
    assert!(
        messages[2].content.contains("capacitor missing"),
        "array-form tool_result content must be extracted: {:?}",
        messages[2].content
    );

    let profile = adapter.profile_session(sid);
    assert_eq!(profile.session_id, sid);
    assert!(profile.line_count >= 5);
    assert_eq!(
        profile.first_ts.as_deref(),
        Some("2026-09-20T10:00:00.000Z")
    );
    assert_eq!(profile.last_ts.as_deref(), Some("2026-09-20T10:02:00.000Z"));
}
