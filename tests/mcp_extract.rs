//! #8: extract_messages / extract_user_messages return bounded envelopes that
//! never exceed the byte ceiling, with an explicit truncation notice and
//! next_offset paging — asserted through the real MCP stdio server.

mod common;

use std::io::{BufRead, Write};

use common::scratch::{child_cwd, scratch};

/// Build a vibe session dir with `n` user messages of ~`msg_len` chars each.
fn make_big_session(root: &std::path::Path, name: &str, n: usize, msg_len: usize) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let body = "x".repeat(msg_len);
    let mut out = String::new();
    for i in 0..n {
        out.push_str(&format!(
            "{{\"role\":\"user\",\"content\":\"msg{i} {body}\",\"timestamp\":\"2026-09-15T09:59:55Z\",\"injected\":false}}\n"
        ));
    }
    std::fs::write(dir.join("messages.jsonl"), out).unwrap();
    std::fs::write(
        dir.join("meta.json"),
        format!(
            "{{\"session_id\":\"uuid-{name}\",\"start_time\":\"2026-09-15T09:59:55+00:00\",\"end_time\":\"2026-09-15T10:00:00+00:00\",\"title\":\"big\",\"total_messages\":{n}}}"
        ),
    )
    .unwrap();
}

/// An MCP child, plus the scratch working directory outside the repo tree it
/// was spawned in. The directory is returned because the child outlives this
/// function: only the caller, which owns the `Child`, can safely clear it.
/// `dotenvy` walks up parent directories, so the CWD must sit where no `.env`
/// is above it, and the vendor keys are removed as well.
fn spawn_mcp(
    root: &std::path::Path,
) -> (
    std::process::Child,
    std::io::BufReader<std::process::ChildStdout>,
    std::process::ChildStdin,
    std::path::PathBuf,
) {
    let cwd = child_cwd("mcp");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_total-recall"))
        .args(["--harness", "vibe", "mcp"])
        .current_dir(&cwd)
        .env("TOTAL_RECALL_VIBE_ROOT", root)
        .env_remove("INCEPTION_API_KEY")
        .env_remove("MISTRAL_API_KEY")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn mcp");
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    (child, std::io::BufReader::new(stdout), stdin, cwd)
}

/// Clear a child's scratch working directory, once that child has been killed.
fn reap_mcp(child: &mut std::process::Child, cwd: &std::path::Path) {
    child.kill().ok();
    child.wait().ok();
    let _ = std::fs::remove_dir_all(cwd);
}

fn send(stdin: &mut std::process::ChildStdin, v: &serde_json::Value) {
    writeln!(stdin, "{}", serde_json::to_string(v).unwrap()).unwrap();
    stdin.flush().unwrap();
}

fn read_id(
    reader: &mut std::io::BufReader<std::process::ChildStdout>,
    id: i64,
) -> serde_json::Value {
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).unwrap();
        assert!(n > 0, "server closed stdout before id {id}");
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim())
            && v.get("id").and_then(|i| i.as_i64()) == Some(id)
        {
            return v;
        }
    }
}

fn init(
    reader: &mut std::io::BufReader<std::process::ChildStdout>,
    stdin: &mut std::process::ChildStdin,
) {
    send(
        stdin,
        &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}),
    );
    assert!(read_id(reader, 1).get("result").is_some());
    send(
        stdin,
        &serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );
}

fn call_tool(
    reader: &mut std::io::BufReader<std::process::ChildStdout>,
    stdin: &mut std::process::ChildStdin,
    id: i64,
    name: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    send(
        stdin,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}}),
    );
    read_id(reader, id)
}

