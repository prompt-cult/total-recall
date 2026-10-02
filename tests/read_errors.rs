//! Read paths must surface I/O damage, never a session that reads as empty.
//!
//! A rollout payload that is missing, unreadable, or unmappable must produce an
//! error naming the path — the same `read_error` vocabulary the session index
//! already uses. A payload that exists and is empty is not damage and reads as
//! an empty session.

mod common;

use std::path::{Path, PathBuf};

use common::scratch::{child_cwd, scratch};
use total_recall::{
    MockAdapter, OpenCodeAdapter, RolloutAdapter, VibeAdapter,
    rollout::{claude::ClaudeAdapter, codex::CodexAdapter},
};

/// A directory where a JSONL payload should be: `read` fails with EISDIR and
/// `Mmap::map` fails, on any uid and without touching file modes.
fn unreadable(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir.join(name)).unwrap();
}

// --- mock adapter -----------------------------------------------------------

#[test]
fn mock_missing_payload_is_an_error_on_every_read_path() {
    let adapter = MockAdapter::new("/nonexistent/definitely/not/here.jsonl");
    for e in [
        adapter.read_session("x").map(|_| ()),
        adapter.read_session_mmap("x").map(|_| ()),
        adapter.read_session_from_compaction("x").map(|_| ()),
        adapter.read_session_entries("x", true, false).map(|_| ()),
        adapter.profile_session("x").map(|_| ()),
        adapter.profile_session_opts("x", false).map(|_| ()),
        adapter.extract_user_messages("x").map(|_| ()),
    ] {
        let err = e.expect_err("missing mock payload must not read as empty");
        assert!(
            err.contains("here.jsonl"),
            "error must name the payload path, got: {err}"
        );
    }
}

#[test]
fn mock_empty_payload_is_not_an_error() {
    let root = scratch("mock_empty");
    let path = root.join("empty.jsonl");
    std::fs::write(&path, "").unwrap();
    let adapter = MockAdapter::new(&path);
    assert!(
        adapter
            .read_session("x")
            .expect("empty payload is readable")
            .is_empty()
    );
    assert!(
        adapter
            .read_session_mmap("x")
            .expect("empty payload mmaps")
            .is_empty()
    );
}

#[test]
fn mock_list_sessions_surfaces_read_damage() {
    let root = scratch("mock_damage");
    unreadable(&root, "broken.jsonl");
    let adapter = MockAdapter::new(root.join("broken.jsonl"));
    let sessions = adapter.list_sessions();
    assert_eq!(sessions.len(), 1, "the session stays listed");
    assert!(
        sessions[0].read_error.is_some(),
        "damage must be visible on the index entry"
    );
}

// --- vibe adapter -----------------------------------------------------------

#[test]
fn vibe_unreadable_payload_is_an_error_on_every_read_path() {
    let root = scratch("vibe");
    let dir = root.join("session_20260623_114518_2a421f21");
    std::fs::create_dir_all(&dir).unwrap();
    unreadable(&dir, "messages.jsonl");
    let adapter = VibeAdapter::with_root(&*root);
    for e in [
        adapter.read_session("2a421f21").map(|_| ()),
        adapter.read_session_mmap("2a421f21").map(|_| ()),
        adapter.read_session_from_compaction("2a421f21").map(|_| ()),
        adapter
            .read_session_entries("2a421f21", true, false)
            .map(|_| ()),
        adapter.profile_session("2a421f21").map(|_| ()),
        adapter.profile_session_opts("2a421f21", false).map(|_| ()),
        adapter.extract_user_messages("2a421f21").map(|_| ()),
    ] {
        let err = e.expect_err("unreadable vibe payload must not read as empty");
        assert!(
            err.contains("messages.jsonl"),
            "error must name the payload path, got: {err}"
        );
    }
}

