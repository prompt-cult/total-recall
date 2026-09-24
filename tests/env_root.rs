//! Step 0: per-harness TOTAL_RECALL_<H>_ROOT overrides and the sandbox guard.
//! Everything runs against hermetic temp fixtures; no live store is read.

mod common;

use common::{EnvGuard, lock_env};
use std::path::{Path, PathBuf};
use total_recall::harness::make_adapter;
use total_recall::rollout::{
    CLAUDE_ROOT_ENV_VAR, CODEX_ROOT_ENV_VAR, OPENCODE_ROOT_ENV_VAR, SANDBOX_ENV_VAR,
    VIBE_ROOT_ENV_VAR,
};

fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::var("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
        .join(format!("tr_env_root_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp root");
    dir
}

/// Build a minimal vibe session dir so list_sessions finds exactly one entry.
fn make_vibe_session(root: &Path, name: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("messages.jsonl"),
        "{\"role\":\"user\",\"content\":\"hello\",\"timestamp\":\"2026-05-23T20:31:11Z\",\"injected\":false}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("meta.json"),
        "{\"session_id\":\"x\",\"start_time\":\"2026-05-23T20:31:11+00:00\",\"end_time\":\"2026-05-23T20:40:00+00:00\",\"title\":\"t\",\"total_messages\":1}",
    )
    .unwrap();
}

#[test]
fn vibe_env_override_points_make_adapter_at_fixture() {
    let _l = lock_env();
    let root = tmp_dir("vibe");
    make_vibe_session(&root, "session_20260523_203111_aaaa1111");
    let _g = EnvGuard::set(VIBE_ROOT_ENV_VAR, root.to_str().unwrap());
    let adapter = make_adapter("vibe").expect("vibe adapter");
    let sessions = adapter.list_sessions();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "session_20260523_203111_aaaa1111");
}

#[test]
fn empty_override_counts_as_unset() {
    let _l = lock_env();
    let _g = EnvGuard::set(VIBE_ROOT_ENV_VAR, "   ");
    // With the override effectively unset, the adapter falls back to the
    // $HOME-derived root; root_is_from_env reports false.
    let adapter = make_adapter("vibe").expect("vibe adapter");
    assert!(!adapter.root_is_from_env());
}

#[test]
fn sandbox_refuses_when_root_not_from_env() {
    let _l = lock_env();
    let _root = tmp_dir("sandbox");
    let _s = EnvGuard::set(SANDBOX_ENV_VAR, "1");
    let _u = EnvGuard::unset(VIBE_ROOT_ENV_VAR);
    let err = match make_adapter("vibe") {
        Ok(_) => panic!("sandbox must refuse live store"),
        Err(e) => e,
    };
    assert!(
        err.contains(SANDBOX_ENV_VAR),
        "names the sandbox var: {err}"
    );
    assert!(
        err.contains(VIBE_ROOT_ENV_VAR),
        "names the missing override: {err}"
    );
}

#[test]
fn sandbox_allows_when_root_from_env() {
    let _l = lock_env();
    let root = tmp_dir("sandbox_ok");
    make_vibe_session(&root, "session_20260523_203111_bbbb2222");
    let _s = EnvGuard::set(SANDBOX_ENV_VAR, "1");
    let _g = EnvGuard::set(VIBE_ROOT_ENV_VAR, root.to_str().unwrap());
    let adapter = make_adapter("vibe").expect("sandbox passes with override set");
    assert_eq!(adapter.list_sessions().len(), 1);
}

#[test]
fn claude_codex_opencode_overrides_are_wired() {
    let _l = lock_env();
    let croot = tmp_dir("claude");
    let xroot = tmp_dir("codex");
    let oroot = tmp_dir("opencode");
    let _c = EnvGuard::set(CLAUDE_ROOT_ENV_VAR, croot.to_str().unwrap());
    let _x = EnvGuard::set(CODEX_ROOT_ENV_VAR, xroot.to_str().unwrap());
    let _o = EnvGuard::set(OPENCODE_ROOT_ENV_VAR, oroot.to_str().unwrap());
    for h in ["claude", "codex", "opencode"] {
        let adapter = make_adapter(h).unwrap_or_else(|e| panic!("{h}: {e}"));
        assert!(adapter.root_is_from_env(), "{h} root must come from env");
        // Empty fixture roots list zero sessions but must not error/panic.
        assert_eq!(adapter.list_sessions().len(), 0, "{h}");
    }
}