fn result_text(resp: &serde_json::Value) -> String {
    resp.pointer("/result/content")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn extract_messages_is_bounded_with_truncation_notice() {
    let root = scratch("big");
    // ~3000 messages * ~2KB = ~6MB raw; forces record_limit truncation at 100.
    make_big_session(&root, "session_20260915_095955_51a9645a", 3000, 2000);
    let (mut child, mut reader, mut stdin, cwd) = spawn_mcp(&root);
    init(&mut reader, &mut stdin);

    let resp = call_tool(
        &mut reader,
        &mut stdin,
        2,
        "extract_messages",
        serde_json::json!({
            "session_id": "51a9645a", "full": true
        }),
    );
    assert!(
        resp.pointer("/result/isError").and_then(|v| v.as_bool()) != Some(true),
        "not a tool error: {resp}"
    );
    let text = result_text(&resp);
    // Bounded: never emits the full ~6MB.
    assert!(
        text.len() <= 8 * 1024 * 1024,
        "payload under ceiling, got {}",
        text.len()
    );
    let env: serde_json::Value = serde_json::from_str(&text).expect("envelope is valid JSON");
    let bounds = env.get("bounds").expect("bounds present");
    assert_eq!(
        bounds.get("truncated").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert!(
        !bounds
            .get("notice")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .is_empty(),
        "notice non-empty"
    );
    assert_eq!(
        bounds.get("returned_records").and_then(|n| n.as_u64()),
        Some(100),
        "default limit 100"
    );
    assert!(
        env.get("messages").and_then(|m| m.as_array()).is_some(),
        "messages array present"
    );
    reap_mcp(&mut child, &cwd);
}

#[test]
fn extract_messages_next_offset_pages_through_session() {
    let root = scratch("paging");
    make_big_session(&root, "session_20260915_095955_51a9645a", 250, 100);
    let (mut child, mut reader, mut stdin, cwd) = spawn_mcp(&root);
    init(&mut reader, &mut stdin);

    // Page 1: offset 0, limit 100
    let p1 = call_tool(
        &mut reader,
        &mut stdin,
        2,
        "extract_messages",
        serde_json::json!({
            "session_id": "51a9645a", "full": true, "offset": 0, "limit": 100
        }),
    );
    let b1: serde_json::Value = serde_json::from_str(&result_text(&p1)).unwrap();
    let next = b1
        .pointer("/bounds/next_offset")
        .and_then(|n| n.as_u64())
        .expect("next_offset");
    assert_eq!(next, 100);

    // Page 2: follow next_offset
    let p2 = call_tool(
        &mut reader,
        &mut stdin,
        3,
        "extract_messages",
        serde_json::json!({
            "session_id": "51a9645a", "full": true, "offset": next, "limit": 100
        }),
    );
    let b2: serde_json::Value = serde_json::from_str(&result_text(&p2)).unwrap();
    assert_eq!(
        b2.pointer("/bounds/offset").and_then(|n| n.as_u64()),
        Some(100)
    );
    assert_eq!(
        b2.pointer("/bounds/next_offset").and_then(|n| n.as_u64()),
        Some(200)
    );

    // Distinct pages, no overlap
    let m1: std::collections::HashSet<String> = b1
        .pointer("/messages")
        .and_then(|m| m.as_array())
        .unwrap()
        .iter()
        .map(|m| m.to_string())
        .collect();
    let m2: std::collections::HashSet<String> = b2
        .pointer("/messages")
        .and_then(|m| m.as_array())
        .unwrap()
        .iter()
        .map(|m| m.to_string())
        .collect();
    assert!(m1.is_disjoint(&m2), "pages must not overlap");
    reap_mcp(&mut child, &cwd);
}

#[test]
fn extract_user_messages_is_bounded() {
    let root = scratch("user");
    make_big_session(&root, "session_20260915_095955_51a9645a", 2000, 1500);
    let (mut child, mut reader, mut stdin, cwd) = spawn_mcp(&root);
    init(&mut reader, &mut stdin);

    let resp = call_tool(
        &mut reader,
        &mut stdin,
        2,
        "extract_user_messages",
        serde_json::json!({
            "session_id": "51a9645a"
        }),
    );
    let text = result_text(&resp);
    assert!(text.len() <= 8 * 1024 * 1024, "user_messages under ceiling");
    let env: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        env.pointer("/bounds/truncated").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert!(
        env.get("user_messages")
            .and_then(|m| m.as_array())
            .is_some(),
        "user_messages key present"
    );
    reap_mcp(&mut child, &cwd);
}

#[test]
fn extract_messages_rejects_excessive_limit() {
    let root = scratch("limit");
    make_big_session(&root, "session_20260915_095955_51a9645a", 10, 10);
    let (mut child, mut reader, mut stdin, cwd) = spawn_mcp(&root);
    init(&mut reader, &mut stdin);
    let resp = call_tool(
        &mut reader,
        &mut stdin,
        2,
        "extract_messages",
        serde_json::json!({
            "session_id": "51a9645a", "full": true, "limit": 5000
        }),
    );
    assert_eq!(
        resp.pointer("/result/isError").and_then(|v| v.as_bool()),
        Some(true),
        "over-max limit is a tool error"
    );
    reap_mcp(&mut child, &cwd);
}

#[test]
fn extract_messages_small_max_bytes_drives_byte_cap() {
    let root = scratch("bytecap");
    // 3000 * ~2KB = 6MB; window default 100 records ~200KB; budget 50KB-8KB
    // forces byte_cap truncation well inside the window.
    make_big_session(&root, "session_20260915_095955_51a9645a", 3000, 2000);
    let (mut child, mut reader, mut stdin, cwd) = spawn_mcp(&root);
    init(&mut reader, &mut stdin);
    let resp = call_tool(
        &mut reader,
        &mut stdin,
        2,
        "extract_messages",
        serde_json::json!({
            "session_id": "51a9645a", "full": true, "max_bytes": 50000
        }),
    );
    assert!(
        resp.pointer("/result/isError").and_then(|v| v.as_bool()) != Some(true),
        "not a tool error: {resp}"
    );
    let text = result_text(&resp);
    let env: serde_json::Value = serde_json::from_str(&text).expect("envelope is valid JSON");
    let b = env.get("bounds").unwrap();
    assert_eq!(b.get("truncated").and_then(|v| v.as_bool()), Some(true));
    // Both flags are legitimately true: the default limit-100 window cuts the
    // 3000-record session, and the 50KB byte cap cuts inside that window.
    assert!(
        b.get("truncation_reason")
            .and_then(|v| v.as_str())
            .unwrap()
            .contains("byte_cap"),
        "reason: {b}"
    );
    assert!(
        b.get("returned_records").and_then(|n| n.as_u64()).unwrap() < 100,
        "byte cap cuts inside the window"
    );
    // Compact emission: bounding is computed on the same serialization that is
    // emitted, so the contract holds exactly.
    assert!(
        text.len() <= 50000,
        "envelope must respect max_bytes exactly, got {}",
        text.len()
    );
    reap_mcp(&mut child, &cwd);
}

#[test]
fn extract_messages_offset_past_end_is_not_truncated() {
    let root = scratch("pastend");
    make_big_session(&root, "session_20260915_095955_51a9645a", 50, 100);
    let (mut child, mut reader, mut stdin, cwd) = spawn_mcp(&root);
    init(&mut reader, &mut stdin);
    let resp = call_tool(
        &mut reader,
        &mut stdin,
        2,
        "extract_messages",
        serde_json::json!({
            "session_id": "51a9645a", "full": true, "offset": 99999, "limit": 100
        }),
    );
    let text = result_text(&resp);
    let env: serde_json::Value = serde_json::from_str(&text).unwrap();
    let b = env.get("bounds").unwrap();
    assert_eq!(
        b.get("truncated").and_then(|v| v.as_bool()),
        Some(false),
        "nothing cut: {b}"
    );
    assert_eq!(b.get("returned_records").and_then(|n| n.as_u64()), Some(0));
    assert_eq!(
        b.get("notice").and_then(|n| n.as_str()).unwrap_or("x"),
        "",
        "notice empty"
    );
    reap_mcp(&mut child, &cwd);
}

#[test]
fn extract_messages_tiny_max_bytes_is_a_tool_error() {
    // Even one record cannot fit under the budget: a descriptive tool error
    // instead of silently breaking the cap.
    let root = scratch("tinycap");
    make_big_session(&root, "session_20260915_095955_51a9645a", 10, 500);
    let (mut child, mut reader, mut stdin, cwd) = spawn_mcp(&root);
    init(&mut reader, &mut stdin);
    let resp = call_tool(
        &mut reader,
        &mut stdin,
        2,
        "extract_messages",
        serde_json::json!({
            "session_id": "51a9645a", "full": true, "max_bytes": 100
        }),
    );
    assert_eq!(
        resp.pointer("/result/isError").and_then(|v| v.as_bool()),
        Some(true)
    );
    let text = result_text(&resp);
    assert!(
        text.contains("even one record"),
        "error names the fix: {text}"
    );
    assert!(
        text.contains("max_record_bytes"),
        "error names the clamp: {text}"
    );
    reap_mcp(&mut child, &cwd);
}
