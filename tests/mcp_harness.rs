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