#[test]
fn vibe_unknown_session_id_is_an_error() {
    let root = scratch("vibe_unknown");
    let adapter = VibeAdapter::with_root(&*root);
    let err = adapter
        .read_session("nosuchsession")
        .expect_err("unresolvable session id must not read as empty");
    assert!(err.contains("nosuchsession"), "got: {err}");
}

#[test]
fn vibe_reads_a_healthy_payload_unchanged() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rollouts/vibe_sessions");
    let adapter = VibeAdapter::with_root(root);
    assert_eq!(
        adapter
            .read_session("4a0051b6")
            .expect("healthy read")
            .len(),
        6
    );
    assert_eq!(
        adapter
            .read_session_mmap("4a0051b6")
            .expect("healthy mmap read")
            .len(),
        6
    );
    assert_eq!(
        adapter
            .profile_session("4a0051b6")
            .expect("healthy profile")
            .line_count,
        6
    );
}

#[test]
fn vibe_empty_payload_is_not_an_error() {
    let root = scratch("vibe_empty");
    let dir = root.join("session_20260623_114518_2a421f21");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("messages.jsonl"), "").unwrap();
    let adapter = VibeAdapter::with_root(&*root);
    assert!(
        adapter
            .read_session("2a421f21")
            .expect("empty payload is readable")
            .is_empty()
    );
}

// --- codex adapter ----------------------------------------------------------

#[test]
fn codex_unreadable_payload_is_an_error_on_every_read_path() {
    let root = scratch("codex");
    unreadable(&root, "codex_small.jsonl");
    let adapter = CodexAdapter::with_root(&*root);
    for e in [
        adapter.read_session("codex_small").map(|_| ()),
        adapter.read_session_mmap("codex_small").map(|_| ()),
        adapter
            .read_session_from_compaction("codex_small")
            .map(|_| ()),
        adapter
            .read_session_entries("codex_small", true, false)
            .map(|_| ()),
        adapter.profile_session("codex_small").map(|_| ()),
        adapter
            .profile_session_opts("codex_small", false)
            .map(|_| ()),
        adapter.extract_user_messages("codex_small").map(|_| ()),
    ] {
        let err = e.expect_err("unreadable codex payload must not read as empty");
        assert!(
            err.contains("codex_small.jsonl"),
            "error must name the payload path, got: {err}"
        );
    }
}

