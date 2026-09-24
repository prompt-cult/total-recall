use std::io::{BufRead, Write};
use std::sync::Mutex;

use total_recall::harness::{HARNESS_ENV_VAR, VALID_HARNESSES, make_adapter, resolve_harness};
use total_recall::mcp::TotalRecallServer;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn with_env(key: &str, value: Option<&str>, f: impl FnOnce()) {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = std::env::var(key).ok();
    match value {
        Some(v) => unsafe { std::env::set_var(key, v) },
        None => unsafe { std::env::remove_var(key) },
    }
    f();
    match saved {
        Some(v) => unsafe { std::env::set_var(key, &v) },
        None => unsafe { std::env::remove_var(key) },
    }
}

#[test]
fn refuses_to_start_without_flag_or_env() {
    with_env(HARNESS_ENV_VAR, None, || {
        let result = resolve_harness(None);
        let err = result.expect_err("must refuse without flag or env");
        assert!(err.contains("HARNESS"), "must name the env var: {err}");
        assert!(err.contains("--harness"), "must name the flag: {err}");
        assert!(
            err.contains("vibe") && err.contains("codex") && err.contains("claude"),
            "must list valid values: {err}"
        );
    });
}

#[test]
fn binds_harness_from_env_var() {
    with_env(HARNESS_ENV_VAR, Some("codex"), || {
        assert_eq!(resolve_harness(None).unwrap(), "codex");
    });
}

#[test]
fn flag_overrides_env_var() {
    with_env(HARNESS_ENV_VAR, Some("vibe"), || {
        assert_eq!(resolve_harness(Some("claude")).unwrap(), "claude");
    });
}

#[test]
fn unknown_harness_value_is_refused() {
    with_env(HARNESS_ENV_VAR, Some("bogus"), || {
        let err = resolve_harness(None).expect_err("unknown env value must be refused");
        assert!(
            err.contains("bogus") || err.contains("vibe|codex|claude"),
            "must explain the refusal: {err}"
        );
    });
    let err = resolve_harness(Some("bogus")).expect_err("unknown flag value must be refused");
    assert!(
        err.contains("vibe|codex|claude") || err.contains("bogus"),
        "must explain the refusal: {err}"
    );
}

#[test]
fn server_binds_and_exposes_harness() {
    let server = TotalRecallServer::with_harness("codex".to_string());
    assert_eq!(server.harness(), "codex");
}

#[test]
fn adapter_factory_errors_on_unknown_harness() {
    assert!(make_adapter("bogus").is_err());
    assert!(make_adapter("vibe").is_ok());
    assert!(make_adapter("codex").is_ok());
    assert!(make_adapter("claude").is_ok());
}

#[test]
fn opencode_harness_is_accepted() {
    assert!(
        VALID_HARNESSES.contains(&"opencode"),
        "opencode must be a valid harness: {:?}",
        VALID_HARNESSES
    );
    with_env(HARNESS_ENV_VAR, Some("opencode"), || {
        assert_eq!(resolve_harness(None).unwrap(), "opencode");
    });
    assert_eq!(resolve_harness(Some("opencode")).unwrap(), "opencode");
    let adapter = make_adapter("opencode").expect("opencode adapter must build");
    assert_eq!(adapter.name(), "opencode");
}

fn spawn_mcp_server(
    harness: &str,
) -> (
    std::process::Child,
    std::io::BufReader<std::process::ChildStdout>,
    std::process::ChildStdin,
) {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_total-recall"))
        .args(["--harness", harness, "mcp"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn total-recall mcp");
    let stdout = child.stdout.take().expect("stdout pipe missing");
    let stdin = child.stdin.take().expect("stdin pipe missing");
    (child, std::io::BufReader::new(stdout), stdin)
}

fn send_json(stdin: &mut std::process::ChildStdin, value: &serde_json::Value) {
    let line = serde_json::to_string(value).expect("valid json");
    writeln!(stdin, "{line}").expect("failed to write to mcp stdin");
    stdin.flush().expect("failed to flush mcp stdin");
}

fn read_response(
    reader: &mut std::io::BufReader<std::process::ChildStdout>,
    expected_id: i64,
) -> serde_json::Value {
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .expect("failed to read mcp stdout");
        assert!(
            n > 0,
            "mcp server closed stdout before returning id {expected_id}"
        );
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
            && v.get("id").and_then(|id| id.as_i64()) == Some(expected_id)
        {
            return v;
        }
    }
}

#[test]
fn mcp_tools_list_includes_index_and_sheep() {
    let (mut child, mut reader, mut stdin) = spawn_mcp_server("vibe");

    send_json(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1.0" }
            }
        }),
    );
    let init = read_response(&mut reader, 1);
    assert!(
        init.get("result").is_some(),
        "initialize must succeed: {init}"
    );

    send_json(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }),
    );

    send_json(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        }),
    );
    let list = read_response(&mut reader, 2);
    let tools = list
        .pointer("/result/tools")
        .expect("tools/list response must contain result.tools")
        .as_array()
        .expect("tools must be an array");
    let names: Vec<String> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(String::from))
        .collect();
    assert!(
        names.iter().any(|n| n == "index_sessions"),
        "tools/list must include index_sessions, got: {names:?}"
    );
    assert!(
        names
            .iter()
            .any(|n| n == "do_android_dream_of_electric_sheep"),
        "tools/list must include do_android_dream_of_electric_sheep, got: {names:?}"
    );

    let index_tool = tools
        .iter()
        .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("index_sessions"))
        .expect("index_sessions metadata");
    let index_desc = index_tool
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        index_desc.contains("index"),
        "index_sessions description must mention indexing, got: {index_desc}"
    );

    let sheep_tool = tools
        .iter()
        .find(|t| {
            t.get("name").and_then(|n| n.as_str()) == Some("do_android_dream_of_electric_sheep")
        })
        .expect("sheep tool metadata");
    let sheep_desc = sheep_tool
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        sheep_desc.contains("search") || sheep_desc.contains("full-text"),
        "sheep tool description must mention search, got: {sheep_desc}"
    );

    let _ = child.kill();
}

#[test]
fn mcp_sheep_errors_on_empty_query() {
    let (mut child, mut reader, mut stdin) = spawn_mcp_server("vibe");

    send_json(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1.0" }
            }
        }),
    );
    let init = read_response(&mut reader, 1);
    assert!(
        init.get("result").is_some(),
        "initialize must succeed: {init}"
    );

    send_json(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }),
    );

    send_json(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "do_android_dream_of_electric_sheep",
                "arguments": {
                    "query": "",
                    "sessions": [],
                    "hours_back": 48,
                    "directory": null
                }
            }
        }),
    );
    let call = read_response(&mut reader, 2);
    let result = call
        .pointer("/result")
        .expect("tools/call response must contain result");
    assert!(
        result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "empty query must produce a tool-level error result: {result}"
    );
    let content_text: String = result
        .get("content")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    assert!(
        content_text.to_lowercase().contains("query"),
        "error content must mention the missing query, got: {content_text}"
    );

    let _ = child.kill();
}
