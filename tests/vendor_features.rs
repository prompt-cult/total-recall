//! The LLM vendor is a compile-time cargo feature (`mercury`, `mistral`), so
//! a vendor-free build (`--no-default-features`) is a first-class artifact:
//! every log-mining tool keeps working and the LLM-backed ones fail with an
//! explicit "not compiled into this build" error instead of a missing key.
//!
//! The binary is always spawned with the vendor key variables removed AND the
//! working directory pointed at an empty scratch dir, so a developer's real
//! `.env` can never be picked up and no request can reach a real API.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use total_recall::mercury::{
    VENDORS, provider_for, unknown_provider_error, vendor_not_compiled_error,
};

const MERCURY_KEY: &str = "INCEPTION_API_KEY";
const MISTRAL_KEY: &str = "MISTRAL_API_KEY";

// --- Feature surface (pure, no I/O) ---

#[test]
fn vendors_list_reflects_the_compiled_features() {
    assert_eq!(
        VENDORS.contains(&"mercury"),
        cfg!(feature = "mercury"),
        "VENDORS must track the `mercury` feature"
    );
    assert_eq!(
        VENDORS.contains(&"mistral"),
        cfg!(feature = "mistral"),
        "VENDORS must track the `mistral` feature"
    );
    if cfg!(feature = "mercury") && cfg!(feature = "mistral") {
        assert_eq!(VENDORS, &["mercury", "mistral"], "default build vendors");
    }
}

#[test]
fn vendor_not_compiled_error_names_the_feature_and_the_rebuild() {
    let msg = vendor_not_compiled_error("mercury");
    assert!(
        msg.contains("not compiled into this build"),
        "must say the vendor is absent: {msg}"
    );
    assert!(
        msg.contains("`mercury`"),
        "must name the cargo feature: {msg}"
    );
    assert!(
        msg.contains("--features mercury"),
        "must name the rebuild command: {msg}"
    );
    assert!(
        msg.contains("no LLM calls") && msg.contains("no API key"),
        "must state what a vendor-free build does: {msg}"
    );
    for vendor in VENDORS {
        assert!(
            msg.contains(vendor),
            "must list the vendors that ARE compiled in: {msg}"
        );
    }
}

#[test]
fn unknown_provider_error_lists_the_compiled_vendors() {
    let msg = unknown_provider_error("bogus");
    assert!(msg.contains("bogus"), "must echo the bad name: {msg}");
    assert!(
        msg.contains("unknown LLM provider"),
        "must name the failure: {msg}"
    );
    assert!(
        msg.contains("--provider"),
        "must name the flag that carries it: {msg}"
    );
    for vendor in VENDORS {
        assert!(
            msg.contains(vendor),
            "must list the compiled vendors: {msg}"
        );
    }
}

/// `MercuryProvider` is not `Debug`, so assert the error by hand rather than
/// through `expect_err`.
fn provider_err(name: Option<&str>) -> String {
    match provider_for(name) {
        Ok(_) => panic!("provider {name:?} must be refused in this build"),
        Err(e) => format!("{e:#}"),
    }
}

#[test]
fn unknown_provider_is_refused() {
    let msg = provider_err(Some("bogus"));
    assert!(msg.contains("bogus"), "{msg}");
    assert!(msg.contains("unknown LLM provider"), "{msg}");
}

#[cfg(not(feature = "mercury"))]
#[test]
fn default_provider_lookup_reports_the_missing_build_not_a_missing_key() {
    let msg = provider_err(None);
    assert!(
        msg.contains("not compiled into this build"),
        "must report the build, not a key: {msg}"
    );
    assert!(
        !msg.contains(MERCURY_KEY),
        "a vendor-free build must not ask for a key: {msg}"
    );
}

#[cfg(not(feature = "mistral"))]
#[test]
fn mistral_lookup_reports_the_missing_feature() {
    let msg = provider_err(Some("mistral"));
    assert!(msg.contains("not compiled into this build"), "{msg}");
    assert!(msg.contains("--features mistral"), "{msg}");
}

// --- Fixture + process helpers ---

