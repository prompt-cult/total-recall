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
    let array: serde_json::Value =
        serde_json::from_str(&text).expect("the response is a bare record array");
    let records = array.as_array().expect("an array");
    let meta = &records[0];
    assert_eq!(meta["type"], "meta", "record 0 is the meta: {meta}");
    assert_eq!(meta["tool"], "extract_messages", "{meta}");
    let bounds = meta
        .get("bounds")
        .expect("the meta carries the paging inputs");
    assert_eq!(
        bounds.get("total_records").and_then(|n| n.as_u64()),
        Some(3000),
        "the meta states the session's record count: {bounds}"
    );
    assert_eq!(
        bounds.get("limit").and_then(|n| n.as_u64()),
        Some(100),
        "default limit 100"
    );
    assert!(bounds.get("clamped_record_indices").is_some(), "{bounds}");
    let messages = &records[1..];
    assert_eq!(messages.len(), 100, "the default window is 100 records");
    for record in messages {
        assert_eq!(record["type"], "message", "{record}");
    }
    reap_mcp(&mut child, &cwd);
}

#[test]
fn extract_messages_next_offset_pages_through_session() {
    let root = scratch("paging");
    make_big_session(&root, "session_20260915_095955_51a9645a", 250, 100);
    let (mut child, mut reader, mut stdin, cwd) = spawn_mcp(&root);
    init(&mut reader, &mut stdin);

    // Page 1: offset 0, limit 100. The next offset is derived from what the
    // response returned: offset + record count.
    let p1 = call_tool(
        &mut reader,
        &mut stdin,
        2,
        "extract_messages",
        serde_json::json!({
            "session_id": "51a9645a", "full": true, "offset": 0, "limit": 100
        }),
    );
    let a1: serde_json::Value = serde_json::from_str(&result_text(&p1)).unwrap();
    let records1 = a1.as_array().unwrap();
    assert_eq!(records1[0]["bounds"]["offset"], 0, "{records1:?}");
    assert_eq!(records1[0]["bounds"]["total_records"], 250, "{records1:?}");
    assert_eq!(records1.len() - 1, 100, "the page returns limit records");
    let next = records1.len() - 1;
    assert_eq!(next, 100);

    // Page 2: follow the derived offset.
    let p2 = call_tool(
        &mut reader,
        &mut stdin,
        3,
        "extract_messages",
        serde_json::json!({
            "session_id": "51a9645a", "full": true, "offset": next, "limit": 100
        }),
    );
    let a2: serde_json::Value = serde_json::from_str(&result_text(&p2)).unwrap();
    let records2 = a2.as_array().unwrap();
    assert_eq!(records2[0]["bounds"]["offset"].as_u64(), Some(100));

    // Distinct pages, no overlap
    let m1: std::collections::HashSet<String> =
        records1[1..].iter().map(|m| m.to_string()).collect();
    let m2: std::collections::HashSet<String> =
        records2[1..].iter().map(|m| m.to_string()).collect();
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
    let array: serde_json::Value = serde_json::from_str(&text).unwrap();
    let records = array.as_array().unwrap();
    assert_eq!(
        records[0]["tool"].as_str(),
        Some("extract_user_messages"),
        "the meta names the tool: {records:?}"
    );
    assert_eq!(
        records[0]["bounds"]["total_records"].as_u64(),
        Some(2000),
        "every fixture record is a user record: {records:?}"
    );
    for record in &records[1..] {
        assert_eq!(record["type"], "message", "{record}");
        assert_eq!(record["role"], "user", "user messages only: {record}");
        assert_eq!(
            record["injected"], false,
            "injected records are skipped: {record}"
        );
    }
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
    // 3000 * ~2KB = 6MB; window default 100 records ~200KB; budget 50KB
    // forces the byte cap inside the window: the fitting-prefix array plus
    // the marker return, and the overflow file carries the whole window.
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
    assert!(
        text.contains("--- [EOF-TRUNCATED] ---"),
        "the byte cap overflows into the marker: {text}"
    );
    let head = text
        .split("--- [EOF-TRUNCATED] ---")
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    assert!(
        head.len() <= 50000,
        "the array prefix respects max_bytes, got {}",
        head.len()
    );
    let array: serde_json::Value =
        serde_json::from_str(&head).expect("the prefix parses — never torn");
    let records = array.as_array().unwrap();
    assert_eq!(records[0]["type"], "meta", "{records:?}");
    let returned = records.len() - 1;
    assert!(
        returned < 100,
        "byte cap cuts inside the window: {returned} records"
    );
    assert!(
        text.contains("records_returned: ") && text.contains("records_total: 100"),
        "the marker states the cut against the window: {text}"
    );
    let path = text
        .lines()
        .find(|l| l.starts_with("full_report: "))
        .unwrap()
        .trim_start_matches("full_report: ")
        .trim()
        .to_string();
    let written = std::fs::read_to_string(&path).unwrap();
    let file_lines = written.lines().count();
    assert_eq!(
        file_lines, 101,
        "the file carries the meta plus every record in the window"
    );
    for line in written.lines() {
        serde_json::from_str::<serde_json::Value>(line).unwrap_or_else(|e| {
            panic!("every overflow-file line is one JSON object ({e}): {line}")
        });
    }
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
    let array: serde_json::Value = serde_json::from_str(&text).unwrap();
    let records = array.as_array().unwrap();
    assert_eq!(
        records.len(),
        1,
        "an empty window is the meta record alone: {records:?}"
    );
    assert_eq!(records[0]["bounds"]["total_records"], 50, "{records:?}");
    assert!(
        !text.contains("EOF-TRUNCATED"),
        "nothing was cut, no marker: {text}"
    );
    reap_mcp(&mut child, &cwd);
}

#[test]
fn extract_messages_tiny_max_bytes_overflows_to_the_file() {
    // The "even one record exceeds max_bytes" hard error is DEAD: the meta
    // plus the marker return, and the file carries every record — the fix a
    // caller used to be told to make by hand is now automatic.
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
        Some(false),
        "an overflowing extraction is NOT a tool error anymore: {resp}"
    );
    let text = result_text(&resp);
    assert!(
        text.contains("EOF-TRUNCATED") && text.contains("full_report"),
        "the marker names the overflow file: {text}"
    );
    let path = text
        .lines()
        .find(|l| l.starts_with("full_report: "))
        .unwrap()
        .trim_start_matches("full_report: ")
        .trim()
        .to_string();
    let written = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(
        lines.len(),
        11,
        "the file carries the meta plus all 10 records the response could not"
    );
    for line in &lines {
        serde_json::from_str::<serde_json::Value>(line).unwrap_or_else(|e| {
            panic!("every overflow-file line is one JSON object ({e}): {line}")
        });
    }
    reap_mcp(&mut child, &cwd);
}
