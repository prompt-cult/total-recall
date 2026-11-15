//! #508: `chat_with_mercury` + CLI `chat` — THE one-call key check. The key
//! guard runs FIRST: `INCEPTION_API_KEY` missing or EMPTY and the call is
//! refused with the house error naming the three setup places, and the
//! request is never attempted (there is no network in the test env; the
//! guard's error must not depend on one). Written RED, before the
//! implementation existed. The success path is live-verified once — vendor-free
//! tests pin the guard and the shapes.

mod common;

use common::scratch::child_cwd;
use serde_json::{Map, Value, json};
use std::io::{BufRead, Write};

struct Mcp {
    child: std::process::Child,
    reader: std::io::BufReader<std::process::ChildStdout>,
    stdin: std::process::ChildStdin,
    cwd: std::path::PathBuf,
    next: u64,
}

impl Mcp {
    fn send(&mut self, v: &Value) {
        writeln!(self.stdin, "{}", serde_json::to_string(v).unwrap()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn read_until_id(&mut self, id: u64) -> Value {
        loop {
            let mut line = String::new();
            self.reader
                .read_line(&mut line)
                .expect("server stdout open");
            if line.trim().is_empty() {
                panic!("server closed stdout waiting for response {id}");
            }
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && v.get("id").and_then(|i| i.as_u64()) == Some(id)
            {
                return v;
            }
        }
    }

    fn call(&mut self, name: &str, args: Value) -> (bool, String) {
        let id = self.next;
        self.next += 1;
        self.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": args}
        }));
        let result = self.read_until_id(id)["result"].clone();
        let is_error = result["isError"].as_bool().unwrap_or(false);
        (
            is_error,
            result["content"][0]["text"]
                .as_str()
                .unwrap_or("")
                .to_string(),
        )
    }

    fn tools_list(&mut self) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"}));
        self.read_until_id(id)["result"]["tools"].clone()
    }

    fn tool(&mut self, name: &str) -> Value {
        self.tools_list()
            .as_array()
            .expect("tools array")
            .iter()
            .find(|t| t["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("{name} is registered in tools/list"))
            .clone()
    }

    fn schema(&mut self, name: &str) -> Map<String, Value> {
        self.tool(name)["inputSchema"]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{name} serves a properties object"))
            .clone()
    }
}

/// Spawn the MCP server with the vendor keys REMOVED, in a working directory
/// with no `.env` above it (the child does not need the harness store: the
/// guard fires before any adapter work).
fn spawn_mcp() -> Mcp {
    let cwd = child_cwd("chat_mercury_mcp");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_total-recall"))
        .args(["--harness", "opencode", "mcp"])
        .current_dir(&cwd)
        .env_remove("INCEPTION_API_KEY")
        .env_remove("MISTRAL_API_KEY")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn mcp");
    let reader = std::io::BufReader::new(child.stdout.take().unwrap());
    let stdin = child.stdin.take().unwrap();
    let mut mcp = Mcp {
        child,
        reader,
        stdin,
        cwd,
        next: 1,
    };
    mcp.send(&json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                   "clientInfo": {"name": "chat-mercury-test", "version": "0"}}
    }));
    let _ = mcp.read_until_id(0);
    mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    mcp
}

fn reap(mcp: &mut Mcp) {
    mcp.child.kill().ok();
    mcp.child.wait().ok();
    let _ = std::fs::remove_dir_all(&mcp.cwd);
}

#[test]
fn missing_key_refuses_at_the_front_door_naming_the_setup_places() {
    let mut mcp = spawn_mcp();
    let started = std::time::Instant::now();
    let (is_error, text) = mcp.call("chat_with_mercury", json!({"message": "hello"}));
    reap(&mut mcp);
    assert!(is_error, "missing key must be a tool error");
    assert!(
        text.contains("INCEPTION_API_KEY"),
        "the error must name the key: {text}"
    );
    assert!(
        text.contains("environment") && text.contains(".env"),
        "the error must name the MCP environment block and the .env walk-up: {text}"
    );
    assert!(
        text.contains("export INCEPTION_API_KEY"),
        "the error must name the shell export: {text}"
    );
    assert!(
        text.contains("chat_with_mercury") || text.contains("`chat`"),
        "the error must name the verifying endpoint: {text}"
    );
    // The guard runs before anything network-shaped: a denial is instant.
    assert!(
        started.elapsed().as_secs() < 5,
        "the guard must fail before any request is attempted, took {:?}",
        started.elapsed()
    );
}

#[test]
fn empty_key_is_the_same_as_missing() {
    let cwd = child_cwd("chat_mercury_empty");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_total-recall"))
        .args(["--harness", "opencode", "mcp"])
        .current_dir(&cwd)
        .env("INCEPTION_API_KEY", "   ")
        .env_remove("MISTRAL_API_KEY")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn mcp");
    let reader = std::io::BufReader::new(child.stdout.take().unwrap());
    let stdin = child.stdin.take().unwrap();
    let mut mcp = Mcp {
        child,
        reader,
        stdin,
        cwd: cwd.clone(),
        next: 1,
    };
    mcp.send(&json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                   "clientInfo": {"name": "chat-mercury-test", "version": "0"}}
    }));
    let _ = mcp.read_until_id(0);
    mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let (is_error, text) = mcp.call("chat_with_mercury", json!({"message": "hello"}));
    reap(&mut mcp);
    assert!(is_error, "a whitespace-only key must be refused");
    assert!(
        text.contains("EMPTY"),
        "the error must say the key is present but EMPTY: {text}"
    );
}

#[test]
fn tool_is_registered_and_describes_the_key_check() {
    let mut mcp = spawn_mcp();
    let tool = mcp.tool("chat_with_mercury");
    let schema = mcp.schema("chat_with_mercury");
    reap(&mut mcp);
    let description = tool["description"].as_str().unwrap_or_default();
    assert!(
        description.contains("key check") && description.contains("INCEPTION_API_KEY"),
        "the tool description must name the key check: {description}"
    );
    assert!(
        schema.contains_key("message") && schema.contains_key("max_tokens"),
        "the schema must serve message and max_tokens: {:?}",
        schema.keys().collect::<Vec<_>>()
    );
}

#[allow(dead_code, unused_imports)]
mod cli {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"));
}

/// The CLI subcommand parses: `chat <message> [--max-tokens N] [--system …]`.
#[test]
fn cli_chat_subcommand_parses() {
    use clap::Parser;
    match cli::Cli::try_parse_from([
        "bin",
        "chat",
        "hello",
        "--max-tokens",
        "128",
        "--system",
        "s",
    ]) {
        Ok(parsed) => match parsed.command {
            cli::Command::Chat {
                message,
                max_tokens,
                system,
            } => {
                assert_eq!(message, "hello");
                assert_eq!(max_tokens, Some(128));
                assert_eq!(system.as_deref(), Some("s"));
            }
            other => panic!("expected Chat, got {other:?}"),
        },
        Err(e) => panic!("chat subcommand must parse: {e}"),
    }
}
