pub mod bound;
pub mod harness;
pub mod index;
pub mod mcp;
pub mod mercury;
pub mod profile_cache;
pub mod profile_cache_types;
pub mod prompt;
pub mod recall;
pub mod redact;
pub mod report_cap;
pub mod rollout;

pub use mercury::{
    KNOWN_VENDORS, MAX_5XX_RETRIES, MAX_429_RETRIES, MAX_BACKOFF, MAX_CONCURRENCY,
    MAX_INPUT_TOKENS_PER_CALL, MercuryProvider, VENDORS, estimate_tokens, provider_for,
    retry_after_or, unknown_provider_error, vendor_not_compiled_error,
};
pub use prompt::{SYSTEM_PROMPT, build_structured_prompt};
pub use recall::{
    BoundedPrompt, GOALS_SYSTEM_PROMPT, MAX_GOALS_BYTES, MAX_ROLLOUT_ROWS, MAX_STATE_BYTES,
    STATE_SYSTEM_PROMPT, build_goals_prompt, build_goals_prompt_bounded, build_plan_files_section,
    build_recall_output, build_recent_rollouts_table, build_state_prompt,
    build_state_prompt_bounded, filter_recent_sessions,
};
pub use redact::redact_secrets;
pub use rollout::mock::MockAdapter;
pub use rollout::opencode::OpenCodeAdapter;
pub use rollout::vibe::VibeAdapter;
pub use rollout::{
    EventType, InterestingEvent, RolloutAdapter, RolloutMessage, SessionProfile, SessionSummary,
    message_to_text, messages_to_text, summarize_tool_call,
};