#[test]
fn codex_list_sessions_surfaces_read_damage() {
    let root = scratch("codex_damage");
    let payload = root.join("codex_small.jsonl");
    std::fs::write(&payload, "{}\n").unwrap();
    std::fs::set_permissions(
        &payload,
        std::os::unix::fs::PermissionsExt::from_mode(0o000),
    )
    .unwrap();
    let adapter = CodexAdapter::with_root(&*root);
    let sessions = adapter.list_sessions();
    assert_eq!(sessions.len(), 1, "the session stays listed");
    let err = sessions[0]
        .read_error
        .as_deref()
        .expect("permission-denied codex payload must surface on the index entry");
    assert!(err.contains("codex_small.jsonl"), "got: {err}");
    std::fs::set_permissions(
        &payload,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
}

#[test]
fn claude_list_sessions_surfaces_read_damage() {
    let root = scratch("claude_damage");
    let payload = root.join("claude_small.jsonl");
    std::fs::write(&payload, "{}\n").unwrap();
    std::fs::set_permissions(
        &payload,
        std::os::unix::fs::PermissionsExt::from_mode(0o000),
    )
    .unwrap();
    let adapter = ClaudeAdapter::with_root(&*root);
    let sessions = adapter.list_sessions();
    assert_eq!(sessions.len(), 1, "the session stays listed");
    let err = sessions[0]
        .read_error
        .as_deref()
        .expect("permission-denied claude payload must surface on the index entry");
    assert!(err.contains("claude_small.jsonl"), "got: {err}");
    std::fs::set_permissions(
        &payload,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
}

#[test]
fn codex_unknown_session_id_is_an_error() {
    let root = scratch("codex_unknown");
    let adapter = CodexAdapter::with_root(&*root);
    let err = adapter
        .read_session("nosuchsession")
        .expect_err("unresolvable session id must not read as empty");
    assert!(err.contains("nosuchsession"), "got: {err}");
}

// --- claude adapter ---------------------------------------------------------

#[test]
fn claude_unreadable_payload_is_an_error_on_every_read_path() {
    let root = scratch("claude");
    // claude discovers sessions as regular `*.jsonl` files, so a directory in
    // that place is not a session at all: the read must still fail loudly
    // rather than come back empty.
    unreadable(&root, "claude_small.jsonl");
    let adapter = ClaudeAdapter::with_root(&*root);
    for e in [
        adapter.read_session("claude_small").map(|_| ()),
        adapter.read_session_mmap("claude_small").map(|_| ()),
        adapter
            .read_session_from_compaction("claude_small")
            .map(|_| ()),
        adapter
            .read_session_entries("claude_small", true, false)
            .map(|_| ()),
        adapter.profile_session("claude_small").map(|_| ()),
        adapter
            .profile_session_opts("claude_small", false)
            .map(|_| ()),
        adapter.extract_user_messages("claude_small").map(|_| ()),
    ] {
        let err = e.expect_err("unreadable claude payload must not read as empty");
        assert!(
            err.contains("claude_small"),
            "error must name the session, got: {err}"
        );
    }
}

/// The permission-denied branch of the payload read. Needs a non-root uid;
/// run as root the chmod is a no-op and the assertions fail loudly rather than
/// passing vacuously.
#[test]
fn claude_permission_denied_payload_names_the_path() {
    let root = scratch("claude_chmod");
    let payload = root.join("claude_small.jsonl");
    std::fs::write(&payload, "{}\n").unwrap();
    std::fs::set_permissions(
        &payload,
        std::os::unix::fs::PermissionsExt::from_mode(0o000),
    )
    .unwrap();
    let adapter = ClaudeAdapter::with_root(&*root);
    for e in [
        adapter.read_session("claude_small"),
        adapter.read_session_mmap("claude_small"),
    ] {
        let err = e.expect_err("permission-denied payload must not read as empty");
        assert!(
            err.contains("claude_small.jsonl"),
            "error must name the payload path, got: {err}"
        );
    }
    std::fs::set_permissions(
        &payload,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
}

#[test]
fn claude_unknown_session_id_is_an_error() {
    let root = scratch("claude_unknown");
    let adapter = ClaudeAdapter::with_root(&*root);
    let err = adapter
        .read_session("nosuchsession")
        .expect_err("unresolvable session id must not read as empty");
    assert!(err.contains("nosuchsession"), "got: {err}");
}

// --- opencode adapter -------------------------------------------------------

fn opencode_fixture(path: &PathBuf, with_session: bool) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session (
            id text PRIMARY KEY, parent_id text, directory text, title text,
            time_created integer NOT NULL, time_updated integer NOT NULL
        );
        CREATE TABLE message (
            id text PRIMARY KEY, session_id text NOT NULL,
            time_created integer NOT NULL, time_updated integer NOT NULL,
            data text NOT NULL
        );
        CREATE TABLE part (
            id text PRIMARY KEY, message_id text NOT NULL, session_id text NOT NULL,
            time_created integer NOT NULL, time_updated integer NOT NULL,
            data text NOT NULL
        );",
    )
    .unwrap();
    if with_session {
        conn.execute(
            "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
             VALUES ('ses_fixture0001aaaa', NULL, '/dev/fixture', 'fixture', 1000, 4000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('msg1', 'ses_fixture0001aaaa', 1000, 1000, '{\"role\":\"user\",\"time\":{\"created\":1000}}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES ('part1', 'msg1', 'ses_fixture0001aaaa', 1000, 1000, '{\"type\":\"text\",\"text\":\"hello\"}')",
            [],
        )
        .unwrap();
    }
}

#[test]
fn opencode_missing_database_is_an_error() {
    let root = scratch("opencode_missing");
    let db = root.join("nope.db");
    let adapter = OpenCodeAdapter::with_root(&db);
    for e in [
        adapter.read_session("ses_x").map(|_| ()),
        adapter.read_session_mmap("ses_x").map(|_| ()),
        adapter.read_session_from_compaction("ses_x").map(|_| ()),
        adapter
            .read_session_entries("ses_x", true, false)
            .map(|_| ()),
        adapter.profile_session("ses_x").map(|_| ()),
        adapter.profile_session_opts("ses_x", false).map(|_| ()),
        adapter.extract_user_messages("ses_x").map(|_| ()),
    ] {
        let err = e.expect_err("missing opencode db must not read as empty");
        assert!(
            err.contains("nope.db"),
            "error must name the db, got: {err}"
        );
    }
}

#[test]
fn opencode_unknown_session_id_is_an_error() {
    let root = scratch("opencode_unknown");
    let db = root.join("opencode.db");
    opencode_fixture(&db, true);
    let adapter = OpenCodeAdapter::with_root(&db);
    let err = adapter
        .read_session("ses_nope")
        .expect_err("unresolvable session id must not read as empty");
    assert!(err.contains("ses_nope"), "got: {err}");
}

#[test]
fn opencode_reads_a_healthy_database_unchanged() {
    let root = scratch("opencode_healthy");
    let db = root.join("opencode.db");
    opencode_fixture(&db, true);
    let adapter = OpenCodeAdapter::with_root(&db);
    assert_eq!(
        adapter
            .read_session("ses_fixture")
            .expect("healthy read")
            .len(),
        1
    );
}

// --- MCP surface ------------------------------------------------------------

/// End-to-end through the real MCP stdio server: a damaged payload must come
/// back as a tool error naming the path, not as an empty envelope.
#[test]
fn mcp_extract_messages_reports_unreadable_payload_as_a_tool_error() {
    use std::io::{BufRead, Write};

    let root = scratch("mcp");
    let dir = root.join("session_20260623_114518_2a421f21");
    std::fs::create_dir_all(&dir).unwrap();
    unreadable(&dir, "messages.jsonl");

    // The child works from an empty directory outside the repo tree, with the
    // vendor keys removed: dotenvy walks up parent directories, so a child
    // left at the crate-root CWD would read the developer's real `.env`.
    let cwd = child_cwd("mcp");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_total-recall"))
        .args(["--harness", "vibe", "mcp"])
        .current_dir(&cwd)
        .env("TOTAL_RECALL_VIBE_ROOT", &*root)
        .env_remove("INCEPTION_API_KEY")
        .env_remove("MISTRAL_API_KEY")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn mcp");
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut stdin = child.stdin.take().unwrap();

    let send = |stdin: &mut std::process::ChildStdin, v: serde_json::Value| {
        writeln!(stdin, "{}", serde_json::to_string(&v).unwrap()).unwrap();
        stdin.flush().unwrap();
    };
    let read_id = |stdout: &mut std::io::BufReader<std::process::ChildStdout>, id: i64| {
        let mut line = String::new();
        loop {
            line.clear();
            let n = stdout.read_line(&mut line).unwrap();
            assert!(n > 0, "server closed stdout before id {id}");
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim())
                && v.get("id").and_then(|i| i.as_i64()) == Some(id)
            {
                return v;
            }
        }
    };

    send(
        &mut stdin,
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}),
    );
    assert!(read_id(&mut stdout, 1).get("result").is_some());
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"extract_messages","arguments":{"session_id":"2a421f21","full":true}}}),
    );
    let resp = read_id(&mut stdout, 2);

    assert_eq!(
        resp.pointer("/result/isError").and_then(|v| v.as_bool()),
        Some(true),
        "damaged payload must be a tool error, not an empty envelope: {resp}"
    );
    let text = resp
        .pointer("/result/content")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    assert!(
        text.contains("messages.jsonl"),
        "tool error must name the payload path, got: {text}"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&cwd);
}