/// A working directory for the child that is OUTSIDE the repository tree.
///
/// `dotenvy::dotenv()` walks up parent directories, so a scratch dir inside
/// the repo would find the developer's real `<repo>/.env` and make live vendor
/// API calls. Tests must never do that: this path has no `.env` anywhere above
/// it, and the vendor key variables are removed from the child environment on
/// top of that.
fn child_cwd(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tr_vendor_cwd_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn tmp_root(tag: &str) -> PathBuf {
    let dir = std::env::var("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
        .join(format!("tr_vendor_features_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const SESSION_ID: &str = "51a9645a";

/// A vibe session dir with two user messages and one assistant reply, so the
/// LLM-backed paths get past session resolution and reach provider creation.
fn make_session(root: &Path) {
    let dir = root.join("session_20260915_095955_51a9645a");
    std::fs::create_dir_all(&dir).unwrap();
    let lines = [
        r#"{"role":"user","content":"build the vendor feature gate","timestamp":"2026-09-15T09:59:55Z","injected":false}"#,
        r#"{"role":"assistant","content":"on it","timestamp":"2026-09-15T09:59:56Z","injected":false}"#,
        r#"{"role":"user","content":"keep recall working","timestamp":"2026-09-15T09:59:57Z","injected":false}"#,
    ];
    std::fs::write(dir.join("messages.jsonl"), lines.join("\n") + "\n").unwrap();
    std::fs::write(
        dir.join("meta.json"),
        r#"{"session_id":"uuid-vendor","start_time":"2026-09-15T09:59:55+00:00","end_time":"2026-09-15T10:00:00+00:00","title":"vendor","total_messages":3}"#,
    )
    .unwrap();
}

/// Run the CLI with the vendor keys removed and an empty CWD (so no `.env`).
fn run_cli(tag: &str, root: &Path, args: &[&str]) -> std::process::Output {
    let empty_cwd = child_cwd(tag);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_total-recall"));
    cmd.args(args)
        .current_dir(&empty_cwd)
        .env("TOTAL_RECALL_VIBE_ROOT", root)
        .env_remove(MERCURY_KEY)
        .env_remove(MISTRAL_KEY);
    cmd.output().expect("failed to spawn total-recall binary")
}

// --- Log mining is unaffected by the vendor features (all configurations) ---

#[test]
fn list_and_user_messages_work_without_any_vendor() {
    let root = tmp_root("tools");
    make_session(&root);

    let out = run_cli(
        "list",
        &root,
        &[
            "--harness",
            "vibe",
            "--session",
            SESSION_ID,
            "list",
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "list must work in every build: stderr {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(SESSION_ID),
        "list must report the fixture session: {stdout}"
    );

    let out = run_cli(
        "umsg",
        &root,
        &[
            "--harness",
            "vibe",
            "--session",
            SESSION_ID,
            "user-messages",
        ],
    );
    assert!(
        out.status.success(),
        "user-messages must work in every build: stderr {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("keep recall working"),
        "user-messages must still mine the log: {stdout}"
    );
}

#[test]
fn extract_by_type_works_without_any_vendor() {
    let root = tmp_root("bytype");
    make_session(&root);
    let out = run_cli(
        "bytype",
        &root,
        &[
            "--harness",
            "vibe",
            "--session",
            SESSION_ID,
            "extract-by-type",
            "--type",
            "user",
        ],
    );
    assert!(
        out.status.success(),
        "extract-by-type must work in every build: stderr {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("keep recall working"),
        "must still mine the log: {stdout}"
    );
}

// --- CLI LLM paths: explicit vendor error, never a key error, no network ---

#[cfg(not(feature = "mercury"))]
#[test]
fn vendor_free_recall_explains_the_build() {
    let root = tmp_root("recall");
    make_session(&root);
    let out = run_cli("recall", &root, &["--harness", "vibe", "recall"]);
    assert!(!out.status.success(), "recall must exit non-zero");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not compiled into this build"),
        "must explain the missing feature: {stderr}"
    );
    assert!(
        !stderr.contains(MERCURY_KEY),
        "a vendor-free build must not ask for a key: {stderr}"
    );
}

#[cfg(not(feature = "mercury"))]
#[test]
fn vendor_free_compact_explains_the_build() {
    let root = tmp_root("compact");
    make_session(&root);
    let out = run_cli("compact", &root, &["--harness", "vibe", "compact"]);
    assert!(!out.status.success(), "compact must exit non-zero");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not compiled into this build"),
        "must explain the missing feature: {stderr}"
    );
}

#[cfg(feature = "mercury")]
#[test]
fn default_build_recall_reports_the_missing_key() {
    let root = tmp_root("keyed");
    make_session(&root);
    let out = run_cli("keyed", &root, &["--harness", "vibe", "recall"]);
    assert!(!out.status.success(), "recall must exit non-zero");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(MERCURY_KEY),
        "a vendor build with no key must name the key: {stderr}"
    );
    assert!(
        !stderr.contains("not compiled into this build"),
        "a vendor build must not claim the feature is missing: {stderr}"
    );
}

#[cfg(not(feature = "mistral"))]
#[test]
fn vendor_free_mistral_selection_explains_the_feature() {
    let root = tmp_root("mistral");
    make_session(&root);
    let out = run_cli(
        "mistral",
        &root,
        &["--harness", "vibe", "--provider", "mistral", "recall"],
    );
    assert!(!out.status.success(), "recall must exit non-zero");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--features mistral"),
        "must name the feature to rebuild with: {stderr}"
    );
}

// --- MCP: the tool NAMES stay registered, the calls report the build ---

fn spawn_mcp(
    root: &Path,
) -> (
    std::process::Child,
    std::io::BufReader<std::process::ChildStdout>,
    std::process::ChildStdin,
) {
    let empty_cwd = child_cwd("mcp");
    let mut child = Command::new(env!("CARGO_BIN_EXE_total-recall"))
        .args(["--harness", "vibe", "mcp"])
        .current_dir(&empty_cwd)
        .env("TOTAL_RECALL_VIBE_ROOT", root)
        .env_remove(MERCURY_KEY)
        .env_remove(MISTRAL_KEY)
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
) -> serde_json::Value {
    send(
        stdin,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":{"session_id":SESSION_ID,"hours_back":48,"query":"vendor","words":"vendor"}}}),
    );
    read_id(reader, id)
}

fn tool_names(resp: &serde_json::Value) -> Vec<String> {
    resp.pointer("/result/tools")
        .and_then(|t| t.as_array())
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn llm_tool_names_are_registered_in_every_build() {
    let root = tmp_root("tools_list");
    make_session(&root);
    let (mut child, mut reader, mut stdin) = spawn_mcp(&root);
    init(&mut reader, &mut stdin);

    send(
        &mut stdin,
        &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    );
    let names = tool_names(&read_id(&mut reader, 2));
    child.kill().ok();

    // Clients bind by name: the LLM-backed tools must stay in tools/list
    // whatever the features are, so a vendor-free build degrades on call
    // rather than breaking the client's tool inventory.
    for expected in [
        "compact_session",
        "total_recall",
        "list_sessions",
        "she_said_he_said_action",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "tools/list must advertise {expected}, got {names:?}"
        );
    }
}

#[cfg(not(feature = "mercury"))]
#[test]
fn vendor_free_mcp_call_reports_the_build() {
    let root = tmp_root("mcp_call");
    make_session(&root);
    let (mut child, mut reader, mut stdin) = spawn_mcp(&root);
    init(&mut reader, &mut stdin);

    let resp = call_tool(&mut reader, &mut stdin, 2, "total_recall");
    child.kill().ok();
    let text = serde_json::to_string(&resp).unwrap();
    assert!(
        text.contains("not compiled into this build"),
        "the tool must report the missing feature: {text}"
    );
    assert!(
        resp.pointer("/result/isError") == Some(&serde_json::Value::Bool(true)),
        "it must come back as a tool error, not a protocol error: {text}"
    );
}

#[cfg(feature = "mercury")]
#[test]
fn vendor_build_mcp_call_reports_the_missing_key() {
    let root = tmp_root("mcp_key");
    make_session(&root);
    let (mut child, mut reader, mut stdin) = spawn_mcp(&root);
    init(&mut reader, &mut stdin);

    let resp = call_tool(&mut reader, &mut stdin, 2, "total_recall");
    child.kill().ok();
    let text = serde_json::to_string(&resp).unwrap();
    assert!(
        text.contains(MERCURY_KEY),
        "with no key the tool must name the key: {text}"
    );
}
