mod common;

use std::io::{BufRead, Write};
use std::sync::Mutex;

use common::scratch::child_cwd;
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
            err.contains("vibe") && err.contains("opencode"),
            "must list valid values: {err}"
        );
    });
}

#[test]
fn binds_harness_from_env_var() {
    with_env(HARNESS_ENV_VAR, Some("vibe"), || {
        assert_eq!(resolve_harness(None).unwrap(), "vibe");
    });
}

#[test]
fn flag_overrides_env_var() {
    with_env(HARNESS_ENV_VAR, Some("vibe"), || {
        assert_eq!(resolve_harness(Some("opencode")).unwrap(), "opencode");
    });
}

#[test]
fn unknown_harness_value_is_refused() {
    with_env(HARNESS_ENV_VAR, Some("bogus"), || {
        let err = resolve_harness(None).expect_err("unknown env value must be refused");
        assert!(
            err.contains("bogus") || err.contains("vibe|opencode"),
            "must explain the refusal: {err}"
        );
    });
    let err = resolve_harness(Some("bogus")).expect_err("unknown flag value must be refused");
    assert!(
        err.contains("vibe|opencode") || err.contains("bogus"),
        "must explain the refusal: {err}"
    );
}

#[test]
fn server_binds_and_exposes_harness() {
    let server = TotalRecallServer::with_harness("opencode".to_string());
    assert_eq!(server.harness(), "opencode");
}

#[test]
fn adapter_factory_errors_on_unknown_harness() {
    assert!(make_adapter("bogus").is_err());
    assert!(make_adapter("vibe").is_ok());
    assert!(make_adapter("opencode").is_ok());
}

/// The supported harness set is `vibe | opencode | codex`. Everywhere a
/// harness can be named — the adapter factory, the `HARNESS` env var, the
/// `--harness` flag — a value outside the set is refused with the house-form
/// error that names the refused value and the supported set.
#[test]
fn removed_harnesses_are_refused_with_the_supported_set() {
    let removed = "claude";
    let err = match make_adapter(removed) {
        Ok(_) => panic!("removed harness must not build an adapter"),
        Err(e) => e,
    };
    assert!(err.contains(removed), "must name the refused value: {err}");
    assert!(
        err.contains("vibe|opencode"),
        "must name the supported set: {err}"
    );
    with_env(HARNESS_ENV_VAR, Some(removed), || {
        let err = resolve_harness(None).expect_err("removed harness must be refused from HARNESS");
        assert!(err.contains(removed), "must name the refused value: {err}");
        assert!(
            err.contains("vibe|opencode"),
            "must name the supported set: {err}"
        );
    });
    let err =
        resolve_harness(Some(removed)).expect_err("removed harness must be refused from --harness");
    assert!(
        err.contains(removed) && err.contains("vibe|opencode"),
        "must name the refused value and the supported set: {err}"
    );
}

#[test]
fn codex_harness_is_accepted() {
    assert!(
        VALID_HARNESSES.contains(&"codex"),
        "codex must be a valid harness: {:?}",
        VALID_HARNESSES
    );
    with_env(HARNESS_ENV_VAR, Some("codex"), || {
        assert_eq!(resolve_harness(None).unwrap(), "codex");
    });
    assert_eq!(resolve_harness(Some("codex")).unwrap(), "codex");
    let adapter = make_adapter("codex").expect("codex adapter must build");
    assert_eq!(adapter.name(), "codex");
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

/// An MCP child, plus the scratch working directory outside the repo tree it
/// was spawned in. The directory is returned because the child outlives this
/// function: only the caller, which owns the `Child`, can safely clear it.
/// `dotenvy` walks up parent directories, so the CWD must sit where no `.env`
/// is above it, and the vendor keys are removed as well.
fn spawn_mcp_server(
    harness: &str,
) -> (
    std::process::Child,
    std::io::BufReader<std::process::ChildStdout>,
    std::process::ChildStdin,
    std::path::PathBuf,
) {
    let cwd = child_cwd("mcp");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_total-recall"))
        .args(["--harness", harness, "mcp"])
        .current_dir(&cwd)
        .env_remove("INCEPTION_API_KEY")
        .env_remove("MISTRAL_API_KEY")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn total-recall mcp");
    let stdout = child.stdout.take().expect("stdout pipe missing");
    let stdin = child.stdin.take().expect("stdin pipe missing");
    (child, std::io::BufReader::new(stdout), stdin, cwd)
}

/// Clear a child's scratch working directory, once that child has been killed.
fn reap_mcp(child: &mut std::process::Child, cwd: &std::path::Path) {
    child.kill().ok();
    child.wait().ok();
    let _ = std::fs::remove_dir_all(cwd);
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
    let (mut child, mut reader, mut stdin, cwd) = spawn_mcp_server("vibe");

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

    reap_mcp(&mut child, &cwd);
}

#[test]
fn mcp_sheep_errors_on_empty_query() {
    let (mut child, mut reader, mut stdin, cwd) = spawn_mcp_server("vibe");

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

    reap_mcp(&mut child, &cwd);
}

/// The `harness` tool's new-world record-stream contract: the response text is
/// a bare compact JSON array — element 0 is the meta record carrying the
/// harness name, and the array has exactly one element (the binding is the
/// whole payload). The old pretty-object shape is dead; no compat shim.
#[test]
fn mcp_harness_tool_returns_the_bare_meta_array() {
    let (mut child, mut reader, mut stdin, cwd) = spawn_mcp_server("opencode");

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
                "name": "harness",
                "arguments": {}
            }
        }),
    );
    let call = read_response(&mut reader, 2);
    let result = call
        .pointer("/result")
        .expect("tools/call response must contain result");
    assert!(
        !result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "a plain binding report must not be a tool-level error: {result}"
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
    let value: serde_json::Value = serde_json::from_str(&content_text).unwrap_or_else(|e| {
        panic!("the response must parse as BARE JSON, no prefix ({e}): {content_text:?}")
    });
    let array = value
        .as_array()
        .expect("the response must be a bare JSON array - the old pretty-object shape is dead");
    assert_eq!(
        array.len(),
        1,
        "the binding is the whole payload: exactly one element, got: {array:?}"
    );
    let meta = &array[0];
    assert_eq!(
        meta.get("type").and_then(|t| t.as_str()),
        Some("meta"),
        "element 0 carries the type discriminator: {meta}"
    );
    assert_eq!(
        meta.get("tool").and_then(|t| t.as_str()),
        Some("harness"),
        "element 0 names the tool: {meta}"
    );
    assert_eq!(
        meta.get("harness").and_then(|h| h.as_str()),
        Some("opencode"),
        "element 0 carries the harness name: {meta}"
    );

    reap_mcp(&mut child, &cwd);
}
