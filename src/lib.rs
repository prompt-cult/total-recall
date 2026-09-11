pub mod harness;
pub mod mcp;
pub mod mercury;
pub mod prompt;
pub mod rollout;

pub use mercury::MercuryProvider;
pub use prompt::{build_structured_prompt, SYSTEM_PROMPT, STRUCTURED_PROMPT};
pub use rollout::cursor::CursorAdapter;
pub use rollout::mock::MockAdapter;
pub use rollout::opencode::OpenCodeAdapter;
pub use rollout::vibe::VibeAdapter;
pub use rollout::{
    EventType, InterestingEvent, RolloutAdapter, RolloutMessage, SessionProfile, SessionSummary,
    messages_to_text, summarize_tool_call,
};
