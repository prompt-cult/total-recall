use crate::rollout::{messages_to_text, RolloutMessage};

pub const SYSTEM_PROMPT: &str =
    "You are a helpful coding assistant that summarizes conversations.";

pub const STRUCTURED_PROMPT: &str = r#"Create a structured summary of this coding conversation. Use these exact sections:

## Accomplished
List what was completed.

## Current Work
What is being worked on now.

## Files Involved
List all files mentioned or modified.

## Next Steps
Clear actions to take.

## Key Decisions/Constraints
Important user preferences, project requirements, or decisions made.

Be precise. Include file paths, function names, and specific details.

--- Conversation ---
{conversation}"#;

/// Build the structured prompt with the conversation text injected.
pub fn build_structured_prompt(messages: &[RolloutMessage]) -> String {
    let conversation = messages_to_text(messages);
    STRUCTURED_PROMPT.replace("{conversation}", &conversation)
}
