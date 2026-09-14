pub mod harness;
pub mod mcp;
pub mod mercury;
pub mod prompt;
pub mod recall;
pub mod rollout;

pub use mercury::MercuryProvider;
pub use prompt::{SYSTEM_PROMPT, build_structured_prompt};
pub use recall::{
    GOALS_SYSTEM_PROMPT, STATE_SYSTEM_PROMPT, build_goals_prompt, build_plan_files_section,
    build_recent_rollouts_table, build_recall_output, build_state_prompt, filter_recent_sessions,
};
pub use rollout::mock::MockAdapter;
pub use rollout::opencode::OpenCodeAdapter;
pub use rollout::vibe::VibeAdapter;
pub use rollout::{
    EventType, InterestingEvent, RolloutAdapter, RolloutMessage, SessionProfile, SessionSummary,
    messages_to_text, summarize_tool_call,
};
