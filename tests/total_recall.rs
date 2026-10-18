//! The rollouts table the recall tools share, and the assembly ordering of
//! `prior_period_summary`. The deleted `total_recall` tool's own output
//! builder is gone with it (#31): `build_prior_period_output` is the one
//! assembly now, so the ordering contract is asserted against it.

use total_recall::{
    SessionListing, SessionSummary,
    recall::{build_prior_period_output, build_recent_rollouts_table},
};

fn summary(session_id: &str) -> SessionSummary {
    SessionSummary {
        session_id: session_id.to_string(),
        title: "t".to_string(),
        start_time: String::new(),
        end_time: String::new(),
        file_size: 1024,
        line_count: 10,
        user_count: 1,
        assistant_count: 1,
        tool_count: 0,
        has_compaction: false,
        directory: None,
        parent_session_id: None,
        child_sessions: Vec::new(),
        has_tantivy_index: false,
        aliases: Vec::new(),
        read_error: None,
    }
}

#[test]
fn test_build_recent_rollouts_table_empty() {
    let table = build_recent_rollouts_table(
        &SessionListing {
            sessions: vec![],
            window_count: 0,
        },
        None,
        24,
    );
    assert!(table.contains("Recent Rollouts"));
    assert!(table.contains("No recent sessions"));
}

#[test]
fn test_build_recent_rollouts_table_with_sessions() {
    let sessions = vec![summary("session_20260914_040914_abc123")];
    let table = build_recent_rollouts_table(
        &SessionListing {
            sessions,
            window_count: 1,
        },
        Some("session_20260914_040914_abc123"),
        24,
    );
    assert!(table.contains("session_20260914_040914_abc123"));
    assert!(table.contains("SUMMARISED"));
}

#[test]
fn test_prior_period_output_contains_every_section() {
    let output = build_prior_period_output(
        "STATE", "GOALS", "ROLLOUTS", "PLANS", "TODOLOG", "COMMITS", "FOOTER",
    );

    assert!(output.contains("## Current State"));
    assert!(output.contains("STATE"));
    assert!(output.contains("## User Goals, Tasks, and Steers"));
    assert!(output.contains("GOALS"));
    assert!(output.contains("ROLLOUTS"));
    assert!(output.contains("PLANS"));
    assert!(output.contains("TODOLOG"));
    assert!(output.contains("COMMITS"));
    assert!(output.contains("FOOTER"));
    assert!(output.contains("## Instructions"));
    assert!(output.contains("take stock and get your bearings"));
}

#[test]
fn test_prior_period_output_ordering_metadata_first_action_last() {
    // Deliberate ordering for autoregressive LLMs:
    // 1. Rollouts (metadata) 2. Plans (metadata) 3. Todo log (ground truth)
    // 4. Commits (ground truth) 5. State 6. Goals 7. Footer 8. Instructions
    // 9. Orientation warning
    let output = build_prior_period_output(
        "STATE_MARKER",
        "GOALS_MARKER",
        "## Recent Rollouts\n\nROLLOUT_MARKER",
        "## Plan and Todo Files\n\nPLAN_MARKER",
        "## Todo Log\n\nTODO_MARKER",
        "## Commits\n\nCOMMIT_MARKER",
        "## Compaction Points\n\nFOOTER_MARKER",
    );

    let pos = |m: &str| output.find(m).expect(m);
    let rollouts_pos = pos("ROLLOUT_MARKER");
    let plan_pos = pos("PLAN_MARKER");
    let todo_pos = pos("TODO_MARKER");
    let commit_pos = pos("COMMIT_MARKER");
    let state_pos = pos("STATE_MARKER");
    let goals_pos = pos("GOALS_MARKER");
    let footer_pos = pos("FOOTER_MARKER");
    let instructions_pos = pos("## Instructions");
    let warning_pos = pos("## Orientation Warning");

    // Metadata first
    assert!(
        rollouts_pos < plan_pos && rollouts_pos < todo_pos && rollouts_pos < state_pos,
        "rollouts must come first"
    );
    assert!(plan_pos < todo_pos, "plans before the todo log");
    // Ground truth before the LLM summaries
    assert!(todo_pos < commit_pos, "todo log before commits");
    assert!(
        commit_pos < state_pos,
        "commits (ground truth) before state (LLM)"
    );
    assert!(state_pos < goals_pos, "state before goals");
    assert!(goals_pos < footer_pos, "goals before the compaction footer");
    assert!(
        footer_pos < instructions_pos && instructions_pos < warning_pos,
        "instructions before the warning, which is last"
    );
    let tail = &output[pos("## Orientation Warning") + "## Orientation Warning".len()..];
    assert!(tail.contains("take stock and get your bearings"));
    assert!(
        tail.trim().len() <= tail.len() && !tail.contains("MARKER"),
        "nothing follows the warning: {tail:?}"
    );
}

#[test]
fn test_recall_output_has_user_steers() {
    let output = build_prior_period_output(
        "state",
        "1. Goal: fix auth\n2. Steer: use rust not python",
        "## Recent Rollouts (24h)\n\ntable",
        "## Plan and Todo Files\n\nplans",
        "## Todo Log\n\ntodos",
        "## Commits\n\ncommits",
        "## Compaction Points\n\nfooter",
    );
    assert!(output.contains("fix auth"));
    assert!(output.contains("use rust not python"));
}

#[test]
fn test_prior_period_output_has_instructions() {
    let output = build_prior_period_output("s", "g", "t", "p", "l", "c", "f");
    assert!(output.contains("clean todo list"));
    assert!(output.contains("material is dropped"));
    assert!(output.contains("mine the rollouts"));
}
