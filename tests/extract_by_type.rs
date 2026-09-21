//! #7: extract_by_type emits one `type,timestamp,json` record per line, bounded,
//! with a `#` header carrying bounds — asserted through the MCP stdio server.

use std::io::{BufRead, Write};
use std::path::PathBuf;

fn tmp_root(tag: &str) -> PathBuf {
    let dir = std::env::var("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
        .join(format!("tr_xbytype_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_session(root: &std::path::Path, name: &str, lines: &[String]) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("messages.jsonl"), lines.join("\n") + "\n").unwrap();
    std::fs::write(
        dir.join("meta.json"),
        format!("{{\"session_id\":\"uuid-{name}\",\"start_time\":\"2026-09-15T09:59:55+00:00\",\"end_time\":\"2026-09-15T10:00:00+00:00\",\"title\":\"t\",\"total_messages\":{}}}", lines.len()),
    )
    .unwrap();
}

fn spawn_mcp(root: &std::path::Path) -> (std::process::Child, std::io::BufReader<std::process::ChildStdout>, std::process::ChildStdin) {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_total-recall"))
        .args(["--harness", "vibe", "mcp"])
        .env("TOTAL_RECALL_VIBE_ROOT", root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn mcp");
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    (child, std::io::BufReader::new(stdout), stdin)
}

fn send(stdin: &mut std::process::ChildStdin, v: &serde_json::Value) {
    writeln!(stdin, "{}", serde_json::to_string(v).unwrap()).unwrap();
    stdin.flush().unwrap();
}

fn read_id(reader: &mut std::io::BufReader<std::process::ChildStdout>, id: i64) -> serde_json::Value {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap() == 0 { panic!("server closed stdout"); }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim())
            && v.get("id").and_then(|i| i.as_i64()) == Some(id) { return v; }
    }
}

fn init(reader: &mut std::io::BufReader<std::process::ChildStdout>, stdin: &mut std::process::ChildStdin) {
    send(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}));
    read_id(reader, 1);
    send(stdin, &serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
}

fn text_of(resp: &serde_json::Value) -> String {
    resp.pointer("/result/content").and_then(|c| c.as_array())
        .map(|a| a.iter().filter_map(|b| b.get("text").and_then(|t| t.as_str())).collect())
        .unwrap_or_default()
}

fn sample_lines() -> Vec<String> {
    vec![
        "{\"role\":\"user\",\"content\":\"first user\",\"timestamp\":\"2026-09-15T09:59:55Z\",\"injected\":false}".to_string(),
        "{\"role\":\"assistant\",\"content\":\"line one\\nline two\",\"timestamp\":\"2026-09-15T09:59:56Z\"}".to_string(),
        "{\"role\":\"tool\",\"content\":\"tool output\",\"timestamp\":\"2026-09-15T09:59:57Z\"}".to_string(),
        "{\"role\":\"user\",\"content\":\"injected marker\",\"timestamp\":\"2026-09-15T09:59:58Z\",\"injected\":true}".to_string(),
    ]
}

fn data_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).collect()
}

#[test]
fn extract_by_type_all_selects_every_type_and_format_is_parseable() {
    let root = tmp_root("all");
    make_session(&root, "session_20260915_095955_51a9645a", &sample_lines());
    let (mut child, mut reader, mut stdin) = spawn_mcp(&root);
    init(&mut reader, &mut stdin);
    send(&mut stdin, &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"extract_by_type","arguments":{"session_id":"51a9645a","full":true,"types":["all"]}}}));
    let resp = read_id(&mut reader, 2);
    let text = text_of(&resp);

    // Header line present and is valid JSON after the '#'.
    let header = text.lines().next().unwrap();
    assert!(header.starts_with('#'));
    let _: serde_json::Value = serde_json::from_str(&header[1..]).expect("header is JSON");

    let lines = data_lines(&text);
    // injected user message excluded -> 3 records (user, assistant, tool)
    assert_eq!(lines.len(), 3, "injected excluded: {lines:?}");
    for l in lines {
        let parts: Vec<&str> = l.splitn(3, ',').collect();
        assert_eq!(parts.len(), 3, "three comma fields: {l}");
        assert!(["user", "assistant", "tool", "thinking"].contains(&parts[0]));
        serde_json::from_str::<serde_json::Value>(parts[2]).expect("third field parses as JSON");
    }
    // Multiline content stayed on one physical line (escaped).
    let assistant = text.lines().find(|l| l.starts_with("assistant,")).unwrap();
    assert!(assistant.contains("\\n"), "embedded newline escaped: {assistant}");
    let _ = child.kill();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn extract_by_type_single_type_filter() {
    let root = tmp_root("filter");
    make_session(&root, "session_20260915_095955_51a9645a", &sample_lines());
    let (mut child, mut reader, mut stdin) = spawn_mcp(&root);
    init(&mut reader, &mut stdin);
    send(&mut stdin, &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"extract_by_type","arguments":{"session_id":"51a9645a","full":true,"types":["user"]}}}));
    let resp = read_id(&mut reader, 2);
    let text = text_of(&resp);
    let lines = data_lines(&text);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with("user,"));
    let _ = child.kill();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn extract_by_type_unknown_type_errors() {
    let root = tmp_root("unknown");
    make_session(&root, "session_20260915_095955_51a9645a", &sample_lines());
    let (mut child, mut reader, mut stdin) = spawn_mcp(&root);
    init(&mut reader, &mut stdin);
    send(&mut stdin, &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"extract_by_type","arguments":{"session_id":"51a9645a","full":true,"types":["bogus"]}}}));
    let resp = read_id(&mut reader, 2);
    assert_eq!(resp.pointer("/result/isError").and_then(|v| v.as_bool()), Some(true));
    assert!(text_of(&resp).contains("bogus"));
    let _ = child.kill();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn extract_by_type_is_bounded_with_notice() {
    let root = tmp_root("bounded");
    let big = "y".repeat(2000);
    let lines: Vec<String> = (0..500).map(|i| format!("{{\"role\":\"user\",\"content\":\"m{i} {big}\",\"timestamp\":\"2026-09-15T09:59:55Z\",\"injected\":false}}")).collect();
    make_session(&root, "session_20260915_095955_51a9645a", &lines);
    let (mut child, mut reader, mut stdin) = spawn_mcp(&root);
    init(&mut reader, &mut stdin);
    send(&mut stdin, &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"extract_by_type","arguments":{"session_id":"51a9645a","full":true,"types":["user"]}}}));
    let resp = read_id(&mut reader, 2);
    let text = text_of(&resp);
    assert!(text.len() <= 8 * 1024 * 1024);
    let header = text.lines().next().unwrap();
    let hj: serde_json::Value = serde_json::from_str(&header[1..]).unwrap();
    assert_eq!(hj.pointer("/bounds/truncated").and_then(|v| v.as_bool()), Some(true));
    assert!(text.lines().any(|l| l.starts_with("# TRUNCATED:")), "truncation notice line present");
    let _ = child.kill();
    let _ = std::fs::remove_dir_all(&root);
}
