mod common;

use common::scratch::{child_cwd, scratch};
use std::io::{BufRead, Write};
use std::path::Path;
use total_recall::mcp::resolve_skill_path;

fn spawn_mcp_server(harness: &str, skill_path: Option<&Path>) -> (std::process::Child, std::io::BufReader<std::process::ChildStdout>, std::process::ChildStdin, std::path::PathBuf) {
    let cwd = child_cwd("help");
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_total-recall"));
    cmd.args(["--harness", harness, "mcp"])
        .current_dir(&cwd)
        .env_remove("INCEPTION_API_KEY")
        .env_remove("MISTRAL_API_KEY");
    if let Some(p) = skill_path {
        cmd.env("TOTAL_RECALL_SKILL_PATH", p);
    } else {
        cmd.env_remove("TOTAL_RECALL_SKILL_PATH");
    }
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn total-recall mcp");
    let stdout = child.stdout.take().expect("stdout pipe missing");
    let stdin = child.stdin.take().expect("stdin pipe missing");
    (child, std::io::BufReader::new(stdout), stdin, cwd)
}

fn reap_mcp(child: &mut std::process::Child, cwd: &std::path::Path) {
    child.kill().ok();
    child.wait().ok();
    let _ = std::fs::remove_dir_all(cwd);
}

fn send_json(stdin: &mut std::process::ChildStdin, value: &serde_json::Value) {
    let line = serde_json::to_string(value).expect("valid json");
    writeln!(stdin, "{line}").expect("write");
    stdin.flush().expect("flush");
}

fn read_response(reader: &mut std::io::BufReader<std::process::ChildStdout>, expected_id: i64) -> serde_json::Value {
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).expect("read");
        assert!(n > 0, "server closed stdout");
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
            && v.get("id").and_then(|id| id.as_i64()) == Some(expected_id)
        {
            return v;
        }
    }
}

fn call_help(skill_path: Option<&Path>) -> (String, bool) {
    let (mut child, mut reader, mut stdin, cwd) = spawn_mcp_server("vibe", skill_path);
    send_json(&mut stdin, &serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}
    }));
    let init = read_response(&mut reader, 1);
    assert!(init.get("result").is_some(), "init failed: {init}");
    send_json(&mut stdin, &serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    send_json(&mut stdin, &serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "help", "arguments": {"max_bytes": 1000000}}
    }));
    let call = read_response(&mut reader, 2);
    let result = call.pointer("/result").expect("result");
    let is_error = result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
    let text: String = result.get("content").and_then(|c| c.as_array())
        .map(|arr| arr.iter().filter_map(|b| b.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join(""))
        .unwrap_or_default();
    reap_mcp(&mut child, &cwd);
    (text, is_error)
}

// --- pure unit tests ---

#[test]
fn resolve_skill_path_env_wins() {
    let env = Some("/tmp/some/SKILL.md");
    let exe = Path::new("/a/b/c/total-recall");
    assert_eq!(resolve_skill_path(env, exe).unwrap(), Path::new("/tmp/some/SKILL.md"));
}

#[test]
fn resolve_skill_path_env_empty_falls_through() {
    let root = scratch("help_walk");
    std::fs::create_dir_all(root.join("skills/total-recall")).unwrap();
    std::fs::write(root.join("skills/total-recall/SKILL.md"), "x").unwrap();
    let exe = root.join("a/b/total-recall");
    assert_eq!(resolve_skill_path(Some(""), &exe).unwrap(), root.join("skills/total-recall/SKILL.md"));
}

#[test]
fn resolve_skill_path_walk_up_find() {
    let root = scratch("help_walk2");
    std::fs::create_dir_all(root.join("skills/total-recall")).unwrap();
    std::fs::write(root.join("skills/total-recall/SKILL.md"), "x").unwrap();
    let exe = root.join("a/b/total-recall");
    assert_eq!(resolve_skill_path(None, &exe).unwrap(), root.join("skills/total-recall/SKILL.md"));
}

#[test]
fn resolve_skill_path_both_missing() {
    let root = scratch("help_miss");
    let exe = root.join("a/b/total-recall");
    let err = resolve_skill_path(None, &exe).unwrap_err();
    assert_eq!(err, root.join("a/b/skills/total-recall/SKILL.md"));
}

