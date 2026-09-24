use crate::{
    RolloutAdapter, VibeAdapter,
    rollout::{claude::ClaudeAdapter, codex::CodexAdapter, opencode::OpenCodeAdapter},
};

pub const HARNESS_ENV_VAR: &str = "HARNESS";

/// Maps a harness name to its storage-root override env var (for sandbox errors).
pub fn root_env_var(harness: &str) -> &'static str {
    match harness {
        "vibe" => crate::rollout::VIBE_ROOT_ENV_VAR,
        "codex" => crate::rollout::CODEX_ROOT_ENV_VAR,
        "claude" => crate::rollout::CLAUDE_ROOT_ENV_VAR,
        "opencode" => crate::rollout::OPENCODE_ROOT_ENV_VAR,
        _ => "TOTAL_RECALL_<H>_ROOT",
    }
}

// One match arm + one VALID_HARNESSES entry per harness.
pub const VALID_HARNESSES: [&str; 4] = ["vibe", "codex", "claude", "opencode"];

pub fn valid_harnesses() -> String {
    VALID_HARNESSES.join("|")
}

pub fn resolve_harness(cli_flag: Option<&str>) -> Result<String, String> {
    let value = match cli_flag {
        Some(name) => name.to_string(),
        None => std::env::var(HARNESS_ENV_VAR).map_err(|_| {
            format!(
                "refusing to start MCP server: harness not defined. Set HARNESS=<{}> in the MCP config environment or pass --harness <name>.",
                valid_harnesses()
            )
        })?,
    };
    if !VALID_HARNESSES.contains(&value.as_str()) {
        return Err(format!(
            "refusing to start MCP server: unknown harness '{}'. Valid values: {}. Set HARNESS=<{}> in the MCP config environment or pass --harness <name>.",
            value,
            valid_harnesses(),
            valid_harnesses()
        ));
    }
    Ok(value)
}

/// Resolve a (possibly empty) session id against the adapter's session list.
/// Empty id = most recent session. Returns `None` when no sessions exist.
pub fn resolve_session(adapter: &dyn RolloutAdapter, session_id: &str) -> Option<String> {
    if session_id.is_empty() {
        adapter.most_recent_session_id()
    } else {
        Some(session_id.to_string())
    }
}

pub fn make_adapter(harness: &str) -> Result<Box<dyn RolloutAdapter>, String> {
    let adapter: Box<dyn RolloutAdapter> = match harness {
        "vibe" => Box::new(VibeAdapter::new()),
        "codex" => Box::new(CodexAdapter::new()),
        "claude" => Box::new(ClaudeAdapter::new()),
        "opencode" => Box::new(OpenCodeAdapter::new()),
        other => {
            return Err(format!(
                "unknown harness '{}'. Valid values: {}",
                other,
                valid_harnesses()
            ));
        }
    };
    if crate::rollout::sandbox_enabled() && !adapter.root_is_from_env() {
        return Err(format!(
            "{} is set but {} is not: refusing to read the live store. Point {} at a fixture or scratch copy.",
            crate::rollout::SANDBOX_ENV_VAR,
            root_env_var(harness),
            root_env_var(harness)
        ));
    }
    Ok(adapter)
}
