use total_recall::{
    SessionSummary,
    recall::{build_recall_output, build_recent_rollouts_table},
};

#[test]
fn test_build_recent_rollouts_table_empty() {
    let table = build_recent_rollouts_table(&[], None, 24);
    assert!(table.contains("Recent Rollouts"));
    assert!(table.contains("No recent sessions"));
}

#[test]
fn test_build_recent_rollouts_table_with_sessions() {
    let sessions = vec![SessionSummary {
        session_id: "session_20260914_040914_abc123".to_string(),
        title: "test session".to_string(),
        start_time: "2026-09-14T04:09:14".to_string(),
        end_time: "2026-09-14T05:00:00".to_string(),
        file_size: 95844,
        line_count: 24,
        user_count: 1,
        assistant_count: 6,
        tool_count: 17,
        has_compaction: false,
        directory: None,
        parent_session_id: None,
        child_sessions: vec![],
    }];
    let table = build_recent_rollouts_table(&sessions, Some("session_20260914_040914_abc123"), 24);
    assert!(table.contains("session_20260914_040914_abc123"));
    assert!(table.contains("test session"));
    assert!(table.contains("SUMMARISED"));
}

#[test]
fn test_build_recall_output_structure() {
    let rollouts =
        "## Recent Rollouts (24h)\n\n| session_abc | test | 100KB | 50 | 2026-09-14 | no | - |";
    let output = build_recall_output(
        "## Accomplished\n- Did thing X\n\n## Current Work\nWorking on Y",
        "1. Goal: fix the bug\n2. Steer: use mistral not openrouter\n3. Correction: don't spend money",
        rollouts,
        "## Plan and Todo Files\n\n- .tmp/delegation/item00.md (mtime: 2026-09-14)",
    );

    assert!(output.contains("## Current State"));
    assert!(output.contains("## User Goals, Tasks, and Steers"));
    assert!(output.contains("## Recent Rollouts"));
    assert!(output.contains("## Plan and Todo Files"));
    assert!(output.contains("## Instructions"));
    assert!(output.contains("total-recall"));
    assert!(output.contains("mine the rollouts"));
}

#[test]
fn test_recall_output_has_user_steers() {
    let output = build_recall_output(
        "state summary",
        "1. Goal: fix auth\n2. Steer: use rust not python",
        "## Recent Rollouts (24h)\n\ntable",
        "## Plan and Todo Files\n\nplans",
    );
    assert!(output.contains("fix auth"));
    assert!(output.contains("use rust not python"));
}

#[test]
fn test_recall_output_has_instructions() {
    let output = build_recall_output("s", "g", "t", "p");
    assert!(output.contains("clean todo list"));
    assert!(output.contains("material is dropped"));
    assert!(output.contains("mine the rollouts"));
}

#[test]
fn test_recall_output_ordering_metadata_first_instructions_last() {
    // Deliberate ordering for autoregressive LLMs:
    // 1. Rollouts (metadata) 2. Plans (metadata) 3. State 4. Goals 5. Instructions
    let rollouts = "## Recent Rollouts (24h)\n\nROLLOUT_MARKER";
    let plans = "## Plan and Todo Files\n\nPLAN_MARKER";
    let state = "STATE_MARKER";
    let goals = "GOALS_MARKER";
    let output = build_recall_output(state, goals, rollouts, plans);

    let rollouts_pos = output.find("ROLLOUT_MARKER").unwrap();
    let plan_pos = output.find("PLAN_MARKER").unwrap();
    let state_pos = output.find("STATE_MARKER").unwrap();
    let goals_pos = output.find("GOALS_MARKER").unwrap();
    let instructions_pos = output.find("## Instructions").unwrap();

    // Metadata first
    assert!(rollouts_pos < plan_pos, "rollouts must come before plans");
    assert!(plan_pos < state_pos, "plans must come before state");
    // Substance in the middle
    assert!(state_pos < goals_pos, "state must come before goals");
    // Instructions last (strongest influence on next token)
    assert!(
        goals_pos < instructions_pos,
        "goals must come before instructions"
    );
    assert!(
        instructions_pos < output.len(),
        "instructions must not be truncated"
    );
    // Instructions should be the last section
    let after_instructions = &output[instructions_pos..];
    assert!(
        !after_instructions.contains("ROLLOUT_MARKER"),
        "nothing after instructions"
    );
    assert!(
        !after_instructions.contains("STATE_MARKER"),
        "nothing after instructions"
    );
    assert!(
        !after_instructions.contains("GOALS_MARKER"),
        "nothing after instructions"
    );
}