// --- integration tests ---

#[test]
fn help_env_var_returns_that_file() {
    let root = scratch("help_envfile");
    let fixture = root.join("SKILL.md");
    std::fs::write(&fixture, "DISTINCTIVE_HELP_CONTENT_42").unwrap();
    let (text, is_error) = call_help(Some(&fixture));
    assert!(!is_error, "help errored: {text}");
    assert!(text.contains("DISTINCTIVE_HELP_CONTENT_42"), "missing fixture content: {text}");
}

#[test]
fn help_env_var_missing_path_complains() {
    let root = scratch("help_missingenv");
    let nonexistent = root.join("nope.md");
    let (text, is_error) = call_help(Some(&nonexistent));
    assert!(is_error, "expected error, got: {text}");
    assert!(text.contains(&nonexistent.display().to_string()), "path not named: {text}");
    assert!(text.contains("TOTAL_RECALL_SKILL_PATH"), "env var not named: {text}");
}

#[test]
fn help_unset_finds_repo_skill() {
    let (text, is_error) = call_help(None);
    assert!(!is_error, "help errored: {text}");
    assert!(text.contains("Total Recall"), "real skill not found: {text}");
}

#[test]
fn help_drift_skill_table_matches_tool_surface() {
    // get tools/list
    let (mut child, mut reader, mut stdin, cwd) = spawn_mcp_server("vibe", None);
    send_json(&mut stdin, &serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}
    }));
    let init = read_response(&mut reader, 1);
    assert!(init.get("result").is_some());
    send_json(&mut stdin, &serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    send_json(&mut stdin, &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let list = read_response(&mut reader, 2);
    let tools = list.pointer("/result/tools").unwrap().as_array().unwrap();
    let names: std::collections::HashSet<String> = tools.iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(String::from))
        .collect();

    // call help
    send_json(&mut stdin, &serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": "help", "arguments": {"max_bytes": 1000000}}
    }));
    let call = read_response(&mut reader, 3);
    let result = call.pointer("/result").unwrap();
    let text: String = result.get("content").and_then(|c| c.as_array())
        .map(|arr| arr.iter().filter_map(|b| b.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join(""))
        .unwrap_or_default();
    reap_mcp(&mut child, &cwd);

    // parse backticked snake_case identifiers from table first column
    let mut skill_tools = std::collections::HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('|') { continue; }
        // first cell
        let cells: Vec<&str> = line.split('|').collect();
        if cells.len() < 2 { continue; }
        let first = cells[1];
        // find backticked identifiers containing underscore (snake_case tool names)
        for cap in first.split('`') {
            // cap is between backticks if odd index; simpler: regex-like filter
            if !cap.is_empty() && cap.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                // but total-recall prefix already stripped? ensure exact tool name
                skill_tools.insert(cap.to_string());
            }
        }
    }
    // The table uses total-recall_ prefix in some rows; our split above only
    // captures pure snake_case tokens, which matches bare tool names. Rows with
    // total-recall_ prefix yield the suffix only if split on backtick? Actually
    // first cell "`total-recall_summary_cache`" -> splitting backticks gives
    // "total-recall_summary_cache"; chars include hyphen, fails all() filter.
    // So also extract trailing snake_case suffix after last hyphen.
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('|') { continue; }
        let cells: Vec<&str> = line.split('|').collect();
        if cells.len() < 2 { continue; }
        let first = cells[1];
        for seg in first.split('`') {
            let mut candidate = seg.trim();
            if let Some(rest) = candidate.strip_prefix("total-recall_") {
                candidate = rest;
            } else {
                candidate = candidate.rsplit('-').next().unwrap_or(candidate);
            }
            if !candidate.is_empty()
                && candidate.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                && !candidate.is_empty()
                && !candidate.starts_with('_')
            {
                skill_tools.insert(candidate.to_string());
            }
        }
    }

    // Compare
    // skill_tools may include non-tool identifiers like extract? filter to names set? We'll assert subset both directions against names.
    // Remove identifiers not in names? Instead assert names subset skill_tools and skill_tools subset names.
    for n in &names {
        assert!(skill_tools.contains(n), "tool {n} missing from skill table");
    }
    for s in &skill_tools {
        assert!(names.contains(s), "skill table has unknown tool {s}");
    }
}
