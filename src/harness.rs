use crate::{
    RolloutAdapter, VibeAdapter,
    rollout::{
        claude::ClaudeAdapter, codex::CodexAdapter, cursor::CursorAdapter,
        opencode::OpenCodeAdapter,
    },
};

pub const HARNESS_ENV_VAR: &str = "HARNESS";

// One match arm + one VALID_HARNESSES entry per harness.
pub const VALID_HARNESSES: [&str; 5] = ["vibe", "codex", "claude", "opencode", "cursor"];

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

pub fn make_adapter(harness: &str) -> Result<Box<dyn RolloutAdapter>, String> {
    match harness {
        "vibe" => Ok(Box::new(VibeAdapter::new())),
        "codex" => Ok(Box::new(CodexAdapter::new())),
        "claude" => Ok(Box::new(ClaudeAdapter::new())),
        "opencode" => Ok(Box::new(OpenCodeAdapter::new())),
        "cursor" => Ok(Box::new(CursorAdapter::new())),
        other => Err(format!(
            "unknown harness '{}'. Valid values: {}",
            other,
            valid_harnesses()
        )),
    }
}
